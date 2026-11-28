//! `devlanes gui`: finds a GUI service through the daemon and drives its GUI
//! host from this process as a `tool` connection. The daemon only says where
//! the host listens; screenshots and input never pass through it.
//!
//! A service of this machine's lanes is found in `status`, and a lane
//! reference this machine does not have (`acme@2`) in `remote ls`, as
//! [`devlanes_daemon::gui::lookup`] reads them; the daemon's auto-open uses
//! the same reading.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use devlanes_cli::{GuiButton, GuiCommand, GuiOutput, Runtime, exit};
use devlanes_core::refs::UnitRef;
use devlanes_daemon::gui::lookup::{self, Found, Service};
use devlanes_gui_codec::Frame;
use devlanes_gui_proto::{Buttons, LockState, Point, Size, WindowInfo, WindowKind};
use devlanes_gui_tool::{After, Chord, KeyError, Session, ToolError};
use devlanes_proto::{Context, LaneSummary, Method, Request, RpcError};
use serde_json::{Value, json};

use crate::output::emit;

/// The name this process gives in its hello, for the host's logs.
const CLIENT_NAME: &str = "devlanes gui";

/// Where screenshots go without `--to`: this folder of the devlanes home,
/// private to the account as the home is.
const SCREENSHOT_DIR: &str = "screenshots";

/// Runs a `devlanes gui` command and prints its result.
pub(crate) async fn run(
    runtime: &Runtime,
    command: GuiCommand,
    context: Context,
) -> Result<u8, RpcError> {
    // Release builds and debug builds without the switch have no GUI
    // services; that is said instead of failing to find one.
    if !crate::gui::enabled() {
        return Err(RpcError::new(
            "not_supported_yet",
            json!({"feature": "devlanes gui"}),
            exit::FAILED,
        ));
    }
    let chords = chords(&command)?;
    // Refused before anything connects, as the host would refuse it.
    if let GuiCommand::Resize { size, .. } = &command
        && size.width.max(size.height) > devlanes_gui_proto::MAX_DIMENSION
    {
        return Err(RpcError::new(
            "gui_size_too_large",
            json!({"max": devlanes_gui_proto::MAX_DIMENSION}),
            exit::USAGE,
        ));
    }
    let cwd = context.cwd.clone();
    let (lane, names) = split_units(&command.units())?;
    let found = find(context, lane).await?;
    if let GuiCommand::Ls { .. } = &command {
        let services = select(&found, &names, true)?;
        let data = ls(&found, &services).await;
        return report(runtime, GuiOutput::Ls, &data);
    }
    let service = select(&found, &names, false)?
        .into_iter()
        .next()
        .ok_or_else(|| RpcError::new("units_required", json!({}), exit::USAGE))?;
    let base = json!({"lane": found.lane, "unit": service.unit, "box": found.host_box});
    let address = reachable(&base, &service)?;
    let mut session = devlanes_gui_tool::connect(address, CLIENT_NAME)
        .await
        .map_err(|error| unreachable_error(&base, address, &error))?;
    let outcome = drive(&mut session, &command, &chords, &base).await;
    session.close();
    let (output, mut data, frame) = outcome?;
    if let Some(frame) = frame {
        let window = data["window"]["id"].as_u64().unwrap_or_default();
        let target = screenshot_target(&cwd, &command, &found.lane, &service.unit, window);
        data["width"] = json!(frame.width());
        data["height"] = json!(frame.height());
        data["path"] = json!(save(frame, target).await?);
    }
    report(runtime, output, &data)
}

