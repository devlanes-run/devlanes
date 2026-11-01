//! `devlanes update` against a fake release of this same binary, served from a
//! directory through the debug-only `DEVLANES_UPDATE_BASE_URL`. A copy of the
//! binary in its own directory stands in for an `install.sh` install. The
//! release has the running version, so the test pins it with `--version`
//! (a pinned version is installed even when it is the one running).

use std::{
    env::consts::EXE_SUFFIX,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use devlanes_e2e::{Error, Lab, Result, alive, bin_dir, lane, pid, start_time, unit};
use devlanes_update::{
    check::BASE_URL_ENV,
    install::{TARGET, sha256_file},
};
use serde_json::json;

use crate::{BUDGET, need, units_in};

/// One service, which must come back after the update's restart.
const CONFIG: &str = r#"project: fixture-update
services:
  api:
    run: [fixture-server, --log-line, api-ready]
    pty: false
    ready: { http: { path: /healthz } }
"#;

/// Bound for the whole update: download, install and restart.
const UPDATE_TIMEOUT: Duration = Duration::from_secs(120);

#[test]
fn update_installs_the_release_and_restarts_the_daemon_with_its_lanes() -> Result<()> {
    // Dropped after the lab, whose daemon runs from the install in it.
    let root = tempfile::tempdir()?;
    let lab = Lab::with_config(CONFIG, BUDGET)?;
    let install = root.path().join("bin");
    std::fs::create_dir_all(&install)?;
    let entry = install.join(format!("devlanes{EXE_SUFFIX}"));
    let built = bin_dir()?.join(format!("devlanes{EXE_SUFFIX}"));
    std::fs::copy(&built, &entry)?;
    #[cfg(unix)]
    std::fs::write(install.join("devlanes-hosts"), "old helper")?;
    let version = reported_version(&entry)?;
    let site = root.path().join("site");
    publish(&site, &built, &version)?;

    // The daemon starts from the installed copy, as it would after install.sh.
    let started = lab.run(
        lab.command_with(&entry, lab.repo(), &["start"])?,
        devlanes_e2e::COMMAND_TIMEOUT,
    )?;
    assert_eq!(started.expect_code(0)?.json()?["ready"], true, "{started}");
    let api = api_pid(&lab)?;
    let api_started = need(start_time(api), "api start time")?;
    let daemon = need(lab.daemon_pid()?, "daemon pid")?;
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(&entry)?.ino()
    };
    #[cfg(windows)]
    let identity = devlanes_sys_windows::file::path_identity(&entry)?;

    let mut update = lab.command_with(
        &entry,
        lab.repo(),
        &["update", "--yes", "--version", &version],
    )?;
    update.env(BASE_URL_ENV, file_url(&site));
    let output = lab.run(update, UPDATE_TIMEOUT)?;
    let restarted = output.expect_code(0)?.json()?;
    assert_eq!(
        restarted["resuming"],
        json!(["fixture-update@0"]),
        "{output}"
    );

    // The binaries were replaced, by renames that leave the old file to the
    // processes still running it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_ne!(
            std::fs::metadata(&entry)?.ino(),
            inode,
            "devlanes was replaced"
        );
        assert_eq!(
            std::fs::read(install.join("devlanes-hosts"))?,
            b"new helper"
        );
    }
    // The running devlanes.exe was renamed out of the way, and deleted at once
    // since the processes running it keep a link to it in the home.
    #[cfg(windows)]
    {
        assert_ne!(
            devlanes_sys_windows::file::path_identity(&entry)?,
            identity,
            "devlanes.exe was replaced"
        );
        let old: Vec<_> = std::fs::read_dir(&install)?
            .flatten()
            .map(|entry| entry.file_name())
            .filter(|name| name.to_string_lossy().starts_with("devlanes.exe.old-"))
            .collect();
        assert!(old.is_empty(), "{old:?}");
    }
    assert!(
        !lab.home().join("update/staged.json").exists(),
        "the staged release was cleared"
    );

    // A new daemon runs from the install's entry path, and the lane is back.
    let new_daemon = need(lab.daemon_pid()?, "new daemon pid")?;
    assert_ne!(new_daemon, daemon, "the daemon restarted");
    let exe = need(executable(new_daemon), "the new daemon's executable")?;
    assert_eq!(
        dunce_like(&exe),
        dunce_like(&entry),
        "the new daemon runs {}",
        exe.display()
    );
    assert!(
        !alive(api) || start_time(api).as_deref() != Some(api_started.as_str()),
        "the old run (pid {api}) stopped"
    );
    lab.eventually("api to be ready again", Duration::from_secs(60), || {
        let lanes = lab.lanes(lab.repo())?;
        let ready =
            lane(&lanes, "fixture-update@0").is_some_and(|lane| units_in(lane, "ready") == ["api"]);
        Ok(ready.then_some(()))
    })?;
    Ok(())
}

