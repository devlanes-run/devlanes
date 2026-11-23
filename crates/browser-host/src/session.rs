//! One extension port: `hello` first, then `watch`, `use` and `stop_and_use`.

use std::{sync::Arc, time::Duration};

use devlanes_client::{Method, RpcError};
use devlanes_proto::{
    LaneSummary, Page, Selection, events,
    remote::{self, UseOutcome, WaitsFor},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{Mutex, mpsc},
    time::{Instant, sleep, sleep_until},
};

use crate::{
    daemon::{self, CallError, Daemon, Queries, Unavailable},
    frame::{self, FrameError},
    message::{
        Call, DaemonState, DaemonStatus, ErrorData, ErrorReply, Event, HelloResult, HostError,
        Notice, Reply, Request, UseParams, UseResult, WatchResult, container_commands,
        stop_commands,
    },
    view::{self, HostList, Hosts, View},
};

/// The events after which the view may have changed.
const REFRESH_EVENTS: &[&str] = &[
    events::LANE_CHANGED,
    events::LANE_REMOVED,
    events::LANE_WARNING,
    events::UNIT_CHANGED,
    remote::STATE_EVENT,
    remote::WARNING_EVENT,
    remote::MIRROR_EVENT,
    events::DAEMON_STOPPING,
    events::RESYNC,
];

/// How long the host waits after the first event of a burst before it
/// composes the view again.
const THROTTLE: Duration = Duration::from_millis(100);

/// How often the host tries to reach a daemon that is down or outdated.
const RECONNECT: Duration = Duration::from_secs(2);

/// How long `stop_and_use` waits for the daemon to stop counting the local
/// lane as running, as the TUI's `m` does.
const SWITCH_WAIT: Duration = Duration::from_secs(30);

/// How often `stop_and_use` asks again while it waits.
const SWITCH_RETRY: Duration = Duration::from_secs(1);

/// Frames waiting for the writer. Replies are small and the view goes out
/// at most once per [`THROTTLE`], so this only fills when Chrome stops reading.
const OUT_QUEUE: usize = 64;

/// What the session's tasks share.
struct Inner<D> {
    daemon: D,
    /// The request connection, opened on first use and dropped when it breaks.
    queries: Mutex<Option<Box<dyn Queries>>>,
    out: mpsc::Sender<Vec<u8>>,
}

/// Serves one extension port until the extension closes it. `Ok` means a
/// clean end of input.
pub async fn serve<D, R, W>(daemon: D, mut reader: R, mut writer: W) -> Result<(), FrameError>
where
    D: Daemon,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (out, mut queue) = mpsc::channel::<Vec<u8>>(OUT_QUEUE);
    // One task owns stdout, so frames from concurrent requests never interleave.
    let mut writing = tokio::spawn(async move {
        while let Some(body) = queue.recv().await {
            frame::write(&mut writer, &body).await?;
        }
        Ok::<(), FrameError>(())
    });
    let inner = Arc::new(Inner {
        daemon,
        queries: Mutex::new(None),
        out,
    });
    let mut hello: Option<DaemonStatus> = None;
    let mut watching = false;
    let mut tasks = tokio::task::JoinSet::new();
    let ended = loop {
        let body = tokio::select! {
            body = frame::read(&mut reader) => body,
            written = &mut writing => break match written {
                Ok(result) => result,
                Err(error) => Err(FrameError::Io(std::io::Error::other(error))),
            },
        };
        let body = match body {
            Ok(Some(body)) => body,
            Ok(None) => break Ok(()),
            Err(error) => break Err(error),
        };
        let request = match Request::parse(&body) {
            Ok(request) => request,
            // A reply without an id can't be matched to anything, so a
            // message without one only gets a line on stderr.
            Err(invalid) => {
                match invalid.id {
                    Some(id) => {
                        let error = HostError::plain("invalid_request", invalid.message);
                        inner.send_error(id, error).await;
                    }
                    None => eprintln!("devlanes browser host: {}", invalid.message),
                }
                continue;
            }
        };
        let id = request.id;
        match (request.call, hello.clone()) {
            (Call::Hello { protocol, .. }, _) if !crate::message::SUPPORTED.contains(&protocol) => {
                inner
                    .send_error(id, HostError::protocol_mismatch(protocol))
                    .await;
            }
            (Call::Hello { .. }, _) => {
                let status = inner.probe().await;
                inner
                    .send(&Reply {
                        id,
                        result: HelloResult {
                            protocol: 1,
                            devlanes: devlanes_version::VERSION.into(),
                            daemon: status.state,
                        },
                    })
                    .await;
                hello = Some(status);
            }
            (_, None) => inner.send_error(id, HostError::hello_required()).await,
            (Call::Watch, Some(status)) => {
                inner
                    .send(&Reply {
                        id,
                        result: WatchResult {},
                    })
                    .await;
                // One view stream per port: a second watch only gets its reply.
                if !watching {
                    watching = true;
                    spawn_reaped(&mut tasks, watch(inner.clone(), status));
                }
            }
            (Call::Use(params), Some(_)) => {
                let inner = inner.clone();
                spawn_reaped(&mut tasks, async move {
                    let result = inner.use_lane(&params).await;
                    inner.answer(id, result).await;
                });
            }
            (Call::StopAndUse(params), Some(_)) => {
                let inner = inner.clone();
                spawn_reaped(&mut tasks, async move {
                    let result = inner.stop_and_use(&params).await;
                    inner.answer(id, result).await;
                });
            }
            (Call::Unknown(method), Some(_)) => {
                inner
                    .send_error(id, HostError::unknown_method(&method))
                    .await;
            }
        }
    };
    tasks.abort_all();
    ended
}

