//! `preset: spring-boot`: binds a Spring Boot app to the service's port.
//!
//! Spring Boot reads its settings from environment variables through relaxed
//! binding (`server.port` from `SERVER_PORT`), so the preset only sets
//! variables and works the same under Maven, Gradle or a plain jar.

use async_trait::async_trait;
use devlanes_core::{
    Error,
    contrib::{ContribCx, Contributor, LaunchDraft},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Preset name, also the key that holds [`SpringBootOptions`] in a service.
pub const NAME: &str = "spring-boot";

/// Address the app binds.
pub const HOST: &str = "127.0.0.1";

/// Named port that moves actuator to its own port when the service references it.
pub const MANAGEMENT_PORT: &str = "management";

/// Options for `preset: spring-boot`, which sets `SERVER_PORT` to the service's port, binds
/// the app to 127.0.0.1, trusts the devlanes proxy's forwarded headers and turns off
/// LiveReload, whose fixed port 35729 collides between lanes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("examples" = [{"docker_compose":true}]))]
pub struct SpringBootOptions {
    /// Lets Spring Boot's own Docker Compose support (`spring-boot-docker-compose`) start the
    /// project's compose file. Default: `false`, which sets `SPRING_DOCKER_COMPOSE_ENABLED=false`:
    /// devlanes already runs the project's containers per lane or shared, and Spring's support
    /// would start a second copy for every worktree. Set `true` only when devlanes does not
    /// manage those containers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("examples" = [true]))]
    pub docker_compose: Option<bool>,
}

impl SpringBootOptions {
    /// Applies a local override or `extends` overlay; set fields replace.
    pub fn merge(&mut self, overlay: Self) {
        if overlay.docker_compose.is_some() {
            self.docker_compose = overlay.docker_compose;
        }
    }
}

/// Returns the named ports to allocate before expressions are evaluated. The
/// management port is allocated only when the service references it.
pub fn required_ports() -> &'static [&'static str] {
    &[]
}

/// Renders the variables for an app on `port`; `ports` are the service's named ports.
pub fn contribution(
    port: u16,
    ports: &BTreeMap<String, u16>,
    options: &SpringBootOptions,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::from([
        ("SERVER_PORT".to_owned(), port.to_string()),
        ("SERVER_ADDRESS".to_owned(), HOST.to_owned()),
        // The proxy replaces Forwarded and X-Forwarded-* on every request, so the app
        // can trust them and build https:// links with the lane's hostname.
        (
            "SERVER_FORWARD_HEADERS_STRATEGY".to_owned(),
            "framework".to_owned(),
        ),
        (
            "SPRING_DEVTOOLS_LIVERELOAD_ENABLED".to_owned(),
            "false".to_owned(),
        ),
    ]);
    if let Some(management) = ports.get(MANAGEMENT_PORT) {
        env.insert("MANAGEMENT_SERVER_PORT".to_owned(), management.to_string());
        // Without it actuator binds every interface on its own port.
        env.insert("MANAGEMENT_SERVER_ADDRESS".to_owned(), HOST.to_owned());
    }
    if options.docker_compose != Some(true) {
        env.insert(
            "SPRING_DOCKER_COMPOSE_ENABLED".to_owned(),
            "false".to_owned(),
        );
    }
    env
}

/// JSON Schema of [`SpringBootOptions`], for the config's `Extensions` registration.
pub fn options_schema() -> schemars::Schema {
    schemars::schema_for!(SpringBootOptions)
}

/// Parses evaluated options; a service without a `spring-boot:` key gets defaults.
pub fn parse_options(value: Option<&serde_json::Value>) -> Result<SpringBootOptions, Error> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(SpringBootOptions::default()),
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|e| Error::InvalidInput(format!("{NAME} options: {e}"))),
    }
}

/// Core contributor for services with `preset: spring-boot`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SpringBootPreset;

