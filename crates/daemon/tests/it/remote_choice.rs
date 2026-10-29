//! Choosing which box serves a lane several boxes run (`remote.use`), and the
//! switch between boxes. A child of `remote`, whose box and client harness it
//! reuses.
//!
//! Both forks of a test project use the same port numbers, as forks of one
//! image do. Box and client share this machine, so at most `alpha` runs real
//! servers on those numbers. A fork that doesn't lists them on units that
//! bind nothing, and its tunnel dials its own servers on other ports instead
//! ([`MapDialer`]).

use std::{
    collections::BTreeMap,
    net::Ipv6Addr,
    sync::{Arc, Mutex},
    time::Duration,
};

use devlanes_daemon::{Handler, Session};
use devlanes_ipc::Frame;
use devlanes_proto::{Context, Control, Method, Request};
use devlanes_state::Store;
use devlanes_transport::{AuthorizedTarget, BoxDuplex, Dialer};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::mpsc,
    task::JoinHandle,
};

use super::{BOX, BoxSide, ClientSide, SOON, box_port, http_get, ipv6_loopback};
use crate::{LIMIT, Temp, cx, eventually, repo, temp};

// Port ranges: 13000-14999, below the DNS server's 15353 and the other remote
// tests, and 26000-26999, which no other test in this binary uses. Each test
// takes its own part.

/// The hostnames the fork project routes.
const FORK_HOSTS: [&str; 2] = ["web.forkapp.localhost", "docs.forkapp.localhost"];

/// Project `forkapp` as fork `fork` runs it, with `web` on port `web` and
/// `docs` on port `docs`. Each answers with its name and the fork's, keeps
/// connections open and takes WebSockets. With `serves`, the services listen
/// on those numbers. Without, they bind nothing, and the hidden `real-web`
/// and `real-docs` answer for them on ports of their own.
fn fork_project(fork: &str, web: u16, docs: u16, serves: bool) -> String {
    let mut config = String::from("project: forkapp\nservices:\n");
    for (name, port) in [("web", web), ("docs", docs)] {
        let run = if serves {
            format!("\"http-keep {name}-from-{fork}\"")
        } else {
            "sleep".to_owned()
        };
        config.push_str(&format!(
            "  {name}:\n    run: {run}\n    hosts: [\"{name}.forkapp.localhost\"]\n    env:\n      PORT: \"{port}\"\n"
        ));
        if !serves {
            config.push_str(&format!(
                "  real-{name}:\n    run: \"http-keep {name}-from-{fork}\"\n    hidden: true\n"
            ));
        }
    }
    config
}

/// A box dialer that dials another port on this machine for the ports in
/// its map, and the port itself otherwise.
#[derive(Default)]
struct MapDialer(Mutex<BTreeMap<u16, u16>>);

#[async_trait::async_trait]
impl Dialer for MapDialer {
    /// Dials `127.0.0.1` at the mapped port.
    async fn dial(&self, target: &AuthorizedTarget) -> std::io::Result<BoxDuplex> {
        let port = self
            .0
            .lock()
            .ok()
            .and_then(|map| map.get(&target.port()).copied())
            .unwrap_or(target.port());
        Ok(Box::new(TcpStream::connect(("127.0.0.1", port)).await?))
    }
}

/// Two forks of `forkapp` with the box port ranges in `ranges`, `alpha`
/// serving `web` and `docs` itself and `beta` through its hidden servers.
async fn forks(tmp: &Temp, ranges: [&str; 2], web: u16, docs: u16) -> (BoxSide, BoxSide) {
    let configs = [
        fork_project("alpha", web, docs, true),
        fork_project("beta", web, docs, false),
    ];
    let mapped: [&[(u16, &str)]; 2] = [&[], &[(web, "real-web"), (docs, "real-docs")]];
    start_forks(tmp, ranges, configs, mapped).await
}

/// Boxes `alpha` and `beta` with the box port ranges in `ranges`, each
/// running its project of `configs`. A box with entries in `mapped`, as
/// `(port, hidden unit)`, dials each port at that unit's port instead.
async fn start_forks(
    tmp: &Temp,
    ranges: [&str; 2],
    configs: [String; 2],
    mapped: [&[(u16, &str)]; 2],
) -> (BoxSide, BoxSide) {
    let mut sides = Vec::new();
    for (index, name) in ["alpha", "beta"].into_iter().enumerate() {
        let project = repo(&tmp.path, &format!("fork{name}"), &configs[index]);
        let dialer = Arc::new(MapDialer::default());
        let side = if mapped[index].is_empty() {
            BoxSide::start(tmp, name, ranges[index]).await
        } else {
            BoxSide::start_with(tmp, name, ranges[index], Some(dialer.clone())).await
        };
        side.h
            .call(Method::Start, json!({}), cx(&project))
            .await
            .expect("box start");
        for (port, unit) in mapped[index] {
            let real = box_port(&side.h, 0, unit).await;
            dialer.0.lock().expect("map").insert(*port, real);
        }
        sides.push(side);
    }
    let beta = sides.pop().expect("beta");
    let alpha = sides.pop().expect("alpha");
    (alpha, beta)
}

