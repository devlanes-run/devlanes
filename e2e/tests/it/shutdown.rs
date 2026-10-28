//! Daemon shutdown with a running lane: SIGTERM (a service manager, logout)
//! stops the units without `lane.stop` tasks, and `devlanes daemon stop` stops
//! them after running the tasks.

use std::time::Duration;

use devlanes_e2e::{Lab, Result, alive, lane, listening, pid, port, start_time, unit};
use serde_json::json;

use crate::{BUDGET, need, units_in};

/// A pipe service, which would outlive a daemon that did not stop it, and a
/// stop task that leaves a marker in `$HOME` (the lab's home).
const CONFIG: &str = r#"project: fixture-shutdown
services:
  api:
    run: [fixture-server, --log-line, api-ready]
    pty: false
    ready: { http: { path: /healthz } }
tasks:
  mark:
    run: 'echo stopped >> "$HOME/lane-stop-marker"'
    when: lane.stop
"#;

/// Starts the lane and returns the service's PID and port.
fn start(lab: &Lab) -> Result<(i32, u16)> {
    let started = lab.json(lab.repo(), &["start"])?;
    assert_eq!(started["ready"], true, "{started}");
    let lanes = lab.lanes(lab.repo())?;
    let api = need(
        lane(&lanes, "fixture-shutdown@0").and_then(|l| unit(l, "api")),
        "api",
    )?;
    let port = need(port(api), "api port")?;
    assert!(listening(port), "api listens on {port}");
    Ok((need(pid(api), "api pid")?, port))
}

/// Waits until `api` is ready again, started by the daemon a restart brought up.
fn api_ready(lab: &Lab) -> Result<()> {
    lab.eventually("api to be ready again", Duration::from_secs(60), || {
        let lanes = lab.lanes(lab.repo())?;
        let ready = lane(&lanes, "fixture-shutdown@0")
            .and_then(|lane| unit(lane, "api"))
            .is_some_and(|api| api["state"] == "ready");
        Ok(ready.then_some(()))
    })
}

/// Waits until the daemon process `daemon` is gone and released its lock.
fn wait_for_exit(lab: &Lab, daemon: i32) -> Result<()> {
    lab.eventually("the daemon to exit", Duration::from_secs(30), || {
        Ok((!alive(daemon) && !lab.daemon_running()).then_some(()))
    })
}

/// SIGTERM to the daemon stops the service (process gone, port closed) and
/// closes its run, so the next daemon finds the lane idle with no orphan; the
/// stop task does not run.
// Signals are Unix. On Windows, logoff and shutdown reach the daemon as
// WM_ENDSESSION; devlanes-sys-windows tests that window.
#[cfg(unix)]
#[test]
fn sigterm_stops_units_without_lane_stop_tasks() -> Result<()> {
    let lab = Lab::with_config(CONFIG, BUDGET)?;
    let (api, port) = start(&lab)?;
    let daemon = need(lab.daemon_pid()?, "daemon pid")?;
    let target = need(rustix::process::Pid::from_raw(daemon), "daemon pid")?;
    rustix::process::kill_process(target, rustix::process::Signal::TERM)
        .map_err(|error| devlanes_e2e::Error::Io(error.into()))?;
    wait_for_exit(&lab, daemon)?;
    assert!(!alive(api), "api (pid {api}) still runs");
    assert!(!listening(port), "port {port} still open");
    assert!(
        !lab.home().join("lane-stop-marker").exists(),
        "a signal shutdown ran the lane.stop task"
    );

    // `status` spawns a new daemon, which reads the closed run from state.db.
    let lanes = lab.lanes(lab.repo())?;
    let zero = need(lane(&lanes, "fixture-shutdown@0"), "lane 0")?;
    assert_eq!(zero["state"], "idle", "{zero}");
    assert!(units_in(zero, "orphaned").is_empty(), "{zero}");
    let respawned = need(lab.daemon_pid()?, "new daemon pid")?;
    assert_ne!(respawned, daemon, "a new daemon answered");
    Ok(())
}

