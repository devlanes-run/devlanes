use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer};

use crate::Error;

/// The parts of `docker compose config --format json` devlanes reads. Unknown keys are
/// ignored so newer Compose releases keep parsing.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ComposeModel {
    pub name: String,
    #[serde(default)]
    pub services: BTreeMap<String, ServiceModel>,
    #[serde(default)]
    pub volumes: BTreeMap<String, VolumeModel>,
}

/// One resolved service. Compose has already applied interpolation, `extends`, `include`
/// and profile filtering.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ServiceModel {
    pub image: Option<String>,
    pub container_name: Option<String>,
    #[serde(default)]
    pub ports: Vec<PortModel>,
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub depends_on: BTreeMap<String, serde_json::Value>,
    pub healthcheck: Option<serde_json::Value>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub volumes: Vec<ServiceVolume>,
    /// The legacy `scale:` key; `deploy.replicas` wins when both are set.
    pub scale: Option<u64>,
    pub deploy: Option<DeployModel>,
}

impl ServiceModel {
    /// How many containers `up` runs for the service.
    pub fn replicas(&self) -> u64 {
        self.deploy
            .as_ref()
            .and_then(|deploy| deploy.replicas)
            .or(self.scale)
            .unwrap_or(1)
    }
}

/// The part of a service's `deploy:` section devlanes reads.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct DeployModel {
    pub replicas: Option<u64>,
}

/// A port in Compose's long syntax. `published` is `None` for ephemeral host ports and may
/// be a range such as `9200-9201`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PortModel {
    pub target: u16,
    #[serde(default, deserialize_with = "string_or_number")]
    pub published: Option<String>,
    pub host_ip: Option<String>,
    #[serde(default = "tcp")]
    pub protocol: String,
}

/// A service mount. Only named volumes matter for lane isolation checks.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ServiceVolume {
    #[serde(rename = "type")]
    pub kind: String,
    pub source: Option<String>,
    pub target: String,
}

/// A top-level volume. Compose fills `name` with `<project>_<key>` unless the file sets one.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct VolumeModel {
    pub name: Option<String>,
    #[serde(default)]
    pub external: Option<serde_json::Value>,
}

impl VolumeModel {
    /// External volumes are shared by every project that names them.
    pub fn is_external(&self) -> bool {
        match &self.external {
            Some(serde_json::Value::Bool(external)) => *external,
            Some(serde_json::Value::Object(_)) => true,
            _ => false,
        }
    }
}

