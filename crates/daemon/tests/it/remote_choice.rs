//! Choosing which box serves a lane several boxes run (`remote.use`), and the
//! switch between boxes. A child of `remote`, whose box and client harness it
//! reuses.
//!
//! Both forks of a test project use the same port numbers, as forks of one
//! image do. Box and client share this machine, so only `alpha` runs real
//! servers on those numbers. `beta` lists them on units that bind nothing,
//! and its tunnel dials its own servers on other ports instead ([`MapDialer`]).

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

// Port ranges: 13500-14999, below the DNS server's 15353 and the other remote
// tests, which no other test in this binary uses. Each test takes its own part.

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
    let alpha_repo = repo(
        &tmp.path,
        "forkalpha",
        &fork_project("alpha", web, docs, true),
    );
    let beta_repo = repo(
        &tmp.path,
        "forkbeta",
        &fork_project("beta", web, docs, false),
    );
    let alpha = BoxSide::start(tmp, "alpha", ranges[0]).await;
    let dialer = Arc::new(MapDialer::default());
    let beta = BoxSide::start_with(tmp, "beta", ranges[1], Some(dialer.clone())).await;
    for (side, repo) in [(&alpha, &alpha_repo), (&beta, &beta_repo)] {
        side.h
            .call(Method::Start, json!({}), cx(repo))
            .await
            .expect("box start");
    }
    let real = [
        (web, box_port(&beta.h, 0, "real-web").await),
        (docs, box_port(&beta.h, 0, "real-docs").await),
    ];
    dialer.0.lock().expect("map").extend(real);
    (alpha, beta)
}

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

/// The `remote.warning` events received within `wait`.
async fn warnings_within(frames: &mut mpsc::Receiver<Frame>, wait: Duration) -> Vec<Value> {
    let mut found = Vec::new();
    let _ = tokio::time::timeout(wait, async {
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

/// The `remote.warning` events received within a short while.
async fn warnings(frames: &mut mpsc::Receiver<Frame>) -> Vec<Value> {
    warnings_within(frames, Duration::from_secs(2)).await
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

/// Sends GETs of `host` through the proxy one after another until `stop`,
/// collecting each answer.
fn request_loop(
    proxy: u16,
    host: &'static str,
    stop: tokio_util::sync::CancellationToken,
) -> JoinHandle<Vec<(u16, String)>> {
    tokio::spawn(async move {
        let mut seen = Vec::new();
        while !stop.is_cancelled() {
            seen.push(status_and_body(proxy, host).await);
        }
        seen
    })
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
            binds(&client, "alpha", WEB).await.then_some(())
        })
        .await;
        assert!(bound.is_some(), "{}", client.host_named("alpha").await);

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
        let before = warnings_within(&mut frames, Duration::from_millis(500)).await;
        assert!(
            !before.iter().any(|w| w["code"] == "port_in_use"),
            "{before:?}"
        );

        let stop = tokio_util::sync::CancellationToken::new();
        let looping = request_loop(client.proxy_port, FORK_HOSTS[0], stop.clone());
        tokio::time::sleep(Duration::from_millis(200)).await;
        client.published.lock().expect("published").clear();
        let reply = use_lane(&client, "forkapp@0", Some("beta")).await;
        assert_eq!(
            reply,
            Ok(
                json!({"lane": "forkapp@0", "host": "beta", "previous": "alpha",
                      "mirrored": true, "pending": false})
            )
        );
        // The pooled connection to alpha is not reused for the next request.
        assert!(answers(&client, FORK_HOSTS[1], "docs-from-beta").await);
        tokio::time::sleep(Duration::from_millis(300)).await;
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
        stop.cancel();
        let seen = looping.await.expect("request loop");
        let first_beta = seen
            .iter()
            .position(|(_, body)| body == "web-from-beta")
            .unwrap_or_else(|| panic!("beta answered: {seen:?}"));
        for (index, (status, body)) in seen.iter().enumerate() {
            assert_eq!(*status, 200, "request {index}: {seen:?}");
            let expected = if index < first_beta {
                "web-from-alpha"
            } else {
                "web-from-beta"
            };
            assert_eq!(body, expected, "request {index}: {seen:?}");
        }
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

        let seen = warnings(&mut frames).await;
        for code in ["port_in_use", "lane_skipped", "open_failed"] {
            assert!(
                !seen.iter().any(|w| w["code"] == code),
                "no {code} for a switch: {seen:?}"
            );
        }
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
                      "mirrored": false, "pending": true})
            )
        );
        assert!(answers(&client, FORK_HOSTS[0], "web-from-alpha").await);
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
/// A choice beats a stopped local lane: the box lane is mirrored and the
/// local lane is listed as yielded, until `remote use --local` gives the
/// hostnames back. Starting the local lane deletes a choice with
/// `choice_cleared` and takes the hostnames back as soon as the start
/// begins, before any service spawns, and while it runs a choice is refused
/// with `local_lane_running`.
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
            Ok(json!({"lane": "boxapp@0", "host": "box", "previous": null,
                      "mirrored": true, "pending": false}))
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
                      "mirrored": true, "pending": false}))
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