/// `devlanes daemon restart` starts what ran again on the new daemon, with the
/// lane's profiles; `daemon stop` then leaves it stopped for the next daemon.
#[test]
fn daemon_restart_brings_back_what_ran() -> Result<()> {
    let lab = Lab::with_config(CONFIG, BUDGET)?;
    let (api, _) = start(&lab)?;
    // Windows hands a freed PID to the next process soon, so "the old run
    // stopped" means no process with this PID and start time.
    let api_started = need(start_time(api), "api start time")?;
    let daemon = need(lab.daemon_pid()?, "daemon pid")?;
    let restarted = lab.json(lab.repo(), &["daemon", "restart", "--yes"])?;
    // The restart ends when the new daemon answers; the lane comes back as a start.
    assert_eq!(
        restarted["resuming"],
        json!(["fixture-shutdown@0"]),
        "{restarted}"
    );
    assert!(
        !alive(api) || start_time(api).as_deref() != Some(api_started.as_str()),
        "the old run (pid {api}) stopped"
    );
    api_ready(&lab)?;
    let lanes = lab.lanes(lab.repo())?;
    let zero = need(lane(&lanes, "fixture-shutdown@0"), "lane 0")?;
    assert_eq!(units_in(zero, "ready"), ["api"], "{zero}");
    assert_ne!(need(lab.daemon_pid()?, "new daemon pid")?, daemon);

    let new_daemon = need(lab.daemon_pid()?, "daemon pid")?;
    lab.json(lab.repo(), &["daemon", "stop", "--yes"])?;
    wait_for_exit(&lab, new_daemon)?;
    let lanes = lab.lanes(lab.repo())?;
    let zero = need(lane(&lanes, "fixture-shutdown@0"), "lane 0")?;
    assert!(
        units_in(zero, "ready").is_empty(),
        "stop keeps it stopped: {zero}"
    );
    Ok(())
}

/// Plain output prints each step of a restart as it finishes, then what starts
/// again and how to wait for it.
#[test]
fn daemon_restart_prints_its_steps() -> Result<()> {
    let lab = Lab::with_config(CONFIG, BUDGET)?;
    start(&lab)?;
    let output = lab.devlanes(
        lab.repo(),
        &["--output", "plain", "daemon", "restart", "--yes"],
    )?;
    output.expect_code(0)?;
    for expected in [
        "stopped 1 process in 1 lane",
        "ran 1 lane.stop task",
        "started devlanes",
        "starting again: fixture-shutdown@0",
        "next: devlanes status",
    ] {
        assert!(output.stdout.contains(expected), "{expected}: {output}");
    }
    api_ready(&lab)
}

/// `daemon stop` on plain output prints its steps, then the report.
#[test]
fn daemon_stop_prints_its_steps() -> Result<()> {
    let lab = Lab::with_config(CONFIG, BUDGET)?;
    start(&lab)?;
    let daemon = need(lab.daemon_pid()?, "daemon pid")?;
    let output = lab.devlanes(
        lab.repo(),
        &["--output", "plain", "daemon", "stop", "--yes"],
    )?;
    output.expect_code(0)?;
    for expected in ["ran 1 lane.stop task", "stopped 1 process in 1 lane"] {
        assert!(output.stdout.contains(expected), "{expected}: {output}");
    }
    wait_for_exit(&lab, daemon)
}

/// Once the old daemon was asked to stop, the restart no longer needs the
/// client: killing it leaves the new daemon to come up and start the lane.
#[test]
fn a_restart_survives_its_client() -> Result<()> {
    let config = format!(
        "{CONFIG}  linger:\n    run: [fixture-server, --port, \"0\", --exit-after, 3s]\n    when: lane.stop\n"
    );
    let lab = Lab::with_config(&config, BUDGET)?;
    start(&lab)?;
    let daemon = need(lab.daemon_pid()?, "daemon pid")?;
    let mut client = lab.spawn(lab.command(lab.repo(), &["daemon", "restart", "--yes"])?)?;
    // The new daemon writes the marker while it waits; the client asks the old
    // one to stop right after, and `linger` holds that shutdown for 3 s.
    let marker = lab.home().join("run").join("daemon.replacing");
    lab.eventually("the new daemon to wait", Duration::from_secs(20), || {
        Ok(marker.exists().then_some(()))
    })?;
    std::thread::sleep(Duration::from_secs(1));
    let _ = client.kill();
    let _ = client.wait();
    // The new daemon holds the lock by then, so only the old pid tells.
    lab.eventually("the old daemon to exit", Duration::from_secs(30), || {
        Ok((!alive(daemon)).then_some(()))
    })?;
    api_ready(&lab)?;
    assert_ne!(need(lab.daemon_pid()?, "new daemon pid")?, daemon);
    Ok(())
}

