//! Installed copies of the `fake-tool` binary, which stand in for external programs
//! (`mise`, `direnv`, `mkcert`) where `#!/bin/sh` scripts cannot run, that
//! is on Windows.
//!
//! [`install`] copies the binary into a directory under the tool's name and writes
//! a [`Behaviour`] next to it (`mise.exe` reads `mise.exe.json`), so one directory
//! can hold several fakes that behave differently. Tests never build the binary:
//! it must exist next to the test binaries (`cargo build -p devlanes-testing`, or
//! any workspace build that includes this crate's tests).
//!
//! ```no_run
//! use devlanes_testing::fake_tool::{Behaviour, install};
//!
//! let bin = tempfile::tempdir()?;
//! let mise = install(
//!     bin.path(),
//!     "mise",
//!     &Behaviour::new()
//!         .args(["env", "--json"])
//!         .stdout_json(serde_json::json!({ "FROM_DIR": "{cwd}" })),
//! )?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::Error;

/// What an installed fake does on every run, in this order: check the arguments,
/// record them, sleep, print `stderr`, print `stdout` then `stdout_json`, copy
/// stdin to stdout (`echo_stdin`), run its last argument (`run_with`), exit.
///
/// Printed strings expand `{cwd}` to the canonical working directory (like
/// `pwd -P`; on Windows the verbatim `\\?\` form `Path::canonicalize` returns) and
/// `{env:NAME}` to a variable of the fake's environment, empty when unset. Other
/// braces stay as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Behaviour {
    /// Exact arguments expected; any others print `bad args: <args>` on stderr and
    /// exit 2 before anything else happens.
    pub args: Option<Vec<String>>,
    /// File that gets one JSON array of the arguments appended per run.
    pub record: Option<PathBuf>,
    /// Milliseconds to sleep, for timeout tests.
    pub sleep_ms: u64,
    pub stderr: String,
    pub stdout: String,
    /// Printed as compact JSON after `stdout`, with every string expanded.
    pub stdout_json: Option<serde_json::Value>,
    /// Copies stdin to stdout byte for byte until stdin ends, for a fake
    /// that stands in for a program speaking a protocol over its stdio.
    pub echo_stdin: bool,
    /// Windows only: runs the last argument the way Win32-OpenSSH's sshd runs
    /// a command with this kind of default shell, sharing the fake's streams,
    /// and exits with its status instead of `exit`. For a fake `ssh` whose
    /// box is this machine. The kinds, after sshd's own (`w32-doexec.c`):
    /// `cmd`, the raw line `"<shell>" /c "<command>"`; `powershell` (Windows
    /// PowerShell) and `bash` (Git Bash), the arguments `-c <command>`;
    /// `pwsh`, which sshd does not recognise by its path, the raw line
    /// `"<shell>" -c "<command>"`.
    pub run_with: Option<String>,
    /// The shell's executable; cmd.exe and Windows PowerShell default to the
    /// ones in System32, the others must be given.
    pub run_shell: Option<PathBuf>,
    /// Variables set for `run_with`'s shell, on top of the fake's own.
    pub run_env: Vec<(String, String)>,
    pub exit: u8,
}

impl Behaviour {
    /// A fake that accepts any arguments, prints nothing and exits 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requires exactly these arguments.
    pub fn args<I: IntoIterator<Item = S>, S: Into<String>>(mut self, args: I) -> Self {
        self.args = Some(args.into_iter().map(Into::into).collect());
        self
    }

    /// Appends the arguments of every run to `file`.
    pub fn record(mut self, file: impl Into<PathBuf>) -> Self {
        self.record = Some(file.into());
        self
    }

    /// Sleeps before printing anything.
    pub fn sleep(mut self, duration: Duration) -> Self {
        self.sleep_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        self
    }

    /// Text for stderr; add the trailing newline `echo` would print.
    pub fn stderr(mut self, text: impl Into<String>) -> Self {
        self.stderr = text.into();
        self
    }

    /// Text for stdout.
    pub fn stdout(mut self, text: impl Into<String>) -> Self {
        self.stdout = text.into();
        self
    }

    /// A JSON document for stdout; strings in it are expanded, then it is
    /// serialized, so expanded paths are escaped correctly.
    pub fn stdout_json(mut self, value: serde_json::Value) -> Self {
        self.stdout_json = Some(value);
        self
    }

    /// Copies stdin to stdout after printing.
    pub fn echo_stdin(mut self) -> Self {
        self.echo_stdin = true;
        self
    }

    /// Runs the last argument as sshd would with the `kind` of shell at
    /// `shell` (see [`Behaviour::run_shell`]), with `env` set for it.
    pub fn run_with<I, K, V>(mut self, kind: &str, shell: Option<&Path>, env: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.run_with = Some(kind.to_owned());
        self.run_shell = shell.map(Path::to_path_buf);
        self.run_env = env
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();
        self
    }

    /// The exit code.
    pub fn exit(mut self, code: u8) -> Self {
        self.exit = code;
        self
    }
}

/// Copies `fake-tool` into `dir` as `name` (plus `.exe` on Windows), writes
/// `behaviour` next to it and returns the program path.
///
/// The binary is looked up as described in [`binary`]. Blocking filesystem work.
pub fn install(dir: &Path, name: &str, behaviour: &Behaviour) -> Result<PathBuf, Error> {
    let program = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(binary()?, &program)?;
    std::fs::write(sidecar(&program), serde_json::to_vec(behaviour)?)?;
    Ok(program)
}