/// `update --next` and `update --stable` write `updates.channel` with the
/// config editor, keeping comments, and install nothing when the channel has
/// no release newer than this build. Neither needs a daemon then.
#[test]
fn update_channel_switches_record_the_channel() -> Result<()> {
    let root = tempfile::tempdir()?;
    let lab = Lab::with_config(CONFIG, BUDGET)?;
    let install = root.path().join("bin");
    std::fs::create_dir_all(&install)?;
    let entry = install.join(format!("devlanes{EXE_SUFFIX}"));
    std::fs::copy(bin_dir()?.join(format!("devlanes{EXE_SUFFIX}")), &entry)?;
    let version = reported_version(&entry)?;
    // `next` has this very build; the latest stable release is older than it.
    let site = root.path().join("site");
    for (folder, latest) in [
        ("releases/download/next", version.as_str()),
        ("releases/latest/download", "0.0.1"),
    ] {
        std::fs::create_dir_all(site.join(folder))?;
        std::fs::write(
            site.join(folder).join("release-manifest.json"),
            json!({"version": latest}).to_string(),
        )?;
    }
    let config = lab.home().join("config.yaml");
    std::fs::write(&config, "# mine\nupdates:\n  mode: notify\n")?;
    let switch = |flag: &str| -> Result<serde_json::Value> {
        let mut command = lab.command_with(&entry, lab.repo(), &["update", flag, "--yes"])?;
        command.env(BASE_URL_ENV, file_url(&site));
        let output = lab.run(command, devlanes_e2e::COMMAND_TIMEOUT)?;
        output.expect_code(0)?.json()
    };

    let next = switch("--next")?;
    assert_eq!(next["updated"], false, "{next}");
    assert_eq!(next["channel"], "next", "{next}");
    assert_eq!(next["latest"], version.as_str(), "{next}");
    assert_eq!(next["newer_than_latest"], false, "{next}");
    let text = std::fs::read_to_string(&config)?;
    assert!(
        text.contains("# mine") && text.contains("channel: next"),
        "{text}"
    );

    let stable = switch("--stable")?;
    assert_eq!(stable["channel"], "stable", "{stable}");
    assert_eq!(stable["latest"], "0.0.1", "{stable}");
    assert_eq!(stable["newer_than_latest"], true, "{stable}");
    assert!(std::fs::read_to_string(&config)?.contains("channel: stable"));
    assert_eq!(reported_version(&entry)?, version, "the build stays");
    assert!(lab.daemon_pid()?.is_none(), "no daemon was started");
    Ok(())
}

/// `updates.mode: disable` and CI markers turn off the daemon's background
/// checks, not an explicit `devlanes update --check`, which then reads the
/// registry itself.
#[test]
fn update_check_reads_the_registry_when_the_daemons_checks_are_off() -> Result<()> {
    let root = tempfile::tempdir()?;
    let lab = Lab::with_config(CONFIG, BUDGET)?;
    let install = root.path().join("bin");
    std::fs::create_dir_all(&install)?;
    let entry = install.join(format!("devlanes{EXE_SUFFIX}"));
    std::fs::copy(bin_dir()?.join(format!("devlanes{EXE_SUFFIX}")), &entry)?;
    let site = root.path().join("site");
    let latest = site.join("releases/latest/download");
    std::fs::create_dir_all(&latest)?;
    std::fs::write(
        latest.join("release-manifest.json"),
        json!({"version": "99.0.0"}).to_string(),
    )?;
    let config = lab.home().join("config.yaml");
    // The daemon reads the fake releases too, so only the config or CI turns
    // its checks off, not the debug build.
    let run = |args: &[&str], env: &[(&str, &str)]| -> Result<serde_json::Value> {
        let mut command = lab.command_with(&entry, lab.repo(), args)?;
        command.env(BASE_URL_ENV, file_url(&site));
        for (key, value) in env {
            command.env(key, value);
        }
        lab.run(command, devlanes_e2e::COMMAND_TIMEOUT)?
            .expect_code(0)?
            .json()
    };
    for (mode, env, reason) in [
        ("disable", &[][..], "config"),
        ("notify", &[("CI", "true")][..], "ci"),
    ] {
        std::fs::write(&config, format!("updates:\n  mode: {mode}\n"))?;
        run(&["daemon", "start"], env)?;
        let status = run(&["daemon", "status"], &[])?;
        assert_eq!(status["release_check"]["reason"], reason, "{status}");
        let check = run(&["update", "--check"], &[])?;
        assert_eq!(check["latest"], "99.0.0", "{mode}: {check}");
        assert_eq!(check["state"], "available", "{mode}: {check}");
        assert_eq!(check["channel"], "stable", "{mode}: {check}");
        run(&["daemon", "stop", "--yes"], &[])?;
    }
    Ok(())
}

