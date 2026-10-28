//! The resume plan of a replacing daemon (`daemon restart`): once it announced
//! readiness, the engine starts the lanes that ran before, as ordinary starts.

use std::{path::Path, time::Duration};

use devlanes_daemon::Handler;
use devlanes_proto::{Method, ResumeLane, ResumePlan, events};
use serde_json::{Value, json};

use crate::{Harness, LIMIT, Setup, client_env, cx, eventually, harness, repo, temp};

/// Two services, so a resume that names one leaves the other alone.
const CONFIG: &str = "project: back\nservices:\n  api: { run: sleep }\n  web: { run: sleep }\n";

/// `status` of the lane at `cwd`.
async fn status(h: &Harness, cwd: &Path) -> Value {
    h.call(Method::Status, json!({}), cx(cwd))
        .await
        .expect("status")
}

/// The state of `unit` in the lane of `status`.
fn unit_state(status: &Value, unit: &str) -> Option<String> {
    status["items"][0]["units"]
        .as_array()?
        .iter()
        .find(|entry| entry["name"] == unit)
        .and_then(|entry| entry["state"].as_str())
        .map(str::to_owned)
}

#[tokio::test(flavor = "multi_thread")]
/// Nothing starts before readiness; afterwards the planned services start
/// with their events on the bus, and a lane that is gone is skipped.
async fn a_resume_plan_starts_its_lanes_after_readiness() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let home = tmp.path.join("home");
        let first = harness(&home, Setup::default()).await;
        first
            .call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        first.close().await;
        drop(first);
        let plan = ResumePlan {
            lanes: vec![
                ResumeLane {
                    lane: "gone@4".into(),
                    units: vec!["api".into()],
                    profiles: Vec::new(),
                },
                ResumeLane {
                    lane: "back@0".into(),
                    units: vec!["api".into()],
                    profiles: Vec::new(),
                },
            ],
            env: client_env(),
            ..ResumePlan::default()
        };
        let second = harness(
            &home,
            Setup {
                resume: Some(plan),
                ..Setup::default()
            },
        )
        .await;
        let before = status(&second, &main).await;
        assert_ne!(
            unit_state(&before, "api").as_deref(),
            Some("ready"),
            "{before}"
        );
        let mut bus = second.engine.subscribe();
        second.engine.on_ready();
        let ready = eventually(Duration::from_secs(20), || async {
            let status = status(&second, &main).await;
            (unit_state(&status, "api").as_deref() == Some("ready")).then_some(status)
        })
        .await
        .expect("api runs again");
        assert_ne!(
            unit_state(&ready, "web").as_deref(),
            Some("ready"),
            "{ready}"
        );
        let mut phases = 0;
        while let Ok(event) = bus.try_recv() {
            if event.event == events::PHASE && event.data["lane"] == "back@0" {
                phases += 1;
            }
        }
        assert!(phases > 0, "the start's phases reach subscribers");
        // Used once: a second readiness starts nothing more.
        second.engine.on_ready();
        second.close().await;
    })
    .await
    .expect("timeout");
}
