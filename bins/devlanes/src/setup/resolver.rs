//! The `resolver` step on macOS and Linux: the OS resolver file that sends
//! `hostnames.tld` to the daemon's DNS server (`/etc/resolver/<tld>`, or a
//! systemd-resolved delegate), written with sudo. Files without the devlanes
//! marker are never touched.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use devlanes_cli::{Rendered, Runtime};
use devlanes_client::Paths;
use devlanes_proto::RpcError;
use devlanes_resolver_dns::ResolverFile;
use devlanes_resolver_hosts::CommandSpec;
use serde_json::{Value, json};

use crate::output::emit;

/// The OS resolver file that sends `tld` lookups to the daemon's DNS server at
/// `server`. Blocking: on Linux it asks `systemctl` for the systemd version.
pub(crate) fn resolver_file(tld: &str, server: SocketAddr) -> Result<ResolverFile, String> {
    let zone = devlanes_resolver_dns::Zone::new(tld).map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    {
        Ok(devlanes_resolver_dns::macos_resolver(&zone, server))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let systemctl = system_program("systemctl")
            .map_err(|_| "the resolver strategy needs systemd-resolved; systemctl was not found")?;
        let output = std::process::Command::new(systemctl)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| format!("the resolver strategy needs systemd-resolved: {error}"))?;
        let version = systemd_version(&String::from_utf8_lossy(&output.stdout))
            .ok_or("cannot read the systemd version from `systemctl --version`")?;
        devlanes_resolver_dns::systemd_resolver(&zone, server, version)
            .map_err(|error| error.to_string())
    }
}

/// `{path, installed, matches}` for the resolver file of `tld`; `matches` is
/// false when the file routes the TLD elsewhere (another port, a stale
/// install), `{error}` when the OS has no form. Blocking.
pub(crate) fn status(tld: &str, server: SocketAddr) -> Value {
    match resolver_file(tld, server) {
        Ok(file) => {
            let current = std::fs::read_to_string(&file.path).ok();
            json!({
                "path": file.path,
                "installed": current.is_some(),
                "matches": current.as_deref() == Some(file.contents.as_str()),
            })
        }
        Err(error) => json!({"error": error}),
    }
}

/// The version from `systemctl --version`'s first line, `systemd 255 (255.4-1)`.
#[cfg(any(not(target_os = "macos"), test))]
fn systemd_version(output: &str) -> Option<u32> {
    let mut words = output.lines().next()?.split_whitespace();
    (words.next()? == "systemd").then_some(())?;
    words.next()?.parse().ok()
}

/// Absolute path of a root-run program from the system directories, never `PATH`.
pub(super) fn system_program(name: &str) -> Result<String, RpcError> {
    devlanes_trust::find_system_program(&devlanes_trust::SystemExecutor, name)
        .map(|path| path.display().to_string())
        .ok_or_else(|| {
            RpcError::new(
                "tool_missing",
                json!({"message": format!("`{name}` was not found in {:?}", devlanes_trust::SYSTEM_PROGRAM_DIRS)}),
                1,
            )
        })
}

/// Where the last install records the file it wrote, so removal finds that
/// file even after `hostnames.tld` changes.
fn record(paths: &Paths) -> PathBuf {
    paths.home.join("resolver-file")
}

/// What occupies a resolver file path now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Occupant {
    Absent,
    /// Starts with the devlanes marker.
    Devlanes,
    /// Someone else's file (Valet and puma-dev both use `/etc/resolver/test`).
    Foreign,
}

/// Reads `path` to see whose file it is. Blocking.
fn occupant(path: &Path) -> Result<Occupant, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) if devlanes_resolver_dns::is_managed(&contents) => Ok(Occupant::Devlanes),
        Ok(_) => Ok(Occupant::Foreign),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Occupant::Absent),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => Ok(Occupant::Foreign),
        Err(error) => Err(format!("reading {}: {error}", path.display())),
    }
}

/// Files to remove and the file to write.
#[derive(Debug, PartialEq, Eq)]
struct Change {
    remove: Vec<ResolverFile>,
    write: Option<ResolverFile>,
}

