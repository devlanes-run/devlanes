//! Pairing over the in-memory transport: a box that requires it serves a
//! client only after the client traded a one-time code for a remote token,
//! the client presents the token from then on, also after its daemon
//! restarts, and `daemon revoke` ends the client's session and refuses it
//! until it pairs again. A child of `remote`, whose box and client harness it
//! reuses.

use std::{net::Ipv6Addr, path::Path};

use bytes::Bytes;
use devlanes_daemon::{Handler, Session};
use devlanes_ipc::Frame;
use devlanes_proto::{
    Context, Control, Method, Request, RpcError,
    remote::{CLIENTS_EVENT, PROMPT_EVENT, TOKEN_PREFIX, is_random_id},
};
use devlanes_state::Store;
use serde_json::{Value, json};
use tokio::net::TcpStream;

use super::{BOX, BoxSide, ClientSide, SOON, box_port, echo, http_get, ipv6_loopback};
use crate::{LIMIT, cx, eventually, repo, temp};

// Port ranges: 15400-15799 and 16000-16999, between the DNS server's 15353
// and `remote_sync.rs`'s 17000, which no other test in this binary uses.
// Each test has its own slice, given where it starts its box.

/// The `fix` of a client waiting for pairing with box `host`.
fn pair_fix(host: &str) -> String {
    format!("devlanes daemon pair on {host}, then devlanes remote connect {host} --code <code>")
}

/// `connect` with `params` from a CLI that cannot answer prompts: its
/// session is gone before the daemon asks anything.
async fn connect_unattended(client: &ClientSide, params: Value) -> Result<Value, RpcError> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(9_400);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (session, frames) = Session::local(id);
    drop(frames);
    let request = Request {
        id,
        method: Method::Connect,
        params,
        context: Context::default(),
    };
    client.h.engine.handle(request, session).await
}

/// The id a client engine keeps in its home.
async fn client_id(client: &ClientSide) -> String {
    tokio::fs::read_to_string(client.h.home.join("client-id"))
        .await
        .expect("the client stored its id")
        .trim()
        .to_owned()
}

/// The remote token the client under `home` keeps for box `host`.
async fn stored_token(home: &Path, host: &str) -> Option<String> {
    let store = Store::open(home.join("state.db")).await.expect("store");
    let token = store.remote_token(host.into()).await.expect("token");
    store.close().await.expect("close");
    token
}

/// The box's `daemon clients` entry of the client with id `id`.
async fn listed(side: &BoxSide, id: &str) -> Option<Value> {
    let clients = side
        .h
        .call(Method::RemoteClients, json!({}), Context::default())
        .await
        .expect("daemon clients");
    // The times are measured against the box's own clock, which it sends.
    assert!(clients["now_ms"].as_u64().is_some(), "{clients}");
    clients["items"]
        .as_array()?
        .iter()
        .find(|item| item["id"] == id)
        .cloned()
}