/// The chords of `gui key`, checked before anything connects.
fn chords(command: &GuiCommand) -> Result<Vec<Chord>, RpcError> {
    let GuiCommand::Key { keys, .. } = command else {
        return Ok(Vec::new());
    };
    keys.iter()
        .map(|key| devlanes_gui_tool::parse_chord(key))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            let (chord, name, reason) = match error {
                KeyError::Empty { chord } => (chord, None, "empty"),
                KeyError::Unknown { chord, name } => (chord, Some(name), "unknown"),
                KeyError::NotAChord { chord } => (chord, None, "not_a_chord"),
            };
            RpcError::new(
                "gui_unknown_key",
                json!({"chord": chord, "name": name, "reason": reason, "known": devlanes_gui_tool::known_names()}),
                exit::USAGE,
            )
        })
}

/// Picks the window and does what `command` asks on `session`. Returns the
/// output kind, its data and, for a screenshot, the picture.
async fn drive(
    session: &mut Session,
    command: &GuiCommand,
    chords: &[Chord],
    base: &Value,
) -> Result<(GuiOutput, Value, Option<Frame>), RpcError> {
    let window = session
        .window(command.window())
        .map_err(|error| tool_error(base, error))?;
    let mut data = base.clone();
    data["window"] = json!({"id": window.id.0, "title": window.title});
    // The host drops input to a locked window, so it would not reach the app.
    if !matches!(command, GuiCommand::Screenshot { .. }) && window.lock != LockState::Unlocked {
        let mut error = data.clone();
        error["lock"] = json!(lock_word(window.lock));
        return Err(RpcError::new("gui_window_locked", error, exit::FAILED));
    }
    let id = window.id;
    let done = async {
        let after = match command {
            GuiCommand::Screenshot { .. } => {
                let frame = session.screenshot(id).await?;
                return Ok((GuiOutput::Screenshot, Some(frame)));
            }
            GuiCommand::Click {
                x,
                y,
                button,
                double,
                ..
            } => {
                let buttons = match button {
                    GuiButton::Left => Buttons::LEFT,
                    GuiButton::Right => Buttons::RIGHT,
                    GuiButton::Middle => Buttons::MIDDLE,
                };
                data["action"] = json!("click");
                data["button"] = json!(button.as_str());
                data["double"] = json!(double);
                data["at"] = json!({"x": x, "y": y});
                session.click(id, point(*x, *y), buttons, *double).await?
            }
            GuiCommand::Type { text, .. } => {
                data["action"] = json!("type");
                data["characters"] = json!(text.chars().count());
                session.type_text(id, text).await?
            }
            GuiCommand::Key { .. } => {
                data["action"] = json!("key");
                data["keys"] = json!(chords.iter().map(|chord| &chord.name).collect::<Vec<_>>());
                session.keys(id, chords).await?
            }
            GuiCommand::Scroll { dx, dy, at, .. } => {
                let at = match at.as_deref() {
                    Some([x, y]) => point(*x, *y),
                    _ => devlanes_gui_tool::centre(&window),
                };
                data["action"] = json!("scroll");
                data["delta"] = json!({"x": dx, "y": dy});
                data["at"] = json!({"x": at.x, "y": at.y});
                // Line counts a person types are far inside f32's exact range.
                session.scroll(id, at, *dx as f32, *dy as f32).await?
            }
            GuiCommand::Resize { size, .. } => {
                let (taken, after) = session
                    .resize(id, Size::new(size.width, size.height))
                    .await?;
                data["action"] = json!("resize");
                data["requested"] = json!({"width": size.width, "height": size.height});
                // A window that closed meanwhile has no size to report.
                data["size"] =
                    json!(taken.map(|taken| json!({"width": taken.width, "height": taken.height})));
                after
            }
            GuiCommand::Ls { .. } => After::Open,
        };
        // Said in the result: the input reached the app, and what became of
        // the window by then.
        data["window_after"] = match after {
            After::Open => json!("open"),
            After::Closed => json!("closed"),
            After::Locked(lock) => json!(lock_word(lock)),
            After::Outside(size) => {
                data["window_size"] = json!({"width": size.width, "height": size.height});
                json!("outside")
            }
        };
        Ok::<_, ToolError>((GuiOutput::Input, None))
    };
    let (output, frame) = done.await.map_err(|error| tool_error(base, error))?;
    Ok((output, data, frame))
}

