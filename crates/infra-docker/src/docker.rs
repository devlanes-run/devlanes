use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};

use bollard::{
    Docker,
    container::LogOutput,
    models::{ContainerCreateBody, ContainerSummary, HostConfig, PortBinding, VolumeCreateRequest},
    query_parameters::{
        CreateContainerOptionsBuilder, CreateImageOptionsBuilder, DataUsageOptionsBuilder,
        EventsOptionsBuilder, ListContainersOptionsBuilder, ListVolumesOptionsBuilder,
        LogsOptionsBuilder, RemoveContainerOptionsBuilder, RemoveVolumeOptionsBuilder,
        StopContainerOptionsBuilder,
    },
};
use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt, stream::BoxStream};
use serde::Serialize;

use crate::{
    ContainerSpec, Error, Health, HealthEvent, Mount, Scope,
    events::{WATCHED_EVENTS, WatchEvent, map_event, resubscribing},
    health::{health_from_inspect, health_from_summary, tcp_probe},
    host::{config_dir, resolve_host},
    labels,
    spec::{config_hash, container_labels, container_name, primary_port, scope_labels, validate},
};

/// Pause between health polls while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const PROBE_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const PROBE_SETTLE: Duration = Duration::from_millis(300);

/// Options for [`DockerInfra::ensure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnsureOptions {
    /// How long to wait for the healthcheck or tcp probe after starting.
    pub health_timeout: Duration,
}

impl Default for EnsureOptions {
    fn default() -> Self {
        Self {
            health_timeout: Duration::from_secs(120),
        }
    }
}

/// What [`DockerInfra::ensure`] had to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnsureAction {
    Created,
    /// The container existed with the same config and was stopped.
    Started,
    /// The container existed with the same config and was running.
    Adopted,
    /// The config changed, so the old container was replaced.
    Recreated,
}

/// Host ports of a container: `port` is `infra.X.port`, `ports` is keyed by port name
/// (`infra.X.ports.http`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Endpoint {
    pub port: Option<u16>,
    pub ports: BTreeMap<String, u16>,
}

/// Result of [`DockerInfra::ensure`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ensured {
    pub container_id: String,
    pub container_name: String,
    pub action: EnsureAction,
    pub endpoint: Endpoint,
    pub health: Health,
}

/// A devlanes container found by label, for re-adoption after a daemon restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManagedContainer {
    pub id: String,
    pub name: String,
    /// The `infra.containers` key.
    pub key: Option<String>,
    pub scope: Option<Scope>,
    pub health: Health,
    /// Host ports by container port, from Docker's port list.
    pub ports: BTreeMap<u16, u16>,
    pub config_hash: Option<String>,
}

/// What [`DockerInfra::remove_lane`] deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Removed {
    pub containers: Vec<String>,
    pub volumes: Vec<String>,
}

/// What [`DockerInfra::lane_contents`] found: the containers and named volumes
/// that removing the lane (and taking its Compose project down with volumes)
/// would delete.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LaneContents {
    pub containers: Vec<LaneContainer>,
    pub volumes: Vec<LaneVolume>,
    /// The lane's Compose project has a container or volume.
    pub compose: bool,
}

/// A container of [`LaneContents`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LaneContainer {
    pub name: String,
    /// `container` for a plain `infra.containers` one, `compose` for a service
    /// of the lane's Compose project.
    pub kind: &'static str,
}

/// A named volume of [`LaneContents`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LaneVolume {
    pub name: String,
    /// Disk use as the engine reports it; `None` when it does not (other volume
    /// drivers, engines before API 1.52, or a size query that took too long).
    pub bytes: Option<u64>,
}

/// A container of a Compose project, from [`DockerInfra::compose_containers`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComposeContainer {
    /// The container name, without Docker's leading `/`.
    pub name: String,
    /// The Compose service it runs.
    pub service: String,
    pub health: Health,
}

/// Label Compose puts on the containers and volumes of a project.
const COMPOSE_PROJECT_LABEL: &str = "com.docker.compose.project";
/// Label Compose puts on a container with the service it runs.
const COMPOSE_SERVICE_LABEL: &str = "com.docker.compose.service";
/// `True` on the containers of `docker compose run`.
const COMPOSE_ONEOFF_LABEL: &str = "com.docker.compose.oneoff";