#[tokio::test(flavor = "multi_thread")]
/// A client with no token and nobody to type a code waits in `needs_pairing`,
/// and the box tells it nothing. With a code it pairs: it keeps a `dl_rt_`
/// token, the box only the token's SHA-256, and `daemon clients` lists it
/// paired and connected. The code is then used up. A restarted client daemon
/// connects again with the token alone.
async fn a_code_pairs_a_client_whose_token_serves_later_connections() {
    if !ipv6_loopback().await {
        eprintln!("skipping a_code_pairs_a_client_whose_token_serves_later_connections: no IPv6");
        return;
    }
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let main = repo(&tmp.path, "boxrepo", BOX);
        let side = BoxSide::start(&tmp, "box", "16000-16124").await;
        side.h
            .call(Method::Start, json!({"units": ["echo"]}), cx(&main))
            .await
            .expect("box start");
        let echo_port = box_port(&side.h, 0, "echo").await;
        let client = ClientSide::start(&tmp, "client", "16125-16249", &side).await;

        let refused = connect_unattended(&client, json!({"host": "box", "background": true}))
            .await
            .expect_err("connected without pairing");
        assert_eq!(refused.code, "remote_needs_pairing", "{refused:?}");
        assert_eq!(refused.data["fix"], pair_fix("box"), "{refused:?}");
        let host = client.host().await;
        assert_eq!(host["state"], "needs_pairing", "{host}");
        assert!(
            host["error"]
                .as_str()
                .is_some_and(|error| error.contains("devlanes daemon pair")),
            "{host}"
        );
        assert_eq!(host["lanes"], 0, "the box sent no registry: {host}");
        let id = client_id(&client).await;
        assert_eq!(
            listed(&side, &id).await,
            None,
            "nothing about it on the box"
        );

        let code = side.code().await;
        let connected = client
            .h
            .call(
                Method::Connect,
                json!({"host": "box", "background": true, "code": code}),
                Context::default(),
            )
            .await
            .expect("connect with a code");
        assert_eq!(connected["state"], "connected", "{connected}");
        let mirrored = eventually(SOON, || async { echo(echo_port).await.then_some(()) }).await;
        assert!(mirrored.is_some(), "echo mirrored after pairing");

        let token = stored_token(&client.h.home, "box")
            .await
            .expect("the client kept a token");
        assert!(is_random_id(&token, TOKEN_PREFIX), "{token}");
        let entry = listed(&side, &id).await.expect("the box lists the client");
        assert_eq!(entry["paired"], true, "{entry}");
        assert_eq!(
            entry["connections"].as_array().map(Vec::len),
            Some(1),
            "{entry}"
        );
        let box_store = Store::open(side.h.home.join("state.db"))
            .await
            .expect("box store");
        let pairings = box_store
            .remote_pairings_of(id.clone())
            .await
            .expect("pairings");
        box_store.close().await.expect("close");
        let [pairing] = pairings.as_slice() else {
            panic!("one pairing: {pairings:?}");
        };
        assert_ne!(pairing.token_sha256, token, "only a hash is kept");
        assert_eq!(pairing.token_sha256.len(), 64, "{}", pairing.token_sha256);

        // The code is used up: a session that offers it again is refused.
        let mut options = devlanes_transport::SessionOptions::new("probe");
        options.client_id = format!("dl_cid_{}", "1".repeat(64));
        options.pairing = Some(super::OneCode::new(code));
        let reused = devlanes_transport::Session::connect(&side.transport, options).await;
        assert!(
            reused.as_ref().is_err_and(|error| error.is_needs_pairing()),
            "{reused:?}"
        );

        client.stop().await;
        let client = ClientSide::start(&tmp, "client", "16125-16249", &side).await;
        let restored = eventually(SOON, || async {
            (client.host().await["state"] == "connected" && echo(echo_port).await).then_some(())
        })
        .await;
        assert!(
            restored.is_some(),
            "the token served the restarted daemon: {}",
            client.host().await
        );
        assert_eq!(
            stored_token(&client.h.home, "box").await.as_deref(),
            Some(token.as_str()),
            "the token did not change"
        );

        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// `daemon revoke` deletes the pairing and ends the client's session at once.
/// The client then waits in `needs_pairing` with its listener bound and its
/// route on the pairing page, the box refuses its token, and a new code
/// pairs it again.
async fn revoking_a_client_ends_its_session_until_it_pairs_again() {
    if !ipv6_loopback().await {
        eprintln!("skipping revoking_a_client_ends_its_session_until_it_pairs_again: no IPv6");
        return;
    }
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let main = repo(&tmp.path, "boxrepo", BOX);
        let side = BoxSide::start(&tmp, "box", "16250-16374").await;
        side.h
            .call(Method::Start, json!({"units": ["web", "echo"]}), cx(&main))
            .await
            .expect("box start");
        let echo_port = box_port(&side.h, 0, "echo").await;
        let client = ClientSide::start(&tmp, "client", "16375-16499", &side).await;
        client.connect("box").await;
        let served = eventually(SOON, || async {
            let web = http_get(client.proxy_port, "web.boxapp.localhost").await?;
            (web.contains("hello-from-box") && echo(echo_port).await).then_some(())
        })
        .await;
        assert!(served.is_some(), "the box is mirrored");

        let id = client_id(&client).await;
        let mut bus = side.h.engine.subscribe();
        let revoked = side
            .h
            .call(
                Method::RemoteRevoke,
                json!({"client": id}),
                Context::default(),
            )
            .await
            .expect("revoke");
        assert_eq!(revoked["revoked"][0]["id"], id.as_str(), "{revoked}");
        assert_eq!(revoked["ended"], 1, "{revoked}");
        // A TUI showing the clients hears of it: the revoke announces it before
        // answering, so the event is queued when the call returns.
        let announced =
            std::iter::from_fn(|| bus.try_recv().ok()).any(|event| event.event == CLIENTS_EVENT);
        assert!(announced, "no {CLIENTS_EVENT} when the revoke returned");

        let host = eventually(SOON, || async {
            let host = client.host().await;
            (host["state"] == "needs_pairing").then_some(host)
        })
        .await;
        let Some(host) = host else {
            panic!("the client needs pairing: {}", client.host().await);
        };
        assert_eq!(host["fix"], pair_fix("box"), "{host}");
        assert!(
            TcpStream::connect((Ipv6Addr::LOCALHOST, echo_port))
                .await
                .is_ok(),
            "the listener stays while the box waits for pairing"
        );
        let route = eventually(SOON, || async {
            let state = client.route_state("web.boxapp.localhost").await;
            (state.as_deref() == Some("pairing_needed")).then_some(())
        })
        .await;
        assert!(route.is_some(), "the route waits for pairing");
        let page = http_get(client.proxy_port, "web.boxapp.localhost")
            .await
            .unwrap_or_default();
        assert!(page.contains("box-pairing-needed"), "{page}");
        assert!(page.contains("devlanes daemon pair"), "{page}");
        let gone = eventually(SOON, || async {
            listed(&side, &id).await.is_none().then_some(())
        })
        .await;
        assert!(gone.is_some(), "{:?}", listed(&side, &id).await);
        let again = side
            .h
            .call(
                Method::RemoteRevoke,
                json!({"client": id}),
                Context::default(),
            )
            .await
            .expect_err("revoked twice");
        assert_eq!(again.code, "client_not_found", "{again:?}");

        // A new code pairs the same link again.
        client.connect("box").await;
        let back = eventually(SOON, || async { echo(echo_port).await.then_some(()) }).await;
        assert!(back.is_some(), "the mirrored port works again");
        let entry = listed(&side, &id).await.expect("paired again");
        assert_eq!(entry["paired"], true, "{entry}");

        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A box with `remote.require_pairing: false` serves a client that brings
/// no code, which `daemon clients` lists as connected but not paired and
/// `daemon revoke` cannot revoke.
async fn a_box_without_pairing_serves_every_client() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start_unpaired(&tmp, "box", "16500-16562").await;
        let client = ClientSide::start(&tmp, "client", "16563-16624", &side).await;
        let connected = connect_unattended(&client, json!({"host": "box", "background": true}))
            .await
            .expect("connect without a code");
        assert_eq!(connected["state"], "connected", "{connected}");
        let id = client_id(&client).await;
        let entry = eventually(SOON, || async { listed(&side, &id).await }).await;
        let Some(entry) = entry else {
            panic!("the box lists the client");
        };
        assert_eq!(entry["paired"], false, "{entry}");
        assert_eq!(stored_token(&client.h.home, "box").await, None);
        let refused = side
            .h
            .call(
                Method::RemoteRevoke,
                json!({"client": id}),
                Context::default(),
            )
            .await
            .expect_err("revoked an unpaired client");
        assert_eq!(refused.code, "client_not_found", "{refused:?}");
        assert_eq!(refused.data["unpaired"], true, "{refused:?}");
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// Without `--code`, the attached `remote connect` is asked for the code
/// with a `code` prompt, and its answer pairs the client.
async fn the_attached_cli_is_asked_for_the_code() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start(&tmp, "box", "16625-16687").await;
        let client = ClientSide::start(&tmp, "client", "16688-16749", &side).await;
        let code = side.code().await;
        let (session, mut frames) = Session::local(9_450);
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
        let mut kinds = Vec::new();
        let connected = loop {
            tokio::select! {
                result = &mut connecting => break result.expect("connect task"),
                frame = frames.recv() => match frame {
                    None => break (&mut connecting).await.expect("connect task"),
                    Some(Frame::Control(Control::Event(event))) if event.event == PROMPT_EVENT => {
                        kinds.push(event.data["kind"].as_str().unwrap_or_default().to_owned());
                        let stream = event.data["stream_id"].as_u64().expect("stream id");
                        engine
                            .input(stream, Bytes::from(code.clone()), session.clone())
                            .await
                            .expect("answer");
                    }
                    Some(_) => {}
                },
            }
        };
        let connected = connected.expect("the typed code pairs");
        assert_eq!(connected["state"], "connected", "{connected}");
        assert_eq!(kinds, ["code"]);
        assert!(stored_token(&client.h.home, "box").await.is_some());
        session.close();
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A wrong code voids the box's outstanding codes, so the right one fails
/// after it. After five failures `daemon pair` hands out no code and says
/// how long, every code is refused, and the client keeps the time the box
/// takes codes again, not a wait that goes stale.
async fn wrong_codes_void_codes_and_lock_out_pairing() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start(&tmp, "box", "16750-16812").await;
        let client = ClientSide::start(&tmp, "client", "16813-16874", &side).await;
        let attempt = |code: String| {
            let client = &client;
            async move {
                connect_unattended(
                    client,
                    json!({"host": "box", "background": true, "code": code}),
                )
                .await
                .expect_err("paired with a refused code")
            }
        };
        let good = side.code().await;
        let wrong = attempt("0000-0000".into()).await;
        assert_eq!(wrong.code, "remote_needs_pairing", "{wrong:?}");
        let message = wrong.data["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("wrong, expired or already used"),
            "{wrong:?}"
        );
        let voided = attempt(good).await;
        assert!(
            voided.data["message"]
                .as_str()
                .is_some_and(|message| message.contains("already used")),
            "{voided:?}"
        );
        for _ in 0..3 {
            attempt("0000-0000".into()).await;
        }
        let paused = side
            .h
            .call(Method::RemotePair, json!({}), Context::default())
            .await
            .expect_err("a code while paused");
        assert_eq!(paused.code, "pairing_paused", "{paused:?}");
        let wait = paused.data["retry_after_ms"].as_u64().unwrap_or_default();
        assert!(wait > 0 && wait <= 10 * 60_000, "{paused:?}");
        let locked = attempt("0000-0000".into()).await;
        assert!(
            locked.data["message"]
                .as_str()
                .is_some_and(|message| message.contains("after 5 wrong codes")),
            "{locked:?}"
        );
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or_default();
        let retry_at = locked.data["retry_at_ms"].as_u64().unwrap_or_default();
        assert!(retry_at > now, "{locked:?}");
        let host = client.host().await;
        assert_eq!(host["state"], "needs_pairing", "{host}");
        assert_eq!(host["pairing_retry_at_ms"], retry_at, "{host}");
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A tunnel that requires pairing gets no feed for a client without a
/// pairing, which closes the gap between its token check and a revoke.
async fn the_feed_refuses_a_client_without_a_pairing() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start(&tmp, "box", "16875-16937").await;
        let id = format!("dl_cid_{}", "2".repeat(64));
        let refused = side
            .h
            .call(
                Method::RemoteRegistry,
                json!({"client": {"pid": 1, "since_ms": 0, "id": id}, "paired": true}),
                Context::default(),
            )
            .await
            .expect_err("a feed for an unpaired client");
        assert_eq!(refused.code, "client_not_paired", "{refused:?}");
        assert_eq!(listed(&side, &id).await, None, "the client is not listed");
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// Revoking one client ends its session only: another client of the same
/// box stays connected and listed.
async fn revoking_one_client_leaves_another_connected() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start(&tmp, "box", "16938-16958").await;
        let first = ClientSide::start(&tmp, "first", "16959-16979", &side).await;
        let second = ClientSide::start(&tmp, "second", "16980-16999", &side).await;
        first.connect("box").await;
        second.connect("box").await;
        let (first_id, second_id) = (client_id(&first).await, client_id(&second).await);
        let both = eventually(SOON, || async {
            let a = listed(&side, &first_id).await?;
            let b = listed(&side, &second_id).await?;
            (a["connections"].as_array()?.len() == 1 && b["connections"].as_array()?.len() == 1)
                .then_some(())
        })
        .await;
        assert!(both.is_some(), "both clients are connected");

        let revoked = side
            .h
            .call(
                Method::RemoteRevoke,
                json!({"client": first_id}),
                Context::default(),
            )
            .await
            .expect("revoke");
        assert_eq!(revoked["ended"], 1, "{revoked}");
        let waiting = eventually(SOON, || async {
            (first.host().await["state"] == "needs_pairing").then_some(())
        })
        .await;
        assert!(waiting.is_some(), "{}", first.host().await);
        // Long enough for a session the revoke ended by mistake to go down.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(second.host().await["state"], "connected");
        let entry = listed(&side, &second_id).await.expect("still paired");
        assert_eq!(entry["paired"], true, "{entry}");
        assert_eq!(
            entry["connections"].as_array().map(Vec::len),
            Some(1),
            "{entry}"
        );
        first.stop().await;
        second.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// `daemon reset-state` on the box deletes every pairing: a connected client
/// loses its session and waits in `needs_pairing`, and a code handed out
/// before the reset no longer works.
async fn a_state_reset_on_the_box_ends_every_pairing() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start(&tmp, "box", "15400-15499").await;
        let client = ClientSide::start(&tmp, "client", "15500-15599", &side).await;
        client.connect("box").await;
        let before = side.code().await;
        let yes = Context {
            yes: true,
            ..Context::default()
        };
        side.h
            .call(Method::DaemonResetState, json!({}), yes)
            .await
            .expect("reset");
        let waiting = eventually(SOON, || async {
            (client.host().await["state"] == "needs_pairing").then_some(())
        })
        .await;
        assert!(waiting.is_some(), "{}", client.host().await);
        let refused = connect_unattended(
            &client,
            json!({"host": "box", "background": true, "code": before}),
        )
        .await
        .expect_err("a code from before the reset");
        assert!(
            refused.data["message"]
                .as_str()
                .is_some_and(|message| message.contains("already used")),
            "{refused:?}"
        );
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A machine that reaches one box under two names pairs once per name, and
/// the second pairing leaves the first working: the box lists one client
/// with two pairings, and a restarted client connects under both names with
/// their tokens.
async fn two_names_for_one_box_keep_both_pairings() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start(&tmp, "box", "15600-15699").await;
        let boxes = [("box", &side), ("alias", &side)];
        let client = ClientSide::start_with(&tmp, "client", "15700-15799", &boxes, None).await;
        client.connect("box").await;
        client.connect("alias").await;
        let id = client_id(&client).await;
        let entry = listed(&side, &id).await.expect("paired");
        assert_eq!(entry["pairings"], 2, "{entry}");
        assert_ne!(
            stored_token(&client.h.home, "box").await,
            stored_token(&client.h.home, "alias").await,
            "one token per name"
        );
        client.stop().await;

        let client = ClientSide::start_with(&tmp, "client", "15700-15799", &boxes, None).await;
        let restored = eventually(SOON, || async {
            (client.host_named("box").await["state"] == "connected"
                && client.host_named("alias").await["state"] == "connected")
                .then_some(())
        })
        .await;
        assert!(
            restored.is_some(),
            "both names connect with their tokens: {} {}",
            client.host_named("box").await,
            client.host_named("alias").await
        );
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A box with `remote.allow_connections: false` turns the client away before
/// pairing: `connect` fails with the box's reason, and the link stays failed
/// with it instead of retrying.
async fn a_box_that_takes_no_connections_fails_the_link_with_its_reason() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let reason =
            "the box does not accept connections. Set remote.allow_connections to true there";
        let side = BoxSide::start_refusing(&tmp, "box", "18200-18224", reason).await;
        let client = ClientSide::start(&tmp, "client", "18225-18249", &side).await;
        let refused = connect_unattended(&client, json!({"host": "box", "background": true}))
            .await
            .expect_err("connected to a box that takes no connections");
        assert_eq!(refused.code, "remote_failed", "{refused:?}");
        assert_eq!(refused.data["message"], reason, "{refused:?}");
        let host = client.host().await;
        assert_eq!(host["state"], "failed", "{host}");
        assert_eq!(host["error"], reason, "{host}");
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// `daemon pair` gives no code while `remote.allow_connections` is false, or
/// while the remote settings it is read with are invalid, and gives one
/// again once connections are allowed.
async fn pairing_needs_connections_to_be_allowed() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start(&tmp, "box", "18200-18224").await;
        let config = side.h.home.join("config.yaml");
        let pair = || async {
            side.h
                .call(Method::RemotePair, json!({}), Context::default())
                .await
        };
        for (source, invalid) in [
            ("remote:\n  allow_connections: false\n", false),
            ("remote:\n  allow_connections: nope\n", true),
        ] {
            tokio::fs::write(&config, source).await.expect("config");
            let refused = pair().await.expect_err("a code with connections off");
            assert_eq!(refused.code, "connections_off", "{refused:?}");
            assert_eq!(refused.data["invalid"], invalid, "{refused:?}");
        }
        tokio::fs::write(&config, "remote:\n  allow_connections: true\n")
            .await
            .expect("config");
        let paired = pair().await.expect("a code");
        assert!(paired["code"].as_str().is_some(), "{paired}");
        side.stop().await;
    })
    .await
    .expect("timeout");
}