/// A window position from command-line pixels.
fn point(x: u32, y: u32) -> Point {
    Point::new(
        i32::try_from(x).unwrap_or(i32::MAX),
        i32::try_from(y).unwrap_or(i32::MAX),
    )
}

/// Prints `data` and returns the exit code.
fn report(runtime: &Runtime, output: GuiOutput, data: &Value) -> Result<u8, RpcError> {
    let rendered = runtime.reporter().gui(output, data).map_err(|error| {
        RpcError::new(
            "encode",
            json!({"message": error.to_string()}),
            exit::FAILED,
        )
    })?;
    emit(&rendered);
    Ok(rendered.exit_code)
}

/// The lane reference the units name, the same for all of them, and their
/// names. Globs and tags are refused: each command acts on named services.
fn split_units(units: &[String]) -> Result<(Option<String>, Vec<String>), RpcError> {
    let mut lane: Option<String> = None;
    let mut names = Vec::new();
    for unit in units {
        let parsed: UnitRef = unit.parse().map_err(|_| {
            RpcError::new("invalid_reference", json!({"reference": unit}), exit::USAGE)
        })?;
        if parsed.selector.starts_with('+') || parsed.selector.contains(['*', '?', '[']) {
            return Err(RpcError::new(
                "gui_bad_reference",
                json!({"reference": unit, "reason": "selector"}),
                exit::USAGE,
            ));
        }
        let this = parsed.lane.map(|lane| match lane.project {
            Some(project) => format!("{project}@{}", lane.selector),
            None => lane.selector,
        });
        if names.is_empty() {
            lane = this;
        } else if this != lane {
            return Err(RpcError::new(
                "gui_bad_reference",
                json!({"reference": unit, "reason": "lanes"}),
                exit::USAGE,
            ));
        }
        names.push(parsed.selector);
    }
    Ok((lane, names))
}

/// The lane `lane` (or the context's) with its GUI services: from `status`,
/// or for a lane reference this machine does not have, from the boxes in
/// `remote ls`.
async fn find(mut context: Context, lane: Option<String>) -> Result<Found, RpcError> {
    if lane.is_some() {
        context.lane = lane;
    }
    context.env = None;
    let named = context.lane.clone();
    let status = crate::commands::call(Request {
        id: 0,
        method: Method::Status,
        params: json!({"mine": false}),
        context: context.clone(),
    })
    .await;
    match status {
        Ok(data) => local_lane(&data, named.is_some()),
        Err(error) if gui_off_in_daemon(&error) => Err(daemon_off()),
        Err(error) => {
            let Some(reference) = named.filter(|_| error.exit_code == exit::NOT_FOUND) else {
                return Err(error);
            };
            let boxes = crate::commands::call(Request {
                id: 0,
                method: Method::HostList,
                params: json!({}),
                context,
            })
            .await
            .unwrap_or_default();
            lookup::box_lane(&boxes, &reference).ok_or(error)
        }
    }
}

/// Whether an error or a lane's reason is the daemon refusing `gui` keys,
/// which means GUI services are off in the daemon though this CLI has them
/// on. Its message has the loader's [`devlanes_config::GUI_UNSUPPORTED`]: the
/// only sign of that switch in what a daemon answers, released ones included.
fn gui_off_in_daemon(error: &RpcError) -> bool {
    error.data["message"]
        .as_str()
        .is_some_and(|message| message.contains(devlanes_config::GUI_UNSUPPORTED))
}

/// `gui_off_in_daemon`: the daemon runs with GUI services off.
fn daemon_off() -> RpcError {
    RpcError::new("gui_off_in_daemon", json!({}), exit::FAILED)
}