/// The refusal for a file at `path` that devlanes did not write.
fn foreign(path: &Path) -> RpcError {
    RpcError::new(
        "resolver_file_foreign",
        json!({
            "path": path,
            "message": format!(
                "{} was not written by devlanes (another tool such as Valet or puma-dev may own it); remove it yourself or choose another hostnames.tld",
                path.display()
            ),
        }),
        1,
    )
}

/// An unreadable resolver file path.
fn unreadable(message: String) -> RpcError {
    RpcError::new("io", json!({"message": message}), 1)
}

/// Writes `file`, the one for the configured TLD, and drops `recorded`, the
/// file the last install wrote, when a TLD change left it behind.
fn install_change(
    file: ResolverFile,
    recorded: Option<ResolverFile>,
    occupant: impl Fn(&Path) -> Result<Occupant, String>,
) -> Result<Change, RpcError> {
    if occupant(&file.path).map_err(unreadable)? == Occupant::Foreign {
        return Err(foreign(&file.path));
    }
    let mut remove = Vec::new();
    if let Some(old) = recorded
        && old.path != file.path
        && occupant(&old.path).map_err(unreadable)? == Occupant::Devlanes
    {
        remove.push(old);
    }
    Ok(Change {
        remove,
        write: Some(file),
    })
}

/// Removes the recorded file, else the one for the configured TLD, when it is ours.
fn removal_change(
    configured: Option<ResolverFile>,
    recorded: Option<ResolverFile>,
    occupant: impl Fn(&Path) -> Result<Occupant, String>,
) -> Result<Change, RpcError> {
    let mut remove = Vec::new();
    if let Some(file) = recorded.or(configured) {
        match occupant(&file.path).map_err(unreadable)? {
            Occupant::Absent => {}
            Occupant::Foreign => return Err(foreign(&file.path)),
            Occupant::Devlanes => remove.push(file),
        }
    }
    Ok(Change {
        remove,
        write: None,
    })
}

/// The file the last install recorded; the record is user-writable, so only
/// devlanes' own paths are accepted. Blocking.
fn recorded(record: &Path) -> Option<ResolverFile> {
    std::fs::read_to_string(record)
        .ok()
        .and_then(|text| devlanes_resolver_dns::generated_file(Path::new(text.trim())))
}

/// Runs blocking work off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| RpcError::new("io", json!({"message": error.to_string()}), 1))
}

/// Writes the resolver file for `tld` and removes the one an earlier TLD left.
pub(super) async fn install(runtime: &Runtime, paths: &Paths, tld: &str) -> Result<u8, RpcError> {
    // The daemon listens on the default address; there is no setting for it.
    let server = devlanes_resolver_dns::default_listen();
    let record = record(paths);
    let (owned, path) = (tld.to_owned(), record.clone());
    let change = blocking(move || {
        let file = resolver_file(&owned, server).map_err(|message| {
            RpcError::new("resolver_unsupported", json!({"message": message}), 2)
        })?;
        install_change(file, recorded(&path), occupant)
    })
    .await??;
    apply(runtime, &change, super::APPLY).await?;
    let written = change.write.as_ref().map(|file| file.path.clone());
    let mut stdout = removed_lines(&change);
    if let Some(path) = &written {
        stdout.push_str(&format!(
            "wrote {}: *.{tld} resolves through the devlanes DNS server at {server}\n",
            path.display()
        ));
    }
    let saved = blocking(move || match written {
        Some(path) => std::fs::write(&record, format!("{}\n", path.display())),
        None => Ok(()),
    })
    .await;
    finish(runtime, stdout, saved);
    Ok(0)
}