/// How long a test waits for an event it expects.
const EVENT_WAIT: Duration = Duration::from_secs(10);

/// Subscribes to the client's events, as an attached CLI would, and returns
/// once the subscription is in place: every event from then on reaches
/// `frames`, and none from before does.
async fn subscribe(
    client: &ClientSide,
    id: u64,
) -> (Session, mpsc::Receiver<Frame>, JoinHandle<()>) {
    let (session, mut frames) = Session::local(id);
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
    // The first event a subscriber gets is its own resync.
    let subscribed = tokio::time::timeout(EVENT_WAIT, async {
        while let Some(frame) = frames.recv().await {
            if let Frame::Control(Control::Event(event)) = frame
                && event.event == devlanes_proto::events::RESYNC
            {
                return;
            }
        }
    })
    .await;
    assert!(subscribed.is_ok(), "the subscription starts");
    (session, frames, task)
}

/// The `remote.warning` events up to and including the first one `last`
/// accepts. Fails when it doesn't arrive within [`EVENT_WAIT`].
async fn warnings_until(
    frames: &mut mpsc::Receiver<Frame>,
    last: impl Fn(&Value) -> bool,
) -> Vec<Value> {
    let mut found = Vec::new();
    let arrived = tokio::time::timeout(EVENT_WAIT, async {
        while let Some(frame) = frames.recv().await {
            if let Frame::Control(Control::Event(event)) = frame
                && event.event == "remote.warning"
            {
                let done = last(&event.data);
                found.push(event.data);
                if done {
                    return true;
                }
            }
        }
        false
    })
    .await;
    assert_eq!(arrived, Ok(true), "the expected warning arrives: {found:?}");
    found
}

/// [`warnings_until`] a warning with `code` about `lane`.
async fn warnings_until_code(
    frames: &mut mpsc::Receiver<Frame>,
    code: &str,
    lane: &str,
) -> Vec<Value> {
    warnings_until(frames, |w| w["code"] == code && w["lane"] == lane).await
}

/// Asserts that no warning in `seen` has one of `codes`.
fn none_of(seen: &[Value], codes: &[&str]) {
    for code in codes {
        assert!(
            !seen.iter().any(|w| w["code"] == *code),
            "no {code}: {seen:?}"
        );
    }
}

/// Waits until none of `hosts` lists a port conflict. A mirror that gave a
/// port up reports it a moment after the switch, so one look could be early.
async fn settles_without_conflicts(client: &ClientSide, hosts: &[&str]) {
    let settled = eventually(SOON, || async {
        for host in hosts {
            if !no_port_conflicts(client, host).await {
                return None;
            }
        }
        Some(())
    })
    .await;
    if settled.is_none() {
        let mut entries = Vec::new();
        for host in hosts {
            entries.push(client.host_named(host).await);
        }
        panic!("port conflicts stay: {entries:?}");
    }
}

/// Whether no port box `host` lists has a conflict.
async fn no_port_conflicts(client: &ClientSide, host: &str) -> bool {
    client.host_named(host).await["ports"]
        .as_array()
        .is_none_or(|ports| {
            ports
                .iter()
                .all(|port| port["conflicts"].as_array().is_none_or(Vec::is_empty))
        })
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

/// The status line's code and the body of one GET through the proxy.
async fn status_and_body(proxy: u16, host: &str) -> (u16, String) {
    let response = http_get(proxy, host).await.unwrap_or_default();
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default();
    (status, body)
}

/// Sends GETs of `host` through the proxy one after another, collecting each
/// answer, until [`NEW_ANSWERS`] of them are `until`, or for 10 seconds at
/// most. `first` is notified once the first answer is in.
fn request_loop(
    proxy: u16,
    host: &'static str,
    until: &'static str,
    first: Arc<tokio::sync::Notify>,
) -> JoinHandle<Vec<(u16, String)>> {
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        let mut new = 0;
        while new < NEW_ANSWERS && tokio::time::Instant::now() < deadline {
            let answer = status_and_body(proxy, host).await;
            if answer.1 == until {
                new += 1;
            }
            seen.push(answer);
            if seen.len() == 1 {
                first.notify_one();
            }
        }
        seen
    })
}

/// How many answers from the new box a request loop waits for.
const NEW_ANSWERS: usize = 5;

/// Checks the answers of a [`request_loop`] across a switch from `from` to
/// `to`: all 200, from `from` until `to` answered, then from `to` only, and
/// [`NEW_ANSWERS`] of them from `to`.
fn switched_without_a_gap(seen: &[(u16, String)], from: &str, to: &str) {
    let first_new = seen
        .iter()
        .position(|(_, body)| body == to)
        .unwrap_or_else(|| panic!("{to} answered: {seen:?}"));
    for (index, (status, body)) in seen.iter().enumerate() {
        assert_eq!(*status, 200, "request {index}: {seen:?}");
        let expected = if index < first_new { from } else { to };
        assert_eq!(body, expected, "request {index}: {seen:?}");
    }
    assert_eq!(
        seen.iter().filter(|(_, body)| body == to).count(),
        NEW_ANSWERS,
        "{seen:?}"
    );
}