/// Longest the engine may take to size volumes for [`DockerInfra::lane_contents`];
/// it walks every volume's files, so on a busy machine it can take seconds.
const VOLUME_SIZE_TIMEOUT: Duration = Duration::from_secs(3);

/// Options for [`DockerInfra::logs`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogOptions {
    /// The last N lines of stored output; `None` sends all of it.
    pub tail: Option<usize>,
    /// Keep the stream open for new output until the container stops.
    pub follow: bool,
    /// Only output from this Unix second on.
    pub since: Option<i64>,
    /// Prefix every line with its RFC 3339 nanosecond timestamp and a space.
    pub timestamps: bool,
}

/// The output a [`LogChunk`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Stdout,
    Stderr,
    /// A container with a TTY, whose stdout and stderr Docker cannot tell apart.
    Console,
}

/// Bytes of container output. A line can span several chunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogChunk {
    pub stream: LogStream,
    pub bytes: Bytes,
}

impl LogChunk {
    /// Maps bollard's output; stdin echo is dropped.
    fn from_output(output: LogOutput) -> Option<Self> {
        let (stream, bytes) = match output {
            LogOutput::StdOut { message } => (LogStream::Stdout, message),
            LogOutput::StdErr { message } => (LogStream::Stderr, message),
            LogOutput::Console { message } => (LogStream::Console, message),
            LogOutput::StdIn { .. } => return None,
        };
        Some(Self { stream, bytes })
    }
}

/// Plain Docker containers through the Engine API. Cloning shares the connection.
#[derive(Debug, Clone)]
pub struct DockerInfra {
    docker: Docker,
}

/// Reads an environment variable, for [`DockerConnector::new`].
type EnvReader = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The engine client, made on the first call that can make one and kept from
/// then on. Every call before that tries again, so an engine that starts after
/// the daemon is found by the next call. `connect` errors are the caller's
/// per-call answer: [`Error::is_engine_stopped`] reads an absent Unix socket as
/// a stopped engine.
pub struct DockerConnector {
    env: EnvReader,
    client: tokio::sync::OnceCell<DockerInfra>,
}

impl DockerConnector {
    /// Resolves the endpoint from `DOCKER_HOST` and the current Docker context
    /// read through `env`, as [`DockerInfra::connect`] does.
    pub fn new(env: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        Self {
            env: Arc::new(env),
            client: tokio::sync::OnceCell::new(),
        }
    }

    /// The process environment.
    pub fn from_process_env() -> Self {
        Self::new(|key| std::env::var(key).ok())
    }

    /// The client, connecting first while there is none. The endpoint is
    /// resolved on each attempt, so a context switched before the engine came
    /// up is used. Concurrent first calls connect once.
    pub async fn get(&self) -> Result<&DockerInfra, Error> {
        self.client
            .get_or_try_init(|| DockerInfra::connect_with(self.env.clone()))
            .await
    }
}

impl DockerInfra {
    /// Connects to the engine selected by `DOCKER_HOST` or the current Docker context.
    pub async fn connect() -> Result<Self, Error> {
        Self::connect_with(Arc::new(|key: &str| std::env::var(key).ok())).await
    }

    /// [`Self::connect`] with the environment read through `env`. Resolving
    /// reads Docker's config files, so it runs off the executor.
    async fn connect_with(env: EnvReader) -> Result<Self, Error> {
        let host = tokio::task::spawn_blocking(move || {
            let env = |key: &str| env(key);
            resolve_host(env, config_dir(env).as_deref())
        })
        .await
        .map_err(|error| Error::Io(std::io::Error::other(error)))??;
        Ok(Self::from_client(Docker::connect_with_host(&host)?))
    }

    /// Wraps an existing client, for example one built with an explicit socket.
    pub fn from_client(docker: Docker) -> Self {
        Self { docker }
    }

    /// The bollard client, for callers that need other API calls (stats, logs).
    pub fn client(&self) -> &Docker {
        &self.docker
    }

    /// The engine version, for `doctor`.
    pub async fn server_version(&self) -> Result<Option<String>, Error> {
        Ok(self.docker.version().await?.version)
    }

