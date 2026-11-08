//! The live infra of one lane, the only place its values are kept: the stored
//! `infra` namespace, the containers behind it, what the engine last said
//! about each namespace key and which runs a bring-up or settle confirmed
//! healthy. A key restarted when its observed runs differ from the stored
//! ones; runs are compared for equality only. Competing writes are ordered per
//! key by the daemon's monotonic clock, so a read that began earlier never
//! overwrites what a later one or an event stored.

use std::collections::{BTreeMap, BTreeSet};

use devlanes_resolve::{DownReason, InfraState};
use serde_json::{Map, Value};
use tokio::time::Instant;

use crate::integrations::{
    ContainerRun, InfraContainer, InfraEvent, InfraEventKind, InfraUp, Observation, StartedAt,
};

/// The live infra of one lane, behind [`super::LaneSt::infra`].
#[derive(Default)]
pub(crate) struct LaneInfra {
    namespace: Option<Map<String, Value>>,
    containers: Vec<InfraContainer>,
    /// The names the lane's infra can hold, from the last read.
    declared: Option<BTreeSet<String>>,
    keys: BTreeMap<String, Key>,
    refresh_queued: bool,
}

/// What the owner knows about one namespace key.
#[derive(Default)]
struct Key {
    state: KeyState,
    /// The runs whose health a bring-up or settle confirmed.
    settled: Option<Vec<ContainerRun>>,
    /// `began` of the write that last set `state`.
    observed_at: Option<Instant>,
    /// A `start` event or a resync arrived at this instant.
    suspect_since: Option<Instant>,
}

/// One field, so the runs and the reason cannot disagree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum KeyState {
    /// Never observed, the last observation was `Unknown`, the runs changed and
    /// their ports were not read yet, or a read's two observations differed.
    #[default]
    Unknown,
    /// Observed or reported not running, and why.
    Down(DownReason),
    /// Running containers with their runs, non-empty, sorted by Docker name.
    /// The stored entry and containers of the key belong to these runs.
    Running(Vec<ContainerRun>),
}

impl KeyState {
    /// The state a definite observation gives; `None` for `Unknown`.
    fn of(observation: &Observation) -> Option<Self> {
        match observation {
            Observation::Running(runs) => Some(Self::Running(runs.clone())),
            Observation::NotRunning { .. } => Some(Self::Down(DownReason::NotRunning)),
            Observation::EngineStopped => Some(Self::Down(DownReason::EngineNotRunning)),
            Observation::Unknown => None,
        }
    }
}

/// One provider read: `began` is taken first, then the declared names, the
/// runs of every declared key, the namespace, and the runs of the same keys
/// again.
pub(crate) struct InfraRead {
    pub began: Instant,
    pub declared: BTreeSet<String>,
    pub before: BTreeMap<String, Observation>,
    pub up: InfraUp,
    pub after: BTreeMap<String, Observation>,
}

/// The lane's live infra for a command outside a start.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CurrentInfra {
    /// The stored namespace entries (`{name: {port, ports}}`) of the names whose
    /// state is `Running`, and nothing else, as resolution reads `infra`.
    pub namespace: Value,
    /// Every declared name with its state. A name it lacks is not declared.
    pub states: BTreeMap<String, InfraState>,
}

/// What [`LaneInfra::apply_event`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EventEffect {
    /// A stored container has the event's Docker name.
    pub matched: bool,
    /// The display state of a stored container changed.
    pub changed: bool,
    /// A key became suspect, so the lane should be checked.
    pub suspect: bool,
}

