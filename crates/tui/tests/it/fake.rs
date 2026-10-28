//! An in-memory daemon for TUI tests.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use bytes::Bytes;
use devlanes_client::restart::Progress;
use devlanes_proto::{
    Actor, ActorKind, AttachParams, AttachResult, Event, LaneSummary, LogLine, LogQuery, Method,
    Page, RpcError, RunParams, Selection, UnitSummary,
};
use devlanes_tui::{
    ApiError, AttachEvent, AttachInput, AttachSession, ConnectEvent, ConnectSession, DaemonApi,
    EventStream, Metrics, MetricsQuery,
    api::{ProfileInfo, ProfileList, ProgressSender},
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// Records every call and serves canned lanes, metrics and terminal output.
#[derive(Default)]
pub struct FakeApi {
    pub lanes: Mutex<Vec<LaneSummary>>,
    pub metrics: Metrics,
    pub screen: Vec<u8>,
    /// When set, start/stop/restart fail with this daemon error.
    pub control_error: Option<RpcError>,
    /// A whole-lane stop leaves the lane idle in what `status` answers next.
    pub stops: bool,
    /// When set, `lane.clean` fails with this daemon error.
    pub clean_error: Option<RpcError>,
    /// The daemon log the TUI tails.
    pub log_path: Option<PathBuf>,
    /// `DEVLANES_HOME`, where config.yaml is written.
    pub home: Option<PathBuf>,
    /// When set, `profile.list` fails with this daemon error.
    pub profiles_error: Option<RpcError>,
    /// What `host.list` answers; no boxes unless a test sets some.
    pub boxes: Mutex<Option<Value>>,
    /// What `remote.clients` answers; unset, the daemon does not know the
    /// method and the boxes view lists no clients.
    pub clients: Mutex<Option<Value>>,
    /// What an attached `connect` asks, in order, as `(kind, prompt)`: each
    /// waits for its answer, recorded as `answer <stream> <text>`, and the
    /// link connects after the last one.
    pub prompts: Mutex<Vec<(String, String)>>,
    /// What `remote.pair` answers; unset, a code valid for ten minutes.
    pub pairing: Mutex<Option<Result<Value, RpcError>>>,
    /// Shared, so a `connect` session records its answers after the call.
    pub calls: Arc<Mutex<Vec<String>>>,
    pub events: Mutex<Option<mpsc::Sender<Result<Event, ApiError>>>>,
    /// Keeps attach output channels open so streams do not end on their own.
    outputs: Mutex<Vec<mpsc::Sender<AttachEvent>>>,
    inputs: Mutex<Vec<mpsc::Receiver<AttachInput>>>,
}

impl FakeApi {
    /// A fake with the given lanes.
    pub fn with_lanes(lanes: Vec<LaneSummary>) -> Self {
        Self {
            lanes: Mutex::new(lanes),
            ..Self::default()
        }
    }

    /// Records a call.
    fn record(&self, call: String) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(call);
        }
    }

    /// Calls recorded so far.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// Pushes an event to the open subscription, if any.
    pub async fn emit(&self, event: &str, data: Value) -> bool {
        let sender = self.events.lock().ok().and_then(|s| s.clone());
        match sender {
            Some(sender) => sender
                .send(Ok(Event {
                    seq: 1,
                    event: event.into(),
                    data,
                }))
                .await
                .is_ok(),
            None => false,
        }
    }
}

#[async_trait]
impl DaemonApi for FakeApi {
    async fn status(&self) -> Result<Vec<LaneSummary>, ApiError> {
        self.record("status".into());
        Ok(self.lanes.lock().map(|l| l.clone()).unwrap_or_default())
    }

    async fn subscribe(&self) -> Result<EventStream, ApiError> {
        self.record("subscribe".into());
        let (sender, receiver) = mpsc::channel(16);
        if let Ok(mut events) = self.events.lock() {
            *events = Some(sender);
        }
        Ok(EventStream::new(receiver))
    }

    async fn attach(&self, lane: String, params: AttachParams) -> Result<AttachSession, ApiError> {
        self.record(format!("attach {} {lane}", params.unit));
        let (output_tx, output_rx) = mpsc::channel(16);
        let (input_tx, input_rx) = mpsc::channel(16);
        if !self.screen.is_empty() {
            let screen = AttachEvent::Output(Bytes::from(self.screen.clone()));
            output_tx
                .send(screen)
                .await
                .map_err(|error| ApiError::Decode(error.to_string()))?;
        }
        if let (Ok(mut outputs), Ok(mut inputs)) = (self.outputs.lock(), self.inputs.lock()) {
            outputs.push(output_tx);
            inputs.push(input_rx);
        }
        Ok(AttachSession::new(
            AttachResult {
                stream_id: 7,
                unit: params.unit,
                lane,
            },
            output_rx,
            input_tx,
        ))
    }

