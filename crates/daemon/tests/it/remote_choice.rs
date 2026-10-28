//! Choosing which box serves a lane several boxes run (`remote.use`). A child
//! of `remote`, whose box and client harness it reuses.

use std::{collections::BTreeSet, net::Ipv6Addr, time::Duration};

use devlanes_daemon::{Handler, Session};
use devlanes_ipc::Frame;
use devlanes_proto::{Context, Control, Method, Request};
use devlanes_state::Store;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    task::JoinHandle,
};

use super::{BOX, BoxSide, ClientSide, SOON, http_get, ipv6_loopback};
use crate::{LIMIT, cx, eventually, repo, temp};

// Port ranges: 13500-14999, below the DNS server's 15353 and the other remote
// tests, which no other test in this binary uses. Each test takes its own part.

/// Project `forkapp` as each fork of one image runs it: `web` and `docs`
/// answer with the fork's name, and `echo` takes the literal port `echo`. It
/// echoes on the fork that `serves_echo` and binds nothing on the other,
/// since both forks share this machine.
fn fork_project(fork: &str, echo: u16, serves_echo: bool) -> String {
    let run = if serves_echo { "tcp-echo" } else { "sleep" };
    format!(
        "project: forkapp\nservices:\n  web:\n    run: \"http web-from-{fork}\"\n    hosts: [\"web.forkapp.localhost\"]\n  docs:\n    run: \"http docs-from-{fork}\"\n    hosts: [\"docs.forkapp.localhost\"]\n  echo:\n    run: {run}\n    env:\n      PORT: \"{echo}\"\n"
    )
}

/// The hostnames `fork_project` routes.
const FORK_HOSTS: [&str; 2] = ["web.forkapp.localhost", "docs.forkapp.localhost"];

/// Subscribes to the client's events, as an attached CLI would.
fn subscribe(client: &ClientSide, id: u64) -> (Session, mpsc::Receiver<Frame>, JoinHandle<()>) {
    let (session, frames) = Session::local(id);
    let request = Request {
        id: 1,
        method: Method::Subscribe,
        params: json!({"follow": true}),
        context: Context::default(),
    };
    let engine = client.h.engine.clone();
    let watching = session.clone();
    let task = tokio::spawn(async move {
        let _ = engine.handle(request, watching).await;
    });
    (session, frames, task)
}

/// The `remote.warning` events received within a short while.
async fn warnings(frames: &mut mpsc::Receiver<Frame>) -> Vec<Value> {
    let mut found = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(frame) = frames.recv().await {
            if let Frame::Control(Control::Event(event)) = frame
                && event.event == "remote.warning"
            {
                found.push(event.data);
            }
        }
    })
    .await;
    found
}

