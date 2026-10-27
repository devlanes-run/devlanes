//! `infra.external` on one machine: entry names are no exec name and take no
//! process command, devlanes' own ports and other projects' ports are config
//! errors, and `config check` merges the same `devlanes.local.yaml` a lane does.

use devlanes_proto::Method;
use serde_json::json;

use crate::{LIMIT, Setup, cx, harness, repo, temp, worktree};

/// A project whose `docs` entry listens on `DOCS_PORT`, replaced by the test.
const DOCS: &str = "project: PROJECT\nservices:\n  api: { run: sleep, ready: started }\ninfra:\n  external:\n    docs: { address: '127.0.0.1:DOCS_PORT' }\n";

/// [`DOCS`] for `project` with `docs` on `port`.
fn docs(project: &str, port: u16) -> String {
    DOCS.replace("PROJECT", project)
        .replace("DOCS_PORT", &port.to_string())
}

#[tokio::test(flavor = "multi_thread")]
/// `exec --name docs` is refused while `docs` is an entry, and `start`,
/// `stop` and `logs` name the entry instead of looking for a process.
async fn entry_names_are_no_processes() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", &docs("acme", 8000));
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::LaneList, json!({}), cx(&main))
            .await
            .expect("lane list");
        let refused = h
            .call(
                Method::Exec,
                json!({"name": "docs", "command": ["sleep"]}),
                cx(&main),
            )
            .await
            .expect_err("exec refused");
        assert_eq!(refused.code, "name_taken", "{refused:?}");
        assert_eq!(refused.data["kind"], "external");
        for (method, params) in [
            (Method::Start, json!({"units": ["docs"]})),
            (Method::Stop, json!({"units": ["docs"]})),
            (Method::Restart, json!({"units": ["docs"]})),
            (Method::Logs, json!({"units": ["docs"]})),
        ] {
            let refused = h
                .call(method, params.clone(), cx(&main))
                .await
                .expect_err("refused");
            assert_eq!(refused.code, "external_unit", "{params}: {refused:?}");
            assert_eq!(refused.data["unit"], "docs");
            assert_eq!(refused.data["lane"], "acme@0");
        }
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// The proxy's configured HTTPS and HTTP ports are refused as devlanes' own,
/// and so is a port another project's lane was given.
async fn ports_devlanes_or_other_projects_hold_are_refused() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        for (port, what) in [
            (443, "the devlanes proxy's HTTPS port"),
            (80, "the devlanes proxy's HTTP port"),
        ] {
            let project = format!("own{port}");
            let main = repo(&tmp.path, &project, &docs(&project, port));
            let refused = h
                .call(Method::Start, json!({}), cx(&main))
                .await
                .expect_err("start refused");
            assert_eq!(refused.code, "config_error", "{refused:?}");
            assert_eq!(
                refused.data["message"],
                format!(
                    "infra.external.docs.address: port {port} is {what}, which devlanes listens on itself"
                )
            );
        }

        let other = repo(
            &tmp.path,
            "other",
            "project: other\nservices:\n  web: { run: sleep, ready: started }\n",
        );
        h.call(Method::LaneList, json!({}), cx(&other))
            .await
            .expect("lane list");
        let status = h
            .call(Method::Status, json!({}), cx(&other))
            .await
            .expect("status");
        let web = status["items"][0]["units"][0]["ports"]["default"]
            .as_u64()
            .and_then(|port| u16::try_from(port).ok())
            .expect("web's port");
        let main = repo(&tmp.path, "clash", &docs("clash", web));
        let refused = h
            .call(Method::Start, json!({}), cx(&main))
            .await
            .expect_err("start refused");
        assert_eq!(refused.code, "config_error", "{refused:?}");
        let message = refused.data["message"].as_str().unwrap_or_default();
        assert!(
            message.starts_with(&format!(
                "infra.external.docs.address: port {web} is services.web's port in other@0; declare a port outside"
            )),
            "{message}"
        );
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// `config check` in a linked worktree without its own `devlanes.local.yaml`
/// merges the primary worktree's, as the lane does, so an error there shows.
async fn config_check_uses_the_primary_local_file() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(
            &tmp.path,
            "r",
            "project: acme\nservices:\n  api: { run: sleep, ready: started }\n",
        );
        let linked = tmp.path.join("linked");
        worktree(&main, &linked, "feat/linked");
        tokio::fs::write(
            main.join("devlanes.local.yaml"),
            "infra:\n  external:\n    tool: { address: '10.0.0.1:5050' }\n",
        )
        .await
        .expect("local file");
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        let refused = h
            .call(Method::ConfigCheck, json!({}), cx(&linked))
            .await
            .expect_err("the primary's local file is merged");
        assert_eq!(refused.code, "config_error", "{refused:?}");
        let message = refused.data["message"].as_str().unwrap_or_default();
        assert!(message.contains("not a loopback address"), "{message}");
        h.close().await;
    })
    .await
    .expect("timeout");
}