    async fn control(
        &self,
        method: Method,
        lane: String,
        selection: Selection,
    ) -> Result<Value, ApiError> {
        if selection.profiles.is_empty() {
            self.record(format!("{method:?} {lane} {:?}", selection.units));
        } else {
            self.record(format!(
                "{method:?} {lane} {:?} profiles={:?}",
                selection.units, selection.profiles
            ));
        }
        if let Some(error) = &self.control_error {
            return Err(ApiError::Rpc(error.clone()));
        }
        if self.stops
            && method == Method::Stop
            && selection.units.is_empty()
            && let Ok(mut lanes) = self.lanes.lock()
            && let Some(stopped) = lanes.iter_mut().find(|l| l.reference == lane)
        {
            stopped.state = "idle".into();
            for unit in &mut stopped.units {
                unit.state = "stopped".into();
            }
        }
        Ok(json!({}))
    }

    async fn run_task(
        &self,
        lane: String,
        params: RunParams,
        yes: bool,
    ) -> Result<Value, ApiError> {
        let yes = if yes { " yes" } else { "" };
        self.record(format!("run {} {lane}{yes}", params.task));
        Ok(json!({}))
    }

    async fn logs(&self, lane: String, query: LogQuery) -> Result<Page<LogLine>, ApiError> {
        self.record(format!("logs {lane} {:?}", query.units));
        Ok(Page::complete(Vec::new()))
    }

    async fn metrics(&self, query: MetricsQuery) -> Result<Metrics, ApiError> {
        self.record(format!("metrics {}", query.range_ms));
        Ok(self.metrics.clone())
    }

    async fn daemon_status(&self) -> Result<Value, ApiError> {
        self.record("daemon.status".into());
        Ok(daemon_status())
    }

    async fn doctor(&self) -> Result<Value, ApiError> {
        self.record("doctor".into());
        Ok(doctor_facts())
    }

    async fn clean_lane(&self, lane: String) -> Result<Value, ApiError> {
        self.record(format!("lane.clean {lane}"));
        if let Some(error) = &self.clean_error {
            return Err(ApiError::Rpc(error.clone()));
        }
        // A cleaned lane keeps its worktree but loses its units and records.
        let mut stopped = Vec::new();
        if let Ok(mut lanes) = self.lanes.lock()
            && let Some(found) = lanes.iter_mut().find(|l| l.reference == lane)
        {
            stopped = found.units.drain(..).map(|u| u.name).collect();
            found.state = "idle".into();
        }
        Ok(json!({"lane": lane, "stopped": stopped, "tasks": [], "teardown": [], "infra": null}))
    }

    async fn lane_gc(&self) -> Result<Value, ApiError> {
        self.record("lane.gc".into());
        Ok(json!({"cleaned": ["acme@4"], "offsets_removed": 0}))
    }

    async fn stop_daemon(&self, progress: ProgressSender) -> Result<Value, ApiError> {
        self.record("daemon.shutdown".into());
        for step in [Progress::Stopping { processes: 3 }, Progress::Stopped] {
            let _ = progress.send(step);
        }
        Ok(json!({"stopping": true, "processes": 3}))
    }

    async fn restart_daemon(&self, progress: ProgressSender) -> Result<Value, ApiError> {
        self.record("daemon.restart".into());
        for step in [
            Progress::Stopping { processes: 3 },
            Progress::Stopped,
            Progress::Started {
                version: VERSION.into(),
                pid: Some(4242),
            },
        ] {
            let _ = progress.send(step);
        }
        let mut status = daemon_status();
        status["resuming"] = json!(["acme@0"]);
        Ok(status)
    }

    async fn start_daemon(&self) -> Result<Value, ApiError> {
        self.record("daemon.start".into());
        Ok(daemon_status())
    }

    async fn profile_list(&self, lane: String) -> Result<ProfileList, ApiError> {
        self.record(format!("profile.list {lane}"));
        if let Some(error) = &self.profiles_error {
            return Err(ApiError::Rpc(error.clone()));
        }
        Ok(ProfileList {
            lane,
            items: profiles(),
        })
    }

    async fn host_list(&self) -> Result<Value, ApiError> {
        self.record("host.list".into());
        let boxes = self.boxes.lock().ok().and_then(|b| b.clone());
        Ok(boxes.unwrap_or_else(|| json!({"items": []})))
    }

    async fn remote_clients(&self) -> Result<Value, ApiError> {
        self.record("remote.clients".into());
        match self.clients.lock().ok().and_then(|c| c.clone()) {
            Some(clients) => Ok(clients),
            None => Err(ApiError::Rpc(RpcError::new(
                "unknown_method",
                json!({"method": "remote.clients"}),
                2,
            ))),
        }
    }