impl ComposeModel {
    /// Parses the JSON printed by `docker compose config --format json`.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Fails for names that are absent from the model, so typos in `exclude`/`per_lane`
    /// and profile mismatches surface before anything starts.
    pub fn require_services<'a>(
        &self,
        names: impl IntoIterator<Item = &'a String>,
    ) -> Result<(), Error> {
        for name in names {
            if !self.services.contains_key(name) {
                return Err(Error::UnknownService(name.clone()));
            }
        }
        Ok(())
    }

    /// Services the shared project starts: everything resolved except `exclude` and
    /// `per_lane`. Fails when a started service depends on a skipped one, because Compose
    /// would start the dependency anyway.
    pub fn shared_services(
        &self,
        exclude: &BTreeSet<String>,
        per_lane: &BTreeSet<String>,
    ) -> Result<Vec<String>, Error> {
        self.require_services(exclude)?;
        self.require_services(per_lane)?;
        let selected: Vec<String> = self
            .services
            .keys()
            .filter(|name| !exclude.contains(*name) && !per_lane.contains(*name))
            .cloned()
            .collect();
        for name in &selected {
            let Some(service) = self.services.get(name) else {
                continue;
            };
            for dependency in service.depends_on.keys() {
                if exclude.contains(dependency) || per_lane.contains(dependency) {
                    return Err(Error::ExcludedDependency {
                        service: name.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
        }
        Ok(selected)
    }

    /// Named volumes of `services` that a lane project would share with other projects,
    /// because they are external or have an explicit `name:` outside the project prefix.
    pub fn shared_volumes(&self, services: &[String]) -> Vec<String> {
        let prefix = format!("{}_", self.name);
        let mut shared = BTreeSet::new();
        for service in services.iter().filter_map(|name| self.services.get(name)) {
            for mount in service
                .volumes
                .iter()
                .filter(|mount| mount.kind == "volume")
            {
                let Some(source) = &mount.source else {
                    continue;
                };
                let Some(volume) = self.volumes.get(source) else {
                    continue;
                };
                let named_outside = volume
                    .name
                    .as_ref()
                    .is_some_and(|name| !name.starts_with(&prefix));
                if volume.is_external() || named_outside {
                    shared.insert(source.clone());
                }
            }
        }
        shared.into_iter().collect()
    }
}

/// Container health as reported by `docker compose ps`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum Health {
    /// Running without a healthcheck.
    Running,
    /// Running and the healthcheck has not passed yet.
    Starting,
    Healthy,
    Unhealthy,
    Exited {
        code: i64,
    },
    /// Created, paused, restarting or another transitional state.
    Other {
        state: String,
    },
    /// The service was selected but no container exists.
    Missing,
}

/// One row of `docker compose ps --all --format json`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ContainerStatus {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Service")]
    pub service: String,
    #[serde(rename = "Project", default)]
    pub project: String,
    #[serde(rename = "State")]
    pub state: String,
    #[serde(rename = "Health", default)]
    pub health: String,
    #[serde(rename = "ExitCode", default)]
    pub exit_code: i64,
    #[serde(rename = "Publishers", default, deserialize_with = "null_as_empty")]
    pub publishers: Vec<Publisher>,
    /// The `com.docker.compose.config-hash` label: the service config the container
    /// was created from, as `docker compose config --hash` prints it.
    #[serde(rename = "Labels", default, deserialize_with = "config_hash_label")]
    pub config_hash: Option<String>,
}

/// Compose's label holding a container's service config hash.
const CONFIG_HASH_LABEL: &str = "com.docker.compose.config-hash";

/// Reads [`CONFIG_HASH_LABEL`] from `ps`'s `Labels`, which Compose prints as one
/// `key=value,key=value` string; a map is accepted too. Other labels can hold commas
/// (paths), so only this one is kept.
fn config_hash_label<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Ok(
        match Option::<serde_json::Value>::deserialize(deserializer)? {
            Some(serde_json::Value::String(text)) => text.split(',').find_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                (key == CONFIG_HASH_LABEL).then(|| value.to_owned())
            }),
            Some(serde_json::Value::Object(map)) => map
                .get(CONFIG_HASH_LABEL)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            _ => None,
        },
    )
}

/// A published port of a running container. Docker lists IPv4 and IPv6 bindings separately.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Publisher {
    #[serde(rename = "URL", default)]
    pub url: String,
    #[serde(rename = "TargetPort")]
    pub target_port: u16,
    #[serde(rename = "PublishedPort")]
    pub published_port: u16,
    #[serde(rename = "Protocol", default = "tcp")]
    pub protocol: String,
}

impl ContainerStatus {
    /// Combines the container state and healthcheck status into one value.
    pub fn health(&self) -> Health {
        match self.state.as_str() {
            "running" => match self.health.as_str() {
                "healthy" => Health::Healthy,
                "unhealthy" => Health::Unhealthy,
                "starting" => Health::Starting,
                _ => Health::Running,
            },
            "exited" | "dead" => Health::Exited {
                code: self.exit_code,
            },
            other => Health::Other {
                state: other.to_owned(),
            },
        }
    }
}

/// Parses `ps --format json` output. Compose 2.21+ prints one object per line; older
/// releases print a single array.
pub fn parse_ps(output: &str) -> Result<Vec<ContainerStatus>, serde_json::Error> {
    let trimmed = output.trim();
    if trimmed.starts_with('[') {
        return serde_json::from_str(trimmed);
    }
    trimmed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect()
}

