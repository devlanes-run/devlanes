//! `devlanes uninstall`: stops the daemon, removes its login service, undoes
//! `devlanes setup`, removes this install the way it was installed, and
//! deletes the devlanes home and lane data only when told to. A step that
//! fails is reported and the next one still runs. `--dry-run` prints the
//! plan and changes nothing; JSON output is the plan or the result.
//!
//! On Windows a running executable cannot delete itself, so a script
//! install's `devlanes.exe` (and the link a package-manager removal leaves,
//! see [`devlanes_update::retain_own_exe`]) are deleted after this process
//! exits, by a detached `cmd.exe` that retries until the file is free.

use std::path::{Path, PathBuf};

use devlanes_cli::{DataChoice, OutputMode, Rendered, Runtime, UninstallAction};
use devlanes_client::{Client, Paths};
use devlanes_core::paths::path_starts_with;
use devlanes_proto::{Context, RpcError};
use devlanes_update::{
    InstallMethod,
    install::{self, Argv},
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    daemon::LoginService,
    lifecycle::{self, Stopped},
    output::emit,
};

/// Where a step stands: planned, or what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum State {
    /// Would change something.
    Todo,
    /// Nothing to do.
    Skip,
    Done,
    Failed,
    /// The data stays.
    Kept,
    /// Left to the person, who is told what to do.
    Manual,
}

impl State {
    /// The word at the start of the step's line.
    fn word(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::Skip => "skip",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Kept => "kept",
            Self::Manual => "manual",
        }
    }
}

/// One step of the plan or the result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Step {
    step: &'static str,
    state: State,
    /// What it changes or changed, or why not; one line each.
    detail: Vec<String>,
}

impl Step {
    /// A step named `step` in `state` with `detail`.
    fn new(step: &'static str, state: State, detail: impl IntoIterator<Item = String>) -> Self {
        Self {
            step,
            state,
            detail: detail.into_iter().collect(),
        }
    }
}

/// What kind of data an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DataKind {
    /// The devlanes home: config, state, CA, logs, and lane data by default.
    Home,
    /// The project folders under `lanes.data_dir` outside the home; the
    /// folder itself may hold other files, which stay.
    LaneData,
    /// The daemon socket outside the home (macOS with a long home path,
    /// Linux's runtime directory).
    #[cfg(unix)]
    Socket,
}

/// A path the data step lists, with its size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct DataItem {
    path: PathBuf,
    bytes: u64,
    kind: DataKind,
}

impl DataItem {
    /// `/Users/a/.devlanes  12.3 MiB`.
    fn line(&self) -> String {
        let what = match self.kind {
            DataKind::Home => "",
            DataKind::LaneData => " (lane data folders)",
            #[cfg(unix)]
            DataKind::Socket => " (daemon socket)",
        };
        format!(
            "{}  {}{what}",
            self.path.display(),
            devlanes_cli::bytes(self.bytes)
        )
    }
}

/// What happens to the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataAction {
    Delete,
    Keep,
    /// Ask on this terminal, no by default.
    Ask,
}

impl DataAction {
    /// The action for `choice`; without a terminal to ask on the data stays.
    fn of(choice: DataChoice, can_prompt: bool) -> Self {
        match choice {
            DataChoice::Remove => Self::Delete,
            DataChoice::Keep => Self::Keep,
            DataChoice::Ask if can_prompt => Self::Ask,
            DataChoice::Ask => Self::Keep,
        }
    }

    /// The word JSON output uses.
    fn word(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Keep => "keep",
            Self::Ask => "ask",
        }
    }
}

/// How this install's files go away.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Binary {
    /// An `install.sh` or `install.ps1` install, or a standalone binary:
    /// delete `files` in `dir`; on Windows also drop `dir` from the user
    /// `PATH` when `path_entry`.
    Files {
        dir: PathBuf,
        files: Vec<PathBuf>,
        path_entry: bool,
    },
    /// Run the package manager's remove commands.
    Commands(Vec<Argv>),
    /// Nothing to remove, and why.
    Nothing(String),
    /// Left to the person: what they do.
    Manual(String),
}

/// The removal for `method`, whose running executable is `exe`, given the
/// devlanes `files` a script install's directory holds and whether the user
/// `PATH` names that directory.
fn binary_of(method: &InstallMethod, exe: &Path, files: Vec<PathBuf>, path_entry: bool) -> Binary {
    if let InstallMethod::Script(dir) = method {
        return Binary::Files {
            dir: dir.clone(),
            files,
            path_entry,
        };
    }
    if let Some(commands) = install::removal_commands(method, exe) {
        return Binary::Commands(commands);
    }
    match method {
        InstallMethod::Uvx | InstallMethod::PipxRun => Binary::Nothing(format!(
            "{} runs devlanes from its cache; nothing is installed to remove",
            method.label()
        )),
        InstallMethod::Local { manifest } => {
            Binary::Manual(format!("remove devlanes from {}", manifest.display()))
        }
        _ => Binary::Nothing(format!("a source build stays: {}", exe.display())),
    }
}

impl Binary {
    /// The plan's step.
    fn plan(&self) -> Step {
        let step = |state, detail: Vec<String>| Step::new("binary", state, detail);
        match self {
            Self::Files {
                dir,
                files,
                path_entry,
            } => {
                let mut detail = Vec::new();
                if !files.is_empty() {
                    detail.push(format!("delete {}", joined(files)));
                }
                if *path_entry {
                    detail.push(format!("remove {} from the user PATH", dir.display()));
                }
                if detail.is_empty() {
                    step(
                        State::Skip,
                        vec![format!("no devlanes files in {}", dir.display())],
                    )
                } else {
                    step(State::Todo, detail)
                }
            }
            Self::Commands(commands) => step(
                State::Todo,
                commands
                    .iter()
                    .map(|argv| format!("run {}", argv.display()))
                    .collect(),
            ),
            Self::Nothing(reason) => step(State::Skip, vec![reason.clone()]),
            Self::Manual(what) => step(State::Manual, vec![what.clone()]),
        }
    }
}