/// The lane of a `status` answer: the one lane a named reference gives, or
/// the request's own (`here`) among the project's.
fn local_lane(data: &Value, named: bool) -> Result<Found, RpcError> {
    let lanes: Vec<LaneSummary> = serde_json::from_value(data["items"].clone()).unwrap_or_default();
    let here = data["here"].as_str();
    let lane = if named {
        lanes.into_iter().next()
    } else {
        if let Some(error) = data.get("here_error").filter(|error| !error.is_null()) {
            return Err(serde_json::from_value(error.clone())
                .unwrap_or_else(|_| RpcError::new("not_found", json!({}), exit::NOT_FOUND)));
        }
        lanes
            .into_iter()
            .find(|lane| Some(lane.reference.as_str()) == here)
    };
    let lane = lane.ok_or_else(|| {
        RpcError::new(
            "not_found",
            json!({"reference": here.unwrap_or_default()}),
            exit::NOT_FOUND,
        )
    })?;
    let services = lookup::lane_services(&lane);
    if services.is_empty() && lane.reason.as_ref().is_some_and(gui_off_in_daemon) {
        return Err(daemon_off());
    }
    Ok(Found {
        lane: lane.reference,
        host_box: None,
        units: lane.units.iter().map(|unit| unit.name.clone()).collect(),
        services,
    })
}

/// The services `names` picks in `found`: by name, or a service's replicas
/// (`desktop` for `desktop.1` and `desktop.2`), every one for `ls` without
/// names. A single command needs exactly one.
fn select(found: &Found, names: &[String], many: bool) -> Result<Vec<Service>, RpcError> {
    if names.is_empty() && many {
        return Ok(found.services.clone());
    }
    let mut picked = Vec::new();
    for name in names {
        let exact: Vec<&Service> = found.services.iter().filter(|s| s.unit == *name).collect();
        let replicas: Vec<&Service> = found
            .services
            .iter()
            .filter(|s| replica_of(&s.unit, name))
            .collect();
        let matched = if exact.is_empty() { replicas } else { exact };
        let exists = found
            .units
            .iter()
            .any(|unit| unit == name || replica_of(unit, name));
        match matched.as_slice() {
            [] if exists => {
                return Err(RpcError::new(
                    "gui_not_gui_service",
                    json!({
                        "lane": found.lane,
                        "unit": name,
                        "box": found.host_box,
                        "services": found.services.iter().map(|s| &s.unit).collect::<Vec<_>>(),
                    }),
                    exit::NOT_FOUND,
                ));
            }
            [one] => picked.push((*one).clone()),
            [_, _, ..] if many => picked.extend(matched.into_iter().cloned()),
            // Nothing by that name, or several replicas for a command that
            // acts on one: the references it could mean.
            _ => {
                let options: Vec<&Service> = if matched.is_empty() {
                    found.services.iter().collect()
                } else {
                    matched
                };
                let candidates: Vec<String> = options
                    .iter()
                    .map(|service| format!("{}:{}", found.lane, service.unit))
                    .collect();
                return Err(RpcError::new(
                    "not_found",
                    json!({"reference": format!("{}:{name}", found.lane), "candidates": candidates}),
                    exit::NOT_FOUND,
                ));
            }
        }
    }
    Ok(picked)
}