/// Spawns `task` into `tasks` after dropping the tasks that already ended. A
/// `JoinSet` keeps every finished task until it is joined, so a port that
/// sends `use` after `use` would otherwise grow without bound.
fn spawn_reaped(
    tasks: &mut tokio::task::JoinSet<()>,
    task: impl std::future::Future<Output = ()> + Send + 'static,
) {
    while tasks.try_join_next().is_some() {}
    tasks.spawn(task);
}

/// The `daemon` status for a daemon the host can't use.
fn unavailable_status(why: &Unavailable) -> DaemonStatus {
    match why {
        Unavailable::Stopped(message) => DaemonStatus {
            state: DaemonState::Stopped,
            message: Some(message.clone()),
        },
        Unavailable::Outdated => DaemonStatus {
            state: DaemonState::Outdated,
            message: Some(daemon::OUTDATED.into()),
        },
    }
}

/// The `daemon` status of a daemon the host reaches.
fn running() -> DaemonStatus {
    DaemonStatus {
        state: DaemonState::Running,
        message: None,
    }
}

/// Sends the view now and after every change, and the daemon's state and
/// notices, until the port closes. It reconnects every [`RECONNECT`] while
/// the daemon is down or outdated. `reported` is what `hello` said.
async fn watch<D: Daemon>(inner: Arc<Inner<D>>, mut reported: DaemonStatus) {
    let mut sent: Option<View> = None;
    loop {
        let mut subscription = match inner.daemon.events().await {
            Ok(subscription) => subscription,
            Err(why) => {
                inner.report(&mut reported, unavailable_status(&why)).await;
                sleep(RECONNECT).await;
                continue;
            }
        };
        inner.report(&mut reported, running()).await;
        inner.refresh(&mut sent).await;
        // Refreshes are throttled: the first event of a burst sets a 100 ms
        // deadline and later ones leave it, so a daemon that changes all the
        // time still gets a view every 100 ms.
        let mut due: Option<Instant> = None;
        loop {
            tokio::select! {
                event = subscription.next() => {
                    let Some(event) = event else { break };
                    if event.event == events::DAEMON_STOPPING {
                        inner
                            .report(&mut reported, unavailable_status(&Unavailable::Stopped(daemon::STOPPED.into())))
                            .await;
                    }
                    if event.event == remote::WARNING_EVENT
                        && let Some(notice) = Notice::of_warning(&event.data)
                    {
                        inner.send(&Event::Notice(notice)).await;
                    }
                    if due.is_none() && REFRESH_EVENTS.contains(&event.event.as_str()) {
                        due = Some(Instant::now() + THROTTLE);
                    }
                }
                () = sleep_until(due.unwrap_or_else(Instant::now)), if due.is_some() => {
                    due = None;
                    inner.refresh(&mut sent).await;
                }
            }
        }
        // The daemon went away or restarted, and the request connection went
        // with it.
        *inner.queries.lock().await = None;
    }
}

impl<D: Daemon> Inner<D> {
    /// Encodes `message` and queues it for the writer. A message over the
    /// frame limit is dropped with a line on stderr, since Chrome would close
    /// the port on it.
    async fn send(&self, message: &impl Serialize) -> bool {
        let body = match serde_json::to_vec(message) {
            Ok(body) => body,
            Err(error) => {
                eprintln!("devlanes browser host: a message didn't encode: {error}");
                return false;
            }
        };
        if let Err(error) = frame::check_out(&body) {
            eprintln!("devlanes browser host: {error}");
            return false;
        }
        self.out.send(body).await.is_ok()
    }

    /// Sends an error reply.
    async fn send_error(&self, id: u64, error: HostError) {
        self.send(&ErrorReply { id, error }).await;
    }

