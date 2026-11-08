//! `devlanes debug bundle`: a redacted `.tar.gz` for bug reports. It reads what
//! a running daemon reports and never spawns one, so a broken setup can still be
//! described.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use devlanes_cli::{OutputMode, Rendered, Runtime};
use devlanes_client::{Client, Context, Method, Paths};
use devlanes_core::redact::Redactor;
use devlanes_proto::RpcError;
use serde_json::{Value, json};

use crate::output::emit;

/// Longest the `-- ARGS` run may take.
const RERUN_TIMEOUT: Duration = Duration::from_secs(300);

/// Collects, redacts and archives; prints the archive's path.
pub async fn run(
    runtime: &Runtime,
    to: Option<PathBuf>,
    log_lines: usize,
    args: Vec<String>,
    context: Context,
) -> Result<u8, RpcError> {
    let paths = Paths::discover().map_err(|error| io(error.to_string()))?;
    let homes = tokio::task::spawn_blocking(home_spellings)
        .await
        .unwrap_or_default();
    let redactor = Redactor::archive(std::env::vars(), &homes);
    let staging = tempfile::Builder::new()
        .prefix("devlanes-debug-")
        .tempdir()
        .map_err(|error| io(error.to_string()))?;
    let mut files: Vec<(&str, String)> = Vec::new();
    let context = Context {
        env: None,
        verbose: 0,
        ..context
    };
    let mut client = Client::connect(&paths).await;
    let mut ask = async |method: Method| -> Value {
        match &mut client {
            Ok(client) => match client.request(method, json!({}), context.clone()).await {
                Ok(data) => data,
                Err(devlanes_client::Error::Rpc(error)) => json!({"error": error}),
                Err(error) => json!({"error": {"message": error.to_string()}}),
            },
            Err(error) => {
                json!({"error": {"code": "daemon_not_connected", "message": error.to_string()}})
            }
        }
    };
    let daemon = ask(Method::DaemonStatus).await;
    let versions = json!({
        "client": {
            "version": env!("CARGO_PKG_VERSION"),
            "build_id": devlanes_version::BUILD_ID,
            "protocol": devlanes_proto::PROTOCOL_VERSION,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "daemon": daemon,
        "output": {"mode": runtime.mode.as_str(), "reason": runtime.reason},
    });
    files.push(("versions.json", pretty(&redactor, versions)));
    files.push(("doctor.json", pretty(&redactor, ask(Method::Doctor).await)));
    files.push((
        "config-check.json",
        pretty(&redactor, ask(Method::ConfigCheck).await),
    ));
    files.push(("status.json", pretty(&redactor, ask(Method::Status).await)));
    let log = tail_file(paths.logs.join("daemon.log"), log_lines).await;
    files.push(("daemon.log", redactor.text(&log)));
    if !args.is_empty() {
        files.push((
            "rerun.txt",
            redactor.text(&rerun(&args, &context.cwd).await),
        ));
    }
    files.push(("README.txt", readme(&args)));
    for (name, contents) in &files {
        let path = staging.path().join(name);
        let contents = contents.clone();
        blocking(move || std::fs::write(path, contents)).await?;
    }
    let archive = match to {
        Some(path) => absolute(&context.cwd, &path),
        None => std::env::temp_dir().join(format!(
            "devlanes-debug-{}.tar.gz",
            devlanes_core::clock::utc_stamp(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_secs())
                    .unwrap_or_default()
            )
        )),
    };
    let status = tokio::process::Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(staging.path())
        .args(files.iter().map(|(name, _)| *name))
        .status()
        .await
        .map_err(|error| io(format!("tar: {error}")))?;
    if !status.success() {
        return Err(io(format!("tar exited with {status}")));
    }
    if !runtime.quiet {
        let names: Vec<&str> = files.iter().map(|(name, _)| *name).collect();
        emit(&Rendered {
            stdout: if runtime.mode == OutputMode::Json {
                format!("{}\n", json!({"path": archive, "files": names}))
            } else {
                let shown = archive.display().to_string();
                // Clickable where the terminal shows links; `--to` may be relative.
                let linked = match (runtime.terminal.links, std::path::absolute(&archive)) {
                    (Some(links), Ok(path)) => links.file(&shown, &path, None, None),
                    _ => shown,
                };
                format!(
                    "wrote {linked}\n{}\n",
                    "secret-looking values are replaced; read the files before you send them"
                )
            },
            ..Rendered::default()
        });
    }
    Ok(0)
}