/// Whether `unit` is a replica of the service `name`: `name.N`.
fn replica_of(unit: &str, name: &str) -> bool {
    unit.strip_prefix(name)
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Where `service`'s host can be reached, or why not.
fn reachable(base: &Value, service: &Service) -> Result<SocketAddr, RpcError> {
    if let Some(address) = service.address {
        return Ok(address);
    }
    let mut data = base.clone();
    data["state"] = json!(service.state);
    let (code, exit_code) = match service.host.as_str() {
        _ if service.unmirrored.is_some() => {
            data["mirror"] = json!(service.unmirrored);
            ("gui_not_mirrored", exit::FAILED)
        }
        devlanes_proto::GUI_HOST_NONE => ("gui_no_host", exit::FAILED),
        devlanes_proto::GUI_HOST_EXITED => ("gui_host_down", exit::FAILED),
        _ => ("gui_not_running", exit::FAILED),
    };
    Err(RpcError::new(code, data, exit_code))
}

/// `gui_unreachable` for a host whose port did not answer the hello.
fn unreachable_error(base: &Value, address: SocketAddr, error: &ToolError) -> RpcError {
    let mut data = base.clone();
    data["address"] = json!(address.to_string());
    data["message"] = json!(chain(error));
    RpcError::new("gui_unreachable", data, exit::FAILED)
}

/// The error of a failed step, with the windows where it is about them.
fn tool_error(base: &Value, error: ToolError) -> RpcError {
    let mut data = base.clone();
    let listed = |windows: &[WindowInfo]| {
        json!(
            windows
                .iter()
                .map(|window| json!({"id": window.id.0, "title": window.title}))
                .collect::<Vec<_>>()
        )
    };
    let (code, exit_code) = match &error {
        ToolError::UnknownWindow { wanted, windows } => {
            data["window"] = json!(wanted);
            data["windows"] = listed(windows);
            ("gui_window_not_found", exit::NOT_FOUND)
        }
        ToolError::AmbiguousWindow { wanted, windows } => {
            data["window"] = json!(wanted);
            data["windows"] = listed(windows);
            ("gui_window_ambiguous", exit::NOT_FOUND)
        }
        ToolError::NoMainWindow { windows } => {
            data["windows"] = listed(windows);
            ("gui_no_window", exit::FAILED)
        }
        ToolError::OutsideWindow { point, size } => {
            data["at"] = json!({"x": point.x, "y": point.y});
            data["size"] = json!({"width": size.width, "height": size.height});
            ("gui_outside_window", exit::USAGE)
        }
        ToolError::TooLarge { max } => {
            data["max"] = json!(max);
            ("gui_size_too_large", exit::USAGE)
        }
        _ => {
            data["message"] = json!(chain(&error));
            ("gui_failed", exit::FAILED)
        }
    };
    RpcError::new(code, data, exit_code)
}

/// `error` and its sources, joined, since the outer messages are short.
fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    text
}

/// `gui ls`: each service with its host's state and, while it runs, the
/// windows its host lists.
async fn ls(found: &Found, services: &[Service]) -> Value {
    let mut items = Vec::new();
    for service in services {
        let mut item = json!({
            "unit": service.unit,
            "state": service.state,
            "host": service.host,
            "runs": service.runs,
            "address": service.address.map(|address| address.to_string()),
            "real_desktop": null,
            "unmirrored": service.unmirrored,
            "error": null,
            "windows": [],
        });
        if let Some(address) = service.address {
            match devlanes_gui_tool::connect(address, CLIENT_NAME).await {
                Ok(session) => {
                    item["real_desktop"] = json!(session.real_desktop());
                    item["windows"] = json!(
                        session
                            .windows()
                            .iter()
                            .map(window_json)
                            .collect::<Vec<_>>()
                    );
                    session.close();
                }
                Err(error) => {
                    item["host"] = json!("unreachable");
                    item["error"] = json!(chain(&error));
                }
            }
        }
        items.push(item);
    }
    json!({
        "lane": found.lane,
        "box": found.host_box,
        "items": items,
        "truncated": false,
        "next_cursor": null,
    })
}

/// A window as `gui ls` lists it.
fn window_json(window: &WindowInfo) -> Value {
    json!({
        "id": window.id.0,
        "title": window.title,
        "app_id": window.app_id,
        "width": window.size.width,
        "height": window.size.height,
        "kind": match window.kind {
            WindowKind::Toplevel => "toplevel",
            WindowKind::Dialog => "dialog",
            WindowKind::Popup => "popup",
            WindowKind::Tooltip => "tooltip",
        },
        "lock": lock_word(window.lock),
        "parent": window.parent.map(|parent| parent.0),
    })
}

