//! The environment of the user's login shell, for a lane that has none from a
//! shell. A host started from the desktop (Claude Desktop from the Dock or the
//! Start menu) gives `devlanes mcp` an environment without what the shell
//! profile adds, such as nvm, fnm or mise directories on `PATH`. The server
//! then runs the user's shell with its profile and has it run
//! `devlanes __shell-env MARK`, which prints the environment between two marks.
//! VS Code, JetBrains IDEs and Zed read the environment of a desktop launch the
//! same way.
//!
//! The result stays in the server process; nothing is written to disk.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{Read, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, Mutex, PoisonError, mpsc},
    time::Duration,
};

/// The hidden subcommand of the devlanes binary that prints its environment.
pub const PAYLOAD_ARG: &str = "__shell-env";

/// `0` turns reading the login shell off and `1` forces it on Unix; on
/// Windows `pwsh` or `powershell` turn it on.
pub const SWITCH_ENV: &str = "DEVLANES_SHELL_ENV";

/// How many seconds the shell may take, 1 to 120.
pub const TIMEOUT_ENV: &str = "DEVLANES_SHELL_ENV_TIMEOUT";

/// Set for the shell, so a profile can tell it runs only to be read.
pub const RESOLVING_ENV: &str = "DEVLANES_RESOLVING_ENVIRONMENT";

/// How long the shell may take without [`TIMEOUT_ENV`].
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Most bytes of shell output read; an environment is a few KiB.
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

/// How long the pipes may stay open after the shell exited: a process the
/// profile started in the background can hold them.
const PIPE_WAIT: Duration = Duration::from_millis(500);

/// How long a timed-out shell gets after the polite signal before it is killed.
const GRACE: Duration = Duration::from_secs(1);

/// Variables that keep Oh My Zsh from asking to update and its tmux plugin from
/// starting tmux, either of which would block the shell. Same guards as
/// sindresorhus/shell-env
/// (https://github.com/sindresorhus/shell-env/blob/1670f367a5442ab33fbc99831daf6ef6837e3ea3/index.js#L12-L18).
const GUARDS: [(&str, &str); 3] = [
    ("DISABLE_AUTO_UPDATE", "true"),
    ("ZSH_TMUX_AUTOSTART", "false"),
    ("ZSH_TMUX_AUTOSTARTED", "true"),
];

/// Variables that describe the shell process that printed the environment
/// rather than the user's setup, so the server's values stay. JetBrains keeps
/// `_`, PWD and SHLVL from the IDE
/// (https://github.com/JetBrains/intellij-community/blob/e385653301f50a247e832f8f4adb4bf255d9c992/platform/util/src/com/intellij/util/EnvironmentUtil.java#L132-L152),
/// and VS Code drops XDG_RUNTIME_DIR
/// (https://github.com/microsoft/vscode/blob/6ed5cb783e62e1dc65aa5c56b42265bb30ceb610/src/vs/platform/shell/node/shellEnv.ts#L197-L212).
const RESTORED: [&str; 5] = ["_", "PWD", "OLDPWD", "SHLVL", "XDG_RUNTIME_DIR"];

/// Variables an Electron host may have set for itself, which would make an
/// Electron program in the lane run as Node or detach from its console.
const DROPPED: [&str; 2] = ["ELECTRON_RUN_AS_NODE", "ELECTRON_NO_ATTACH_CONSOLE"];

/// The environment a login shell printed, cleaned for a lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellEnv {
    /// The shell's name, such as `zsh` or `pwsh`.
    pub shell: String,
    pub env: BTreeMap<String, String>,
}

/// Why the login shell's environment is not available. Each message is the
/// reason an agent reads after "check why the login shell failed:".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShellEnvError {
    #[error("skipped, DEVLANES_SHELL_ENV is 0")]
    Disabled,
    /// `SHLVL` says a shell started the server, so its environment already
    /// went through the profile.
    #[error(
        "skipped, the MCP server was started from a shell (SHLVL={0}); set DEVLANES_SHELL_ENV=1 \
         to read it anyway"
    )]
    FromShell(String),
    /// Windows without `DEVLANES_SHELL_ENV=pwsh` or `=powershell`.
    #[error("off on Windows unless DEVLANES_SHELL_ENV is pwsh or powershell")]
    Off,
    #[error("{0} is not on PATH")]
    NotFound(String),
    #[error("the devlanes path {0} cannot be quoted for a shell")]
    Unquotable(String),
    #[error("{shell} could not start: {message}")]
    Spawn { shell: String, message: String },
    #[error(
        "{shell} did not finish within {seconds} s; raise DEVLANES_SHELL_ENV_TIMEOUT or check \
         the profile"
    )]
    Timeout { shell: String, seconds: u64 },
    #[error("{shell} printed over 4 MiB")]
    TooLarge { shell: String },
    /// The profile's own output stays out of the message, which reaches the
    /// agent: a profile can print secrets.
    #[error("{shell} ended ({status}) without printing the environment")]
    NoOutput { shell: String, status: String },
    #[error("{shell} printed an environment that is not a JSON object of strings: {message}")]
    Invalid { shell: String, message: String },
}

/// Reads the login shell's environment for `exe`, the devlanes binary, from
/// a server whose environment is `server_env`. Blocks for as long as the shell
/// takes, at most the timeout.
pub type Resolver = fn(&Path, &BTreeMap<String, String>) -> Result<ShellEnv, ShellEnvError>;

/// What `devlanes __shell-env MARK` prints: `MARK`, this process's
/// environment as one JSON object, `MARK`. Names and values that are not
/// UTF-8 are converted lossily, with U+FFFD for the bytes that are not.
pub fn payload(mark: &str) -> String {
    let env: BTreeMap<String, String> = std::env::vars_os()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect();
    let json = serde_json::to_string(&env).unwrap_or_else(|_| "{}".to_owned());
    format!("{mark}{json}{mark}")
}

