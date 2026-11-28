//! A client paired by ticket over memory peer endpoints: `remote.connect`
//! with a ticket pairs with the phase 2 box and keeps the box under its
//! name, the link reconnects while the box has no direct path, and stops
//! for good once the box refuses this machine. A starting daemon restores
//! such boxes from stored rows, giving each its own attempts however long
//! the machine was off, and never resolves a name paired by ticket another
//! way. Taken names fail and store nothing. A child of `remote`, whose box
//! and client harness it reuses.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};

use devlanes_proto::{Context, Method};
use devlanes_state::{P2pBox, RemoteHost, Store};
use devlanes_transport::{
    Admission, Alpn, PairAnswer, PairDesk, PairRequest, PeerEndpoint, PeerId, Ticket, TicketSecret,
    answer_pairing, memory::MemoryPeers,
};
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
/// nothing. A new ticket for the box keeps its name and permanent pairing,
/// whatever name and kind it comes with, and replaces the link. With no
/// direct path the link reconnects with that reason and
/// comes back once there is one. Forgetting the box deletes its pairing and
/// record, so a later ticket pairs it as new. A revoke on the box, which then stops
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
        assert!(
            reused.data["message"]
                .as_str()
                .is_some_and(|message| message.contains("no pairing ticket open")),
            "{reused:?}"
        );
        assert_eq!(
            store.p2p_boxes().await.expect("boxes").len(),
            1,
            "the refused pairing stored nothing"
        );

        // A new ticket for the same box, ephemeral and with a name that is
        // taken here, keeps the name and the permanent pairing and opens a
        // new link.
        tokio::fs::write(
            client.h.home.join("config.yaml"),
            "remote:\n  boxes:\n    lab:\n      command: [devlanes-no-such-program, tunnel]\n",
        )
        .await
        .expect("config");
        let mut bus = client.h.engine.subscribe();
        let again = side
            .h
            .call(
                Method::RemotePair,
                json!({"ticket": true, "ephemeral": true}),
                Context::default(),
            )
            .await
            .expect("an ephemeral ticket");
        let repaired = client
            .h
            .call(
                Method::Connect,
                json!({"ticket": again["ticket"], "name": "lab", "background": true}),
                Context::default(),
            )
            .await
            .expect("pair again");
        assert_eq!(repaired["host"], name.as_str(), "{repaired}");
        assert_eq!(
            repaired["paired"],
            json!({"name": name, "kept": true, "name_ignored": true}),
            "{repaired}"
        );
        assert_eq!(repaired["state"], "connected", "{repaired}");
        let boxes = store.p2p_boxes().await.expect("boxes");
        assert!(
            matches!(boxes.as_slice(), [kept] if kept.name == name && !kept.ephemeral),
            "{boxes:?}"
        );
        // The old link was connected before the bus was subscribed and is
        // closed, which can only announce it stopped; `connected` for the
        // box comes from the watcher of a new link, which announces every
        // state it sees.
        let reopened = tokio::time::timeout(SOON, async {
            loop {
                match bus.recv().await {
                    Ok(event)
                        if event.event == devlanes_proto::remote::STATE_EVENT
                            && event.data["host"] == name.as_str()
                            && event.data["state"] == "connected" =>
                    {
                        return true;
                    }
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                }
            }
        })
        .await;
        assert_eq!(reopened, Ok(true), "the pairing replaced the live link");

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

        // Forgetting closes the live link and deletes the pairing, so the
        // next ticket pairs a box this machine does not know; only names
        // paired by ticket are forgotten.
        let forgot = client
            .h
            .call(
                Method::Disconnect,
                json!({"host": name, "forget": true}),
                Context::default(),
            )
            .await
            .expect("forget");
        assert_eq!(forgot["forgotten"], json!([name]), "{forgot}");
        assert!(store.p2p_boxes().await.expect("boxes").is_empty());
        assert!(
            !store
                .remote_hosts()
                .await
                .expect("hosts")
                .iter()
                .any(|host| host.name == name),
            "the connection record is gone too"
        );
        assert_eq!(client.host_named(&name).await, Value::Null);
        let refused = client
            .h
            .call(
                Method::Disconnect,
                json!({"host": "lab", "forget": true}),
                Context::default(),
            )
            .await
            .expect_err("a remote.boxes entry is not forgotten");
        assert_eq!(refused.code, "box_not_paired_by_ticket", "{refused:?}");
        let fresh = new_ticket(&side).await.encode();
        let paired_again = client
            .h
            .call(
                Method::Connect,
                json!({"ticket": fresh, "name": "lab2", "background": true}),
                Context::default(),
            )
            .await
            .expect("pair after forgetting");
        assert_eq!(
            paired_again["paired"],
            json!({"name": "lab2", "kept": false, "name_ignored": false}),
            "{paired_again}"
        );
        let name = "lab2".to_owned();

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

