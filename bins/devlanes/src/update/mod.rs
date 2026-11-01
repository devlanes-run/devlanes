//! `devlanes update`: checks for a newer release, asks, installs it the way
//! devlanes was installed (`devlanes_update::install`), then runs the new
//! binary with `update --finish`, which restarts the daemon into it in the
//! same step block and says what changed, from the notes the new binary
//! carries (`crate::notes`). Also the interactive startup rule:
//! a TUI that opens over an older daemon, or over a release the daemon staged,
//! switches to the new version first.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use crossterm::{
    cursor::MoveToPreviousLine,
    terminal::{Clear, ClearType},
};
use devlanes_cli::{ChannelSwitch, OutputMode, Rendered, Runtime, UpdateAction};
use devlanes_client::{Client, Paths, restart::RowState};
use devlanes_config::UpdateChannel;
use devlanes_proto::{
    Context, LaneSummary, Method, Page, RpcError,
    release::{UpdateInfo, UpdateNote},
};
use devlanes_tui::{REOPEN_ENV, Reopen};
use devlanes_update::{
    InstallMethod,
    check::{Channel, Source, Urls},
    install::{self, InstallError, Lock, Plan, Release, Staged, Target},
    notes::{Level, WhatsNew},
    version::{Bump, Version, bump, is_newer},
};
use serde_json::{Value, json};

use crate::{
    driver::rpc_error,
    lifecycle::{self, Handoff, Kind, Outcome, STEPS_ENV, View},
    output::emit,
};

/// This binary's version.
const OWN: &str = devlanes_version::VERSION;

/// How long `devlanes update` waits for an install another process runs
/// (the daemon's background download, another `devlanes update`).
const LOCK_WAIT: Duration = Duration::from_secs(15 * 60);

/// Limit for reading `SHA256SUMS`.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// How [`install`] ended without an error.
enum Installed {
    /// The new binary ran `update --finish` (on Windows; on Unix it took this
    /// process's place), which exited with this status.
    Finished(u8),
    /// Nothing was installed: no newer release, or the question was declined.
    Nothing(u8),
}

/// `devlanes update` and its `--check`, `--notes`, `--next`, `--stable` and
/// `--finish` forms.
pub async fn run(runtime: &Runtime, paths: &Paths, action: UpdateAction) -> Result<u8, RpcError> {
    match action {
        UpdateAction::Check => check(runtime, paths).await,
        UpdateAction::Notes => Ok(notes(runtime)),
        UpdateAction::Install { version, yes } => {
            match install(runtime, paths, version, yes, None).await? {
                Installed::Finished(code) | Installed::Nothing(code) => Ok(code),
            }
        }
        UpdateAction::Switch { channel, yes } => switch(runtime, paths, channel, yes).await,
        UpdateAction::Finish => finish(runtime, paths).await,
    }
}

/// The update the TUI's `U` asked for, already confirmed there: runs
/// `devlanes update` and opens the TUI again on `reopen` from the new binary.
/// When nothing is installed (no newer release, a manual install, a failure),
/// says why and opens the TUI again from the binary installed now.
pub async fn from_tui(runtime: &Runtime, paths: &Paths, reopen: Reopen) -> Result<u8, RpcError> {
    match install(runtime, paths, None, true, Some(&reopen)).await {
        Ok(Installed::Finished(code)) => return Ok(code),
        Ok(Installed::Nothing(_)) => {}
        Err(error) => emit(&runtime.reporter().error(&error, None)),
    }
    tokio::task::spawn_blocking(move || crate::relaunch::exec_tui(&reopen))
        .await
        .map_err(|error| io_error(&error))?
}

/// The interactive startup rule, before the TUI opens on a terminal in human
/// mode. A patch release the daemon staged for this install is put in place,
/// and this command runs again from it. A daemon older than this binary is
/// restarted into it: at once for a patch release, after asking for a minor
/// one. A project's own devlanes never restarts the shared daemon and warns
/// instead. Agent, JSON, CI and plain output never restart. Nothing here
/// keeps the TUI from opening: a daemon that cannot be restarted (an invalid
/// config, a failed connection or restart) gets a warning on stderr.
pub async fn startup(runtime: &Runtime, paths: &Paths) {
    let interactive = runtime.mode == OutputMode::Human
        && runtime.stdout_tty
        && runtime.stdin_tty
        && runtime.ci.is_none();
    if !interactive {
        return;
    }
    if let Err(error) = switch_on_start(runtime, paths).await {
        let detail = error
            .data
            .get("message")
            .or_else(|| error.data.get("title"))
            .and_then(Value::as_str)
            .unwrap_or(&error.code);
        emit(&Rendered {
            stderr: format!(
                "warning: the daemon was not switched to devlanes {OWN}: {detail}; devlanes daemon restart tries again\n"
            ),
            ..Rendered::default()
        });
    }
}

/// The work of [`startup`], whose failures it reports as warnings.
async fn switch_on_start(runtime: &Runtime, paths: &Paths) -> Result<(), RpcError> {
    let (method, exe) = current().await?;
    if let Some(staged) = apply_staged(paths, &method).await {
        tracing::info!(version = staged, "applied the staged update");
        let entry = method.entry(&exe);
        let args: Vec<OsString> = std::env::args_os().skip(1).collect();
        let code = exec_entry(&entry, &args, &[]).await?;
        // Windows: the new binary ran this command to its end.
        std::process::exit(i32::from(code));
    }
    let mut client = match Client::connect_any(paths).await {
        Ok(client) => client,
        Err(error) if error.is_not_running() => return Ok(()),
        Err(error) => return Err(rpc_error(error)),
    };
    let daemon = client.peer.version.clone();
    let Some(bump) = bump(&daemon, OWN) else {
        return Ok(());
    };
    if let InstallMethod::Local { manifest } = &method {
        emit(&Rendered {
            stderr: format!(
                "the daemon runs devlanes {daemon} and {} pins {OWN}; devlanes daemon restart switches the daemon (it stops services)\n",
                manifest.display()
            ),
            ..Rendered::default()
        });
        return Ok(());
    }
    if bump == Bump::Minor {
        let shown = WhatsNew::since(&crate::notes::embedded(), &daemon, OWN);
        print_minor_notes(shown.releases.iter().flat_map(|release| {
            release
                .entries
                .iter()
                .filter(|entry| entry.level == Level::Minor)
                .map(|entry| (release.version.as_str(), entry.line.as_str()))
        }));
        let question = format!(
            "Restart the daemon from devlanes {daemon} into {OWN}? It stops and starts {} again.",
            lanes_word(lanes_running(&mut client).await)
        );
        if !ask(runtime, &question, false).await? {
            return Ok(());
        }
    }
    drop(client);
    crate::user_config::load(paths).await?;
    let mut view = View::new(
        runtime,
        paths,
        Kind::Update {
            from: daemon,
            to: OWN.to_owned(),
        },
    );
    take_over(&mut view);
    lifecycle::restart_in(runtime, paths, restart_context(runtime), view)
        .await
        .map(drop)
}

