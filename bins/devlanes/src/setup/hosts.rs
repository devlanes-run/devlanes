//! The `hosts` step: the root-owned `/etc/hosts` helper and its sudo rule, on
//! macOS and Linux. The commands and their checks live in
//! `devlanes_resolver_hosts::install`; this module picks the helper binary,
//! reports, and makes a running daemon retry at once.
//!
//! devlanes never writes the hosts file on Windows, so there the step is never
//! needed; a failing `*.localhost` probe only gets a warning.

#[cfg(unix)]
use std::{ffi::OsString, path::PathBuf};
use std::{path::Path, sync::Arc};

#[cfg(unix)]
use devlanes_cli::{Rendered, Runtime};
#[cfg(unix)]
use devlanes_client::{Client, Paths};
#[cfg(unix)]
use devlanes_proto::{Context, Method, RpcError};
#[cfg(windows)]
use devlanes_resolver_hosts::LocalhostResolution;
use devlanes_resolver_hosts::{
    FileKind, FsProbe, HostsClient, INSTALL_PATH, REVISION, SystemExecutor, SystemFs,
};
#[cfg(unix)]
use devlanes_resolver_hosts::{
    InstallError,
    install::{self, HELPER_SOURCE_ENV},
};
use serde_json::Value;
#[cfg(unix)]
use serde_json::json;

#[cfg(unix)]
use crate::output::emit;

/// Marker the helper writes first; its presence means a block exists.
#[cfg(unix)]
const BLOCK_BEGIN: &str = "# devlanes begin";

/// `devlanes_daemon::hosts::helper_check` of the installed helper, `None` when
/// none is installed. Runs `version` without sudo, so it never prompts.
pub(crate) async fn installed_helper() -> Option<Value> {
    if !helper_present() {
        return None;
    }
    let client = HostsClient::new(Arc::new(SystemExecutor), Arc::new(SystemFs), None).ok()?;
    Some(devlanes_daemon::hosts::helper_check(
        &client.version().await,
        REVISION,
    ))
}

/// Whether a regular file sits at the helper's install path.
fn helper_present() -> bool {
    matches!(
        SystemFs.lstat(Path::new(INSTALL_PATH)),
        Ok(Some(info)) if info.kind == FileKind::File
    )
}

/// The warning for a `*.localhost` probe that did not answer loopback; devlanes
/// writes no hosts file on Windows, so this names what the user can do instead.
#[cfg(windows)]
pub(super) fn windows_warning(resolution: LocalhostResolution) -> Option<String> {
    let word = match resolution {
        LocalhostResolution::Loopback => return None,
        LocalhostResolution::NotLoopback => "answered with an address that is not loopback",
        LocalhostResolution::Fails => "failed",
    };
    Some(format!(
        "warning: a test lookup of a *.localhost name {word}, and devlanes does not write the hosts file on Windows; add the lane hostnames to %SystemRoot%\\System32\\drivers\\etc\\hosts from an elevated shell, or set hostnames.resolver: resolver and hostnames.tld, then run devlanes setup\n"
    ))
}

/// Installs the helper and its sudo rule, then asks a running daemon to reapply.
#[cfg(unix)]
pub(super) async fn install(runtime: &Runtime) -> Result<u8, RpcError> {
    let source = helper_source(
        &SystemFs,
        std::env::var_os(HELPER_SOURCE_ENV),
        std::env::current_exe(),
    )
    .map_err(install_error)?;
    let executor = super::sudo::privileged(runtime, super::APPLY).await?;
    let version = install::install(executor.as_ref(), &SystemFs, &source)
        .await
        .map_err(install_error)?;
    let mut stdout = format!(
        "installed devlanes-hosts {} at {INSTALL_PATH}\n",
        version.version
    );
    let mut code = 0;
    if let Some(hosts) = retry_daemon().await {
        match hosts.get("state").and_then(Value::as_str) {
            Some("applied") => stdout.push_str(&format!(
                "the daemon wrote {} names to /etc/hosts\n",
                hosts.get("names").and_then(Value::as_u64).unwrap_or(0)
            )),
            Some("failed") => {
                code = 1;
                let message = hosts
                    .pointer("/last_error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                stdout.push_str(&format!(
                    "the daemon could not update /etc/hosts: {message}\n"
                ));
            }
            _ => {}
        }
    }
    if runtime.quiet && code == 0 {
        stdout.clear();
    }
    emit(&Rendered {
        stdout,
        ..Rendered::default()
    });
    Ok(code)
}

/// Clears the block, removes the rule and the helper, and prints what could not
/// be done. Without a helper and a block nothing runs, so machines that never
/// installed it get no sudo prompt.
#[cfg(unix)]
pub(super) async fn remove(runtime: &Runtime) -> Result<u8, RpcError> {
    let had_helper = helper_present();
    let block = block_present().await;
    if !had_helper && !block {
        if !runtime.quiet {
            emit(&Rendered {
                stdout: "no /etc/hosts helper to remove\n".into(),
                ..Rendered::default()
            });
        }
        return Ok(0);
    }
    let executor = super::sudo::privileged(runtime, super::REMOVE).await?;
    let warnings = install::uninstall(executor.as_ref(), &SystemFs)
        .await
        .map_err(install_error)?;
    let mut stderr: String = warnings
        .iter()
        .map(|warning| format!("warning: {warning}\n"))
        .collect();
    // Without the helper nothing can clear the block; the README says to edit it by hand.
    if !had_helper && block {
        stderr.push_str(
            "warning: the helper was already gone, so the devlanes block is still in /etc/hosts; delete the lines from `# devlanes begin` to `# devlanes end` by hand\n",
        );
    }
    // The daemon's next attempt reports `helper_missing` instead of a stale success.
    let _ = retry_daemon().await;
    emit(&Rendered {
        stdout: if runtime.quiet {
            String::new()
        } else {
            format!("removed {INSTALL_PATH} and its sudo rule\n")
        },
        stderr,
        ..Rendered::default()
    });
    Ok(0)
}