/// Health per selected service; services without a container are `Missing`. With several
/// replicas the least healthy one wins.
pub fn service_health(
    services: &[String],
    statuses: &[ContainerStatus],
) -> BTreeMap<String, Health> {
    services
        .iter()
        .map(|service| {
            let health = statuses
                .iter()
                .filter(|status| &status.service == service)
                .map(ContainerStatus::health)
                .min_by_key(health_rank)
                .unwrap_or(Health::Missing);
            (service.clone(), health)
        })
        .collect()
}

/// Names of the running containers per selected service (one per replica), for
/// per-container stats. Services without a running container are left out.
pub fn service_containers(
    services: &[String],
    statuses: &[ContainerStatus],
) -> BTreeMap<String, Vec<String>> {
    services
        .iter()
        .filter_map(|service| {
            let mut names: Vec<String> = statuses
                .iter()
                .filter(|status| &status.service == service && status.state == "running")
                .map(|status| status.name.clone())
                .collect();
            names.sort();
            (!names.is_empty()).then(|| (service.clone(), names))
        })
        .collect()
}

/// Whether `service` already runs the way `up --wait` would leave it: as many
/// containers as it has replicas, each running and healthy (or without a
/// healthcheck), and, when `hashes` is given, each created from the config hash
/// Compose computes now. A paused, starting or unhealthy container, or one left over
/// from a bigger scale, makes it false.
pub(crate) fn service_current(
    model: &ComposeModel,
    service: &str,
    statuses: &[ContainerStatus],
    hashes: Option<&BTreeMap<String, String>>,
) -> bool {
    let replicas = model
        .services
        .get(service)
        .map_or(1, ServiceModel::replicas);
    let containers: Vec<&ContainerStatus> = statuses
        .iter()
        .filter(|status| status.service == service)
        .collect();
    let expected = hashes.map(|hashes| hashes.get(service));
    u64::try_from(containers.len()).is_ok_and(|count| count == replicas)
        && containers.iter().all(|status| {
            status.state == "running"
                && (status.health == "healthy" || status.health.is_empty())
                && expected.is_none_or(|hash| hash.is_some() && status.config_hash.as_ref() == hash)
        })
}

/// The `services` that `up` has to act on: those [`service_current`] rejects with
/// `hashes` from `docker compose config --hash`. An empty result means `up` would
/// change nothing.
pub(crate) fn stale_services(
    model: &ComposeModel,
    services: &[String],
    statuses: &[ContainerStatus],
    hashes: &BTreeMap<String, String>,
) -> Vec<String> {
    services
        .iter()
        .filter(|service| !service_current(model, service, statuses, Some(hashes)))
        .cloned()
        .collect()
}

/// Lower is worse; used to report the least healthy replica.
fn health_rank(health: &Health) -> u8 {
    match health {
        Health::Missing => 0,
        Health::Exited { .. } => 1,
        Health::Unhealthy => 2,
        Health::Other { .. } => 3,
        Health::Starting => 4,
        Health::Running => 5,
        Health::Healthy => 6,
    }
}

fn tcp() -> String {
    "tcp".to_owned()
}

/// Compose prints `published` as a string, but hand-written fixtures and older releases
/// use numbers.
fn string_or_number<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::String(text)) if !text.is_empty() => Some(text),
        Some(serde_json::Value::Number(number)) => Some(number.to_string()),
        _ => None,
    })
}