/// `a, b` for paths.
fn joined(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The devlanes files in a script install's `dir`: the release's binaries,
/// and on Windows the `devlanes.exe.old-*` copies updates leave. Blocking.
fn script_files(dir: &Path) -> Vec<PathBuf> {
    let mut old: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .filter(|entry| {
                    cfg!(windows)
                        && entry
                            .file_name()
                            .to_string_lossy()
                            .to_lowercase()
                            .starts_with("devlanes.exe.old-")
                })
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default();
    old.sort();
    install::BINARIES
        .iter()
        .map(|name| dir.join(name))
        .filter(|path| path.is_file())
        .chain(old)
        .collect()
}

/// The plan's [`Binary`] for the running executable. Blocking.
fn current_binary() -> (InstallMethod, Binary) {
    let exe = devlanes_update::running_exe().unwrap_or_else(|_| PathBuf::from("devlanes"));
    let method = InstallMethod::current();
    let (files, path_entry) = match &method {
        InstallMethod::Script(dir) => (script_files(dir), on_user_path(dir)),
        _ => (Vec::new(), false),
    };
    let binary = binary_of(&method, &exe, files, path_entry);
    (method, binary)
}

/// Whether the user `PATH` names `dir`. Blocking.
#[cfg(windows)]
fn on_user_path(dir: &Path) -> bool {
    matches!(
        devlanes_sys_windows::registry::read_text("Environment", "Path"),
        Ok(Some((value, _))) if without_path_entry(&value, dir, expand_env).is_some()
    )
}

/// Install scripts on macOS and Linux leave the shell profile to the person.
#[cfg(unix)]
fn on_user_path(_dir: &Path) -> bool {
    false
}

/// `value` (a `PATH` value) without its entries that name `dir`, compared
/// as `install.ps1` compares them: quotes and surrounding spaces dropped,
/// `%VARIABLES%` expanded by `expand`, a trailing `\` ignored, case
/// ignored. The other entries stay byte for byte. `None` when no entry
/// names `dir`.
#[cfg(any(windows, test))]
fn without_path_entry(value: &str, dir: &Path, expand: impl Fn(&str) -> String) -> Option<String> {
    let normal = |path: &str| path.trim().trim_end_matches('\\').to_lowercase();
    let wanted = normal(&dir.to_string_lossy());
    let mut found = false;
    let kept: Vec<&str> = value
        .split(';')
        .filter(|entry| {
            let entry = entry.trim().trim_matches('"');
            let same = !entry.is_empty() && normal(&expand(entry)) == wanted;
            found |= same;
            !same
        })
        .collect();
    found.then(|| kept.join(";"))
}

/// `%NAME%` references in `text` replaced by `lookup(NAME)`; unknown names
/// stay as written, as Windows' `ExpandEnvironmentStrings` leaves them.
#[cfg(any(windows, test))]
fn expand_variables(text: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => match lookup(&after[..end]) {
                Some(value) => {
                    out.push_str(&value);
                    rest = &after[end + 1..];
                }
                None => {
                    out.push('%');
                    rest = after;
                }
            },
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// [`expand_variables`] with this process's environment.
#[cfg(windows)]
fn expand_env(text: &str) -> String {
    expand_variables(text, |name| std::env::var(name).ok())
}

/// Removes `dir` from the user `PATH`, keeping the value's type, and tells
/// running programs, as `install.ps1` does after adding it. `false` when the
/// `PATH` did not name it. Blocking.
#[cfg(windows)]
fn remove_user_path_entry(dir: &Path) -> std::io::Result<bool> {
    use devlanes_sys_windows::registry;
    let Some((value, expandable)) = registry::read_text("Environment", "Path")? else {
        return Ok(false);
    };
    let Some(updated) = without_path_entry(&value, dir, expand_env) else {
        return Ok(false);
    };
    if updated.trim_matches(';').is_empty() {
        registry::delete_value("Environment", "Path")?;
    } else {
        registry::write_text("Environment", "Path", &updated, expandable)?;
    }
    registry::broadcast_environment_change();
    Ok(true)
}

/// What a detached `cmd.exe` deletes once this process exits (Windows).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AfterExit {
    /// Files this process keeps busy: its own executable, or the link to it.
    files: Vec<PathBuf>,
    /// Folders removed with everything in them once `files` are gone.
    trees: Vec<PathBuf>,
    /// Folders removed once `files` are gone, only when empty.
    dirs: Vec<PathBuf>,
}

/// The `cmd.exe /c` text for `after`: try to delete the files every second
/// for two minutes; once they are gone, remove the folders and stop. A
/// running executable cannot be deleted, so the deletion succeeds only
/// after this process (and the image it maps) has ended. `None` without
/// files, or when a path has `"` or `%`, which cmd cannot take literally.
#[cfg(any(windows, test))]
fn after_exit_script(after: &AfterExit) -> Option<String> {
    let quote = |path: &PathBuf| -> Option<String> {
        let text = path.to_str()?;
        (!text.contains(['"', '%'])).then(|| format!("\"{text}\""))
    };
    let files = after.files.iter().map(quote).collect::<Option<Vec<_>>>()?;
    if files.is_empty() {
        return None;
    }
    let mut then = String::new();
    for tree in &after.trees {
        then.push_str(&format!("rmdir /s /q {} >nul 2>&1 & ", quote(tree)?));
    }
    for dir in &after.dirs {
        then.push_str(&format!("rmdir {} >nul 2>&1 & ", quote(dir)?));
    }
    then.push_str("exit 0");
    let gone: String = files
        .iter()
        .map(|file| format!("if not exist {file} "))
        .collect();
    Some(format!(
        "for /l %i in (1,1,120) do @(del /f /q {} >nul 2>&1 & ping -n 2 127.0.0.1 >nul & {gone}({then}))",
        files.join(" ")
    ))
}

/// Starts the detached `cmd.exe` for `after`, outside this process's job
/// when the job allows it, with no console window and `NUL` for its
/// streams, so it holds nothing a caller waits on. Blocking.
#[cfg(windows)]
fn spawn_after_exit(after: &AfterExit) -> std::io::Result<()> {
    use devlanes_sys_windows::spawn::{self, CreationFlags, Spawn, Stdio, Stream};
    let Some(script) = after_exit_script(after) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a path has a character cmd.exe cannot take",
        ));
    };
    let system = devlanes_sys_windows::file::system_directory()?;
    let program = system.join("cmd.exe");
    // `/s`: cmd drops the outer quotes and runs the rest as written.
    let command_line = std::ffi::OsString::from(format!("cmd.exe /d /s /c \"{script}\""));
    let mut spec = Spawn {
        program: &program,
        command_line: &command_line,
        environment: None,
        cwd: Some(&system),
        stdio: Stdio::Handles {
            stdin: Stream::Null,
            stdout: Stream::Null,
            stderr: Stream::Null,
        },
        inherit: &[],
        job: None,
        flags: CreationFlags {
            no_window: true,
            new_process_group: true,
            breakaway_from_job: true,
            ..CreationFlags::default()
        },
    };
    match spawn::spawn(&spec) {
        Ok(_) => Ok(()),
        // A job that does not allow breakaway; the child then ends with
        // the job, which a terminal session does not close early.
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            spec.flags.breakaway_from_job = false;
            spawn::spawn(&spec).map(drop)
        }
        Err(error) => Err(error),
    }
}