/// A new daemon that dies while the old one stops does not cost the lanes:
/// the restarting client starts a daemon itself and brings them back there.
#[test]
fn a_restart_survives_a_new_daemon_that_dies() -> Result<()> {
    let config = format!(
        "{CONFIG}  linger:\n    run: [fixture-server, --port, \"0\", --exit-after, 3s]\n    when: lane.stop\n"
    );
    let lab = Lab::with_config(&config, BUDGET)?;
    start(&lab)?;
    let daemon = need(lab.daemon_pid()?, "daemon pid")?;
    let restart = std::thread::scope(|scope| -> Result<devlanes_e2e::Output> {
        let cli = scope.spawn(|| lab.devlanes(lab.repo(), &["daemon", "restart", "--yes"]));
        let marker = lab.home().join("run").join("daemon.replacing");
        let successor =
            lab.eventually("the new daemon to wait", Duration::from_secs(20), || {
                Ok(std::fs::read_to_string(&marker)
                    .ok()
                    .and_then(|pid| pid.trim().parse::<i32>().ok()))
            })?;
        // The client says go and asks the old daemon to stop right after the
        // marker appears; `linger` keeps that shutdown going for 3 s.
        std::thread::sleep(Duration::from_secs(1));
        devlanes_e2e::kill(successor);
        cli.join()
            .map_err(|_| devlanes_e2e::Error::Setup("restart thread panicked".into()))?
    })?;
    restart.expect_code(0)?;
    lab.eventually("the old daemon to exit", Duration::from_secs(30), || {
        Ok((!alive(daemon)).then_some(()))
    })?;
    api_ready(&lab)?;
    Ok(())
}

/// An on-demand service next to the default one, for a selection a plain start
/// would not pick by itself.
const ON_DEMAND: &str = r#"project: fixture-restore
services:
  api:
    run: [fixture-server, --log-line, api-ready]
    pty: false
    ready: { http: { path: /healthz } }
  worker:
    run: [fixture-server, --log-line, worker-ready]
    pty: false
    autostart: false
    ready: { http: { path: /healthz } }
"#;

/// After the daemon stops without a `devlanes stop`, a plain start brings back
/// every service that ran, the on-demand one included; after `devlanes stop`
/// it starts the default selection again.
#[test]
fn plain_start_after_daemon_stop_restores_what_ran() -> Result<()> {
    let lab = Lab::with_config(ON_DEMAND, BUDGET)?;
    let ready = |lab: &Lab| -> Result<Vec<String>> {
        let lanes = lab.lanes(lab.repo())?;
        let zero = need(lane(&lanes, "fixture-restore@0"), "lane 0")?;
        let mut names = units_in(zero, "ready");
        names.sort_unstable();
        Ok(names)
    };
    assert_eq!(lab.json(lab.repo(), &["start"])?["ready"], true);
    assert_eq!(lab.json(lab.repo(), &["start", "worker"])?["ready"], true);
    let daemon = need(lab.daemon_pid()?, "daemon pid")?;
    lab.json(lab.repo(), &["daemon", "stop", "--yes"])?;
    wait_for_exit(&lab, daemon)?;

    let restored = lab.json(lab.repo(), &["start"])?;
    assert_eq!(restored["ready"], true, "{restored}");
    assert_eq!(ready(&lab)?, ["api", "worker"]);

    lab.json(lab.repo(), &["stop"])?;
    let fresh = lab.json(lab.repo(), &["start"])?;
    assert_eq!(fresh["ready"], true, "{fresh}");
    assert_eq!(ready(&lab)?, ["api"], "a stopped lane starts its default");
    lab.json(lab.repo(), &["stop"])?;
    Ok(())
}

/// `devlanes daemon stop` runs the stop task, then stops the service, and
/// reports both.
#[test]
fn daemon_stop_runs_lane_stop_tasks() -> Result<()> {
    let lab = Lab::with_config(CONFIG, BUDGET)?;
    let (api, port) = start(&lab)?;
    let daemon = need(lab.daemon_pid()?, "daemon pid")?;
    let stopped = lab.json(lab.repo(), &["daemon", "stop", "--yes"])?;
    assert_eq!(stopped["reason"], "explicit", "{stopped}");
    assert_eq!(
        stopped["stopped"],
        json!([{"lane": "fixture-shutdown@0", "unit": "api"}]),
        "{stopped}"
    );
    assert_eq!(
        stopped["tasks"],
        json!([{"lane": "fixture-shutdown@0", "task": "mark", "exit_code": 0}]),
        "{stopped}"
    );
    wait_for_exit(&lab, daemon)?;
    assert!(!alive(api), "api (pid {api}) still runs");
    assert!(!listening(port), "port {port} still open");
    let marker = std::fs::read_to_string(lab.home().join("lane-stop-marker"))?;
    assert_eq!(marker, "stopped\n");
    Ok(())
}