/// Milliseconds since the Unix epoch, the clock stored times are on.
fn now_ms() -> i64 {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock");
    i64::try_from(elapsed.as_millis()).expect("millis")
}

/// One hour in milliseconds.
const HOUR: i64 = 60 * 60 * 1000;

/// A pairing with box `name`, key `key` repeated to 64 digits, made
/// `paired_hours` ago.
fn paired_box(name: &str, key: char, ephemeral: bool, paired_hours: i64) -> P2pBox {
    P2pBox {
        name: name.into(),
        peer_key: key.to_string().repeat(64),
        relay_url: None,
        direct_addrs: Vec::new(),
        ephemeral,
        paired_at: now_ms() - paired_hours * HOUR,
    }
}

/// A `remote_hosts` row for box `name` over `transport`, last connected
/// `hours` ago when set.
fn host_row(name: &str, transport: &str, wanted: bool, hours: Option<i64>) -> RemoteHost {
    RemoteHost {
        name: name.into(),
        transport: transport.into(),
        address: name.into(),
        daemon_version: None,
        last_connected: hours.map(|hours| now_ms() - hours * HOUR),
        wanted,
    }
}

/// Writes `boxes`, `hosts` and the user config `config` into the client
/// home `home`, before its engine starts.
async fn seed(home: &Path, boxes: &[P2pBox], hosts: &[RemoteHost], config: Option<&str>) {
    std::fs::create_dir_all(home).expect("home");
    if let Some(config) = config {
        std::fs::write(home.join("config.yaml"), config).expect("config");
    }
    let store = Store::open(home.join("state.db")).await.expect("store");
    for paired in boxes {
        store.add_p2p_box(paired.clone()).await.expect("pairing");
    }
    for host in hosts {
        store
            .upsert_remote_host(host.clone())
            .await
            .expect("connection record");
    }
    store.close().await.expect("close");
}

#[tokio::test(flavor = "multi_thread")]
/// A starting daemon restores boxes paired by ticket that are offline, and
/// none gives up at its first failed dial, however long ago it was last
/// reached: a box last connected 2 days ago, an ephemeral one 10 hours ago,
/// one 23 hours ago and paired 30 hours ago, and one that never connected.
/// Each reconnects with the reason, and every pairing stays.
async fn a_restored_box_keeps_trying_after_the_machine_was_off() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let home = tmp.path.join("client");
        let rows = [
            ("weekend", 'a', false, Some(60), 70),
            ("night", 'b', true, Some(10), 30),
            ("day", 'c', false, Some(23), 30),
            ("never", 'd', false, None, 30),
        ];
        let boxes: Vec<P2pBox> = rows
            .iter()
            .map(|(name, key, ephemeral, _, paired)| paired_box(name, *key, *ephemeral, *paired))
            .collect();
        let hosts: Vec<RemoteHost> = rows
            .iter()
            .map(|(name, _, _, last, _)| host_row(name, "p2p", true, *last))
            .collect();
        seed(&home, &boxes, &hosts, None).await;
        // No endpoint has the stored keys, so every dial is unreachable.
        let client = ClientSide::start_peer(
            &tmp,
            "client",
            "11900-11949",
            &[],
            MemoryPeers::new().endpoint(),
        )
        .await;
        for (name, ..) in rows {
            let retrying = host_when(&client, name, |host| {
                host["state"] == "reconnecting"
                    && host["error"]
                        .as_str()
                        .is_some_and(|error| error.contains("not reachable"))
            })
            .await;
            assert!(
                retrying.is_some(),
                "{name}: {}",
                client.host_named(name).await
            );
        }
        let store = Store::open(home.join("state.db")).await.expect("store");
        assert_eq!(store.p2p_boxes().await.expect("boxes").len(), rows.len());
        store.close().await.expect("close");
        client.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A restored box recorded as paired by ticket whose pairing is gone fails
