//! A client paired by ticket over memory peer endpoints: `remote.connect`
//! with a ticket pairs with the phase 2 box and keeps the box under its
//! name, the link reconnects while the box has no direct path, and stops
//! for good once the box refuses this machine. A child of `remote`, whose
//! box and client harness it reuses.

use devlanes_proto::{Context, Method};
use devlanes_state::Store;
use devlanes_transport::{PeerEndpoint, memory::MemoryPeers};
use serde_json::{Value, json};

use super::{
    BOX, BoxSide, ClientSide, SOON, box_port, echo, ipv6_loopback,
    pairing::{client_id, connect_unattended, new_ticket},
};
use crate::{LIMIT, cx, eventually, repo, temp};

// Port ranges: 11700-11999, which no other test in this binary uses.

/// The `remote ls` entry of box `name` once `check` holds for it.
async fn host_when(
    client: &ClientSide,
    name: &str,
    check: impl Fn(&Value) -> bool,
) -> Option<Value> {
    eventually(SOON, || async {
        let host = client.host_named(name).await;
        check(&host).then_some(host)
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
/// `remote.connect` with a ticket pairs, keeps the box under the name it
/// answered with and the key the ticket named, connects and mirrors its
/// port. The ticket then works no more, and a failed pairing stores
/// nothing. With no direct path the link reconnects with that reason and
/// comes back once there is one. A revoke on the box, which then stops
/// listening, ends the link `failed` with the pair-again text.
async fn a_ticket_pairs_and_connects_until_the_box_refuses_the_key() {
    if !ipv6_loopback().await {
        eprintln!("skipping a_ticket_pairs_and_connects_until_the_box_refuses_the_key: no IPv6");
        return;
    }
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let main = repo(&tmp.path, "boxrepo", BOX);
        let peers = MemoryPeers::new();
        let endpoint = peers.endpoint();
        let side = BoxSide::start_peer(&tmp, "box", "11700-11799", endpoint.clone()).await;
        side.h
            .call(Method::Start, json!({"units": ["echo"]}), cx(&main))
            .await
            .expect("box start");
        let echo_port = box_port(&side.h, 0, "echo").await;
        let client =
            ClientSide::start_peer(&tmp, "client", "11800-11899", &[], peers.endpoint()).await;

        let ticket = new_ticket(&side).await.encode();
        let connected = client
            .h
            .call(
                Method::Connect,
                json!({"ticket": ticket, "background": true}),
                Context::default(),
            )
            .await
            .expect("connect with a ticket");
        assert_eq!(connected["state"], "connected", "{connected}");
        let name = connected["host"].as_str().unwrap_or_default().to_owned();
        let key = endpoint.id().await.expect("id");
        let store = Store::open(client.h.home.join("state.db"))
            .await
            .expect("client store");
        let boxes = store.p2p_boxes().await.expect("boxes");
        let [paired] = boxes.as_slice() else {
            panic!("one box paired: {boxes:?}");
        };
        assert_eq!(
            (paired.name.as_str(), paired.peer_key.as_str()),
            (name.as_str(), key.to_string().as_str())
        );
        let mirrored = eventually(SOON, || async { echo(echo_port).await.then_some(()) }).await;
        assert!(
            mirrored.is_some(),
            "echo mirrored over the ticket's pairing"
        );
        let listed = client.host_named(&name).await;
        assert_eq!(listed["transport"], "p2p", "{listed}");
        assert_eq!(
            listed["address"],
            format!("p2p:{}", key.short()),
            "{listed}"
        );

        let reused = client
            .h
            .call(
                Method::Connect,
                json!({"ticket": ticket, "name": "other", "background": true}),
                Context::default(),
            )
            .await
            .expect_err("a ticket used twice");
        assert_eq!(reused.code, "pairing_refused", "{reused:?}");
        assert_eq!(
            store.p2p_boxes().await.expect("boxes").len(),
            1,
            "the refused pairing stored nothing"
        );

        client
            .h
            .call(
                Method::Disconnect,
                json!({"host": name}),
                Context::default(),
            )
            .await
            .expect("disconnect");
        peers.set_reachable(key, false);
        connect_unattended(&client, json!({"host": name}))
            .await
            .expect("connect without a direct path");
        let retrying = host_when(&client, &name, |host| {
            host["state"] == "reconnecting"
                && host["error"]
                    .as_str()
                    .is_some_and(|error| error.contains("no direct connection"))
        })
        .await;
        assert!(retrying.is_some(), "{}", client.host_named(&name).await);
        peers.set_reachable(key, true);
        let back = host_when(&client, &name, |host| host["state"] == "connected").await;
        assert!(back.is_some(), "{}", client.host_named(&name).await);

        let id = client_id(&client).await;
        side.h
            .call(
                Method::RemoteRevoke,
                json!({"client": id}),
                Context::default(),
            )
            .await
            .expect("revoke");
        assert!(!endpoint.is_listening(), "no client and no ticket is left");
        let failed = host_when(&client, &name, |host| host["state"] == "failed").await;
        let Some(failed) = failed else {
            panic!("the link fails: {}", client.host_named(&name).await);
        };
        let error = failed["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("no longer accepts this machine") && error.contains("<ticket>"),
            "{failed}"
        );
        store.close().await.expect("close");
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}
