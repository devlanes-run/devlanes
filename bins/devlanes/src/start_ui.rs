//! `devlanes start` (and `restart`, `reload`, `lane resume`, `lane new --start`) inside
//! the TUI, as mprocs does. The request goes out first: a start the daemon
//! refuses at once (no config, unknown unit) is printed as a CLI error with its
//! usual exit status, and the TUI only opens once the start is underway.

use std::time::Duration;

use devlanes_cli::{Hint, Rendered, Runtime};
use devlanes_client::{Client, Paths};
use devlanes_proto::{Context, Method, Request, RpcError};
use devlanes_tui::{
    ApiError,
    start::{StartExit, StartFrame, StartHandoff, await_start, follow_start},
};
use serde_json::{Value, json};

use crate::{driver::rpc_error, output::emit};

/// How long a start may stay silent before the TUI opens anyway. Refusals come
/// back within milliseconds; a start that says nothing for this long is running.
const OPEN_AFTER: Duration = Duration::from_millis(300);

/// Exit status when the user leaves before the start's result, as for a
/// Ctrl-C detach: the daemon keeps starting.
const DETACHED: u8 = 130;

/// Whether `tui.open_on_start` in the user config allows it; true when unset
/// or invalid (with a warning).
pub fn open_on_start(paths: &Paths) -> bool {
    crate::user_config::display(paths).tui.open_on_start
}

/// The command's verb, shown in the TUI's header.
fn label(method: Method) -> &'static str {
    match method {
        Method::Restart => "restart",
        Method::Reload => "reload",
        Method::Resume => "resume",
        Method::LaneNew => "new lane",
        _ => "start",
    }
}

/// Applies an installed update, sends `request` to the daemon (spawning one
/// when none runs), then runs the TUI on it. Returns 0 when the start was
/// ready, 1 when it failed, 130 when the user quit before its result; a TUI
/// that closed to update, reload or set up returns what that did.
pub async fn run(
    runtime: &Runtime,
    paths: &Paths,
    request: Request,
    hint: impl Fn(&RpcError) -> Option<Hint>,
) -> Result<u8, RpcError> {
    // Before connecting: applying an update restarts the daemon.
    crate::update::startup(runtime, paths).await;
    let mut client = Client::connect_or_spawn(paths).await.map_err(rpc_error)?;
    let mut params = request.params.clone();
    params["events"] = json!(true);
    // The lane the daemon started, for the TUI's scope.
    let accepted = matches!(
        request.method,
        Method::Start | Method::Restart | Method::Reload
    ) && client.supports(devlanes_proto::features::START_ACCEPTED);
    if accepted {
        params["accepted"] = json!(true);
    }
    let id = client
        .begin_request(request.method, params, request.context.clone())
        .await
        .map_err(rpc_error)?;
    let stream = follow_start(client, id);
    let verb = label(request.method).into();
    let waited = tokio::select! {
        waited = await_start(verb, stream, OPEN_AFTER, accepted) => waited,
        _ = tokio::signal::ctrl_c() => {
            keeps_running();
            return Ok(DETACHED);
        }
    };
    let handoff = match waited {
        Ok(handoff) => handoff,
        Err(error) => {
            let error = api_error(error);
            let rendered = runtime.reporter().error(&error, hint(&error).as_ref());
            emit(&rendered);
            return Ok(rendered.exit_code);
        }
    };
    if let Some(data) = nothing_reloaded(request.method, &handoff) {
        return crate::commands::render_result(runtime, request.method, data, None);
    }
    // Scoped to the lane the daemon started: a cwd-only first `status` would
    // register the cwd's project when the start named another project's lane.
    // Without `start.accepted` (an older daemon, `resume`, `lane new`) the
    // request's own context.
    let scope = Context {
        lane: handoff.lane.clone().or(request.context.lane),
        ..request.context
    };
    let options = devlanes_tui::Options {
        scope: Some(scope),
        ..crate::wiring::tui_options(paths).await
    };
    let (exit, relaunch) = devlanes_tui::run_start(paths.clone(), options, handoff)
        .await
        .map_err(crate::wiring::tui_error)?;
    // The start goes on in the daemon (an update's restart starts its lanes
    // again); the TUI that opens next shows it.
    if let Some(relaunch) = relaunch {
        return crate::relaunch::after(runtime, paths, relaunch).await;
    }
    Ok(match exit {
        StartExit::Ready => 0,
        StartExit::Failed => 1,
        StartExit::Pending => {
            keeps_running();
            DETACHED
        }
    })
}

/// The result of a `reload` that answered before the TUI opened without starting
/// anything (nothing changed, or only removed units stopped). It prints as the
/// CLI result, since the TUI would open only to show one line.
fn nothing_reloaded(method: Method, handoff: &StartHandoff) -> Option<&Value> {
    let Some(StartFrame::Done(Ok(data))) = handoff.frames.last() else {
        return None;
    };
    let empty = |key: &str| {
        data.get(key)
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
    };
    (method == Method::Reload && empty("started") && empty("failures")).then_some(data)
}

/// Says that leaving did not stop the start.
fn keeps_running() {
    emit(&Rendered {
        stderr: "the start keeps running in the daemon; devlanes status shows it\n".into(),
        ..Rendered::default()
    });
}

/// The TUI's view of a failed request as the CLI's error.
fn api_error(error: ApiError) -> RpcError {
    match error {
        ApiError::Rpc(error) => error,
        ApiError::Disconnected(message) => {
            RpcError::new("daemon_unreachable", json!({"message": message}), 4)
        }
        ApiError::Decode(message) => RpcError::new("bad_response", json!({"message": message}), 1),
        ApiError::Incompatible { peer } => rpc_error(devlanes_client::Error::Incompatible { peer }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reload whose result came first and started nothing prints as text; one
    /// that started a unit, or any other start, opens the TUI.
    #[test]
    fn a_reload_that_started_nothing_skips_the_tui() {
        let handoff = |data: Value| StartHandoff {
            label: "reload".into(),
            lane: None,
            frames: vec![StartFrame::Done(Ok(data))],
            stream: tokio::sync::mpsc::channel(1).1,
        };
        let nothing = handoff(json!({"lane": "acme@2", "started": [], "failures": []}));
        assert!(nothing_reloaded(Method::Reload, &nothing).is_some());
        assert!(nothing_reloaded(Method::Start, &nothing).is_none());
        let started = handoff(json!({"lane": "acme@2", "started": ["acme@2:api"]}));
        assert!(nothing_reloaded(Method::Reload, &started).is_none());
    }
}