/// `devlanes update --check`: checks now and shows the install, the running
/// and latest versions and what changed.
async fn check(runtime: &Runtime, paths: &Paths) -> Result<u8, RpcError> {
    let (method, _) = current().await?;
    let mut client = compatible_daemon(paths).await?;
    let daemon = client.as_ref().map(|client| client.peer.version.clone());
    let from = daemon.clone().unwrap_or_else(|| OWN.to_owned());
    let info = latest(client.as_mut(), &method, paths, &from, true).await?;
    let channel = match &info {
        Some(info) => info.channel.clone(),
        None => effective_channel(&crate::user_config::lenient(paths).await)
            .as_str()
            .to_owned(),
    };
    let command = info
        .as_ref()
        .map(|info| devlanes_update::update_command(&method, &info.latest));
    let data = json!({
        "method": method.describe(devlanes_core::paths::user_home().as_deref()),
        "channel": channel,
        "running": OWN,
        "daemon": daemon,
        "latest": info.as_ref().map(|info| &info.latest),
        "bump": info.as_ref().map(|info| &info.bump),
        "state": info.as_ref().map(|info| &info.state),
        "installed": info.as_ref().and_then(|info| info.installed.as_ref()),
        "error": info.as_ref().and_then(|info| info.error.as_ref()),
        "notes": info.as_ref().map(|info| &info.notes),
        "command": command,
    });
    if matches!(runtime.mode, OutputMode::Json) {
        emit(&Rendered {
            stdout: format!("{data}\n"),
            ..Rendered::default()
        });
        return Ok(0);
    }
    if runtime.quiet {
        return Ok(0);
    }
    let daemon = match &daemon {
        Some(version) => format!("daemon {version}"),
        None => "no daemon of this protocol runs".to_owned(),
    };
    let mut out = format!(
        "devlanes {OWN} ({}), {daemon}, channel {channel}\n",
        data["method"].as_str().unwrap_or_default()
    );
    match &info {
        None => out.push_str("no newer release\n"),
        Some(info) => {
            out.push_str(&format!("latest: {} ({} release)", info.latest, info.bump));
            match info.state.as_str() {
                "installing" => out.push_str(", installing in the background"),
                "ready" => out.push_str(", ready: run devlanes to switch to it"),
                "failed" => out.push_str(&format!(
                    ", the background install failed: {}",
                    info.error.as_deref().unwrap_or("unknown error")
                )),
                _ => {}
            }
            out.push('\n');
            for note in sorted_notes(&info.notes) {
                out.push_str(&format!("  {}  {}\n", note.version, note.line));
            }
            if let Some(command) = command {
                out.push_str(&format!("next: {command}\n"));
            }
        }
    }
    emit(&Rendered {
        stdout: out,
        ..Rendered::default()
    });
    Ok(0)
}

/// `devlanes update`: decides what to install, then [`deliver`]s it. The
/// latest release is skipped when the install already holds it (the daemon
/// or a package manager put it there), so only the restart is left; a
/// `version` asked for is always installed unless it is older than this
/// build.
async fn install(
    runtime: &Runtime,
    paths: &Paths,
    version: Option<String>,
    yes: bool,
    reopen: Option<&Reopen>,
) -> Result<Installed, RpcError> {
    let version = version.as_deref().map(release_version).transpose()?;
    let (method, exe) = current().await?;
    if matches!(method, InstallMethod::Local { .. } | InstallMethod::Source) {
        return Err(manual(
            &method,
            &method.upgrade_command(version.as_deref().unwrap_or(OWN)),
        ));
    }
    let mut client = compatible_daemon(paths).await?;
    let from = client
        .as_ref()
        .map_or_else(|| OWN.to_owned(), |client| client.peer.version.clone());
    let info = match &version {
        None => latest(client.as_mut(), &method, paths, &from, false).await?,
        Some(_) => None,
    };
    let pinned = version.is_some();
    let to = match (version, &info) {
        (Some(version), _) => match install::target(OWN, &version, false) {
            Target::Older => {
                return Err(RpcError::new(
                    "update_older",
                    json!({"version": version, "running": OWN}),
                    1,
                ));
            }
            Target::Current | Target::Install { .. } => version,
        },
        (None, Some(info)) if is_newer(&info.latest, OWN) || is_newer(&info.latest, &from) => {
            info.latest.clone()
        }
        (None, _) => return up_to_date(runtime, &from).map(Installed::Nothing),
    };
    let channel = info.as_ref().map_or_else(
        || Channel::of_version(&to),
        |info| channel_of(&info.channel),
    );
    let delivery = Delivery {
        method,
        exe,
        client,
        from,
        to,
        reinstall: pinned,
        older: false,
        channel,
        switched: false,
        info,
    };
    deliver(runtime, paths, delivery, yes, reopen).await
}

