//! Infra failures carry what ran; units that a failed phase kept idle say why.

use devlanes_daemon::integrations::{InfraError, STDERR_TAIL_LINES};
use devlanes_proto::{LaneSummary, Method, Page, events};
use serde_json::{Value, json};

use crate::{LIMIT, Setup, cx, harness, repo, temp};

const CONFIG: &str = r#"project: acme
infra:
  containers:
    db: { image: "postgres:16", ports: { pg: 5432 }, per_lane: true }
services:
  api: { run: sleep }
  worker: { run: sleep }
"#;

/// The lane of `cwd`.
async fn first_lane(h: &crate::Harness, cwd: &std::path::Path) -> LaneSummary {
    let value = h
        .call(Method::Status, json!({}), cx(cwd))
        .await
        .expect("status");
    let lanes: Page<LaneSummary> = serde_json::from_value(value).expect("page");
    lanes.items[0].clone()
}

/// The reason of `unit` in the lane of `cwd`.
async fn unit_reason(h: &crate::Harness, cwd: &std::path::Path, unit: &str) -> Value {
    let value = h
        .call(Method::Status, json!({}), cx(cwd))
        .await
        .expect("status");
    let lanes: Page<LaneSummary> = serde_json::from_value(value).expect("page");
    let summary = lanes.items[0]
        .units
        .iter()
        .find(|u| u.name == unit)
        .expect("unit")
        .clone();
    serde_json::to_value(summary.reason).expect("reason")
}

#[tokio::test(flavor = "multi_thread")]
/// A failed `compose up` reports command, status and a stripped stderr tail; the
/// selected units carry `phase_failed` until a start reaches the services phase.
async fn infra_failure_is_structured_and_marks_units() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let h = harness(
            &tmp.path.join("home"),
            Setup {
                infra: true,
                ..Setup::default()
            },
        )
        .await;
        let stderr: String = (0..60)
            .map(|i| format!("\x1b[31mline {i}\x1b[0m\n"))
            .collect();
        let failure = InfraError::new("compose_failed", "compose up failed").with_command(
            "docker compose -p acme up --detach --wait -- db",
            Some("exit status: 1".into()),
            Some(&stderr),
        );
        *h.infra.fail_up.lock().expect("lock") = Some(failure);
        let mut bus = h.engine.subscribe();

        let error = h
            .call(Method::Start, json!({}), cx(&main))
            .await
            .expect_err("infra fails");
        assert_eq!(error.code, "compose_failed");
        assert_eq!(error.data["message"], "compose up failed");
        assert_eq!(
            error.data["command"],
            "docker compose -p acme up --detach --wait -- db"
        );
        assert_eq!(error.data["status"], "exit status: 1");
        let tail = error.data["stderr"].as_str().expect("stderr");
        assert_eq!(tail.lines().count(), STDERR_TAIL_LINES);
        assert!(!tail.contains('\x1b'), "ANSI is stripped");
        assert_eq!(tail.lines().last(), Some("line 59"));
        assert_eq!(error.data["lane"], "acme@0");

        let reason = unit_reason(&h, &main, "api").await;
        assert_eq!(reason["code"], "phase_failed");
        assert_eq!(reason["data"]["phase"], "infra");
        assert_eq!(reason["data"]["lane"], "acme@0");
        assert_eq!(reason["data"]["error"]["code"], "compose_failed");
        assert_eq!(reason["data"]["error"]["data"]["status"], "exit status: 1");
        assert_eq!(
            unit_reason(&h, &main, "worker").await["code"],
            "phase_failed"
        );
        let mut announced = Vec::new();
        while let Ok(event) = bus.try_recv() {
            if event.event == events::UNIT_CHANGED
                && event.data["unit"]["reason"]["code"] == "phase_failed"
            {
                announced.push(
                    event.data["unit"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                );
            }
        }
        announced.sort();
        assert_eq!(announced, ["api", "worker"]);

        *h.infra.fail_up.lock().expect("lock") = None;
        let started = h
            .call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        assert_eq!(started["ready"], true, "{started}");
        assert_eq!(unit_reason(&h, &main, "api").await, Value::Null);
        assert_eq!(unit_reason(&h, &main, "worker").await, Value::Null);
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A stop that names a crashed service or exec unit settles it, and an
/// explicit full stop settles every crashed service, so the lane reads `idle`;
/// the crash stays in `errors` (run history).
async fn full_stop_settles_a_crashed_lane() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let config = "project: acme\nservices:\n  api: { run: sleep }\n  crash: { run: fail }\n  named: { run: fail }\n";
        let main = repo(&tmp.path, "r", config);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        let _ = h.call(Method::Start, json!({}), cx(&main)).await;
        let failed = crate::eventually(std::time::Duration::from_secs(5), || async {
            (first_lane(&h, &main).await.state == "failed").then_some(())
        })
        .await;
        assert!(failed.is_some(), "the crash fails the lane first");

        h.call(Method::Stop, json!({"units": ["api"]}), cx(&main))
            .await
            .expect("partial stop");
        assert_eq!(
            first_lane(&h, &main).await.state,
            "failed",
            "a partial stop keeps it"
        );

        // Named, a failed unit reads stopped and keeps its exit code.
        let unit_of = |lane: &LaneSummary, name: &str| {
            lane.units.iter().find(|u| u.name == name).cloned()
        };
        let stopped = h
            .call(Method::Stop, json!({"units": ["named"]}), cx(&main))
            .await
            .expect("named stop");
        assert_eq!(stopped["items"][0]["name"], "named", "{stopped}");
        assert_eq!(stopped["items"][0]["state"], "stopped", "{stopped}");
        assert_eq!(stopped["items"][0]["exit_code"], 1, "{stopped}");
        let lane = first_lane(&h, &main).await;
        assert_eq!(lane.state, "failed", "crash is not named");
        assert_eq!(unit_of(&lane, "named").expect("named").state, "stopped");
        assert_eq!(unit_of(&lane, "crash").expect("crash").state, "failed");

        h.call(
            Method::Exec,
            json!({"name": "job", "command": ["fail"]}),
            cx(&main),
        )
        .await
        .expect("exec");
        let job_failed = crate::eventually(std::time::Duration::from_secs(5), || async {
            let lane = first_lane(&h, &main).await;
            (unit_of(&lane, "job")?.state == "failed").then_some(())
        })
        .await;
        assert!(job_failed.is_some(), "the exec unit fails");
        let stopped = h
            .call(Method::Stop, json!({"units": ["job"]}), cx(&main))
            .await
            .expect("exec stop");
        assert_eq!(stopped["items"][0]["name"], "job", "{stopped}");
        assert_eq!(stopped["items"][0]["state"], "stopped", "{stopped}");
        assert_eq!(stopped["items"][0]["exit_code"], 1, "{stopped}");
        let lane = first_lane(&h, &main).await;
        assert_eq!(unit_of(&lane, "job").expect("job").state, "stopped");

        h.call(Method::Stop, json!({}), cx(&main))
            .await
            .expect("stop");
        let summary = first_lane(&h, &main).await;
        assert_eq!(summary.state, "idle");
        let crash = summary
            .units
            .iter()
            .find(|u| u.name == "crash")
            .expect("crash");
        assert_eq!(crash.state, "stopped");
        assert_eq!(crash.exit_code, Some(1));
        let errors = h
            .call(Method::Errors, json!({}), cx(&main))
            .await
            .expect("errors");
        assert!(errors.to_string().contains("crash"), "{errors}");
        h.close().await;
    })
    .await
    .expect("timeout");
}