/// `devlanes __shell-env MARK`: prints [`payload`] for the mark that `args`
/// (the arguments after `__shell-env`) start with.
pub fn print_payload(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    let Some(mark) = args.next() else {
        eprintln!("usage: devlanes {PAYLOAD_ARG} MARK");
        return ExitCode::from(2);
    };
    let mut stdout = std::io::stdout().lock();
    let printed = stdout
        .write_all(payload(&mark.to_string_lossy()).as_bytes())
        .and_then(|()| stdout.flush());
    match printed {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

/// Runs the user's shell with its profile and reads the environment it
/// prints, cleaned for a lane. `server_env` is the environment of
/// `devlanes mcp`, which the shell starts from; `exe` prints the environment.
/// Blocking: call it off the async executor.
pub fn resolve(
    exe: &Path,
    server_env: &BTreeMap<String, String>,
) -> Result<ShellEnv, ShellEnvError> {
    let mark = new_mark();
    let invocation = invocation(exe, server_env, &mark)?;
    let shell = shell_name(&invocation.program);
    let limit = timeout(server_env);
    let captured = run(
        &invocation,
        &child_env(server_env, cfg!(windows)),
        home(server_env),
        limit,
    )
    .map_err(|failure| match failure {
        Failure::Spawn(error) => ShellEnvError::Spawn {
            shell: shell.clone(),
            message: error.to_string(),
        },
        Failure::Timeout => ShellEnvError::Timeout {
            shell: shell.clone(),
            seconds: limit.as_secs(),
        },
    })?;
    // Only the size: a profile can print secrets, and the log may be shared.
    if !captured.stderr.is_empty() {
        tracing::debug!(%shell, bytes = captured.stderr.len(), "login shell wrote to stderr");
    }
    if captured.stdout.len() > OUTPUT_LIMIT {
        return Err(ShellEnvError::TooLarge { shell });
    }
    // Same as Zed's parse_env_output: a profile command that fails makes the
    // shell exit non-zero after the payload printed a good environment
    // (https://github.com/zed-industries/zed/blob/4a6121bb0f10c6e8037b741b7dc0a4cd639ae289/crates/util/src/shell_env.rs#L41-L72).
    match parse(&String::from_utf8_lossy(&captured.stdout), &mark) {
        Some(Ok(env)) => {
            if !captured.success {
                tracing::debug!(%shell, status = %captured.status, "login shell failed after printing its environment");
            }
            Ok(ShellEnv {
                shell,
                env: clean(env, server_env, cfg!(windows)),
            })
        }
        Some(Err(message)) => Err(ShellEnvError::Invalid { shell, message }),
        None => Err(ShellEnvError::NoOutput {
            shell,
            status: captured.status,
        }),
    }
}

/// `base` with every variable of `overlay` set over it. On Windows names
/// match without case, so a shell's `PATH` replaces the server's `Path`.
pub(crate) fn merge(
    base: &BTreeMap<String, String>,
    overlay: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    merge_with(base, overlay, cfg!(windows))
}

/// The login shell's environment, read on first use and kept, failures
/// included, for the life of the server. Same as VS Code's one cached promise
/// per process, which also keeps a failure
/// (https://github.com/microsoft/vscode/blob/6ed5cb783e62e1dc65aa5c56b42265bb30ceb610/src/vs/platform/shell/node/shellEnv.ts#L63-L67).
#[derive(Debug, Clone)]
pub(crate) struct Cache {
    cell: Arc<std::sync::OnceLock<Result<ShellEnv, ShellEnvError>>>,
    /// Set once the read started, so only the first caller starts it.
    started: Arc<std::sync::atomic::AtomicBool>,
    /// Wakes the callers waiting for the read.
    ready: Arc<tokio::sync::Notify>,
    resolve: Resolver,
    exe: PathBuf,
    env: Arc<BTreeMap<String, String>>,
}

impl Cache {
    /// A cache that reads with `resolve` for `exe` from `env`, the server's
    /// environment, when first asked.
    pub(crate) fn new(resolve: Resolver, exe: PathBuf, env: BTreeMap<String, String>) -> Self {
        Self {
            cell: Arc::default(),
            started: Arc::default(),
            ready: Arc::default(),
            resolve,
            exe,
            env: Arc::new(env),
        }
    }

    /// The login shell's environment, read on a blocking thread the first
    /// time. The read runs in a task of its own, so a call cancelled while
    /// the shell runs leaves the result to the next call instead of starting
    /// the shell again.
    pub(crate) async fn get(&self) -> &Result<ShellEnv, ShellEnvError> {
        if !self.started.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let (cell, ready) = (self.cell.clone(), self.ready.clone());
            let (resolve, exe, env) = (self.resolve, self.exe.clone(), self.env.clone());
            tokio::spawn(async move {
                let resolved = tokio::task::spawn_blocking(move || resolve(&exe, &env))
                    .await
                    .unwrap_or_else(|error| {
                        Err(ShellEnvError::Spawn {
                            shell: "the login shell".to_owned(),
                            message: error.to_string(),
                        })
                    });
                match &resolved {
                    Ok(shell) => tracing::debug!(
                        shell = %shell.shell,
                        variables = shell.env.len(),
                        "read the login shell's environment"
                    ),
                    Err(error) => tracing::debug!(%error, "no login shell environment"),
                }
                let _ = cell.set(resolved);
                ready.notify_waiters();
            });
        }
        loop {
            let notified = self.ready.notified();
            tokio::pin!(notified);
            // Registered before the check, so a result set in between still wakes it.
            notified.as_mut().enable();
            if let Some(resolved) = self.cell.get() {
                return resolved;
            }
            notified.await;
        }
    }
}

/// The note a call gets when its lane took the login shell's environment.
pub(crate) fn resolved_note(shell: &str) -> String {
    let source = if cfg!(windows) {
        "your PowerShell profile"
    } else {
        "your login shell"
    };
    format!(
        "note: the lane had no environment from a shell, so devlanes mcp read it from {source} \
         ({shell})"
    )
}

/// The fix for a program not found on `PATH` when the lane got the server's
/// environment because reading the login shell's failed with `error`.
pub(crate) fn not_found_hint(lane: Option<&str>, error: &ShellEnvError) -> String {
    let restart = match lane {
        Some(lane) => format!("devlanes restart --lane {lane}"),
        None => "devlanes restart".to_owned(),
    };
    match error {
        ShellEnvError::Off => format!(
            "run {restart} in a terminal once, or set DEVLANES_SHELL_ENV=pwsh in the MCP \
             server's env to read your PowerShell profile"
        ),
        error => format!(
            "run {restart} in a terminal once, or check why the login shell failed: {error}"
        ),
    }
}

/// How to start the shell: the program, its `argv[0]` when that differs, and
/// the arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Invocation {
    program: PathBuf,
    arg0: Option<String>,
    args: Vec<String>,
}