/// with `box_pairing_missing` instead of becoming an SSH destination, a
/// name that is both a paired box and a `remote.boxes` entry fails with
/// `box_name_ambiguous` on restore and on connect, a pairing that never
/// connected is listed as disconnected, and a code for a box paired by
/// ticket is refused.
async fn names_paired_by_ticket_are_never_resolved_another_way() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let home = tmp.path.join("client");
        let config =
            "remote:\n  boxes:\n    twice:\n      command: [devlanes-no-such-program, tunnel]\n";
        seed(
            &home,
            &[
                paired_box("solo", '5', false, 1),
                paired_box("twice", '6', false, 1),
            ],
            &[
                host_row("gone", "p2p", true, Some(1)),
                host_row("twice", "p2p", true, Some(1)),
            ],
            Some(config),
        )
        .await;
        let client = ClientSide::start_peer(
            &tmp,
            "client",
            "11950-11974",
            &[],
            MemoryPeers::new().endpoint(),
        )
        .await;
        for (name, text) in [
            ("gone", "no longer has the pairing"),
            (
                "twice",
                "both a box paired by ticket and an entry of remote.boxes",
            ),
        ] {
            let failed = host_when(&client, name, |host| host["state"] == "failed").await;
            let Some(failed) = failed else {
                panic!("{name}: {}", client.host_named(name).await);
            };
            assert!(
                failed["error"]
                    .as_str()
                    .is_some_and(|error| error.contains(text)),
                "{failed}"
            );
        }
        let ambiguous = connect_unattended(&client, json!({"host": "twice"}))
            .await
            .expect_err("a name claimed twice");
        assert_eq!(ambiguous.code, "box_name_ambiguous", "{ambiguous:?}");
        assert_eq!(ambiguous.data["paired_name"], "twice", "{ambiguous:?}");
        let solo = client.host_named("solo").await;
        assert_eq!(
            (&solo["state"], &solo["transport"], &solo["address"]),
            (
                &json!("disconnected"),
                &json!("p2p"),
                &json!("p2p:55555555")
            ),
            "{solo}"
        );
        // A name that differs from a paired box's only in case is the same
        // conflict, with or without a code, and never an SSH destination
        // sharing the box's sync directory.
        for params in [
            json!({"host": "SOLO"}),
            json!({"host": "SOLO", "code": "ABCD-EFGH"}),
        ] {
            let spelled = connect_unattended(&client, params)
                .await
                .expect_err("a paired box's name in another case");
            assert_eq!(spelled.code, "box_name_ambiguous", "{spelled:?}");
            assert_eq!(
                (&spelled.data["name"], &spelled.data["paired_name"]),
                (&json!("SOLO"), &json!("solo")),
                "{spelled:?}"
            );
            assert!(
                spelled.data["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("over SSH")),
                "{spelled:?}"
            );
        }
        let coded = connect_unattended(&client, json!({"host": "solo", "code": "ABCD-EFGH"}))
            .await
            .expect_err("a code for a box paired by ticket");
        assert_eq!(coded.code, "bad_request", "{coded:?}");
        assert!(
            coded.data["message"]
                .as_str()
                .is_some_and(|message| message.contains("pairs by ticket")),
            "{coded:?}"
        );
        client.stop().await;
    })
    .await
    .expect("timeout");
}