    /// Sends the reply of `use` or `stop_and_use`.
    async fn answer(&self, id: u64, result: Result<UseResult, HostError>) {
        match result {
            Ok(result) => {
                self.send(&Reply { id, result }).await;
            }
            Err(error) => self.send_error(id, error).await,
        }
    }

    /// Sends a `daemon` event when `status` differs from what was `reported`.
    async fn report(&self, reported: &mut DaemonStatus, status: DaemonStatus) {
        if *reported != status {
            self.send(&Event::Daemon(status.clone())).await;
            *reported = status;
        }
    }

    /// The daemon's state for `hello`, opening the request connection.
    async fn probe(&self) -> DaemonStatus {
        let mut queries = self.queries.lock().await;
        if queries.is_some() {
            return running();
        }
        match self.daemon.queries().await {
            Ok(connection) => {
                *queries = Some(connection);
                running()
            }
            Err(why) => unavailable_status(&why),
        }
    }

    /// Sends one request on the request connection, opening it when needed. A
    /// broken connection is dropped so the next request opens a new one.
    async fn call(
        &self,
        method: Method,
        params: Value,
        lane: Option<String>,
    ) -> Result<Value, CallError> {
        let mut queries = self.queries.lock().await;
        let connection = match queries.take() {
            Some(connection) => connection,
            None => self
                .daemon
                .queries()
                .await
                .map_err(CallError::Unavailable)?,
        };
        let connection = queries.insert(connection);
        let result = connection.call(method, params, daemon::context(lane)).await;
        if matches!(result, Err(CallError::Lost(_))) {
            *queries = None;
        }
        result
    }

    /// [`Inner::call`] with the reply read as `T`.
    async fn fetch<T: DeserializeOwned>(
        &self,
        method: Method,
        params: Value,
    ) -> Result<T, CallError> {
        let reply = self.call(method, params, None).await?;
        serde_json::from_value(reply).map_err(|error| {
            CallError::Lost(format!(
                "the daemon's {method:?} reply didn't parse: {error}"
            ))
        })
    }

    /// Composes the view and sends it when it differs from the last one sent.
    async fn refresh(&self, sent: &mut Option<View>) {
        let composed = async {
            let status: Page<LaneSummary> =
                self.fetch(Method::Status, json!({"all": true})).await?;
            let hosts: Hosts = self.fetch(Method::Hosts, json!({"all": true})).await?;
            let host_list: HostList = self.fetch(Method::HostList, json!({})).await?;
            Ok::<_, CallError>(view::compose(&status, &hosts, &host_list))
        };
        let view = match composed.await {
            Ok(view) => view,
            // The subscription sees the daemon go away and reports it.
            Err(error) => {
                eprintln!("devlanes browser host: the view couldn't be built: {error:?}");
                return;
            }
        };
        if sent.as_ref() == Some(&view) {
            return;
        }
        if self.send(&Event::View(view.clone())).await {
            *sent = Some(view);
        }
    }

    /// Sends `remote.use` for `params`.
    async fn remote_use(&self, params: &UseParams) -> Result<UseResult, CallError> {
        let reply = self
            .call(
                Method::RemoteUse,
                json!({"lane": params.lane, "host": params.host}),
                None,
            )
            .await?;
        Ok(use_result(params, &reply))
    }

    /// `use`: one `remote.use`.
    async fn use_lane(&self, params: &UseParams) -> Result<UseResult, HostError> {
        self.remote_use(params).await.map_err(use_error)
    }

    /// `stop_and_use`, as the TUI's `m` on a lane a running local lane keeps
    /// out: stop the local lane, then ask again once a second for up to 30 s
    /// while the daemon still counts it as running. The lane's own containers
    /// keep its ports after a stop, so then it only stops the processes and
    /// returns the commands that stop the containers. It never runs docker.
    async fn stop_and_use(&self, params: &UseParams) -> Result<UseResult, HostError> {
        let refusal = match self.remote_use(params).await {
            Ok(result) => return Ok(result),
            Err(CallError::Rpc(error)) if error.code == "local_lane_running" => error,
            Err(error) => return Err(use_error(error)),
        };
        let Some(local) = refusal.data["local_lane"].as_str().map(str::to_owned) else {
            return Err(use_error(CallError::Rpc(refusal)));
        };
        if refusal.data["holds"] == "processes" {
            let selection = serde_json::to_value(Selection::default()).unwrap_or_default();
            self.call(Method::Stop, selection, Some(local.clone()))
                .await
                .map_err(|error| stop_error(&local, error))?;
        }
        let containers = container_commands(&refusal.data);
        if !containers.is_empty() {
            return Err(containers_hold_ports(&local, containers));
        }
        let deadline = Instant::now() + SWITCH_WAIT;
        loop {
            match self.remote_use(params).await {
                Ok(result) => return Ok(result),
                Err(CallError::Rpc(error)) if error.code == "local_lane_running" => {
                    let containers = container_commands(&error.data);
                    if !containers.is_empty() {
                        return Err(containers_hold_ports(&local, containers));
                    }
                    if Instant::now() >= deadline {
                        return Err(use_error(CallError::Rpc(error)));
                    }
                    sleep(SWITCH_RETRY).await;
                }
                Err(error) => return Err(use_error(error)),
            }
        }
    }
}

