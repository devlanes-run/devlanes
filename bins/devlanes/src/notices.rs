//! Release and version notices. After a command in human mode, one stderr line
//! per notice, each at most once a day; `doctor` always shows the facts. The
//! daemon finds releases; nothing here downloads, installs or restarts anything.

use devlanes_cli::{OutputMode, Rendered, Runtime};
use devlanes_client::{Client, Paths};
use devlanes_proto::{
    Context, Hello, Method,
    release::{ReleaseCheck, UpdateInfo},
};
use devlanes_update::{
    InstallMethod, NoticeLog, now_ms, skew_key, skew_line, update_command, update_key, update_line,
    version::{Bump, bump},
};
use serde_json::{Value, json};

use crate::output::emit;

/// This binary's version.
const OWN: &str = env!("CARGO_PKG_VERSION");

/// Prints due notices after `method` completed. Human mode outside CI only;
/// `daemon status` already reports a version difference itself.
pub async fn after_command(client: &mut Client, runtime: &Runtime, paths: &Paths, method: Method) {
    if runtime.mode != OutputMode::Human || runtime.quiet || runtime.ci.is_some() {
        return;
    }
    if matches!(method, Method::DaemonShutdown | Method::DaemonResetState) {
        return;
    }
    let daemon = (method != Method::DaemonStatus)
        .then(|| client.peer.version.clone())
        .filter(|daemon| daemon != OWN);
    let release = client
        .request(Method::DaemonStatus, json!({}), Context::default())
        .await
        .ok()
        .and_then(|status| status.get("update").cloned())
        .and_then(|update| serde_json::from_value::<UpdateInfo>(update).ok())
        .and_then(|update| Some((bump(OWN, &update.latest)?, update.latest)));
    if daemon.is_none() && release.is_none() {
        return;
    }
    let home = paths.home.clone();
    let lines = tokio::task::spawn_blocking(move || {
        let mut log = NoticeLog::load(&home);
        let lines = due_lines(
            &mut log,
            now_ms(),
            daemon.as_deref(),
            release
                .as_ref()
                .map(|(bump, latest)| (latest.as_str(), *bump)),
            |latest| update_command(&InstallMethod::current(), latest),
        );
        if !lines.is_empty() {
            // A lost write only repeats the notice sooner.
            let _ = log.save(&home);
        }
        lines
    })
    .await
    .unwrap_or_default();
    emit(&Rendered {
        stderr: lines,
        ..Rendered::default()
    });
}

/// The notice lines due at `now`, marked as shown in `log`: a daemon on another
/// version, then a newer release `bump` away with `command` for it.
fn due_lines(
    log: &mut NoticeLog,
    now: u64,
    daemon: Option<&str>,
    release: Option<(&str, Bump)>,
    command: impl Fn(&str) -> String,
) -> String {
    let mut lines = String::new();
    if let Some(daemon) = daemon {
        let key = skew_key(daemon, OWN);
        if log.due(&key, now) {
            lines.push_str(&format!("{}\n", skew_line(daemon, OWN)));
            log.mark_shown(&key, now);
        }
    }
    if let Some((latest, bump)) = release {
        let key = update_key(latest);
        if log.due(&key, now) {
            let line = update_line(latest, OWN, bump, &command(latest));
            lines.push_str(&format!("{line}\n"));
            log.mark_shown(&key, now);
        }
    }
    lines
}

