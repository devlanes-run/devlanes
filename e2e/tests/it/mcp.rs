//! `devlanes mcp` over stdio, as an agent host drives it: exec a server through
//! the `devlanes` tool, read its logs with `devlanes_read`, stop it, and stop
//! units of the same agent session with `stop --mine` from either side.

use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use devlanes_e2e::{Error, Lab, Result, alive, pid};
use serde_json::{Value, json};

use crate::{BUDGET, need};

/// Longest wait for one response.
const RESPONSE: Duration = Duration::from_secs(60);

/// A running `devlanes mcp` with its stdout split into lines.
struct Mcp {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<String>>,
    next_id: u64,
}

impl Mcp {
    /// Starts `devlanes mcp` in the lab's repository.
    fn start(lab: &Lab) -> Result<Self> {
        Self::spawn(lab, lab.command(lab.repo(), &["mcp"])?)
    }

    /// Starts `command`, a `devlanes mcp` command line, and completes the
    /// `initialize` handshake.
    fn spawn(lab: &Lab, command: Command) -> Result<Self> {
        Self::spawn_with(lab, command, json!({}))
    }

    /// Like [`Mcp::spawn`], for a client with these `capabilities`.
    fn spawn_with(lab: &Lab, mut command: Command, capabilities: Value) -> Result<Self> {
        command.stdin(Stdio::piped());
        let mut child = lab.spawn(command)?;
        let stdin = need(child.stdin.take(), "mcp stdin")?;
        let stdout = need(child.stdout.take(), "mcp stdout")?;
        let mut stderr_pipe = need(child.stderr.take(), "mcp stderr")?;
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout)
                .lines()
                .map_while(std::io::Result::ok)
            {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let collected = stderr.clone();
        thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr_pipe.read_to_string(&mut text);
            if let Ok(mut stderr) = collected.lock() {
                stderr.push_str(&text);
            }
        });
        let mut mcp = Self {
            child,
            stdin,
            lines,
            stderr,
            next_id: 1,
        };
        let init = mcp.request(
            lab,
            "initialize",
            json!({
                "protocolVersion": "2025-11-25",
                "capabilities": capabilities,
                "clientInfo": {"name": "e2e", "version": "1.0.0"},
            }),
        )?;
        assert_eq!(init["serverInfo"]["name"], "devlanes", "{init}");
        mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(mcp)
    }

    /// Writes one JSON-RPC message.
    fn send(&mut self, message: &Value) -> Result<()> {
        writeln!(self.stdin, "{message}")?;
        self.stdin.flush()?;
        Ok(())
    }

    /// Sends a request and waits for its response, skipping notifications. Every
    /// stdout line must be JSON-RPC: the protocol owns stdout.
    fn request(&mut self, lab: &Lab, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + RESPONSE.min(lab.remaining());
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self.lines.recv_timeout(left).map_err(|_| Error::Timeout {
                what: format!("devlanes mcp: {method}"),
                after: RESPONSE,
                stdout: String::new(),
                stderr: self.stderr.lock().map(|s| s.clone()).unwrap_or_default(),
            })?;
            let message: Value = serde_json::from_str(&line).map_err(|error| {
                Error::Unexpected(format!("not JSON-RPC on stdout: {line}: {error}"))
            })?;
            assert_eq!(message["jsonrpc"], "2.0", "{line}");
            if message["id"] != json!(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(Error::Unexpected(format!("{method}: {error}")));
            }
            return Ok(message["result"].clone());
        }
    }

    /// Calls `tool` with `args`; the result must not be an error.
    fn call(&mut self, lab: &Lab, tool: &str, args: &[&str]) -> Result<Value> {
        let result = self.request(
            lab,
            "tools/call",
            json!({"name": tool, "arguments": {"args": args}}),
        )?;
        if result["isError"] == true {
            return Err(Error::Unexpected(format!("{tool} {args:?}: {result}")));
        }
        Ok(result)
    }
}

impl Drop for Mcp {
    /// Ends the server; the lab stops the daemon afterwards.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The text content of a tool result.
fn text(result: &Value) -> String {
    result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block["text"].as_str())
        .collect()
}

#[test]
fn mcp_exec_logs_and_stop() -> Result<()> {
    let lab = Lab::new("two-services", BUDGET)?;
    let mut mcp = Mcp::start(&lab)?;
    let tools = mcp.request(&lab, "tools/list", json!({}))?;
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(names, ["devlanes", "devlanes_read"], "{tools}");
    assert!(!lab.daemon_running(), "the server alone started a daemon");

    let exec = mcp.call(
        &lab,
        "devlanes",
        &[
            "exec",
            "--name",
            "probe",
            "--",
            "fixture-server",
            "--port",
            "0",
            "--log-line",
            "probe-hello",
        ],
    )?;
    let unit = &exec["structuredContent"]["data"];
    assert_eq!(unit["name"], "probe", "{exec}");
    // A fresh daemon has no shell env for the lane, so the server's is used.
    assert!(
        text(&exec).contains("environment of the devlanes mcp server"),
        "{exec}"
    );
    let pid = need(
        unit["pid"].as_i64().and_then(|pid| i32::try_from(pid).ok()),
        "probe pid",
    )?;
    lab.track(pid);

    lab.eventually(
        "probe output through devlanes_read",
        Duration::from_secs(15),
        || {
            let logs = mcp.call(&lab, "devlanes_read", &["logs", "probe", "--tail", "5"])?;
            Ok(text(&logs).contains("probe-hello").then_some(()))
        },
    )?;

    let stopped = mcp.call(&lab, "devlanes", &["stop", "probe"])?;
    assert_eq!(stopped["structuredContent"]["exit_code"], 0, "{stopped}");
    lab.eventually("probe to exit", Duration::from_secs(15), || {
        Ok((!alive(pid)).then_some(()))
    })?;
    Ok(())
}

