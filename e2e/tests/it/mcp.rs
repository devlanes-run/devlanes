//! `devlanes mcp` over stdio, as an agent host drives it: exec a server through
//! the `devlanes` tool, read its logs with `devlanes_read`, stop it.

use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use devlanes_e2e::{Error, Lab, Result, alive};
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
        let mut command = lab.command(lab.repo(), &["mcp"])?;
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
        Ok(Self {
            child,
            stdin,
            lines,
            stderr,
            next_id: 1,
        })
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
    let init = mcp.request(
        &lab,
        "initialize",
        json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "e2e", "version": "1.0.0"},
        }),
    )?;
    assert_eq!(init["serverInfo"]["name"], "devlanes", "{init}");
    mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
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