/// Lets every peer in, for both ALPNs.
struct AdmitAll;

impl Admission for AdmitAll {
    /// Always true.
    fn admit(&self, _: &PeerId, _: Alpn) -> bool {
        true
    }
}

/// A box that pairs every request under the name `Mesa`, counting them.
#[derive(Default)]
struct PairsAll {
    answered: AtomicU32,
}

#[async_trait::async_trait]
impl PairDesk for PairsAll {
    /// Pairs, as a box with a valid ticket would.
    async fn decide(&self, _: &PairRequest) -> PairAnswer {
        self.answered.fetch_add(1, Ordering::SeqCst);
        PairAnswer::Paired {
            name: "Mesa".into(),
            server: "devlanes test-box".into(),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
/// A name another box has here, in any case, fails with `box_name_taken`
/// and says what has it: a given name before the dial, a derived one whose
/// suffixes are all taken after the box answered. A name that starts like a
/// ticket is refused. None of them stores anything.
async fn a_taken_name_fails_and_stores_nothing() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let home = tmp.path.join("client");
        let mut hosts = vec![host_row("desk", "ssh", false, Some(1))];
        hosts.extend(
            std::iter::once("mesa".to_owned())
                .chain((2..=9).map(|n| format!("mesa-{n}")))
                .map(|name| host_row(&name, "ssh", false, Some(1))),
        );
        let config =
            "remote:\n  boxes:\n    lab:\n      command: [devlanes-no-such-program, tunnel]\n";
        seed(
            &home,
            &[paired_box("acme", 'b', false, 1)],
            &hosts,
            Some(config),
        )
        .await;
        let peers = MemoryPeers::new();
        let fake = peers.endpoint();
        fake.listen(Some(Arc::new(AdmitAll))).await.expect("listen");
        let desk = Arc::new(PairsAll::default());
        let answering = {
            let (fake, desk) = (fake.clone(), desk.clone());
            tokio::spawn(async move {
                while let Some(incoming) = fake.accept().await {
                    let _ = answer_pairing(incoming.connection, &*desk).await;
                }
            })
        };
        let ticket = Ticket {
            addr: fake.addr().await.expect("addr"),
            secret: TicketSecret([7; 16]),
            ephemeral: false,
        }
        .encode();
        let client =
            ClientSide::start_peer(&tmp, "client", "11975-11999", &[], peers.endpoint()).await;
        let rows = [
            (Some("ACME"), "box_name_taken", Some("p2p")),
            (Some("desk"), "box_name_taken", Some("ssh_or_command")),
            (Some("Lab"), "box_name_taken", Some("user_config")),
            (Some("dlp1lab"), "bad_request", None),
            (None, "box_name_taken", Some("ssh_or_command")),
        ];
        for (name, code, taken_by) in rows {
            let refused = client
                .h
                .call(
                    Method::Connect,
                    json!({"ticket": ticket, "name": name, "background": true}),
                    Context::default(),
                )
                .await
                .expect_err("paired under a taken name");
            assert_eq!(refused.code, code, "{name:?}: {refused:?}");
            if let Some(taken_by) = taken_by {
                assert_eq!(refused.data["taken_by"], taken_by, "{name:?}: {refused:?}");
            }
        }
        assert_eq!(
            desk.answered.load(Ordering::SeqCst),
            1,
            "only the derived name reached the box"
        );
        let store = Store::open(home.join("state.db")).await.expect("store");
        let names: Vec<String> = store
            .p2p_boxes()
            .await
            .expect("boxes")
            .into_iter()
            .map(|paired| paired.name)
            .collect();
        assert_eq!(names, ["acme"], "no pairing was kept");
        store.close().await.expect("close");
        answering.abort();
        client.stop().await;
    })
    .await
    .expect("timeout");
}
