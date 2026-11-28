//! A client paired by ticket over memory peer endpoints: `remote.connect`
//! with a ticket pairs with the phase 2 box and keeps the box under its
//! name, the link reconnects while the box has no direct path, and stops
//! for good once the box refuses this machine. A starting daemon restores
//! such a box and stops it, or forgets an ephemeral one, once it stayed
//! away too long, from stored times. A child of `remote`, whose box and
//! client harness it reuses.

use devlanes_proto::{Context, Method};
use devlanes_state::{P2pBox, RemoteHost, RemoteLaneChoice, Store};
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

#[tokio::test(flavor = "multi_thread")]
/// A starting daemon restores a box paired by ticket that is offline,
/// counting from when it was last reached. Away 25 hours, the first failed
/// dial ends the link `failed` with the give-up text, keeping its pairing,
/// and `remote.connect` tries again. Ephemeral and away 2 hours, it is
/// forgotten: its connection record, lane choices and pairing are deleted,
/// another box's lane choice stays, and `remote ls` no longer lists it.
async fn a_restored_box_that_stayed_away_too_long_stops_or_is_forgotten() {
    tokio::time::timeout(LIMIT, async {
        let hour = 60 * 60 * 1000;
        for (ephemeral, away, ports) in [
            (false, 25 * hour, "11900-11949"),
            (true, 2 * hour, "11950-11999"),
        ] {
            let tmp = temp();
            let home = tmp.path.join("client");
            std::fs::create_dir_all(&home).expect("home");
            let store = Store::open(home.join("state.db")).await.expect("store");
            let now = i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_millis(),
            )
            .expect("millis");
            let key = "e".repeat(64);
            store
                .upsert_remote_host(RemoteHost {
                    name: "acme".into(),
                    transport: "p2p".into(),
                    address: "p2p:eeeeeeee".into(),
                    daemon_version: None,
                    last_connected: Some(now - away),
                    wanted: true,
                })
                .await
                .expect("connection record");
            store
                .add_p2p_box(P2pBox {
                    name: "acme".into(),
                    peer_key: key,
                    relay_url: None,
                    direct_addrs: Vec::new(),
                    ephemeral,
                    paired_at: now - 30 * hour,
                })
                .await
                .expect("pairing");
            for (lane, host) in [("boxapp@0", "acme"), ("otherapp@0", "other")] {
                store
                    .set_remote_lane_choice(RemoteLaneChoice {
                        lane: lane.into(),
                        host: host.into(),
                        chosen_at: now,
                    })
                    .await
                    .expect("lane choice");
            }
            store.close().await.expect("close");

            // No endpoint has the stored key, so every dial is unreachable.
            let client =
                ClientSide::start_peer(&tmp, "client", ports, &[], MemoryPeers::new().endpoint())
                    .await;
            let store = Store::open(home.join("state.db")).await.expect("store");
            if ephemeral {
                let forgotten = eventually(SOON, || async {
                    let boxes = store.p2p_boxes().await.ok()?;
                    let hosts = store.remote_hosts().await.ok()?;
                    (boxes.is_empty() && hosts.is_empty()).then_some(())
                })
                .await;
                assert!(forgotten.is_some(), "{:?}", store.p2p_boxes().await);
                let choices: Vec<String> = store
                    .remote_lane_choices()
                    .await
                    .expect("choices")
                    .into_iter()
                    .map(|choice| choice.host)
                    .collect();
                assert_eq!(choices, ["other"]);
                assert_eq!(client.host_named("acme").await, Value::Null);
            } else {
                let failed = host_when(&client, "acme", |host| host["state"] == "failed").await;
                let Some(failed) = failed else {
                    panic!("the link stops: {}", client.host_named("acme").await);
                };
                assert!(
                    failed["error"]
                        .as_str()
                        .is_some_and(|error| error.contains("for 24 hours")),
                    "{failed}"
                );
                assert_eq!(store.p2p_boxes().await.expect("boxes").len(), 1);
                connect_unattended(&client, json!({"host": "acme"}))
                    .await
                    .expect("connect again");
                let retrying = host_when(&client, "acme", |host| {
                    host["state"] == "reconnecting"
                        && host["error"]
                            .as_str()
                            .is_some_and(|error| error.contains("not reachable"))
                })
                .await;
                assert!(retrying.is_some(), "{}", client.host_named("acme").await);
            }
            store.close().await.expect("close");
            client.stop().await;
        }
    })
    .await
    .expect("timeout");
}