/// The cards poll `app_snapshot`, which never starts a daemon: without one it
/// says the daemon is stopped.
#[test]
fn mcp_snapshot_never_spawns() -> Result<()> {
    let lab = Lab::new("two-services", BUDGET)?;
    let cards = json!({
        "extensions": {
            "io.modelcontextprotocol/ui": {"mimeTypes": ["text/html;profile=mcp-app"]}
        }
    });
    let mut mcp = Mcp::spawn_with(&lab, lab.command(lab.repo(), &["mcp"])?, cards)?;
    let snapshot = mcp.request(
        &lab,
        "tools/call",
        json!({"name": "app_snapshot", "arguments": {"metrics": true}}),
    )?;
    assert_eq!(
        snapshot["structuredContent"],
        json!({"status": null, "top": null, "hosts": null, "daemon": "stopped", "read_only": false}),
        "{snapshot}"
    );
    assert!(!lab.daemon_running(), "app_snapshot started a daemon");
    Ok(())
}

/// A `devlanes` command in the lab's repository that runs as Claude Code
/// session `session` would: is-ai-agent reads the session id from
/// `CLAUDE_CODE_SESSION_ID`.
fn as_agent(lab: &Lab, session: &str, args: &[&str]) -> Result<Command> {
    let mut command = lab.command(lab.repo(), args)?;
    command
        .env_remove("DEVLANES_AGENT")
        .env("CLAUDECODE", "1")
        .env("CLAUDE_CODE_SESSION_ID", session);
    Ok(command)
}

/// `devlanes stop --mine` from the CLI, as `session`: the names it stopped.
fn cli_stop_mine(lab: &Lab, session: &str) -> Result<Vec<String>> {
    let output = lab.run(as_agent(lab, session, &["stop", "--mine"])?, RESPONSE)?;
    output.expect_code(0)?;
    Ok(names(&output.json()?))
}

/// `stop --mine` through the `devlanes` tool: the names it stopped.
fn mcp_stop_mine(mcp: &mut Mcp, lab: &Lab) -> Result<Vec<String>> {
    let stopped = mcp.call(lab, "devlanes", &["stop", "--mine"])?;
    Ok(names(&stopped["structuredContent"]["data"]))
}

/// The unit names in a `stop` result.
fn names(stopped: &Value) -> Vec<String> {
    stopped["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|unit| unit["name"].as_str().map(str::to_owned))
        .collect()
}

/// The `fixture-server` command line of an exec named `name`.
fn exec_args(name: &str) -> [&str; 7] {
    [
        "exec",
        "--name",
        name,
        "--",
        "fixture-server",
        "--port",
        "0",
    ]
}

/// Units started through `devlanes mcp` and through the CLI carry the same
/// actor, so `stop --mine` from either side stops them for the same agent
/// session and leaves them running for another one.
#[test]
fn mcp_and_cli_stop_mine_agree() -> Result<()> {
    let lab = Lab::new("two-services", BUDGET)?;
    let mine = format!("e2e-{}-mine", std::process::id());
    let other = format!("e2e-{}-other", std::process::id());
    let mut mcp = Mcp::spawn(&lab, as_agent(&lab, &mine, &["mcp"])?)?;

    let exec = mcp.call(&lab, "devlanes", &exec_args("from-mcp"))?;
    let from_mcp = need(pid(&exec["structuredContent"]["data"]), "from-mcp pid")?;
    lab.track(from_mcp);
    assert_eq!(cli_stop_mine(&lab, &other)?, Vec::<String>::new());
    assert!(alive(from_mcp), "another session stopped from-mcp");
    assert_eq!(cli_stop_mine(&lab, &mine)?, ["from-mcp"]);
    lab.eventually("from-mcp to exit", Duration::from_secs(15), || {
        Ok((!alive(from_mcp)).then_some(()))
    })?;

    let exec = lab.run(as_agent(&lab, &mine, &exec_args("from-cli"))?, RESPONSE)?;
    exec.expect_code(0)?;
    let from_cli = need(pid(&exec.json()?), "from-cli pid")?;
    let mut elsewhere = Mcp::spawn(&lab, as_agent(&lab, &other, &["mcp"])?)?;
    assert_eq!(mcp_stop_mine(&mut elsewhere, &lab)?, Vec::<String>::new());
    assert!(alive(from_cli), "another session stopped from-cli");
    assert_eq!(mcp_stop_mine(&mut mcp, &lab)?, ["from-cli"]);
    lab.eventually("from-cli to exit", Duration::from_secs(15), || {
        Ok((!alive(from_cli)).then_some(()))
    })?;
    Ok(())
}