/// A default shell of Win32-OpenSSH's sshd, as [`Behaviour::run_with`]
/// emulates it.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshdShell {
    /// cmd.exe, sshd's default.
    Cmd,
    /// Windows PowerShell 5.1.
    WindowsPowerShell,
    /// PowerShell 7.
    Pwsh,
    /// Git for Windows' bash.
    GitBash,
}

#[cfg(windows)]
impl SshdShell {
    /// Every shell, in that order.
    pub const ALL: [Self; 4] = [
        Self::Cmd,
        Self::WindowsPowerShell,
        Self::Pwsh,
        Self::GitBash,
    ];

    /// The kind [`Behaviour::run_with`] takes.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Cmd => "cmd",
            Self::WindowsPowerShell => "powershell",
            Self::Pwsh => "pwsh",
            Self::GitBash => "bash",
        }
    }

    /// The shell's executable on this machine, `None` when it is not
    /// installed: cmd.exe and Windows PowerShell from System32, `pwsh.exe`
    /// from `PATH` (an App Installer alias counts) or `%ProgramFiles%`, and
    /// `bin\bash.exe` of the Git for Windows that `git --exec-path` names.
    /// Blocking.
    pub fn program(self) -> Option<PathBuf> {
        let system = PathBuf::from(std::env::var_os("SystemRoot")?).join("System32");
        match self {
            Self::Cmd => Some(system.join("cmd.exe")),
            Self::WindowsPowerShell => Some(system.join(r"WindowsPowerShell\v1.0\powershell.exe")),
            Self::Pwsh => {
                let on_path = std::env::var_os("PATH")
                    .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
                    .unwrap_or_default();
                let installed = std::env::var_os("ProgramFiles")
                    .map(|dir| PathBuf::from(dir).join(r"PowerShell\7"));
                on_path
                    .into_iter()
                    .chain(installed)
                    .map(|dir| dir.join("pwsh.exe"))
                    .find(|candidate| std::fs::symlink_metadata(candidate).is_ok())
            }
            Self::GitBash => {
                let output = std::process::Command::new("git")
                    .arg("--exec-path")
                    .stdin(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .output()
                    .ok()?;
                // `<root>/mingw64/libexec/git-core`.
                let exec = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
                let bash = exec.ancestors().nth(3)?.join(r"bin\bash.exe");
                bash.is_file().then_some(bash)
            }
        }
    }
}

/// The behaviour file a fake at `program` reads: its full file name plus
/// `.json` (`mise.exe.json`, `mise.old.json`), so names with dots of their own
/// keep distinct files.
pub fn sidecar(program: &Path) -> PathBuf {
    let mut name = program.as_os_str().to_owned();
    name.push(".json");
    PathBuf::from(name)
}

/// The prebuilt `fake-tool` in the target directory of the running test
/// (`<target>/<profile>/fake-tool`, next to the `deps` directory that holds test
/// binaries). Missing is an error that says how to build it; tests never run
/// cargo themselves.
pub fn binary() -> Result<PathBuf, Error> {
    let exe = std::env::current_exe()?;
    let profile_dir = exe
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| Error::FakeTool(format!("no target directory above {}", exe.display())))?;
    let program = profile_dir.join(format!("fake-tool{}", std::env::consts::EXE_SUFFIX));
    if program.is_file() {
        Ok(program)
    } else {
        Err(Error::FakeTool(format!(
            "{} is missing; build it first with `cargo build -p devlanes-testing`",
            program.display()
        )))
    }
}

/// Expands `{cwd}` and `{env:NAME}` in `text`; `env` looks variables up.
pub fn expand(text: &str, cwd: &str, env: &impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        if let Some(after) = tail.strip_prefix("{cwd}") {
            out.push_str(cwd);
            rest = after;
        } else if let Some(name) = tail.strip_prefix("{env:")
            && let Some(end) = name.find('}')
        {
            out.push_str(&env(&name[..end]).unwrap_or_default());
            rest = &name[end + 1..];
        } else {
            out.push('{');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// [`expand`] applied to every string in a JSON value, keys excluded.
pub fn expand_json(
    value: &serde_json::Value,
    cwd: &str,
    env: &impl Fn(&str) -> Option<String>,
) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::String(text) => Value::String(expand(text, cwd, env)),
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| expand_json(v, cwd, env)).collect())
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, v)| (key.clone(), expand_json(v, cwd, env)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known placeholders expand; unknown braces and unterminated ones stay.
    #[test]
    fn expands_placeholders() {
        let env = |name: &str| (name == "MARK").then(|| "seen".to_owned());
        assert_eq!(
            expand("{cwd}|{env:MARK}|{env:NONE}|{x}|{env:OPEN", "/w", &env),
            "/w|seen||{x}|{env:OPEN"
        );
        let value = serde_json::json!({"DIR": "{cwd}", "list": ["{env:MARK}"], "gone": null});
        assert_eq!(
            expand_json(&value, r"C:\w", &env),
            serde_json::json!({"DIR": r"C:\w", "list": ["seen"], "gone": null})
        );
    }

    /// Behaviours survive the sidecar round trip; missing fields take defaults.
    #[test]
    fn behaviour_round_trip() -> Result<(), serde_json::Error> {
        let behaviour = Behaviour::new()
            .args(["env", "--json"])
            .stderr("oops\n")
            .sleep(Duration::from_millis(5))
            .exit(3);
        let text = serde_json::to_string(&behaviour)?;
        assert_eq!(serde_json::from_str::<Behaviour>(&text)?, behaviour);
        assert_eq!(serde_json::from_str::<Behaviour>("{}")?, Behaviour::new());
        assert!(serde_json::from_str::<Behaviour>(r#"{"exit_code": 1}"#).is_err());
        Ok(())
    }
}
