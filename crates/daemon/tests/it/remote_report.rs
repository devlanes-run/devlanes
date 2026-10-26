//! Mirror reports and `devlanes expose`: what a client tells the box it
//! mirrors reaches the box's `status`, `url` and `hosts`; exposed ports are
//! routed on the box, mirrored to clients that list `exposed_units` and gone
//! after removal, a whole-lane stop or a daemon restart. A child of `remote`,
//! whose box and client harness it reuses.

use std::{net::Ipv6Addr, sync::Arc};

use devlanes_pki::CertificateAuthority;
use devlanes_proto::{Context, Method};
use devlanes_transport::{
    DialTarget, EXPOSED_UNITS, FEATURES, MIRROR_REPORT, OpenError, RejectReason, Session,
    SessionOptions, UnitKind,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

use super::{BOX, BoxSide, ClientSide, SOON, box_port, engine_with, http_get, ipv6_loopback};
use crate::{Harness, LIMIT, cx, eventually, repo, temp};

// Port ranges: 18000-18999, below the default pool and below `remote_view.rs`'s
// 19000-19499, which no other test in this binary uses.

/// A box project whose one service is hidden but routed.
const VAULT: &str = r#"project: vaultapp
services:
  vault:
    run: tcp-echo
    hidden: true
    hosts: ["vault.${{ lane.domain }}"]
"#;

/// The box's `status --all` entry of the connected client whose id is `id`.
async fn listed_client(side: &BoxSide, id: &str) -> Option<Value> {
    let status = side
        .h
        .call(Method::Status, json!({"all": true}), Context::default())
        .await
        .ok()?;
    status["clients"]
        .as_array()?
        .iter()
        .find(|client| client["id"] == id)
        .cloned()
}

/// The id a client engine keeps in its home.
fn client_id(client: &ClientSide) -> String {
    std::fs::read_to_string(client.h.home.join("remote-client-id"))
        .expect("the client stored its id")
        .trim()
        .to_owned()
}

/// `url <unit>` on `h` for the lane of `cwd`.
async fn url(h: &Harness, cwd: &std::path::Path, unit: &str) -> Value {
    h.call(Method::Url, json!({"unit": unit}), cx(cwd))
        .await
        .expect("url")
}

/// The mirrored port `port` in a client report.
fn reported(report: &Value, port: u16) -> Option<&Value> {
    report["ports"]
        .as_array()?
        .iter()
        .find(|entry| entry["port"] == port)
}

/// A session straight to the box listing `features`, with its own id.
async fn direct(side: &BoxSide, id: &str, features: &[&str]) -> Session {
    let mut options = SessionOptions::new("devlanes test-old-client");
    options.client_id = id.into();
    options.client_name = "old-laptop".into();
    options.features = features.iter().map(|name| (*name).to_owned()).collect();
    Session::connect(&side.transport, options)
        .await
        .expect("direct session")
}

/// This build's features without `left_out`.
fn features_without(left_out: &str) -> Vec<&'static str> {
    FEATURES
        .iter()
        .copied()
        .filter(|name| *name != left_out)
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
/// A connected client reports what it mirrors: the box's `status` shows it as
/// `clients[].mirror`, `url` gives the URL on the client's own HTTPS port when
/// it cannot serve the box's, and names a port taken on the client and a
/// hostname it does not route as unreachable with the reason; `url ls` adds
/// the client to each route. A client without `mirror_report` sends no
/// report and gets no URLs, and the report goes with the client's feed.
async fn a_mirror_report_reaches_status_and_url_on_the_box() {
    if !ipv6_loopback().await {
        eprintln!("skipping a_mirror_report_reaches_status_and_url_on_the_box: no IPv6 loopback");
        return;
    }
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let path = tmp.path.join("ca");
        let ca = Arc::new(
            tokio::task::spawn_blocking(move || CertificateAuthority::load_or_create(&path))
                .await
                .expect("ca task")
                .expect("ca"),
        );
        // The box's HTTPS port, past its lane's block, is held here, so the
        // client serves the box's hostnames on its own port only.
        let box_https = 18200;
        let client_https = 18450;
        let squatter = std::net::TcpListener::bind(("127.0.0.1", box_https)).expect("squatter");
        let main = repo(&tmp.path, "reportrepo", BOX);
        let side = BoxSide::start_on_https(&tmp, "box", "18000-18199", box_https).await;
        side.h
            .call(Method::Start, json!({"units": ["web", "echo"]}), cx(&main))
            .await
            .expect("box start");
        let (web, echo) = (
            box_port(&side.h, 0, "web").await,
            box_port(&side.h, 0, "echo").await,
        );
        // Something on the client holds echo's number on the address it mirrors on.
        let busy = TcpListener::bind((Ipv6Addr::LOCALHOST, echo))
            .await
            .expect("hold echo's port");
        let client = ClientSide::start_with(
            &tmp,
            "client",
            "18250-18449",
            &[("box", &side)],
            Some((client_https, ca.clone())),
        )
        .await;
        client.connect("box").await;
        let id = client_id(&client);

        let mirror = eventually(SOON, || async {
            let mirror = listed_client(&side, &id).await?["mirror"].clone();
            let bound = reported(&mirror, web)?["state"] == "bound";
            let held = reported(&mirror, echo)?["state"] == "in_use";
            (bound && held).then_some(mirror)
        })
        .await;
        let Some(mirror) = mirror else {
            let listed = listed_client(&side, &id).await;
            panic!("the box shows the client's mirror: {listed:?}");
        };
        assert_eq!(mirror["alias"], "box", "{mirror}");
        assert_eq!(mirror["https_port"], client_https, "{mirror}");
        assert_eq!(mirror["serves_box_port"], false, "{mirror}");
        assert!(
            mirror["skipped_hosts"]
                .as_array()
                .is_some_and(|hosts| hosts.iter().any(|host| host == "api.example.com")),
            "{mirror}"
        );

        let routed = url(&side.h, &main, "web").await;
        let clients = routed["clients"].as_array().expect("clients");
        assert_eq!(clients.len(), 1, "{routed}");
        assert_eq!(
            clients[0]["url"],
            format!("https://web.boxapp.localhost:{client_https}"),
            "{routed}"
        );
        assert_eq!(clients[0]["reachable"], true);
        assert_eq!(clients[0]["alias"], "box");

        let held = url(&side.h, &main, "echo").await;
        let entry = &held["clients"][0];
        assert_eq!(entry["reachable"], false, "{held}");
        assert!(
            entry["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains(&format!("port {echo} is in use there"))),
            "{held}"
        );
        let foreign = url(&side.h, &main, "api").await;
        assert_eq!(foreign["clients"][0]["reachable"], false, "{foreign}");
        assert!(
            foreign["clients"][0]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("not routed there")),
            "{foreign}"
        );

        let hosts = side
            .h
            .call(Method::Hosts, json!({}), Context::default())
            .await
            .expect("box hosts");
        let route = hosts["items"]
            .as_array()
            .and_then(|items| items.iter().find(|i| i["host"] == "web.boxapp.localhost"))
            .expect("web route");
        assert_eq!(route["clients"][0]["reachable"], true, "{route}");

        // A hidden unit's hostname is routed on the box but opens on no client.
        let vault = repo(&tmp.path, "vaultrepo", VAULT);
        side.h
            .call(Method::Start, json!({}), cx(&vault))
            .await
            .expect("vault start");
        let hosts = side
            .h
            .call(Method::Hosts, json!({}), Context::default())
            .await
            .expect("box hosts");
        let route = hosts["items"]
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|i| i["host"] == "vault.vaultapp.localhost")
            })
            .expect("vault route");
        assert_eq!(route["clients"][0]["reachable"], false, "{route}");
        assert!(
            route["clients"][0]["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("hidden")),
            "{route}"
        );

        // A client of a release without mirror reports is listed without one.
        let old = direct(&side, "0ld0c11e", &features_without(MIRROR_REPORT)).await;
        assert!(
            !old.report(json!({"alias": "box"})),
            "the box does not take it"
        );
        let listed = eventually(SOON, || async { listed_client(&side, "0ld0c11e").await })
            .await
            .expect("the old client is listed");
        assert!(listed.get("mirror").is_none(), "{listed}");
        let routed = url(&side.h, &main, "web").await;
        assert_eq!(
            routed["clients"].as_array().map(Vec::len),
            Some(1),
            "only the reporting client has URLs: {routed}"
        );
        old.close();

        // The report goes with the client's feed.
        client.stop().await;
        let gone = eventually(SOON, || async {
            let routed = url(&side.h, &main, "web").await;
            routed["clients"]
                .as_array()
                .is_some_and(Vec::is_empty)
                .then_some(())
        })
        .await;
        assert!(gone.is_some(), "{}", url(&side.h, &main, "web").await);
        drop(busy);
        drop(squatter);
        side.stop().await;
    })
    .await
    .expect("timeout");
}