/// Asserts that once a table of `tables` routes every one of `hosts`, every
/// later table does too.
fn hosts_never_drop(tables: &[Vec<(String, u16)>], hosts: &[&str]) {
    let routes_all = |table: &Vec<(String, u16)>| {
        hosts
            .iter()
            .all(|host| table.iter().any(|(routed, _)| routed == host))
    };
    let Some(first) = tables.iter().position(routes_all) else {
        return;
    };
    for (index, table) in tables.iter().enumerate().skip(first) {
        assert!(
            routes_all(table),
            "table {index} drops a hostname: {tables:?}"
        );
    }
}

/// One GET on a kept-alive connection `stream`, which must answer `body`.
async fn keep_alive_get(stream: &mut BufReader<TcpStream>, body: &str) {
    stream
        .get_mut()
        .write_all(b"GET / HTTP/1.1\r\nHost: web.forkapp.localhost\r\n\r\n")
        .await
        .expect("request");
    let mut length = 0;
    loop {
        let mut line = String::new();
        stream.read_line(&mut line).await.expect("head");
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().expect("length");
        }
        if line == "\r\n" {
            break;
        }
    }
    let mut answer = vec![0u8; length];
    stream.read_exact(&mut answer).await.expect("body");
    assert_eq!(String::from_utf8_lossy(&answer), body);
}

