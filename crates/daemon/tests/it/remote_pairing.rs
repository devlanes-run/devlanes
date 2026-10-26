//! Pairing over the in-memory transport: a box that requires it serves a
//! client only after the client traded a one-time code for a remote token,
//! the client presents the token from then on, also after its daemon
//! restarts, and `remote revoke` ends the client's session and refuses it
//! until it pairs again. A child of `remote`, whose box and client harness it
//! reuses.

use std::{net::Ipv6Addr, path::Path};

use bytes::Bytes;
use devlanes_daemon::{Handler, Session};
use devlanes_ipc::Frame;
use devlanes_proto::{
    Context, Control, Method, Request, RpcError,
    remote::{PROMPT_EVENT, TOKEN_PREFIX, is_random_id},
};
use devlanes_state::Store;
use serde_json::{Value, json};
use tokio::net::TcpStream;

use super::{BOX, BoxSide, ClientSide, SOON, box_port, echo, http_get, ipv6_loopback};
use crate::{LIMIT, cx, eventually, repo, temp};

// Port ranges: 16000-16999, below `remote_sync.rs`'s 17000 and above the DNS
// server's 15353, which no other test in this binary uses.

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
fn client_id(client: &ClientSide) -> String {
    std::fs::read_to_string(client.h.home.join("client-id"))
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

/// The box's `remote clients` entry of the client with id `id`.
async fn listed(side: &BoxSide, id: &str) -> Option<Value> {
    let clients = side
        .h
        .call(Method::RemoteClients, json!({}), Context::default())
        .await
        .expect("remote clients");
    clients["items"]
        .as_array()?
        .iter()
        .find(|item| item["id"] == id)
        .cloned()
}

#[tokio::test(flavor = "multi_thread")]
/// A client with no token and nobody to type a code waits in `needs_pairing`,
/// and the box tells it nothing. With a code it pairs: it keeps a `dl_rt_`
/// token, the box only the token's SHA-256, and `remote clients` lists it
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
        assert_eq!(
            refused.data["fix"], "devlanes remote connect box --code <code>",
            "{refused:?}"
        );
        let host = client.host().await;
        assert_eq!(host["state"], "needs_pairing", "{host}");
        assert!(
            host["error"]
                .as_str()
                .is_some_and(|error| error.contains("devlanes remote pair")),
            "{host}"
        );
        assert_eq!(host["lanes"], 0, "the box sent no registry: {host}");
        let id = client_id(&client);
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
        let pairing = box_store
            .remote_pairing(id.clone())
            .await
            .expect("pairing")
            .expect("the box keeps the pairing");
        box_store.close().await.expect("close");
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
/// `remote revoke` deletes the pairing and ends the client's session at once.
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

        let id = client_id(&client);
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

        let host = eventually(SOON, || async {
            let host = client.host().await;
            (host["state"] == "needs_pairing").then_some(host)
        })
        .await;
        let Some(host) = host else {
            panic!("the client needs pairing: {}", client.host().await);
        };
        assert_eq!(
            host["fix"], "devlanes remote connect box --code <code>",
            "{host}"
        );
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
        assert!(page.contains("devlanes remote pair"), "{page}");
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
/// no code, which `remote clients` lists as connected but not paired.
async fn a_box_without_pairing_serves_every_client() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start_unpaired(&tmp, "box", "16500-16624").await;
        let client = ClientSide::start(&tmp, "client", "16625-16749", &side).await;
        let connected = connect_unattended(&client, json!({"host": "box", "background": true}))
            .await
            .expect("connect without a code");
        assert_eq!(connected["state"], "connected", "{connected}");
        let id = client_id(&client);
        let entry = eventually(SOON, || async { listed(&side, &id).await }).await;
        let Some(entry) = entry else {
            panic!("the box lists the client");
        };
        assert_eq!(entry["paired"], false, "{entry}");
        assert_eq!(stored_token(&client.h.home, "box").await, None);
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
        let side = BoxSide::start(&tmp, "box", "16750-16874").await;
        let client = ClientSide::start(&tmp, "client", "16875-16999", &side).await;
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
/// after it, and after five failures a fresh code is refused with the wait,
/// which `remote pair` reports too.
async fn wrong_codes_void_codes_and_lock_out_pairing() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let side = BoxSide::start(&tmp, "box", "16500-16624").await;
        let client = ClientSide::start(&tmp, "client", "16625-16749", &side).await;
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
            .expect("pair");
        assert!(paused["retry_after_ms"].as_u64().is_some(), "{paused}");
        let fresh = paused["code"].as_str().expect("code").to_owned();
        let locked = attempt(fresh).await;
        assert!(
            locked.data["message"]
                .as_str()
                .is_some_and(|message| message.contains("too many wrong pairing codes")),
            "{locked:?}"
        );
        assert_eq!(client.host().await["state"], "needs_pairing");
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
        let side = BoxSide::start(&tmp, "box", "16500-16624").await;
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
