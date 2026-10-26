//! Mirror reports and `infra.external`: what a client tells the box it
//! mirrors reaches the box's `status`, `url` and `hosts`; external ports are
//! routed where they are declared, mirrored to clients that list
//! `external_units` while a lane of their project runs, and kept out of port
//! blocks. A child of `remote`, whose box and client harness it reuses.

use std::{net::Ipv6Addr, sync::Arc};

use devlanes_pki::CertificateAuthority;
use devlanes_proto::{Context, Method};
use devlanes_transport::{
    DialTarget, EXTERNAL_UNITS, FEATURES, MIRROR_REPORT, OpenError, RejectReason, Session,
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
    std::fs::read_to_string(client.h.home.join("client-id"))
        .expect("the client stored its id")
        .trim()
        .to_owned()
}

/// A client id made of `hex`, eight hex digits, repeated.
fn id_of(hex: &str) -> String {
    format!("dl_cid_{}", hex.repeat(8))
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

/// A session straight to the box listing `features`, with its own id
/// ([`id_of`] `hex`), paired with a fresh code.
async fn direct(side: &BoxSide, hex: &str, features: &[&str]) -> Session {
    let mut options = SessionOptions::new("devlanes test-old-client");
    options.client_id = id_of(hex);
    options.client_name = "old-laptop".into();
    options.features = features.iter().map(|name| (*name).to_owned()).collect();
    options.pairing = Some(super::OneCode::new(side.code().await));
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
        let old = direct(&side, "01d0c11e", &features_without(MIRROR_REPORT)).await;
        assert!(
            !old.report(json!({"alias": "box"})),
            "the box does not take it"
        );
        let old_id = id_of("01d0c11e");
        let listed = eventually(SOON, || async { listed_client(&side, &old_id).await })
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

/// The units of kind `external` of the lanes of `cwd`'s project.
async fn external_units(h: &Harness, cwd: &std::path::Path) -> Vec<Value> {
    let status = h
        .call(Method::Status, json!({}), cx(cwd))
        .await
        .expect("status");
    status["items"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|lane| lane["units"].as_array().cloned().unwrap_or_default())
        .filter(|unit| unit["kind"] == "external")
        .collect()
}

/// The lane summary of `cwd`'s lane.
async fn lane_of(h: &Harness, cwd: &std::path::Path) -> Value {
    let status = h
        .call(Method::Status, json!({}), cx(cwd))
        .await
        .expect("status");
    status["items"][0].clone()
}

/// Whether `session`'s registry lists a unit of kind external.
fn lists_external(session: &Session) -> bool {
    session
        .registry()
        .current()
        .units()
        .any(|unit| unit.key.kind == UnitKind::External)
}

/// How `session`'s box answers an open of `port`: the refusal, or `None` when
/// the box dialed it.
async fn refusal(session: &Session, port: u16) -> Option<RejectReason> {
    match session.open(DialTarget::Port { port }).await {
        Err(OpenError::Rejected { reason, .. }) => Some(reason),
        Ok(_) => None,
        Err(error) => panic!("unexpected open failure: {error}"),
    }
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

/// A box project with one service and two external entries, `files` routed
/// and `vault` hidden, on the ports the test puts in place of `FILES_PORT`
/// and `VAULT_PORT`.
const EXTERNAL: &str = r#"project: extapp
services:
  echo:
    run: tcp-echo
infra:
  external:
    files:
      address: "127.0.0.1:FILES_PORT"
      hosts: "files.${{ lane.domain }}"
    vault:
      address: "127.0.0.1:VAULT_PORT"
      hidden: true
"#;

#[tokio::test(flavor = "multi_thread")]
/// External entries on the box: every lane lists them, the box routes their
/// hostnames while nothing runs, and a client with `external_units` mirrors
/// and routes `files` to whatever listens there only while a lane of the
/// project runs. A client without the feature never sees or dials them, and
/// the hidden `vault` is refused to everyone. On the client, its own entry is
/// routed by its proxy.
async fn external_ports_are_routed_and_mirrored_while_a_lane_runs() {
    if !ipv6_loopback().await {
        eprintln!(
            "skipping external_ports_are_routed_and_mirrored_while_a_lane_runs: no IPv6 loopback"
        );
        return;
    }
    tokio::time::timeout(LIMIT * 2, async {
        let tmp = temp();
        let (port, server) = plain_server("hello-external").await;
        let (vault, vault_server) = plain_server("hello-vault").await;
        let config = EXTERNAL
            .replace("FILES_PORT", &port.to_string())
            .replace("VAULT_PORT", &vault.to_string());
        let main = repo(&tmp.path, "extrepo", &config);
        let side = BoxSide::start(&tmp, "box", "18500-18699").await;
        side.h
            .call(Method::LaneList, json!({}), cx(&main))
            .await
            .expect("lane list");
        let listed = external_units(&side.h, &main).await;
        assert_eq!(listed.len(), 2, "{listed:?}");
        assert_eq!(listed[0]["name"], "files");
        assert_eq!(listed[0]["state"], "external");
        assert_eq!(listed[0]["ports"]["default"], port);
        assert_eq!(listed[1]["hidden"], true, "{listed:?}");
        assert!(
            routes(&side.h, "files.extapp.localhost").await,
            "routed on the box while nothing runs"
        );
        let own = url(&side.h, &main, "files").await;
        assert_eq!(own["url"], "https://files.extapp.localhost", "{own}");

        let client = ClientSide::start(&tmp, "client", "18700-18899", &side).await;
        client.connect("box").await;
        // Nothing runs on the box yet: listed, kept free, not dialable.
        let current = direct(&side, "c0ffee00", FEATURES).await;
        let files = current
            .registry()
            .current()
            .units()
            .find(|unit| unit.key.kind == UnitKind::External && unit.key.name == "files")
            .cloned()
            .expect("files is listed");
        assert_eq!(files.state, "stopped", "{files:?}");
        assert_eq!(refusal(&current, port).await, Some(RejectReason::NotRunning));
        current.close();

        side.h
            .call(Method::Start, json!({}), cx(&main))
            .await
            .expect("box start");
        let mirrored = eventually(SOON, || async {
            client.mirrored().await.contains(&port).then_some(())
        })
        .await;
        assert!(mirrored.is_some(), "mirrored: {}", client.host().await);
        let answered = eventually(SOON, || async {
            http_get(client.proxy_port, "files.extapp.localhost")
                .await
                .filter(|response| response.contains("hello-external"))
        })
        .await;
        assert!(answered.is_some(), "routed through the client's proxy");
        assert!(
            !client.mirrored().await.contains(&vault),
            "a hidden entry is not mirrored: {}",
            client.host().await
        );
        let current = direct(&side, "c0ffee01", FEATURES).await;
        assert_eq!(refusal(&current, vault).await, Some(RejectReason::Hidden));
        assert_eq!(refusal(&current, port).await, None, "files is dialable");
        current.close();

        // An older client never gets the unit kind it would reject, nor dials it.
        let old = direct(&side, "01d0c11e", &features_without(EXTERNAL_UNITS)).await;
        assert!(!lists_external(&old), "an old client got an external unit");
        assert_eq!(refusal(&old, port).await, Some(RejectReason::NotRegistered));
        old.close();

        // The client's own entry goes through its own proxy.
        let (local, local_server) = plain_server("hello-local").await;
        let mine = repo(
            &tmp.path,
            "minerepo",
            &format!(
                "project: mine\ninfra:\n  external:\n    docs:\n      address: 127.0.0.1:{local}\n      hosts: docs.mine.localhost\n"
            ),
        );
        client
            .h
            .call(Method::LaneList, json!({}), cx(&mine))
            .await
            .expect("client lane list");
        let answered = eventually(SOON, || async {
            http_get(client.proxy_port, "docs.mine.localhost")
                .await
                .filter(|response| response.contains("hello-local"))
        })
        .await;
        assert!(answered.is_some(), "routed by the client's proxy");

        // Once the box's lane stops, the client lets the port go.
        side.h
            .call(Method::Stop, json!({}), cx(&main))
            .await
            .expect("box stop");
        let unmirrored = eventually(SOON, || async {
            (!client.mirrored().await.contains(&port)).then_some(())
        })
        .await;
        assert!(
            unmirrored.is_some(),
            "the stop unmirrors: {}",
            client.host().await
        );

        server.abort();
        vault_server.abort();
        local_server.abort();
        client.stop().await;
        side.stop().await;
    })
    .await
    .expect("timeout");
}

/// A project whose `api` service gets the first port of its block, and whose
/// `tool` entry listens on `TOOL_PORT`, replaced by the test.
const TOOL: &str = "project: PROJECT\nlanes: { block_size: 4 }\nservices:\n  api: { run: sleep, ready: started }\ninfra:\n  external:\n    tool: { address: '127.0.0.1:TOOL_PORT' }\n";

#[tokio::test(flavor = "multi_thread")]
/// New port blocks of every project skip external ports, also inside the
/// pool; a block from before the entry existed that hands its port to a
/// service makes the lane fail with a config error naming both.
async fn external_ports_stay_out_of_port_blocks() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let h = engine_with(&tmp.path.join("home"), "18900-18999", None, None).await;
        let config = |project: &str, port: u16| {
            TOOL.replace("PROJECT", project)
                .replace("TOOL_PORT", &port.to_string())
        };
        // The first port of the pool would start the first block.
        let first = repo(&tmp.path, "firstrepo", &config("first", 18900));
        h.call(Method::LaneList, json!({}), cx(&first))
            .await
            .expect("lane list");
        let block = |lane: &Value| {
            let block = &lane["port_block"];
            (
                block[0].as_u64().unwrap_or_default(),
                block[1].as_u64().unwrap_or_default(),
            )
        };
        let lane = lane_of(&h, &first).await;
        let (start, end) = block(&lane);
        assert!(start > 18900, "the block skips the external port: {lane}");
        let tool = external_units(&h, &first).await;
        assert_eq!(tool.len(), 1, "{tool:?}");
        assert_eq!(tool[0]["state"], "external");
        assert_eq!(tool[0]["ports"]["default"], 18900);

        // Another project's block skips it too, wherever it lands.
        let other = repo(
            &tmp.path,
            "otherrepo",
            "project: other\nservices:\n  web: { run: sleep, ready: started }\n",
        );
        h.call(Method::LaneList, json!({}), cx(&other))
            .await
            .expect("lane list");
        let (other_start, other_end) = block(&lane_of(&h, &other).await);
        assert!(
            !(other_start..=other_end).contains(&18900) && other_start > end,
            "{other_start}-{other_end} after {start}-{end}"
        );

        // A lane whose service already has the port an entry now names.
        let late = repo(
            &tmp.path,
            "laterepo",
            "project: late\nlanes: { block_size: 4 }\nservices:\n  api: { run: sleep, ready: started }\n",
        );
        h.call(Method::LaneList, json!({}), cx(&late))
            .await
            .expect("lane list");
        let api = lane_of(&h, &late).await["units"][0]["ports"]["default"]
            .as_u64()
            .and_then(|port| u16::try_from(port).ok())
            .expect("api's port");
        std::fs::write(late.join("devlanes.yaml"), config("late", api)).expect("edit config");
        let refused = h
            .call(Method::Start, json!({}), cx(&late))
            .await
            .expect_err("the start is refused");
        assert_eq!(refused.code, "config_error", "{refused:?}");
        let message = refused.data["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&format!(
                "infra.external.tool.address: port {api} is services.api's port in late@0"
            )),
            "{message}"
        );
        let reason = &lane_of(&h, &late).await["reason"];
        assert_eq!(reason["code"], "config_error", "{reason}");
        h.close().await;
    })
    .await
    .expect("timeout");
}