/// The paths the data step lists: the devlanes home, the lane data folders
/// under `data_root` when it lies outside the home, and a daemon socket
/// outside the home. Blocking.
fn data_items(paths: &Paths, data_root: &Path) -> Vec<DataItem> {
    let mut items = Vec::new();
    if paths.home.symlink_metadata().is_ok() {
        items.push(DataItem {
            path: paths.home.clone(),
            bytes: size(&paths.home),
            kind: DataKind::Home,
        });
    }
    if !path_starts_with(data_root, &paths.home) {
        let folders = crate::daemon::project_data_folders(data_root).unwrap_or_default();
        if !folders.is_empty() {
            items.push(DataItem {
                path: data_root.to_path_buf(),
                bytes: folders.iter().map(|name| size(&data_root.join(name))).sum(),
                kind: DataKind::LaneData,
            });
        }
    }
    // On Windows the socket is a named pipe, which no folder holds.
    #[cfg(unix)]
    if paths.socket.parent() != Some(paths.run.as_path())
        && !path_starts_with(&paths.socket, &paths.home)
        && paths.socket.symlink_metadata().is_ok()
    {
        items.push(DataItem {
            path: paths.socket.clone(),
            bytes: 0,
            kind: DataKind::Socket,
        });
    }
    items
}

/// The bytes of the files under `path`, symlinks counted as links. Blocking.
fn size(path: &Path) -> u64 {
    let Ok(meta) = path.symlink_metadata() else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .map(|entries| entries.flatten().map(|entry| size(&entry.path())).sum())
        .unwrap_or(0)
}

/// Whether deleting `home` would take the user's home directory or a whole
/// file system with it, as a `DEVLANES_HOME` of `~` or `/` would.
fn holds_the_user_home(home: &Path, user_home: Option<&Path>) -> bool {
    home.parent().is_none() || user_home.is_some_and(|user| path_starts_with(user, home))
}

/// Deletes `items` (see [`DataKind`] for what each covers). Returns the
/// failures. A home that holds a file of `after` (the link to the running
/// executable on Windows) cannot go yet, so `after` gets it. Blocking.
fn delete_data(items: &[DataItem], after: &mut AfterExit) -> Vec<String> {
    let mut failures = Vec::new();
    for item in items {
        let result = match item.kind {
            DataKind::Home
                if holds_the_user_home(&item.path, devlanes_client::user_home().as_deref()) =>
            {
                Err(std::io::Error::other("it holds your home directory"))
            }
            DataKind::Home => std::fs::remove_dir_all(&item.path),
            DataKind::LaneData => crate::daemon::project_data_folders(&item.path)
                .map_err(|error| std::io::Error::other(error.code))
                .and_then(|folders| {
                    folders
                        .iter()
                        .try_for_each(|name| std::fs::remove_dir_all(item.path.join(name)))
                })
                .map(|()| {
                    // The folder may hold other files; it goes only when empty.
                    let _ = std::fs::remove_dir(&item.path);
                }),
            #[cfg(unix)]
            DataKind::Socket => std::fs::remove_file(&item.path).map(|()| {
                // Other homes' sockets may share the folder.
                if let Some(parent) = item.path.parent() {
                    let _ = std::fs::remove_dir(parent);
                }
            }),
        };
        if let Err(error) = result {
            let busy = item.kind == DataKind::Home
                && after
                    .files
                    .iter()
                    .any(|file| path_starts_with(file, &item.path));
            if busy {
                after.trees.push(item.path.clone());
            } else {
                failures.push(format!("{}: {error}", item.path.display()));
            }
        }
    }
    failures
}