/// The shell to run on Unix, once the server's environment allows it.
#[cfg(unix)]
fn invocation(
    exe: &Path,
    env: &BTreeMap<String, String>,
    mark: &str,
) -> Result<Invocation, ShellEnvError> {
    unix_allowed(env)?;
    let shell = unix_shell(
        env,
        || std::fs::read_to_string("/etc/passwd").ok(),
        rustix::process::getuid().as_raw(),
    );
    Ok(unix_invocation(
        &shell,
        &quotable(exe, &shell_name(&shell))?,
        mark,
    ))
}

/// The PowerShell to run on Windows, when `DEVLANES_SHELL_ENV` names one.
#[cfg(windows)]
fn invocation(
    exe: &Path,
    env: &BTreeMap<String, String>,
    mark: &str,
) -> Result<Invocation, ShellEnvError> {
    let name = windows_shell(env)?;
    let shell = locate_powershell(name, env).ok_or_else(|| ShellEnvError::NotFound(name.into()))?;
    Ok(windows_invocation(&shell, &quotable(exe, name)?, mark))
}

/// Whether the server's environment lets it read the login shell on Unix:
/// `DEVLANES_SHELL_ENV=0` says no and `=1` yes; otherwise yes unless `SHLVL`
/// says a shell started the server. Same rule as JetBrains, which counts a
/// `SHLVL` that is not a number as set
/// (https://github.com/JetBrains/intellij-community/blob/e385653301f50a247e832f8f4adb4bf255d9c992/platform/platform-impl/bootstrap/src/com/intellij/platform/ide/bootstrap/startup.kt#L704-L727).
#[cfg(any(unix, test))]
fn unix_allowed(env: &BTreeMap<String, String>) -> Result<(), ShellEnvError> {
    match env.get(SWITCH_ENV).map(|value| value.trim()) {
        Some("0") => Err(ShellEnvError::Disabled),
        Some("1") => Ok(()),
        _ => match env.get("SHLVL").filter(|level| !level.trim().is_empty()) {
            Some(level) if level.trim().parse::<i64>().unwrap_or(1) > 0 => {
                Err(ShellEnvError::FromShell(level.clone()))
            }
            _ => Ok(()),
        },
    }
}

/// The user's shell: `$SHELL`, else the passwd entry of `uid`, else
/// `/bin/sh`. A shell that only refuses logins (`false`, `nologin`) is
/// skipped. Same order as VS Code's getSystemShell
/// (https://github.com/microsoft/vscode/blob/6ed5cb783e62e1dc65aa5c56b42265bb30ceb610/src/vs/base/node/shell.ts#L29-L62).
#[cfg(any(unix, test))]
fn unix_shell(
    env: &BTreeMap<String, String>,
    passwd: impl FnOnce() -> Option<String>,
    uid: u32,
) -> PathBuf {
    let usable = |shell: &str| {
        shell.starts_with('/') && !shell.ends_with("/false") && !shell.ends_with("nologin")
    };
    if let Some(shell) = env.get("SHELL").filter(|shell| usable(shell)) {
        return PathBuf::from(shell);
    }
    passwd()
        .and_then(|text| {
            passwd_shell(&text, uid)
                .filter(|shell| usable(shell))
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("/bin/sh"))
}

/// The login shell of `uid` in passwd-format `text`.
#[cfg(any(unix, test))]
fn passwd_shell(text: &str, uid: u32) -> Option<&str> {
    text.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        (fields.len() >= 7 && fields[2].parse::<u32>().ok() == Some(uid)).then(|| fields[6].trim())
    })
}

/// The arguments that make `shell` an interactive login shell running `exe`
/// with the payload. Same commands per shell as VS Code's
/// doResolveUnixShellEnv: an interactive login shell runs the profile
/// (https://github.com/microsoft/vscode/blob/6ed5cb783e62e1dc65aa5c56b42265bb30ceb610/src/vs/platform/shell/node/shellEnv.ts#L128-L151).
#[cfg(any(unix, test))]
fn unix_invocation(shell: &Path, exe: &str, mark: &str) -> Invocation {
    let run = format!("'{exe}' {PAYLOAD_ARG} {mark}");
    let name = shell_name(shell);
    let strings =
        |args: &[&str]| -> Vec<String> { args.iter().map(|arg| (*arg).to_owned()).collect() };
    let (arg0, args) = match name.as_str() {
        name if is_powershell(name) => (
            None,
            strings(&["-Login", "-NoLogo", "-Command", &format!("& {run}")]),
        ),
        // Nushell runs a quoted path as a command only after `^`.
        "nu" => (None, strings(&["-i", "-l", "-c", &format!("^{run}")])),
        // xonsh reads Python, so VS Code prints the environment in Python too.
        "xonsh" => (
            None,
            strings(&[
                "-i",
                "-l",
                "-c",
                &format!(
                    "import os, json; print(\"{mark}\" + json.dumps(dict(os.environ)) + \"{mark}\")"
                ),
            ]),
        ),
        // csh and tcsh take no -l with -c; a `-` at the start of argv[0]
        // makes them a login shell, as Zed does
        // (https://github.com/zed-industries/zed/blob/4a6121bb0f10c6e8037b741b7dc0a4cd639ae289/crates/util/src/shell_env.rs#L93-L96).
        "csh" | "tcsh" => (Some(format!("-{name}")), strings(&["-i", "-c", &run])),
        // direnv, asdf and mise hook fish's prompt event, which a -c shell
        // never fires on its own; Zed emits it
        // (https://github.com/zed-industries/zed/blob/4a6121bb0f10c6e8037b741b7dc0a4cd639ae289/crates/util/src/shell_env.rs#L97-L101).
        "fish" => (
            None,
            strings(&["-i", "-l", "-c", &format!("emit fish_prompt; {run}")]),
        ),
        _ => (None, strings(&["-i", "-l", "-c", &run])),
    };
    Invocation {
        program: shell.to_path_buf(),
        arg0,
        args,
    }
}

