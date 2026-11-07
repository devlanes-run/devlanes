//! What `devlanes mcp` relies on: [`Command::is_read_only`],
//! [`Cli::terminal_only`] and the `DEVLANES_JSON_OUT` and
//! `DEVLANES_ENV_FROM_LANE` hooks.

use std::{collections::BTreeSet, path::PathBuf};

use clap::{CommandFactory, Parser};
use devlanes_cli::*;
use devlanes_proto::{Method, RpcError};
use serde_json::{Value, json};

/// One command line per leaf command (and the variants that classify
/// differently), whether it only reads, and whether it needs a terminal.
const COMMANDS: &[(&[&str], bool, bool)] = &[
    (&["start"], false, false),
    (&["start", "--attach"], false, true),
    (&["stop"], false, false),
    (&["restart"], false, false),
    (&["reload"], false, false),
    (&["status"], true, false),
    (&["status", "--statusline"], true, true),
    (&["status", "--watch"], true, true),
    (&["status", "--watch", "--timeout", "5s"], true, false),
    (&["logs", "api"], true, false),
    (&["logs", "api", "-f"], true, true),
    (&["logs", "api", "-f", "--timeout", "5s"], true, false),
    (&["wait", "api"], true, false),
    (&["with", "--", "true"], false, false),
    (&["run", "build"], false, false),
    (&["run", "--list"], true, false),
    (
        &["exec", "--name", "web", "--", "pnpm", "dev"],
        false,
        false,
    ),
    (&["attach", "api"], false, true),
    (&["ui"], false, true),
    (&["lane", "ls"], true, false),
    (&["lane", "new"], false, false),
    (&["lane", "clean", "2"], false, false),
    (&["lane", "clean", "2", "--dry-run"], true, false),
    (&["lane", "gc"], false, false),
    (&["lane", "resume"], false, false),
    (&["debug", "why", "api"], true, false),
    (&["debug", "errors"], true, false),
    (&["debug", "events"], true, false),
    (&["debug", "events", "-f"], true, true),
    (&["debug", "events", "-f", "--timeout", "5s"], true, false),
    (&["debug", "top"], true, false),
    (&["debug", "freeze"], false, false),
    (&["debug", "thaw"], false, false),
    (&["debug", "config"], true, false),
    (&["debug", "bundle"], false, false),
    (&["env", "show"], true, false),
    (&["env", "export"], true, false),
    (&["env", "shell"], false, true),
    (&["url", "print", "web"], true, false),
    (&["url", "open", "web"], true, false),
    (&["url", "ls"], true, false),
    (&["infra", "up"], false, false),
    (&["infra", "down"], false, false),
    (&["infra", "status"], true, false),
    (&["infra", "logs", "db"], true, false),
    (&["infra", "logs", "db", "-f"], true, true),
    (
        &["infra", "logs", "db", "-f", "--timeout", "5s"],
        true,
        false,
    ),
    (&["config", "check"], true, false),
    (&["config", "get"], true, false),
    (&["config", "set", "tui.theme", "dark"], false, false),
    (&["config", "unset", "tui.theme"], false, false),
    (&["setup"], false, true),
    (&["update"], false, true),
    (&["uninstall"], false, true),
    (&["remote", "connect", "box"], false, false),
    (&["remote", "disconnect"], false, false),
    (&["remote", "ls"], true, false),
    (&["remote", "sync"], true, false),
    (&["remote", "use", "box", "acme@0"], false, false),
    (&["daemon", "start"], false, false),
    (&["daemon", "status"], true, false),
    (&["daemon", "stop"], false, false),
    (&["daemon", "restart"], false, false),
    (&["daemon", "logs"], true, false),
    (&["daemon", "logs", "-f"], true, true),
    (&["daemon", "install"], false, false),
    (&["daemon", "uninstall"], false, false),
    (&["daemon", "reset-state"], false, false),
    (&["daemon", "pair"], false, false),
    (&["daemon", "clients"], true, false),
    (&["daemon", "revoke", "laptop"], false, false),
    (&["daemon", "run"], false, true),
    (&["doctor"], true, false),
    (&["completions", "bash"], false, true),
    (&["mcp"], false, true),
    (&["tunnel", "--stdio"], false, true),
    (&["askpass"], false, true),
];

/// The command path of every leaf command clap knows, hidden ones included.
fn leaves(command: &clap::Command, prefix: &str, out: &mut BTreeSet<String>) {
    let mut subcommands = command
        .get_subcommands()
        .filter(|sub| sub.get_name() != "help")
        .peekable();
    if subcommands.peek().is_none() {
        out.insert(prefix.trim().to_owned());
        return;
    }
    for sub in subcommands {
        leaves(sub, &format!("{prefix} {}", sub.get_name()), out);
    }
}

#[test]
/// Every command is classified, and the table above covers every leaf command,
/// so a new command needs a row here as well as an arm in `is_read_only`.
fn every_command_is_classified() {
    let mut all = BTreeSet::new();
    leaves(&Cli::command(), "", &mut all);
    let mut covered = BTreeSet::new();
    for (args, read_only, terminal) in COMMANDS {
        let cli = Cli::try_parse_from(std::iter::once("devlanes").chain(args.iter().copied()))
            .unwrap_or_else(|error| panic!("{args:?}: {error}"));
        let command = cli.command.as_ref().expect("a command");
        assert_eq!(command.is_read_only(), *read_only, "is_read_only {args:?}");
        assert_eq!(
            cli.terminal_only().is_some(),
            *terminal,
            "terminal_only {args:?}: {:?}",
            cli.terminal_only()
        );
        covered.insert(
            command_path(&args.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>()).join(" "),
        );
    }
    let missing: Vec<_> = all.difference(&covered).collect();
    assert!(missing.is_empty(), "commands without a row: {missing:?}");
}