/// `devlanes update --next` and `--stable [--force]`: records the channel in
/// `updates.channel`, then installs its latest release when that is newer
/// than this build, or older with `--force`. Otherwise it says this build
/// stays. `--next` refuses before changing the config when the install
/// cannot take release archives.
async fn switch(
    runtime: &Runtime,
    paths: &Paths,
    choice: ChannelSwitch,
    yes: bool,
) -> Result<u8, RpcError> {
    let (channel, force) = match choice {
        ChannelSwitch::Next => (Channel::Next, false),
        ChannelSwitch::Stable { force } => (Channel::Stable, force),
    };
    let (method, exe) = current().await?;
    if channel == Channel::Next {
        let (planned, path) = (method.clone(), exe.clone());
        // A script install in a directory it cannot write to is a manual one.
        let archive = tokio::task::spawn_blocking(move || {
            let retained = devlanes_update::own_exe_retained();
            matches!(
                Plan::new(&planned, &path, OWN, retained),
                Ok(Plan::Archive { .. })
            )
        })
        .await
        .map_err(|error| io_error(&error))?;
        if !install::follows(&method, channel) || !archive {
            return Err(next_unsupported(&method, &exe).await);
        }
    }
    let source = Source::for_method(&method, channel)
        .map_err(|_| manual(&method, &method.upgrade_command(OWN)))?;
    let before = effective_channel(&crate::user_config::lenient(paths).await);
    let file = crate::user_config::write(paths, "updates.channel", json!(channel.as_str())).await?;
    let latest = devlanes_update::check::latest(source, &Urls::from_env(), FETCH_TIMEOUT)
        .await
        .map_err(|error| {
            check_failed(
                channel,
                &error,
                &format!("devlanes update --{}", channel.as_str()),
            )
        })?;
    let (to, older) = match install::target(OWN, &latest, force) {
        Target::Install { version, older } => (version, older),
        decided => {
            return Ok(stays(
                runtime,
                &Stay {
                    channel,
                    changed: before != channel,
                    file: &file,
                    latest: &latest,
                    older: decided == Target::Older,
                },
            ));
        }
    };
    if older {
        emit(&Rendered {
            stderr: format!(
                "warning: devlanes {to} is older than this build ({OWN}). If its daemon refuses the state database the newer build migrated, devlanes daemon reset-state empties it; lanes then get new numbers and ports.\n"
            ),
            ..Rendered::default()
        });
    }
    let client = compatible_daemon(paths).await?;
    let from = client
        .as_ref()
        .map_or_else(|| OWN.to_owned(), |client| client.peer.version.clone());
    let delivery = Delivery {
        method,
        exe,
        client,
        from,
        to,
        reinstall: false,
        older,
        channel,
        switched: true,
        info: None,
    };
    match deliver(runtime, paths, delivery, yes, None).await? {
        Installed::Finished(code) | Installed::Nothing(code) => Ok(code),
    }
}

/// What [`switch`] reports when nothing is installed.
struct Stay<'a> {
    channel: Channel,
    /// Whether the config followed another channel before.
    changed: bool,
    /// The user config file.
    file: &'a Path,
    /// The channel's latest release.
    latest: &'a str,
    /// Whether `latest` is older than this build, which then stays.
    older: bool,
}

/// Says that `updates.channel` now names the channel and why this build
/// stays: it is the channel's latest release, or newer than it.
fn stays(runtime: &Runtime, stay: &Stay<'_>) -> u8 {
    let channel = stay.channel.as_str();
    let rendered = match runtime.mode {
        OutputMode::Json => Rendered {
            stdout: format!(
                "{}\n",
                json!({
                    "updated": false,
                    "channel": channel,
                    "file": stay.file,
                    "running": OWN,
                    "latest": stay.latest,
                    "newer_than_latest": stay.older,
                })
            ),
            ..Rendered::default()
        },
        _ if runtime.quiet => Rendered::default(),
        _ => Rendered {
            stdout: stay_text(stay),
            ..Rendered::default()
        },
    };
    emit(&rendered);
    0
}

/// The human text of [`stays`].
fn stay_text(stay: &Stay<'_>) -> String {
    let (channel, latest) = (stay.channel.as_str(), stay.latest);
    let what = match stay.channel {
        Channel::Stable => "stable release",
        Channel::Next => "next build",
    };
    let mut out = format!("updates.channel is {channel} in {}", stay.file.display());
    if stay.changed {
        out.push_str("; the daemon follows it from its next restart");
    }
    out.push('\n');
    if !stay.older {
        out.push_str(&format!("devlanes {OWN} is the latest {what}\n"));
        return out;
    }
    out.push_str(&format!(
        "devlanes {OWN} stays until a {what} newer than it ships; the latest is {latest}\n"
    ));
    if stay.channel == Channel::Stable {
        out.push_str(&format!(
            "devlanes update --stable --force installs {latest} now\n"
        ));
    }
    out
}

/// The channel the user config `config` makes devlanes follow: its
/// `updates.channel`, else the channel of this build.
fn effective_channel(config: &devlanes_config::UserConfig) -> Channel {
    match config.updates.channel {
        Some(UpdateChannel::Stable) => Channel::Stable,
        Some(UpdateChannel::Next) => Channel::Next,
        None => Channel::of_version(OWN),
    }
}

/// `update_next_unsupported` for `method`: how to reinstall with the install
/// script in its default directory, after removing this install when a
/// package manager owns it.
async fn next_unsupported(method: &InstallMethod, exe: &Path) -> RpcError {
    let (method, exe) = (method.clone(), exe.to_path_buf());
    let found = tokio::task::spawn_blocking(move || {
        let remove = install::removal_commands(&method, &exe).map(|commands| {
            commands
                .iter()
                .map(install::Argv::display)
                .collect::<Vec<_>>()
                .join(" && ")
        });
        let home = devlanes_core::paths::user_home();
        (method.describe(home.as_deref()), remove)
    })
    .await;
    let (described, remove) = found.unwrap_or_else(|_| ("this install".into(), None));
    let dir = devlanes_update::default_script_dir().unwrap_or_else(|| {
        PathBuf::from(if cfg!(windows) {
            r"%LOCALAPPDATA%\Programs\devlanes"
        } else {
            "~/.local/bin"
        })
    });
    let mut data = json!({
        "method": described,
        "script": devlanes_update::SCRIPT_NAME,
        "install": devlanes_update::script_install_command(&dir),
    });
    if let Some(remove) = remove {
        data["remove"] = json!(remove);
    }
    RpcError::new("update_next_unsupported", data, 1)
}

/// The daemon when one runs and speaks this binary's protocol.
async fn compatible_daemon(paths: &Paths) -> Result<Option<Client>, RpcError> {
    match Client::connect_any(paths).await {
        Ok(client) if client.compatible() => Ok(Some(client)),
        Ok(_) => Ok(None),
        Err(error) if error.is_not_running() => Ok(None),
        Err(error) => Err(rpc_error(error)),
    }
}

