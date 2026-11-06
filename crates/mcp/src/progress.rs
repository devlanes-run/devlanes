//! `notifications/progress` for a start: a second child follows the daemon's
//! events and each `lane.phase` event of the started lane becomes one
//! notification.

use std::{path::Path, process::Stdio, time::Duration};

use rmcp::{
    model::{ProgressNotificationParam, ProgressToken},
    service::{Peer, RoleServer},
};
use serde_json::Value;
use tokio::io::AsyncBufReadExt;

use crate::child::Runner;

/// Follows `devlanes debug events -f` in `cwd` for `limit` and reports phases
/// to `peer` until the returned future is dropped. `phases` carries the count
/// over to the next run of the same call, so progress never goes back.
pub async fn follow(
    runner: &Runner,
    cwd: &Path,
    limit: Duration,
    peer: Peer<RoleServer>,
    token: ProgressToken,
    phases: &mut Phases,
) {
    // A little longer than the start, which ends this follow by finishing first.
    let timeout = format!("{}s", limit.as_secs() + 5);
    let mut command = runner.command(cwd);
    command
        .args([
            "--output",
            "json",
            "debug",
            "events",
            "-f",
            "--timeout",
            &timeout,
        ])
        // Nobody reads it, and a full pipe would stall the follow.
        .stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        return;
    };
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Some(message) = phases.message(&line) else {
            continue;
        };
        let param = ProgressNotificationParam::new(token.clone(), f64::from(phases.count))
            .with_message(message);
        if peer.notify_progress(param).await.is_err() {
            break;
        }
    }
    // `kill_on_drop` ends the child when this future is dropped early.
}

/// The lane whose phases are reported, and how many were.
#[derive(Default)]
pub struct Phases {
    lane: Option<String>,
    pub count: u32,
}

impl Phases {
    /// Phases of `lane` when the call names it as `project@index`, the form
    /// events use; otherwise of the first lane an event names.
    pub fn of(lane: Option<&str>) -> Self {
        let canonical = lane.filter(|lane| {
            lane.split_once('@').is_some_and(|(project, index)| {
                !project.is_empty()
                    && !index.is_empty()
                    && index.bytes().all(|byte| byte.is_ascii_digit())
            })
        });
        Self {
            lane: canonical.map(str::to_owned),
            count: 0,
        }
    }

    /// The progress message for one NDJSON event line, such as
    /// `acme@1 infra started`; `None` for other events and other lanes. The first
    /// lane seen is the started one, so a start elsewhere does not mix in.
    pub fn message(&mut self, line: &str) -> Option<String> {
        let event: Value = serde_json::from_str(line).ok()?;
        if event.get("event").and_then(Value::as_str) != Some(devlanes_proto::events::PHASE) {
            return None;
        }
        let data = event.get("data")?;
        let lane = data.get("lane").and_then(Value::as_str)?;
        if self.lane.get_or_insert_with(|| lane.to_owned()) != lane {
            return None;
        }
        let phase = data.get("phase").and_then(Value::as_str).unwrap_or("start");
        let state = data
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("progress");
        self.count += 1;
        Some(format!("{lane} {phase} {state}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `lane.phase` event line.
    fn event(lane: &str, phase: &str, state: &str) -> String {
        serde_json::json!({"event": "lane.phase", "seq": 1, "data": {"lane": lane, "phase": phase, "state": state}})
            .to_string()
    }

    /// Phases of the first lane count up; other events and lanes are skipped.
    #[test]
    fn phases_of_one_lane() {
        let mut phases = Phases::default();
        assert_eq!(
            phases
                .message(&event("acme@1", "infra", "started"))
                .as_deref(),
            Some("acme@1 infra started")
        );
        assert_eq!(phases.message(&event("acme@2", "infra", "started")), None);
        assert_eq!(
            phases.message("{\"event\":\"unit.changed\",\"data\":{}}"),
            None
        );
        assert_eq!(phases.message("not json"), None);
        assert_eq!(
            phases
                .message(&event("acme@1", "services", "done"))
                .as_deref(),
            Some("acme@1 services done")
        );
        assert_eq!(phases.count, 2);
    }

    /// A call that names its lane as `project@index` follows that lane from
    /// the first event; other references wait for the first event.
    #[test]
    fn the_calls_lane_comes_first() {
        let mut named = Phases::of(Some("acme@2"));
        assert_eq!(named.message(&event("acme@1", "infra", "started")), None);
        assert!(
            named
                .message(&event("acme@2", "infra", "started"))
                .is_some()
        );
        for other in [Some("2"), Some("feat/x"), Some("acme@"), None] {
            let mut phases = Phases::of(other);
            assert!(
                phases
                    .message(&event("acme@1", "infra", "started"))
                    .is_some(),
                "{other:?}"
            );
        }
    }
}
