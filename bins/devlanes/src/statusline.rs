//! `devlanes status --statusline`: the line a status line host (Claude Code's
//! `statusLine` command) shows for the lane of its directory. A status line runs
//! on every refresh, so this connects only to a daemon that already runs, asks
//! for status once, and prints nothing at all when anything goes wrong.

use std::{
    io::{IsTerminal, Read, Write},
    process::ExitCode,
    time::Duration,
};

use devlanes_cli::{StatuslineInput, statusline, statusline_lane};
use devlanes_client::{Client, Paths};
use devlanes_proto::{Context, LaneSummary, Method, Page};
use serde_json::json;

/// Longest the whole run may take, stdin to the printed line. Claude Code runs
/// the command again on its next update, debounced to 300 ms, and cancels a run
/// still going then.
const BUDGET: Duration = Duration::from_millis(300);

/// Most bytes read from stdin; Claude Code sends a few KiB.
const STDIN_LIMIT: u64 = 1 << 20;

/// Prints the line, or nothing; always exits 0 so the host shows no error.
pub fn run() -> ExitCode {
    let Ok(tokio) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return ExitCode::SUCCESS;
    };
    let line = tokio.block_on(async { tokio::time::timeout(BUDGET, line()).await.ok().flatten() });
    // A stdin read the budget cut short must not keep the process alive.
    tokio.shutdown_background();
    if let Some(line) = line {
        let _ = writeln!(std::io::stdout(), "{line}");
    }
    ExitCode::SUCCESS
}

/// The rendered line, or `None` without a daemon, a lane or an answer.
async fn line() -> Option<String> {
    let input = tokio::task::spawn_blocking(|| {
        let mut text = String::new();
        let stdin = std::io::stdin();
        // A terminal means a person typed the command: there is no JSON to wait for.
        if !stdin.is_terminal() {
            let _ = stdin.lock().take(STDIN_LIMIT).read_to_string(&mut text);
        }
        StatuslineInput::parse(&text)
    })
    .await
    .ok()?;
    let cwd = match input.cwd {
        Some(cwd) => cwd,
        None => std::env::current_dir().ok()?,
    };
    let paths = Paths::discover().ok()?;
    // `connect` never spawns a daemon, unlike the other commands' connect.
    let mut client = Client::connect(&paths).await.ok()?;
    // `all` lists the lanes without registering the directory's project or
    // giving its worktree a lane, which a plain `status` there would do.
    let data = client
        .request(Method::Status, json!({"all": true}), Context::default())
        .await
        .ok()?;
    let lanes: Page<LaneSummary> = serde_json::from_value(data).ok()?;
    let lane = statusline_lane(&lanes.items, &cwd)?;
    let color = std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
    Some(statusline(lane, input.session_id.as_deref(), color))
}