/// The paths that name the user's home: [`devlanes_core::paths::user_home`],
/// and on Windows also its 8.3 short spelling (`C:\Users\JOHNSM~1`), which
/// `TEMP` and other variables often hold, when that differs.
fn home_spellings() -> Vec<PathBuf> {
    let Some(home) = devlanes_core::paths::user_home() else {
        return Vec::new();
    };
    #[cfg(windows)]
    if let Ok(short) = devlanes_sys_windows::file::short_path(&home)
        && short != home
    {
        return vec![home, short];
    }
    vec![home]
}

/// What the archive holds, for whoever opens it.
fn readme(args: &[String]) -> String {
    let mut text = String::from(
        "devlanes debug bundle\n\n\
versions.json      client and daemon versions, output mode\n\
doctor.json        devlanes doctor --json\n\
config-check.json  devlanes config check --json\n\
status.json        devlanes status --json\n\
daemon.log         the end of the daemon's log\n",
    );
    if !args.is_empty() {
        text.push_str(&format!(
            "rerun.txt          devlanes -vv {}: exit code, stdout, stderr\n",
            args.join(" ")
        ));
    }
    text.push_str(
        "\nValues of variables named like tokens, keys or passwords, passwords in URLs,\n\
env maps and the home directory were replaced before writing.\n",
    );
    text
}

/// Runs `devlanes -vv ARGS` in `cwd` and returns its exit code and output.
async fn rerun(args: &[String], cwd: &Path) -> String {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => return format!("could not find devlanes: {error}\n"),
    };
    let mut command = tokio::process::Command::new(exe);
    command
        .arg("-vv")
        .args(args)
        .current_dir(cwd)
        // The TUI would wait for a person; this run is read, not watched.
        .env("DEVLANES_NO_UI", "1")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut text = format!("$ devlanes -vv {}\n", args.join(" "));
    match tokio::time::timeout(RERUN_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => {
            text.push_str(&format!(
                "exit: {}\n\n--- stdout ---\n{}\n--- stderr ---\n{}",
                output
                    .status
                    .code()
                    .map_or_else(|| "signal".to_owned(), |code| code.to_string()),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(Err(error)) => text.push_str(&format!("could not run: {error}\n")),
        Err(_) => text.push_str(&format!(
            "stopped after {}s without finishing\n",
            RERUN_TIMEOUT.as_secs()
        )),
    }
    text
}

/// The last `lines` lines of `path`, or a note when it cannot be read.
async fn tail_file(path: PathBuf, lines: usize) -> String {
    tokio::task::spawn_blocking(move || match std::fs::read(&path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let all: Vec<&str> = text.lines().collect();
            let mut tail = all[all.len().saturating_sub(lines)..].join("\n");
            tail.push('\n');
            tail
        }
        Err(error) => format!("{}: {error}\n", path.display()),
    })
    .await
    .unwrap_or_default()
}

/// `path` relative to `cwd` unless absolute.
fn absolute(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    }
}

/// Runs file work off the executor.
async fn blocking(
    work: impl FnOnce() -> std::io::Result<()> + Send + 'static,
) -> Result<(), RpcError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| io(error.to_string()))?
        .map_err(|error| io(error.to_string()))
}

/// An `io` error with `message`.
fn io(message: String) -> RpcError {
    RpcError::new("io", json!({"message": message}), 1)
}

/// `value` redacted for the archive, as pretty JSON. Strings are redacted
/// before serialization, where escaping would hide Windows paths.
fn pretty(redactor: &Redactor, mut value: Value) -> String {
    redactor.json(&mut value);
    let text = serde_json::to_string_pretty(&value).unwrap_or_default();
    format!("{text}\n")
}
