//! Stands in for the devlanes binary in the server's tests: records each run,
//! prints what it was asked, and writes JSON where `DEVLANES_JSON_OUT` says.
//! `FAKE_*` variables in its environment pick the behaviour.

use std::{
    fs::OpenOptions,
    io::Write as _,
    process::ExitCode,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

/// The argument the fake starts its own background process with.
const GRANDCHILD: &str = "--fake-grandchild";

/// Runs one invocation.
fn main() -> ExitCode {
    // The payload a login shell runs, as `devlanes __shell-env MARK` does.
    if std::env::args().nth(1).as_deref() == Some(devlanes_mcp::shell_env::PAYLOAD_ARG) {
        return devlanes_mcp::shell_env::print_payload(std::env::args_os().skip(2));
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    // A process the command leaves running with its stdout: ticks into
    // `FAKE_GRANDCHILD_TICKS` for `FAKE_GRANDCHILD_MS` (60 s by default).
    if args.first().map(String::as_str) == Some(GRANDCHILD) {
        let ms = var("FAKE_GRANDCHILD_MS")
            .and_then(|ms| ms.parse().ok())
            .unwrap_or(60_000);
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            if let Some(ticks) = var("FAKE_GRANDCHILD_TICKS") {
                append(&ticks, "tick");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        return ExitCode::SUCCESS;
    }
    if var("FAKE_GRANDCHILD_TICKS").is_some()
        && let Ok(exe) = std::env::current_exe()
    {
        let _ = std::process::Command::new(exe)
            .arg(GRANDCHILD)
            .stdin(std::process::Stdio::null())
            .spawn();
    }
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    let cwd = std::env::current_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_default();
    if let Some(log) = var("FAKE_LOG") {
        let record = json!({
            "args": args,
            "cwd": cwd,
            "from_lane": var("DEVLANES_ENV_FROM_LANE"),
            "ai_agent": var("AI_AGENT"),
            "codex_thread_id": var("CODEX_THREAD_ID"),
            "no_spawn": var("DEVLANES_NO_SPAWN"),
            "json_out": var("DEVLANES_JSON_OUT").is_some(),
            "profile": var("FAKE_PROFILE"),
        });
        append(&log, &record.to_string());
    }
    let json_out = |value: Value| {
        if let Some(path) = var("DEVLANES_JSON_OUT") {
            append(&path, &value.to_string());
        }
    };
    // `--output json debug events -f`: the phases of a start, then waits to be killed.
    if has("events") {
        for (phase, state) in [
            ("infra", "started"),
            ("infra", "done"),
            ("services", "started"),
        ] {
            let event = json!({"event": "lane.phase", "seq": 1, "data": {"lane": "acme@1", "phase": phase, "state": state}});
            println!("{event}");
        }
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_secs(60));
        return ExitCode::SUCCESS;
    }
    if has("--dry-run") {
        println!("would delete the data of lane 2");
        return ExitCode::SUCCESS;
    }
    // No daemon runs, and the server asked not to start one.
    if var("FAKE_STOPPED").is_some() && var("DEVLANES_NO_SPAWN").is_some() {
        json_out(json!({"error": {"code": "daemon_not_running", "data": {}, "exit_code": 4}}));
        eprintln!("error: the daemon is not running");
        return ExitCode::from(4);
    }
    if var("FAKE_NEEDS_ENV").is_some() && var("DEVLANES_ENV_FROM_LANE").is_some() {
        json_out(
            json!({"error": {"code": "needs_env", "data": {"lane": "acme@1"}, "exit_code": 1}}),
        );
        // `with` goes on to write its summary after the error.
        if var("FAKE_STREAM").is_some() {
            json_out(json!({"event": "with.summary", "lane": "acme@1", "items": []}));
        }
        eprintln!("error: the daemon restarted and lost this lane's shell environment");
        return ExitCode::from(1);
    }
    // A unit whose program is not on the lane's PATH, as the agent output says it.
    if var("FAKE_NOT_FOUND").is_some() {
        println!(
            "acme@1 (main): not ready, 1 failed\nfailed api\n  couldn't start: pnpm: command not \
             found on PATH"
        );
        return ExitCode::from(1);
    }
    if var("FAKE_CONFIRM").is_some() && !has("--yes") {
        json_out(
            json!({"error": {"code": "confirmation_required", "data": {"flag": "--yes", "task": "deploy"}, "exit_code": 2}}),
        );
        eprintln!("error: task deploy asks: deploy now?; rerun with --yes");
        return ExitCode::from(2);
    }
    // Until the test says so, for at most 20 s.
    if let Some(marker) = var("FAKE_WAIT_FOR") {
        let until = Instant::now() + Duration::from_secs(20);
        while !std::path::Path::new(&marker).exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    if let Some(ms) = var("FAKE_SLEEP_MS").and_then(|ms| ms.parse::<u64>().ok()) {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            if let Some(ticks) = var("FAKE_TICKS") {
                append(&ticks, "tick");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    let data = if var("FAKE_LARGE").is_some() {
        json!({"items": ["x".repeat(300 * 1024)]})
    } else if has("status") {
        let mut status =
            json!({"items": [{"reference": "acme@1", "state": "running", "units": []}]});
        // A linked box, which `status` lists by its link state only.
        if var("FAKE_HOSTS").is_some() {
            status["hosts"] = json!([{"host": "hetzner", "state": "connected"}]);
        }
        status
    } else if has("remote") && has("ls") {
        json!({"items": [{"host": "hetzner", "state": "connected", "registry": {"lanes": [], "units": []}}]})
    } else if has("top") {
        json!({"lanes": []})
    } else if has("logs") {
        json!({"items": [{"unit": "api", "seq": 1, "text": "hello"}], "truncated": false})
    } else {
        json!({"ran": args})
    };
    if args.windows(2).any(|pair| pair == ["--output", "json"]) {
        println!("{data}");
    } else {
        println!("ran {}", args.join(" "));
    }
    json_out(data);
    let code = var("FAKE_EXIT")
        .and_then(|code| code.parse::<u8>().ok())
        .unwrap_or(0);
    ExitCode::from(code)
}

/// Appends one line to the file at `path`.
fn append(path: &str, line: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        // One write per line: a start's events child appends to the same log at
        // the same time, and `writeln!` may split the line and its newline into
        // two writes that the other process's line can land between.
        let _ = file.write_all(format!("{line}\n").as_bytes());
    }
}