/// The PID of the lane's `api` service.
fn api_pid(lab: &Lab) -> Result<i32> {
    let lanes = lab.lanes(lab.repo())?;
    let api = need(
        lane(&lanes, "fixture-update@0").and_then(|lane| unit(lane, "api")),
        "api",
    )?;
    need(pid(api), "api pid")
}

/// The version `<exe> --version` prints.
fn reported_version(exe: &Path) -> Result<String> {
    let output = Command::new(exe).arg("--version").output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim()
        .strip_prefix("devlanes ")
        .map(str::to_owned)
        .ok_or_else(|| Error::Unexpected(format!("unexpected --version output: {text}")))
}

/// Publishes `binary` as release `version` for this target under
/// `site/releases/download/v<version>/`, with its `SHA256SUMS`.
fn publish(site: &Path, binary: &Path, version: &str) -> Result<()> {
    let top = format!("devlanes-{version}-{TARGET}");
    let work = site.join("work");
    let package = work.join(&top);
    std::fs::create_dir_all(&package)?;
    std::fs::write(package.join("LICENSE"), "Apache-2.0")?;
    std::fs::copy(binary, package.join(format!("devlanes{EXE_SUFFIX}")))?;
    #[cfg(unix)]
    std::fs::write(package.join("devlanes-hosts"), "new helper")?;
    let folder = site.join("releases/download").join(format!("v{version}"));
    std::fs::create_dir_all(&folder)?;
    #[cfg(unix)]
    let (suffix, tar, flags) = (".tar.gz", PathBuf::from("tar"), &["-czf"][..]);
    #[cfg(windows)]
    let (suffix, tar, flags) = (
        ".zip",
        devlanes_e2e::system32("tar.exe"),
        &["-a", "-cf"][..],
    );
    let name = format!("{top}{suffix}");
    let archive = folder.join(&name);
    let status = Command::new(tar)
        .args(flags)
        .arg(&archive)
        .arg("-C")
        .arg(&work)
        .arg(&top)
        .status()?;
    if !status.success() {
        return Err(Error::Setup(format!("tar could not pack {name}")));
    }
    let sha = sha256_file(&archive).map_err(|error| Error::Setup(error.to_string()))?;
    std::fs::write(folder.join("SHA256SUMS"), format!("{sha}  {name}\n"))?;
    Ok(())
}

/// `file:///...` for a local directory.
fn file_url(path: &Path) -> String {
    let path = path.display().to_string().replace('\\', "/");
    format!("file:///{}", path.trim_start_matches('/'))
}

/// A path compared without case or `\\?\` prefix on Windows, canonical on Unix.
fn dunce_like(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = path.display().to_string();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.to_owned()
    }
}

/// The executable process `pid` runs.
#[cfg(target_os = "linux")]
fn executable(pid: i32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// The executable process `pid` runs.
#[cfg(target_os = "macos")]
fn executable(pid: i32) -> Option<PathBuf> {
    let output = Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

/// The executable process `pid` runs.
#[cfg(windows)]
fn executable(pid: i32) -> Option<PathBuf> {
    let output = Command::new(devlanes_e2e::system32(
        r"WindowsPowerShell\v1.0\powershell.exe",
    ))
    .args([
        "-NoProfile",
        "-Command",
        &format!("(Get-Process -Id {pid}).Path"),
    ])
    .output()
    .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then(|| PathBuf::from(text))
}