/// An HTTP server on `127.0.0.1` that answers every request with `body`, as
/// something devlanes did not start: its port and the accepting task.
async fn plain_server(body: &'static str) -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("server");
    let port = listener.local_addr().expect("server addr").port();
    let task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    (port, task)
}

/// The box lane's units of kind `exposed`.
async fn exposed_units(h: &Harness, cwd: &std::path::Path) -> Vec<Value> {
    let status = h
        .call(Method::Status, json!({}), cx(cwd))
        .await
        .expect("status");
    status["items"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|lane| lane["units"].as_array().cloned().unwrap_or_default())
        .filter(|unit| unit["kind"] == "exposed")
        .collect()
}

/// Whether the box routes `host` itself.
async fn routes(h: &Harness, host: &str) -> bool {
    let hosts = h
        .call(Method::Hosts, json!({}), Context::default())
        .await
        .expect("hosts");
    hosts["items"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["host"] == host))
}

#[tokio::test(flavor = "multi_thread")]
/// `expose <port> --host files` on the box: the lane lists `port-<port>`, the
/// box's proxy routes `files.<domain>`, and a client with `exposed_units`
/// mirrors and routes it to whatever listens there, while a client without
/// the feature never sees or dials it. Hostnames outside the lane's domain,
/// another unit's port, the proxy's port and a taken hostname are refused.
/// `--remove` and a whole-lane stop take it away on both machines; a hidden
/// unit that later takes the port wins over the exposure; `lane clean` drops it.
async fn exposed_ports_are_routed_mirrored_and_removed() {
    if !ipv6_loopback().await {
        eprintln!("skipping exposed_ports_are_routed_mirrored_and_removed: no IPv6 loopback");
        return;
    }
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let main = repo(&tmp.path, "exposerepo", BOX);
        let side = BoxSide::start(&tmp, "box", "18500-18699").await;
        side.h
            .call(Method::Start, json!({"units": ["echo"]}), cx(&main))
            .await
            .expect("box start");
        let echo = box_port(&side.h, 0, "echo").await;
        let (port, server) = plain_server("hello-exposed").await;
        let expose = |params: Value| side.h.call(Method::Expose, params, cx(&main));

        let added = expose(json!({"port": port, "host": "files"}))
            .await
            .expect("expose");
        let unit = format!("port-{port}");
        assert_eq!(added["lane"], "boxapp@0", "{added}");
        assert_eq!(added["exposed"][0]["unit"], unit, "{added}");
        assert_eq!(added["exposed"][0]["host"], "files.boxapp.localhost");
        let listed = exposed_units(&side.h, &main).await;
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(listed[0]["name"], unit);
        assert_eq!(listed[0]["ports"]["default"], port);
        assert!(
            routes(&side.h, "files.boxapp.localhost").await,
            "routed on the box"
        );
        let own = url(&side.h, &main, &unit).await;
        assert_eq!(own["url"], "https://files.boxapp.localhost", "{own}");

        // Refused before anything binds it; the box's lane block ends well below.
        let other = 18698;
        for (params, code) in [
            (
                json!({"port": other, "host": "files.example.com"}),
                "host_not_allowed",
            ),
            // Below a routed suffix, but not the lane's domain.
            (
                json!({"port": other, "host": "docs.localhost"}),
                "host_not_allowed",
            ),
            // The proxy's own port (the default user config's 443).
            (json!({"port": 443}), "port_reserved"),
            (
                json!({"port": other, "host": "*.boxapp.localhost"}),
                "invalid_host",
            ),
            (json!({"port": echo}), "port_taken"),
            (json!({"port": other, "host": "web"}), "host_taken"),
            (json!({"port": other, "remove": true}), "not_exposed"),
        ] {
            let refused = expose(params.clone()).await.expect_err("refused");
            assert_eq!(refused.code, code, "{params}: {refused:?}");
        }

        let client = ClientSide::start(&tmp, "client", "18700-18899", &side).await;
        client.connect("box").await;
        let mirrored = eventually(SOON, || async {
            client.mirrored().await.contains(&port).then_some(())
        })
        .await;
        assert!(mirrored.is_some(), "mirrored: {}", client.host().await);
        let answered = eventually(SOON, || async {
            http_get(client.proxy_port, "files.boxapp.localhost")
                .await
                .filter(|response| response.contains("hello-exposed"))
        })
        .await;
        assert!(answered.is_some(), "routed through the client's proxy");

        // An older client never gets the unit kind it would reject, nor dials it.
        let old = direct(&side, "0ld0c11e", &features_without(EXPOSED_UNITS)).await;
        assert!(
            !old.registry()
                .current()
                .units()
                .any(|unit| unit.key.kind == UnitKind::Exposed),
            "an old client got an exposed unit"
        );
        match old.open(DialTarget::Port { port }).await {
            Err(OpenError::Rejected { reason, .. }) => {
                assert_eq!(reason, RejectReason::NotRegistered);
            }
            Ok(_) => panic!("an old client dialed an exposed port"),
            Err(error) => panic!("unexpected open failure: {error}"),
        }
        old.close();

        expose(json!({"port": port, "remove": true}))
            .await
            .expect("remove");
        assert!(exposed_units(&side.h, &main).await.is_empty());
        assert!(!routes(&side.h, "files.boxapp.localhost").await);
        let unmirrored = eventually(SOON, || async {
            (!client.mirrored().await.contains(&port)).then_some(())
        })
        .await;
        assert!(
            unmirrored.is_some(),
            "removal unmirrors: {}",
            client.host().await
        );

        // A whole-lane stop takes exposed ports with it.
        expose(json!({"port": port})).await.expect("expose again");
        let mirrored = eventually(SOON, || async {
            client.mirrored().await.contains(&port).then_some(())
        })
        .await;
        assert!(
            mirrored.is_some(),
            "mirrored again: {}",
            client.host().await
        );
        side.h
            .call(Method::Stop, json!({}), cx(&main))
            .await
            .expect("box stop");
        assert!(exposed_units(&side.h, &main).await.is_empty());
        let unmirrored = eventually(SOON, || async {
            (!client.mirrored().await.contains(&port)).then_some(())
        })
        .await;
        assert!(
            unmirrored.is_some(),
            "the stop unmirrors: {}",
            client.host().await
        );

        // A hidden unit that takes an exposed port later wins: the exposure
        // leaves the feed and the port is refused as hidden.
        expose(json!({"port": port})).await.expect("expose once more");
        let hidden = repo(
            &tmp.path,
            "hiddenrepo",
            &format!(
                "project: hiddenapp\nservices:\n  vault:\n    run: sleep\n    hidden: true\n    env:\n      PORT: {port}\n"
            ),
        );
        side.h
            .call(Method::Start, json!({}), cx(&hidden))
            .await
            .expect("hidden start");
        let unmirrored = eventually(SOON, || async {
            (!client.mirrored().await.contains(&port)).then_some(())
        })
        .await;
        assert!(
            unmirrored.is_some(),
            "a hidden unit's port is not mirrored: {}",
            client.host().await
        );
        let current = direct(&side, "c0ffee00", FEATURES).await;
        assert!(
            !current
                .registry()
                .current()
                .units()
                .any(|unit| unit.key.kind == UnitKind::Exposed),
            "the exposure of a hidden unit's port was sent"
        );
        match current.open(DialTarget::Port { port }).await {
            Err(OpenError::Rejected { reason, .. }) => assert_eq!(reason, RejectReason::Hidden),
            Ok(_) => panic!("a hidden unit's port was dialed through an exposure"),
            Err(error) => panic!("unexpected open failure: {error}"),
        }
        current.close();

        // `lane clean` drops what the lane exposes.
        assert_eq!(exposed_units(&side.h, &main).await.len(), 1);
        let mut clean = cx(&main);
        clean.yes = true;
        side.h
            .call(Method::LaneClean, json!({}), clean)
            .await
            .expect("lane clean");
        let listed = side
            .h
            .call(Method::Expose, json!({"all": true}), Context::default())
            .await
            .expect("list");
        assert_eq!(listed["items"], json!([]), "{listed}");

        server.abort();
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// Exposed ports live in the daemon's memory: one that restarts on the same
/// home has none, and `expose` without a port lists them.
async fn a_daemon_restart_forgets_exposed_ports() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "restartrepo", BOX);
        let home = tmp.path.join("box");
        let h = engine_with(&home, "18900-18999", None, None).await;
        h.call(Method::LaneList, json!({}), cx(&main))
            .await
            .expect("lane list");
        h.call(Method::Expose, json!({"port": 8000}), cx(&main))
            .await
            .expect("expose");
        let listed = h
            .call(Method::Expose, json!({}), cx(&main))
            .await
            .expect("list");
        assert_eq!(listed["items"][0]["unit"], "port-8000", "{listed}");
        assert_eq!(
            listed["items"][0]["url"], "http://127.0.0.1:8000",
            "{listed}"
        );
        h.close().await;

        let h = engine_with(&home, "18900-18999", None, None).await;
        let listed = h
            .call(Method::Expose, json!({}), cx(&main))
            .await
            .expect("list after restart");
        assert_eq!(listed["items"], json!([]), "{listed}");
        h.close().await;
    })
    .await
    .expect("timeout");
}
