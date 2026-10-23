//! `devlanes __agent-hook --shell powershell` as the agent plugin runs it: the
//! tool call as JSON on stdin, a JSON deny on stdout and exit 0 to block. The
//! plugin's command must also exit 0 with no output in both shells Claude Code
//! runs it with on Windows (Git Bash, else PowerShell) when devlanes is missing
//! or too old to know `__agent-hook`.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Exit status, stdout and stderr of a finished process.
type Outcome = (Option<i32>, String, String);

/// Runs `command` with `payload` on stdin. The child is killed if it outlives
/// the deadline.
fn run_with_stdin(mut command: Command, payload: &str) -> Outcome {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("start the hook");
    let mut stdin = child.stdin.take().expect("stdin");
    // Without a devlanes that reads it, the shell may exit before the write.
    match stdin.write_all(payload.as_bytes()) {
        Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => {
            panic!("write payload: {error}")
        }
        _ => {}
    }
    drop(stdin);
    let mut stdout = child.stdout.take().expect("stdout");
    let mut stderr = child.stderr.take().expect("stderr");
    // Read on threads so a full pipe cannot stall the child.
    let out = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let err = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the hook did not exit within 60 s");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    (
        status.code(),
        out.join().expect("stdout reader"),
        err.join().expect("stderr reader"),
    )
}

/// A PowerShell tool call of `command` in `cwd`.
fn payload(cwd: &Path, command: &str) -> String {
    serde_json::json!({
        "session_id": "s",
        "cwd": cwd,
        "hook_event_name": "PreToolUse",
        "tool_name": "PowerShell",
        "tool_input": {"command": command, "description": "x", "run_in_background": false},
    })
    .to_string()
}

/// Asserts that `stdout` is the PreToolUse deny with the devlanes message.
fn assert_denied(outcome: &Outcome, context: &str) {
    let (code, stdout, stderr) = outcome;
    assert_eq!(*code, Some(0), "{context}: {stderr}");
    let answer: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|error| panic!("{context}: not JSON ({error}): {stdout}"));
    let output = &answer["hookSpecificOutput"];
    assert_eq!(output["hookEventName"], "PreToolUse", "{context}");
    assert_eq!(output["permissionDecision"], "deny", "{context}");
    assert!(
        output["permissionDecisionReason"]
            .as_str()
            .is_some_and(|reason| reason.contains("devlanes exec --name NAME")),
        "{context}: {stdout}"
    );
}

/// Asserts that the hook let the call through: exit 0 and nothing on stdout.
fn assert_allowed(outcome: &Outcome, context: &str) {
    let (code, stdout, stderr) = outcome;
    assert_eq!(*code, Some(0), "{context}: {stderr}");
    assert_eq!(stdout.trim(), "", "{context}");
}

/// A temp directory holding a devlanes project.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("devlanes.yaml"), "project: acme\n").expect("config");
    dir
}

#[test]
fn agent_hook_denies_through_the_binary() {
    let dir = project();
    let home = dir.path().join("home");
    let hook = |payload: &str, args: &[&str], opt_out: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_devlanes"));
        command
            .arg("__agent-hook")
            .args(args)
            .env("DEVLANES_HOME", &home)
            .env_remove("DEVLANES_BLOCK_BACKGROUND");
        if opt_out {
            command.env("DEVLANES_BLOCK_BACKGROUND", "0");
        }
        run_with_stdin(command, payload)
    };
    let powershell = ["--shell", "powershell"];
    let blocked = payload(dir.path(), "Start-Process npm -ArgumentList 'run','dev'");
    assert_denied(&hook(&blocked, &powershell, false), "Start-Process");
    assert_allowed(&hook(&blocked, &powershell, true), "turned off");
    assert_allowed(&hook(&blocked, &[], false), "no shell");
    assert_allowed(&hook(&blocked, &["--shell", "fish"], false), "other shell");
    let allowed = payload(dir.path(), "npm run dev");
    assert_allowed(&hook(&allowed, &powershell, false), "foreground");
    assert_allowed(&hook("not json", &powershell, false), "bad payload");
    assert!(!home.exists(), "the hook must not create a devlanes home");
}

/// The `command` of the plugin's PowerShell hook in `agent-plugin/hooks/hooks.json`.
fn plugin_command() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../agent-plugin/hooks/hooks.json");
    let hooks: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("hooks.json"))
            .expect("hooks.json is JSON");
    let group = hooks["hooks"]["PreToolUse"]
        .as_array()
        .expect("PreToolUse hooks")
        .iter()
        .find(|group| group["matcher"] == "PowerShell")
        .expect("a PowerShell hook");
    group["hooks"][0]["command"]
        .as_str()
        .expect("a shell-form command")
        .to_owned()
}

/// Git for Windows' `sh.exe`, three levels above `git --exec-path`.
fn git_sh() -> PathBuf {
    let mut git = Command::new("git");
    git.arg("--exec-path");
    let (code, stdout, stderr) = run_with_stdin(git, "");
    assert_eq!(code, Some(0), "git --exec-path: {stderr}");
    let root = Path::new(stdout.trim())
        .ancestors()
        .nth(3)
        .expect("Git's root")
        .to_path_buf();
    root.join(r"usr\bin\sh.exe")
}

#[test]
fn plugin_command_passes_with_a_missing_or_old_devlanes() {
    let dir = project();
    let command = plugin_command();
    let system32 = Path::new(&std::env::var_os("SystemRoot").expect("SystemRoot")).join("System32");
    let powershell = system32.join(r"WindowsPowerShell\v1.0\powershell.exe");
    let sh = git_sh();
    // `old` answers `__agent-hook` the way a devlanes without it does: a usage
    // error on stderr and exit 2. `empty` has no devlanes at all.
    let old = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        old.path().join("devlanes.cmd"),
        "@echo error: unrecognized subcommand 1>&2\r\n@exit /b 2\r\n",
    )
    .expect("old devlanes.cmd");
    std::fs::write(
        old.path().join("devlanes"),
        "#!/bin/sh\necho 'error: unrecognized subcommand' >&2\nexit 2\n",
    )
    .expect("old devlanes");
    let empty = tempfile::tempdir().expect("temp dir");
    let current = Path::new(env!("CARGO_BIN_EXE_devlanes"))
        .parent()
        .expect("target dir");
    let blocked = payload(dir.path(), "pnpm dev &");
    let shells: [(&Path, &[&str]); 2] = [
        (&powershell, &["-NoProfile", "-NonInteractive", "-Command"]),
        (&sh, &["-c"]),
    ];
    for (shell, args) in shells {
        let run = |path_dir: &Path| {
            let mut child = Command::new(shell);
            child
                .args(args)
                .arg(&command)
                .env(
                    "PATH",
                    std::env::join_paths([path_dir, system32.as_path()]).expect("PATH"),
                )
                .env_remove("DEVLANES_BLOCK_BACKGROUND")
                .env("DEVLANES_HOME", dir.path().join("home"));
            run_with_stdin(child, &blocked)
        };
        let name = shell.file_name().expect("shell name").to_string_lossy();
        assert_denied(&run(current), &format!("{name}: current devlanes"));
        assert_allowed(&run(old.path()), &format!("{name}: old devlanes"));
        assert_allowed(&run(empty.path()), &format!("{name}: no devlanes"));
    }
}
