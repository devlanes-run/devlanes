//! `status --statusline`: the line for each lane state, its colors and the
//! stdin fields it reads.

use std::path::PathBuf;

use devlanes_cli::{StatuslineInput, statusline};
use devlanes_proto::LaneSummary;
use serde_json::{Value, json};

use crate::render::{lane, unit};

/// A [`LaneSummary`] from the render fixtures' JSON.
fn summary(value: Value) -> LaneSummary {
    serde_json::from_value(value).expect("lane summary")
}

/// The line without colors.
fn plain(value: Value) -> String {
    statusline(&summary(value), None, false)
}

#[test]
fn a_running_lane_shows_its_ready_count_and_first_url() {
    let running = lane(
        Some(1),
        "feat/billing",
        "running",
        vec![
            unit("worker", "ready"),
            unit("web", "ready"),
            unit("api", "ready"),
        ],
    );
    assert_eq!(
        plain(running),
        "acme@1 3/3 ready · web web.acme2.localhost:8443"
    );
}

#[test]
fn a_failed_unit_comes_first_with_the_ready_count() {
    let failed = lane(
        Some(1),
        "feat/billing",
        "failed",
        vec![
            unit("api", "ready"),
            unit("admin", "failed"),
            unit("web", "ready"),
        ],
    );
    assert_eq!(plain(failed), "acme@1 ✕ admin failed · 2/3");
    let mut lane_failed = lane(Some(1), "feat/billing", "failed", vec![unit("api", "idle")]);
    lane_failed["reason"] = json!({"code": "docker_unavailable", "data": {}, "exit_code": 1});
    assert_eq!(plain(lane_failed), "acme@1 ✕ failed · 0/1");
}

#[test]
fn the_sessions_own_units_come_first() {
    let mut mailer = unit("mailer", "failed");
    mailer["actor"] = json!({"kind": "agent", "agent_id": "claude-code", "session_id": "s1"});
    let mut probe = unit("probe", "ready");
    probe["kind"] = json!("exec");
    probe["actor"] = mailer["actor"].clone();
    let failed = summary(lane(
        Some(1),
        "feat/billing",
        "failed",
        vec![unit("admin", "failed"), mailer, unit("api", "ready")],
    ));
    assert_eq!(
        statusline(&failed, None, false),
        "acme@1 ✕ admin +1 failed · 1/3"
    );
    assert_eq!(
        statusline(&failed, Some("s1"), false),
        "acme@1 ✕ mailer +1 failed · 1/3"
    );
    let running = summary(lane(
        Some(1),
        "feat/billing",
        "running",
        vec![unit("api", "ready"), probe],
    ));
    assert_eq!(
        statusline(&running, Some("s1"), false),
        "acme@1 1/1 ready · probe probe.acme2.localhost:8443",
        "the exec unit is not a service but its URL is the session's"
    );
}

#[test]
fn starting_idle_and_frozen_lanes_say_so() {
    let starting = lane(
        Some(1),
        "feat/billing",
        "starting",
        vec![
            unit("api", "ready"),
            unit("web", "starting"),
            unit("search", "queued"),
        ],
    );
    assert_eq!(plain(starting), "acme@1 starting 1/3 ready");
    let mut frozen = lane(
        Some(0),
        "main",
        "running",
        vec![unit("api", "ready"), unit("web", "ready")],
    );
    frozen["power"] = json!({"level": "frozen", "since_ms": 1});
    assert_eq!(plain(frozen.clone()), "acme@0 ❄ frozen · 2/2");
    frozen["power"]["level"] = json!("throttled");
    assert_eq!(plain(frozen), "acme@0 throttled · 2/2");
    assert_eq!(
        plain(lane(
            Some(3),
            "fix/login",
            "idle",
            vec![unit("api", "idle")]
        )),
        "acme@3 idle"
    );
    assert_eq!(
        plain(lane(Some(3), "fix/login", "running", vec![])),
        "acme@3 running"
    );
}

#[test]
fn colors_mark_ready_and_failed_and_no_color_drops_them() {
    let ready = summary(lane(
        Some(1),
        "feat/billing",
        "running",
        vec![unit("api", "ready")],
    ));
    assert_eq!(
        statusline(&ready, None, true),
        "acme@1 \x1b[32m1/1 ready\x1b[0m · api api.acme2.localhost:8443"
    );
    let failed = summary(lane(
        Some(1),
        "feat/billing",
        "failed",
        vec![unit("api", "failed")],
    ));
    assert_eq!(
        statusline(&failed, None, true),
        "acme@1 \x1b[31m✕\x1b[0m api failed · 0/1"
    );
    assert!(!statusline(&failed, None, false).contains('\x1b'));
}

#[test]
fn input_prefers_the_workspace_directory() {
    let input = StatuslineInput::parse(
        r#"{"cwd": "/work/a", "workspace": {"current_dir": "/work/b"}, "session_id": "s1", "model": {"id": "x"}}"#,
    );
    assert_eq!(input.cwd, Some(PathBuf::from("/work/b")));
    assert_eq!(input.session_id.as_deref(), Some("s1"));
    assert_eq!(
        StatuslineInput::parse(r#"{"cwd": "/work/a"}"#).cwd,
        Some(PathBuf::from("/work/a"))
    );
    assert_eq!(
        StatuslineInput::parse("not json"),
        StatuslineInput::default()
    );
    assert_eq!(StatuslineInput::parse(""), StatuslineInput::default());
}