/// A release [`deliver`] installs, and what it knows around it.
struct Delivery {
    method: InstallMethod,
    /// This binary's executable.
    exe: PathBuf,
    /// The daemon, when one runs on this protocol.
    client: Option<Client>,
    /// The daemon's version, else this binary's.
    from: String,
    to: String,
    /// Install `to` even when the install already holds it (`--version`).
    reinstall: bool,
    /// `to` is older than this build (`update --stable --force`).
    older: bool,
    /// The channel `to` comes from.
    channel: Channel,
    /// `update --next` or `--stable` set `updates.channel` to `channel`.
    switched: bool,
    /// The daemon's report of `to`, whose notes the question lists.
    info: Option<UpdateInfo>,
}

/// Asks unless `yes`, installs the release in the step block, then runs the
/// new binary's `update --finish`, which opens the TUI on `reopen` when given.
async fn deliver(
    runtime: &Runtime,
    paths: &Paths,
    delivery: Delivery,
    yes: bool,
    reopen: Option<&Reopen>,
) -> Result<Installed, RpcError> {
    let Delivery {
        method,
        exe,
        mut client,
        from,
        to,
        reinstall,
        older,
        channel,
        switched,
        info,
    } = delivery;
    let entry = method.entry(&exe);
    let installed = install::reported_version(&entry).await;
    let plan = if !reinstall && installed.as_deref() == Some(to.as_str()) {
        None
    } else {
        let (planned, exe, to) = (method.clone(), exe.clone(), to.clone());
        let plan = tokio::task::spawn_blocking(move || {
            Plan::new(&planned, &exe, &to, devlanes_update::own_exe_retained())
        })
        .await
        .map_err(|error| io_error(&error))?
        .map_err(|error| failed(&error))?;
        if let Plan::Manual { command } = &plan {
            return Err(manual(&method, command));
        }
        Some(plan)
    };
    if !yes {
        let lanes = match client.as_mut() {
            Some(client) => lanes_running(client).await,
            None => 0,
        };
        let notes = info
            .as_ref()
            .map(|info| info.notes.as_slice())
            .unwrap_or_default();
        if !confirm_update(runtime, &from, &to, lanes, notes, older).await? {
            return Ok(Installed::Nothing(devlanes_cli::exit::USAGE));
        }
    }
    drop(client);
    let mut view = View::new(
        runtime,
        paths,
        Kind::Update {
            from,
            to: to.clone(),
        },
    );
    if switched {
        view.set(
            "channel",
            format!("following {} (updates.channel)", channel_words(channel)),
            RowState::Done,
            None,
        );
    }
    if let Some(plan) = plan
        && let Err(error) = run_plan(&mut view, paths, &plan, &to, channel, &entry).await
    {
        view.fail(&error);
        return Err(error);
    }
    let mut envs: Vec<(&str, String)> = Vec::new();
    if let Some(handoff) = view.handoff() {
        envs.push((
            STEPS_ENV,
            serde_json::to_string(&handoff).unwrap_or_default(),
        ));
    }
    if let Some(reopen) = reopen {
        envs.push((
            REOPEN_ENV,
            serde_json::to_string(reopen).unwrap_or_default(),
        ));
    }
    exec_entry(&entry, &finish_args(runtime), &envs)
        .await
        .map(Installed::Finished)
}

/// Installs `to` by `plan` with a step row in `view`: stages the release
/// archive and puts it in place, or runs the package manager. Waits for an
/// install another process runs first.
async fn run_plan(
    view: &mut View,
    paths: &Paths,
    plan: &Plan,
    to: &str,
    channel: Channel,
    entry: &Path,
) -> Result<(), RpcError> {
    const ROW: &str = "install";
    let home = paths.home.clone();
    let lock = {
        let home = home.clone();
        tokio::task::spawn_blocking(move || Lock::acquire(&home, Duration::ZERO))
            .await
            .map_err(|error| io_error(&error))?
    };
    let _lock = match lock {
        Ok(lock) => lock,
        Err(InstallError::Busy) => {
            view.set(
                ROW,
                "waiting for another update to finish".into(),
                RowState::Running,
                None,
            );
            let home = home.clone();
            let lock = view
                .spin(tokio::task::spawn_blocking(move || {
                    Lock::acquire(&home, LOCK_WAIT)
                }))
                .await
                .map_err(|error| io_error(&error))?
                .map_err(|error| failed(&error))?;
            // The daemon's background install, or another `devlanes update`,
            // may have run the package manager for this release meanwhile.
            if matches!(plan, Plan::PackageManager(_))
                && install::reported_version(entry).await.as_deref() == Some(to)
            {
                view.set(
                    ROW,
                    format!("devlanes {to} is installed"),
                    RowState::Done,
                    None,
                );
                return Ok(());
            }
            lock
        }
        Err(error) => return Err(failed(&error)),
    };
    match plan {
        Plan::Archive { dir } => {
            view.set(
                ROW,
                format!("downloading devlanes {to}"),
                RowState::Running,
                None,
            );
            let release = match channel {
                Channel::Next if Channel::of_version(to) == Channel::Next => Release::Next,
                _ => Release::Version(to.to_owned()),
            };
            let urls = devlanes_update::check::Urls::from_env();
            let staged = view
                .spin(install::stage(&home, &urls, &release, dir, FETCH_TIMEOUT))
                .await
                .map_err(|error| failed(&error))?;
            let applied = tokio::task::spawn_blocking(move || install::apply(&home, &staged))
                .await
                .map_err(|error| io_error(&error))?
                .map_err(|error| failed(&error))?;
            tracing::debug!(in_use = ?applied.in_use, "installed the staged release");
            view.set(
                ROW,
                format!("installed devlanes {to} in {}", dir.display()),
                RowState::Done,
                None,
            );
        }
        Plan::PackageManager(commands) => {
            let shown = commands
                .iter()
                .map(install::Argv::display)
                .collect::<Vec<_>>()
                .join(", then ");
            view.set(ROW, format!("running {shown}"), RowState::Running, None);
            view.spin(install::run_package_manager(
                commands,
                &paths.logs.join("update.log"),
            ))
            .await
            .map_err(|error| failed(&error))?;
            // Homebrew's tap may name another version; the entry says what runs now.
            match install::reported_version(entry).await {
                Some(found) if found == to || is_newer(&found, OWN) => {}
                found => {
                    return Err(failed(&InstallError::Release(format!(
                        "{shown} finished, but {} still reports {}",
                        entry.display(),
                        found.as_deref().unwrap_or("no version")
                    ))));
                }
            }
            view.set(
                ROW,
                format!("installed devlanes {to} with {shown}"),
                RowState::Done,
                None,
            );
        }
        Plan::Manual { .. } => {}
    }
    Ok(())
}