    /// Drops the client with id `client` from what `remote.clients` answers.
    async fn revoke_client(&self, client: String) -> Result<Value, ApiError> {
        self.record(format!("remote.revoke {client}"));
        let mut revoked = Vec::new();
        let mut ended = 0;
        if let Ok(mut clients) = self.clients.lock()
            && let Some(items) = clients
                .as_mut()
                .and_then(|c| c.get_mut("items"))
                .and_then(Value::as_array_mut)
        {
            items.retain(|item| {
                let gone = item["id"] == client.as_str();
                if gone {
                    revoked.push(json!({"id": item["id"], "name": item["name"]}));
                    ended += item["connections"].as_array().map_or(0, Vec::len);
                }
                !gone
            });
        }
        Ok(json!({"revoked": revoked, "ended": ended}))
    }

    /// Plays [`FakeApi::prompts`]: asks each one and waits for its answer,
    /// then reports the link connected and ends the request.
    async fn connect(&self, host: String) -> Result<ConnectSession, ApiError> {
        self.record(format!("connect {host}"));
        let prompts = self.prompts.lock().map(|p| p.clone()).unwrap_or_default();
        let (events_tx, events) = mpsc::channel(16);
        let (answers, mut answers_rx) = mpsc::channel::<(u64, Bytes)>(16);
        let calls = Arc::clone(&self.calls);
        tokio::spawn(async move {
            let event = |name: &str, data: Value| {
                ConnectEvent::Event(Event {
                    seq: 0,
                    event: name.into(),
                    data,
                })
            };
            let state = |state: &str| {
                event(
                    devlanes_proto::remote::STATE_EVENT,
                    json!({"host": host, "state": state, "attempt": 1}),
                )
            };
            if events_tx.send(state("connecting")).await.is_err() {
                return;
            }
            for (index, (kind, prompt)) in prompts.into_iter().enumerate() {
                let stream_id = 3 + index as u64;
                let asked = event(
                    devlanes_proto::remote::PROMPT_EVENT,
                    json!({"host": host, "kind": kind, "prompt": prompt, "stream_id": stream_id}),
                );
                if events_tx.send(asked).await.is_err() {
                    return;
                }
                let Some((stream, answer)) = answers_rx.recv().await else {
                    return;
                };
                if let Ok(mut calls) = calls.lock() {
                    calls.push(format!(
                        "answer {stream} {}",
                        String::from_utf8_lossy(&answer).trim_end()
                    ));
                }
            }
            let _ = events_tx.send(state("connected")).await;
            let done = json!({"host": host, "state": "connected"});
            let _ = events_tx.send(ConnectEvent::Done(Ok(done))).await;
        });
        Ok(ConnectSession { events, answers })
    }

    async fn disconnect(&self, host: String) -> Result<Value, ApiError> {
        self.record(format!("disconnect {host}"));
        Ok(json!({"disconnected": [host]}))
    }

    /// Records `remote.use <lane> <host>`, with `null` for this machine, and
    /// answers as if the switch happened, with the daemon's `lane_switched`
    /// notice first.
    async fn use_lane(&self, lane: String, host: Option<String>) -> Result<Value, ApiError> {
        let shown = host.as_deref().unwrap_or("null");
        self.record(format!("remote.use {lane} {shown}"));
        let to = host.as_deref().unwrap_or("this machine");
        let notice = json!({"code": "lane_switched", "lane": lane, "to": host, "from": "fork-a",
                            "host": host.as_deref().unwrap_or("fork-a"),
                            "message": format!("{lane} now comes from {to}")});
        self.emit(devlanes_proto::remote::WARNING_EVENT, notice)
            .await;
        Ok(json!({"lane": lane, "host": host, "previous": "fork-a",
                  "mirrored": true, "pending": false}))
    }

    async fn pair(&self) -> Result<Value, ApiError> {
        self.record("remote.pair".into());
        match self.pairing.lock().ok().and_then(|p| p.clone()) {
            Some(answer) => answer.map_err(ApiError::Rpc),
            None => Ok(crate::connect::pair_reply("K7QM-3XWD")),
        }
    }

    fn daemon_log(&self) -> Option<PathBuf> {
        self.log_path.clone()
    }

    fn home(&self) -> Option<PathBuf> {
        self.home.clone()
    }

    /// Fixed: a source build with only the welcome dismissed, whatever
    /// binary runs the tests and whatever its home holds.
    fn notices(&self) -> (devlanes_client::update::InstallMethod, Vec<String>) {
        (
            devlanes_client::update::InstallMethod::Source,
            vec![devlanes_tui::welcome::KEY.into()],
        )
    }
}