/// The steps that need no change yet: whether the daemon runs, the login
/// service, and what `setup --remove` undoes.
async fn plan_daemon(paths: &Paths) -> Step {
    match Client::connect_any(paths).await {
        Ok(client) => {
            let detail = match client.peer.managed_processes {
                0 => "stop the daemon".to_owned(),
                processes => format!(
                    "stop the daemon and {}",
                    lifecycle::processes_word(processes)
                ),
            };
            Step::new("daemon", State::Todo, [detail])
        }
        Err(error) if error.is_not_running() => {
            Step::new("daemon", State::Skip, ["the daemon is not running".into()])
        }
        Err(error) => Step::new("daemon", State::Todo, [format!("stop the daemon: {error}")]),
    }
}

/// The login service step: what would be removed.
async fn plan_service(paths: &Paths) -> (Step, bool) {
    match crate::daemon::login_service(paths).await {
        Ok(LoginService::Absent) => (
            Step::new(
                "service",
                State::Skip,
                ["no login service is installed".into()],
            ),
            false,
        ),
        Ok(LoginService::OtherHome { location, home }) => (
            Step::new(
                "service",
                State::Skip,
                [format!(
                    "{location} starts the daemon of {}, so it stays",
                    home.display()
                )],
            ),
            false,
        ),
        Ok(LoginService::Installed { location }) => (
            Step::new("service", State::Todo, [format!("remove {location}")]),
            true,
        ),
        Err(error) => (
            Step::new("service", State::Failed, [message(&error)]),
            false,
        ),
    }
}

/// The setup step: what `setup --remove` changes.
async fn plan_setup(paths: &Paths) -> Step {
    let plan = crate::setup::removal_plan(paths, false).await;
    if plan.is_empty() {
        return Step::new(
            "setup",
            State::Skip,
            ["devlanes setup changed nothing on this machine".into()],
        );
    }
    Step::new(
        "setup",
        State::Todo,
        plan.into_iter()
            .map(|item| format!("{}: {}", item.step, item.what)),
    )
}

/// The data step of the plan.
fn plan_data(items: &[DataItem], action: DataAction) -> Step {
    if items.is_empty() {
        return Step::new("data", State::Skip, ["no devlanes data here".into()]);
    }
    let what = match action {
        DataAction::Delete => "delete:",
        DataAction::Keep => "keep:",
        DataAction::Ask => "ask before deleting (no by default):",
    };
    let state = if action == DataAction::Delete {
        State::Todo
    } else {
        State::Kept
    };
    Step::new(
        "data",
        state,
        std::iter::once(what.to_owned()).chain(items.iter().map(DataItem::line)),
    )
}

/// The message of an error, or its code.
fn message(error: &RpcError) -> String {
    error
        .data
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or(&error.code)
        .to_owned()
}

/// `state  step     detail`, continuation lines aligned under the detail.
fn lines(step: &Step) -> String {
    let head = format!("{:<6}  {:<7}  ", step.state.word(), step.step);
    let mut out = String::new();
    for (index, detail) in step.detail.iter().enumerate() {
        if index == 0 {
            out.push_str(&head);
        } else {
            out.push_str(&" ".repeat(head.len()));
        }
        out.push_str(detail);
        out.push('\n');
    }
    if step.detail.is_empty() {
        out.push_str(head.trim_end());
        out.push('\n');
    }
    out
}

/// Prints `step` as it finishes: nothing for JSON, only failures (on
/// stderr) with `--quiet`.
fn show(runtime: &Runtime, step: &Step) {
    if runtime.mode == OutputMode::Json {
        return;
    }
    if runtime.quiet {
        if step.state == State::Failed {
            emit(&Rendered {
                stderr: lines(step),
                ..Rendered::default()
            });
        }
        return;
    }
    emit(&Rendered {
        stdout: lines(step),
        ..Rendered::default()
    });
}

/// `Uninstall devlanes?`, naming the processes the daemon stops when some run.
fn question(processes: usize) -> String {
    match processes {
        0 => "Uninstall devlanes?".to_owned(),
        n => format!(
            "Uninstall devlanes and stop {}?",
            lifecycle::processes_word(n)
        ),
    }
}

/// Asks once before anything changes, unless `yes` (`--yes`); without a
/// terminal to ask on it refuses with `confirmation_required`, whether or not
/// processes run. A yes on a terminal erases the question.
async fn confirmed(runtime: &Runtime, yes: bool, processes: usize) -> Result<bool, RpcError> {
    if yes {
        return Ok(true);
    }
    if !runtime.can_prompt() {
        return Err(RpcError::new(
            "confirmation_required",
            json!({"flag": "--yes", "processes": processes}),
            2,
        ));
    }
    let (asking, question) = (runtime.clone(), question(processes));
    let yes = blocking(move || crate::commands::confirm(&asking, &question)).await??;
    let mut stderr = std::io::stderr();
    if yes && std::io::IsTerminal::is_terminal(&stderr) {
        // The answer's newline left the cursor on the line below the question.
        let _ = crossterm::execute!(
            stderr,
            crossterm::cursor::MoveToPreviousLine(1),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine)
        );
    }
    Ok(yes)
}

/// Runs blocking work off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| RpcError::new("io", json!({"message": error.to_string()}), 1))
}

