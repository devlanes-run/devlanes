//! Read-mostly methods: status, lanes, logs, wait, errors, why, env, url, doctor
//! and subscriptions.

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use devlanes_core::env::EnvLayer;
use devlanes_logs::{Resumable, Resume};
use devlanes_proto::{Context, LaneSummary, LogQuery, Method, Page, RpcError, events};
use devlanes_state::EndedBy;
use futures_util::{StreamExt, stream::FuturesUnordered};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    sync::{broadcast, watch},
    time::Instant,
};

use crate::{
    Session,
    engine::{Inner, LanePair, params, required},
    model::{ConfigState, Kind, is_mine, lane_summary},
    registry::internal,
    stream::LaneEvents,
    target::Policy,
    units::{Logged, UnitHost, UnitState, now_ms, query_log_dir},
};

/// `status` parameters.
#[derive(Debug, Default, Deserialize)]
struct StatusParams {
    #[serde(default)]
    all: bool,
    #[serde(default)]
    mine: bool,
    #[serde(default)]
    watch: bool,
    /// The command the client completes with the lane `status` reports as `here`;
    /// `status` itself when absent.
    #[serde(default)]
    here_for: Option<Method>,
}

/// `logs` parameters: proto's [`LogQuery`] plus `all` (cli `LaneLogQuery`).
#[derive(Debug, Default, Deserialize)]
struct LogsParams {
    #[serde(flatten)]
    query: LogQuery,
    #[serde(default)]
    all: bool,
}

/// `wait` parameters.
#[derive(Debug, Deserialize)]
struct WaitParams {
    target: String,
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// Stream the lane's (or unit's) events on the request while waiting.
    #[serde(default)]
    events: bool,
}

/// `errors` parameters.
#[derive(Debug, Default, Deserialize)]
struct ErrorsParams {
    #[serde(default)]
    since: Option<String>,
}

/// Parameters naming one unit.
#[derive(Debug, Default, Deserialize)]
struct UnitParams {
    #[serde(default)]
    unit: Option<String>,
}

/// `env` parameters.
#[derive(Debug, Default, Deserialize)]
struct EnvParams {
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    explain: bool,
    #[serde(default)]
    lane_env: bool,
}

/// `subscribe` parameters.
#[derive(Debug, Default, Deserialize)]
struct SubscribeParams {
    #[serde(default)]
    follow: Option<bool>,
}

/// Byte budget of a log query that names none (the logs crate's default).
const LOG_BUDGET: usize = 1024 * 1024;

/// Parses `5m`, `30s`, `2h`, `1d` or milliseconds into a duration.
fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    let value: u64 = digits.parse().ok()?;
    let unit = &text[digits.len()..];
    let seconds = match unit {
        "" | "ms" => return Some(Duration::from_millis(value)),
        "s" => value,
        "m" => value.checked_mul(60)?,
        "h" => value.checked_mul(3600)?,
        "d" => value.checked_mul(86_400)?,
        _ => return None,
    };
    Some(Duration::from_secs(seconds))
}

/// Whether `errors` counts a finished run as a crash. A run its own time
/// limit stopped always is, as it reads `failed`; one that ended with no stop
/// is when it has a non-zero exit code or a signal. A requested stop never is,
/// and neither is a run whose end nobody knows (rows from before this was
/// recorded, earlier boots, orphans that ended unwatched).
fn is_crash(run: &devlanes_state::UnitRun) -> bool {
    match run.ended_by {
        Some(EndedBy::TimedOut) => true,
        Some(EndedBy::Itself) => run
            .outcome
            .is_some_and(|o| o.code.is_some_and(|c| c != 0) || o.signal.is_some()),
        Some(EndedBy::Requested) | None => false,
    }
}

/// Lowercase name of an env layer for `env --explain`.
fn layer_word(layer: EnvLayer) -> &'static str {
    match layer {
        EnvLayer::Client => "client",
        EnvLayer::Contributor => "contributor",
        EnvLayer::Devlanes => "devlanes",
        EnvLayer::Global => "global",
        EnvLayer::Service => "service",
        EnvLayer::Profile => "profile",
        EnvLayer::Override => "override",
    }
}

/// `logs` selectors with each task that has only `steps` (it never runs a
/// unit of its own) replaced by the units of its steps among `candidates`:
/// `infra.setup.X` is `setup.X`, `infra.teardown.X` is `teardown.X`, and a
/// nested steps task its own steps. A selector with none of them stays, so
/// it still fails as not found.
fn step_units(
    selectors: Vec<String>,
    candidates: &[(String, Vec<String>)],
    config: Option<&devlanes_config::ProjectConfig>,
) -> Vec<String> {
    let Some(config) = config else {
        return selectors;
    };
    let known = |name: &str| candidates.iter().any(|(candidate, _)| candidate == name);
    let mut out: Vec<String> = Vec::new();
    for selector in selectors {
        if known(&selector) {
            out.push(selector);
            continue;
        }
        let mut units = Vec::new();
        let mut visited = BTreeSet::new();
        let mut pending = vec![selector.clone()];
        while let Some(name) = pending.pop() {
            // Steps may name each other in a cycle, which `run` refuses.
            if !visited.insert(name.clone()) {
                continue;
            }
            if let Some(entry) = name.strip_prefix("infra.setup.") {
                units.push(format!("setup.{entry}"));
            } else if let Some(entry) = name.strip_prefix("infra.teardown.") {
                units.push(format!("teardown.{entry}"));
            } else {
                match config.tasks.get(&name) {
                    Some(task) if task.run.is_none() && task.run_script.is_none() => {
                        pending.extend(task.steps.iter().flatten().rev().cloned());
                    }
                    _ => units.push(name),
                }
            }
        }
        units.retain(|unit| known(unit));
        if units.is_empty() {
            out.push(selector);
        }
        for unit in units {
            if !out.contains(&unit) {
                out.push(unit);
            }
        }
    }
    out
}

/// The program name of `pid`, read off the executor; `None` when it is gone or hidden.
async fn process_name(pid: u32) -> Option<String> {
    tokio::task::spawn_blocking(move || {
        devlanes_sys::process(pid)
            .ok()
            .flatten()
            .map(|info| info.name)
    })
    .await
    .ok()
    .flatten()
}