    /// Creates or adopts the container for `spec`, starts it and waits until it is healthy.
    /// A container with the same name and config hash is reused and never stopped; a
    /// changed config replaces it. A same-named container that devlanes does not own for
    /// this project is an error.
    pub async fn ensure(
        &self,
        spec: &ContainerSpec,
        options: EnsureOptions,
    ) -> Result<Ensured, Error> {
        let mounts = validate(spec)?;
        let hash = config_hash(spec, &mounts);
        let name = container_name(&spec.project, &spec.name, spec.scope);
        self.create_volumes(spec, &mounts).await?;

        let action = match self.docker.inspect_container(&name, None).await {
            Ok(existing) => {
                let existing_labels = existing
                    .config
                    .as_ref()
                    .and_then(|config| config.labels.clone())
                    .unwrap_or_default();
                let owned = existing_labels.get(labels::PROJECT) == Some(&spec.project)
                    && existing_labels.get(labels::CONTAINER) == Some(&spec.name);
                if !owned {
                    return Err(Error::NameConflict {
                        name,
                        owner: existing_labels.get(labels::PROJECT).cloned(),
                    });
                }
                let running = existing
                    .state
                    .as_ref()
                    .and_then(|state| state.running)
                    .unwrap_or(false);
                let paused = existing
                    .state
                    .as_ref()
                    .and_then(|state| state.paused)
                    .unwrap_or(false);
                // A frozen lane whose thaw failed still starts. Shared containers are
                // never paused by devlanes, so one that is paused stays that way.
                if paused && matches!(spec.scope, Scope::Lane(_)) {
                    match self
                        .docker
                        .unpause_container(&name)
                        .await
                        .map_err(Error::from)
                    {
                        Ok(()) => {}
                        Err(error) if error.is_state_race() => {}
                        Err(error) => return Err(error),
                    }
                }
                if existing_labels.get(labels::CONFIG_HASH) != Some(&hash) {
                    self.remove(&name).await?;
                    self.create(spec, &mounts, &hash, &name).await?;
                    EnsureAction::Recreated
                } else if running {
                    EnsureAction::Adopted
                } else {
                    EnsureAction::Started
                }
            }
            Err(error) => {
                let error = Error::from(error);
                if !error.is_not_found() {
                    return Err(error);
                }
                self.create(spec, &mounts, &hash, &name).await?;
                EnsureAction::Created
            }
        };
        if action != EnsureAction::Adopted {
            self.docker.start_container(&name, None).await?;
        }
        let probe = primary_port(&spec.ports);
        let health = self
            .wait_healthy(&name, probe, options.health_timeout)
            .await?;
        let inspect = self.docker.inspect_container(&name, None).await?;
        Ok(Ensured {
            container_id: inspect.id.unwrap_or_default(),
            container_name: name,
            action,
            endpoint: Endpoint {
                port: probe,
                ports: spec
                    .ports
                    .iter()
                    .map(|port| (port.name.clone(), port.host))
                    .collect(),
            },
            health,
        })
    }

    /// One health check: Docker's healthcheck when the image has one, else a tcp probe of
    /// `probe_port` when given, else plain running.
    pub async fn health(&self, container: &str, probe_port: Option<u16>) -> Result<Health, Error> {
        let inspect = match self.docker.inspect_container(container, None).await {
            Ok(inspect) => inspect,
            Err(error) => {
                let error = Error::from(error);
                return if error.is_not_found() {
                    Ok(Health::Missing)
                } else {
                    Err(error)
                };
            }
        };
        let state = health_from_inspect(&inspect);
        match (state.needs_probe, probe_port) {
            (true, Some(port)) => Ok(
                if tcp_probe(port, PROBE_CONNECT_TIMEOUT, PROBE_SETTLE).await {
                    Health::Healthy
                } else {
                    Health::Starting
                },
            ),
            _ => Ok(state.health),
        }
    }

    /// Polls [`Self::health`] until healthy (or running without any check), failing on
    /// unhealthy/exited containers and after `timeout`.
    pub async fn wait_healthy(
        &self,
        container: &str,
        probe_port: Option<u16>,
        timeout: Duration,
    ) -> Result<Health, Error> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let health = self.health(container, probe_port).await?;
            match &health {
                Health::Healthy | Health::Running => return Ok(health),
                failed if failed.is_final_failure() => {
                    return Err(Error::Unhealthy {
                        name: container.to_owned(),
                        health,
                    });
                }
                _ => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::HealthTimeout {
                    name: container.to_owned(),
                    timeout,
                    last: health,
                });
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Every container labelled with `project`, running or not. Listing never changes
    /// container state, so it is safe for re-adoption after a daemon restart.
    pub async fn list_managed(&self, project: &str) -> Result<Vec<ManagedContainer>, Error> {
        let filters = label_filters(&BTreeMap::from([(
            labels::PROJECT.to_owned(),
            project.to_owned(),
        )]));
        let summaries = self.list(&filters, true).await?;
        Ok(summaries.into_iter().map(managed_from_summary).collect())
    }