/// Runs `devlanes uninstall`.
pub(crate) async fn run(runtime: &Runtime, action: UninstallAction) -> Result<u8, RpcError> {
    let paths = Paths::discover()
        .map_err(|error| RpcError::new("io", json!({"message": error.to_string()}), 1))?;
    let (method, binary) = blocking(current_binary).await?;
    let config = crate::user_config::lenient(&paths).await;
    let data_root = config.lanes.data_root(&paths.home);
    let data_action = DataAction::of(action.data, runtime.can_prompt());
    let data = {
        let (paths, data_root) = (paths.clone(), data_root.clone());
        blocking(move || data_items(&paths, &data_root)).await?
    };
    if action.dry_run {
        let (service, _) = plan_service(&paths).await;
        let steps = vec![
            plan_daemon(&paths).await,
            service,
            plan_setup(&paths).await,
            binary.plan(),
            plan_data(&data, data_action),
        ];
        if runtime.mode == OutputMode::Json {
            emit(&Rendered {
                stdout: format!(
                    "{}\n",
                    json!({
                        "dry_run": true,
                        "method": method.label(),
                        "steps": steps,
                        "data": {"action": data_action.word(), "items": data},
                    })
                ),
                ..Rendered::default()
            });
        } else if !runtime.quiet {
            let mut stdout: String = steps.iter().map(lines).collect();
            stdout.push_str("dry run: nothing was changed\n");
            emit(&Rendered {
                stdout,
                ..Rendered::default()
            });
        }
        return Ok(0);
    }

    let processes = match Client::connect_any(&paths).await {
        Ok(client) => client.peer.managed_processes,
        Err(_) => 0,
    };
    // Nothing has changed yet, so an agent refused here can rerun with --yes.
    if !confirmed(runtime, action.context.yes, processes).await? {
        if !runtime.quiet {
            emit(&Rendered {
                stderr: "nothing was changed\n".into(),
                ..Rendered::default()
            });
        }
        return Ok(devlanes_cli::exit::USAGE);
    }

    let mut steps = Vec::new();
    // Steps run quietly: each prints one line when it ends.
    let quiet = Runtime {
        quiet: true,
        ..runtime.clone()
    };

    // Asked above, once for everything.
    let context = Context {
        yes: true,
        ..action.context.clone()
    };
    let stopped =
        lifecycle::stop_daemon(runtime, &paths, &context, "uninstall devlanes and stop").await;
    let (daemon, drawn) = match stopped {
        Ok(Stopped::NotRunning) => (
            Step::new("daemon", State::Skip, ["the daemon was not running".into()]),
            false,
        ),
        Ok(Stopped::Done { live, .. }) => (
            Step::new("daemon", State::Done, ["stopped the daemon".into()]),
            live,
        ),
        Ok(Stopped::Declined) => (
            Step::new(
                "daemon",
                State::Skip,
                ["the daemon was left running".into()],
            ),
            false,
        ),
        Err(error) => (Step::new("daemon", State::Failed, [message(&error)]), false),
    };
    // The step block already showed the stop.
    if !drawn {
        show(runtime, &daemon);
    }
    steps.push(daemon);

    let (service, installed) = plan_service(&paths).await;
    let service = if installed {
        match crate::daemon::remove_login_service().await {
            Ok(location) => Step::new("service", State::Done, [format!("removed {location}")]),
            Err(error) => Step::new("service", State::Failed, [message(&error)]),
        }
    } else {
        service
    };
    show(runtime, &service);
    steps.push(service);

    let setup = plan_setup(&paths).await;
    let setup = if setup.state == State::Todo {
        let failure = match crate::setup::remove(&quiet, &paths, false).await {
            Ok(0) => None,
            // The step printed its error.
            Ok(_) => Some("could not undo all of it".to_owned()),
            Err(error) => Some(message(&error)),
        };
        match failure {
            None => Step {
                state: State::Done,
                ..setup
            },
            // The binary goes next, so name what is left to undo by hand.
            Some(failure) => {
                let left = plan_setup(&paths).await;
                let detail: Vec<String> = if left.state == State::Todo {
                    std::iter::once(format!("{failure}; still in place:"))
                        .chain(left.detail)
                        .collect()
                } else {
                    vec![failure]
                };
                Step::new("setup", State::Failed, detail)
            }
        }
    } else {
        setup
    };
    show(runtime, &setup);
    steps.push(setup);

    let mut after = AfterExit::default();
    let removed = remove_binary(&binary, &paths, &mut after).await;
    show(runtime, &removed);
    steps.push(removed);

    let (data_step, deleted) = remove_data(runtime, data, data_action, &mut after).await?;
    show(runtime, &data_step);
    steps.push(data_step);

    #[cfg(windows)]
    if !after.files.is_empty()
        && let Err(error) = blocking({
            let after = after.clone();
            move || spawn_after_exit(&after)
        })
        .await?
    {
        let step = Step::new(
            "binary",
            State::Failed,
            [format!(
                "could not start the deletion of {} after exit: {error}; delete it once this command ends",
                joined(&after.files)
            )],
        );
        show(runtime, &step);
        steps.push(step);
    }

    let failed = steps.iter().any(|step| step.state == State::Failed);
    if runtime.mode == OutputMode::Json {
        emit(&Rendered {
            stdout: format!(
                "{}\n",
                json!({
                    "dry_run": false,
                    "method": method.label(),
                    "steps": steps,
                    "data": {"deleted": deleted},
                })
            ),
            ..Rendered::default()
        });
    }
    Ok(u8::from(failed))
}

