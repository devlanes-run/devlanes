//! Opening the TUI again from the binary installed now: after an update, a
//! reload for a newer daemon, or `devlanes setup`. The new TUI reads where to
//! open from [`devlanes_tui::REOPEN_ENV`].

use devlanes_cli::{Runtime, SetupAction};
use devlanes_client::Paths;
use devlanes_proto::RpcError;
use devlanes_tui::{ExitReason, REOPEN_ENV, Relaunch, Reopen};
use serde_json::json;

/// The [`Reopen`] a relaunch passed to this process, if any. A value that does
/// not parse opens the TUI as usual.
///
/// The variable stays in this process's environment (removing it needs
/// `unsafe` since Rust 2024); the TUI leaves it out of the environment it
/// gives lanes, and the next relaunch sets it again.
pub fn reopen() -> Option<Reopen> {
    let text = std::env::var(REOPEN_ENV).ok()?;
    serde_json::from_str(&text).ok()
}

/// Does what the TUI closed for, in this terminal, then opens it again where
/// it was: the update flow, `devlanes setup` (the user confirmed it in the
/// TUI), or nothing for a reload. A failed setup stays on screen with its
/// exit status instead of being covered by the TUI.
pub async fn after(
    runtime: &Runtime,
    paths: &Paths,
    (reason, reopen): Relaunch,
) -> Result<u8, RpcError> {
    match reason {
        ExitReason::Update => crate::update::from_tui(runtime, paths, reopen).await,
        ExitReason::Reload => tui(reopen).await,
        ExitReason::Setup => {
            let code = crate::setup::run(runtime, SetupAction::Apply { yes: true }).await?;
            if code != 0 {
                return Ok(code);
            }
            tui(reopen).await
        }
    }
}

/// [`exec_tui`] off the async threads.
async fn tui(reopen: Reopen) -> Result<u8, RpcError> {
    tokio::task::spawn_blocking(move || exec_tui(&reopen))
        .await
        .map_err(|error| failed(&error))?
}

/// Runs `devlanes ui` from the install's stable path with [`REOPEN_ENV`] set
/// to `reopen`. On Unix the process becomes the new TUI and this returns only
/// when that fails; on Windows it waits for the new TUI and returns its exit
/// status, leaving Ctrl-C to it meanwhile. Blocking.
///
/// It runs `ui`, not this process's arguments: a TUI opened by `devlanes
/// start`, `restart` or `lane new --start` would send that request again.
pub fn exec_tui(reopen: &Reopen) -> Result<u8, RpcError> {
    let exe = devlanes_update::spawn_exe().map_err(|error| failed(&error))?;
    run(tui_command(&exe, reopen)?)
}

/// `<exe> ui` with [`REOPEN_ENV`] set to `reopen`, and without the step
/// block `devlanes update` handed this process, which the TUI's lanes must
/// not inherit.
fn tui_command(exe: &std::path::Path, reopen: &Reopen) -> Result<std::process::Command, RpcError> {
    let reopen = serde_json::to_string(reopen).map_err(|error| failed(&error))?;
    let mut command = std::process::Command::new(exe);
    command
        .arg("ui")
        .env(REOPEN_ENV, reopen)
        .env_remove(crate::lifecycle::STEPS_ENV);
    Ok(command)
}

/// Replaces this process with `command`.
#[cfg(unix)]
fn run(mut command: std::process::Command) -> Result<u8, RpcError> {
    use std::os::unix::process::CommandExt;
    Err(failed(&command.exec()))
}

/// Runs `command` on this console and passes its exit status on. The child
/// starts before this process ignores Ctrl-C, so it does not inherit that.
#[cfg(windows)]
fn run(mut command: std::process::Command) -> Result<u8, RpcError> {
    let mut child = command.spawn().map_err(|error| failed(&error))?;
    // The new TUI handles Ctrl-C itself; this process only waits for it.
    let _ = devlanes_sys_windows::console::set_ctrl_c_ignored(true);
    let status = child.wait();
    let _ = devlanes_sys_windows::console::set_ctrl_c_ignored(false);
    let status = status.map_err(|error| failed(&error))?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1))
}

/// A relaunch that could not start.
fn failed(error: &dyn std::fmt::Display) -> RpcError {
    RpcError::new(
        "tui_failed",
        json!({"message": format!("could not open the TUI again: {error}")}),
        1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The new TUI gets where to open, never the update's step block.
    #[test]
    fn the_new_tui_gets_the_reopen_request_alone() {
        let reopen = Reopen {
            lane: Some("acme@1".into()),
            view: None,
        };
        let command = tui_command(std::path::Path::new("devlanes"), &reopen).expect("command");
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(envs.contains(&(
            REOPEN_ENV.to_owned(),
            Some(r#"{"lane":"acme@1","view":null}"#.to_owned())
        )));
        assert!(envs.contains(&(crate::lifecycle::STEPS_ENV.to_owned(), None)));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [std::ffi::OsStr::new("ui")]
        );
    }
}
