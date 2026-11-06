//! `notifications/progress` for a start: a second child follows the daemon's
//! events and each `lane.phase` event of the started lane becomes one
//! notification.

use std::{path::Path, time::Duration};

use rmcp::{
    model::{ProgressNotificationParam, ProgressToken},
    service::{Peer, RoleServer},
};
use serde_json::Value;
use tokio::io::AsyncBufReadExt;

use crate::child::Runner;

/// Follows `devlanes debug events -f` in `cwd` for `limit` and reports phases
/// to `peer` until the returned future is dropped.
pub async fn follow(
    runner: &Runner,
    cwd: &Path,
    limit: Duration,
    peer: Peer<RoleServer>,
    token: ProgressToken,
) {
    // A little longer than the start, which ends this follow by finishing first.
    let timeout = format!("{}s", limit.as_secs() + 5);
    let mut command = runner.command(cwd);
    command.args([
        "--output",
        "json",
        "debug",
        "events",
        "-f",
        "--timeout",
        &timeout,
    ]);
    let Ok(mut child) = command.spawn() else {
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        return;
    };
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let mut phases = Phases::default();
    while let Ok(Some(line)) = lines.next_line().await {
        let Some(message) = phases.message(&line) else {
            continue;
        };
        let param = ProgressNotificationParam::new(token.clone(), phases.count as f64)
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

    /// Phases of the first lane count up; other events and lanes are skipped.
    #[test]
    fn phases_of_one_lane() {
        let mut phases = Phases::default();
        let event = |lane: &str, phase: &str, state: &str| {
            serde_json::json!({"event": "lane.phase", "seq": 1, "data": {"lane": lane, "phase": phase, "state": state}})
                .to_string()
        };
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
}