/// Whether `name` is PowerShell's, as VS Code matches it.
fn is_powershell(name: &str) -> bool {
    let name = name.strip_suffix("-preview").unwrap_or(name);
    name == "pwsh" || name == "powershell"
}

/// The PowerShell `DEVLANES_SHELL_ENV` names on Windows. Reading the profile
/// is off by default there, as in VS Code
/// (https://github.com/microsoft/vscode/blob/6ed5cb783e62e1dc65aa5c56b42265bb30ceb610/src/vs/platform/shell/node/shellEnv.ts#L41-L46),
/// JetBrains and Zed: Windows gives desktop apps the user's environment from
/// the registry, and few tools add to `PATH` only from a profile.
#[cfg(any(windows, test))]
fn windows_shell(env: &BTreeMap<String, String>) -> Result<&'static str, ShellEnvError> {
    let value = lookup(env, SWITCH_ENV, true).map(|(_, value)| value.trim().to_ascii_lowercase());
    match value.as_deref() {
        Some("pwsh") => Ok("pwsh"),
        Some("powershell") => Ok("powershell"),
        _ => Err(ShellEnvError::Off),
    }
}

/// The arguments that make PowerShell load the user's profile and run `exe`
/// with the payload. Without -NoProfile, which Zed passes
/// (https://github.com/zed-industries/zed/blob/4a6121bb0f10c6e8037b741b7dc0a4cd639ae289/crates/util/src/shell_env.rs#L254-L262),
/// since the profile is what this reads; without -ExecutionPolicy, so the
/// user's policy decides whether the profile runs.
#[cfg(any(windows, test))]
fn windows_invocation(shell: &Path, exe: &str, mark: &str) -> Invocation {
    Invocation {
        program: shell.to_path_buf(),
        arg0: None,
        args: [
            "-NoLogo",
            "-NonInteractive",
            "-Command",
            &format!("& '{exe}' {PAYLOAD_ARG} {mark}"),
        ]
        .map(str::to_owned)
        .to_vec(),
    }
}

/// `name`.exe on the server's `PATH`, else where its installer puts it.
#[cfg(windows)]
fn locate_powershell(name: &str, env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let file = format!("{name}.exe");
    let path = lookup(env, "PATH", true).map_or("", |(_, path)| path.as_str());
    let installed = match name {
        "powershell" => lookup(env, "SystemRoot", true).map(|(_, root)| {
            Path::new(root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
        }),
        _ => lookup(env, "ProgramFiles", true)
            .map(|(_, root)| Path::new(root).join(r"PowerShell\7\pwsh.exe")),
    };
    // Not `is_file`: the Microsoft Store's pwsh.exe is an app execution alias,
    // a reparse point that metadata cannot follow but CreateProcessW starts.
    std::env::split_paths(path)
        .map(|dir| dir.join(&file))
        .chain(installed)
        .find(|candidate| std::fs::symlink_metadata(candidate).is_ok_and(|meta| !meta.is_dir()))
}

/// `exe` as text that fits between single quotes for the shell `shell` (a
/// [`shell_name`]): no shell can escape a `'` there and a newline ends the
/// command. bash, zsh and csh expand `!` even inside them in an interactive
/// shell; PowerShell also ends the quotes at the typographic quotes ‘ ’ ‚ ‛;
/// fish reads `\\` and `\'` inside them as escapes.
fn quotable(exe: &Path, shell: &str) -> Result<String, ShellEnvError> {
    let text = exe.to_string_lossy();
    let unsafe_char = |c: char| match c {
        '\'' | '\n' | '\r' => true,
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => is_powershell(shell),
        '!' => !is_powershell(shell),
        '\\' => shell == "fish",
        _ => false,
    };
    if exe.to_str().is_none() || text.contains(unsafe_char) {
        return Err(ShellEnvError::Unquotable(text.into_owned()));
    }
    Ok(text.into_owned())
}

/// The shell's file name without `.exe`.
fn shell_name(shell: &Path) -> String {
    let mut name = shell
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| shell.display().to_string());
    let stem = name.len().checked_sub(4).filter(|&stem| {
        name.get(stem..)
            .is_some_and(|extension| extension.eq_ignore_ascii_case(".exe"))
    });
    if let Some(stem) = stem {
        name.truncate(stem);
    }
    name
}

/// A random word that marks where the payload's JSON starts and ends, like
/// VS Code's 12 hex digits of a UUID
/// (https://github.com/microsoft/vscode/blob/6ed5cb783e62e1dc65aa5c56b42265bb30ceb610/src/vs/platform/shell/node/shellEnv.ts#L109-L110).
fn new_mark() -> String {
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut hasher = std::hash::RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        hasher.write_u128(now.as_nanos());
    }
    format!("DEVLANES{:016x}", hasher.finish())
}

/// How long the shell may take: [`TIMEOUT_ENV`] seconds, clamped to 1 to 120
/// as VS Code clamps its setting
/// (https://github.com/microsoft/vscode/blob/6ed5cb783e62e1dc65aa5c56b42265bb30ceb610/src/vs/platform/shell/node/shellEnv.ts#L70-L74).
fn timeout(env: &BTreeMap<String, String>) -> Duration {
    lookup(env, TIMEOUT_ENV, cfg!(windows))
        .and_then(|(_, seconds)| seconds.trim().parse::<u64>().ok())
        .map_or(DEFAULT_TIMEOUT, |seconds| {
            Duration::from_secs(seconds.clamp(1, 120))
        })
}

/// The directory the shell starts in, the user's home as in Zed, so tools that
/// hook `cd` see the directory a new terminal opens in.
fn home(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    lookup(env, name, cfg!(windows))
        .map(|(_, home)| PathBuf::from(home))
        .filter(|home| home.is_dir())
}

