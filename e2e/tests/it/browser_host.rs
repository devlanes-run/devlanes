//! The browser extension's native host, driven over stdin and stdout as Chrome
//! drives it: `devlanes chrome-extension://<id>/`, then framed JSON.

use std::{
    io::{Read, Write},
    process::{Child, ChildStdin, ExitStatus, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use devlanes_e2e::{Error, Lab, Result};
use serde_json::{Value, json};

use crate::{BUDGET, need};

/// The origin Chrome passes for the devlanes extension.
const ORIGIN: &str = "chrome-extension://konofbnhgejkgkonkleipkkpkphbfgee/";

/// Longest wait for one message from the host.
const MESSAGE: Duration = Duration::from_secs(30);

/// A running host with its stdout split into messages.
struct Host {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: mpsc::Receiver<Value>,
    stderr: Arc<Mutex<String>>,
}

impl Host {
    /// Starts the host as Chrome does, with the extension origin as its only
    /// argument.
    fn start(lab: &Lab) -> Result<Self> {
        let mut command = lab.command(lab.repo(), &[ORIGIN])?;
        command.stdin(Stdio::piped());
        let mut child = lab.spawn(command)?;
        let stdin = need(child.stdin.take(), "host stdin")?;
        let mut stdout = need(child.stdout.take(), "host stdout")?;
        let mut stderr_pipe = need(child.stderr.take(), "host stderr")?;
        let (sender, messages) = mpsc::channel();
        // Every byte on stdout must be a frame: anything else breaks the parse.
        thread::spawn(move || {
            loop {
                let mut length = [0u8; 4];
                if stdout.read_exact(&mut length).is_err() {
                    return;
                }
                let mut body = vec![0u8; u32::from_ne_bytes(length) as usize];
                if stdout.read_exact(&mut body).is_err() {
                    return;
                }
                let message = serde_json::from_slice(&body).unwrap_or_else(|error| {
                    json!({"not_json": String::from_utf8_lossy(&body), "error": error.to_string()})
                });
                if sender.send(message).is_err() {
                    return;
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
            stdin: Some(stdin),
            messages,
            stderr,
        })
    }

    /// Writes one framed message.
    fn send(&mut self, message: &Value) -> Result<()> {
        let stdin = need(self.stdin.as_mut(), "an open host stdin")?;
        let body = message.to_string();
        stdin.write_all(&(body.len() as u32).to_ne_bytes())?;
        stdin.write_all(body.as_bytes())?;
        stdin.flush()?;
        Ok(())
    }

    /// The next message, or `None` when none comes within `wait`.
    fn next(&self, wait: Duration) -> Option<Value> {
        self.messages.recv_timeout(wait).ok()
    }

    /// The first message `wanted` accepts, skipping the others, within
    /// [`MESSAGE`] and the test's deadline.
    fn until(&self, lab: &Lab, what: &str, wanted: impl Fn(&Value) -> bool) -> Result<Value> {
        let deadline = Instant::now() + MESSAGE.min(lab.remaining());
        let mut seen = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Some(message) = self.next(left) else {
                return Err(Error::Timeout {
                    what: format!("browser host: {what}"),
                    after: MESSAGE,
                    stdout: seen.join("\n"),
                    stderr: self.stderr.lock().map(|s| s.clone()).unwrap_or_default(),
                });
            };
            assert!(message.get("not_json").is_none(), "{message}");
            if wanted(&message) {
                return Ok(message);
            }
            seen.push(message.to_string());
        }
    }

    /// Sends `hello` and returns its result.
    fn hello(&mut self, lab: &Lab) -> Result<Value> {
        self.send(
            &json!({"id": 1, "method": "hello", "params": {"protocol": 1, "extension": "0.1.0"}}),
        )?;
        let reply = self.until(lab, "hello", |message| message["id"] == 1)?;
        Ok(reply["result"].clone())
    }

    /// Closes stdin, as Chrome closes the port, and waits for the exit.
    fn close(mut self, lab: &Lab) -> Result<ExitStatus> {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(10).min(lab.remaining());
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(Error::Unexpected(
                    "the browser host didn't exit after its stdin closed".into(),
                ));
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Host {
    /// Ends a host that is still running; the lab stops the daemon afterwards.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `hello` and `watch` give a view with the started lane, its running source
/// and its routes, and the host exits 0 when the port closes.
#[test]
fn the_view_lists_a_started_lane_with_its_routes() -> Result<()> {
    let lab = Lab::new("two-services", BUDGET)?;
    let started = lab.json(lab.repo(), &["start"])?;
    assert_eq!(started["ready"], true, "{started}");
    let reference = need(started["lane"].as_str(), "the started lane")?.to_owned();
    let routes = lab.json(lab.repo(), &["url", "ls", "--json"])?;
    let mut hosts: Vec<String> = routes["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|route| route["lane"] == reference.as_str())
        .filter_map(|route| route["host"].as_str().map(str::to_owned))
        .collect();
    hosts.sort();
    assert!(!hosts.is_empty(), "{routes}");

    let mut host = Host::start(&lab)?;
    let hello = host.hello(&lab)?;
    assert_eq!(hello["protocol"], 1, "{hello}");
    assert_eq!(hello["daemon"], "running", "{hello}");
    host.send(&json!({"id": 2, "method": "watch"}))?;
    let watched = host.until(&lab, "watch", |message| message["id"] == 2)?;
    assert_eq!(watched, json!({"id": 2, "result": {}}));
    let view = host.until(&lab, "a view with the started lane", |message| {
        message["event"] == "view"
            && message["data"]["projects"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|project| project["lanes"].as_array().into_iter().flatten())
                .any(|lane| {
                    lane["ref"] == reference.as_str() && lane["sources"][0]["state"] == "running"
                })
    })?;
    let lane = view["data"]["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|project| project["lanes"].as_array().into_iter().flatten())
        .find(|lane| lane["ref"] == reference.as_str())
        .cloned()
        .unwrap_or_default();
    assert_eq!(lane["served_by"], Value::Null, "{lane}");
    assert_eq!(lane["sources"][0]["serving"], true, "{lane}");
    let mut viewed: Vec<String> = lane["routes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|route| route["host"].as_str().map(str::to_owned))
        .collect();
    viewed.sort();
    assert_eq!(viewed, hosts, "{lane}");
    // A wildcard host has no single URL, and every other route has one.
    for route in lane["routes"].as_array().into_iter().flatten() {
        let wildcard = route["host"].as_str().is_some_and(|h| h.starts_with("*."));
        match route["url"].as_str() {
            Some(url) => assert!(!wildcard && url.starts_with("https://"), "{route}"),
            None => assert!(wildcard, "{route}"),
        }
    }

    let status = host.close(&lab)?;
    assert!(status.success(), "{status:?}");
    Ok(())
}

/// Without a daemon the host says so, keeps answering and starts none.
#[test]
fn without_a_daemon_the_host_reports_stopped_and_starts_none() -> Result<()> {
    let lab = Lab::without_config(BUDGET)?;
    let mut host = Host::start(&lab)?;
    let hello = host.hello(&lab)?;
    assert_eq!(hello["daemon"], "stopped", "{hello}");
    host.send(&json!({"id": 2, "method": "watch"}))?;
    let watched = host.until(&lab, "watch", |message| message["id"] == 2)?;
    assert_eq!(watched, json!({"id": 2, "result": {}}));
    // Past two reconnects: no view, no state change and no daemon.
    let deadline = Instant::now() + Duration::from_secs(5);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if let Some(message) = host.next(left) {
            panic!("the host sent {message} without a daemon");
        }
    }
    assert!(!lab.daemon_running(), "the host started a daemon");
    let status = host.close(&lab)?;
    assert!(status.success(), "{status:?}");
    Ok(())
}

/// An origin other than the devlanes extension's exits 1 and writes nothing
/// on stdout.
#[test]
fn an_unknown_origin_exits_with_code_1() -> Result<()> {
    let lab = Lab::without_config(BUDGET)?;
    let output = lab.devlanes(
        lab.repo(),
        &["chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/"],
    )?;
    output.expect_code(1)?;
    assert!(output.stdout.is_empty(), "{output}");
    assert!(output.stderr.contains("refuses the origin"), "{output}");
    Ok(())
}