/// `devlanes update --finish`, run by the binary just installed: restarts
/// the daemon into it in the block the old binary drew, and opens the TUI
/// again when the update came from it, which shows what changed. Otherwise
/// prints what changed since the daemon's version, from this binary's
/// changelog, and records it as seen so the next TUI does not repeat it.
async fn finish(runtime: &Runtime, paths: &Paths) -> Result<u8, RpcError> {
    let mut code = 0;
    let reopen = crate::relaunch::reopen();
    match Client::connect_any(paths).await {
        Ok(client) => {
            let from = client.peer.version.clone();
            drop(client);
            crate::user_config::load(paths).await?;
            let mut view = View::new(
                runtime,
                paths,
                Kind::Update {
                    from: from.clone(),
                    to: OWN.to_owned(),
                },
            );
            take_over(&mut view);
            code = lifecycle::restart_in(runtime, paths, restart_context(runtime), view).await?;
            if reopen.is_none() && runtime.mode == OutputMode::Human && !runtime.quiet {
                print_whats_new(paths, &from).await;
            }
        }
        Err(error) if error.is_not_running() => {
            // No daemon to restart; the block the old binary drew stays.
            if let Some(handoff) = handoff_from_env()
                && !matches!(runtime.mode, OutputMode::Json | OutputMode::Agent)
            {
                let mut view = View::new(
                    runtime,
                    paths,
                    Kind::Update {
                        from: OWN.to_owned(),
                        to: OWN.to_owned(),
                    },
                );
                view.take_over(handoff);
                view.finish(Outcome::Done);
            }
        }
        Err(error) => return Err(rpc_error(error)),
    }
    if let Some(reopen) = reopen {
        return tokio::task::spawn_blocking(move || crate::relaunch::exec_tui(&reopen))
            .await
            .map_err(|error| io_error(&error))?;
    }
    Ok(code)
}

/// Puts a patch release the daemon staged for this script install in place,
/// under the update lock. Returns its version; `None` when there is none,
/// another update holds the lock, or it fails (logged).
async fn apply_staged(paths: &Paths, method: &InstallMethod) -> Option<String> {
    let InstallMethod::Script(dir) = method else {
        return None;
    };
    let (home, dir) = (paths.home.clone(), dir.clone());
    tokio::task::spawn_blocking(move || {
        let staged = Staged::load(&home)?;
        let patch = bump(OWN, &staged.version) == Some(Bump::Patch);
        if !patch || !devlanes_core::paths::same_path(&staged.target, &dir) {
            return None;
        }
        let _lock = Lock::acquire(&home, Duration::ZERO).ok()?;
        match install::apply(&home, &staged) {
            Ok(_) => Some(staged.version),
            Err(error) => {
                tracing::warn!(%error, "the staged update was not applied");
                None
            }
        }
    })
    .await
    .ok()
    .flatten()
}

/// The install method of this binary and its executable. Reads the
/// filesystem off the async threads.
async fn current() -> Result<(InstallMethod, PathBuf), RpcError> {
    tokio::task::spawn_blocking(|| {
        let exe = devlanes_update::running_exe().unwrap_or_else(|_| PathBuf::from("devlanes"));
        (InstallMethod::current(), exe)
    })
    .await
    .map_err(|error| io_error(&error))
}

/// The release newer than `from` (the daemon's version, else this build's),
/// if any. A daemon of this protocol checks it, forced, so its status and
/// notes follow; it installs what it finds in the background only with
/// `install`, which `devlanes update` turns off because it installs the
/// release itself. Without an answer from one (none runs, its checks are off
/// by `updates.mode: disable`, CI or the environment, or it predates
/// `update.check`), this process reads the registry of `method` itself:
/// an explicit `devlanes update` always checks.
async fn latest(
    client: Option<&mut Client>,
    method: &InstallMethod,
    paths: &Paths,
    from: &str,
    install: bool,
) -> Result<Option<UpdateInfo>, RpcError> {
    if let Some(client) = client {
        let reply = client
            .request(
                Method::UpdateCheck,
                json!({"force": true, "install": install}),
                Context::default(),
            )
            .await
            .map_err(rpc_error);
        let off = matches!(reply, Ok(Value::Null)) && checks_off(client).await;
        if let Some(answer) = daemon_answer(reply, off)? {
            return Ok(answer);
        }
    }
    let channel = effective_channel(&crate::user_config::lenient(paths).await);
    // Project dependencies and source builds have no registry to read.
    let Ok(source) = Source::for_method(method, channel) else {
        return Ok(None);
    };
    let latest = devlanes_update::check::latest(source, &Urls::from_env(), FETCH_TIMEOUT)
        .await
        .map_err(|error| check_failed(source.channel(), &error, "devlanes update"))?;
    Ok(found(source.channel(), latest, from))
}

/// What the daemon's `reply` to a forced `update.check` says: `Some` with
/// the newer release or none, or `None` when it could not tell, because it
/// does not know the method (`unknown_method`) or its checks are `off`.
/// Other errors pass through, `update_failed` from a registry it could not
/// read among them, so the command fails as when it reads one itself.
fn daemon_answer(
    reply: Result<Value, RpcError>,
    off: bool,
) -> Result<Option<Option<UpdateInfo>>, RpcError> {
    match reply {
        Err(error) if error.code == "unknown_method" => Ok(None),
        Err(error) => Err(error),
        Ok(Value::Null) if off => Ok(None),
        Ok(answer) => Ok(Some(serde_json::from_value(answer).ok())),
    }
}

/// Whether the daemon's release checks are off (`daemon.status` says
/// `release_check.state: off`).
async fn checks_off(client: &mut Client) -> bool {
    client
        .request(Method::DaemonStatus, json!({}), Context::default())
        .await
        .is_ok_and(|status| status.pointer("/release_check/state") == Some(&json!("off")))
}

/// `latest` of `channel` as the report a daemon would give, when it is newer
/// than `from` or this build: without notes, which only the daemon reads.
fn found(channel: Channel, latest: String, from: &str) -> Option<UpdateInfo> {
    let bump = bump(from, &latest).or_else(|| bump(OWN, &latest))?;
    Some(UpdateInfo {
        latest,
        channel: channel.as_str().into(),
        bump: bump.as_str().into(),
        state: "available".into(),
        installed: None,
        error: None,
        checked_at: devlanes_update::now_ms(),
        notes: Vec::new(),
    })
}