/// The shell's environment: the server's without the values of its own call,
/// plus the guards and [`RESOLVING_ENV`].
fn child_env(server: &BTreeMap<String, String>, windows: bool) -> BTreeMap<String, String> {
    let mut env = server.clone();
    for name in devlanes_proto::CALL_ENV {
        remove(&mut env, name, windows);
    }
    let added = GUARDS
        .into_iter()
        .chain([(RESOLVING_ENV, "1")])
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    merge_with(&env, &added, windows)
}

/// The environment between the first pair of marks in `stdout` that holds a
/// JSON object of strings; `None` when no pair of marks is there. Noise from
/// the profile before and after is skipped, and so is a mark the shell echoed
/// with the command.
fn parse(stdout: &str, mark: &str) -> Option<Result<BTreeMap<String, String>, String>> {
    let starts: Vec<usize> = stdout.match_indices(mark).map(|(at, _)| at).collect();
    let mut error = None;
    for pair in starts.windows(2) {
        let json = stdout[pair[0] + mark.len()..pair[1]].trim();
        match serde_json::from_str(json) {
            Ok(env) => return Some(Ok(env)),
            Err(parse) => {
                error.get_or_insert_with(|| parse.to_string());
            }
        }
    }
    error.map(Err)
}

/// `env` from the shell with what the resolver added taken out, the
/// variables of the shell process restored from `server`, and on Windows
/// `PATH` under that name, as Zed normalises it
/// (https://github.com/zed-industries/zed/blob/4a6121bb0f10c6e8037b741b7dc0a4cd639ae289/crates/project/src/environment.rs#L351-L357).
fn clean(
    mut env: BTreeMap<String, String>,
    server: &BTreeMap<String, String>,
    windows: bool,
) -> BTreeMap<String, String> {
    for name in RESTORED.into_iter().chain(GUARDS.map(|(name, _)| name)) {
        remove(&mut env, name, windows);
        if let Some((name, value)) = lookup(server, name, windows) {
            env.insert(name.clone(), value.clone());
        }
    }
    for name in DROPPED
        .into_iter()
        .chain(devlanes_proto::CALL_ENV)
        .chain([RESOLVING_ENV])
    {
        remove(&mut env, name, windows);
    }
    if windows {
        // The policy a host set for its own PowerShell, which the lane's
        // scripts should not inherit.
        remove(&mut env, "PSExecutionPolicyPreference", true);
        // pwsh 7 puts its own module folders first when it starts, and
        // Windows PowerShell 5.1 started from the lane would then load
        // pwsh's modules.
        remove(&mut env, "PSModulePath", true);
        if let Some((name, value)) = lookup(server, "PSModulePath", true) {
            env.insert(name.clone(), value.clone());
        }
        if let Some(path) = remove(&mut env, "PATH", true) {
            env.insert("PATH".to_owned(), path);
        }
    }
    env
}

/// [`merge`], matching names without case when `ignore_case`.
fn merge_with(
    base: &BTreeMap<String, String>,
    overlay: &BTreeMap<String, String>,
    ignore_case: bool,
) -> BTreeMap<String, String> {
    let mut merged = base.clone();
    for (name, value) in overlay {
        remove(&mut merged, name, ignore_case);
        merged.insert(name.clone(), value.clone());
    }
    merged
}

/// The variable `name` in `env`, matched without case when `ignore_case`.
fn lookup<'a>(
    env: &'a BTreeMap<String, String>,
    name: &str,
    ignore_case: bool,
) -> Option<(&'a String, &'a String)> {
    if ignore_case {
        env.iter().find(|(key, _)| key.eq_ignore_ascii_case(name))
    } else {
        env.get_key_value(name)
    }
}

/// Removes `name` from `env`, every spelling of it when `ignore_case`, and
/// returns a removed value.
fn remove(env: &mut BTreeMap<String, String>, name: &str, ignore_case: bool) -> Option<String> {
    if !ignore_case {
        return env.remove(name);
    }
    let keys: Vec<String> = env
        .keys()
        .filter(|key| key.eq_ignore_ascii_case(name))
        .cloned()
        .collect();
    let mut removed = None;
    for key in keys {
        removed = env.remove(&key).or(removed);
    }
    removed
}

/// What the shell printed and how it ended.
struct Captured {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: String,
    success: bool,
}

/// Why the shell gave no output.
enum Failure {
    Spawn(std::io::Error),
    Timeout,
}

/// Runs the shell as a session leader without a terminal, as VS Code
/// (detached) and Zed (setsid) do, so an interactive shell cannot take over
/// the terminal the server may have
/// (https://github.com/zed-industries/zed/blob/4a6121bb0f10c6e8037b741b7dc0a4cd639ae289/crates/util/src/util.rs#L430-L445).
/// At `limit` the shell's process group gets SIGTERM, and SIGKILL after
/// [`GRACE`], as JetBrains ends a shell that hangs
/// (https://github.com/JetBrains/intellij-community/blob/e385653301f50a247e832f8f4adb4bf255d9c992/platform/platform-util-io/src/com/intellij/util/ShellEnvironmentReader.java#L285-L307).
#[cfg(unix)]
fn run(
    invocation: &Invocation,
    env: &BTreeMap<String, String>,
    cwd: Option<PathBuf>,
    limit: Duration,
) -> Result<Captured, Failure> {
    use std::os::unix::process::CommandExt as _;
    use std::process::Stdio;
    let mut command = std::process::Command::new(&invocation.program);
    if let Some(arg0) = &invocation.arg0 {
        command.arg0(arg0);
    }
    command
        .args(&invocation.args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    devlanes_sys::new_session(&mut command);
    let mut child = command.spawn().map_err(Failure::Spawn)?;
    let stdout = Capture::start(child.stdout.take());
    let stderr = Capture::start(child.stderr.take());
    let deadline = std::time::Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                kill_group(&mut child);
                return Err(Failure::Timeout);
            }
            Err(error) => {
                kill_group(&mut child);
                return Err(Failure::Spawn(error));
            }
        }
    };
    Ok(Captured {
        stdout: stdout.finish(),
        stderr: stderr.finish(),
        status: status.to_string(),
        success: status.success(),
    })
}

