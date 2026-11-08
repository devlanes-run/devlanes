//! `status --statusline`: one short line for the lane of a directory, for the
//! status line of an editor or agent, such as Claude Code's `statusLine` command.

use std::path::{Path, PathBuf};

use devlanes_proto::{LaneSummary, UnitSummary};
use serde_json::Value;

use super::{clean, lanes};

/// The fields `status --statusline` reads from the JSON a status line host
/// writes on stdin. Claude Code sends many more.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatuslineInput {
    /// `workspace.current_dir`, else `cwd`.
    pub cwd: Option<PathBuf>,
    /// The host's session id, which is also the session id of the units its
    /// agent started.
    pub session_id: Option<String>,
}

impl StatuslineInput {
    /// The fields of `text`; missing ones, and all of them for text that is
    /// not JSON, stay empty.
    pub fn parse(text: &str) -> Self {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return Self::default();
        };
        let field = |pointer: &str| {
            value
                .pointer(pointer)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        Self {
            cwd: field("/workspace/current_dir")
                .or_else(|| field("/cwd"))
                .map(PathBuf::from),
            session_id: field("/session_id"),
        }
    }
}

/// The lane whose worktree holds `cwd`, picked as the daemon picks the lane of
/// a command's directory: the deepest worktree that still exists, and a
/// folder's own lane before the extra lanes `lane new` added to it.
pub fn statusline_lane<'a>(lanes: &'a [LaneSummary], cwd: &Path) -> Option<&'a LaneSummary> {
    lanes
        .iter()
        .filter(|lane| {
            lane.reason
                .as_ref()
                .is_none_or(|reason| reason.code != "worktree_missing")
        })
        .filter(|lane| devlanes_core::paths::path_starts_with(cwd, &lane.path))
        .max_by_key(|lane| (lane.path.components().count(), lane.name.is_none()))
}

/// The status line of `lane`, without a newline:
///
/// - `acme@1 3/3 ready · web acme1.localhost` when it runs, with the first
///   proxied URL of a ready or running unit;
/// - `acme@1 ✕ api failed · 2/3` when a unit failed (`api +1 failed` for more);
/// - `acme@1 starting 1/3 ready` while it starts;
/// - `acme@1 ❄ frozen · 3/3` or `acme@1 throttled · 3/3` while it is idle;
/// - `acme@1 idle` and the like for every other state.
///
/// Units the host's `session` started come first among the failures and URLs.
/// `color` adds SGR colors: red for failures only, green when every service is
/// ready.
pub fn statusline(lane: &LaneSummary, session: Option<&str>, color: bool) -> String {
    let paint = |text: &str, sgr: &str| {
        if color {
            format!("\x1b[{sgr}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    };
    let mine = |unit: &UnitSummary| {
        session.is_some_and(|session| unit.actor.session_id.as_deref() == Some(session))
    };
    let reference = clean(&lane.reference);
    let services = lanes::services(lane);
    let ready = services.iter().filter(|unit| unit.state == "ready").count();
    let count = format!("{ready}/{}", services.len());
    let tally = if services.is_empty() {
        String::new()
    } else {
        format!(" · {count}")
    };
    let mut failed: Vec<&UnitSummary> = lane
        .units
        .iter()
        .filter(|unit| unit.state == "failed")
        .collect();
    failed.sort_by_key(|unit| !mine(unit));
    if let Some(first) = failed.first() {
        let more = match failed.len() {
            1 => String::new(),
            n => format!(" +{}", n - 1),
        };
        return format!(
            "{reference} {} {}{more} failed{tally}",
            paint("✕", "31"),
            clean(first.label())
        );
    }
    match lane.state.as_str() {
        "running" => {}
        "failed" => return format!("{reference} {}{tally}", paint("✕ failed", "31")),
        "starting" if !services.is_empty() => {
            return format!("{reference} {} {count} ready", paint("starting", "33"));
        }
        "starting" | "stopping" => {
            return format!("{reference} {}", paint(&clean(&lane.state), "33"));
        }
        other => return format!("{reference} {}", paint(&clean(other), "2")),
    }
    if let Some(power) = &lane.power {
        let word = if power.level == "frozen" {
            paint("❄ frozen", "34")
        } else {
            paint(&clean(&power.level), "2")
        };
        return format!("{reference} {word}{tally}");
    }
    let mut line = if services.is_empty() {
        format!("{reference} {}", paint("running", "32"))
    } else if ready == services.len() {
        format!("{reference} {}", paint(&format!("{count} ready"), "32"))
    } else {
        format!("{reference} {count} ready")
    };
    if let Some((unit, url)) = shown_url(lane, &mine) {
        line.push_str(&format!(" · {} {url}", clean(unit.label())));
    }
    line
}

/// A ready or running unit's first proxied URL without its scheme
/// (`web.acme1.localhost:8443`), one the `mine` session started first.
fn shown_url<'a>(
    lane: &'a LaneSummary,
    mine: &impl Fn(&UnitSummary) -> bool,
) -> Option<(&'a UnitSummary, String)> {
    let mut units: Vec<&UnitSummary> = lane
        .units
        .iter()
        .filter(|unit| matches!(unit.state.as_str(), "ready" | "running"))
        .collect();
    units.sort_by_key(|unit| !mine(unit));
    units.into_iter().find_map(|unit| {
        let url = unit.urls.iter().find(|url| !lanes::loopback(url))?;
        let bare = url.split_once("://").map_or(url.as_str(), |(_, rest)| rest);
        Some((unit, clean(bare.trim_end_matches('/'))))
    })
}