/// A runtime from fixed signals with these hooks.
fn hooked(json_out: Option<PathBuf>, env_from_lane: bool) -> Runtime {
    let cli = Cli::parse_from(["devlanes", "--output", "agent"]);
    Runtime::resolve(
        &cli,
        Signals {
            json_out,
            env_from_lane,
            ..Signals::default()
        },
    )
    .expect("runtime")
}

#[test]
/// With `DEVLANES_JSON_OUT` the file gets what `--output json` prints, plus
/// `{"error": ...}` for an error, while the caller still sees agent text.
fn json_out_copies_json_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("out.json");
    let runtime = hooked(Some(path.clone()), false);
    let data = json!({"items": [], "truncated": false, "next_cursor": null});
    let rendered = runtime
        .reporter()
        .result(&Report::new(Method::LaneList, &data))
        .expect("render");
    assert!(!rendered.stdout.starts_with('{'), "{}", rendered.stdout);
    let error = RpcError::new("needs_env", json!({"lane": "acme@1"}), 1);
    let shown = runtime.reporter().error(&error, None);
    assert_eq!(shown.exit_code, 1);
    let written = std::fs::read_to_string(&path).expect("json out");
    let lines: Vec<Value> = written
        .lines()
        .map(|line| serde_json::from_str(line).expect("json line"))
        .collect();
    assert_eq!(lines, [data, json!({"error": error})]);
    // Without the variable nothing is written anywhere.
    let plain = hooked(None, false);
    assert!(plain.json_out.is_none());
}

#[test]
/// With `DEVLANES_ENV_FROM_LANE` the process-starting requests carry no client
/// env, so the daemon uses the lane's; without it they carry the shell's.
fn env_from_lane_sends_no_env() {
    for (args, method) in [
        (&["start"][..], Method::Start),
        (&["restart"], Method::Restart),
        (&["run", "build"], Method::Run),
        (
            &["exec", "--name", "web", "--", "pnpm", "dev"],
            Method::Exec,
        ),
        (&["lane", "resume"], Method::Resume),
        (&["env", "show", "api"], Method::Env),
    ] {
        let cli = Cli::try_parse_from(std::iter::once("devlanes").chain(args.iter().copied()))
            .expect("parse");
        for from_lane in [false, true] {
            let runtime = hooked(None, from_lane);
            let action = cli
                .action_with(&runtime, PathBuf::from("/fixture/worktree"), || {
                    [("PATH".to_owned(), "/bin".to_owned())].into()
                })
                .expect("action");
            let Action::Rpc(request) = action else {
                panic!("{args:?}: expected an RPC");
            };
            assert_eq!(request.method, method);
            assert_eq!(request.context.env.is_none(), from_lane, "{args:?}");
        }
    }
}

/// A shell env with the variables `devlanes mcp` sets for one command.
fn mcp_child_env() -> std::collections::BTreeMap<String, String> {
    [
        ("PATH", "/bin"),
        ("DEVLANES_JSON_OUT", "/tmp/devlanes-mcp-1.json"),
        ("DEVLANES_ENV_FROM_LANE", "1"),
    ]
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .into()
}

#[test]
/// `DEVLANES_JSON_OUT` and `DEVLANES_ENV_FROM_LANE` belong to the one process
/// `devlanes mcp` started: no request env and no `with` command gets them.
fn mcp_call_variables_stay_in_the_process() {
    let runtime = hooked(None, false);
    for args in [
        &["start"][..],
        &["exec", "--name", "web", "--", "pnpm", "dev"],
        &["daemon", "restart"],
    ] {
        let cli = Cli::try_parse_from(std::iter::once("devlanes").chain(args.iter().copied()))
            .expect("parse");
        let action = cli
            .action_with(&runtime, PathBuf::from("/fixture/worktree"), mcp_child_env)
            .expect("action");
        let env = match action {
            Action::Rpc(request) => request.context.env,
            Action::DaemonRestart(context) => context.env,
            _ => panic!("{args:?}: unexpected action"),
        }
        .expect("an env");
        assert_eq!(env.keys().collect::<Vec<_>>(), ["PATH"], "{args:?}");
    }
    let cli = Cli::parse_from(["devlanes", "with", "--", "pnpm", "test"]);
    let Ok(Action::With(plan)) =
        cli.action_with(&runtime, PathBuf::from("/fixture/worktree"), mcp_child_env)
    else {
        panic!("with plan");
    };
    assert_eq!(plan.env.keys().collect::<Vec<_>>(), ["PATH"]);
    let start = plan.start.context.env.expect("start env");
    assert_eq!(start.keys().collect::<Vec<_>>(), ["PATH"]);
}

#[test]
/// With `DEVLANES_ENV_FROM_LANE` the start of `with` sends no env, so the lane
/// keeps its own, while the command still gets the shell's.
fn with_reuses_the_lanes_env() {
    let cli = Cli::parse_from(["devlanes", "with", "--", "pnpm", "test"]);
    let Ok(Action::With(plan)) = cli.action_with(
        &hooked(None, true),
        PathBuf::from("/fixture/worktree"),
        mcp_child_env,
    ) else {
        panic!("with plan");
    };
    assert!(plan.start.context.env.is_none());
    assert_eq!(plan.env["PATH"], "/bin");
}