/// Profiles as `profile.list` describes them: a default, an extra and one without infra.
pub fn profiles() -> Vec<ProfileInfo> {
    let profile = |name: &str, include: &[&str], skips_infra, is_default| ProfileInfo {
        name: name.into(),
        include: include.iter().map(|s| (*s).to_owned()).collect(),
        exclude: Vec::new(),
        skips_infra,
        is_default,
    };
    vec![
        profile("web", &["web", "api"], false, true),
        profile("e2e", &["+e2e", "web", "api", "worker"], false, false),
        profile("frontend", &["web"], true, false),
    ]
}

/// The fake daemon's version: this build's, so no version-skew notice shows
/// unless a test sets another version on purpose.
pub const VERSION: &str = devlanes_version::VERSION;

/// `daemon.status` as the daemon sends it.
pub fn daemon_status() -> Value {
    json!({
        "version": VERSION,
        "pid": 4242,
        "uptime_ms": 7_500_000,
        "managed_processes": 3,
        "socket": "/home/acme/.devlanes/run/daemon.sock",
    })
}

/// `doctor` facts with a fallback port, a missing CA and an applied hosts file.
pub fn doctor_facts() -> Value {
    json!({
        "version": VERSION,
        "pid": 4242,
        "home": "/home/acme/.devlanes",
        "db_path": "/home/acme/.devlanes/state.db",
        "schema_version": 7,
        "supported_schema_version": 7,
        "socket": "/home/acme/.devlanes/run/daemon.sock",
        "sandbox": {"codex_sandbox": false, "app_sandbox": false},
        "infra": {
            "docker": {"reachable": true, "version": "27.1.1"},
            "compose": {"version": "2.29.1", "supports_lane_overrides": true}
        },
        "proxy": {
            "http": {"bound": true, "port": 80},
            "https": {"bound": true, "port": 8443, "fallback_reason": "port 443 needs root"}
        },
        "ca": {"present": false, "error": "no CA has been created yet"},
        "projects": 2,
        "subscribers": 4,
        "hosts": {"mode": "auto", "probe": "localhost_resolves", "state": "applied", "names": 6}
    })
}

/// A unit in `lane` with `state`.
pub fn unit(lane: &str, name: &str, state: &str) -> UnitSummary {
    UnitSummary {
        name: name.into(),
        lane: lane.into(),
        state: state.into(),
        kind: "service".into(),
        ..UnitSummary::default()
    }
}

/// A lane of `project` with an index, branch and units.
pub fn lane(
    project: &str,
    index: Option<u32>,
    branch: &str,
    state: &str,
    units: Vec<UnitSummary>,
) -> LaneSummary {
    let reference = match index {
        Some(index) => format!("{project}@{index}"),
        None => format!("{project}@{branch}"),
    };
    LaneSummary {
        reference,
        project: project.into(),
        index,
        branch: Some(branch.into()),
        path: PathBuf::from(format!("/work/{project}/{branch}")),
        state: state.into(),
        units,
        ..LaneSummary::default()
    }
}

/// Two projects with lanes in every state the tree distinguishes.
pub fn mixed_lanes() -> Vec<LaneSummary> {
    let mut worker = unit("acme@0", "worker", "failed");
    worker.exit_code = Some(1);
    worker.actor = Actor {
        kind: ActorKind::Agent,
        agent_id: Some("claude".into()),
        session_id: None,
    };
    let mut api = unit("acme@0", "api", "ready");
    api.urls = vec!["https://api.acme.localhost".into()];
    api.cpu_percent = Some(12.0);
    api.memory_bytes = Some(256 * 1024 * 1024);
    let mut migrate = unit("acme@0", "migrate", "exited");
    migrate.kind = "task".into();
    migrate.exit_code = Some(0);
    let main = lane(
        "acme",
        Some(0),
        "main",
        "running",
        vec![api, unit("acme@0", "web", "starting"), worker, migrate],
    );
    let idle = lane(
        "acme",
        Some(1),
        "feat/login",
        "idle",
        vec![unit("acme@1", "api", "idle")],
    );
    let mut moving = lane("acme", Some(2), "fix/cache", "idle", Vec::new());
    moving.moves_to = Some(3);
    moving.config_changed = true;
    let unassigned = lane("acme", None, "spike", "unassigned", Vec::new());
    let shop = lane(
        "shop",
        Some(0),
        "main",
        "running",
        vec![
            unit("shop@0", "storefront", "ready"),
            unit("shop@0", "admin", "stopped"),
        ],
    );
    vec![main, idle, moving, unassigned, shop]
}