/// A lock state's word in output.
fn lock_word(lock: LockState) -> &'static str {
    match lock {
        LockState::Unlocked => "unlocked",
        LockState::Resizing => "resizing",
        LockState::Waiting => "waiting",
        LockState::Reconnecting => "reconnecting",
        LockState::Frozen => "frozen",
        LockState::Closed => "closed",
        LockState::Locked => "locked",
    }
}

/// Where a screenshot goes.
#[derive(Debug)]
enum Target {
    /// This file name in the screenshots folder of the devlanes home.
    Folder(String),
    /// The `--to` file.
    File(PathBuf),
}

/// `--to` taken from `cwd`, or `LANE-UNIT-WINDOW.png` in the screenshots
/// folder, a name the next screenshot of the window reuses. A box's lane and
/// unit names come from another machine, so the name keeps only characters
/// that are safe in a file name on every OS.
fn screenshot_target(
    cwd: &Path,
    command: &GuiCommand,
    lane: &str,
    unit: &str,
    window: u64,
) -> Target {
    if let GuiCommand::Screenshot { to: Some(to), .. } = command {
        return Target::File(cwd.join(to));
    }
    let name: String = format!("{lane}-{unit}-{window}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    Target::Folder(format!("{name}.png"))
}

/// Writes `frame` as a PNG where `target` says and returns the absolute
/// path. In the screenshots folder the file goes through a temporary file in
/// the folder, so a reader never sees half a picture; a `--to` file is
/// written in place, with the permissions a new file normally gets.
async fn save(frame: Frame, target: Target) -> Result<PathBuf, RpcError> {
    let write = move || -> Result<PathBuf, (String, String)> {
        let failed = |path: &Path| {
            let path = path.display().to_string();
            move |error: &dyn std::fmt::Display| (path.clone(), error.to_string())
        };
        match target {
            Target::Folder(name) => {
                let paths = devlanes_client::Paths::discover()
                    .map_err(|error| failed(Path::new(SCREENSHOT_DIR))(&error))?;
                let dir = paths
                    .private_subdirectory(SCREENSHOT_DIR)
                    .map_err(|error| failed(&paths.home.join(SCREENSHOT_DIR))(&error))?;
                let path = dir.join(name);
                let fail = failed(&path);
                let mut file = tempfile::NamedTempFile::new_in(&dir).map_err(|e| fail(&e))?;
                devlanes_gui_tool::write_png(&frame, std::io::BufWriter::new(file.as_file_mut()))
                    .map_err(|e| fail(&e))?;
                file.persist(&path).map_err(|e| fail(&e))?;
                Ok(path)
            }
            Target::File(path) => {
                let path = std::path::absolute(&path).map_err(|e| failed(&path)(&e))?;
                let fail = failed(&path);
                let file = std::fs::File::create(&path).map_err(|e| fail(&e))?;
                devlanes_gui_tool::write_png(&frame, std::io::BufWriter::new(file))
                    .map_err(|e| fail(&e))?;
                Ok(path)
            }
        }
    };
    let io = |(path, message): (String, String)| {
        RpcError::new(
            "gui_write_failed",
            json!({"path": path, "message": message}),
            exit::FAILED,
        )
    };
    tokio::task::spawn_blocking(write)
        .await
        .map_err(|error| io((String::new(), error.to_string())))?
        .map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devlanes_daemon::gui::lookup::box_lane;

    /// A `remote ls` answer with one box lane `acme@2` whose `desktop` has a
    /// GUI port in `state`, mirrored when `bound`.
    fn boxes(state: &str, bound: bool) -> Value {
        json!({"items": [{
            "host": "hetzner",
            "state": "connected",
            "ports": [{
                "port": 26012, "lane": "acme@2", "unit": "desktop", "kind": "service",
                "state": if bound { "bound" } else { "in_use" },
                "addresses": if bound { json!(["127.0.0.1", "::1"]) } else { json!([]) },
                "conflicts": if bound { json!([]) } else {
                    json!([{"message": "port 26012 is not mirrored because node (pid 7) uses it here"}])
                },
            }],
            "registry": {"lanes": [], "units": [
                {"key": {"lane": {"project": "acme", "lane": "2"}, "kind": "service", "name": "desktop"},
                 "hidden": false, "state": state,
                 "ports": [{"name": "default", "port": 26010}, {"name": "devlanes_gui", "port": 26012}],
                 "hosts": []},
                {"key": {"lane": {"project": "acme", "lane": "2"}, "kind": "service", "name": "api"},
                 "hidden": false, "state": state, "ports": [{"name": "default", "port": 26011}], "hosts": []},
            ]},
        }]})
    }

    /// The loader's error for a `gui: true` service with GUI services off, as
    /// the daemon sends it (`config_error`, the file's text in `message`'s
    /// neighbours), tells that the daemon has them off; another config error
    /// in a file that mentions `gui` does not.
    #[test]
    fn a_daemon_with_gui_services_off_is_recognised() {
        let dir = tempfile::tempdir().expect("dir");
        let load = |yaml: &str| {
            let file = dir.path().join("devlanes.yaml");
            std::fs::write(&file, yaml).expect("write");
            let error = devlanes_config::check(&file, devlanes_config::ProjectOptions::default())
                .expect_err("refused");
            RpcError::new(
                "config_error",
                json!({"message": error.message, "file": error.location.file, "line": error.location.line}),
                1,
            )
        };
        let off = load("project: acme\nservices:\n  desktop:\n    run: [app]\n    gui: true\n");
        assert!(gui_off_in_daemon(&off), "{off:?}");
        let other = load(&format!(
            "project: acme\n# {}\nservices:\n  desktop:\n    runn: [app]\n",
            devlanes_config::GUI_UNSUPPORTED
        ));
        assert!(!gui_off_in_daemon(&other), "{other:?}");
    }

    /// A box lane's GUI service is reached through the port its mirror binds
    /// here; a port something else holds here, or a stopped unit, is not.
    #[test]
    fn box_services_use_the_mirrored_port() {
        let found = box_lane(&boxes("ready", true), "acme@2").expect("the box lane");
        assert_eq!(found.host_box.as_deref(), Some("hetzner"));
        assert_eq!(found.units, ["desktop", "api"]);
        let [desktop] = found.services.as_slice() else {
            panic!("one GUI service: {found:?}");
        };
        assert_eq!(
            desktop.address,
            Some("127.0.0.1:26012".parse().expect("address"))
        );

        let held = box_lane(&boxes("ready", false), "acme@2").expect("the box lane");
        let error = reachable(&json!({}), &held.services[0]).expect_err("not mirrored");
        assert_eq!(error.code, "gui_not_mirrored");
        assert!(
            error.data["mirror"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("node (pid 7)")
        );

        let stopped = box_lane(&boxes("stopped", false), "acme@2").expect("the box lane");
        assert_eq!(
            reachable(&json!({}), &stopped.services[0])
                .expect_err("stopped")
                .code,
            "gui_not_running"
        );
        assert!(box_lane(&boxes("ready", true), "acme@3").is_none());

        // Two boxes with the lane: the mirrored one, else the one `remote
        // use` chose.
        let mut two = boxes("ready", false);
        let mut other = two["items"][0].clone();
        other["host"] = json!("fork");
        two["items"]
            .as_array_mut()
            .expect("items")
            .push(other.clone());
        assert_eq!(
            box_lane(&two, "acme@2").and_then(|found| found.host_box),
            Some("hetzner".into())
        );
        two["items"][1]["chosen_lanes"] = json!(["acme@2"]);
        assert_eq!(
            box_lane(&two, "acme@2").and_then(|found| found.host_box),
            Some("fork".into())
        );
        two["items"][0] = boxes("ready", true)["items"][0].clone();
        assert_eq!(
            box_lane(&two, "acme@2").and_then(|found| found.host_box),
            Some("hetzner".into())
        );
    }
}