/// `update_failed` for a registry of `channel` that could not be read, with
/// `retry` as the command to run again.
fn check_failed(channel: Channel, error: &dyn std::fmt::Display, retry: &str) -> RpcError {
    RpcError::new(
        "update_failed",
        json!({
            "title": format!("cannot read the latest {} release: {error}", channel.as_str()),
            "hint": format!("retry with {retry}"),
        }),
        1,
    )
}

/// `--version` as a release version: one leading `v` is dropped, then it
/// must be exactly `MAJOR.MINOR.PATCH[-PRERELEASE]`.
fn release_version(text: &str) -> Result<String, RpcError> {
    let bare = text.strip_prefix('v').unwrap_or(text);
    Version::parse_release(bare)
        .map(|_| bare.to_owned())
        .ok_or_else(|| failed(&InstallError::Version(text.to_owned())))
}

/// Prints what changed from `from` to this binary, from its changelog, and
/// records it as seen in the notice log, as the TUI does when it shows it.
async fn print_whats_new(paths: &Paths, from: &str) {
    let releases = crate::notes::embedded();
    let shown = WhatsNew::since(&releases, from, OWN);
    if !shown.is_empty() {
        emit(&Rendered {
            stdout: format!("\n{}", crate::notes::render(&shown)),
            ..Rendered::default()
        });
    }
    let home = paths.home.clone();
    let _ = tokio::task::spawn_blocking(move || {
        devlanes_update::claim_whats_new(&home, OWN, crate::notes::local_build(), &releases)
    })
    .await;
}