/// Removes this install's files, runs its package manager's removal, or
/// says what stays.
async fn remove_binary(binary: &Binary, paths: &Paths, after: &mut AfterExit) -> Step {
    let step = |state, detail: Vec<String>| Step::new("binary", state, detail);
    match binary {
        Binary::Files {
            dir,
            files,
            path_entry,
        } => {
            let (dir, files, path_entry) = (dir.clone(), files.clone(), *path_entry);
            let outcome = blocking(move || remove_files(&dir, &files, path_entry)).await;
            match outcome {
                Ok((detail, later, failed)) => {
                    after.files.extend(later.files);
                    after.dirs.extend(later.dirs);
                    step(if failed { State::Failed } else { State::Done }, detail)
                }
                Err(error) => step(State::Failed, vec![message(&error)]),
            }
        }
        Binary::Commands(commands) => {
            // Windows refuses to delete the running devlanes.exe unless a
            // second link to it exists; the link goes after this process.
            #[cfg(windows)]
            let retained = {
                let home = paths.home.clone();
                blocking(move || devlanes_update::retain_own_exe(&home))
                    .await
                    .ok()
                    .flatten()
            };
            #[cfg(windows)]
            if let Some(link) = &retained {
                after.files.push(link.path().to_path_buf());
            }
            let log = paths.logs.join("uninstall.log");
            let shown: Vec<String> = commands
                .iter()
                .map(|argv| format!("ran {}", argv.display()))
                .collect();
            match install::run_commands(commands, &log, "devlanes uninstall").await {
                Ok(()) => step(State::Done, shown),
                Err(install::InstallError::Command(failure)) => {
                    let mut detail = vec![failure.title.clone()];
                    detail.extend(failure.tail.lines().map(str::to_owned));
                    detail.push(failure.hint.clone());
                    step(State::Failed, detail)
                }
                Err(error) => step(State::Failed, vec![error.to_string()]),
            }
        }
        Binary::Nothing(_) | Binary::Manual(_) => binary.plan(),
    }
}

/// Deletes a script install's `files` in `dir` and, on Windows, its user
/// `PATH` entry. Files that stay busy (the running `devlanes.exe`) go into
/// the returned [`AfterExit`], with the install script's default folder.
/// Returns the step's lines and whether something failed. Blocking.
fn remove_files(dir: &Path, files: &[PathBuf], path_entry: bool) -> (Vec<String>, AfterExit, bool) {
    let mut later = AfterExit::default();
    let (mut deleted, mut detail, mut failed) = (Vec::new(), Vec::new(), false);
    for file in files {
        match std::fs::remove_file(file) {
            Ok(()) => deleted.push(file.clone()),
            Err(_) if cfg!(windows) => later.files.push(file.clone()),
            Err(error) => {
                failed = true;
                detail.push(format!("could not delete {}: {error}", file.display()));
            }
        }
    }
    if !deleted.is_empty() {
        detail.insert(0, format!("deleted {}", joined(&deleted)));
    }
    if !later.files.is_empty() {
        detail.push(format!(
            "{} is deleted once this command exits",
            joined(&later.files)
        ));
    }
    #[cfg(windows)]
    {
        if path_entry {
            match remove_user_path_entry(dir) {
                Ok(true) => detail.push(format!("removed {} from the user PATH", dir.display())),
                Ok(false) => {}
                Err(error) => {
                    failed = true;
                    detail.push(format!(
                        "could not remove {} from the user PATH: {error}",
                        dir.display()
                    ));
                }
            }
        }
        // install.ps1 made its default folder; it goes once empty.
        let default = std::env::var_os("LOCALAPPDATA")
            .filter(|dir| !dir.is_empty())
            .map(|local| Path::new(&local).join("Programs").join("devlanes"));
        if default.is_some_and(|default| devlanes_core::paths::same_path(&default, dir)) {
            later.dirs.push(dir.to_path_buf());
        }
    }
    #[cfg(unix)]
    let _ = (dir, path_entry);
    if detail.is_empty() {
        detail.push(format!("no devlanes files in {}", dir.display()));
    }
    (detail, later, failed)
}