    /// Every container of Compose project `project`, each replica, running or
    /// not, without `docker compose run` one-offs. One engine list, no Compose
    /// process.
    pub async fn compose_containers(&self, project: &str) -> Result<Vec<ComposeContainer>, Error> {
        let mut filters = label_filters(&BTreeMap::from([(
            COMPOSE_PROJECT_LABEL.to_owned(),
            project.to_owned(),
        )]));
        filters
            .entry("label".to_owned())
            .or_default()
            .push(format!("{COMPOSE_ONEOFF_LABEL}=False"));
        Ok(self
            .list(&filters, true)
            .await?
            .into_iter()
            .filter_map(|summary| {
                let service = summary.labels.as_ref()?.get(COMPOSE_SERVICE_LABEL)?.clone();
                Some(ComposeContainer {
                    name: summary_name(summary.names.as_deref()),
                    service,
                    health: health_from_summary(
                        summary.state,
                        summary.health.and_then(|health| health.status),
                    ),
                })
            })
            .collect())
    }

    /// Docker's `State.StartedAt` of `container` (a name or id) while its
    /// process runs, paused or not; `None` when it does not run or no longer
    /// exists.
    pub async fn started_at(&self, container: &str) -> Result<Option<String>, Error> {
        let inspect = match self.docker.inspect_container(container, None).await {
            Ok(inspect) => inspect,
            Err(error) => {
                let error = Error::from(error);
                return if error.is_not_found() {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
        };
        Ok(inspect.state.and_then(|state| {
            let runs = state.running == Some(true) || state.paused == Some(true);
            state.started_at.filter(|_| runs)
        }))
    }

    /// Stops running containers of `project`, limited to `scope` when given. Returns the
    /// stopped container names.
    pub async fn stop(
        &self,
        project: &str,
        scope: Option<Scope>,
        timeout: Duration,
    ) -> Result<Vec<String>, Error> {
        let mut wanted = BTreeMap::from([(labels::PROJECT.to_owned(), project.to_owned())]);
        if let Some(scope) = scope {
            wanted.extend(scope_labels(project, scope));
        }
        let seconds = i32::try_from(timeout.as_secs()).unwrap_or(i32::MAX);
        let mut stopped = Vec::new();
        for summary in self.list(&label_filters(&wanted), false).await? {
            let Some(id) = summary.id else { continue };
            let options = StopContainerOptionsBuilder::new().t(seconds).build();
            self.docker.stop_container(&id, Some(options)).await?;
            stopped.push(summary_name(summary.names.as_deref()));
        }
        Ok(stopped)
    }

    /// Removes the lane's containers (with anonymous volumes) and its named volumes.
    /// Shared containers and other lanes are untouched.
    pub async fn remove_lane(&self, project: &str, lane: u32) -> Result<Removed, Error> {
        let filters = label_filters(&scope_labels(project, Scope::Lane(lane)));
        let mut removed = Removed::default();
        for summary in self.list(&filters, true).await? {
            let Some(id) = summary.id else { continue };
            self.remove(&id).await?;
            removed
                .containers
                .push(summary_name(summary.names.as_deref()));
        }
        let options = ListVolumesOptionsBuilder::new().filters(&filters).build();
        let volumes = self.docker.list_volumes(Some(options)).await?;
        for volume in volumes.volumes.unwrap_or_default() {
            let options = RemoveVolumeOptionsBuilder::new().force(true).build();
            self.docker
                .remove_volume(&volume.name, Some(options))
                .await?;
            removed.volumes.push(volume.name);
        }
        Ok(removed)
    }

    /// The lane's containers and named volumes, plain ones by label and those of
    /// `compose_project` (the lane's Compose project) by Compose's label, with
    /// volume sizes when the engine reports them in time. Changes nothing.
    pub async fn lane_contents(
        &self,
        project: &str,
        lane: u32,
        compose_project: Option<&str>,
    ) -> Result<LaneContents, Error> {
        let mut sets = vec![(
            "container",
            label_filters(&scope_labels(project, Scope::Lane(lane))),
        )];
        if let Some(name) = compose_project {
            sets.push((
                "compose",
                label_filters(&BTreeMap::from([(
                    COMPOSE_PROJECT_LABEL.to_owned(),
                    name.to_owned(),
                )])),
            ));
        }
        let mut contents = LaneContents::default();
        let mut volumes = Vec::new();
        for (kind, filters) in &sets {
            for summary in self.list(filters, true).await? {
                let name = summary_name(summary.names.as_deref());
                if !contents.containers.iter().any(|known| known.name == name) {
                    contents.compose |= *kind == "compose";
                    contents.containers.push(LaneContainer { name, kind });
                }
            }
            let options = ListVolumesOptionsBuilder::new().filters(filters).build();
            for volume in self
                .docker
                .list_volumes(Some(options))
                .await?
                .volumes
                .unwrap_or_default()
            {
                if !volumes.contains(&volume.name) {
                    contents.compose |= *kind == "compose";
                    volumes.push(volume.name);
                }
            }
        }
        let sizes = if volumes.is_empty() {
            HashMap::new()
        } else {
            self.volume_sizes().await
        };
        contents.volumes = volumes
            .into_iter()
            .map(|name| LaneVolume {
                bytes: sizes.get(&name).copied(),
                name,
            })
            .collect();
        Ok(contents)
    }

    /// Sizes of the engine's volumes by name, from `system df` (what `docker
    /// system df -v` shows). Empty when the engine does not answer within
    /// [`VOLUME_SIZE_TIMEOUT`], fails, or reports sizes in a form this API
    /// version does not read.
    async fn volume_sizes(&self) -> HashMap<String, u64> {
        let options = DataUsageOptionsBuilder::new()
            ._type(vec!["volume".to_owned()])
            .verbose(true)
            .build();
        let Ok(Ok(usage)) =
            tokio::time::timeout(VOLUME_SIZE_TIMEOUT, self.docker.df(Some(options))).await
        else {
            return HashMap::new();
        };
        usage
            .volume_usage
            .and_then(|usage| usage.items)
            .unwrap_or_default()
            .iter()
            .filter_map(|item| {
                let name = item.get("Name")?.as_str()?;
                // The engine reports -1 when the volume driver cannot tell.
                let size = u64::try_from(item.pointer("/UsageData/Size")?.as_i64()?).ok()?;
                Some((name.to_owned(), size))
            })
            .collect()
    }

    /// Pauses the lane's own running containers while the lane is frozen. Shared
    /// containers and other lanes are untouched. Returns how many were paused; one that
    /// stopped, vanished or got paused by someone else in the meantime is skipped.
    pub async fn pause_lane(&self, project: &str, lane: u32) -> Result<usize, Error> {
        let mut paused = 0;
        for id in self.lane_ids_in(project, lane, "running").await? {
            match self.docker.pause_container(&id).await.map_err(Error::from) {
                Ok(()) => paused += 1,
                Err(error) if error.is_state_race() => {}
                Err(error) => return Err(error),
            }
        }
        Ok(paused)
    }

    /// Unpauses the lane's own paused containers. Returns how many were unpaused.
    ///
    /// Docker reports a container `unhealthy` from the unpause until its next
    /// healthcheck runs, without sending an event; [`Self::health`] reads it as
    /// healthy when it was healthy before the pause.
    pub async fn unpause_lane(&self, project: &str, lane: u32) -> Result<usize, Error> {
        let mut unpaused = 0;
        for id in self.lane_ids_in(project, lane, "paused").await? {
            match self
                .docker
                .unpause_container(&id)
                .await
                .map_err(Error::from)
            {
                Ok(()) => unpaused += 1,
                Err(error) if error.is_state_race() => {}
                Err(error) => return Err(error),
            }
        }
        Ok(unpaused)
    }

    /// Ids of the lane-scoped containers of `project` in Docker state `status`. The
    /// lane and scope labels together never match a shared container.
    async fn lane_ids_in(
        &self,
        project: &str,
        lane: u32,
        status: &str,
    ) -> Result<Vec<String>, Error> {
        let mut filters = label_filters(&scope_labels(project, Scope::Lane(lane)));
        filters.insert("status".to_owned(), vec![status.to_owned()]);
        Ok(self
            .list(&filters, true)
            .await?
            .into_iter()
            .filter_map(|summary| summary.id)
            .collect())
    }

    /// Container lifecycle events for containers carrying all `labels` (for example
    /// `dev.devlanes.project=acme`, or `com.docker.compose.project=acme-lane1`). The stream
    /// ends when Docker closes it (engine restart); resubscribe and re-check health then.
    /// Events start at the second this method is called, even if the stream is polled later.
    pub fn events(
        &self,
        labels: &BTreeMap<String, String>,
    ) -> BoxStream<'static, Result<HealthEvent, Error>> {
        let mut filters = label_filters(labels);
        filters.insert("type".to_owned(), vec!["container".to_owned()]);
        filters.insert(
            "event".to_owned(),
            WATCHED_EVENTS
                .iter()
                .map(|event| (*event).to_owned())
                .collect(),
        );
        // bollard sends the request on first poll; `since` keeps events that happen between
        // this call and that poll. Second granularity may replay events from just before.
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default()
            .to_string();
        let options = EventsOptionsBuilder::new()
            .since(&since)
            .filters(&filters)
            .build();
        self.docker
            .events(Some(options))
            .map_err(Error::from)
            .try_filter_map(|event| async move { Ok(map_event(&event)) })
            .boxed()
    }

    /// Lifecycle events of every container on the engine, plain or Compose, for as
    /// long as the stream is polled. When Docker closes the stream (an engine
    /// restart) it yields [`WatchEvent::Ended`], waits until the engine answers
    /// again, yields [`WatchEvent::Reconnected`] and subscribes again.
    pub fn watch(&self) -> BoxStream<'static, WatchEvent> {
        let events = self.clone();
        let ping = self.clone();
        resubscribing(
            move || events.events(&BTreeMap::new()),
            move || {
                let ping = ping.clone();
                async move { ping.server_version().await.is_ok() }
            },
            Duration::from_secs(1),
            Duration::from_secs(30),
        )
        .boxed()
    }