/// Removes the resolver file the last install recorded, or the one for `tld`
/// (`hostnames.tld` under the resolver strategy) when there is no record.
pub(super) async fn remove(
    runtime: &Runtime,
    paths: &Paths,
    tld: Option<String>,
) -> Result<u8, RpcError> {
    let server = devlanes_resolver_dns::default_listen();
    let record = record(paths);
    let path = record.clone();
    let change = blocking(move || {
        // A TLD the OS cannot route has no file to remove.
        let configured = tld.and_then(|tld| resolver_file(&tld, server).ok());
        removal_change(configured, recorded(&path), occupant)
    })
    .await??;
    if change.remove.is_empty() {
        blocking(move || std::fs::remove_file(record)).await.ok();
        if !runtime.quiet {
            emit(&Rendered {
                stdout: "no devlanes resolver file to remove\n".into(),
                ..Rendered::default()
            });
        }
        return Ok(0);
    }
    apply(runtime, &change, super::REMOVE).await?;
    let stdout = removed_lines(&change);
    let saved = blocking(move || match std::fs::remove_file(&record) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    })
    .await;
    finish(runtime, stdout, saved);
    Ok(0)
}

/// The files [`remove`] deletes, read-only.
pub(super) async fn removal(paths: &Paths, tld: Option<String>) -> Result<Vec<PathBuf>, RpcError> {
    let server = devlanes_resolver_dns::default_listen();
    let record = record(paths);
    let change = blocking(move || {
        let configured = tld.and_then(|tld| resolver_file(&tld, server).ok());
        removal_change(configured, recorded(&record), occupant)
    })
    .await??;
    Ok(change.remove.into_iter().map(|file| file.path).collect())
}