/// The result of `use` from the daemon's `remote.use` reply, read with
/// [`UseOutcome::of_reply`] as every client reads it.
fn use_result(params: &UseParams, reply: &Value) -> UseResult {
    let text = |key: &str| reply.get(key).and_then(Value::as_str).map(str::to_owned);
    let (mirrored, waits_for) = match UseOutcome::of_reply(reply) {
        UseOutcome::Mirrored => (true, None),
        UseOutcome::Pending(waits_for) => (false, Some(waits_for)),
        UseOutcome::NothingToMirror | UseOutcome::NotMirrored => (false, None::<WaitsFor>),
    };
    UseResult {
        lane: text("lane").unwrap_or_else(|| params.lane.clone()),
        host: text("host"),
        previous: text("previous"),
        mirrored,
        pending: waits_for.is_some(),
        waits_for,
    }
}

/// The daemon's own `message` in an error's data.
fn daemon_message(error: &RpcError) -> Option<String> {
    error.data["message"].as_str().map(str::to_owned)
}

/// The host error for a failed request that has no mapping of its own:
/// `daemon_failed`, with the daemon's code when it answered, else
/// `daemon_not_running` when no daemon runs, `daemon_outdated` when it is too
/// old for the host, and `daemon_connection_lost` when the connection broke
/// or the reply didn't parse.
fn unmapped(error: CallError) -> HostError {
    match error {
        CallError::Rpc(error) => {
            let message = daemon_message(&error).unwrap_or_else(|| {
                format!("devlanes answered {}. Run devlanes doctor.", error.code)
            });
            HostError::daemon_failed(&error.code, message)
        }
        CallError::Unavailable(Unavailable::Stopped(message)) => {
            HostError::daemon_failed("daemon_not_running", message)
        }
        CallError::Unavailable(Unavailable::Outdated) => {
            HostError::daemon_failed("daemon_outdated", daemon::OUTDATED)
        }
        CallError::Lost(detail) => HostError::daemon_failed(
            "daemon_connection_lost",
            format!("The connection to devlanes broke: {detail}. Try again."),
        ),
    }
}

/// The host error for a failed `remote.use`. Its refusals keep their code
/// and the daemon's message.
fn use_error(error: CallError) -> HostError {
    let CallError::Rpc(error) = error else {
        return unmapped(error);
    };
    match error.code.as_str() {
        "local_lane_running" => {
            let message = daemon_message(&error).unwrap_or_else(|| {
                "A lane runs on this machine with the same hostnames. Stop it, then switch again."
                    .into()
            });
            HostError {
                code: error.code.clone(),
                message,
                data: Some(ErrorData::LocalLaneRunning {
                    holds: error.data["holds"].as_str().unwrap_or("processes").into(),
                    local_lane: error.data["local_lane"].as_str().unwrap_or_default().into(),
                    commands: stop_commands(&error.data),
                }),
            }
        }
        "box_unknown" | "lane_not_on_box" | "local_lane_unknown" => {
            let message = daemon_message(&error)
                .unwrap_or_else(|| format!("devlanes refused the switch with {}.", error.code));
            HostError::plain(&error.code, message)
        }
        _ => unmapped(CallError::Rpc(error)),
    }
}

/// The host error for a failed stop of local lane `local`.
fn stop_error(local: &str, error: CallError) -> HostError {
    match error {
        CallError::Rpc(error) => {
            let message = daemon_message(&error).unwrap_or_else(|| {
                format!(
                    "devlanes couldn't stop {local} ({}). Run devlanes stop --lane {local} in a terminal to see why.",
                    error.code
                )
            });
            HostError::daemon_failed(&error.code, message)
        }
        error => unmapped(error),
    }
}

/// `containers_hold_ports`: the processes of `local` are stopped, and its
/// own containers still hold the ports.
fn containers_hold_ports(local: &str, commands: Vec<String>) -> HostError {
    HostError {
        code: "containers_hold_ports".into(),
        message: format!(
            "{local}'s own containers keep its ports. Stop them with {}, then switch again.",
            commands.join(" and ")
        ),
        data: Some(ErrorData::Commands { commands }),
    }
}

#[cfg(test)]
mod tests;