/// SIGTERM to the process group `child` leads, SIGKILL after [`GRACE`], then
/// reaps `child`. An interactive shell ignores SIGTERM; what its profile
/// started in the group does not.
#[cfg(unix)]
fn kill_group(child: &mut std::process::Child) {
    use rustix::process::{Pid, Signal, kill_process_group};
    let group = i32::try_from(child.id()).ok().and_then(Pid::from_raw);
    if let Some(group) = group {
        let _ = kill_process_group(group, Signal::TERM);
    }
    let until = std::time::Instant::now() + GRACE;
    while std::time::Instant::now() < until && matches!(child.try_wait(), Ok(None)) {
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Some(group) = group {
        let _ = kill_process_group(group, Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Runs PowerShell in a job, so a timeout ends it and everything its profile
/// started, sharing the server's console when it has one so no window opens.
#[cfg(windows)]
fn run(
    invocation: &Invocation,
    env: &BTreeMap<String, String>,
    cwd: Option<PathBuf>,
    limit: Duration,
) -> Result<Captured, Failure> {
    use std::os::windows::io::AsHandle as _;

    use devlanes_sys_windows::{
        console,
        job::{Job, JobOptions},
        spawn::{self, CreationFlags, Environment, Spawn, Stdio, Stream},
    };
    let line = std::iter::once(invocation.program.to_string_lossy().into_owned())
        .chain(invocation.args.iter().cloned())
        .map(|arg| spawn::quote_arg(&arg))
        .collect::<Vec<_>>()
        .join(" ");
    let environment = Environment::new(env.iter().filter(|(name, _)| !name.starts_with('=')))
        .map_err(|error| {
            Failure::Spawn(std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
        })?;
    let job = Job::create(JobOptions {
        kill_on_close: false,
        die_on_unhandled_exception: true,
    })
    .map_err(Failure::Spawn)?;
    let (out_read, out_write) = spawn::pipe().map_err(Failure::Spawn)?;
    let (err_read, err_write) = spawn::pipe().map_err(Failure::Spawn)?;
    let child = spawn::spawn(&Spawn {
        program: &invocation.program,
        command_line: std::ffi::OsStr::new(&line),
        environment: Some(&environment),
        cwd: cwd.as_deref(),
        stdio: Stdio::Handles {
            stdin: Stream::Null,
            stdout: Stream::Handle(out_write.as_handle()),
            stderr: Stream::Handle(err_write.as_handle()),
        },
        inherit: &[],
        job: Some(&job),
        flags: CreationFlags {
            no_window: !console::has_console(),
            ..CreationFlags::default()
        },
    })
    .map_err(Failure::Spawn)?;
    // Only the shell holds the writing ends now, so its exit ends the reads.
    drop((out_write, err_write));
    let stdout = Capture::start(Some(out_read));
    let stderr = Capture::start(Some(err_read));
    let code = match child.wait_timeout(limit) {
        Ok(Some(code)) => code,
        Ok(None) => {
            let _ = job.terminate(1);
            let _ = child.wait_timeout(GRACE);
            return Err(Failure::Timeout);
        }
        Err(error) => {
            let _ = job.terminate(1);
            return Err(Failure::Spawn(error));
        }
    };
    Ok(Captured {
        stdout: stdout.finish(),
        stderr: stderr.finish(),
        status: format!("exit code {code}"),
        success: code == 0,
    })
}

/// A pipe read on a thread of its own up to just over [`OUTPUT_LIMIT`],
/// whose bytes so far stay available when a process the profile started
/// keeps the pipe open. After a timeout the shell's process group (its job on
/// Windows) is killed, which closes the pipe. A process the profile left
/// running after a shell that exited keeps the thread blocked until it exits
/// or closes the pipe; the result is cached, so that is at most two threads
/// for the life of the server.
struct Capture {
    bytes: Arc<Mutex<Vec<u8>>>,
    done: mpsc::Receiver<()>,
}

impl Capture {
    /// Starts reading `pipe`.
    fn start(pipe: Option<impl Read + Send + 'static>) -> Self {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (sender, done) = mpsc::channel();
        if let Some(mut pipe) = pipe {
            let bytes = bytes.clone();
            std::thread::spawn(move || {
                let mut chunk = vec![0; 8192];
                while let Ok(read @ 1..) = pipe.read(&mut chunk) {
                    let mut bytes = bytes.lock().unwrap_or_else(PoisonError::into_inner);
                    bytes.extend_from_slice(&chunk[..read]);
                    if bytes.len() > OUTPUT_LIMIT {
                        break;
                    }
                }
                let _ = sender.send(());
            });
        }
        Self { bytes, done }
    }

    /// What the pipe delivered by its end, or by [`PIPE_WAIT`] from now.
    fn finish(self) -> Vec<u8> {
        let _ = self.done.recv_timeout(PIPE_WAIT);
        std::mem::take(&mut *self.bytes.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map of these pairs.
    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    /// The JSON between the marks, with the profile's noise around it.
    #[test]
    fn parses_between_the_marks() {
        let mark = "DEVLANESabc";
        let printed = format!("Welcome!\n{mark}{{\"A\":\"1\",\"PATH\":\"/x\"}}{mark}\nbye\n");
        assert_eq!(
            parse(&printed, mark),
            Some(Ok(env(&[("A", "1"), ("PATH", "/x")])))
        );
        // xonsh prints with spaces around the JSON.
        assert_eq!(
            parse(&format!("{mark} {{\"A\":\"1\"}} {mark}\n"), mark),
            Some(Ok(env(&[("A", "1")])))
        );
    }

    /// A mark the shell echoed with the command does not hide the payload.
    #[test]
    fn skips_an_echoed_mark() {
        let mark = "DEVLANESabc";
        let printed = format!("+ 'devlanes' __shell-env {mark}\n{mark}{{\"A\":\"1\"}}{mark}");
        assert_eq!(parse(&printed, mark), Some(Ok(env(&[("A", "1")]))));
    }

    /// No pair of marks is no environment; a pair around something else is
    /// an error that says why.
    #[test]
    fn missing_marks_and_bad_json() {
        let mark = "DEVLANESabc";
        assert_eq!(parse("", mark), None);
        assert_eq!(parse(&format!("{mark}{{\"A\":\"1\"}}"), mark), None);
        assert!(matches!(
            parse(&format!("{mark}not json{mark}"), mark),
            Some(Err(_))
        ));
        assert!(matches!(
            parse(&format!("{mark}{{\"A\":1}}{mark}"), mark),
            Some(Err(_))
        ));
    }

    /// The payload round-trips through the parser.
    #[test]
    fn payload_parses() {
        let mark = new_mark();
        let parsed = parse(&payload(&mark), &mark);
        assert!(matches!(parsed, Some(Ok(env)) if !env.is_empty()));
    }

    /// Each shell gets the flags that make it a login shell that runs the
    /// profile, with the payload quoted.
    #[test]
    fn arguments_per_shell() {
        let exe = "/opt/acme/devlanes";
        let run = format!("'{exe}' __shell-env M");
        let args = |shell: &str| {
            let invocation = unix_invocation(Path::new(shell), exe, "M");
            (invocation.arg0, invocation.args)
        };
        let login = |command: &str| {
            (
                None,
                vec!["-i".into(), "-l".into(), "-c".into(), command.to_owned()],
            )
        };
        for shell in [
            "/bin/sh",
            "/bin/bash",
            "/bin/zsh",
            "/bin/dash",
            "/bin/ksh",
            "/opt/acme/oddsh",
        ] {
            assert_eq!(args(shell), login(&run), "{shell}");
        }
        assert_eq!(
            args("/usr/bin/fish"),
            login(&format!("emit fish_prompt; {run}"))
        );
        assert_eq!(args("/usr/bin/nu"), login(&format!("^{run}")));
        let (arg0, xonsh) = args("/usr/bin/xonsh");
        assert_eq!(arg0, None);
        assert_eq!(xonsh[..3], ["-i", "-l", "-c"]);
        assert!(
            xonsh[3].starts_with("import os, json; print(\"M\""),
            "{}",
            xonsh[3]
        );
        assert_eq!(
            args("/bin/tcsh"),
            (
                Some("-tcsh".into()),
                vec!["-i".into(), "-c".into(), run.clone()]
            )
        );
        assert_eq!(
            args("/bin/csh"),
            (
                Some("-csh".into()),
                vec!["-i".into(), "-c".into(), run.clone()]
            )
        );
        for shell in [
            "/usr/local/bin/pwsh",
            "/usr/local/bin/pwsh-preview",
            "/usr/bin/powershell",
        ] {
            assert_eq!(
                args(shell),
                (
                    None,
                    vec![
                        "-Login".into(),
                        "-NoLogo".into(),
                        "-Command".into(),
                        format!("& {run}")
                    ]
                ),
                "{shell}"
            );
        }
        let windows = windows_invocation(
            Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            r"C:\acme\devlanes.exe",
            "M",
        );
        assert_eq!(
            windows.args,
            [
                "-NoLogo",
                "-NonInteractive",
                "-Command",
                r"& 'C:\acme\devlanes.exe' __shell-env M"
            ]
        );
    }

    /// Paths a single-quoted word cannot hold are refused.
    #[test]
    fn refuses_unquotable_paths() {
        let refused = |path: &str, shell: &str| {
            matches!(
                quotable(Path::new(path), shell),
                Err(ShellEnvError::Unquotable(_))
            )
        };
        assert_eq!(
            quotable(Path::new("/opt/acme/devlanes"), "bash").as_deref(),
            Ok("/opt/acme/devlanes")
        );
        for path in [
            "/opt/it's/devlanes",
            "/opt/acme!/devlanes",
            "/opt/a\ncme/devlanes",
        ] {
            assert!(refused(path, "bash"), "{path}");
        }
        // PowerShell does not expand `!`, but ends a quote at a typographic one.
        assert!(quotable(Path::new(r"C:\acme!\devlanes.exe"), "pwsh").is_ok());
        assert!(quotable(Path::new(r"C:\acme\devlanes.exe"), "powershell").is_ok());
        for quote in ['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'] {
            let path = format!(r"C:\it{quote}s\devlanes.exe");
            assert!(refused(&path, "pwsh"), "{path}");
            assert!(refused(&path, "powershell"), "{path}");
        }
        assert!(quotable(Path::new("/opt/it\u{2019}s/devlanes"), "bash").is_ok());
        // fish reads `\\` and `\'` as escapes inside single quotes.
        assert!(refused(r"/opt/ac\me/devlanes", "fish"));
        assert!(quotable(Path::new(r"/opt/ac\me/devlanes"), "zsh").is_ok());
    }

    /// The shell does not get the variables of the server's own call.
    #[test]
    fn the_shell_gets_no_call_env() {
        let server = env(&[
            ("PATH", "/usr/bin"),
            (devlanes_proto::JSON_OUT_ENV, "/tmp/x.json"),
            (devlanes_proto::ENV_FROM_LANE_ENV, "1"),
            (devlanes_proto::NO_SPAWN_ENV, "1"),
        ]);
        let child = child_env(&server, false);
        for name in devlanes_proto::CALL_ENV {
            assert!(!child.contains_key(name), "{name}");
        }
        assert_eq!(child.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert_eq!(child.get(RESOLVING_ENV).map(String::as_str), Some("1"));
        assert_eq!(
            child.get("DISABLE_AUTO_UPDATE").map(String::as_str),
            Some("true")
        );
        // On Windows in any spelling.
        let server = env(&[("devlanes_json_out", "C:\\x.json")]);
        assert!(!child_env(&server, true).contains_key("devlanes_json_out"));
    }

    /// `DEVLANES_SHELL_ENV` decides first; otherwise a positive or unreadable
    /// `SHLVL` means a shell started the server.
    #[test]
    fn shlvl_and_the_switch() {
        assert_eq!(unix_allowed(&env(&[])), Ok(()));
        assert_eq!(unix_allowed(&env(&[("SHLVL", "0")])), Ok(()));
        assert_eq!(unix_allowed(&env(&[("SHLVL", "")])), Ok(()));
        assert_eq!(
            unix_allowed(&env(&[("SHLVL", "2")])),
            Err(ShellEnvError::FromShell("2".into()))
        );
        assert_eq!(
            unix_allowed(&env(&[("SHLVL", "x")])),
            Err(ShellEnvError::FromShell("x".into()))
        );
        assert_eq!(
            unix_allowed(&env(&[("SHLVL", "2"), (SWITCH_ENV, "1")])),
            Ok(())
        );
        assert_eq!(
            unix_allowed(&env(&[(SWITCH_ENV, "0")])),
            Err(ShellEnvError::Disabled)
        );
        assert_eq!(windows_shell(&env(&[])), Err(ShellEnvError::Off));
        assert_eq!(
            windows_shell(&env(&[(SWITCH_ENV, "1")])),
            Err(ShellEnvError::Off)
        );
        assert_eq!(windows_shell(&env(&[(SWITCH_ENV, "pwsh")])), Ok("pwsh"));
        assert_eq!(
            windows_shell(&env(&[("devlanes_shell_env", "PowerShell")])),
            Ok("powershell")
        );
    }

    /// `$SHELL`, else passwd, else `/bin/sh`; shells that refuse logins are skipped.
    #[test]
    fn picks_the_users_shell() {
        let passwd =
            "root:x:0:0:root:/root:/bin/bash\nacme:x:501:20:Acme:/home/acme:/usr/bin/zsh\n";
        let none = || None;
        let file = || Some(passwd.to_owned());
        assert_eq!(
            unix_shell(&env(&[("SHELL", "/bin/fish")]), none, 501),
            Path::new("/bin/fish")
        );
        assert_eq!(unix_shell(&env(&[]), file, 501), Path::new("/usr/bin/zsh"));
        assert_eq!(
            unix_shell(&env(&[("SHELL", "/bin/false")]), file, 501),
            Path::new("/usr/bin/zsh")
        );
        assert_eq!(
            unix_shell(&env(&[("SHELL", "zsh")]), file, 501),
            Path::new("/usr/bin/zsh")
        );
        let nologin = || Some("acme:x:501:20::/home/acme:/usr/sbin/nologin\n".to_owned());
        assert_eq!(unix_shell(&env(&[]), nologin, 501), Path::new("/bin/sh"));
        assert_eq!(unix_shell(&env(&[]), file, 999), Path::new("/bin/sh"));
    }

    /// The timeout is 10 s unless set, and stays within 1 to 120 s.
    #[test]
    fn timeout_is_clamped() {
        assert_eq!(timeout(&env(&[])), Duration::from_secs(10));
        assert_eq!(timeout(&env(&[(TIMEOUT_ENV, "3")])), Duration::from_secs(3));
        assert_eq!(timeout(&env(&[(TIMEOUT_ENV, "0")])), Duration::from_secs(1));
        assert_eq!(
            timeout(&env(&[(TIMEOUT_ENV, "600")])),
            Duration::from_secs(120)
        );
        assert_eq!(
            timeout(&env(&[(TIMEOUT_ENV, "soon")])),
            Duration::from_secs(10)
        );
    }

    /// The shell process's own variables come from the server or go; what the
    /// resolver and the host added goes.
    #[test]
    fn restores_and_drops() {
        let server = env(&[("PWD", "/srv"), ("SHLVL", "0"), ("_", "/usr/bin/acme")]);
        let shell = env(&[
            ("PATH", "/home/acme/.fnm:/usr/bin"),
            ("FROM_PROFILE", "1"),
            ("PWD", "/home/acme"),
            ("OLDPWD", "/"),
            ("SHLVL", "1"),
            ("_", "/opt/acme/devlanes"),
            ("XDG_RUNTIME_DIR", "/run/user/501"),
            ("DISABLE_AUTO_UPDATE", "true"),
            ("ZSH_TMUX_AUTOSTART", "false"),
            ("ZSH_TMUX_AUTOSTARTED", "true"),
            (RESOLVING_ENV, "1"),
            ("ELECTRON_RUN_AS_NODE", "1"),
            ("ELECTRON_NO_ATTACH_CONSOLE", "1"),
            (devlanes_proto::JSON_OUT_ENV, "/tmp/x.json"),
            (devlanes_proto::ENV_FROM_LANE_ENV, "1"),
            (devlanes_proto::NO_SPAWN_ENV, "1"),
        ]);
        assert_eq!(
            clean(shell, &server, false),
            env(&[
                ("PATH", "/home/acme/.fnm:/usr/bin"),
                ("FROM_PROFILE", "1"),
                ("PWD", "/srv"),
                ("SHLVL", "0"),
                ("_", "/usr/bin/acme"),
            ])
        );
    }

    /// On Windows `Path` becomes `PATH`, names match without case, the
    /// host's execution policy does not reach the lane, and PSModulePath is
    /// the server's rather than the one pwsh 7 made.
    #[test]
    fn windows_path_and_case() {
        let pwsh_modules =
            r"C:\Users\acme\Documents\PowerShell\Modules;C:\Program Files\PowerShell\7\Modules";
        let shell = env(&[
            ("Path", r"C:\fnm;C:\Windows"),
            ("PSExecutionPolicyPreference", "Bypass"),
            ("PSModulePath", pwsh_modules),
            ("devlanes_resolving_environment", "1"),
            ("FROM_PROFILE", "1"),
        ]);
        let server_modules = r"C:\WINDOWS\system32\WindowsPowerShell\v1.0\Modules";
        assert_eq!(
            clean(
                shell.clone(),
                &env(&[("PSModulePath", server_modules)]),
                true
            )
            .get("PSModulePath")
            .map(String::as_str),
            Some(server_modules)
        );
        let cleaned = clean(shell, &env(&[]), true);
        assert_eq!(
            cleaned,
            env(&[("PATH", r"C:\fnm;C:\Windows"), ("FROM_PROFILE", "1")])
        );
        let server = env(&[
            ("Path", r"C:\Windows"),
            ("AI_AGENT", "claude"),
            ("FROM_PROFILE", "0"),
        ]);
        assert_eq!(
            merge_with(&server, &cleaned, true),
            env(&[
                ("PATH", r"C:\fnm;C:\Windows"),
                ("AI_AGENT", "claude"),
                ("FROM_PROFILE", "1"),
            ])
        );
        // On Unix the two names are two variables.
        assert_eq!(merge_with(&server, &cleaned, false).len(), 4);
    }
}
