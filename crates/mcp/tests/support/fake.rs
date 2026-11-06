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

/// Runs one invocation.
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
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
            "json_out": var("DEVLANES_JSON_OUT").is_some(),
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
    if var("FAKE_NEEDS_ENV").is_some() && var("DEVLANES_ENV_FROM_LANE").is_some() {
        json_out(
            json!({"error": {"code": "needs_env", "data": {"lane": "acme@1"}, "exit_code": 1}}),
        );
        eprintln!("error: the daemon restarted and lost this lane's shell environment");
        return ExitCode::from(1);
    }
    if var("FAKE_CONFIRM").is_some() && !has("--yes") {
        json_out(
            json!({"error": {"code": "confirmation_required", "data": {"flag": "--yes", "task": "deploy"}, "exit_code": 2}}),
        );
        eprintln!("error: task deploy asks: deploy now?; rerun with --yes");
        return ExitCode::from(2);
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
    let data = if has("status") {
        json!({"items": [{"reference": "acme@1", "state": "running", "units": []}]})
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
        json_out(data);
    }
    let code = var("FAKE_EXIT")
        .and_then(|code| code.parse::<u8>().ok())
        .unwrap_or(0);
    ExitCode::from(code)
}

/// Appends one line to the file at `path`.
fn append(path: &str, line: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}