/// Resolves once one of `written` reports lines its log thread stored. A
/// finished log never stores more, so its closed watch is left out.
async fn any_written(written: &mut [watch::Receiver<Logged>]) {
    let mut changes: FuturesUnordered<_> =
        written.iter_mut().map(|logged| logged.changed()).collect();
    while let Some(changed) = changes.next().await {
        if changed.is_ok() {
            return;
        }
    }
    std::future::pending().await
}

/// Resolves once a unit of `units` runs another run than `hosts` (the ones
/// last read, `None` for no run), checked at each `unit.changed` of the bus.
/// Missed events count as a change.
async fn run_replaced(
    changes: &mut broadcast::Receiver<devlanes_proto::Event>,
    lane: &crate::model::LaneRt,
    units: &[String],
    hosts: &[Option<UnitHost>],
) {
    loop {
        match changes.recv().await {
            Ok(event) if event.event == events::UNIT_CHANGED => {
                let st = lane.st();
                let replaced = units.iter().zip(hosts).any(|(unit, read)| {
                    match (st.units.get(unit).map(|rec| &rec.host), read) {
                        (Some(now), Some(read)) => !now.same_run(read),
                        (now, read) => now.is_some() != read.is_some(),
                    }
                });
                if replaced {
                    return;
                }
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => return,
            Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
        }
    }
}

impl Inner {
    /// Lane summaries with the sampler's latest CPU and memory.
    pub(crate) fn summaries(
        &self,
        lanes: &[LanePair],
        mine: Option<&devlanes_proto::Actor>,
    ) -> Vec<LaneSummary> {
        let mut items = Vec::new();
        let gate = self.gate_snapshot();
        for (project, lane) in lanes {
            if !lane.st().listed() {
                continue;
            }
            let mut summary = lane_summary(&project.name(), &lane.st());
            if lane.st().record.is_some() {
                crate::model::add_shared(&mut summary, &crate::model::shared_containers(project));
            }
            self.fill_queued_with(gate.as_ref(), &mut summary);
            if let Some(caller) = mine {
                summary
                    .units
                    .retain(|unit| is_mine(&unit.actor, caller) && unit.state != "idle");
                if summary.units.is_empty() {
                    continue;
                }
            }
            self.fill_metrics(&mut summary);
            items.push(summary);
        }
        items.sort_by(|a, b| (a.project.as_str(), a.index).cmp(&(b.project.as_str(), b.index)));
        items
    }

    /// `status`, optionally streaming `lane.changed` until the client leaves.
    pub(crate) async fn status_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
        session: &Session,
    ) -> Result<Value, RpcError> {
        let args: StatusParams = params(value)?;
        let scope = self
            .read_scope(context, args.all, Policy::of(Method::Status))
            .await?;
        // The request's own lane as the command the client completes would resolve
        // it, after this status's own registration (none with `all`).
        let here = match args.here_for {
            Some(method) => Some(self.here(context, method).await),
            None if !args.all => Some(self.here(context, Method::Status).await),
            None => None,
        };
        let lanes = scope.lanes();
        let mine = args.mine.then_some(&context.actor);
        let items = self.summaries(&lanes, mine);
        if args.watch {
            // The first snapshot goes to this watcher only. It is built once the
            // watcher subscribed: a change between `items` and the subscription
            // would otherwise reach the watcher in neither.
            let snapshot = |seq: u64| {
                self.summaries(&lanes, mine)
                    .iter()
                    .map(|item| devlanes_proto::Event {
                        seq,
                        event: events::LANE_CHANGED.into(),
                        data: json!({"lane": item}),
                    })
                    .collect()
            };
            // By project rather than by the snapshot's lanes, so a lane the project
            // gains later (`lane new`, a new worktree) shows too.
            self.bus
                .forward_from(session, snapshot, |event| {
                    let lane = &event.data["lane"];
                    event.event == events::LANE_CHANGED
                        && scope.covers(
                            lane["project"].as_str().unwrap_or_default(),
                            lane["reference"].as_str().unwrap_or_default(),
                        )
                })
                .await;
        }
        let mut reply =
            serde_json::to_value(Page::complete(items)).map_err(|e| internal("encode", e))?;
        match here {
            Some(Ok(reference)) => reply["here"] = json!(reference),
            Some(Err(error)) => reply["here_error"] = json!(error),
            None => {}
        }
        Ok(reply)
    }

    /// `lane.list`: re-lists worktrees first so new ones show immediately.
    pub(crate) async fn lane_list(self: &Arc<Self>, context: &Context) -> Result<Value, RpcError> {
        let lanes = self
            .read_scope(context, false, Policy::of(Method::LaneList))
            .await?
            .lanes();
        serde_json::to_value(Page::complete(self.summaries(&lanes, None)))
            .map_err(|e| internal("encode", e))
    }

    /// Lines of one unit from its live host or, without one, its log directory,
    /// with where a follower continues.
    async fn unit_logs(
        &self,
        pair: &LanePair,
        unit: &str,
        query: LogQuery,
    ) -> Result<Resumable, RpcError> {
        let (project, lane) = pair;
        let host = lane.st().units.get(unit).map(|rec| rec.host.clone());
        match host {
            // A closed run's snapshot misses notes written after it, such as a
            // failed respawn's `[devlanes]` line; the directory has them.
            Some(host) if !host.log_closed() => host.logs_resumable(query, 0).await,
            _ => {
                let Some(index) = lane.st().index() else {
                    return Ok(Resumable {
                        page: Page::complete(Vec::new()),
                        resume: Resume::default(),
                    });
                };
                let dir =
                    crate::lifecycle::unit_log_dir(&self.opts.logs, &project.name(), index, unit);
                query_log_dir(dir, unit.to_owned(), query).await
            }
        }
    }

    /// `logs`: one unit as a page; several (or `all`) grouped by unit; `follow` streams.
    pub(crate) async fn logs_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
        session: &Session,
    ) -> Result<Value, RpcError> {
        let args: LogsParams = params(value)?;
        let mut query = args.query;
        let policy = Policy::of(Method::Logs);
        let (pair, selectors) = if query.units.is_empty() {
            (self.lane(context, policy).await?, Vec::new())
        } else {
            self.lane_for_units(&query.units, context, policy).await?
        };
        let (project, lane) = &pair;
        Self::refuse_external(project, lane, &selectors)?;
        let reference = lane.st().reference(&project.name());
        if query.since.as_deref() == Some("start") {
            let started = lane.st().started_at_ms.unwrap_or(0);
            query.since = Some(started.to_string());
        }
        let (candidates, selectors) = {
            let st = lane.st();
            let services = st.view.iter().flat_map(|view| {
                view.services
                    .iter()
                    .map(|s| (s.name.as_str(), s.unit.as_str()))
            });
            let selectors = crate::engine::expand_replicas(&selectors, services);
            let mut names: Vec<(String, Vec<String>)> = st
                .units
                .iter()
                .filter(|(_, rec)| !query.mine || is_mine(&rec.host.info().actor, &context.actor))
                .map(|(name, rec)| (name.clone(), rec.tags.clone()))
                .collect();
            if let Some(view) = &st.view
                && !query.mine
            {
                for service in &view.services {
                    if !names.iter().any(|(n, _)| *n == service.unit) {
                        names.push((service.unit.clone(), service.tags.clone()));
                    }
                }
            }
            (names, selectors)
        };
        let units = if args.all || selectors.is_empty() {
            if !args.all && selectors.is_empty() {
                return Err(RpcError::new(
                    "units_required",
                    json!({"lane": reference}),
                    2,
                ));
            }
            candidates.iter().map(|(name, _)| name.clone()).collect()
        } else {
            let config = lane.st().config.loaded().cloned();
            let selectors = step_units(selectors, &candidates, config.as_deref());
            crate::engine::match_units(&selectors, &candidates, &reference)?
        };
        query.units = Vec::new();
        query.mine = false;
        let follow = std::mem::take(&mut query.follow);
        // One response carries every unit's page, so they share the byte budget.
        let mut each = query.clone();
        if units.len() > 1 {
            let budget = query.max_bytes.unwrap_or(LOG_BUDGET);
            each.max_bytes = Some((budget / units.len()).max(1));
        }
        if args.all {
            let mut groups = Vec::new();
            for unit in &units {
                let page = self.unit_logs(&pair, unit, each.clone()).await?.page;
                groups
                    .push(json!({"unit": unit, "lines": page.items, "truncated": page.truncated}));
            }
            return Ok(json!({"lane": reference, "units": groups}));
        }
        // Per unit, where a follower continues.
        let mut resumes = Vec::new();
        let page = if let [unit] = units.as_slice() {
            let first = self.unit_logs(&pair, unit, query.clone()).await?;
            resumes.push(first.resume);
            first.page
        } else {
            let mut items = Vec::new();
            let mut truncated = false;
            for (index, unit) in units.iter().enumerate() {
                let first = self.unit_logs(&pair, unit, each.clone()).await?;
                truncated |= first.page.truncated;
                items.extend(first.page.items.into_iter().map(|line| (index, line)));
                resumes.push(first.resume);
            }
            items.sort_by_key(|(_, line)| (line.timestamp_ms, line.seq));
            // `head` is the first N of the merged range, not N per unit.
            if let Some(head) = query.head
                && items.len() > head
            {
                let cut: BTreeSet<usize> = items[head..].iter().map(|(index, _)| *index).collect();
                items.truncate(head);
                truncated = true;
                // A unit cut short continues after its last line shown.
                for index in cut {
                    resumes[index] = items
                        .iter()
                        .rev()
                        .find(|(shown, _)| *shown == index)
                        .map_or_else(Resume::default, |(_, line)| Resume::after(line));
                }
            }
            Page {
                items: items.into_iter().map(|(_, line)| line).collect(),
                truncated,
                next_cursor: None,
            }
        };
        if !follow {
            return serde_json::to_value(page).map_err(|e| internal("encode", e));
        }
        for line in &page.items {
            let event = devlanes_proto::Event {
                seq: 0,
                event: events::LOG.into(),
                data: json!({"unit": line.unit, "line": line}),
            };
            if session.event(event).await.is_err() {
                return Ok(json!({"items": [], "truncated": false, "next_cursor": null}));
            }
        }
        self.follow_logs(lane.clone(), units, query, resumes, session)
            .await;
        Ok(json!({"items": [], "truncated": false, "next_cursor": null}))
    }

    /// Streams new lines of `units` that `filter` (the first page's query)
    /// selects until the client disconnects, following restarts. Each unit's
    /// reads continue from its entry in `resumes`. It reads again when a
    /// unit's log thread stores lines or a unit's run is replaced.
    async fn follow_logs(
        &self,
        lane: Arc<crate::model::LaneRt>,
        units: Vec<String>,
        filter: LogQuery,
        mut resumes: Vec<Resume>,
        session: &Session,
    ) {
        let disconnected = session.disconnected();
        // Subscribed before the first read, so no replaced run goes unnoticed.
        let mut changes = self.bus.subscribe();
        loop {
            let mut hosts = Vec::with_capacity(units.len());
            let mut written = Vec::with_capacity(units.len());
            let mut more = false;
            for (unit, resume) in units.iter().zip(&mut resumes) {
                let host = lane.st().units.get(unit).map(|rec| rec.host.clone());
                hosts.push(host.clone());
                let Some(host) = host else { continue };
                // Taken before the read: lines stored after it mark the watch
                // changed, and every line stored before it is in the read.
                let mut logged = host.watch_logged();
                logged.borrow_and_update();
                written.push(logged);
                let query = LogQuery {
                    cursor: resume.cursor.clone(),
                    max_bytes: Some(256 * 1024),
                    tail: None,
                    head: None,
                    around: None,
                    ..filter.clone()
                };
                let Ok(read) = host.logs_resumable(query, resume.trailing).await else {
                    continue;
                };
                *resume = read.resume;
                // A full page leaves stored lines unread, which no watch reports again.
                more |= read.page.truncated;
                for line in read.page.items {
                    let event = devlanes_proto::Event {
                        seq: 0,
                        event: events::LOG.into(),
                        data: json!({"unit": unit, "line": line}),
                    };
                    if session.event(event).await.is_err() {
                        return;
                    }
                }
            }
            if more {
                continue;
            }
            tokio::select! {
                () = disconnected.cancelled() => return,
                () = any_written(&mut written) => {}
                () = run_replaced(&mut changes, &lane, &units, &hosts) => {}
            }
        }
    }

    /// `wait`: returns once a unit or lane is ready or failed; `timeout` (exit 124)
    /// otherwise. A task or exec unit is never `ready`: it is waited on until its
    /// command exits, and `exited` (on its own with 0) answers `ready: true`. A
    /// timeout on a unit carries its `state` and `timeout_ms`.
    pub(crate) async fn wait_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
        session: &Session,
    ) -> Result<Value, RpcError> {
        let args: WaitParams = required(value)?;
        let timeout = args
            .timeout_ms
            .map_or(self.opts.start_timeout, Duration::from_millis);
        let deadline = Instant::now() + timeout;
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let policy = Policy::of(Method::Wait);
        let as_unit = self
            .lane_for_units(std::slice::from_ref(&args.target), context, policy)
            .await;
        // A unit added by an edit just now must be known before it is looked up.
        if let Ok(((project, lane), _)) = &as_unit {
            self.fresh_config(project, lane).await;
        }
        let unit_target = match &as_unit {
            Ok(((_, lane), selectors)) => {
                let st = lane.st();
                let selector = selectors.first().cloned().unwrap_or_default();
                let known = st.units.contains_key(&selector)
                    || st
                        .view
                        .as_ref()
                        .is_some_and(|v| v.services.iter().any(|s| s.unit == selector));
                known.then_some(selector)
            }
            Err(_) => None,
        };
        let (project, lane, unit) = match (as_unit, unit_target) {
            (Ok(((project, lane), _)), Some(unit)) => (project, lane, Some(unit)),
            _ => {
                let as_lane = Context {
                    lane: Some(args.target.clone()),
                    ..context.clone()
                };
                let (project, lane) = self.lane(&as_lane, policy).await?;
                (project, lane, None)
            }
        };
        let reference = lane.st().reference(&project.name());
        let stream = args.events.then(|| {
            let mut summary = lane_summary(&project.name(), &lane.st());
            self.fill_queued(&mut summary);
            LaneEvents::lane(project.clone(), lane.clone(), unit.clone()).with_snapshot(summary)
        });
        let disconnected = session.disconnected();
        let wait = async {
            loop {
                let (hosts, busy): (Vec<(String, UnitHost)>, bool) = {
                    let st = lane.st();
                    let hosts = match &unit {
                        Some(unit) => st
                            .units
                            .get(unit)
                            .map(|rec| vec![(unit.clone(), rec.host.clone())])
                            .unwrap_or_default(),
                        None => st
                            .units
                            .iter()
                            .filter(|(_, rec)| rec.kind == Kind::Service)
                            .map(|(name, rec)| (name.clone(), rec.host.clone()))
                            .collect(),
                    };
                    (hosts, st.is_busy())
                };
                let states: Vec<UnitState> = hosts.iter().map(|(_, host)| host.state()).collect();
                let failed: Vec<usize> = states
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| matches!(s, UnitState::Failed(_)))
                    .map(|(i, _)| i)
                    .collect();
                if let Some(&first) = failed.first() {
                    let (name, host) = &hosts[first];
                    let failure = crate::lifecycle::failure_of(&reference, name, host).await;
                    let mut answer = json!({
                        "lane": reference, "unit": unit, "state": "failed", "ready": false,
                        "failed": failed.iter().map(|i| hosts[*i].0.clone()).collect::<Vec<_>>(),
                        "reason": failure.reason, "exit_code": failure.exit_code, "log_tail": failure.log_tail,
                    });
                    if !failure.log_tail_styled.is_empty() {
                        answer["log_tail_styled"] = json!(failure.log_tail_styled);
                    }
                    return Ok(answer);
                }
                let all_ready =
                    !states.is_empty() && states.iter().all(|s| matches!(s, UnitState::Ready));
                if all_ready && !busy {
                    return Ok(
                        json!({"lane": reference, "unit": unit, "state": "ready", "ready": true}),
                    );
                }
                if hosts.is_empty() && !busy {
                    return Ok(
                        json!({"lane": reference, "unit": unit, "state": "idle", "ready": false}),
                    );
                }
                let target = hosts.first().filter(|_| unit.is_some());
                if let Some((name, host)) = target
                    && let state @ (UnitState::Stopped(_) | UnitState::Exited(_)) = host.state()
                {
                    let code = host.exit_outcome().and_then(|o| o.code);
                    // Only a command that exited on its own with 0 did what the wait was for.
                    let ready = matches!(state, UnitState::Exited(_));
                    return Ok(
                        json!({"lane": reference, "unit": name, "state": state.word(), "ready": ready, "exit_code": code}),
                    );
                }
                if Instant::now() >= deadline {
                    let mut data = json!({"lane": reference, "unit": unit});
                    // A unit's state says why it is not ready; `running` is a command
                    // still at work, which the CLI words as such.
                    if let Some((_, host)) = target {
                        data["state"] = json!(host.state().word());
                        data["timeout_ms"] = json!(timeout_ms);
                    }
                    return Err(RpcError::new("timeout", data, 124));
                }
                // Nobody is left to tell; polling on would only hold the lane's state.
                tokio::select! {
                    () = disconnected.cancelled() => {
                        return Err(RpcError::new("client_disconnected", Value::Null, 4));
                    }
                    () = tokio::time::sleep(Duration::from_millis(150)) => {}
                }
            }
        };
        self.with_lane_events(session, stream, wait).await
    }

    /// `errors`: exit codes, crash counts, last error lines and port conflicts.
    /// Crashes are counted by [`is_crash`].
    pub(crate) async fn errors_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let args: ErrorsParams = params(value)?;
        let since_ms = args
            .since
            .as_deref()
            .and_then(parse_duration)
            .map_or(0, |d| {
                now_ms().saturating_sub(u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            });
        let mut items = Vec::new();
        let scope = self
            .read_scope(context, false, Policy::of(Method::Errors))
            .await?;
        for (project, lane) in scope.lanes() {
            let (reference, lane_id, units) = {
                let st = lane.st();
                let units: Vec<_> = st
                    .units
                    .iter()
                    .map(|(name, rec)| {
                        (
                            name.clone(),
                            rec.host.clone(),
                            (rec.failure_tail.clone(), rec.failure_tail_styled.clone()),
                            rec.ports.clone(),
                        )
                    })
                    .collect();
                (
                    st.reference(&project.name()),
                    st.record.as_ref().map(|r| r.id),
                    units,
                )
            };
            let history = match lane_id {
                Some(id) => self.store.unit_runs(id, 500).await.unwrap_or_default(),
                None => Vec::new(),
            };
            for (name, host, tail, ports) in units {
                let crashes = history
                    .iter()
                    .filter(|run| {
                        run.unit == name
                            && u64::try_from(run.started_at).unwrap_or(0) >= since_ms
                            && is_crash(run)
                    })
                    .count();
                let state = host.state();
                let failed = matches!(state, UnitState::Failed(_));
                let (error_lines, error_lines_styled) = if failed {
                    tail
                } else {
                    let query = LogQuery {
                        tail: Some(10),
                        level: Some("error".into()),
                        since: (since_ms > 0).then(|| since_ms.to_string()),
                        styled: true,
                        ..LogQuery::default()
                    };
                    devlanes_proto::split_styled(
                        host.logs(query)
                            .await
                            .map(|page| {
                                page.items
                                    .into_iter()
                                    .filter(|line| host.of_this_run(line))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    )
                };
                let mut conflicts = Vec::new();
                if failed {
                    for port in ports.values() {
                        if crate::ready::tcp_open(*port).await {
                            let pid = self.integ.ports.holder(*port).await;
                            if pid.is_some_and(|pid| Some(pid) != host.identity().map(|i| i.pid))
                                || !host.is_alive()
                            {
                                conflicts.push(json!({"port": port, "pid": pid}));
                            }
                        }
                    }
                }
                if crashes == 0 && !failed && error_lines.is_empty() {
                    continue;
                }
                let reason = match &state {
                    UnitState::Failed(reason) => reason.clone(),
                    _ => None,
                };
                items.push(json!({
                    "lane": reference,
                    "unit": name,
                    "state": state.word(),
                    "exit_code": host.exit_outcome().and_then(|o| o.code),
                    "crashes": crashes,
                    "reason": reason,
                    "error_lines": error_lines,
                    "port_conflicts": conflicts,
                }));
                // Colors only when some line had them, as `log_tail_styled`.
                if !error_lines_styled.is_empty()
                    && let Some(item) = items.last_mut()
                {
                    item["error_lines_styled"] = json!(error_lines_styled);
                }
            }
        }
        Ok(json!({"items": items, "truncated": false, "next_cursor": null}))
    }

    /// `why`: reason codes explaining a unit's state.
    pub(crate) async fn why_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let args: UnitParams = params(value)?;
        let unit = args
            .unit
            .ok_or_else(|| RpcError::new("units_required", json!({}), 2))?;
        let ((project, lane), selectors) = self
            .lane_for_units(
                std::slice::from_ref(&unit),
                context,
                Policy::of(Method::Why),
            )
            .await?;
        let name = selectors.into_iter().next().unwrap_or_default();
        let reference = lane.st().reference(&project.name());
        let mut reasons: Vec<Value> = Vec::new();
        let push = |reasons: &mut Vec<Value>, code: &str, data: Value| {
            reasons.push(json!({"code": code, "data": data}))
        };
        let (host, service, view_error, needs_env, config, starting, unassigned, orphan, saved) = {
            let st = lane.st();
            let orphan = st.orphan_error(&project.name(), &name);
            // A start by name adds to these, and a plain start brings them back.
            let saved: Vec<String> = st
                .record
                .as_ref()
                .map(|record| record.services.clone())
                .unwrap_or_default();
            let host = st.units.get(&name).map(|rec| rec.host.clone());
            let service = st
                .view
                .as_ref()
                .and_then(|v| v.services.iter().find(|s| s.unit == name).cloned());
            (
                host,
                service,
                st.view_error.clone(),
                st.env.is_none(),
                st.config.clone(),
                st.starts_in_phases() > 0,
                st.unassigned.clone(),
                orphan,
                saved,
            )
        };
        // Left running by a previous daemon at `started_at_ms`, found at
        // `orphaned_since_ms`, pid `pid`; `command` stops it.
        if let Some(orphan) = orphan {
            push(&mut reasons, "orphaned", orphan.data);
            return Ok(json!({
                "lane": reference,
                "unit": name,
                "state": devlanes_proto::ORPHANED,
                "reasons": reasons,
            }));
        }
        let alive = host.as_ref().is_some_and(UnitHost::is_alive);
        match unassigned {
            Some(reason) => push(&mut reasons, "unassigned", reason.data),
            // Without a layout no service of the lane is known or can start.
            None => {
                if let Some(error) = view_error.filter(|_| !alive) {
                    push(&mut reasons, &error.code, error.data);
                }
            }
        }
        match &config {
            ConfigState::Missing => push(&mut reasons, "no_config", json!({})),
            ConfigState::Invalid(error) => push(&mut reasons, "config_error", error.data.clone()),
            ConfigState::Elsewhere(other) => push(
                &mut reasons,
                "config_names_other_project",
                crate::model::elsewhere_error(other, Some(&reference)).data,
            ),
            ConfigState::Loaded(_) => {}
        }
        if host.is_none() && service.is_none() && reasons.is_empty() {
            return Err(RpcError::new("not_found", json!({"reference": unit}), 3));
        }
        let state = host.as_ref().map_or("idle", |h| h.state().word());
        if let Some(host) = &host {
            match host.state() {
                UnitState::Failed(Some(reason)) => push(&mut reasons, &reason.code, reason.data),
                UnitState::Starting => push(&mut reasons, "starting", json!({})),
                UnitState::Stopped(outcome) | UnitState::Exited(outcome) => push(
                    &mut reasons,
                    "stopped",
                    json!({"exit_code": outcome.code, "signal": outcome.signal, "requested": host.stop_was_requested()}),
                ),
                _ => {}
            }
        }
        if !alive && let Some(service) = &service {
            if needs_env {
                push(&mut reasons, "needs_env", json!({"lane": reference}));
            }
            if let Some((profiles, selected)) = Inner::profile_services(&lane)
                && !selected.contains(&service.name)
                && !saved.contains(&service.name)
            {
                if !service.autostart {
                    push(&mut reasons, "autostart_off", json!({}));
                } else {
                    push(
                        &mut reasons,
                        "not_in_profile",
                        json!({"profile": profiles.first(), "profiles": profiles}),
                    );
                }
            }
            if crate::ready::tcp_open(service.port).await {
                let pid = self.integ.ports.holder(service.port).await;
                push(
                    &mut reasons,
                    "port_in_use",
                    json!({"port": service.port, "pid": pid}),
                );
            }
        }
        if alive && host.as_ref().is_some_and(|h| h.state() == UnitState::Ready) {
            for (port, listening) in self.closed_ports(&lane, &name).await {
                push(
                    &mut reasons,
                    "not_listening",
                    json!({"port": port, "listening": listening}),
                );
            }
        }
        if starting
            && let Some(config) = config.loaded()
            && let Some(task) = config.tasks.get(&name)
            && task.after.is_some()
        {
            push(&mut reasons, "waiting_after_services", json!({}));
        }
        if let Some((code, data)) = self.hosts_reason(&reference, &name) {
            push(&mut reasons, code, data);
        }
        let state = match self.queued_reason(&reference, &name) {
            Some(data) => {
                push(&mut reasons, "queued", data);
                "queued"
            }
            None => state,
        };
        let mut result =
            json!({"lane": reference, "unit": name, "state": state, "reasons": reasons});
        // What the last run left behind: kept daemons, or processes that escaped.
        let leftovers = lane.st().units.get(&name).map(|rec| rec.leftovers(&name));
        if let Some(leftovers) = leftovers.filter(|l| !l.is_empty()) {
            if !leftovers.left_running.is_empty() {
                result["left_running"] = json!(leftovers.left_running);
            }
            if !leftovers.escaped.is_empty() {
                result["escaped"] = json!(leftovers.escaped);
            }
        }
        Ok(result)
    }

    /// The ports of running unit `name` that should accept connections and do
    /// not, each with the ports its processes listen on: the ports its hosts
    /// route to, or its main port when a `tcp` or `http` ready check looked at
    /// it. A unit without either (a worker) is not checked.
    async fn closed_ports(&self, lane: &crate::model::LaneRt, name: &str) -> Vec<(u16, Vec<u16>)> {
        let (mut ports, managed) = {
            let st = lane.st();
            let Some(rec) = st.units.get(name) else {
                return Vec::new();
            };
            // A live unit keeps the ports it was spawned with, as the proxy does.
            let mut ports: Vec<u16> = st
                .view
                .iter()
                .flat_map(|view| view.routes.iter())
                .filter(|route| route.unit == name)
                .map(|route| {
                    rec.ports
                        .get(&route.port_name)
                        .copied()
                        .unwrap_or(route.port)
                })
                .collect();
            if ports.is_empty() && matches!(rec.host.info().goal.check(), Some("tcp" | "http")) {
                ports.extend(rec.ports.get("default").copied());
            }
            (ports, rec.managed.clone())
        };
        ports.retain(|port| *port != 0);
        ports.sort_unstable();
        ports.dedup();
        // Concurrent, since a closed port costs up to a second on Windows.
        let open =
            futures_util::future::join_all(ports.iter().map(|port| crate::ready::tcp_open(*port)))
                .await;
        let closed: Vec<u16> = ports
            .into_iter()
            .zip(open)
            .filter(|(_, open)| !open)
            .map(|(port, _)| port)
            .collect();
        if closed.is_empty() {
            return Vec::new();
        }
        let listening = managed.listening_ports().await;
        closed
            .into_iter()
            .map(|port| (port, listening.clone()))
            .collect()
    }

    /// `env`: lane env (`lane_env: true` or no unit) or one unit's env, optionally
    /// with provenance.
    pub(crate) async fn env_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let args: EnvParams = params(value)?;
        let policy = Policy::of(Method::Env);
        if args.lane_env || args.unit.is_none() {
            let (project, lane) = self.lane(context, policy).await?;
            let (resolved, layers) = self
                .resolve_lane_env(
                    &project,
                    &lane,
                    crate::engine::carries_env(context).unwrap_or_default(),
                )
                .await?;
            let env = devlanes_core::env::assemble([
                (EnvLayer::Devlanes, resolved.lane_env().clone()),
                (EnvLayer::Global, layers.global),
                (EnvLayer::Profile, layers.profile),
            ]);
            let reference = lane.st().reference(&project.name());
            let mut result = json!({"lane": reference, "env": env.values});
            if !layers.unset.is_empty() {
                result["left_out"] = crate::resolve::left_out(&layers.unset);
            }
            return Ok(result);
        }
        let unit = args.unit.unwrap_or_default();
        let ((project, lane), selectors) = self
            .lane_for_units(std::slice::from_ref(&unit), context, policy)
            .await?;
        let name = selectors.into_iter().next().unwrap_or_default();
        let reference = lane.st().reference(&project.name());
        let live = lane
            .st()
            .units
            .get(&name)
            .filter(|rec| rec.host.is_alive())
            .map(|rec| (rec.env.clone(), rec.notes.clone(), rec.left_out.clone()));
        let mut left_out = std::collections::BTreeMap::new();
        let (env, notes) = match live {
            Some((env, notes, unset)) => {
                left_out = unset;
                (env, notes)
            }
            None => {
                let resolved = self
                    .resolve_with(&project, &lane, crate::engine::carries_env(context))
                    .await?;
                if let Some(error) = resolved
                    .errors
                    .get(&format!("services.{name}"))
                    .or_else(|| resolved.errors.get(&format!("tasks.{name}")))
                {
                    return Err(crate::resolve::resolve_error(error, &reference));
                }
                let launch = resolved
                    .services
                    .get(&name)
                    .map(|s| &s.launch)
                    .or_else(|| resolved.tasks.get(&name).and_then(|t| t.launch.as_ref()))
                    .ok_or_else(|| {
                        // Units with an environment of their own: services, then
                        // tasks that launch something.
                        let mut candidates: Vec<&String> = resolved.services.keys().collect();
                        candidates.extend(
                            resolved
                                .tasks
                                .iter()
                                .filter(|(_, task)| task.launch.is_some())
                                .map(|(name, _)| name),
                        );
                        RpcError::new(
                            "not_found",
                            json!({"reference": unit, "candidates": candidates}),
                            3,
                        )
                    })?;
                (
                    Arc::new(crate::lifecycle::copy_env(&launch.env)),
                    launch.notes.clone(),
                )
            }
        };
        let mut result = if args.explain {
            let items: Vec<Value> = env
                .values
                .iter()
                .map(|(key, value)| {
                    let layer = env.sources.get(key).copied().unwrap_or(EnvLayer::Client);
                    json!({"key": key, "value": value, "layer": layer_word(layer)})
                })
                .collect();
            json!({"lane": reference, "unit": name, "items": items, "notes": notes, "truncated": false, "next_cursor": null})
        } else {
            json!({"lane": reference, "unit": name, "env": env.values})
        };
        if !left_out.is_empty() {
            result["left_out"] = crate::resolve::left_out(&left_out);
        }
        Ok(result)
    }

    /// `url`: the public URL of a service or `infra.external` entry (internal URL
    /// without hosts). On a box, `clients` adds the URL each connected client
    /// that reports its mirror opens it at, or why it cannot.
    pub(crate) async fn url_method(
        self: &Arc<Self>,
        value: Value,
        context: &Context,
    ) -> Result<Value, RpcError> {
        let args: UnitParams = params(value)?;
        let unit = args
            .unit
            .ok_or_else(|| RpcError::new("units_required", json!({}), 2))?;
        let ((project, lane), selectors) = self
            .lane_for_units(
                std::slice::from_ref(&unit),
                context,
                Policy::of(Method::Url),
            )
            .await?;
        let name = selectors.into_iter().next().unwrap_or_default();
        let (reference, found) = {
            let st = lane.st();
            let reference = st.reference(&project.name());
            let service = st.view.as_ref().and_then(|view| {
                view.services
                    .iter()
                    .find(|s| s.unit == name || s.name == name)
            });
            let found = match service {
                Some(service) => {
                    let mut urls = vec![service.url.clone()];
                    urls.extend(service.public_url.clone());
                    let host = service
                        .public_url
                        .as_deref()
                        .and_then(url_host)
                        .map(str::to_owned);
                    Some((
                        service.unit.clone(),
                        urls,
                        host,
                        service.port,
                        service.hidden,
                    ))
                }
                None => st
                    .view
                    .iter()
                    .flat_map(|view| &view.externals)
                    .find(|external| external.name == name)
                    .map(|external| {
                        let summary = crate::model::external_summary(&st, &reference, external);
                        let host = external
                            .routes
                            .iter()
                            .find(|route| route.url.is_some())
                            .map(|route| route.host.clone());
                        (
                            summary.name,
                            summary.urls,
                            host,
                            external.port,
                            external.hidden,
                        )
                    }),
            };
            (reference, found)
        };
        let (unit_name, urls, host, port, hidden) =
            found.ok_or_else(|| RpcError::new("not_found", json!({"reference": unit}), 3))?;
        let clients = self.client_urls(&crate::remote::UnitTarget {
            lane: &reference,
            unit: &unit_name,
            host: host.as_deref(),
            port: Some(port),
            unmirrored: hidden.then_some(crate::remote::HIDDEN),
        });
        Ok(json!({
            "lane": reference,
            "unit": unit_name,
            "url": urls.last(),
            "urls": urls,
            "clients": clients,
        }))
    }

    /// `subscribe`: streams every event until disconnect; `follow: false` returns
    /// the recent events instead.
    pub(crate) async fn subscribe_method(
        self: &Arc<Self>,
        value: Value,
        session: &Session,
    ) -> Result<Value, RpcError> {
        let args: SubscribeParams = params(value)?;
        if args.follow == Some(false) {
            let items = self.bus.recent();
            return serde_json::to_value(Page::complete(items)).map_err(|e| internal("encode", e));
        }
        // A resync to this subscriber only: a status fetched right after it
        // subscribed may predate a change, so the client fetches again.
        let resync = |seq: u64| {
            vec![devlanes_proto::Event {
                seq,
                event: events::RESYNC.into(),
                data: json!({"reason": "subscribed"}),
            }]
        };
        self.bus.forward_from(session, resync, |_| true).await;
        Ok(json!({}))
    }

    /// `doctor`: daemon, state, sandbox, Docker, proxy and CA facts.
    pub(crate) async fn doctor(self: &Arc<Self>) -> Result<Value, RpcError> {
        let schema = self.store.schema_version().await.ok();
        let sandbox = json!({
            "codex_sandbox": std::env::var_os("CODEX_SANDBOX").is_some(),
            "app_sandbox": std::env::var_os("APP_SANDBOX_CONTAINER_ID").is_some(),
        });
        let mut doctor = json!({
            "version": devlanes_version::VERSION,
            "pid": std::process::id(),
            "home": self.opts.home,
            "db_path": self.opts.db_path,
            "schema_version": schema,
            "supported_schema_version": devlanes_state::SCHEMA_VERSION,
            "socket": self.opts.socket,
            "sandbox": sandbox,
            "infra": self.integ.infra.doctor().await,
            "proxy": self.proxy_doctor().await,
            "ca": self.integ.ca_status,
            // Routed hosts the CA cannot certify (`host_outside_ca`, see `certs`).
            "ca_warnings": self.all_ca_warnings(),
            // Hostnames two lanes claim and names that are not valid DNS names.
            "route_warnings": self.route_warnings(),
            "projects": self.all_projects().len(),
            "subscribers": self.bus.subscribers(),
            "hosts": self.hosts_status(),
            "dns": self.dns_doctor().await,
            "state": self.state_doctor().await,
            "user_config": self.user_config_doctor().await,
            // The CLI compares them with the service manager's stop window.
            "stop_timeouts": self.stop_timeouts(),
        });
        // An elevated daemon runs every unit elevated; `doctor` warns.
        #[cfg(windows)]
        {
            doctor["elevated"] = json!(devlanes_sys_windows::token::elevated().ok());
            doctor["reserved_ports"] = self.reserved_ports_doctor().await;
            // `run:` strings and `ready.cmd` need Git for Windows' sh.exe.
            doctor["shell"] = json!({ "path": self.integ.shell });
        }
        // Inside WSL only: worktrees on Windows drives, where file watching misses edits.
        if let Some(worktrees) = self.windows_drive_doctor().await {
            doctor["windows_drive_worktrees"] = worktrees;
        }
        Ok(doctor)
    }

    /// `doctor`'s `reserved_ports`: the Windows reserved port ranges checked
    /// against `ports.range`, the lanes' assigned ports, the proxy ports and
    /// the mirrored ports. Read through the shared cache, since the TUI polls
    /// `doctor`.
    #[cfg(windows)]
    async fn reserved_ports_doctor(&self) -> Value {
        let reading = crate::reserved_ports::RESERVED.get().await;
        let user = &self.opts.user;
        let assigned: Vec<(String, std::collections::BTreeSet<u16>)> = self
            .all_lanes()
            .into_iter()
            .filter_map(|(project, lane)| {
                let st = lane.st();
                let allocation = st.allocation.as_ref()?;
                let block = allocation.block;
                let ports = allocation
                    .offsets
                    .values()
                    .chain(&allocation.reserved)
                    .filter(|offset| **offset < block.size)
                    .filter_map(|offset| block.start.checked_add(*offset))
                    .collect();
                Some((st.reference(&project.name()), ports))
            })
            .collect();
        crate::reserved_ports::report(
            &reading,
            user.ports.bounds().ok(),
            &assigned,
            [
                ("http", user.proxy.http_port),
                ("https", user.proxy.https_port),
            ],
            &self.remote.advertised_ports(),
        )
    }

    /// `doctor`'s `stop_timeouts`: the longest stop timeout of each service of
    /// each project, `[{project, unit, timeout_ms}]`, from running services and
    /// the loaded configs. A timeout written as an expression counts once a run
    /// resolved it.
    fn stop_timeouts(&self) -> Vec<Value> {
        use devlanes_config::Templated;
        let mut longest: std::collections::BTreeMap<(String, String), Duration> =
            std::collections::BTreeMap::new();
        let mut note = |project: &str, unit: &str, timeout: Duration| {
            let entry = longest
                .entry((project.to_owned(), unit.to_owned()))
                .or_default();
            *entry = (*entry).max(timeout);
        };
        for (project, lane) in self.all_lanes() {
            let name = project.name();
            let st = lane.st();
            for (unit, rec) in &st.units {
                if rec.kind == crate::model::Kind::Service {
                    note(&name, unit, rec.stop.timeout);
                }
            }
            let Some(config) = st.config.loaded() else {
                continue;
            };
            for (service, service_config) in &config.services {
                let written = service_config
                    .stop
                    .as_ref()
                    .and_then(|stop| stop.timeout.as_ref());
                let timeout = match written {
                    None => devlanes_core::StopPolicy::default().timeout,
                    Some(Templated::Literal(timeout)) => timeout.0,
                    Some(Templated::Expression(_)) => continue,
                };
                note(&name, service, timeout);
            }
        }
        longest
            .into_iter()
            .map(|((project, unit), timeout)| {
                json!({
                    "project": project,
                    "unit": unit,
                    "timeout_ms": u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                })
            })
            .collect()
    }

    /// The proxy's listener status; a listener that fell back from its
    /// configured port (`requested_port`) names what holds that port now as
    /// `requested_holder: {pid, command}`, and one on its configured port names
    /// the other processes listening there too as `shared_with: [{pid, command}]`.
    async fn proxy_doctor(&self) -> Value {
        let mut status = self.integ.routes.status();
        for listener in ["http", "https"] {
            let Some(entry) = status.get_mut(listener) else {
                continue;
            };
            let requested = entry
                .get("requested_port")
                .and_then(Value::as_u64)
                .and_then(|port| u16::try_from(port).ok());
            let bound = entry
                .get("port")
                .and_then(Value::as_u64)
                .and_then(|port| u16::try_from(port).ok());
            if let Some(requested) = requested.filter(|requested| Some(*requested) != bound) {
                if let Some(pid) = self.integ.ports.holder(requested).await {
                    entry["requested_holder"] =
                        json!({"pid": pid, "command": process_name(pid).await});
                }
                continue;
            }
            // Windows lets another program listen on the wildcard address next to
            // the proxy's loopback listener (or the reverse), so both start and
            // local connections reach only the proxy.
            let Some(port) = bound.filter(|_| entry["bound"] == true) else {
                continue;
            };
            let own = std::process::id();
            let mut shared = Vec::new();
            for pid in self.integ.ports.holders(port).await {
                if pid != own {
                    shared.push(json!({"pid": pid, "command": process_name(pid).await}));
                }
            }
            if !shared.is_empty() {
                entry["shared_with"] = Value::Array(shared);
            }
        }
        status
    }

    /// `doctor`'s `user_config`: the file on disk checked now, since it may have
    /// changed since the daemon loaded it. `errors` are located; with any, the next
    /// daemon start (or `daemon restart`) refuses to run until the file is fixed.
    async fn user_config_doctor(&self) -> Value {
        let path = self.opts.home.join("config.yaml");
        let file = path.clone();
        let checked = tokio::task::spawn_blocking(move || {
            if !file.exists() {
                return None;
            }
            let (config, errors) = devlanes_config::load_user_lenient(&file);
            let errors: Vec<Value> = errors.iter().map(located).collect();
            Some((errors, config.warnings().to_vec()))
        })
        .await
        .ok()
        .flatten();
        // Without a file the loaded settings are all there is to report.
        let (errors, warnings) =
            checked.unwrap_or_else(|| (Vec::new(), self.opts.user.warnings().to_vec()));
        json!({
            "path": path,
            "valid": errors.is_empty(),
            // The first error alone, for clients that show one.
            "error": errors.first(),
            "errors": errors,
            "warnings": warnings,
        })
    }
}

/// The hostname of an `https://host[:port]` URL.
fn url_host(url: &str) -> Option<&str> {
    let authority = url.strip_prefix("https://")?.split('/').next()?;
    Some(
        authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host),
    )
}

/// A config error as `{message, file, line, column}`.
pub(crate) fn located(error: &devlanes_config::ConfigError) -> Value {
    json!({
        "message": error.message,
        "file": error.location.file,
        "line": error.location.line,
        "column": error.location.column,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// `errors --since` accepts the CLI's duration forms.
    fn durations() {
        assert_eq!(parse_duration("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_duration("250"), Some(Duration::from_millis(250)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_duration("x"), None);
        // Overflow is no duration rather than a panic (debug) or a wrap (release).
        assert_eq!(parse_duration(&format!("{}d", u64::MAX)), None);
    }

    #[test]
    /// The hostname of a public URL, with or without its port.
    fn url_hosts() {
        assert_eq!(
            url_host("https://web.acme.localhost"),
            Some("web.acme.localhost")
        );
        assert_eq!(
            url_host("https://web.acme.localhost:8443/x"),
            Some("web.acme.localhost")
        );
        assert_eq!(url_host("http://127.0.0.1:3000"), None);
    }
}