/// `devlanes update --notes`: the notes of the running version, or for a
/// `next` build of the release it builds on.
fn notes(runtime: &Runtime) -> u8 {
    let shown = WhatsNew::latest(&crate::notes::embedded(), OWN);
    let stdout = match runtime.mode {
        OutputMode::Json => {
            let releases: Vec<Value> = shown
                .releases
                .iter()
                .map(|release| {
                    json!({
                        "version": release.version,
                        "entries": release.entries.iter().map(|entry| json!({
                            "level": entry.level.as_str(),
                            "line": entry.line,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            format!("{}\n", json!({"running": OWN, "releases": releases}))
        }
        _ if runtime.quiet => String::new(),
        _ if shown.is_empty() => format!("devlanes {OWN} carries no release notes\n"),
        _ => crate::notes::render(&shown),
    };
    emit(&Rendered {
        stdout,
        ..Rendered::default()
    });
    0
}

/// How many lanes run services, which a restart starts again.
async fn lanes_running(client: &mut Client) -> usize {
    client
        .request(Method::Status, json!({"all": true}), Context::default())
        .await
        .ok()
        .and_then(|status| serde_json::from_value::<Page<LaneSummary>>(status).ok())
        .map_or(0, |page| {
            devlanes_client::restart::running_lanes(&page.items).len()
        })
}

/// Minor entries first, each group in the daemon's order (newest release first).
fn sorted_notes(notes: &[UpdateNote]) -> Vec<&UpdateNote> {
    let mut sorted: Vec<&UpdateNote> = notes.iter().collect();
    sorted.sort_by_key(|note| note.level != "minor");
    sorted
}

/// Prints minor (possibly breaking) entries, as their version and line, to
/// stderr above a question.
fn print_minor_notes<'a>(minor: impl IntoIterator<Item = (&'a str, &'a str)>) {
    let lines: String = minor
        .into_iter()
        .map(|(version, line)| format!("  {version}  {line}\n"))
        .collect();
    if !lines.is_empty() {
        emit(&Rendered {
            stderr: format!("Changes that may break what worked:\n{lines}"),
            ..Rendered::default()
        });
    }
}

/// Asks before updating from `from` to `to`: `[Y/n]` for a patch release, and
/// `[y/N]` after the minor entries of `notes` for a minor one or for an
/// `older` release than this build.
async fn confirm_update(
    runtime: &Runtime,
    from: &str,
    to: &str,
    lanes: usize,
    notes: &[UpdateNote],
    older: bool,
) -> Result<bool, RpcError> {
    let minor = bump(from, to) == Some(Bump::Minor);
    if minor {
        print_minor_notes(
            notes
                .iter()
                .filter(|note| note.level == "minor")
                .map(|note| (note.version.as_str(), note.line.as_str())),
        );
    }
    let restarts = match lanes {
        0 => "The daemon restarts.".to_owned(),
        n => format!("The daemon restarts and starts {} again.", lanes_word(n)),
    };
    ask(
        runtime,
        &format!("Update devlanes {from} -> {to}? {restarts}"),
        !minor && !older,
    )
    .await
}

/// `next builds` or `stable releases`, as the channel row names them.
fn channel_words(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "stable releases",
        Channel::Next => "next builds",
    }
}

/// `1 lane`, `3 lanes`.
fn lanes_word(n: usize) -> String {
    if n == 1 {
        "1 lane".into()
    } else {
        format!("{n} lanes")
    }
}

/// Asks `question` on the terminal, with `default` for an empty answer,
/// reading the answer off the async threads. Without a terminal to ask on,
/// refuses with `confirmation_required`. A yes erases the question, so the
/// step block takes its place.
async fn ask(runtime: &Runtime, question: &str, default: bool) -> Result<bool, RpcError> {
    if !runtime.can_prompt() {
        return Err(RpcError::new(
            "confirmation_required",
            json!({"flag": "--yes"}),
            2,
        ));
    }
    let choices = if default { "[Y/n]" } else { "[y/N]" };
    eprint!("{question} {choices} ");
    let answer = tokio::task::spawn_blocking(|| {
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer).map(|_| answer)
    })
    .await
    .map_err(|error| io_error(&error))?
    .map_err(|error| io_error(&error))?;
    let yes = match answer.trim() {
        "" => default,
        answer => matches!(answer, "y" | "Y" | "yes"),
    };
    let mut stderr = std::io::stderr();
    if yes && stderr.is_terminal() {
        // The answer's newline left the cursor on the line below the question.
        let _ = crossterm::execute!(stderr, MoveToPreviousLine(1), Clear(ClearType::CurrentLine));
        let _ = stderr.flush();
    }
    Ok(yes)
}

/// Says that nothing newer than `running` exists.
fn up_to_date(runtime: &Runtime, running: &str) -> Result<u8, RpcError> {
    let rendered = match runtime.mode {
        OutputMode::Json => Rendered {
            stdout: format!("{}\n", json!({"updated": false, "running": running})),
            ..Rendered::default()
        },
        _ if runtime.quiet => Rendered::default(),
        _ => Rendered {
            stdout: format!("devlanes {running} is the latest release\n"),
            ..Rendered::default()
        },
    };
    emit(&rendered);
    Ok(0)
}

/// The arguments that run `update --finish` in this output mode.
fn finish_args(runtime: &Runtime) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["--output".into(), runtime.mode.as_str().into()];
    if runtime.quiet {
        args.push("--quiet".into());
    }
    args.extend(["update".into(), "--finish".into()]);
    args
}

/// The block an earlier `devlanes update` handed over, if any.
fn handoff_from_env() -> Option<Handoff> {
    serde_json::from_str(&std::env::var(STEPS_ENV).ok()?).ok()
}

/// Takes over the block an earlier `devlanes update` drew, if any.
fn take_over(view: &mut View) {
    if let Some(handoff) = handoff_from_env() {
        view.take_over(handoff);
    }
}

/// The context lanes start again with after a restart: this shell's
/// environment, as `daemon restart` passes it, without the variables the
/// update itself set.
fn restart_context(runtime: &Runtime) -> Context {
    let env: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| {
            Some((
                devlanes_core::env::fold_name(key.into_string().ok()?),
                value.into_string().ok()?,
            ))
        })
        .filter(|(key, _)| key != STEPS_ENV && key != REOPEN_ENV)
        .collect();
    Context {
        env: Some(env),
        actor: runtime.actor.clone(),
        ..Context::default()
    }
}

/// The channel an update report names.
fn channel_of(channel: &str) -> Channel {
    if channel == Channel::Next.as_str() {
        Channel::Next
    } else {
        Channel::Stable
    }
}

/// Runs `entry` with `args` and `envs` in place of this process. On Unix this
/// returns only when that fails. Windows has no exec: the new process runs on
/// this console while this one waits, ignoring Ctrl-C, and passes its exit
/// status on.
async fn exec_entry(
    entry: &Path,
    args: &[OsString],
    envs: &[(&str, String)],
) -> Result<u8, RpcError> {
    let mut command = std::process::Command::new(entry);
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(io_error(&command.exec()))
    }
    #[cfg(windows)]
    {
        tokio::task::spawn_blocking(move || {
            let mut child = command.spawn().map_err(|error| io_error(&error))?;
            let _ = devlanes_sys_windows::console::set_ctrl_c_ignored(true);
            let status = child.wait();
            let _ = devlanes_sys_windows::console::set_ctrl_c_ignored(false);
            let status = status.map_err(|error| io_error(&error))?;
            Ok(status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .unwrap_or(1))
        })
        .await
        .map_err(|error| io_error(&error))?
    }
}

/// `update_manual` for `method`, naming `command`.
fn manual(method: &InstallMethod, command: &str) -> RpcError {
    RpcError::new(
        "update_manual",
        json!({"method": method.label(), "command": command}),
        1,
    )
}

/// An install failure as the CLI's error.
fn failed(error: &InstallError) -> RpcError {
    match error {
        InstallError::Busy => RpcError::new("update_busy", json!({}), 1),
        InstallError::Command(failure) => RpcError::new(
            "update_failed",
            json!({
                "title": failure.title,
                "tail": failure.tail,
                "command": failure.command,
                "hint": failure.hint,
            }),
            1,
        ),
        other => RpcError::new(
            "update_failed",
            json!({"title": other.to_string(), "hint": "retry with devlanes update"}),
            1,
        ),
    }
}

/// An I/O or task failure as the CLI's error.
fn io_error(error: &dyn std::fmt::Display) -> RpcError {
    RpcError::new("io", json!({"message": error.to_string()}), 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minor_notes_come_first() {
        let note = |version: &str, level: &str, line: &str| UpdateNote {
            version: version.into(),
            level: level.into(),
            line: line.into(),
        };
        let notes = [
            note("0.10.1", "patch", "Fixed a crash."),
            note("0.10.0", "minor", "Removed --acme."),
            note("0.10.0", "patch", "Added acme."),
        ];
        let lines: Vec<&str> = sorted_notes(&notes)
            .iter()
            .map(|note| note.line.as_str())
            .collect();
        assert_eq!(lines, ["Removed --acme.", "Fixed a crash.", "Added acme."]);
        assert_eq!(lanes_word(1), "1 lane");
        assert_eq!(lanes_word(3), "3 lanes");
        assert_eq!(channel_of("next"), Channel::Next);
        assert_eq!(channel_of("stable"), Channel::Stable);
    }

    /// Without an install, a switch says the channel is recorded and why this
    /// build stays.
    #[test]
    fn switches_that_install_nothing_say_why() {
        let file = Path::new("/home/a/.devlanes/config.yaml");
        let stay = |channel, changed, older| Stay {
            channel,
            changed,
            file,
            latest: "0.9.0",
            older,
        };
        let text = stay_text(&stay(Channel::Stable, true, true));
        assert_eq!(
            text,
            format!(
                "updates.channel is stable in {}; the daemon follows it from its next restart\n\
                 devlanes {OWN} stays until a stable release newer than it ships; the latest is 0.9.0\n\
                 devlanes update --stable --force installs 0.9.0 now\n",
                file.display()
            )
        );
        let text = stay_text(&stay(Channel::Next, false, true));
        assert!(
            text.contains("stays until a next build newer than it ships"),
            "{text}"
        );
        assert!(!text.contains("--force"), "{text}");
        assert!(!text.contains("restart"), "{text}");
        let text = stay_text(&stay(Channel::Next, false, false));
        assert!(
            text.ends_with(&format!("devlanes {OWN} is the latest next build\n")),
            "{text}"
        );
    }

    /// `--version` takes one leading `v`; anything else must be an exact
    /// release version.
    #[test]
    fn requested_versions_are_normalised_once() {
        assert_eq!(release_version("0.9.1").ok().as_deref(), Some("0.9.1"));
        assert_eq!(release_version("v0.9.1").ok().as_deref(), Some("0.9.1"));
        assert_eq!(
            release_version("v0.9.1-next.3").ok().as_deref(),
            Some("0.9.1-next.3")
        );
        for refused in ["vv0.9.1", "0.9.1+x", "v0.9.1+x", " 0.9.1", "0.9.1 ", "0.9"] {
            let error = release_version(refused).expect_err(refused);
            assert_eq!(error.code, "update_failed", "{refused:?}");
        }
    }

    /// The daemon's answer counts unless it could not tell: a daemon that
    /// does not know `update.check`, or whose checks are off, leaves the
    /// check to this process.
    #[test]
    fn the_daemon_answers_unless_it_cannot_tell() -> Result<(), RpcError> {
        let unknown = RpcError::new("unknown_method", json!({}), 2);
        assert_eq!(daemon_answer(Err(unknown), false)?, None);
        let other = RpcError::new("io", json!({}), 1);
        assert_eq!(
            daemon_answer(Err(other), false).map_err(|error| error.code),
            Err("io".to_owned())
        );
        // A forced check the daemon could not read fails here too.
        let unread = check_failed(Channel::Stable, &"timed out", "devlanes update");
        let passed = daemon_answer(Err(unread.clone()), false).expect_err("passes through");
        assert_eq!((passed.code, passed.data), (unread.code, unread.data));
        assert_eq!(daemon_answer(Ok(Value::Null), true)?, None);
        assert_eq!(daemon_answer(Ok(Value::Null), false)?, Some(None));
        let info = found(Channel::Stable, "99.0.0".into(), OWN).expect("newer");
        let reply = serde_json::to_value(&info).map_err(|error| io_error(&error))?;
        assert_eq!(daemon_answer(Ok(reply), false)?, Some(Some(info)));
        Ok(())
    }

    /// A `devlanes` that reports `version`: a script on Unix, the prebuilt
    /// `fake-tool` on Windows, which cannot run scripts as programs.
    fn fake_devlanes(dir: &Path, version: &str) -> PathBuf {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let program = dir.join("devlanes");
            std::fs::write(&program, format!("#!/bin/sh\necho 'devlanes {version}'\n"))
                .expect("script");
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
            program
        }
        #[cfg(windows)]
        {
            use devlanes_testing::fake_tool::{Behaviour, install};
            install(
                dir,
                "devlanes",
                &Behaviour::new().stdout(format!("devlanes {version}\n")),
            )
            .expect("install fake-tool")
        }
    }

    /// A `devlanes update` that waited for another install to finish does
    /// not run the package manager again when that one installed the release.
    #[tokio::test]
    async fn waiting_for_an_install_of_the_same_release_skips_the_package_manager() {
        use devlanes_cli::clap::Parser;
        let dir = tempfile::tempdir().expect("dir");
        let paths = Paths::new(dir.path().join("home")).expect("paths");
        paths.prepare().expect("directories");
        let entry = fake_devlanes(dir.path(), "99.0.1");
        let held = Lock::acquire(&paths.home, Duration::ZERO).expect("lock");
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        // Fails if it runs.
        let plan = Plan::PackageManager(vec![install::Argv {
            program: dir.path().join("no-such-npm"),
            args: Vec::new(),
        }]);
        let cli = devlanes_cli::Cli::parse_from(["devlanes", "--output", "plain"]);
        let runtime = Runtime::resolve(&cli, devlanes_cli::Signals::default()).expect("runtime");
        let kind = Kind::Update {
            from: OWN.to_owned(),
            to: "99.0.1".to_owned(),
        };
        let mut view = View::new(&runtime, &paths, kind);
        let ran = tokio::time::timeout(
            Duration::from_secs(30),
            run_plan(&mut view, &paths, &plan, "99.0.1", Channel::Stable, &entry),
        )
        .await
        .expect("in time");
        assert!(ran.is_ok(), "{ran:?}");
        release.join().expect("released");
    }

    /// A release read here is reported when it is newer than the daemon or
    /// this build, as the daemon would report it, without notes.
    #[test]
    fn releases_read_here_are_reported_like_the_daemons() {
        let info = found(Channel::Next, "99.0.0-next.1".into(), OWN).expect("newer");
        assert_eq!(
            (
                info.channel.as_str(),
                info.bump.as_str(),
                info.state.as_str()
            ),
            ("next", "minor", "available")
        );
        assert!(info.notes.is_empty());
        assert_eq!(found(Channel::Stable, OWN.into(), OWN), None);
        // This build is newer than the daemon, and the latest release.
        assert!(found(Channel::Stable, OWN.into(), "0.0.1").is_some());
    }

    /// A daemon that cannot be reached is a warning, never a reason for the
    /// TUI not to open.
    #[tokio::test]
    async fn an_unreachable_daemon_does_not_keep_the_tui_closed() {
        use devlanes_cli::clap::Parser;
        let home = tempfile::tempdir().expect("home");
        let paths = Paths::new(home.path().join("home")).expect("paths");
        paths.prepare().expect("directories");
        // Every connection closes before the hello.
        let mut listener = devlanes_ipc::local::LocalListener::bind(&paths.socket)
            .await
            .expect("listener");
        let server = tokio::spawn(async move {
            while let Ok(stream) = listener.accept().await {
                drop(stream);
            }
        });
        let cli = devlanes_cli::Cli::parse_from(["devlanes", "--output", "human"]);
        let mut runtime =
            Runtime::resolve(&cli, devlanes_cli::Signals::default()).expect("runtime");
        (runtime.stdout_tty, runtime.stdin_tty, runtime.ci) = (true, true, None);
        let limit = Duration::from_secs(30);
        let failed = tokio::time::timeout(limit, switch_on_start(&runtime, &paths))
            .await
            .expect("in time");
        assert!(failed.is_err(), "the connection fails");
        tokio::time::timeout(limit, startup(&runtime, &paths))
            .await
            .expect("startup returns, so the TUI opens");
        server.abort();
    }

    #[test]
    fn the_config_channel_wins_over_the_build() {
        let mut config = devlanes_config::UserConfig::default();
        assert_eq!(effective_channel(&config), Channel::of_version(OWN));
        config.updates.channel = Some(UpdateChannel::Next);
        assert_eq!(effective_channel(&config), Channel::Next);
        config.updates.channel = Some(UpdateChannel::Stable);
        assert_eq!(effective_channel(&config), Channel::Stable);
        assert_eq!(channel_words(Channel::Next), "next builds");
    }
}
