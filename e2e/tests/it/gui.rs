//! GUI services with the real `devlanes-gui` and its test-pattern backend,
//! which a debug build turns on with `DEVLANES_GUI_BACKEND`: the host's port
//! answers, a restart brings it back, a stop takes it away, and a `devlanes`
//! with no `devlanes-gui` next to it fails the service with `gui_host_missing`.

use std::{path::Path, process::Command, time::Duration};

use devlanes_e2e::{Error, Lab, Output, Result, bin_dir, lane, listening, unit};
use devlanes_gui_client::{ClientOptions, connect};
use devlanes_gui_proto::Role;
use serde_json::Value;

use crate::{BUDGET, need};

/// A GUI service that serves HTTP on its own port as well, so it has a ready check.
const CONFIG: &str = "\
project: fixture-gui
services:
  desktop:
    run: [fixture-server, --log-line, desktop-ready]
    ready: { http: { path: /healthz } }
    gui: true
";

/// `command` with GUI services on and the test-pattern backend; the daemon
/// the first command spawns keeps the variable.
fn with_gui(mut command: Command) -> Command {
    command.env("DEVLANES_GUI_BACKEND", "test-pattern");
    command
}

/// `devlanes <args>` in the repository with GUI services on.
fn devlanes(lab: &Lab, args: &[&str]) -> Result<Output> {
    let command = with_gui(lab.command(lab.repo(), args)?);
    lab.run(command, Duration::from_secs(60))
}

/// The `desktop` unit of lane 0.
fn desktop(lab: &Lab) -> Result<Value> {
    let status = devlanes(lab, &["status", "--json"])?
        .expect_code(0)?
        .json()?;
    let lanes = status["items"].as_array().cloned().unwrap_or_default();
    let found = lane(&lanes, "fixture-gui@0").and_then(|lane| unit(lane, "desktop"));
    need(found.cloned(), "the desktop unit")
}

/// The unit's `devlanes_gui` port.
fn gui_port(unit: &Value) -> Result<u16> {
    let port = unit["ports"]["devlanes_gui"].as_u64();
    need(
        port.and_then(|port| u16::try_from(port).ok()),
        "the devlanes_gui port",
    )
}

/// Says hello on `port` as a tool and takes a screenshot of the first
/// window; its size.
fn screenshot(port: u16) -> Result<(u32, u32)> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let options = ClientOptions::new(Role::Tool, "e2e", "e2e");
        let work = async {
            let (client, _events) = connect(("127.0.0.1", port), options).await?;
            let window = client.welcome().windows.first().map(|window| window.id);
            let shot = client.screenshot(window).await?;
            client.close();
            Ok::<_, devlanes_gui_client::ClientError>((shot.frame.width(), shot.frame.height()))
        };
        tokio::time::timeout(Duration::from_secs(20), work)
            .await
            .map_err(|_| Error::Unexpected(format!("no screenshot from port {port} in time")))?
            .map_err(|error| Error::Unexpected(format!("GUI client on port {port}: {error}")))
    })
}

/// Start, a hello and a screenshot through the host's port, restart, stop.
#[test]
fn a_gui_service_serves_its_windows_until_it_stops() -> Result<()> {
    let lab = Lab::with_config(CONFIG, BUDGET)?;
    let started = devlanes(&lab, &["start"])?.expect_code(0)?.json()?;
    assert_eq!(started["ready"], true, "{started}");
    let unit = desktop(&lab)?;
    assert_eq!(unit["gui"]["host"], "running", "{unit}");
    let port = gui_port(&unit)?;
    let (width, height) = screenshot(port)?;
    assert!(width > 0 && height > 0, "an empty screenshot");

    devlanes(&lab, &["restart", "desktop"])?.expect_code(0)?;
    let unit = desktop(&lab)?;
    assert_eq!(unit["gui"]["host"], "running", "{unit}");
    assert_eq!(gui_port(&unit)?, port, "the port moved");
    screenshot(port)?;

    devlanes(&lab, &["stop", "desktop"])?.expect_code(0)?;
    lab.eventually(
        "the GUI host to close its port",
        Duration::from_secs(15),
        || Ok((!listening(port)).then_some(())),
    )?;
    assert_eq!(desktop(&lab)?["gui"]["host"], "stopped");
    Ok(())
}

/// A `devlanes` without `devlanes-gui` next to it fails a GUI service with
/// `gui_host_missing` and runs the rest.
#[test]
fn a_missing_devlanes_gui_fails_the_service() -> Result<()> {
    // Dropped after the lab, whose daemon runs from this copy.
    let copy = tempfile::tempdir()?;
    let program = copy
        .path()
        .join(format!("devlanes{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(
        bin_dir()?.join(format!("devlanes{}", std::env::consts::EXE_SUFFIX)),
        &program,
    )?;
    let yaml = format!("{CONFIG}  api:\n    run: [fixture-server, --log-line, api-ready]\n");
    let lab = Lab::with_config(&yaml, BUDGET)?;
    let start = with_gui(lab.command_with(&program, lab.repo(), &["start"])?);
    let output = lab.run(start, Duration::from_secs(60))?;
    assert!(
        output.stdout.contains("gui_host_missing"),
        "{}",
        output.stdout
    );
    let unit = unit_of(&lab, &program, "desktop")?;
    assert_eq!(unit["state"], "failed", "{unit}");
    assert_eq!(unit["reason"]["code"], "gui_host_missing", "{unit}");
    let api = unit_of(&lab, &program, "api")?;
    assert_eq!(api["state"], "ready", "{api}");
    Ok(())
}

/// Unit `name` of lane 0, asked through `program`; status exits 1 for a
/// lane with a failed unit.
fn unit_of(lab: &Lab, program: &Path, name: &str) -> Result<Value> {
    let status = with_gui(lab.command_with(program, lab.repo(), &["status", "--json"])?);
    let status = lab.run(status, Duration::from_secs(60))?.json()?;
    let lanes = status["items"].as_array().cloned().unwrap_or_default();
    let found = lane(&lanes, "fixture-gui@0").and_then(|lane| unit(lane, name));
    need(found.cloned(), name)
}