fn null_as_empty<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = include_str!("../tests/fixtures/config.json");
    const PS: &str = include_str!("../tests/fixtures/ps.ndjson");

    fn model() -> ComposeModel {
        ComposeModel::from_json(CONFIG).expect("fixture parses")
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn parses_resolved_model() {
        let model = model();
        assert_eq!(model.name, "acme");
        let postgres = &model.services["postgres"];
        assert_eq!(postgres.container_name.as_deref(), Some("acme-postgres"));
        assert_eq!(postgres.ports[0].target, 5432);
        assert_eq!(postgres.ports[0].published.as_deref(), Some("15432"));
        assert!(postgres.healthcheck.is_some());
        let redis = &model.services["redis"];
        assert_eq!(redis.ports[0].published, None);
        assert_eq!(redis.ports[0].host_ip.as_deref(), Some("127.0.0.1"));
        assert!(
            !model.services.contains_key("search"),
            "profile-disabled services are absent"
        );
    }

    #[test]
    fn shared_services_skip_exclude_and_per_lane() {
        let model = model();
        let selected = model
            .shared_services(&set(&["caddy"]), &set(&["mailpit"]))
            .expect("valid selection");
        assert_eq!(selected, ["postgres", "redis"]);
    }

    #[test]
    fn unknown_names_are_errors() {
        let error = model().shared_services(&set(&["cady"]), &set(&[])).err();
        assert!(matches!(error, Some(Error::UnknownService(name)) if name == "cady"));
    }

    #[test]
    fn dependency_on_skipped_service_is_an_error() {
        let error = model().shared_services(&set(&["redis"]), &set(&[])).err();
        assert!(
            matches!(&error, Some(Error::ExcludedDependency { dependency, .. }) if dependency == "redis"),
            "{error:?}"
        );
    }

    #[test]
    fn detects_volumes_shared_across_projects() {
        let json = r#"{"name":"acme-lane1","services":{"db":{"volumes":[
            {"type":"volume","source":"own","target":"/a"},
            {"type":"volume","source":"fixed","target":"/b"},
            {"type":"volume","source":"ext","target":"/c"},
            {"type":"bind","source":"/tmp","target":"/d"}]}},
            "volumes":{"own":{"name":"acme-lane1_own"},"fixed":{"name":"pgdata"},
            "ext":{"name":"ext","external":true}}}"#;
        let model = ComposeModel::from_json(json).expect("valid model");
        assert_eq!(model.shared_volumes(&["db".to_owned()]), ["ext", "fixed"]);
    }

    #[test]
    fn parses_ps_lines_and_arrays() {
        let statuses = parse_ps(PS).expect("fixture parses");
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].service, "postgres");
        assert_eq!(statuses[0].health(), Health::Healthy);
        assert_eq!(statuses[1].health(), Health::Running);
        let array = format!("[{}]", PS.trim().lines().collect::<Vec<_>>().join(","));
        assert_eq!(parse_ps(&array).unwrap_or_default(), statuses);
        assert_eq!(parse_ps("").unwrap_or_default(), []);
    }

    #[test]
    fn health_per_service() {
        let statuses = parse_ps(PS).unwrap_or_default();
        let services = [
            "postgres".to_owned(),
            "redis".to_owned(),
            "mailpit".to_owned(),
        ];
        let health = service_health(&services, &statuses);
        assert_eq!(health["postgres"], Health::Healthy);
        assert_eq!(health["redis"], Health::Running);
        assert_eq!(health["mailpit"], Health::Missing);
    }

    #[test]
    fn running_containers_per_service() {
        let ps = r#"{"ID":"a","Name":"acme-web-2","Service":"web","State":"running"}
{"ID":"b","Name":"acme-web-1","Service":"web","State":"running"}
{"ID":"c","Name":"acme-db-1","Service":"db","State":"exited"}
{"ID":"d","Name":"acme-other-1","Service":"other","State":"running"}"#;
        let statuses = parse_ps(ps).unwrap_or_default();
        let services = ["web".to_owned(), "db".to_owned()];
        let containers = service_containers(&services, &statuses);
        assert_eq!(
            containers,
            BTreeMap::from([("web".into(), vec!["acme-web-1".into(), "acme-web-2".into()])])
        );
    }

    #[test]
    fn config_hash_from_labels() {
        let line = r#"{"ID":"a","Name":"n","Service":"s","State":"running","Labels":"com.docker.compose.project.working_dir=C:\\a,b,com.docker.compose.config-hash=10a2,com.docker.compose.service=s"}"#;
        let statuses = parse_ps(line).unwrap_or_default();
        assert_eq!(statuses[0].config_hash.as_deref(), Some("10a2"));
        let line = r#"{"ID":"a","Name":"n","Service":"s","State":"running","Labels":{"com.docker.compose.config-hash":"10a2"}}"#;
        let statuses = parse_ps(line).unwrap_or_default();
        assert_eq!(statuses[0].config_hash.as_deref(), Some("10a2"));
        let line = r#"{"ID":"a","Name":"n","Service":"s","State":"running","Labels":""}"#;
        let statuses = parse_ps(line).unwrap_or_default();
        assert_eq!(statuses[0].config_hash, None);
    }

    /// One `ps` row of service `service` labelled with config hash `hash`.
    fn row(service: &str, state: &str, health: &str, hash: &str) -> ContainerStatus {
        ContainerStatus {
            id: format!("id-{service}"),
            name: format!("acme-{service}-1"),
            service: service.into(),
            project: "acme".into(),
            state: state.into(),
            health: health.into(),
            exit_code: 0,
            publishers: Vec::new(),
            config_hash: Some(hash.into()),
        }
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn hashes() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("postgres".to_owned(), "h-pg".to_owned()),
            ("redis".to_owned(), "h-redis".to_owned()),
        ])
    }

    #[test]
    fn up_skips_services_running_with_the_current_hash() {
        let model = model();
        let services = names(&["postgres", "redis"]);
        let statuses = [
            row("postgres", "running", "healthy", "h-pg"),
            row("redis", "running", "", "h-redis"),
        ];
        assert!(stale_services(&model, &services, &statuses, &hashes()).is_empty());
    }

    #[test]
    fn up_selects_only_the_missing_service() {
        let statuses = [row("postgres", "running", "healthy", "h-pg")];
        assert_eq!(
            stale_services(
                &model(),
                &names(&["postgres", "redis"]),
                &statuses,
                &hashes()
            ),
            ["redis"]
        );
    }

    #[test]
    fn up_selects_unhealthy_starting_paused_and_changed_services() {
        let model = model();
        let services = names(&["postgres", "redis"]);
        for (state, health, hash) in [
            ("running", "unhealthy", "h-pg"),
            ("running", "starting", "h-pg"),
            ("paused", "healthy", "h-pg"),
            ("exited", "", "h-pg"),
            ("running", "healthy", "old"),
        ] {
            let statuses = [
                row("postgres", state, health, hash),
                row("redis", "running", "", "h-redis"),
            ];
            assert_eq!(
                stale_services(&model, &services, &statuses, &hashes()),
                ["postgres"],
                "{state} {health} {hash}"
            );
        }
        // A service Compose printed no hash for is never taken as current.
        let statuses = [row("postgres", "running", "healthy", "h-pg")];
        assert_eq!(
            stale_services(&model, &names(&["postgres"]), &statuses, &BTreeMap::new()),
            ["postgres"]
        );
    }

    #[test]
    fn up_selects_services_short_of_their_replicas() {
        let json =
            r#"{"name":"acme","services":{"web":{"deploy":{"replicas":2}},"old":{"scale":2}}}"#;
        let model = ComposeModel::from_json(json).expect("valid model");
        assert_eq!(model.services["web"].replicas(), 2);
        assert_eq!(model.services["old"].replicas(), 2);
        let hashes = BTreeMap::from([("web".to_owned(), "h".to_owned())]);
        let one = [row("web", "running", "", "h")];
        assert_eq!(
            stale_services(&model, &names(&["web"]), &one, &hashes),
            ["web"]
        );
        let two = [
            row("web", "running", "", "h"),
            row("web", "running", "", "h"),
        ];
        assert!(stale_services(&model, &names(&["web"]), &two, &hashes).is_empty());
        // Without hashes only the running state counts, which decides whether
        // `config --hash` is worth running.
        assert!(service_current(&model, "web", &two, None));
    }

    #[test]
    fn exited_and_transitional_states() {
        let line = r#"{"ID":"x","Name":"n","Service":"s","State":"exited","ExitCode":3,"Publishers":null}"#;
        let statuses = parse_ps(line).unwrap_or_default();
        assert_eq!(statuses[0].health(), Health::Exited { code: 3 });
        let line = r#"{"ID":"x","Name":"n","Service":"s","State":"created"}"#;
        let statuses = parse_ps(line).unwrap_or_default();
        assert_eq!(
            statuses[0].health(),
            Health::Other {
                state: "created".into()
            }
        );
    }
}
