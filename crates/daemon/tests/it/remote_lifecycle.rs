//! A box connection across client daemon restarts, and a reconnect that needs
//! a person to sign in. A child of `remote`, whose box and client harness it
//! reuses.

use std::{
    net::Ipv6Addr,
    path::Path,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use bytes::Bytes;
use devlanes_daemon::{Handler, Session};
use devlanes_ipc::Frame;
use devlanes_proto::{
    Context, Control, Method, Request,
    remote::{PROMPT_EVENT, STATE_EVENT},
};
use devlanes_state::Store;
use diesel::{Connection, SqliteConnection, connection::SimpleConnection};
use serde_json::json;
use tokio::net::TcpStream;

use super::{
    BOX, BoxSide, ClientSide, Controls, PASSWORD, SOON, box_port, echo, http_get, ipv6_loopback,
};
use crate::{LIMIT, cx, eventually, repo, temp};

// Port ranges: these tests take 21500-21999, which no other test in this
// binary uses (see `remote_boxes.rs`), and 20000-20999, the start of the
// default pool other tests bind in, only for boxes and clients that bind
// nothing.

/// Whether the store under the client home `home` still wants box `name`.
async fn wanted(home: &Path, name: &str) -> bool {
    let store = Store::open(home.join("state.db")).await.expect("store");
    let hosts = store.remote_hosts().await.expect("hosts");
    store.close().await.expect("close");
    hosts.iter().any(|host| host.name == name && host.wanted)
}

#[tokio::test(flavor = "multi_thread")]
/// A box connected before a client daemon restart is connected again by the
/// next daemon, as a box that connected before, without any `connect`, and
/// mirrors its ports again. `disconnect` makes it unwanted.
async fn a_restart_restores_wanted_boxes() {
    if !ipv6_loopback().await {
        eprintln!("skipping a_restart_restores_wanted_boxes: no IPv6 loopback");
        return;
    }
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "boxrepo", BOX);
        let side = BoxSide::start(&tmp, "box", "21500-21624").await;
        side.h
            .call(Method::Start, json!({"units": ["echo"]}), cx(&main))
            .await
            .expect("box start");
        let echo_port = box_port(&side.h, 0, "echo").await;

        let client = ClientSide::start(&tmp, "client", "21625-21749", &side).await;
        client.connect("box").await;
        let mirrored = eventually(SOON, || async {
            client.mirrored().await.contains(&echo_port).then_some(())
        })
        .await;
        assert!(mirrored.is_some(), "echo mirrored before the restart");
        assert_eq!(
            *client.controls.handed_out.lock().expect("handed out"),
            [("box".to_owned(), false)]
        );
        client.stop().await;

        let client = ClientSide::start(&tmp, "client", "21625-21749", &side).await;
        let restored = eventually(SOON, || async {
            (client.host().await["state"] == "connected" && echo(echo_port).await).then_some(())
        })
        .await;
        assert!(
            restored.is_some(),
            "the restarted daemon reconnected the box: {}",
            client.host().await
        );
        assert_eq!(
            *client.controls.handed_out.lock().expect("handed out"),
            [("box".to_owned(), true)],
            "a restored box counts as connected before"
        );

        client
            .h
            .call(
                Method::Disconnect,
                json!({"host": "box"}),
                Context::default(),
            )
            .await
            .expect("disconnect");
        let home = client.h.home.clone();
        client.stop().await;
        assert!(!wanted(&home, "box").await, "disconnect clears wanted");

        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// Of two connected boxes, the one disconnected before a client daemon
/// restart stays disconnected, and the other is restored. The restore opens
/// wanted boxes in name order, so `box` would be handed out before `zbox`.
async fn a_restart_leaves_a_disconnected_box_alone() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let first = BoxSide::start(&tmp, "box", "20250-20374").await;
        let second = BoxSide::start(&tmp, "zbox", "20375-20499").await;
        let boxes = [("box", &first), ("zbox", &second)];
        let client = ClientSide::start_with(&tmp, "client", "20500-20624", &boxes, None).await;
        client.connect("box").await;
        client.connect("zbox").await;
        client
            .h
            .call(
                Method::Disconnect,
                json!({"host": "box"}),
                Context::default(),
            )
            .await
            .expect("disconnect");
        client.stop().await;

        let client = ClientSide::start_with(&tmp, "client", "20500-20624", &boxes, None).await;
        let restored = eventually(SOON, || async {
            (client.host_named("zbox").await["state"] == "connected").then_some(())
        })
        .await;
        assert!(
            restored.is_some(),
            "zbox is restored: {}",
            client.host_named("zbox").await
        );
        assert_eq!(
            *client.controls.handed_out.lock().expect("handed out"),
            [("zbox".to_owned(), true)],
            "only zbox is opened"
        );
        let host = client.host().await;
        assert_eq!(host["state"], "disconnected", "{host}");

        client.stop().await;
        first.stop().await;
        second.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// `connect --background` answers only once the connection is recorded, so a
/// daemon stopped right after it restores the box. A write lock on the
/// client's database holds the record back while the box is connected.
async fn a_background_connect_waits_for_the_record() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start(&tmp, "box", "20625-20749").await;
        let client = ClientSide::start(&tmp, "client", "20750-20874", &side).await;
        let db = client.h.home.join("state.db");
        let mut writer =
            SqliteConnection::establish(db.to_str().expect("path")).expect("connection");
        writer
            .batch_execute("PRAGMA busy_timeout = 5000; BEGIN IMMEDIATE;")
            .expect("write lock");

        let (session, _frames) = Session::local(9_304);
        let engine = client.h.engine.clone();
        let params = client.connect_params("box").await;
        let connecting = tokio::spawn(async move {
            let request = Request {
                id: 1,
                method: Method::Connect,
                params,
                context: Context::default(),
            };
            engine.handle(request, session).await
        });
        // The box lists the client once its session is up; the client's
        // record of it is what waits on the lock.
        let served = eventually(SOON, || async {
            let status = side
                .h
                .call(Method::Status, json!({"all": true}), Context::default())
                .await
                .ok()?;
            status["clients"]
                .as_array()
                .is_some_and(|c| !c.is_empty())
                .then_some(())
        })
        .await;
        assert!(served.is_some(), "the box serves the client");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !connecting.is_finished(),
            "connect answered before the connection was recorded"
        );
        writer.batch_execute("COMMIT;").expect("release");
        drop(writer);
        let connected = connecting.await.expect("connect task").expect("connect");
        assert_eq!(connected["state"], "connected", "{connected}");
        client.stop().await;

        let client = ClientSide::start(&tmp, "client", "20750-20874", &side).await;
        let restored = eventually(SOON, || async {
            (client.host().await["state"] == "connected").then_some(())
        })
        .await;
        assert!(restored.is_some(), "restored: {}", client.host().await);
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A reconnect whose prompt has no CLI to answer it waits for a sign-in:
/// `remote ls` and `status` show `needs_auth` with the command to run, the
/// listeners stay and the route serves the sign-in page. `remote connect`
/// with a CLI that answers resumes the link.
async fn an_unanswered_prompt_waits_for_sign_in() {
    if !ipv6_loopback().await {
        eprintln!("skipping an_unanswered_prompt_waits_for_sign_in: no IPv6 loopback");
        return;
    }
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "boxrepo", BOX);
        let side = BoxSide::start(&tmp, "box", "21750-21874").await;
        side.h
            .call(Method::Start, json!({"units": ["web", "echo"]}), cx(&main))
            .await
            .expect("box start");
        let echo_port = box_port(&side.h, 0, "echo").await;
        let client = ClientSide::start(&tmp, "client", "21875-21999", &side).await;
        client.connect("box").await;
        let served = eventually(SOON, || async {
            http_get(client.proxy_port, "web.boxapp.localhost")
                .await
                .filter(|response| response.contains("hello-from-box"))
        })
        .await;
        assert!(served.is_some(), "web is routed to the box");
        // Bound before the link drops, so the check below proves it survives.
        let mirrored = eventually(SOON, || async {
            client.mirrored().await.contains(&echo_port).then_some(())
        })
        .await;
        assert!(mirrored.is_some(), "echo mirrored before the sign-in");

        // The box now asks for a password, and no CLI is attached to answer.
        client.controls.sign_in.store(true, Ordering::SeqCst);
        side.transport.last_link().expect("link").sever();
        let host = eventually(SOON, || async {
            let host = client.host().await;
            (host["state"] == "needs_auth").then_some(host)
        })
        .await;
        let host = match host {
            Some(host) => host,
            None => panic!("the link needs a sign-in: {}", client.host().await),
        };
        assert_eq!(host["fix"], "devlanes remote connect box", "{host}");
        let error = host["error"].as_str().unwrap_or_default();
        assert!(error.contains("Permission denied"), "{host}");
        let status = client
            .h
            .call(Method::Status, json!({"all": true}), Context::default())
            .await
            .expect("status");
        assert_eq!(status["hosts"][0]["state"], "needs_auth", "{status}");
        assert_eq!(
            status["hosts"][0]["fix"], "devlanes remote connect box",
            "{status}"
        );

        assert!(
            TcpStream::connect((Ipv6Addr::LOCALHOST, echo_port))
                .await
                .is_ok(),
            "the listener stays while the box waits for a sign-in"
        );
        let route = eventually(SOON, || async {
            let state = client.route_state("web.boxapp.localhost").await;
            (state.as_deref() == Some("sign_in_needed")).then_some(())
        })
        .await;
        assert!(route.is_some(), "the route waits for a sign-in");
        let page = http_get(client.proxy_port, "web.boxapp.localhost")
            .await
            .unwrap_or_default();
        assert!(page.contains("box-sign-in-needed"), "{page}");
        assert!(page.contains("devlanes remote connect box"), "{page}");

        // `remote connect` with a CLI attached: its prompt is answered and the
        // link connects again.
        let (session, mut frames) = Session::local(9_301);
        let engine = client.h.engine.clone();
        let attached = session.clone();
        let mut connecting = tokio::spawn(async move {
            let request = Request {
                id: 1,
                method: Method::Connect,
                params: json!({"host": "box", "background": true}),
                context: Context::default(),
            };
            engine.handle(request, attached).await
        });
        let engine = client.h.engine.clone();
        let mut prompts = 0;
        let connected = loop {
            tokio::select! {
                result = &mut connecting => break result.expect("connect task"),
                frame = frames.recv() => match frame {
                    None => break (&mut connecting).await.expect("connect task"),
                    Some(Frame::Control(Control::Event(event))) if event.event == PROMPT_EVENT => {
                        prompts += 1;
                        let stream = event.data["stream_id"].as_u64().expect("stream id");
                        engine
                            .input(stream, Bytes::from_static(PASSWORD.as_bytes()), session.clone())
                            .await
                            .expect("answer");
                    }
                    Some(_) => {}
                },
            }
        };
        let connected = connected.expect("connect resumes the link");
        assert_eq!(connected["state"], "connected", "{connected}");
        assert_eq!(prompts, 1, "one prompt for the resumed attempt");
        let back = eventually(SOON, || async {
            http_get(client.proxy_port, "web.boxapp.localhost")
                .await
                .filter(|response| response.contains("hello-from-box"))
        })
        .await;
        assert!(back.is_some(), "web is served again");
        assert!(echo(echo_port).await, "the mirrored port works again");

        session.close();
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A `disconnect` while the restarted daemon's restore is about to open the
/// box wins: the box ends up closed and unwanted, whichever of the two gets
/// to it first.
async fn a_disconnect_during_the_restore_wins() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start(&tmp, "box", "20000-20124").await;
        let client = ClientSide::start(&tmp, "client", "20125-20249", &side).await;
        client.connect("box").await;
        client.stop().await;

        let controls = Arc::new(Controls::default());
        controls.open.arm();
        let client =
            ClientSide::start_controlled(&tmp, "client", "20125-20249", &side, controls.clone())
                .await;
        controls.open.reached().await;
        let (session, _frames) = Session::local(9_302);
        let engine = client.h.engine.clone();
        let disconnecting = tokio::spawn(async move {
            let request = Request {
                id: 1,
                method: Method::Disconnect,
                params: json!({"host": "box"}),
                context: Context::default(),
            };
            engine.handle(request, session).await
        });
        // Time for a disconnect that does not wait for the restore to run
        // first; the outcome must be the same either way.
        tokio::time::sleep(Duration::from_millis(200)).await;
        controls.open.release();
        let disconnected = disconnecting
            .await
            .expect("disconnect task")
            .expect("disconnect");
        assert_eq!(
            disconnected["disconnected"],
            json!(["box"]),
            "{disconnected}"
        );
        let host = client.host().await;
        assert_eq!(host["state"], "disconnected", "{host}");

        let home = client.h.home.clone();
        client.stop().await;
        assert!(!wanted(&home, "box").await, "the box stays disconnected");
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// `remote connect` right after a restart, while the restore's attempt is
/// still under way after asking its prompt with nobody attached: that attempt
/// needs a sign-in, and `connect` resumes the link once, so the next attempt
/// asks this CLI and connects.
async fn a_connect_during_the_restore_answers_the_next_prompt() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start(&tmp, "box", "20875-20937").await;
        let client = ClientSide::start(&tmp, "client", "20938-20999", &side).await;
        client.connect("box").await;
        client.stop().await;

        let controls = Arc::new(Controls::default());
        controls.sign_in.store(true, Ordering::SeqCst);
        controls.unanswered.arm();
        let client =
            ClientSide::start_controlled(&tmp, "client", "20938-20999", &side, controls.clone())
                .await;
        controls.unanswered.reached().await;

        let (session, mut frames) = Session::local(9_303);
        let engine = client.h.engine.clone();
        let attached = session.clone();
        let mut connecting = tokio::spawn(async move {
            let request = Request {
                id: 1,
                method: Method::Connect,
                params: json!({"host": "box"}),
                context: Context::default(),
            };
            engine.handle(request, attached).await
        });
        // The CLI is registered once it gets its first state, so the held
        // attempt may fail now.
        let first = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match frames.recv().await {
                    Some(Frame::Control(Control::Event(event))) if event.event == STATE_EVENT => {
                        break event.data["state"].as_str().map(str::to_owned);
                    }
                    Some(_) => {}
                    None => break None,
                }
            }
        })
        .await;
        let Ok(Some(first)) = first else {
            panic!(
                "connect sent no first state within 10 s (ended: {})",
                connecting.is_finished()
            );
        };
        controls.unanswered.release();
        let engine = client.h.engine.clone();
        let mut prompts = 0;
        let mut states = vec![first];
        loop {
            tokio::select! {
                result = &mut connecting => {
                    panic!("connect ended early: {result:?}, states {states:?}");
                }
                frame = frames.recv() => match frame {
                    None => panic!("the session closed, states {states:?}"),
                    Some(Frame::Control(Control::Event(event))) if event.event == PROMPT_EVENT => {
                        prompts += 1;
                        let stream = event.data["stream_id"].as_u64().expect("stream id");
                        engine
                            .input(stream, Bytes::from_static(PASSWORD.as_bytes()), session.clone())
                            .await
                            .expect("answer");
                    }
                    Some(Frame::Control(Control::Event(event))) if event.event == STATE_EVENT => {
                        let state = event.data["state"].as_str().unwrap_or_default().to_owned();
                        states.push(state.clone());
                        if state == "connected" {
                            break;
                        }
                    }
                    Some(_) => {}
                },
            }
        }
        assert_eq!(prompts, 1, "the resumed attempt asks once: {states:?}");
        assert_eq!(client.host().await["state"], "connected");

        session.close();
        let detached = connecting.await.expect("connect task");
        assert!(detached.is_ok(), "{detached:?}");
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A box the connector reaches by a command is recorded with that transport:
/// `remote ls` shows it while connected and after `disconnect`, and a
/// restarted daemon restores it through the connector again.
async fn command_boxes_keep_their_transport() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start(&tmp, "box", "20000-20124").await;
        let command = || {
            let controls = Arc::new(Controls::default());
            controls
                .commands
                .lock()
                .expect("commands")
                .insert("box".to_owned());
            controls
        };
        let client =
            ClientSide::start_controlled(&tmp, "client", "20125-20249", &side, command()).await;
        client.connect("box").await;
        let host = client.host().await;
        assert_eq!(host["transport"], "command", "{host}");
        client.stop().await;

        let client =
            ClientSide::start_controlled(&tmp, "client", "20125-20249", &side, command()).await;
        let restored = eventually(SOON, || async {
            (client.host().await["state"] == "connected").then_some(())
        })
        .await;
        assert!(restored.is_some(), "restored: {}", client.host().await);
        assert_eq!(
            *client.controls.handed_out.lock().expect("handed out"),
            [("box".to_owned(), true)]
        );
        client
            .h
            .call(
                Method::Disconnect,
                json!({"host": "box"}),
                Context::default(),
            )
            .await
            .expect("disconnect");
        let host = client.host().await;
        assert_eq!(
            (&host["state"], &host["transport"]),
            (&json!("disconnected"), &json!("command")),
            "{host}"
        );
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A box recorded as reached by a command is not restored over ssh once the
/// connector has no command for it: `remote ls` shows it failed with the
/// reason, nothing connects, and `disconnect` forgets it.
async fn a_restored_command_box_without_its_command_fails() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start(&tmp, "box", "20000-20124").await;
        let controls = Arc::new(Controls::default());
        controls
            .commands
            .lock()
            .expect("commands")
            .insert("box".to_owned());
        let client =
            ClientSide::start_controlled(&tmp, "client", "20125-20249", &side, controls).await;
        client.connect("box").await;
        client.stop().await;

        let client =
            ClientSide::start_controlled(&tmp, "client", "20125-20249", &side, Arc::default())
                .await;
        let failed = eventually(SOON, || async {
            let host = client.host().await;
            (host["state"] == "failed").then_some(host)
        })
        .await;
        let Some(host) = failed else {
            panic!("the restore did not fail: {}", client.host().await);
        };
        assert_eq!(host["transport"], "command", "{host}");
        let error = host["error"].as_str().unwrap_or_default();
        assert!(error.contains("remote.boxes.box.command"), "{host}");
        assert_eq!(
            *client.controls.handed_out.lock().expect("handed out"),
            [("box".to_owned(), true)]
        );
        // The failure stays; no link retries in the background.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(client.host().await["state"], "failed");

        client
            .h
            .call(
                Method::Disconnect,
                json!({"host": "box"}),
                Context::default(),
            )
            .await
            .expect("disconnect");
        let host = client.host().await;
        assert_eq!(host["state"], "disconnected", "{host}");
        assert!(host.get("error").is_none(), "{host}");
        let home = client.h.home.clone();
        client.stop().await;
        assert!(!wanted(&home, "box").await, "disconnect clears wanted");
        side.stop().await;
    })
    .await
    .expect("timeout");
}