    /// Events of every plain container of `project`.
    pub fn project_events(&self, project: &str) -> BoxStream<'static, Result<HealthEvent, Error>> {
        self.events(&BTreeMap::from([(
            labels::PROJECT.to_owned(),
            project.to_owned(),
        )]))
    }

    /// Stdout and stderr of `container` (a name or id), as Docker sends it. Any
    /// container works, plain or compose. With `follow` the stream stays open
    /// until the container stops; without it, it ends after the stored output.
    pub fn logs(
        &self,
        container: &str,
        options: LogOptions,
    ) -> BoxStream<'static, Result<LogChunk, Error>> {
        let tail = options
            .tail
            .map_or_else(|| "all".to_owned(), |tail| tail.to_string());
        let mut builder = LogsOptionsBuilder::new()
            .stdout(true)
            .stderr(true)
            .follow(options.follow)
            .timestamps(options.timestamps)
            .tail(&tail);
        if let Some(since) = options.since {
            builder = builder.since(i32::try_from(since).unwrap_or(i32::MAX));
        }
        self.docker
            .logs(container, Some(builder.build()))
            .map_err(Error::from)
            .try_filter_map(|output| async move { Ok(LogChunk::from_output(output)) })
            .boxed()
    }

    async fn list(
        &self,
        filters: &HashMap<String, Vec<String>>,
        all: bool,
    ) -> Result<Vec<ContainerSummary>, Error> {
        let options = ListContainersOptionsBuilder::new()
            .all(all)
            .filters(filters)
            .build();
        Ok(self.docker.list_containers(Some(options)).await?)
    }

    async fn remove(&self, container: &str) -> Result<(), Error> {
        let options = RemoveContainerOptionsBuilder::new()
            .force(true)
            .v(true)
            .build();
        Ok(self
            .docker
            .remove_container(container, Some(options))
            .await?)
    }

    /// Creates labelled named volumes up front; Docker would otherwise create them on
    /// demand without labels, and `remove_lane` finds volumes by label.
    async fn create_volumes(&self, spec: &ContainerSpec, mounts: &[Mount]) -> Result<(), Error> {
        for mount in mounts {
            let Mount::Named { name, key, .. } = mount else {
                continue;
            };
            let mut volume_labels = scope_labels(&spec.project, spec.scope);
            volume_labels.insert(labels::VOLUME.to_owned(), key.clone());
            let request = VolumeCreateRequest {
                name: Some(name.clone()),
                labels: Some(volume_labels.into_iter().collect()),
                ..Default::default()
            };
            self.docker.create_volume(request).await?;
        }
        Ok(())
    }

    /// Creates the container, pulling the image once when it is missing locally.
    async fn create(
        &self,
        spec: &ContainerSpec,
        mounts: &[Mount],
        hash: &str,
        name: &str,
    ) -> Result<(), Error> {
        let body = create_body(spec, mounts, hash);
        let options = CreateContainerOptionsBuilder::new().name(name).build();
        match self
            .docker
            .create_container(Some(options.clone()), body.clone())
            .await
        {
            Ok(_) => Ok(()),
            Err(error) => {
                let error = Error::from(error);
                if !error.is_not_found() {
                    return Err(error);
                }
                self.pull(&spec.image).await?;
                self.docker.create_container(Some(options), body).await?;
                Ok(())
            }
        }
    }

    async fn pull(&self, image: &str) -> Result<(), Error> {
        let options = CreateImageOptionsBuilder::new().from_image(image).build();
        self.docker
            .create_image(Some(options), None, None)
            .try_for_each(|_| async { Ok(()) })
            .await?;
        Ok(())
    }
}

