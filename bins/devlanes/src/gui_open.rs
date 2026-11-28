//! `devlanes gui open`: starts the viewer of a GUI service on this machine
//! from the caller's environment, or asks the one that runs for its windows.
//!
//! The viewer is `devlanes-gui view` next to this `devlanes`, started on its
//! own so it outlives this command. A service has one viewer here
//! ([`devlanes_gui_client::instance`]), shared with the daemon's auto-open.

use std::path::Path;
use std::time::Duration;

use devlanes_cli::exit;
use devlanes_daemon::gui::lookup::{Found, Service};
use devlanes_daemon::gui::viewer::{self, Launch};
use devlanes_gui_client::instance;
use devlanes_proto::RpcError;
use serde_json::{Value, json};

use crate::gui_commands::{reachable, unreachable_error};

/// The name this process gives in its hello, for the host's logs.
const CLIENT_NAME: &str = "devlanes gui open";

/// How long a new viewer has to fail before the command counts it as up:
/// a missing display or a host that refuses it fails at once.
const STARTUP: Duration = Duration::from_secs(1);

/// Exit code of `devlanes-gui view` on a machine without a display.
const NO_DISPLAY_EXIT: i32 = 3;

/// Opens `services` of `found`: the ones the command `named`, each of which
/// must run, or else every GUI service of the lane, of which the running
/// ones open. Returns `{lane, box, items: [{unit, address, viewer}]}`.
pub(crate) async fn open(
    found: &Found,
    services: Vec<Service>,
    named: bool,
) -> Result<Value, RpcError> {
    if !has_display() {
        return Err(RpcError::new("gui_no_display", json!({}), exit::FAILED));
    }
    let program = tokio::task::spawn_blocking(crate::gui::program)
        .await
        .map_err(|error| failed(found, "", &error.to_string()))?;
    let present = {
        let program = program.clone();
        tokio::task::spawn_blocking(move || program.is_file())
            .await
            .unwrap_or(false)
    };
    if !present {
        return Err(RpcError::new(
            "gui_viewer_missing",
            json!({"path": program}),
            exit::FAILED,
        ));
    }
    let devlanes = std::env::current_exe().ok();
    let home = devlanes_client::Paths::discover()
        .map_err(|error| failed(found, "", &error.to_string()))?
        .home;
    // Without a unit, only the services that run: the others say why in `gui ls`.
    let wanted: Vec<&Service> = services
        .iter()
        .filter(|service| named || service.address.is_some())
        .collect();
    if wanted.is_empty() {
        // Why the one service there cannot be opened, else that none runs.
        return Err(match services.as_slice() {
            [one] => reachable(&base(found, one), one)
                .err()
                .unwrap_or_else(|| no_services(found)),
            _ => no_services(found),
        });
    }
    let mut items = Vec::new();
    for service in wanted {
        let base = base(found, service);
        let address = reachable(&base, service)?;
        // The port answers the protocol, or the viewer would only reconnect.
        devlanes_gui_tool::connect(address, CLIENT_NAME)
            .await
            .map_err(|error| unreachable_error(&base, address, &error))?
            .close();
        let label = format!("{}:{}", found.lane, service.unit);
        let launch = Launch {
            program: program.clone(),
            address,
            service: label.clone(),
            instance_dir: viewer::instance_dir(&home),
            auto: false,
            devlanes: devlanes.clone(),
            log: viewer::log_path(&home, &instance::key(&label, address.port())),
            env: Vec::new(),
        };
        let state = start(launch)
            .await
            .map_err(|message| failed(found, &service.unit, &message))?;
        items.push(json!({"unit": service.unit, "address": address.to_string(), "viewer": state}));
    }
    Ok(json!({"lane": found.lane, "box": found.host_box, "items": items}))
}

/// Starts the viewer `launch` describes, or asks the running one for its
/// windows. Returns `opened` or `already_open`, or why the viewer failed.
async fn start(launch: Launch) -> Result<&'static str, String> {
    let key = instance::key(&launch.service, launch.address.port());
    let dir = launch.instance_dir.clone();
    let asked = {
        let (locks, key) = (dir.clone(), key.clone());
        tokio::task::spawn_blocking(move || -> std::io::Result<bool> {
            if instance::running(&locks, &key)? {
                instance::ask_to_show(&locks, &key)?;
                return Ok(true);
            }
            Ok(false)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("cannot use the viewer lock in {}: {error}", dir.display()))?
    };
    if asked {
        return Ok("already_open");
    }
    let log = launch.log.clone();
    let mut child = tokio::task::spawn_blocking(move || viewer::spawn(&launch))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    let deadline = tokio::time::Instant::now() + STARTUP;
    while tokio::time::Instant::now() < deadline {
        match child.try_wait() {
            // Another viewer took the lock first and was asked for its windows.
            Ok(Some(status)) if status.success() => return Ok("already_open"),
            Ok(Some(status)) if status.code() == Some(NO_DISPLAY_EXIT) => {
                return Err("there is no display to open windows on".into());
            }
            Ok(Some(status)) => return Err(log_tail(&log).unwrap_or_else(|| status.to_string())),
            Ok(None) => tokio::time::sleep(Duration::from_millis(100)).await,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok("opened")
}

/// The last line of the viewer's log, which says why it ended.
fn log_tail(log: &Path) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().trim_start_matches("devlanes-gui: ").to_owned())
}

/// Whether this machine has a display to open windows on, from the caller's
/// environment, which the viewer inherits. Only Linux can lack one.
fn has_display() -> bool {
    if !cfg!(target_os = "linux") {
        return true;
    }
    ["WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DISPLAY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

/// `{lane, unit, box}` of `service`, which errors carry.
fn base(found: &Found, service: &Service) -> Value {
    json!({"lane": found.lane, "unit": service.unit, "box": found.host_box})
}

/// `gui_no_services`: nothing to open in the lane.
fn no_services(found: &Found) -> RpcError {
    RpcError::new(
        "gui_no_services",
        json!({"lane": found.lane, "box": found.host_box}),
        exit::FAILED,
    )
}

/// `gui_viewer_failed` for `unit` with `message`.
fn failed(found: &Found, unit: &str, message: &str) -> RpcError {
    RpcError::new(
        "gui_viewer_failed",
        json!({"lane": found.lane, "unit": unit, "box": found.host_box, "message": message}),
        exit::FAILED,
    )
}