/// The data step: deletes, keeps, or asks first (no by default).
async fn remove_data(
    runtime: &Runtime,
    items: Vec<DataItem>,
    action: DataAction,
    after: &mut AfterExit,
) -> Result<(Step, bool), RpcError> {
    if items.is_empty() {
        return Ok((plan_data(&items, action), false));
    }
    let delete = match action {
        DataAction::Delete => true,
        DataAction::Keep => false,
        DataAction::Ask => {
            let list: String = items
                .iter()
                .map(|item| format!("  {}\n", item.line()))
                .collect();
            emit(&Rendered {
                stderr: format!("devlanes data on this machine:\n{list}"),
                ..Rendered::default()
            });
            let runtime = runtime.clone();
            blocking(move || crate::commands::confirm(&runtime, "Delete these?")).await??
        }
    };
    let lines = || items.iter().map(DataItem::line);
    if !delete {
        return Ok((
            Step::new(
                "data",
                State::Kept,
                std::iter::once("kept, delete by hand when no longer needed:".to_owned())
                    .chain(lines()),
            ),
            false,
        ));
    }
    let (failures, later) = {
        let items = items.clone();
        let mut later = after.clone();
        blocking(move || (delete_data(&items, &mut later), later)).await?
    };
    let deferred: Vec<PathBuf> = later.trees[after.trees.len()..].to_vec();
    *after = later;
    if failures.is_empty() {
        let mut detail: Vec<String> = std::iter::once("deleted:".to_owned())
            .chain(lines())
            .collect();
        if !deferred.is_empty() {
            detail.push(format!(
                "{} goes once this command exits",
                joined(&deferred)
            ));
        }
        Ok((Step::new("data", State::Done, detail), true))
    } else {
        Ok((
            Step::new(
                "data",
                State::Failed,
                failures
                    .into_iter()
                    .map(|failure| format!("could not delete {failure}")),
            ),
            false,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each method's removal in the dry-run plan.
    #[test]
    fn the_plan_follows_the_install_method() {
        #[cfg(unix)]
        let (dir, exe) = (
            PathBuf::from("/Users/a/.local/bin"),
            "/Users/a/.local/bin/devlanes",
        );
        #[cfg(windows)]
        let (dir, exe) = (
            PathBuf::from(r"C:\Users\a\AppData\Local\Programs\devlanes"),
            r"C:\Users\a\AppData\Local\Programs\devlanes\devlanes.exe",
        );
        let exe = Path::new(exe);
        let plan = |method: InstallMethod, files: Vec<PathBuf>, path_entry: bool| {
            binary_of(&method, exe, files, path_entry).plan()
        };

        let files: Vec<PathBuf> = install::BINARIES
            .iter()
            .map(|name| dir.join(name))
            .collect();
        let script = plan(
            InstallMethod::Script(dir.clone()),
            files.clone(),
            cfg!(windows),
        );
        assert_eq!(script.state, State::Todo);
        assert_eq!(script.detail[0], format!("delete {}", joined(&files)));
        #[cfg(windows)]
        assert_eq!(
            script.detail[1],
            format!("remove {} from the user PATH", dir.display())
        );
        #[cfg(unix)]
        assert_eq!(script.detail.len(), 1, "{script:?}");
        let empty = plan(InstallMethod::Script(dir.clone()), Vec::new(), false);
        assert_eq!(empty.state, State::Skip);

        // Windows names the `.cmd` and `.exe` files; `PATH` finds them by full name.
        let npm = plan(InstallMethod::Npm, Vec::new(), false);
        assert_eq!(npm.state, State::Todo);
        assert!(
            npm.detail[0].starts_with("run npm")
                && npm.detail[0].ends_with(" uninstall -g devlanes"),
            "{npm:?}"
        );
        let brew = plan(InstallMethod::Homebrew, Vec::new(), false);
        assert!(
            brew.detail[0].ends_with(" uninstall devlanes-run/tap/devlanes"),
            "{brew:?}"
        );
        #[cfg(unix)]
        assert_eq!(
            plan(InstallMethod::CargoInstall, Vec::new(), false).detail,
            ["run cargo uninstall devlanes devlanes-hosts-helper"]
        );
        #[cfg(windows)]
        assert_eq!(
            plan(InstallMethod::CargoInstall, Vec::new(), false).detail,
            ["run cargo.exe uninstall devlanes"]
        );

        for method in [InstallMethod::Uvx, InstallMethod::PipxRun] {
            let label = method.label();
            let cached = plan(method, Vec::new(), false);
            assert_eq!(cached.state, State::Skip);
            assert_eq!(
                cached.detail,
                [format!(
                    "{label} runs devlanes from its cache; nothing is installed to remove"
                )]
            );
        }
        let local = plan(
            InstallMethod::Local {
                manifest: PathBuf::from("acme/package.json"),
            },
            Vec::new(),
            false,
        );
        assert_eq!(local.state, State::Manual);
        assert_eq!(
            local.detail,
            [format!(
                "remove devlanes from {}",
                Path::new("acme/package.json").display()
            )]
        );
        let source = plan(InstallMethod::Source, Vec::new(), false);
        assert_eq!(source.state, State::Skip);
        assert!(source.detail[0].starts_with("a source build stays"));
    }

    /// The data step asks only where someone can answer; flags decide otherwise.
    #[test]
    fn data_is_kept_unless_asked_or_told() {
        use DataChoice::{Ask, Keep, Remove};
        assert_eq!(DataAction::of(Ask, true), DataAction::Ask);
        assert_eq!(DataAction::of(Ask, false), DataAction::Keep);
        assert_eq!(DataAction::of(Remove, false), DataAction::Delete);
        assert_eq!(DataAction::of(Keep, true), DataAction::Keep);

        let items = [DataItem {
            path: PathBuf::from("home"),
            bytes: 2048,
            kind: DataKind::Home,
        }];
        let line = format!("{}  2.0 KiB", Path::new("home").display());
        let ask = plan_data(&items, DataAction::Ask);
        assert_eq!(ask.state, State::Kept);
        assert_eq!(
            ask.detail,
            [
                "ask before deleting (no by default):".to_owned(),
                line.clone()
            ]
        );
        let delete = plan_data(&items, DataAction::Delete);
        assert_eq!(delete.state, State::Todo);
        assert_eq!(delete.detail, ["delete:".to_owned(), line]);
        assert_eq!(plan_data(&[], DataAction::Delete).state, State::Skip);
    }

    /// The home, lane data outside it and its size; a lane data root inside
    /// the home is part of the home.
    #[test]
    fn data_lists_the_home_and_lane_data_outside_it() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(temp.path().join("home")).expect("paths");
        std::fs::create_dir_all(paths.home.join("data/acme/1")).expect("home");
        std::fs::write(paths.home.join("config.yaml"), "x".repeat(10)).expect("config");
        let items = data_items(&paths, &paths.home.join("data"));
        assert_eq!(
            items,
            [DataItem {
                path: paths.home.clone(),
                bytes: 10,
                kind: DataKind::Home
            }]
        );

        let outside = temp.path().join("fast");
        std::fs::create_dir_all(outside.join("acme/1")).expect("lane folder");
        std::fs::write(outside.join("acme/1/db"), "y".repeat(5)).expect("lane file");
        std::fs::write(outside.join("notes.txt"), "not ours").expect("other file");
        let items = data_items(&paths, &outside);
        assert_eq!(
            items[1],
            DataItem {
                path: outside.clone(),
                bytes: 5,
                kind: DataKind::LaneData
            }
        );

        // Deleting keeps what devlanes did not make.
        let mut after = AfterExit::default();
        assert!(delete_data(&items, &mut after).is_empty());
        assert!(!paths.home.exists());
        assert!(!outside.join("acme").exists());
        assert!(outside.join("notes.txt").is_file());
        assert!(data_items(&paths, &outside).is_empty());
    }

    #[test]
    fn a_home_that_holds_the_user_home_is_never_deleted() {
        let user = Path::new("/home/acme");
        assert!(holds_the_user_home(Path::new("/home/acme"), Some(user)));
        assert!(holds_the_user_home(Path::new("/home"), Some(user)));
        assert!(holds_the_user_home(Path::new("/"), None));
        assert!(!holds_the_user_home(
            Path::new("/home/acme/.devlanes"),
            Some(user)
        ));
    }

    /// The entry install.ps1 added goes; the rest stays byte for byte.
    #[test]
    fn the_user_path_loses_only_the_install_entry() {
        let dir = Path::new(r"C:\Users\a\AppData\Local\Programs\devlanes");
        let lookup =
            |name: &str| (name == "LOCALAPPDATA").then(|| r"C:\Users\a\AppData\Local".to_owned());
        let expand = |entry: &str| expand_variables(entry, lookup);
        assert_eq!(
            without_path_entry(
                r"C:\Users\a\AppData\Local\Programs\devlanes;%USERPROFILE%\bin;;C:\tools",
                dir,
                expand
            )
            .as_deref(),
            Some(r"%USERPROFILE%\bin;;C:\tools")
        );
        assert_eq!(
            without_path_entry(
                r#"C:\tools;"c:\users\A\appdata\local\programs\DEVLANES\";%LOCALAPPDATA%\Programs\devlanes"#,
                dir,
                expand
            )
            .as_deref(),
            Some(r"C:\tools")
        );
        assert_eq!(
            without_path_entry(r"C:\Users\a\AppData\Local\Programs\devlanes", dir, expand)
                .as_deref(),
            Some("")
        );
        assert_eq!(
            without_path_entry(r"C:\tools;C:\devlanes-other", dir, expand),
            None
        );
        assert_eq!(without_path_entry("", dir, expand), None);
        assert_eq!(expand_variables("%NOPE%\\x;100%", lookup), "%NOPE%\\x;100%");
        assert_eq!(expand_variables("%%", lookup), "%%");
    }

    /// The detached cleanup deletes the files, then the folders, and refuses
    /// paths cmd.exe would read differently.
    #[test]
    fn the_cleanup_after_exit_waits_for_the_files() {
        let after = AfterExit {
            files: vec![PathBuf::from(r"C:\Programs\devlanes\devlanes.exe")],
            trees: vec![PathBuf::from(r"C:\Users\a\.devlanes")],
            dirs: vec![PathBuf::from(r"C:\Programs\devlanes")],
        };
        assert_eq!(
            after_exit_script(&after).as_deref(),
            Some(
                r#"for /l %i in (1,1,120) do @(del /f /q "C:\Programs\devlanes\devlanes.exe" >nul 2>&1 & ping -n 2 127.0.0.1 >nul & if not exist "C:\Programs\devlanes\devlanes.exe" (rmdir /s /q "C:\Users\a\.devlanes" >nul 2>&1 & rmdir "C:\Programs\devlanes" >nul 2>&1 & exit 0))"#
            )
        );
        assert_eq!(after_exit_script(&AfterExit::default()), None);
        let odd = AfterExit {
            files: vec![PathBuf::from(r"C:\100%\devlanes.exe")],
            ..AfterExit::default()
        };
        assert_eq!(after_exit_script(&odd), None);
    }

    /// The whole uninstall asks once, with or without processes running, and
    /// only `--yes` goes on without a terminal to ask on.
    #[tokio::test]
    async fn uninstalling_asks_once_whatever_runs() {
        use devlanes_cli::clap::Parser;
        assert_eq!(question(0), "Uninstall devlanes?");
        assert_eq!(question(1), "Uninstall devlanes and stop 1 process?");
        assert_eq!(question(3), "Uninstall devlanes and stop 3 processes?");
        let cli = devlanes_cli::Cli::parse_from(["devlanes", "--output", "plain"]);
        let mut runtime =
            Runtime::resolve(&cli, devlanes_cli::Signals::default()).expect("runtime");
        runtime.stdin_tty = false;
        for processes in [0, 3] {
            let error = confirmed(&runtime, false, processes)
                .await
                .expect_err("no terminal");
            assert_eq!(error.code, "confirmation_required");
            assert_eq!(error.data["processes"], processes);
            assert!(confirmed(&runtime, true, processes).await.expect("--yes"));
        }
    }

    #[test]
    fn step_lines_align_their_details() {
        let step = Step::new(
            "setup",
            State::Todo,
            ["hosts: remove x".to_owned(), "ca: untrust y".to_owned()],
        );
        assert_eq!(
            lines(&step),
            "todo    setup    hosts: remove x\n                 ca: untrust y\n"
        );
    }
}
