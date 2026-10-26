//! `host.list` as the TUI's box view reads it: each connected box's registry
//! and mirror view, and `remote.mirror.changed` when they change. A child of
//! `remote`, whose box and client harness it reuses.

use devlanes_proto::{Event, Method, remote::MIRROR_EVENT};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::{BOX, BoxSide, ClientSide, SOON, box_port, ipv6_loopback};
use crate::{LIMIT, cx, eventually, repo, temp};

// Port ranges: 19000-19499, below the default pool, which no other test in
// this binary uses (see `remote_boxes.rs`, `remote_lifecycle.rs` and
// `remote_report.rs`, which takes 18000-18999).

/// Waits for the next `remote.mirror.changed` of box `host`; false when none
/// came within [`SOON`].
async fn next_change(events: &mut broadcast::Receiver<Event>, host: &str) -> bool {
    let wait = async {
        loop {
            match events.recv().await {
                Ok(event) if event.event == MIRROR_EVENT && event.data["host"] == host => {
                    return true;
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return false,
            }
        }
    };
    tokio::time::timeout(SOON, wait).await.unwrap_or(false)
}

/// The registry unit of `host` named `name`.
fn unit<'a>(host: &'a Value, name: &str) -> Option<&'a Value> {
    host["registry"]["units"]
        .as_array()?
        .iter()
        .find(|unit| unit["key"]["name"] == name)
}

/// The mirrored port `port` of `host`.
fn mirrored(host: &Value, port: u16) -> Option<&Value> {
    host["ports"]
        .as_array()?
        .iter()
        .find(|entry| entry["port"] == port)
}

#[tokio::test(flavor = "multi_thread")]
/// `host.list` carries the box's registry (hidden and idle units included),
/// ports with their mirror state, routes with the URL to open here and their
/// state, the box's handshake info, when the link came up and the names it
/// may route; a unit starting on the box is announced with
/// `remote.mirror.changed`, after which `host.list` shows it mirrored.
async fn host_list_carries_the_registry_and_mirror_view() {
    if !ipv6_loopback().await {
        eprintln!("skipping host_list_carries_the_registry_and_mirror_view: no IPv6 loopback");
        return;
    }
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "viewrepo", BOX);
        let side = BoxSide::start(&tmp, "box", "19000-19249").await;
        side.h
            .call(
                Method::Start,
                json!({"units": ["web", "secret"]}),
                cx(&main),
            )
            .await
            .expect("box start");
        let (web, echo) = (
            box_port(&side.h, 0, "web").await,
            box_port(&side.h, 0, "echo").await,
        );
        let client = ClientSide::start(&tmp, "client", "19250-19499", &side).await;
        let mut events = client.h.engine.subscribe();
        client.connect("box").await;

        let host = eventually(SOON, || async {
            let host = client.host().await;
            let bound = mirrored(&host, web).is_some_and(|p| p["state"] == "bound");
            let routed = host["routes"].as_array().is_some_and(|routes| {
                routes
                    .iter()
                    .any(|r| r["host"] == "web.boxapp.localhost" && r["state"] == "ready")
            });
            (bound && routed).then_some(host)
        })
        .await
        .expect("web mirrored and routed");
        assert!(next_change(&mut events, "box").await, "connect announced");

        let lanes = host["registry"]["lanes"]
            .as_array()
            .expect("registry lanes");
        assert_eq!(lanes.len(), 1, "{host}");
        assert_eq!(lanes[0]["key"]["project"], "boxapp");
        let web_unit = unit(&host, "web").expect("web listed");
        assert!(
            matches!(web_unit["state"].as_str(), Some("ready" | "running")),
            "{web_unit}"
        );
        assert_eq!(web_unit["hosts"][0]["host"], "web.boxapp.localhost");
        assert_eq!(
            unit(&host, "secret").expect("secret listed")["hidden"],
            true
        );
        assert!(
            mirrored(&host, box_port(&side.h, 0, "secret").await).is_none(),
            "a hidden unit is listed, never mirrored: {host}"
        );
        let idle = unit(&host, "echo").expect("echo listed");
        assert_eq!(idle["ports"][0]["port"], echo, "{idle}");
        assert!(mirrored(&host, echo).is_none(), "an idle unit is not bound");
        // This client has no HTTPS port, so the URL names none.
        let route = host["routes"]
            .as_array()
            .and_then(|routes| routes.iter().find(|r| r["host"] == "web.boxapp.localhost"))
            .expect("web route");
        assert_eq!(route["url"], "https://web.boxapp.localhost", "{route}");
        assert_eq!(host["box"]["ports_range"], "19000-19249", "{host}");
        assert!(host["connected_since_ms"].is_u64(), "{host}");
        assert!(
            host["suffixes"]
                .as_array()
                .is_some_and(|s| s.iter().any(|s| s == "localhost")),
            "{host}"
        );

        // A unit that starts on the box is announced, and then listed as mirrored.
        side.h
            .call(Method::Start, json!({"units": ["echo"]}), cx(&main))
            .await
            .expect("box start echo");
        let mut announced = false;
        for _ in 0..20 {
            if !next_change(&mut events, "box").await {
                break;
            }
            let host = client.host().await;
            if mirrored(&host, echo).is_some_and(|p| p["state"] == "bound") {
                announced = true;
                break;
            }
        }
        assert!(
            announced,
            "echo announced and mirrored: {}",
            client.host().await
        );

        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}