/// The Engine API body for `spec`. Ports bind to 127.0.0.1 so lane services are not
/// exposed on the LAN.
pub fn create_body(spec: &ContainerSpec, mounts: &[Mount], hash: &str) -> ContainerCreateBody {
    let mut bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
    for port in &spec.ports {
        bindings
            .entry(format!("{}/{}", port.container, port.protocol))
            .or_insert_with(|| Some(Vec::new()))
            .get_or_insert_with(Vec::new)
            .push(PortBinding {
                host_ip: Some("127.0.0.1".to_owned()),
                host_port: Some(port.host.to_string()),
            });
    }
    let mut exposed: Vec<String> = bindings.keys().cloned().collect();
    exposed.sort();
    let anonymous: Vec<String> = mounts
        .iter()
        .filter_map(|mount| match mount {
            Mount::Anonymous { target } => Some(target.clone()),
            _ => None,
        })
        .collect();
    ContainerCreateBody {
        image: Some(spec.image.clone()),
        cmd: spec.command.clone(),
        env: Some(
            spec.env
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect(),
        ),
        labels: Some(container_labels(spec, hash).into_iter().collect()),
        exposed_ports: Some(exposed),
        volumes: (!anonymous.is_empty()).then_some(anonymous),
        host_config: Some(HostConfig {
            binds: Some(mounts.iter().filter_map(Mount::bind).collect()),
            port_bindings: Some(bindings),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Docker's `label=key=value` filters; several labels must all match.
fn label_filters(labels: &BTreeMap<String, String>) -> HashMap<String, Vec<String>> {
    HashMap::from([(
        "label".to_owned(),
        labels
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect(),
    )])
}

fn summary_name(names: Option<&[String]>) -> String {
    names
        .and_then(<[String]>::first)
        .map(|name| name.trim_start_matches('/').to_owned())
        .unwrap_or_default()
}

fn managed_from_summary(summary: ContainerSummary) -> ManagedContainer {
    let labels: BTreeMap<String, String> = summary.labels.unwrap_or_default().into_iter().collect();
    let ports = summary
        .ports
        .unwrap_or_default()
        .into_iter()
        .filter_map(|port| Some((port.private_port, port.public_port?)))
        .collect();
    ManagedContainer {
        id: summary.id.unwrap_or_default(),
        name: summary_name(summary.names.as_deref()),
        key: labels.get(labels::CONTAINER).cloned(),
        scope: Scope::from_labels(&labels),
        health: health_from_summary(
            summary.state,
            summary.health.and_then(|health| health.status),
        ),
        ports,
        config_hash: labels.get(labels::CONFIG_HASH).cloned(),
    }
}

#[cfg(test)]
mod tests {
    use bollard::models::{ContainerSummaryStateEnum, PortSummary};

    use super::*;
    use crate::{NamedPort, PortBlock, Protocol};

    fn spec() -> ContainerSpec {
        ContainerSpec {
            project: "acme".into(),
            name: "cerbos".into(),
            image: "cerbos:1".into(),
            command: None,
            env: BTreeMap::from([("A".into(), "1".into())]),
            volumes: vec![
                "./policies:/policies:ro".into(),
                "data:/data".into(),
                "/tmp/x".into(),
            ],
            ports: vec![
                NamedPort {
                    name: "http".into(),
                    container: 3592,
                    host: 21000,
                    protocol: Protocol::Tcp,
                },
                NamedPort {
                    name: "dns".into(),
                    container: 53,
                    host: 21001,
                    protocol: Protocol::Udp,
                },
            ],
            scope: Scope::Lane(1),
            base_dir: "/work/acme".into(),
            block: PortBlock::new(21000, 10).expect("valid block"),
        }
    }

    /// A daemon started before the engine finds it on a later call: a socket
    /// file that appears between two calls is connected to. Only Unix fails
    /// at connect; a Windows pipe is first opened by a request.
    #[cfg(unix)]
    #[tokio::test]
    async fn connector_connects_once_the_engine_appears() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("docker.sock");
        let host = format!("unix://{}", path.display());
        let connector =
            DockerConnector::new(move |key| (key == "DOCKER_HOST").then(|| host.clone()));
        let error = connector.get().await.expect_err("no socket yet");
        assert!(error.is_engine_stopped(), "{error:?}");
        let _engine = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        assert!(connector.get().await.is_ok());
    }

    #[test]
    fn create_body_binds_ports_and_volumes() {
        let spec = spec();
        let mounts = validate(&spec).expect("valid");
        let body = create_body(&spec, &mounts, "hash");
        let host = body.host_config.expect("host config");
        // Relative sources resolve to host paths, `\`-separated on Windows.
        let policies = "/work/acme/policies".replace('/', std::path::MAIN_SEPARATOR_STR);
        assert_eq!(
            host.binds.expect("binds"),
            [
                format!("{policies}:/policies:ro"),
                "devlanes-acme-lane1-data:/data".to_owned()
            ]
        );
        let bindings = host.port_bindings.expect("bindings");
        let http = bindings["3592/tcp"].as_ref().expect("http binding");
        assert_eq!(http[0].host_ip.as_deref(), Some("127.0.0.1"));
        assert_eq!(http[0].host_port.as_deref(), Some("21000"));
        assert!(bindings.contains_key("53/udp"));
        assert_eq!(body.exposed_ports.expect("exposed"), ["3592/tcp", "53/udp"]);
        assert_eq!(body.volumes.expect("anonymous"), ["/tmp/x"]);
        assert_eq!(body.env.expect("env"), ["A=1"]);
        let labels = body.labels.expect("labels");
        assert_eq!(labels["dev.devlanes.config-hash"], "hash");
        assert_eq!(labels["dev.devlanes.lane"], "1");
    }

    #[test]
    fn filters_require_every_label() {
        let filters = label_filters(&scope_labels("acme", Scope::Lane(2)));
        assert_eq!(
            filters["label"],
            [
                "dev.devlanes.kind=container",
                "dev.devlanes.lane=2",
                "dev.devlanes.project=acme",
                "dev.devlanes.scope=lane"
            ]
        );
    }

    #[test]
    fn managed_container_from_summary() {
        let summary = ContainerSummary {
            id: Some("abc".into()),
            names: Some(vec!["/devlanes-acme-lane1-cerbos".into()]),
            labels: Some(
                container_labels(&spec(), "h1")
                    .into_iter()
                    .collect::<HashMap<_, _>>(),
            ),
            state: Some(ContainerSummaryStateEnum::RUNNING),
            ports: Some(vec![
                PortSummary {
                    ip: Some("127.0.0.1".into()),
                    private_port: 3592,
                    public_port: Some(21000),
                    typ: None,
                },
                PortSummary {
                    ip: None,
                    private_port: 9999,
                    public_port: None,
                    typ: None,
                },
            ]),
            ..Default::default()
        };
        let managed = managed_from_summary(summary);
        assert_eq!(managed.name, "devlanes-acme-lane1-cerbos");
        assert_eq!(managed.key.as_deref(), Some("cerbos"));
        assert_eq!(managed.scope, Some(Scope::Lane(1)));
        assert_eq!(managed.health, Health::Running);
        assert_eq!(managed.ports, BTreeMap::from([(3592, 21000)]));
        assert_eq!(managed.config_hash.as_deref(), Some("h1"));
    }
}