/// A WebSocket through the proxy to `host`, with its greeting line.
async fn websocket(proxy: u16, host: &str) -> (BufReader<TcpStream>, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", proxy))
        .await
        .expect("proxy");
    let request = format!(
        "GET /ws HTTP/1.1\r\nHost: {host}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.expect("upgrade");
    let mut stream = BufReader::new(stream);
    let mut status = String::new();
    stream.read_line(&mut status).await.expect("status");
    assert!(status.contains("101"), "{status}");
    loop {
        let mut line = String::new();
        stream.read_line(&mut line).await.expect("head");
        if line == "\r\n" {
            break;
        }
    }
    let mut greeting = String::new();
    stream.read_line(&mut greeting).await.expect("greeting");
    (stream, greeting.trim().to_owned())
}

/// Whether the other end closes `stream` within `wait`.
async fn closes(stream: &mut BufReader<TcpStream>, wait: Duration) -> bool {
    let mut rest = Vec::new();
    matches!(
        tokio::time::timeout(wait, stream.read_to_end(&mut rest)).await,
        Ok(Ok(_) | Err(_))
    )
}

#[tokio::test(flavor = "multi_thread")]
/// `remote use` moves a lane from the box connected first to the other and
/// back. Requests in a loop through the proxy never fail or miss a route,
/// and once the reply is there the next request reaches the new box. The
/// lane's port is never unbound, a raw TCP connection, an idle pooled
/// connection and a WebSocket to the old box close, and nothing warns about
/// the switch, which one `lane_switched` notice announces. Each refusal has
/// its code, a disconnected box's choice waits until it connects, and
/// `remote disconnect` deletes the choice.
async fn remote_use_switches_a_lane_between_boxes() {
    if !ipv6_loopback().await {
        eprintln!("skipping remote_use_switches_a_lane_between_boxes: no IPv6 loopback");
        return;
    }
    const WEB: u16 = 14_490;
    const DOCS: u16 = 14_491;
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let (alpha, beta) = forks(&tmp, ["14000-14199", "14200-14399"], WEB, DOCS).await;
        let client = ClientSide::start_with(
            &tmp,
            "client",
            "14400-14489",
            &[("alpha", &alpha), ("beta", &beta)],
            None,
        )
        .await;
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
            binds(&client, "alpha", WEB).await.then_some(())
        })
        .await;
        assert!(bound.is_some(), "{}", client.host_named("alpha").await);
        let (session, mut frames, events) = subscribe(&client, 9_201).await;

        for (lane, host, code) in [
            ("forkapp@0", Some("gamma"), "box_unknown"),
            ("forkapp@7", Some("beta"), "lane_not_on_box"),
            ("forkapp@0", None, "local_lane_unknown"),
        ] {
            assert_eq!(use_lane(&client, lane, host).await, Err(code.to_owned()));
        }

        // Open across the switch: a raw TCP connection through the lane's
        // listener, a WebSocket through the proxy, and the proxy's pooled
        // connection from the request just before.
        let raw = TcpStream::connect((Ipv6Addr::LOCALHOST, WEB))
            .await
            .expect("connect to the mirrored port");
        let mut raw = BufReader::new(raw);
        keep_alive_get(&mut raw, "web-from-alpha").await;
        let (mut socket, greeting) = websocket(client.proxy_port, FORK_HOSTS[0]).await;
        assert_eq!(greeting, "web-from-alpha");
        assert!(answers(&client, FORK_HOSTS[1], "docs-from-alpha").await);
        for host in ["alpha", "beta"] {
            assert!(
                no_port_conflicts(&client, host).await,
                "{}",
                client.host_named(host).await
            );
        }

        let first = Arc::new(tokio::sync::Notify::new());
        let looping = request_loop(
            client.proxy_port,
            FORK_HOSTS[0],
            "web-from-beta",
            first.clone(),
        );
        first.notified().await;
        client.published.lock().expect("published").clear();
        let reply = use_lane(&client, "forkapp@0", Some("beta")).await;
        assert_eq!(
            reply,
            Ok(
                json!({"lane": "forkapp@0", "host": "beta", "previous": "alpha",
                      "mirrored": true, "pending": false, "replaced": []})
            )
        );
        // The pooled connection to alpha is not reused for the next request.
        assert!(answers(&client, FORK_HOSTS[1], "docs-from-beta").await);
        let seen = looping.await.expect("request loop");
        switched_without_a_gap(&seen, "web-from-alpha", "web-from-beta");
        // Every table published during the switch routes both hostnames, to
        // the same local port: only the box behind it changed.
        let tables = client.published.lock().expect("published").clone();
        assert!(!tables.is_empty());
        for table in &tables {
            let ports: Vec<u16> = FORK_HOSTS
                .iter()
                .map(|host| {
                    table
                        .iter()
                        .find(|(routed, _)| routed == host)
                        .map(|(_, port)| *port)
                        .unwrap_or_else(|| panic!("{host} missing from {tables:?}"))
                })
                .collect();
            assert_eq!(ports.len(), FORK_HOSTS.len());
        }
        let first_ports: Vec<&(String, u16)> = tables[0].iter().collect();
        assert!(
            tables
                .iter()
                .all(|table| table.iter().collect::<Vec<_>>() == first_ports),
            "the route targets never change: {tables:?}"
        );
        assert!(
            binds(&client, "beta", WEB).await,
            "{}",
            client.host_named("beta").await
        );
        assert!(
            !binds(&client, "alpha", WEB).await,
            "{}",
            client.host_named("alpha").await
        );
        settles_without_conflicts(&client, &["alpha", "beta"]).await;
        assert!(
            closes(&mut raw, Duration::from_secs(6)).await,
            "the raw connection to alpha closes"
        );
        assert!(
            closes(&mut socket, Duration::from_secs(6)).await,
            "the WebSocket to alpha closes"
        );
        let (_socket, greeting) = websocket(client.proxy_port, FORK_HOSTS[0]).await;
        assert_eq!(
            greeting, "web-from-beta",
            "a reconnecting WebSocket reaches beta"
        );
        let mut raw = BufReader::new(
            TcpStream::connect((Ipv6Addr::LOCALHOST, WEB))
                .await
                .expect("connect again"),
        );
        keep_alive_get(&mut raw, "web-from-beta").await;

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

        // Back to alpha, which connected first, so its mirror is woken first.
        let reply = use_lane(&client, "forkapp@0", Some("alpha")).await;
        assert_eq!(
            reply.map(|reply| reply["previous"].clone()),
            Ok(json!("beta"))
        );
        assert!(answers(&client, FORK_HOSTS[0], "web-from-alpha").await);
        assert!(
            binds(&client, "alpha", WEB).await,
            "{}",
            client.host_named("alpha").await
        );
        settles_without_conflicts(&client, &["alpha", "beta"]).await;

        // `remote.use` announces a switch after the mirrors applied it, so
        // whatever they said about it came first.
        let mut seen = warnings_until_code(&mut frames, "lane_switched", "forkapp@0").await;
        seen.extend(warnings_until_code(&mut frames, "lane_switched", "forkapp@0").await);
        none_of(&seen, &["port_in_use", "lane_skipped", "open_failed"]);
        let notices: Vec<&Value> = seen
            .iter()
            .filter(|w| w["code"] == "lane_switched")
            .collect();
        assert_eq!(notices.len(), 2, "{seen:?}");
        assert_eq!(notices[0]["message"], "forkapp@0 now comes from beta");
        assert_eq!(
            (&notices[0]["from"], &notices[0]["to"]),
            (&json!("alpha"), &json!("beta"))
        );

        // A disconnected box keeps a choice, which applies once it connects.
        super::boxes::disconnect(&client, "beta").await;
        assert_eq!(client.host_named("beta").await["chosen_lanes"], json!([]));
        let reply = use_lane(&client, "forkapp@0", Some("beta")).await;
        assert_eq!(
            reply,
            Ok(
                json!({"lane": "forkapp@0", "host": "beta", "previous": "alpha",
                      "mirrored": false, "pending": true, "replaced": []})
            )
        );
        assert!(answers(&client, FORK_HOSTS[0], "web-from-alpha").await);
        client.published.lock().expect("published").clear();
        client.connect("beta").await;
        let taken = eventually(SOON, || async {
            answers(&client, FORK_HOSTS[0], "web-from-beta")
                .await
                .then_some(())
        })
        .await;
        assert!(
            taken.is_some(),
            "the chosen box takes the lane once it connects"
        );
        // Beta only wins once its routes are built, so alpha's stay until
        // then and the hostnames never lead nowhere.
        let tables = client.published.lock().expect("published").clone();
        assert!(
            tables
                .first()
                .is_some_and(|table| table.len() == FORK_HOSTS.len()),
            "{tables:?}"
        );
        hosts_never_drop(&tables, &FORK_HOSTS);

        // A disconnect deletes the choice: alpha serves again.
        super::boxes::disconnect(&client, "beta").await;
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
/// With this machine mirroring on `127.0.0.1` too, both forks' routes name
/// the lane's port itself. A switch keeps every route as it is, requests in
/// a loop see no gap, and a raw connection to the port reaches the new box.
/// Neither fork binds the numbers here: both answer through their hidden
/// servers.
async fn a_switch_keeps_routes_that_name_the_port() {
    if !ipv6_loopback().await {
        eprintln!("skipping a_switch_keeps_routes_that_name_the_port: no IPv6 loopback");
        return;
    }
    const WEB: u16 = 26_490;
    const DOCS: u16 = 26_491;
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let mapped: &[(u16, &str)] = &[(WEB, "real-web"), (DOCS, "real-docs")];
        let configs = [
            fork_project("alpha", WEB, DOCS, false),
            fork_project("beta", WEB, DOCS, false),
        ];
        let (alpha, beta) = start_forks(
            &tmp,
            ["26000-26199", "26200-26399"],
            configs,
            [mapped, mapped],
        )
        .await;
        let controls = Arc::new(super::Controls::default());
        controls
            .both_loopbacks
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let boxes = [("alpha", &alpha), ("beta", &beta)];
        let client =
            ClientSide::start_full(&tmp, "client", "26400-26489", &boxes, None, controls).await;
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
        assert!(skip.is_some(), "beta's lane is skipped");
        let direct = [
            (FORK_HOSTS[0].to_owned(), WEB),
            (FORK_HOSTS[1].to_owned(), DOCS),
        ];
        let named = |table: &Vec<(String, u16)>| direct.iter().all(|route| table.contains(route));
        let ported = eventually(SOON, || async {
            let tables = client.published.lock().expect("published").clone();
            tables.last().filter(|table| named(table)).map(|_| ())
        })
        .await;
        assert!(
            ported.is_some(),
            "the routes name the ports: {:?}",
            client.published.lock().expect("published").last()
        );
        let (session, mut frames, events) = subscribe(&client, 9_205).await;

        let first = Arc::new(tokio::sync::Notify::new());
        let looping = request_loop(
            client.proxy_port,
            FORK_HOSTS[0],
            "web-from-beta",
            first.clone(),
        );
        first.notified().await;
        client.published.lock().expect("published").clear();
        let reply = use_lane(&client, "forkapp@0", Some("beta")).await;
        assert_eq!(
            reply.map(|reply| reply["mirrored"].clone()),
            Ok(json!(true))
        );
        let seen = looping.await.expect("request loop");
        switched_without_a_gap(&seen, "web-from-alpha", "web-from-beta");
        let tables = client.published.lock().expect("published").clone();
        assert!(tables.iter().all(named), "{tables:?}");
        let mut raw = BufReader::new(
            TcpStream::connect(("127.0.0.1", WEB))
                .await
                .expect("connect to the port"),
        );
        keep_alive_get(&mut raw, "web-from-beta").await;
        let seen = warnings_until_code(&mut frames, "lane_switched", "forkapp@0").await;
        none_of(&seen, &["port_in_use", "lane_skipped", "open_failed"]);

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
/// The same lane on two boxes clashes without any hostname: `dbapp@0` only
/// has a port. The box connected first serves it, the other one's lane is
/// skipped instead of competing for the port, and `remote use` moves the
/// port without a `port_in_use` or a `lane_skipped`.
async fn a_lane_without_hostnames_switches_by_its_port() {
    if !ipv6_loopback().await {
        eprintln!("skipping a_lane_without_hostnames_switches_by_its_port: no IPv6 loopback");
        return;
    }
    const DB: u16 = 26_790;
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let configs = [
            format!(
                "project: dbapp\nservices:\n  db:\n    run: \"http-keep db-from-alpha\"\n    env:\n      PORT: \"{DB}\"\n"
            ),
            format!(
                "project: dbapp\nservices:\n  db:\n    run: sleep\n    env:\n      PORT: \"{DB}\"\n  real-db:\n    run: \"http-keep db-from-beta\"\n    hidden: true\n"
            ),
        ];
        let (alpha, beta) = start_forks(
            &tmp,
            ["26500-26599", "26600-26699"],
            configs,
            [&[], &[(DB, "real-db")]],
        )
        .await;
        let boxes = [("alpha", &alpha), ("beta", &beta)];
        let client = ClientSide::start_with(&tmp, "client", "26700-26789", &boxes, None).await;
        client.connect("alpha").await;
        let bound = eventually(SOON, || async {
            binds(&client, "alpha", DB).await.then_some(())
        })
        .await;
        assert!(bound.is_some(), "{}", client.host_named("alpha").await);
        client.connect("beta").await;
        let skip = eventually(SOON, || skipped(&client, "beta", "dbapp@0")).await;
        assert_eq!(
            skip.map(|skip| skip["cause"].clone()),
            Some(json!("box_earlier"))
        );
        // The port went to alpha only, and nothing competes for it.
        for host in ["alpha", "beta"] {
            assert!(
                no_port_conflicts(&client, host).await,
                "{}",
                client.host_named(host).await
            );
        }
        let (session, mut frames, events) = subscribe(&client, 9_206).await;
        let mut raw = BufReader::new(
            TcpStream::connect((Ipv6Addr::LOCALHOST, DB))
                .await
                .expect("connect to the port"),
        );
        keep_alive_get(&mut raw, "db-from-alpha").await;

        let reply = use_lane(&client, "dbapp@0", Some("beta")).await;
        assert_eq!(
            reply.map(|reply| reply["mirrored"].clone()),
            Ok(json!(true))
        );
        assert!(binds(&client, "beta", DB).await, "{}", client.host_named("beta").await);
        assert!(
            closes(&mut raw, Duration::from_secs(6)).await,
            "the connection to alpha closes"
        );
        let mut raw = BufReader::new(
            TcpStream::connect((Ipv6Addr::LOCALHOST, DB))
                .await
                .expect("connect again"),
        );
        keep_alive_get(&mut raw, "db-from-beta").await;
        let seen = warnings_until_code(&mut frames, "lane_switched", "dbapp@0").await;
        none_of(&seen, &["port_in_use", "lane_skipped"]);

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
/// hostnames back. Starting the local lane takes the hostnames back as soon
/// as the start passed its checks, before any service spawns, and deletes
/// the choice with `choice_cleared` once something runs, here its
/// `lane.start` task. While it runs a choice is refused with
/// `local_lane_running`.
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
        // A task that waits for the test holds the start before its services.
        let local_config = format!(
            "{}tasks:\n  warmup: {{ run: hold, when: lane.start }}\n",
            BOX.replace("hello-from-box", "hello-from-local")
        );
        let local_repo = repo(&tmp.path, "localrepo", &local_config);
        let client = ClientSide::start(&tmp, "client", "13750-13999", &side).await;
        client
            .h
            .call(Method::LaneList, json!({}), cx(&local_repo))
            .await
            .expect("local lane list");
        let (session, mut frames, events) = subscribe(&client, 9_202).await;
        client.connect("box").await;
        // The first skip is said.
        warnings_until_code(&mut frames, "lane_skipped", "boxapp@0").await;
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
            Ok(json!({"lane": "boxapp@0", "host": "box", "previous": null,
                      "mirrored": true, "pending": false, "replaced": []}))
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
            Ok(json!({"lane": "boxapp@0", "host": null, "previous": "box",
                      "mirrored": true, "pending": false, "replaced": []}))
        );
        assert!(!answers(&client, "web.boxapp.localhost", "hello-from-box").await);
        assert_eq!(client.host().await["chosen_lanes"], json!([]));

        assert!(use_lane(&client, "boxapp@0", Some("box")).await.is_ok());
        let engine = client.h.engine.clone();
        let context = cx(&local_repo);
        let start = tokio::spawn(async move {
            let request = Request {
                id: 77,
                method: Method::Start,
                params: json!({"units": ["web"]}),
                context,
            };
            let (session, _frames) = Session::local(9_203);
            engine.handle(request, session).await
        });
        // The task the start runs counts as something running.
        let cleared = eventually(SOON, || async {
            (client.host().await["chosen_lanes"] == json!([])).then_some(())
        })
        .await;
        assert!(
            cleared.is_some(),
            "the start clears the choice before any service runs"
        );
        assert!(
            !answers(&client, "web.boxapp.localhost", "hello-from-box").await,
            "the box lane is not routed while the start waits"
        );
        client.h.runner.release();
        start.await.expect("start task").expect("local start");
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
        assert_eq!(
            use_lane(&client, "boxapp@0", Some("box")).await,
            Err("local_lane_running".to_owned())
        );
        let seen = warnings_until_code(&mut frames, "choice_cleared", "boxapp@0").await;
        let cleared = seen.last().expect("choice_cleared");
        assert_eq!(cleared["host"], "box", "{cleared}");
        assert_eq!(
            cleared["message"],
            "boxapp@0 started on this machine, so box's boxapp@0 is no longer mirrored or chosen."
        );
        // `--local` and the cleared choice were said by their own notices.
        none_of(&seen, &["lane_skipped"]);

        // A box that is not connected is refused the same while the lane
        // runs here, and once it is stopped the choice waits for the box.
        super::boxes::disconnect(&client, "box").await;
        assert_eq!(
            use_lane(&client, "boxapp@0", Some("box")).await,
            Err("local_lane_running".to_owned())
        );
        client
            .h
            .call(Method::Stop, json!({}), cx(&local_repo))
            .await
            .expect("local stop");
        let reply = use_lane(&client, "boxapp@0", Some("box")).await;
        assert_eq!(reply.map(|reply| reply["pending"].clone()), Ok(json!(true)));
        assert_eq!(client.host().await["chosen_lanes"], json!(["boxapp@0"]));
        // Starting the lane deletes that choice too, connected or not.
        client
            .h
            .call(Method::Start, json!({"units": ["web"]}), cx(&local_repo))
            .await
            .expect("local start again");
        assert_eq!(client.host().await["chosen_lanes"], json!([]));
        let seen = warnings_until_code(&mut frames, "choice_cleared", "boxapp@0").await;
        assert_eq!(seen.last().map(|w| w["host"].clone()), Some(json!("box")));
        let stored = eventually(SOON, || async {
            let home = client.h.home.join("state.db");
            let store = Store::open(home).await.ok()?;
            let choices = store.remote_lane_choices().await.ok()?;
            store.close().await.ok()?;
            choices.is_empty().then_some(())
        })
        .await;
        assert!(stored.is_some(), "the store loses the cleared choice");

        session.close();
        let _ = events.await;
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A local start whose infra fails before any container runs holds the lane
/// only while it runs: the chosen box lane is mirrored again afterwards, and
/// its choice kept. One that fails with a container of the lane up anyway
/// clears the choice and says the containers did. A start that ends with
/// only the lane's containers up gets a new choice refused with
/// `holds: containers` and the Compose project that stops them.
async fn a_failed_start_gives_the_box_lane_back() {
    if !ipv6_loopback().await {
        eprintln!("skipping a_failed_start_gives_the_box_lane_back: no IPv6 loopback");
        return;
    }
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let box_repo = repo(&tmp.path, "boxrepo", BOX);
        let side = BoxSide::start(&tmp, "box", "26800-26899").await;
        side.h
            .call(Method::Start, json!({"units": ["web"]}), cx(&box_repo))
            .await
            .expect("box start");
        let local_repo = repo(
            &tmp.path,
            "localrepo",
            "project: boxapp\ninfra:\n  containers:\n    db: { image: \"postgres:16\", ports: { pg: 5432 }, per_lane: true }\nservices:\n  web:\n    run: fail\n    hosts: [\"web.${{ lane.domain }}\"]\n",
        );
        let client = ClientSide::start(&tmp, "client", "26900-26999", &side).await;
        client
            .h
            .call(Method::LaneList, json!({}), cx(&local_repo))
            .await
            .expect("local lane list");
        client.connect("box").await;
        let skip = eventually(SOON, || skipped(&client, "box", "boxapp@0")).await;
        assert!(skip.is_some(), "the stopped local lane wins first");
        assert!(use_lane(&client, "boxapp@0", Some("box")).await.is_ok());
        assert!(answers(&client, "web.boxapp.localhost", "hello-from-box").await);

        *client.h.infra.fail_up.lock().expect("lock") = Some(
            devlanes_daemon::integrations::InfraError::new("docker_failed", "docker is down"),
        );
        *client.h.infra.namespace_db_state.lock().expect("lock") = Some("exited 1".into());
        let failed = client
            .h
            .call(Method::Start, json!({"units": ["web"]}), cx(&local_repo))
            .await;
        assert!(failed.is_err(), "{failed:?}");
        let back = eventually(SOON, || async {
            answers(&client, "web.boxapp.localhost", "hello-from-box")
                .await
                .then_some(())
        })
        .await;
        assert!(back.is_some(), "the box lane is mirrored again");
        assert_eq!(client.host().await["chosen_lanes"], json!(["boxapp@0"]));

        // `infra up` fails partway, with the lane's db up anyway.
        let (session, mut frames, events) = subscribe(&client, 9_207).await;
        *client.h.infra.namespace_db_state.lock().expect("lock") = None;
        let failed = client
            .h
            .call(Method::InfraUp, json!({}), cx(&local_repo))
            .await;
        assert!(failed.is_err(), "{failed:?}");
        let seen = warnings_until_code(&mut frames, "choice_cleared", "boxapp@0").await;
        assert_eq!(
            seen.last().map(|w| w["message"].clone()),
            Some(json!(
                "boxapp@0's containers run on this machine, so box's boxapp@0 is no longer mirrored or chosen. Stop them, then run devlanes remote use box boxapp@0 to switch back."
            ))
        );
        assert_eq!(client.host().await["chosen_lanes"], json!([]));
        assert!(!answers(&client, "web.boxapp.localhost", "hello-from-box").await);

        *client.h.infra.fail_up.lock().expect("lock") = None;
        let ended = client
            .h
            .call(Method::Start, json!({"units": ["web"]}), cx(&local_repo))
            .await
            .expect("a start whose service fails");
        assert_eq!(ended["ready"], false, "{ended}");
        let refused = client
            .h
            .call(
                Method::RemoteUse,
                json!({"lane": "boxapp@0", "host": "box"}),
                Context::default(),
            )
            .await
            .expect_err("the containers hold the lane");
        assert_eq!(refused.code, "local_lane_running");
        assert_eq!(refused.data["holds"], "containers", "{:?}", refused.data);
        assert_eq!(refused.data["compose_project"], "boxapp-lane0");
        assert_eq!(
            refused.data["message"],
            "boxapp@0's containers run on this machine. Stop them with docker compose -p boxapp-lane0 stop. Then run devlanes remote use box boxapp@0."
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
/// A start whose only service exits at once still clears the choice it
/// beats, and a daemon that restarts with the lane's container up knows it:
/// a new choice is refused with `holds: containers`.
async fn what_ran_counts_whatever_the_timing() {
    if !ipv6_loopback().await {
        eprintln!("skipping what_ran_counts_whatever_the_timing: no IPv6 loopback");
        return;
    }
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let box_repo = repo(&tmp.path, "boxrepo", BOX);
        let side = BoxSide::start(&tmp, "box", "13400-13449").await;
        side.h
            .call(Method::Start, json!({"units": ["web"]}), cx(&box_repo))
            .await
            .expect("box start");
        let local_repo = repo(
            &tmp.path,
            "localrepo",
            "project: boxapp\nservices:\n  web:\n    run: fail\n    hosts: [\"web.${{ lane.domain }}\"]\n",
        );
        let client = ClientSide::start(&tmp, "client", "13450-13499", &side).await;
        client
            .h
            .call(Method::LaneList, json!({}), cx(&local_repo))
            .await
            .expect("local lane list");
        client.connect("box").await;
        let skip = eventually(SOON, || skipped(&client, "box", "boxapp@0")).await;
        assert!(skip.is_some(), "the stopped local lane wins first");
        assert!(use_lane(&client, "boxapp@0", Some("box")).await.is_ok());
        let ended = client
            .h
            .call(Method::Start, json!({"units": ["web"]}), cx(&local_repo))
            .await
            .expect("a start whose service exits");
        assert_eq!(ended["ready"], false, "{ended}");
        assert_eq!(
            client.host().await["chosen_lanes"],
            json!([]),
            "the service ran, so the choice is gone"
        );

        // The lane gets infra, whose container the next daemon finds up.
        std::fs::write(
            local_repo.join("devlanes.yaml"),
            "project: boxapp\ninfra:\n  containers:\n    db: { image: \"postgres:16\", ports: { pg: 5432 }, per_lane: true }\nservices:\n  web:\n    run: fail\n    hosts: [\"web.${{ lane.domain }}\"]\n",
        )
        .expect("config");
        client.stop().await;
        let client = ClientSide::start(&tmp, "client", "13450-13499", &side).await;
        let refused = eventually(SOON, || async {
            client
                .h
                .call(
                    Method::RemoteUse,
                    json!({"lane": "boxapp@0", "host": "box"}),
                    Context::default(),
                )
                .await
                .err()
                .filter(|error| error.code == "local_lane_running")
        })
        .await
        .expect("the restarted daemon knows the container holds the lane");
        assert_eq!(refused.data["holds"], "containers", "{:?}", refused.data);
        assert_eq!(refused.data["compose_project"], "boxapp-lane0");

        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// `remote use --local` takes this machine's lane's hostnames back from a
/// chosen lane of another project, deletes that choice and names it in
/// `replaced`. Nothing says the box lane is skipped again: `lane_switched`
/// did.
async fn local_takes_its_hostnames_back_from_another_projects_choice() {
    if !ipv6_loopback().await {
        eprintln!(
            "skipping local_takes_its_hostnames_back_from_another_projects_choice: no IPv6 loopback"
        );
        return;
    }
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let box_repo = repo(&tmp.path, "boxrepo", BOX);
        let side = BoxSide::start(&tmp, "box", "13000-13199").await;
        side.h
            .call(Method::Start, json!({"units": ["web"]}), cx(&box_repo))
            .await
            .expect("box start");
        let shop = repo(
            &tmp.path,
            "shoprepo",
            "project: shop\nservices:\n  web:\n    run: \"http hello-from-shop\"\n    hosts: [\"web.boxapp.localhost\"]\n",
        );
        let client = ClientSide::start(&tmp, "client", "13200-13399", &side).await;
        client
            .h
            .call(Method::LaneList, json!({}), cx(&shop))
            .await
            .expect("local lane list");
        client.connect("box").await;
        let skip = eventually(SOON, || skipped(&client, "box", "boxapp@0")).await;
        assert_eq!(
            skip.map(|skip| (skip["cause"].clone(), skip["winner_lane"].clone())),
            Some((json!("local_stopped"), json!("shop@0")))
        );
        let (session, mut frames, events) = subscribe(&client, 9_204).await;

        assert!(use_lane(&client, "boxapp@0", Some("box")).await.is_ok());
        assert!(answers(&client, "web.boxapp.localhost", "hello-from-box").await);
        let reply = use_lane(&client, "shop@0", None).await;
        assert_eq!(
            reply,
            Ok(json!({"lane": "shop@0", "host": null, "previous": "box",
                      "mirrored": true, "pending": false,
                      "replaced": [{"lane": "boxapp@0", "host": "box"}]}))
        );
        assert_eq!(client.host().await["chosen_lanes"], json!([]));
        assert!(!answers(&client, "web.boxapp.localhost", "hello-from-box").await);
        let skip = skipped(&client, "box", "boxapp@0").await.expect("skipped");
        assert_eq!(skip["cause"], "local_stopped", "{skip}");
        let seen = warnings_until_code(&mut frames, "lane_switched", "shop@0").await;
        none_of(&seen, &["lane_skipped"]);

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
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let (alpha, beta) = forks(&tmp, ["14500-14699", "14700-14899"], 14_990, 14_991).await;
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
        // Whichever box the restore opens first, the hostnames, once routed,
        // never lead nowhere while the chosen one takes over.
        hosts_never_drop(&client.published.lock().expect("published"), &FORK_HOSTS);
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