/// The helper to install: `DEVLANES_HOSTS_HELPER` when set, else `devlanes-hosts`
/// next to this binary (also next to the target of a symlinked `devlanes`).
#[cfg(unix)]
fn helper_source(
    fs: &dyn FsProbe,
    override_path: Option<OsString>,
    current_exe: std::io::Result<PathBuf>,
) -> Result<PathBuf, InstallError> {
    let override_path = override_path
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    match current_exe {
        Ok(exe) => install::locate_helper_source(fs, override_path, &exe),
        // Without our own path only the override can name the binary.
        Err(_) => match override_path {
            Some(path) => install::locate_helper_source(fs, Some(path), Path::new("/")),
            None => Err(InstallError::SourceMissing(Vec::new())),
        },
    }
}

/// Maps installer failures to the CLI's error shape, keeping their stable codes.
#[cfg(unix)]
fn install_error(error: InstallError) -> RpcError {
    let exit = match error {
        InstallError::Unsupported => 2,
        _ => 1,
    };
    RpcError::new(error.code(), json!({"message": error.to_string()}), exit)
}

/// Whether `/etc/hosts` (world-readable) still holds a devlanes block.
#[cfg(unix)]
async fn block_present() -> bool {
    tokio::fs::read_to_string("/etc/hosts")
        .await
        .is_ok_and(|hosts| hosts.lines().any(|line| line.trim() == BLOCK_BEGIN))
}

/// Asks a running daemon to reapply now; a stopped daemon applies at its next start.
/// Returns the daemon's `hosts` status after the attempt.
#[cfg(unix)]
async fn retry_daemon() -> Option<Value> {
    let paths = Paths::discover().ok()?;
    let mut client = Client::connect(&paths).await.ok()?;
    let reply = client
        .request(Method::HostsRetry, json!({}), Context::default())
        .await
        .ok()?;
    reply.get("hosts").filter(|hosts| !hosts.is_null()).cloned()
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    /// A working probe needs no warning; a failing one names the hosts file and
    /// the resolver strategy.
    #[test]
    fn a_failing_probe_warns_with_the_remedies() {
        assert_eq!(windows_warning(LocalhostResolution::Loopback), None);
        for failed in [LocalhostResolution::Fails, LocalhostResolution::NotLoopback] {
            let warning = windows_warning(failed).expect("warning");
            assert!(warning.starts_with("warning: "), "{warning}");
            assert!(warning.contains("devlanes setup"), "{warning}");
            assert!(warning.contains(r"drivers\etc\hosts"), "{warning}");
        }
    }
}

// The helper and sudo are Unix only; devlanes writes no hosts file on Windows.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A directory with a `devlanes` binary and, when `sibling`, `devlanes-hosts`.
    fn layout(sibling: bool) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("devlanes");
        std::fs::write(&exe, b"").expect("exe");
        if sibling {
            std::fs::write(dir.path().join("devlanes-hosts"), b"").expect("helper");
        }
        (dir, exe)
    }

    #[test]
    fn the_sibling_of_the_running_binary_is_installed() {
        let (dir, exe) = layout(true);
        let found = helper_source(&SystemFs, None, Ok(exe)).expect("sibling");
        assert_eq!(found, dir.path().join("devlanes-hosts"));
        // An empty variable counts as unset.
        let (dir, exe) = layout(true);
        let found = helper_source(&SystemFs, Some(OsString::new()), Ok(exe)).expect("sibling");
        assert_eq!(found, dir.path().join("devlanes-hosts"));
    }

    #[test]
    fn a_symlinked_binary_finds_the_helper_next_to_its_target() {
        let (dir, exe) = layout(true);
        let bin = tempfile::tempdir().expect("bin");
        let link = bin.path().join("devlanes");
        std::os::unix::fs::symlink(&exe, &link).expect("symlink");
        let found = helper_source(&SystemFs, None, Ok(link)).expect("through the link");
        let expected = dir.path().join("devlanes-hosts");
        assert_eq!(
            found.canonicalize().expect("found"),
            expected.canonicalize().expect("expected")
        );
    }

    #[test]
    fn the_override_wins_and_is_not_second_guessed() {
        let (dir, exe) = layout(true);
        let custom = dir.path().join("custom-helper");
        std::fs::write(&custom, b"").expect("custom");
        let found = helper_source(
            &SystemFs,
            Some(custom.clone().into_os_string()),
            Ok(exe.clone()),
        )
        .expect("override");
        assert_eq!(found, custom);
        // A wrong override fails rather than silently using the sibling.
        let missing = dir.path().join("missing");
        let error = helper_source(&SystemFs, Some(missing.clone().into_os_string()), Ok(exe))
            .expect_err("missing override");
        assert!(matches!(error, InstallError::SourceMissing(paths) if paths == [missing]));
    }

    #[test]
    fn a_missing_helper_names_where_it_looked() {
        let (dir, exe) = layout(false);
        let error = helper_source(&SystemFs, None, Ok(exe)).expect_err("missing");
        assert_eq!(error.code(), "helper_source_missing");
        assert!(
            error
                .to_string()
                .contains(&dir.path().join("devlanes-hosts").display().to_string()),
            "{error}"
        );
        let error = helper_source(&SystemFs, None, Err(std::io::Error::other("no exe")))
            .expect_err("no exe");
        assert_eq!(error.code(), "helper_source_missing");
    }
}