/// `remote.use` for `lane` with `host` (null for this machine's lane).
async fn use_lane(client: &ClientSide, lane: &str, host: Option<&str>) -> Result<Value, String> {
    client
        .h
        .call(
            Method::RemoteUse,
            json!({"lane": lane, "host": host}),
            Context::default(),
        )
        .await
        .map_err(|error| {
            assert!(
                error.data["message"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "{error:?} has a message"
            );
            error.code
        })
}

/// The entry for `lane` among the `skipped_lanes` of box `host`.
async fn skipped(client: &ClientSide, host: &str, lane: &str) -> Option<Value> {
    client.host_named(host).await["skipped_lanes"]
        .as_array()?
        .iter()
        .find(|entry| entry["lane"] == lane)
        .cloned()
}

/// The ports the routes of box `host` point at, where the proxy reaches it.
async fn route_ports(client: &ClientSide, host: &str) -> BTreeSet<u16> {
    client.host_named(host).await["routes"]
        .as_array()
        .map(|routes| {
            routes
                .iter()
                .filter_map(|route| u16::try_from(route["port"].as_u64()?).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether box `host` holds `port` on at least one address.
async fn binds(client: &ClientSide, host: &str, port: u16) -> bool {
    client.host_named(host).await["ports"]
        .as_array()
        .is_some_and(|ports| {
            ports
                .iter()
                .any(|entry| entry["port"] == port && entry["addresses"] != json!([]))
        })
}

/// Whether a GET of `host` through the client's proxy answers with `body`.
async fn answers(client: &ClientSide, host: &str, body: &str) -> bool {
    http_get(client.proxy_port, host)
        .await
        .is_some_and(|response| response.contains(body))
}

#[tokio::test(flavor = "multi_thread")]
/// `remote use` moves a lane two boxes run from the box connected first to
/// the other. It replies once the routes and the shared port moved, the old
/// box's open connection is closed, no published route table ever has the
/// lane's hostnames on both boxes, and no port is reported in use. Each
/// refusal has its code, and `remote disconnect` deletes the choice.
async fn remote_use_switches_a_lane_between_boxes() {
    if !ipv6_loopback().await {
        eprintln!("skipping remote_use_switches_a_lane_between_boxes: no IPv6 loopback");
        return;
    }
    const ECHO: u16 = 14_490;
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let alpha_repo = repo(&tmp.path, "forkalpha", &fork_project("alpha", ECHO, true));
        let beta_repo = repo(&tmp.path, "forkbeta", &fork_project("beta", ECHO, false));
        let alpha = BoxSide::start(&tmp, "alpha", "14000-14199").await;
        let beta = BoxSide::start(&tmp, "beta", "14200-14399").await;
        for (side, repo) in [(&alpha, &alpha_repo), (&beta, &beta_repo)] {
            side.h
                .call(Method::Start, json!({}), cx(repo))
                .await
                .expect("box start");
        }
        let client = ClientSide::start_with(
            &tmp,
            "client",
            "14400-14489",
            &[("alpha", &alpha), ("beta", &beta)],
            None,
        )
        .await;
        let (session, mut frames, events) = subscribe(&client, 9_201);
        client.connect("alpha").await;
        let routed = eventually(SOON, || async {
            answers(&client, FORK_HOSTS[0], "web-from-alpha")
                .await
                .then_some(())
        })
        .await;
        assert!(routed.is_some(), "alpha's lane is mirrored");
        client.connect("beta").await;
        let skip = eventually(SOON, || skipped(&client, "beta", "forkapp@0")).await;
        let skip = skip.expect("beta's lane is skipped");
        assert_eq!(
            (&skip["cause"], &skip["winner"], &skip["can_use"]),
            (&json!("box_earlier"), &json!("alpha"), &json!(true)),
            "{skip}"
        );
        let bound = eventually(SOON, || async {
            binds(&client, "alpha", ECHO).await.then_some(())
        })
        .await;
        assert!(bound.is_some(), "{}", client.host_named("alpha").await);

        // A connection through alpha's mirror, open across the switch.
        let mut open = TcpStream::connect((Ipv6Addr::LOCALHOST, ECHO))
            .await
            .expect("connect to the mirrored echo");
        open.write_all(b"ping\n").await.expect("write");
        let mut buffer = [0u8; 5];
        open.read_exact(&mut buffer)
            .await
            .expect("echo through alpha");
        assert_eq!(&buffer, b"ping\n");

        for (lane, host, code) in [
            ("forkapp@0", Some("gamma"), "box_unknown"),
            ("forkapp@7", Some("beta"), "lane_not_on_box"),
            ("forkapp@0", None, "local_lane_unknown"),
        ] {
            assert_eq!(use_lane(&client, lane, host).await, Err(code.to_owned()));
        }

        let alpha_ports = route_ports(&client, "alpha").await;
        assert_eq!(alpha_ports.len(), 2, "{}", client.host_named("alpha").await);
        client.published.lock().expect("published").clear();
        let reply = use_lane(&client, "forkapp@0", Some("beta")).await;
        assert_eq!(
            reply,
            Ok(json!({"lane": "forkapp@0", "host": "beta", "previous": "alpha", "mirrored": true}))
        );
        // Applied by the time it replies.
        for (host, body) in FORK_HOSTS.iter().zip(["web-from-beta", "docs-from-beta"]) {
            assert!(answers(&client, host, body).await, "{host} reaches beta");
        }
        assert!(
            binds(&client, "beta", ECHO).await,
            "{}",
            client.host_named("beta").await
        );
        assert!(
            !binds(&client, "alpha", ECHO).await,
            "{}",
            client.host_named("alpha").await
        );
        let closed = tokio::time::timeout(Duration::from_secs(5), open.read(&mut buffer)).await;
        assert!(
            matches!(closed, Ok(Ok(0) | Err(_))),
            "the connection through alpha is closed: {closed:?}"
        );

        // Routes reach each box through a relay on an ephemeral port, which
        // tells the boxes apart unless the system handed beta a port alpha
        // just closed.
        let beta_ports = route_ports(&client, "beta").await;
        assert_eq!(beta_ports.len(), 2, "{}", client.host_named("beta").await);
        let tables = client.published.lock().expect("published").clone();
        if alpha_ports.is_disjoint(&beta_ports) {
            let mut beta_seen = false;
            for table in &tables {
                let ports: BTreeSet<u16> = table
                    .iter()
                    .filter(|(host, _)| FORK_HOSTS.contains(&host.as_str()))
                    .map(|(_, port)| *port)
                    .collect();
                let on_alpha = !ports.is_disjoint(&alpha_ports);
                let on_beta = !ports.is_disjoint(&beta_ports);
                assert!(
                    !(on_alpha && on_beta),
                    "both boxes in one table: {tables:?}"
                );
                assert!(!(beta_seen && on_alpha), "alpha came back: {tables:?}");
                beta_seen |= on_beta;
            }
            assert!(beta_seen, "{tables:?}");
        } else {
            eprintln!("beta got a relay port alpha had, so the route tables are not compared");
        }

        let host = client.host_named("beta").await;
        assert_eq!(host["chosen_lanes"], json!(["forkapp@0"]), "{host}");
        let skip = skipped(&client, "alpha", "forkapp@0")
            .await
            .expect("alpha skips");
        assert_eq!(
            (&skip["cause"], &skip["winner"], &skip["reason"]),
            (
                &json!("box_chosen"),
                &json!("beta"),
                &json!("beta's forkapp@0 is in use")
            ),
            "{skip}"
        );
        assert_eq!(client.host_named("alpha").await["chosen_lanes"], json!([]));

        // Back to alpha, which connected first, so its mirror is woken before
        // beta's and reaches the shared port while beta still holds it.
        let reply = use_lane(&client, "forkapp@0", Some("alpha")).await;
        assert_eq!(
            reply,
            Ok(json!({"lane": "forkapp@0", "host": "alpha", "previous": "beta", "mirrored": true}))
        );
        assert!(answers(&client, FORK_HOSTS[0], "web-from-alpha").await);
        assert!(
            binds(&client, "alpha", ECHO).await,
            "{}",
            client.host_named("alpha").await
        );
        let reply = use_lane(&client, "forkapp@0", Some("beta")).await;
        assert_eq!(
            reply.map(|reply| reply["previous"].clone()),
            Ok(json!("alpha"))
        );
        assert!(
            binds(&client, "beta", ECHO).await,
            "{}",
            client.host_named("beta").await
        );

        let seen = warnings(&mut frames).await;
        assert!(
            !seen.iter().any(|w| w["code"] == "port_in_use"),
            "no port is reported in use: {seen:?}"
        );
        assert!(
            seen.iter().any(|w| w["host"] == "alpha"
                && w["code"] == "lane_skipped"
                && w["cause"] == "box_chosen"
                && w["box"] == "beta"),
            "{seen:?}"
        );

        // A disconnect deletes beta's choice: alpha serves again, and beta,
        // connected later, stays skipped once it is back.
        super::boxes::disconnect(&client, "beta").await;
        assert_eq!(
            use_lane(&client, "forkapp@0", Some("beta")).await,
            Err("box_not_connected".to_owned())
        );
        let back = eventually(SOON, || async {
            answers(&client, FORK_HOSTS[0], "web-from-alpha")
                .await
                .then_some(())
        })
        .await;
        assert!(back.is_some(), "alpha serves the lane again");
        assert_eq!(client.host_named("beta").await["chosen_lanes"], json!([]));
        client.connect("beta").await;
        let skip = eventually(SOON, || skipped(&client, "beta", "forkapp@0")).await;
        assert_eq!(
            skip.map(|skip| skip["cause"].clone()),
            Some(json!("box_earlier"))
        );

        session.close();
        let _ = events.await;
        client.stop().await;
        alpha.stop().await;
        beta.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A choice beats a stopped local lane: the box lane is mirrored and the
/// local lane is listed as yielded, until `remote use --local` gives the
/// hostnames back. Starting the local lane deletes a choice with
/// `choice_cleared` and takes the hostnames back, and while it runs a choice
/// is refused with `local_lane_running`.
async fn a_choice_and_this_machines_own_lane() {
    if !ipv6_loopback().await {
        eprintln!("skipping a_choice_and_this_machines_own_lane: no IPv6 loopback");
        return;
    }
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let box_repo = repo(&tmp.path, "boxrepo", BOX);
        let side = BoxSide::start(&tmp, "box", "13500-13749").await;
        side.h
            .call(Method::Start, json!({"units": ["web"]}), cx(&box_repo))
            .await
            .expect("box start");
        let local_repo = repo(
            &tmp.path,
            "localrepo",
            &BOX.replace("hello-from-box", "hello-from-local"),
        );
        let client = ClientSide::start(&tmp, "client", "13750-13999", &side).await;
        client
            .h
            .call(Method::LaneList, json!({}), cx(&local_repo))
            .await
            .expect("local lane list");
        let (session, mut frames, events) = subscribe(&client, 9_202);
        client.connect("box").await;
        let skip = eventually(SOON, || skipped(&client, "box", "boxapp@0")).await;
        let skip = skip.expect("the box lane is skipped");
        assert_eq!(
            (&skip["cause"], &skip["winner"], &skip["can_use"]),
            (&json!("local_stopped"), &Value::Null, &json!(true)),
            "{skip}"
        );
        assert!(!answers(&client, "web.boxapp.localhost", "hello-from-box").await);

        let reply = use_lane(&client, "boxapp@0", Some("box")).await;
        assert_eq!(
            reply,
            Ok(json!({"lane": "boxapp@0", "host": "box", "previous": null, "mirrored": true}))
        );
        assert!(answers(&client, "web.boxapp.localhost", "hello-from-box").await);
        let list = client
            .h
            .call(Method::HostList, json!({}), Context::default())
            .await
            .expect("host ls");
        assert_eq!(
            list["yielded_local_lanes"],
            json!([{"lane": "boxapp@0", "winner": "box"}]),
            "{list}"
        );

        let reply = use_lane(&client, "boxapp@0", None).await;
        assert_eq!(
            reply,
            Ok(json!({"lane": "boxapp@0", "host": null, "previous": "box", "mirrored": true}))
        );
        assert!(!answers(&client, "web.boxapp.localhost", "hello-from-box").await);
        assert_eq!(client.host().await["chosen_lanes"], json!([]));

        assert!(use_lane(&client, "boxapp@0", Some("box")).await.is_ok());
        client
            .h
            .call(Method::Start, json!({"units": ["web"]}), cx(&local_repo))
            .await
            .expect("local start");
        let local = eventually(SOON, || async {
            answers(&client, "web.boxapp.localhost", "hello-from-local")
                .await
                .then_some(())
        })
        .await;
        assert!(local.is_some(), "the local lane takes its hostnames back");
        let skip = skipped(&client, "box", "boxapp@0").await.expect("skipped");
        assert_eq!(skip["cause"], "local_running", "{skip}");
        assert_eq!(skip["can_use"], false, "{skip}");
        assert_eq!(client.host().await["chosen_lanes"], json!([]));
        assert_eq!(
            use_lane(&client, "boxapp@0", Some("box")).await,
            Err("local_lane_running".to_owned())
        );
        let seen = warnings(&mut frames).await;
        assert!(
            seen.iter().any(|w| w["code"] == "choice_cleared"
                && w["host"] == "box"
                && w["lane"] == "boxapp@0"),
            "{seen:?}"
        );

        session.close();
        let _ = events.await;
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A choice survives a client daemon restart: the restore opens `alpha`
/// first, and `beta`, the chosen box, takes the lane over once it is back.
/// `remote disconnect` deletes the choice from the store.
async fn a_choice_survives_a_restart() {
    if !ipv6_loopback().await {
        eprintln!("skipping a_choice_survives_a_restart: no IPv6 loopback");
        return;
    }
    const ECHO: u16 = 14_990;
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let alpha_repo = repo(&tmp.path, "forkalpha", &fork_project("alpha", ECHO, true));
        let beta_repo = repo(&tmp.path, "forkbeta", &fork_project("beta", ECHO, false));
        let alpha = BoxSide::start(&tmp, "alpha", "14500-14699").await;
        let beta = BoxSide::start(&tmp, "beta", "14700-14899").await;
        for (side, repo) in [(&alpha, &alpha_repo), (&beta, &beta_repo)] {
            side.h
                .call(Method::Start, json!({"units": ["web"]}), cx(repo))
                .await
                .expect("box start");
        }
        let boxes = [("alpha", &alpha), ("beta", &beta)];
        let client = ClientSide::start_with(&tmp, "client", "14900-14989", &boxes, None).await;
        client.connect("alpha").await;
        client.connect("beta").await;
        let reply = use_lane(&client, "forkapp@0", Some("beta")).await;
        assert_eq!(
            reply.map(|reply| reply["mirrored"].clone()),
            Ok(json!(true))
        );
        assert!(answers(&client, FORK_HOSTS[0], "web-from-beta").await);
        client.stop().await;

        let client = ClientSide::start_with(&tmp, "client", "14900-14989", &boxes, None).await;
        let restored = eventually(SOON, || async {
            answers(&client, FORK_HOSTS[0], "web-from-beta")
                .await
                .then_some(())
        })
        .await;
        assert!(
            restored.is_some(),
            "beta serves the lane after the restart: {}",
            client.host_named("beta").await
        );
        assert_eq!(
            client.host_named("beta").await["chosen_lanes"],
            json!(["forkapp@0"])
        );
        let skip = eventually(SOON, || skipped(&client, "alpha", "forkapp@0")).await;
        assert_eq!(
            skip.map(|skip| skip["cause"].clone()),
            Some(json!("box_chosen"))
        );

        super::boxes::disconnect(&client, "beta").await;
        let home = client.h.home.clone();
        client.stop().await;
        let store = Store::open(home.join("state.db")).await.expect("store");
        let choices = store.remote_lane_choices().await.expect("choices");
        store.close().await.expect("close");
        assert!(choices.is_empty(), "{choices:?}");

        alpha.stop().await;
        beta.stop().await;
    })
    .await
    .expect("timeout");
}