/// What a read says about one key, from its two observations.
enum Paired<'a> {
    /// Both observed the same runs, so the ports read in between are theirs.
    Running(&'a [ContainerRun]),
    /// Both observed it down.
    Down(DownReason, Option<&'a str>),
    /// They differ, or one did not answer.
    Unknown,
}

/// Pairs a key's observations before and after a namespace read.
fn pair<'a>(before: Option<&'a Observation>, after: Option<&'a Observation>) -> Paired<'a> {
    match (before, after) {
        (Some(Observation::Running(a)), Some(Observation::Running(b))) if a == b => {
            Paired::Running(b)
        }
        (Some(Observation::NotRunning { .. }), Some(Observation::NotRunning { state })) => {
            Paired::Down(DownReason::NotRunning, Some(state))
        }
        (Some(Observation::EngineStopped), Some(Observation::EngineStopped)) => {
            Paired::Down(DownReason::EngineNotRunning, None)
        }
        _ => Paired::Unknown,
    }
}

impl LaneInfra {
    /// The stored namespace; `None` while the lane's infra never ran.
    pub(crate) fn namespace(&self) -> Option<&Map<String, Value>> {
        self.namespace.as_ref()
    }

    /// The stored namespace as resolution reads `infra`: `null` when none.
    pub(crate) fn namespace_value(&self) -> Value {
        self.namespace.clone().map_or(Value::Null, Value::Object)
    }

    /// The containers behind the stored namespace, for stats, logs and port holds.
    pub(crate) fn containers(&self) -> &[InfraContainer] {
        &self.containers
    }

    /// Whether a namespace is stored.
    pub(crate) fn tracked(&self) -> bool {
        self.namespace.is_some()
    }

    /// The run setup records compare with: that of the key's first running
    /// container by Docker name, while the key is known to run.
    pub(crate) fn setup_run(&self, key: &str) -> Option<&StartedAt> {
        match &self.keys.get(key)?.state {
            KeyState::Running(runs) => runs.first().map(|run| &run.started_at),
            _ => None,
        }
    }

    /// The runs of `key` while it is known to run.
    pub(crate) fn runs(&self, key: &str) -> Option<&[ContainerRun]> {
        match &self.keys.get(key)?.state {
            KeyState::Running(runs) => Some(runs),
            _ => None,
        }
    }

    /// Each declared name with its state, and the stored entries of the
    /// running ones.
    pub(crate) fn current(&self) -> CurrentInfra {
        let mut states = BTreeMap::new();
        let mut namespace = Map::new();
        for name in self.declared.iter().flatten() {
            let state = match self.keys.get(name).map(|key| &key.state) {
                Some(KeyState::Running(_)) => InfraState::Running,
                Some(KeyState::Down(reason)) => InfraState::Down(*reason),
                Some(KeyState::Unknown) | None => InfraState::Unknown,
            };
            if state == InfraState::Running
                && let Some(entry) = self.namespace.as_ref().and_then(|ns| ns.get(name))
            {
                namespace.insert(name.clone(), entry.clone());
            }
            states.insert(name.clone(), state);
        }
        CurrentInfra {
            namespace: Value::Object(namespace),
            states,
        }
    }

    /// The keys a refresh must observe, plus `extra`: suspect keys and declared
    /// keys not known to run, or every declared key while no namespace is
    /// stored. A down container is checked on every refresh, so a lost `start`
    /// event is made up for by the next one.
    pub(crate) fn to_verify(&self, extra: &[String]) -> BTreeSet<String> {
        let mut keys: BTreeSet<String> = extra.iter().cloned().collect();
        let declared = self.declared.iter().flatten();
        if !self.tracked() {
            keys.extend(declared.cloned());
            return keys;
        }
        for name in declared {
            let running = self
                .keys
                .get(name)
                .is_some_and(|key| matches!(key.state, KeyState::Running(_)));
            if !running {
                keys.insert(name.clone());
            }
        }
        keys.extend(
            self.keys
                .iter()
                .filter(|(_, key)| key.suspect_since.is_some())
                .map(|(name, _)| name.clone()),
        );
        keys
    }

    /// Running keys whose runs no bring-up or settle confirmed healthy.
    pub(crate) fn unsettled(&self) -> BTreeSet<String> {
        self.keys
            .iter()
            .filter(|(_, key)| match &key.state {
                KeyState::Running(runs) => key.settled.as_ref() != Some(runs),
                _ => false,
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Whether a start into the running lane has nothing to check (with the
    /// setup-backed keys `extra`) and no run to wait for.
    pub(crate) fn settled(&self, extra: &[String]) -> bool {
        self.to_verify(extra).is_empty() && self.unsettled().is_empty()
    }

    /// A full write: the read's entries and containers of every key it paired,
    /// the state of every key it observed, and the keys it no longer declares
    /// dropped. Keys a later write set are left alone. Returns whether the
    /// namespace changed.
    pub(super) fn replace(&mut self, read: InfraRead) -> bool {
        let old = self.namespace.clone();
        self.namespace.get_or_insert_with(Map::new);
        let gone: Vec<String> = self
            .keys
            .iter()
            .filter(|(name, key)| !read.declared.contains(*name) && admits(key, read.began))
            .map(|(name, _)| name.clone())
            .collect();
        for name in gone {
            self.keys.remove(&name);
            self.drop_stored(&name);
        }
        let stored = self.namespace.iter().flat_map(|ns| ns.keys().cloned());
        let stale: Vec<String> = stored
            .chain(self.containers.iter().map(|c| c.name.clone()))
            .filter(|name| !read.declared.contains(name) && !self.keys.contains_key(name))
            .collect();
        for name in stale {
            self.drop_stored(&name);
        }
        for name in &read.declared {
            self.apply_read(&read, name);
        }
        self.declared = Some(read.declared);
        old != self.namespace
    }

    /// A partial write of `keys` from `read`, as [`Self::replace`] applies
    /// them. Stores nothing while no namespace is stored, so a refresh that
    /// read before a lane was cleaned cannot bring its infra back. Returns
    /// whether the namespace changed.
    pub(super) fn merge(&mut self, read: &InfraRead, keys: &BTreeSet<String>) -> bool {
        if !self.tracked() {
            return false;
        }
        let old = self.namespace.clone();
        for name in keys {
            self.apply_read(read, name);
        }
        old != self.namespace
    }

    /// Keeps the declared names of a lane whose infra never ran, so
    /// [`Self::current`] answers for each of them.
    pub(super) fn note_declared(&mut self, declared: BTreeSet<String>) {
        self.declared = Some(declared);
    }

    /// Stores containers only, such as the ones an `up` that failed left running.
    pub(super) fn store_containers(&mut self, containers: Vec<InfraContainer>) {
        self.containers = containers;
    }

    /// Records each definite observation as the key's state, through the
    /// ordering rule. A key whose runs changed becomes unknown until a read
    /// pairs its ports with the new runs. A stopped engine counts for every
    /// declared key. Returns the keys that run with other runs than stored.
    pub(super) fn apply_observed(
        &mut self,
        observed: &BTreeMap<String, Observation>,
        began: Instant,
    ) -> BTreeSet<String> {
        let stopped = Observation::EngineStopped;
        let mut observed: Vec<(String, &Observation)> =
            observed.iter().map(|(k, o)| (k.clone(), o)).collect();
        if observed.iter().any(|(_, o)| **o == stopped) {
            for name in self.declared.iter().flatten() {
                if !observed.iter().any(|(k, _)| k == name) {
                    observed.push((name.clone(), &stopped));
                }
            }
        }
        let mut changed = BTreeSet::new();
        for (name, observation) in observed {
            let Some(state) = KeyState::of(observation) else {
                self.set_unknown(&name, began);
                continue;
            };
            let key = self.keys.entry(name.clone()).or_default();
            if !admits(key, began) {
                continue;
            }
            let state = match state {
                KeyState::Running(_) if key.state != state => {
                    changed.insert(name.clone());
                    KeyState::Unknown
                }
                state => state,
            };
            settle_write(key, state, began);
            if let Observation::NotRunning { state } = observation {
                self.set_display(&name, state);
            }
        }
        changed
    }

    /// Records that `keys` were asked about and got no answer: a key no later
    /// write set becomes unknown, so the next refresh checks it again.
    pub(super) fn mark_unknown(&mut self, keys: &BTreeSet<String>, began: Instant) {
        for name in keys {
            self.set_unknown(name, began);
        }
    }

    /// Applies a container event at `now`: the display state of the stored
    /// containers with its Docker name, a `start` marks their key suspect, and
    /// a stop or exit removes that container's run (the key is down once no
    /// run is left).
    pub(super) fn apply_event(&mut self, event: &InfraEvent, now: Instant) -> EventEffect {
        let mut effect = EventEffect::default();
        let state = event.container_state();
        let mut names = BTreeSet::new();
        for container in self
            .containers
            .iter_mut()
            .filter(|container| container.docker == event.container)
        {
            effect.matched = true;
            names.insert(container.name.clone());
            // `stop` follows `die`, which already carried the exit code.
            let keeps_code = state == "exited"
                && container
                    .state
                    .as_deref()
                    .is_some_and(|old| old.starts_with("exited "));
            if !keeps_code && container.state.as_deref() != Some(state.as_str()) {
                container.state = Some(state.clone());
                effect.changed = true;
            }
        }
        let tracked = self.tracked();
        for name in names {
            let key = self.keys.entry(name).or_default();
            match event.kind {
                InfraEventKind::Started if tracked => {
                    key.suspect_since = Some(now);
                    effect.suspect = true;
                }
                InfraEventKind::Died | InfraEventKind::Stopped | InfraEventKind::Removed => {
                    if let KeyState::Running(runs) = &key.state
                        && admits(key, now)
                    {
                        let left: Vec<ContainerRun> = runs
                            .iter()
                            .filter(|run| run.docker != event.container)
                            .cloned()
                            .collect();
                        let state = if left.is_empty() {
                            KeyState::Down(DownReason::NotRunning)
                        } else {
                            KeyState::Running(left)
                        };
                        key.state = state;
                        key.observed_at = Some(now);
                    }
                }
                _ => {}
            }
        }
        effect
    }

    /// Marks every key of a lane that stores its infra suspect at `now`: events
    /// may have been lost.
    pub(super) fn suspect_all(&mut self, now: Instant) -> bool {
        if !self.tracked() {
            return false;
        }
        let names: Vec<String> = self.declared.iter().flatten().cloned().collect();
        for name in names {
            self.keys.entry(name).or_default();
        }
        for key in self.keys.values_mut() {
            key.suspect_since = Some(now);
        }
        true
    }

    /// Confirms `runs` of `key` healthy, while they are still its runs.
    pub(super) fn mark_settled(&mut self, key: &str, runs: &[ContainerRun]) {
        if let Some(key) = self.keys.get_mut(key)
            && matches!(&key.state, KeyState::Running(now) if now == runs)
        {
            key.settled = Some(runs.to_vec());
        }
    }

    /// Schedules the background refresh unless one is scheduled.
    pub(super) fn queue_refresh(&mut self) -> bool {
        !std::mem::replace(&mut self.refresh_queued, true)
    }

    /// The scheduled refresh started.
    pub(super) fn refresh_taken(&mut self) {
        self.refresh_queued = false;
    }

    /// Applies one key of `read` through the ordering rule: the entry and
    /// containers only for equal running observations with an entry to apply,
    /// the state always.
    fn apply_read(&mut self, read: &InfraRead, name: &str) {
        let key = self.keys.entry(name.to_owned()).or_default();
        if !admits(key, read.began) {
            return;
        }
        let entry = match &read.up.namespace {
            Value::Object(namespace) => namespace.get(name),
            _ => None,
        };
        match (pair(read.before.get(name), read.after.get(name)), entry) {
            (Paired::Running(runs), Some(entry)) => {
                settle_write(key, KeyState::Running(runs.to_vec()), read.began);
                if let Some(namespace) = self.namespace.as_mut() {
                    namespace.insert(name.to_owned(), entry.clone());
                }
                self.containers.retain(|container| container.name != name);
                self.containers.extend(
                    read.up
                        .containers
                        .iter()
                        .filter(|container| container.name == name)
                        .cloned(),
                );
            }
            (Paired::Down(reason, display), _) => {
                settle_write(key, KeyState::Down(reason), read.began);
                if let Some(display) = display {
                    self.set_display(name, display);
                }
            }
            (Paired::Running(_) | Paired::Unknown, _) => self.set_unknown(name, read.began),
        }
    }

    /// `Unknown` for `name` unless a write that began after `began` set it.
    fn set_unknown(&mut self, name: &str, began: Instant) {
        let key = self.keys.entry(name.to_owned()).or_default();
        if admits(key, began) {
            key.state = KeyState::Unknown;
        }
    }

    /// The display state of the stored containers of `name`.
    fn set_display(&mut self, name: &str, state: &str) {
        for container in self.containers.iter_mut().filter(|c| c.name == name) {
            container.state = Some(state.to_owned());
        }
    }

    /// Forgets the stored entry and containers of `name`.
    fn drop_stored(&mut self, name: &str) {
        if let Some(namespace) = self.namespace.as_mut() {
            namespace.remove(name);
        }
        self.containers.retain(|container| container.name != name);
    }
}

/// Whether a write that began at `began` may set `key`: no later write did.
fn admits(key: &Key, began: Instant) -> bool {
    key.observed_at.is_none_or(|at| began >= at)
}

/// Sets a definite state written at `began`, clearing a suspect mark set
/// before it.
fn settle_write(key: &mut Key, state: KeyState, began: Instant) {
    key.state = state;
    key.observed_at = Some(began);
    if key.suspect_since.is_some_and(|since| since < began) {
        key.suspect_since = None;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;

    /// One step of a case, at `at` milliseconds after the case's start.
    enum Step {
        /// A full read whose namespace gives each key the port.
        Replace(u64, Reads),
        /// A partial read of the keys.
        Merge(u64, Reads, &'static [&'static str]),
        /// Observations only.
        Observe(u64, &'static [(&'static str, Obs)]),
        /// A container event of the key's replica.
        Event(u64, &'static str, u32, InfraEventKind),
        /// A resync.
        Resync(u64),
        /// The declared names of a lane whose infra never ran.
        Declare,
    }

    /// What a read observes per key: before, port, after.
    type Reads = &'static [(&'static str, Obs, Option<u16>, Obs)];

    /// An observation of a key's containers: `Run(n)` runs `<key>-1` (and
    /// `<key>-2` with `Two`) in run `n`.
    #[derive(Clone, Copy)]
    enum Obs {
        Run(u32),
        Two(u32, u32),
        Down,
        Stopped,
        Unknown,
    }

    fn run(key: &str, replica: u32, n: u32) -> ContainerRun {
        ContainerRun {
            docker: format!("{key}-{replica}"),
            started_at: format!("2026-01-01T00:00:{n:02}Z").parse().expect("run"),
        }
    }

    fn observation(key: &str, obs: Obs) -> Observation {
        match obs {
            Obs::Run(n) => Observation::Running(vec![run(key, 1, n)]),
            Obs::Two(a, b) => Observation::Running(vec![run(key, 1, a), run(key, 2, b)]),
            Obs::Down => Observation::NotRunning {
                state: "exited 0".into(),
            },
            Obs::Stopped => Observation::EngineStopped,
            Obs::Unknown => Observation::Unknown,
        }
    }

    fn container(key: &str, replica: u32) -> InfraContainer {
        InfraContainer {
            name: key.into(),
            docker: format!("{key}-{replica}"),
            shared: false,
            compose_project: Some("acme".into()),
            publishes: true,
            state: Some("running".into()),
        }
    }

    /// The names every case declares.
    fn declared() -> BTreeSet<String> {
        ["cache", "db", "search"].map(String::from).into()
    }

    fn read(start: Instant, at: u64, reads: Reads) -> InfraRead {
        let mut namespace = Map::new();
        let mut containers = Vec::new();
        let mut before = BTreeMap::new();
        let mut after = BTreeMap::new();
        for (key, first, port, second) in reads {
            namespace.insert((*key).into(), json!({"port": port, "ports": {}}));
            containers.push(container(key, 1));
            if matches!(second, Obs::Two(..)) {
                containers.push(container(key, 2));
            }
            before.insert((*key).into(), observation(key, *first));
            after.insert((*key).into(), observation(key, *second));
        }
        InfraRead {
            began: start + Duration::from_millis(at),
            declared: declared(),
            before,
            up: InfraUp {
                namespace: Value::Object(namespace),
                containers,
                ..InfraUp::default()
            },
            after,
        }
    }

    fn port(infra: &LaneInfra, key: &str) -> Option<u64> {
        infra.namespace()?.get(key)?.get("port")?.as_u64()
    }

    fn apply(infra: &mut LaneInfra, start: Instant, steps: &[Step]) {
        let at = |ms: &u64| start + Duration::from_millis(*ms);
        for step in steps {
            match step {
                Step::Replace(ms, reads) => {
                    infra.replace(read(start, *ms, reads));
                }
                Step::Merge(ms, reads, keys) => {
                    let keys = keys.iter().map(|k| (*k).to_owned()).collect();
                    infra.merge(&read(start, *ms, reads), &keys);
                }
                Step::Observe(ms, observed) => {
                    let observed = observed
                        .iter()
                        .map(|(key, obs)| ((*key).to_owned(), observation(key, *obs)))
                        .collect();
                    infra.apply_observed(&observed, at(ms));
                }
                Step::Event(ms, key, replica, kind) => {
                    let event = InfraEvent {
                        project: "acme".into(),
                        lane: None,
                        container: format!("{key}-{replica}"),
                        kind: *kind,
                        exit_code: None,
                    };
                    infra.apply_event(&event, at(ms));
                }
                Step::Resync(ms) => {
                    infra.suspect_all(at(ms));
                }
                Step::Declare => infra.note_declared(declared()),
            }
        }
    }

    /// The owner's rules, each case a sequence of writes and the port, state
    /// and recheck it leaves for one key. `search` is never observed, so it
    /// stays unknown and is always checked.
    #[test]
    fn writes_are_paired_ordered_and_rechecked() {
        use InfraEventKind::{Died, Started};
        use Obs::{Down, Run, Stopped, Two, Unknown};
        let up = InfraState::Running;
        let down = InfraState::Down(DownReason::NotRunning);
        let unknown = InfraState::Unknown;
        /// The case, its steps, the key, its port and state, and whether the
        /// next refresh checks it.
        type Case = (
            &'static str,
            Vec<Step>,
            &'static str,
            Option<u64>,
            InfraState,
            bool,
        );
        let cases: Vec<Case> = vec![
            (
                "a run that changed between the observations is not applied",
                vec![
                    Step::Replace(0, &[("db", Run(1), Some(5432), Run(1))]),
                    Step::Merge(10, &[("db", Run(1), Some(6000), Run(2))], &["db"]),
                ],
                "db",
                Some(5432),
                unknown,
                true,
            ),
            (
                "equal runs apply their entry, also one that lost its port",
                vec![
                    Step::Replace(0, &[("db", Run(1), Some(5432), Run(1))]),
                    Step::Replace(10, &[("db", Run(1), None, Run(1))]),
                ],
                "db",
                None,
                up,
                false,
            ),
            (
                "a run with an earlier time than the stored one is a restart",
                vec![
                    Step::Replace(0, &[("db", Run(5), Some(5432), Run(5))]),
                    Step::Observe(10, &[("db", Run(1))]),
                ],
                "db",
                Some(5432),
                unknown,
                true,
            ),
            (
                "a full read after scaling down stores the one replica left",
                vec![
                    Step::Replace(0, &[("db", Two(1, 2), Some(5432), Two(1, 2))]),
                    Step::Replace(10, &[("db", Run(1), Some(5433), Run(1))]),
                ],
                "db",
                Some(5433),
                up,
                false,
            ),
            (
                "a read that could not observe keeps the entry and checks again",
                vec![
                    Step::Replace(0, &[("db", Run(1), Some(5432), Run(1))]),
                    Step::Replace(10, &[("db", Unknown, Some(6000), Unknown)]),
                ],
                "db",
                Some(5432),
                unknown,
                true,
            ),
            (
                "a merge that began before a replace is dropped",
                vec![
                    Step::Replace(10, &[("db", Run(2), Some(6000), Run(2))]),
                    Step::Merge(5, &[("db", Run(1), Some(5432), Run(1))], &["db"]),
                ],
                "db",
                Some(6000),
                up,
                false,
            ),
            (
                "a replace that began before a merge keeps the merge's key",
                vec![
                    Step::Replace(0, &[("db", Run(1), Some(5432), Run(1))]),
                    Step::Merge(10, &[("db", Run(2), Some(6000), Run(2))], &["db"]),
                    Step::Replace(5, &[("db", Run(1), Some(5432), Run(1))]),
                ],
                "db",
                Some(6000),
                up,
                false,
            ),
            (
                "a merge on a lane that stores nothing stores nothing, and all is checked",
                vec![
                    Step::Declare,
                    Step::Merge(0, &[("db", Run(1), Some(5432), Run(1))], &["db"]),
                ],
                "db",
                None,
                unknown,
                true,
            ),
            (
                "a key read down twice keeps its port and is checked again",
                vec![
                    Step::Replace(0, &[("db", Run(1), Some(5432), Run(1))]),
                    Step::Replace(10, &[("db", Down, None, Down)]),
                ],
                "db",
                Some(5432),
                down,
                true,
            ),
            (
                "an exit of one of two replicas keeps the key running",
                vec![
                    Step::Replace(0, &[("db", Two(1, 2), Some(5432), Two(1, 2))]),
                    Step::Event(10, "db", 1, Died),
                ],
                "db",
                Some(5432),
                up,
                false,
            ),
            (
                "an exit of the last replica makes the key down",
                vec![
                    Step::Replace(0, &[("db", Two(1, 2), Some(5432), Two(1, 2))]),
                    Step::Event(10, "db", 1, Died),
                    Step::Event(20, "db", 2, Died),
                ],
                "db",
                Some(5432),
                down,
                true,
            ),
            (
                "a start marked after a read began survives that read",
                vec![
                    Step::Replace(0, &[("db", Run(1), Some(5432), Run(1))]),
                    Step::Event(20, "db", 1, Started),
                    Step::Observe(10, &[("db", Run(1))]),
                ],
                "db",
                Some(5432),
                up,
                true,
            ),
            (
                "a resync observed with the same runs needs no check",
                vec![
                    Step::Replace(0, &[("db", Run(1), Some(5432), Run(1))]),
                    Step::Resync(5),
                    Step::Observe(10, &[("db", Run(1))]),
                ],
                "db",
                Some(5432),
                up,
                false,
            ),
            (
                "a stopped engine makes every declared name down",
                vec![
                    Step::Replace(0, &[("db", Run(1), Some(5432), Run(1))]),
                    Step::Observe(10, &[("db", Stopped)]),
                ],
                "cache",
                None,
                InfraState::Down(DownReason::EngineNotRunning),
                true,
            ),
            (
                "an engine that answers again says which one is down",
                vec![
                    Step::Replace(0, &[("db", Run(1), Some(5432), Run(1))]),
                    Step::Observe(10, &[("db", Stopped)]),
                    Step::Observe(20, &[("db", Down)]),
                ],
                "db",
                Some(5432),
                down,
                true,
            ),
            (
                "a name never observed is unknown, never down",
                vec![Step::Replace(0, &[("db", Run(1), Some(5432), Run(1))])],
                "search",
                None,
                unknown,
                true,
            ),
        ];
        for (case, steps, key, expected_port, state, checked) in cases {
            let start = Instant::now();
            let mut infra = LaneInfra::default();
            apply(&mut infra, start, &steps);
            assert_eq!(port(&infra, key), expected_port, "{case}");
            let current = infra.current();
            assert_eq!(current.states.get(key), Some(&state), "{case}");
            assert_eq!(
                current.namespace.get(key).is_some(),
                state == InfraState::Running,
                "{case}: commands see only running entries"
            );
            assert_eq!(infra.to_verify(&[]).contains(key), checked, "{case}");
        }
    }
}