/// Adds `release` to doctor data: this binary's version, build, install method
/// and the path that runs the install, the daemon's release check (mode,
/// channel, source, latest release and bump), and the command that installs
/// the latest release. Without a daemon (`client` is `None`) the release check
/// and the daemon's version are unknown; this binary's facts are still there.
pub async fn annotate_doctor(client: Option<&mut Client>, data: &mut Value) {
    let (status, daemon_version) = match client {
        Some(client) => (
            client
                .request(Method::DaemonStatus, json!({}), Context::default())
                .await
                .unwrap_or(Value::Null),
            Some(client.peer.version.clone()),
        ),
        None => (Value::Null, None),
    };
    let check: ReleaseCheck = status
        .get("release_check")
        .cloned()
        .and_then(|check| serde_json::from_value(check).ok())
        .unwrap_or_default();
    let (method, entry) = tokio::task::spawn_blocking(InstallMethod::current_with_entry)
        .await
        .unwrap_or_else(|_| {
            (
                InstallMethod::Script(Default::default()),
                Default::default(),
            )
        });
    let latest = check.latest.clone();
    let target = latest.as_deref().unwrap_or(OWN);
    let reported = status
        .get("update")
        .cloned()
        .and_then(|update| serde_json::from_value::<UpdateInfo>(update).ok());
    if let Some(map) = data.as_object_mut() {
        map.insert(
            "release".into(),
            json!({
                "version": OWN,
                "build": Hello::current(0).build_id,
                "daemon_version": daemon_version,
                "install_method": method.describe(
                    std::env::var_os("HOME").map(std::path::PathBuf::from).as_deref()
                ),
                "entry": entry,
                "command": update_command(&method, target),
                "check": check.state,
                "check_off_reason": check.reason,
                "mode": (!check.mode.is_empty()).then_some(check.mode),
                "channel": check.channel,
                "source": check.source,
                "latest": latest,
                "checked_at": check.checked_at,
                // What this binary is behind by, which the daemon's bump may not
                // be when the two run different versions.
                "bump": reported
                    .as_ref()
                    .and_then(|update| bump(OWN, &update.latest))
                    .map(Bump::as_str),
                "update_available": reported
                    .is_some_and(|update| bump(OWN, &update.latest).is_some()),
            }),
        );
    }
    #[cfg(windows)]
    remove_old_copies(data).await;
}

/// Deletes the `devlanes.exe.old-*` copies `install.ps1` left next to this
/// executable and adds what happened to doctor's `release` facts. A copy that
/// still runs (the daemon before its restart) stays until a later doctor.
#[cfg(windows)]
async fn remove_old_copies(data: &mut Value) {
    let copies = tokio::task::spawn_blocking(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(devlanes_update::remove_old_copies))
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default();
    if let Some(release) = data.get_mut("release").and_then(Value::as_object_mut) {
        release.insert(
            "old_copies".into(),
            json!({"removed": copies.removed, "in_use": copies.in_use}),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_show_once_a_day_per_version() {
        let mut log = NoticeLog::default();
        let command = |v: &str| format!("brew upgrade devlanes-run/tap/devlanes # {v}");
        let lines = due_lines(
            &mut log,
            1_000,
            Some("0.0.1"),
            Some(("99.0.0", Bump::Minor)),
            command,
        );
        assert_eq!(
            lines,
            format!(
                "daemon runs 0.0.1, this CLI is {OWN}; run devlanes daemon restart when convenient (it stops services)\n\
                 devlanes 99.0.0 is available (this is {OWN}): brew upgrade devlanes-run/tap/devlanes # 99.0.0 (breaking changes)\n"
            )
        );
        assert_eq!(
            due_lines(
                &mut log,
                2_000,
                Some("0.0.1"),
                Some(("99.0.0", Bump::Minor)),
                command
            ),
            ""
        );
        // A newer release is news at once; the old one waits a day.
        assert_eq!(
            due_lines(
                &mut log,
                3_000,
                None,
                Some(("99.0.1", Bump::Patch)),
                command
            ),
            format!(
                "devlanes 99.0.1 is available (this is {OWN}): brew upgrade devlanes-run/tap/devlanes # 99.0.1\n"
            )
        );
        let day = 24 * 3600 * 1000;
        assert_eq!(
            due_lines(&mut log, 1_000 + day, Some("0.0.1"), None, command)
                .lines()
                .count(),
            1
        );
    }
}