#[async_trait]
impl Contributor for SpringBootPreset {
    fn id(&self) -> &'static str {
        "preset-spring-boot"
    }

    fn priority(&self) -> i32 {
        100
    }

    /// Sets the Spring Boot variables; other services are left untouched.
    async fn apply(&self, draft: &mut LaunchDraft, cx: &ContribCx<'_>) -> Result<(), Error> {
        if cx.preset != Some(NAME) {
            return Ok(());
        }
        let options = parse_options(cx.preset_options)?;
        let port = cx
            .port
            .ok_or_else(|| Error::InvalidInput(format!("preset {NAME} needs a service port")))?;
        draft.env.extend(contribution(port, cx.ports, &options));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devlanes_core::contrib::{ContribMemo, DraftCommand, UnitKind};
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    /// Runs the contributor on a Maven script draft with a fake context.
    async fn apply(
        preset: Option<&str>,
        options: Option<serde_json::Value>,
        port: Option<u16>,
        ports: BTreeMap<String, u16>,
    ) -> Result<LaunchDraft, Error> {
        let empty = BTreeMap::new();
        let memo = ContribMemo::default();
        let cx = ContribCx {
            kind: UnitKind::Service,
            project: "acme",
            name: "api",
            replica: Some(1),
            lane_index: 0,
            lane_name: "main",
            lane_path: Path::new("/lanes/acme"),
            lane_data_path: Path::new("/data/acme/1"),
            primary_path: Path::new("/lanes/acme"),
            platform: Some("java"),
            preset,
            preset_options: options.as_ref(),
            port,
            ports: &ports,
            public_url: None,
            pty: true,
            script: None,
            client_env: &empty,
            ca_env: &empty,
            memo: &memo,
        };
        let mut draft = LaunchDraft {
            command: DraftCommand::Script {
                source: "pom.xml".into(),
                name: "spring-boot:run".into(),
            },
            args: Vec::new(),
            cwd: PathBuf::from("/lanes/acme"),
            env: BTreeMap::new(),
            remove: BTreeSet::new(),
            files: Vec::new(),
            notes: Vec::new(),
        };
        SpringBootPreset.apply(&mut draft, &cx).await?;
        Ok(draft)
    }

    /// Spring Boot services get variables only, never runner arguments.
    #[tokio::test]
    async fn sets_env_for_spring_boot_services_only() {
        let other = apply(None, None, Some(20013), BTreeMap::new())
            .await
            .expect("apply");
        assert!(other.env.is_empty());
        let draft = apply(Some(NAME), None, Some(20013), BTreeMap::new())
            .await
            .expect("apply");
        assert!(draft.args.is_empty());
        let env: Vec<_> = draft.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        assert_eq!(
            env,
            [
                "SERVER_ADDRESS=127.0.0.1",
                "SERVER_FORWARD_HEADERS_STRATEGY=framework",
                "SERVER_PORT=20013",
                "SPRING_DEVTOOLS_LIVERELOAD_ENABLED=false",
                "SPRING_DOCKER_COMPOSE_ENABLED=false",
            ]
        );
    }

    /// A referenced `management` port moves actuator there, on loopback.
    #[tokio::test]
    async fn management_port_only_when_referenced() {
        let ports = BTreeMap::from([("management".to_owned(), 20014)]);
        let draft = apply(Some(NAME), None, Some(20013), ports)
            .await
            .expect("apply");
        assert_eq!(draft.env["MANAGEMENT_SERVER_PORT"], "20014");
        assert_eq!(draft.env["MANAGEMENT_SERVER_ADDRESS"], "127.0.0.1");
        let ports = BTreeMap::from([("debug".to_owned(), 20015)]);
        let draft = apply(Some(NAME), None, Some(20013), ports)
            .await
            .expect("apply");
        assert!(!draft.env.contains_key("MANAGEMENT_SERVER_PORT"));
    }

    /// `docker_compose: true` leaves Spring's own Compose support alone.
    #[tokio::test]
    async fn docker_compose_option_unsets_the_switch() {
        let draft = apply(
            Some(NAME),
            Some(serde_json::json!({"docker_compose": true})),
            Some(20013),
            BTreeMap::new(),
        )
        .await
        .expect("apply");
        assert!(!draft.env.contains_key("SPRING_DOCKER_COMPOSE_ENABLED"));
    }

    /// Without a service port the launch fails instead of guessing 8080.
    #[tokio::test]
    async fn needs_a_service_port() {
        let Err(error) = apply(Some(NAME), None, None, BTreeMap::new()).await else {
            panic!("a service without a port must fail");
        };
        assert!(
            error.to_string().contains("needs a service port"),
            "{error}"
        );
    }

    /// Overlays replace only the fields they set.
    #[test]
    fn merge_keeps_unset_fields() {
        let mut options = SpringBootOptions {
            docker_compose: Some(true),
        };
        options.merge(SpringBootOptions::default());
        assert_eq!(options.docker_compose, Some(true));
        options.merge(SpringBootOptions {
            docker_compose: Some(false),
        });
        assert_eq!(options.docker_compose, Some(false));
    }

    /// The options reject unknown keys so typos surface in `config check`.
    #[test]
    fn options_reject_unknown_keys() {
        let error = parse_options(Some(&serde_json::json!({"port": 8080}))).expect_err("unknown");
        assert!(error.to_string().contains("spring-boot options"), "{error}");
        assert!(options_schema().to_value()["properties"]["docker_compose"].is_object());
    }
}