/// Runs `change` with sudo, then tells the OS to reload. `command` is what to
/// run from a terminal when this session cannot use sudo.
async fn apply(runtime: &Runtime, change: &Change, command: &str) -> Result<(), RpcError> {
    let executor = super::sudo::privileged(runtime, command).await?;
    let sudo = system_program("sudo")?;
    let step = |program: &str, args: &[&str]| -> Result<CommandSpec, RpcError> {
        Ok(CommandSpec::new(sudo.as_str())
            .arg(system_program(program)?)
            .args(args.iter().map(|arg| (*arg).to_owned())))
    };
    let mut steps = Vec::new();
    let mut activations: Vec<Vec<String>> = Vec::new();
    for old in &change.remove {
        steps.push(step("rm", &["-f", &old.path.display().to_string()])?);
        activations.extend(old.activate.clone());
    }
    if let Some(file) = &change.write {
        let path = file.path.display().to_string();
        if let Some(parent) = file.path.parent() {
            steps.push(step("mkdir", &["-p", &parent.display().to_string()])?);
        }
        steps.push(step("tee", &[&path])?.stdin(file.contents.clone().into_bytes()));
        steps.push(step("chmod", &["644", &path])?);
        activations.extend(file.activate.clone());
    }
    activations.dedup();
    for activate in activations {
        let Some((program, args)) = activate.split_first() else {
            continue;
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        steps.push(step(program, &args)?);
    }
    for step in steps {
        let failure = match executor.run(&step).await {
            Ok(output) if output.success() => continue,
            Ok(output) => output.stderr.trim().to_owned(),
            Err(error) => error.to_string(),
        };
        return Err(RpcError::new(
            "resolver_install_failed",
            json!({"message": format!("`{step}` failed: {failure}"), "command": step.to_string(), "stderr": failure}),
            1,
        ));
    }
    Ok(())
}

/// `removed <path>` lines for the files `change` removed.
fn removed_lines(change: &Change) -> String {
    change
        .remove
        .iter()
        .map(|file| format!("removed {}\n", file.path.display()))
        .collect()
}

/// Prints the step's lines, warning when the record could not be updated.
fn finish(runtime: &Runtime, mut stdout: String, saved: Result<std::io::Result<()>, RpcError>) {
    let mut stderr = String::new();
    if let Ok(Err(error)) = saved {
        stderr.push_str(&format!(
            "warning: could not record the installed resolver file: {error}\n"
        ));
    }
    if runtime.quiet {
        stdout.clear();
    }
    emit(&Rendered {
        stdout,
        stderr,
        ..Rendered::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// The systemd version picks the resolved file form.
    fn systemd_version_from_systemctl() {
        assert_eq!(
            systemd_version("systemd 255 (255.4-1ubuntu8)\n+PAM +AUDIT"),
            Some(255)
        );
        assert_eq!(systemd_version("systemd 258\n"), Some(258));
        assert_eq!(systemd_version("elogind 252\n"), None);
        assert_eq!(systemd_version(""), None);
    }

    /// A resolver file at `path` (contents are irrelevant to planning).
    fn file(path: &str) -> ResolverFile {
        ResolverFile {
            path: PathBuf::from(path),
            contents: String::new(),
            activate: None,
        }
    }

    /// Occupants by path for planning tests.
    fn occupants(
        entries: &'static [(&'static str, Occupant)],
    ) -> impl Fn(&Path) -> Result<Occupant, String> {
        move |path| {
            Ok(entries
                .iter()
                .find(|(p, _)| Path::new(p) == path)
                .map_or(Occupant::Absent, |(_, o)| *o))
        }
    }

    #[test]
    /// Valet's or puma-dev's `/etc/resolver/test` is neither overwritten nor removed.
    fn foreign_resolver_files_are_refused() {
        let foreign = occupants(&[("/etc/resolver/test", Occupant::Foreign)]);
        let error = install_change(file("/etc/resolver/test"), None, &foreign)
            .expect_err("install refused");
        assert_eq!(error.code, "resolver_file_foreign");
        let error = removal_change(Some(file("/etc/resolver/test")), None, &foreign)
            .expect_err("removal refused");
        assert_eq!(error.code, "resolver_file_foreign");
    }

    #[test]
    /// Install writes the configured file and drops the recorded one after a TLD
    /// change; removal takes the recorded file, not the configured one.
    fn recorded_file_follows_tld_changes() {
        let both = occupants(&[
            ("/etc/resolver/test", Occupant::Devlanes),
            ("/etc/resolver/dev.internal", Occupant::Devlanes),
        ]);
        let change = install_change(
            file("/etc/resolver/dev.internal"),
            Some(file("/etc/resolver/test")),
            &both,
        )
        .expect("change");
        assert_eq!(change.remove, [file("/etc/resolver/test")]);
        assert_eq!(change.write, Some(file("/etc/resolver/dev.internal")));

        let change = removal_change(
            Some(file("/etc/resolver/dev.internal")),
            Some(file("/etc/resolver/test")),
            &both,
        )
        .expect("change");
        assert_eq!(change.remove, [file("/etc/resolver/test")]);
        assert_eq!(change.write, None);

        // A recorded file someone replaced since is left alone on install.
        let replaced = occupants(&[("/etc/resolver/test", Occupant::Foreign)]);
        let change = install_change(
            file("/etc/resolver/dev.internal"),
            Some(file("/etc/resolver/test")),
            &replaced,
        )
        .expect("change");
        assert!(change.remove.is_empty());

        // Nothing configured or recorded, or a file already gone: nothing to remove.
        for recorded in [None, Some(file("/etc/resolver/test"))] {
            let change = removal_change(None, recorded, occupants(&[])).expect("nothing");
            assert_eq!(
                change,
                Change {
                    remove: Vec::new(),
                    write: None
                }
            );
        }
    }

    #[test]
    /// Only marker-first files count as devlanes'.
    fn occupant_reads_the_marker() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ours = dir.path().join("ours");
        std::fs::write(&ours, "# Managed by devlanes: x\nnameserver 127.0.0.1\n").expect("write");
        let valet = dir.path().join("valet");
        std::fs::write(&valet, "nameserver 127.0.0.1\n").expect("write");
        assert_eq!(occupant(&ours), Ok(Occupant::Devlanes));
        assert_eq!(occupant(&valet), Ok(Occupant::Foreign));
        assert_eq!(occupant(&dir.path().join("none")), Ok(Occupant::Absent));
    }

    #[cfg(target_os = "macos")]
    #[test]
    /// macOS gets `/etc/resolver/<tld>` pointing at the daemon's server.
    fn macos_resolver_file() {
        let file = resolver_file("test", devlanes_resolver_dns::default_listen()).expect("file");
        assert_eq!(file.path, std::path::Path::new("/etc/resolver/test"));
        assert!(file.contents.contains("port 15353"));
    }
}