/// The summary of `unit` in the lane of `cwd`.
async fn unit_summary(
    h: &crate::Harness,
    cwd: &std::path::Path,
    unit: &str,
) -> Option<devlanes_proto::UnitSummary> {
    first_lane(h, cwd)
        .await
        .units
        .into_iter()
        .find(|u| u.name == unit)
}

/// Asserts that a start or restart answer reports `unit` failed with `code`,
/// as the error itself or as one of the result's failures.
fn assert_unit_failed(answer: Result<Value, devlanes_proto::RpcError>, unit: &str, code: &str) {
    match answer {
        Ok(value) => {
            let failed = value["failures"].as_array().is_some_and(|failures| {
                failures
                    .iter()
                    .any(|failure| failure["unit"] == unit && failure["reason"]["code"] == code)
            });
            assert!(failed, "{value}");
        }
        Err(error) => assert_eq!(error.code, code, "{error:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
/// A service whose program could not be started stays failed with the spawn
/// error, for every reader, until a restart tries again or a stop clears it.
async fn spawn_failure_stays_failed() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let config =
            "project: acme\nservices:\n  api: { run: sleep }\n  missing: { run: nospawn }\n";
        let main = repo(&tmp.path, "r", config);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        let _ = h.call(Method::Start, json!({}), cx(&main)).await;

        let lane = first_lane(&h, &main).await;
        assert_eq!(lane.state, "failed");
        let missing = unit_summary(&h, &main, "missing").await.expect("missing");
        assert_eq!(missing.state, "failed");
        assert_eq!(missing.reason.expect("reason").code, "spawn_failed");

        let waited = h
            .call(
                Method::Wait,
                json!({"target": "missing", "timeout_ms": 5_000}),
                cx(&main),
            )
            .await
            .expect("wait");
        assert_eq!(waited["state"], "failed", "{waited}");
        assert_eq!(waited["reason"]["code"], "spawn_failed", "{waited}");
        assert!(
            waited["log_tail"].to_string().contains("could not start"),
            "{waited}"
        );
        let errors = h
            .call(Method::Errors, json!({}), cx(&main))
            .await
            .expect("errors");
        let listed = errors["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["unit"] == "missing"));
        assert!(listed, "{errors}");

        // A restart tries the spawn again instead of finding it running.
        let restarted = h
            .call(Method::Restart, json!({"units": ["missing"]}), cx(&main))
            .await;
        assert_unit_failed(restarted, "missing", "spawn_failed");

        let stopped = h
            .call(Method::Stop, json!({"units": ["missing"]}), cx(&main))
            .await
            .expect("stop");
        assert_eq!(stopped["items"][0]["name"], "missing", "{stopped}");
        assert_eq!(stopped["items"][0]["state"], "stopped", "{stopped}");
        assert_eq!(first_lane(&h, &main).await.state, "running");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A run that failed its ready check and then exits on its own is recorded
/// as finished, so `errors` counts its crash.
async fn ready_timeout_then_exit_is_recorded() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let config = "project: acme\nservices:\n  slow: { run: \"hold; exit 3\", ready: { log: never, timeout: 100ms } }\n";
        let main = repo(&tmp.path, "r", config);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        let _ = h.call(Method::Start, json!({}), cx(&main)).await;
        let timed_out = crate::eventually(std::time::Duration::from_secs(5), || async {
            let reason = unit_summary(&h, &main, "slow").await?.reason?;
            (reason.code == "ready_timeout").then_some(())
        })
        .await;
        assert!(timed_out.is_some(), "the ready check fails first");
        // Only now does the unit exit, so its exit is no state change.
        h.runner.release();
        let crashed = crate::eventually(std::time::Duration::from_secs(5), || async {
            let errors = h.call(Method::Errors, json!({}), cx(&main)).await.ok()?;
            let slow = errors["items"]
                .as_array()?
                .iter()
                .find(|item| item["unit"] == "slow")?
                .clone();
            (slow["crashes"] == 1).then_some(slow)
        })
        .await;
        assert!(crashed.is_some(), "the exit is recorded");
        let slow = unit_summary(&h, &main, "slow").await.expect("slow");
        assert_eq!(slow.reason.expect("reason").code, "ready_timeout");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A service whose config does not resolve reads failed with the error and
/// its log says why, while a running one keeps its process.
async fn unresolved_service_is_failed_unless_running() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let valid = "project: acme\nservices:\n  bad: { run: sleep }\n";
        let invalid =
            "project: acme\nservices:\n  bad: { run: sleep, cwd: \"${{ self.port }}\" }\n";
        let main = repo(&tmp.path, "r", invalid);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;

        let _ = h.call(Method::Start, json!({}), cx(&main)).await;
        let bad = unit_summary(&h, &main, "bad").await.expect("bad");
        assert_eq!(bad.state, "failed");
        assert_eq!(bad.reason.expect("reason").code, "config_error");
        let logs = h
            .call(
                Method::Logs,
                json!({"units": ["bad"], "tail": 10}),
                cx(&main),
            )
            .await
            .expect("logs");
        assert!(logs.to_string().contains("could not start"), "{logs}");

        std::fs::write(main.join("devlanes.yaml"), valid).expect("fix");
        let started = h
            .call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        assert_eq!(started["ready"], true, "{started}");
        let pid = unit_summary(&h, &main, "bad").await.expect("bad").pid;
        assert!(pid.is_some());

        std::fs::write(main.join("devlanes.yaml"), invalid).expect("break");
        let again = h.call(Method::Start, json!({}), cx(&main)).await;
        assert_unit_failed(again, "bad", "config_error");
        let bad = unit_summary(&h, &main, "bad").await.expect("bad");
        assert_eq!((bad.state.as_str(), bad.pid), ("ready", pid));
        let stopped = h
            .call(Method::Stop, json!({"units": ["bad"]}), cx(&main))
            .await
            .expect("stop");
        assert_eq!(stopped["items"][0]["state"], "stopped", "{stopped}");
        h.close().await;
    })
    .await
    .expect("timeout");
}
