//! `preset: quarkus`: binds Quarkus dev mode to the service's ports.
//!
//! The HTTP settings are environment variables Quarkus reads at startup. The
//! dev-mode debugger port is the `debug` system property, which both the Maven
//! and the Gradle plugin read, so it goes to the build tool as `-Ddebug=<port>`.

use async_trait::async_trait;
use devlanes_core::{
    Error,
    contrib::{ContribCx, Contributor, LaunchDraft, ServiceView},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Preset name, also the key that holds [`QuarkusOptions`] in a service.
pub const NAME: &str = "quarkus";

/// Address the app binds.
pub const HOST: &str = "127.0.0.1";

/// Named port for the dev-mode debugger (`self.ports.debug`).
pub const DEBUG_PORT: &str = "debug";

/// Script runners that can pass `-Ddebug` to the Quarkus plugin.
const RUNNERS: [&str; 2] = ["maven", "gradle"];

/// Options for `preset: quarkus`, which sets the HTTP port and host, lets continuous testing
/// pick a free port and moves the dev-mode debugger from 5005 to `self.ports.debug`, so two
/// lanes can run dev mode side by side. The preset has no settings, so the key can be left out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("examples" = [{}]))]
pub struct QuarkusOptions {}

impl QuarkusOptions {
    /// Applies a local override or `extends` overlay; there are no fields to replace.
    pub fn merge(&mut self, _overlay: Self) {}
}

/// Arguments and environment a preset adds to a service's launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contribution {
    /// Build-tool arguments the runner appends after the user's.
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// Returns the named ports to allocate before expressions are evaluated.
pub fn required_ports() -> &'static [&'static str] {
    &[DEBUG_PORT]
}

/// Renders the variables and build-tool arguments for dev mode on `port`, with the
/// debugger on `debug`.
pub fn contribution(port: u16, debug: u16) -> Contribution {
    let env = BTreeMap::from([
        ("QUARKUS_HTTP_PORT".to_owned(), port.to_string()),
        ("QUARKUS_HTTP_HOST".to_owned(), HOST.to_owned()),
        // Continuous testing listens on a fixed 8081 otherwise; 0 lets Quarkus pick a free port.
        ("QUARKUS_HTTP_TEST_PORT".to_owned(), "0".to_owned()),
    ]);
    Contribution {
        args: vec![
            format!("-Ddebug={debug}"),
            // The first dev-mode run asks a yes/no question about build analytics and
            // waits for the answer, so a service started by devlanes would hang there.
            "-Dquarkus.analytics.disabled=true".to_owned(),
        ],
        env,
    }
}

/// JSON Schema of [`QuarkusOptions`], for the config's `Extensions` registration.
pub fn options_schema() -> schemars::Schema {
    schemars::schema_for!(QuarkusOptions)
}

/// Parses evaluated options; a service without a `quarkus:` key gets defaults.
pub fn parse_options(value: Option<&serde_json::Value>) -> Result<QuarkusOptions, Error> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(QuarkusOptions::default()),
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|e| Error::InvalidInput(format!("{NAME} options: {e}"))),
    }
}

/// Core contributor for services with `preset: quarkus`.
#[derive(Debug, Default, Clone, Copy)]
pub struct QuarkusPreset;

#[async_trait]
impl Contributor for QuarkusPreset {
    fn id(&self) -> &'static str {
        "preset-quarkus"
    }

    fn priority(&self) -> i32 {
        100
    }

    /// Asks for `debug` on quarkus services only.
    fn required_ports(&self, service: &ServiceView<'_>) -> Vec<String> {
        if service.preset == Some(NAME) {
            required_ports()
                .iter()
                .map(|name| (*name).to_owned())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Sets the HTTP variables and appends `-Ddebug=<port>` unless the script name
    /// has its own `-Ddebug`; other services are left untouched. Fails unless the
    /// service runs through the Maven or Gradle runner.
    async fn apply(&self, draft: &mut LaunchDraft, cx: &ContribCx<'_>) -> Result<(), Error> {
        if cx.preset != Some(NAME) {
            return Ok(());
        }
        parse_options(cx.preset_options)?;
        let runner = cx.script.map(|script| script.runner);
        if !runner.is_some_and(|runner| RUNNERS.contains(&runner)) {
            return Err(Error::InvalidInput(format!(
                "preset {NAME} needs run_script with a pom.xml or build.gradle(.kts), such as `run_script: {{ file: pom.xml, name: quarkus:dev }}`, so it can pass the debug port to the build tool"
            )));
        }
        let port = cx
            .port
            .ok_or_else(|| Error::InvalidInput(format!("preset {NAME} needs a service port")))?;
        let debug = cx.ports.get(DEBUG_PORT).copied().ok_or_else(|| {
            Error::InvalidInput(format!("preset {NAME} needs the `{DEBUG_PORT}` port"))
        })?;
        let mut contribution = contribution(port, debug);
        // A `-Ddebug` of the user's own (`-Ddebug=false`, another port) wins.
        if cx.script.is_some_and(|script| sets_debug(&script.name)) {
            contribution.args.retain(|arg| !arg.starts_with("-Ddebug="));
        }
        // Quarkus 3.38+ bound to loopback answers 400 to any other Host header, which
        // is every request through the proxy. 127.0.0.1 stays for the ready check.
        if let Some(host) = cx.public_url.and_then(url_host) {
            contribution.env.insert(
                ALLOWED_HOSTS_ENV.to_owned(),
                format!("{host},localhost,{HOST}"),
            );
        }
        draft.args.extend(contribution.args);
        draft.env.extend(contribution.env);
        Ok(())
    }
}

/// Variable listing the Host headers Quarkus accepts on a loopback address.
const ALLOWED_HOSTS_ENV: &str = "QUARKUS_HTTP_HOST_VALIDATION_ALLOWED_HOSTS";

/// The host name of a URL such as `https://api.acme.localhost:8443/`, without the port.
fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _)| host);
    (!host.is_empty()).then_some(host)
}

/// Whether the `run_script` name already passes the `debug` property.
fn sets_debug(name: &str) -> bool {
    let words = shell_words::split(name)
        .unwrap_or_else(|_| name.split_whitespace().map(str::to_owned).collect());
    words
        .iter()
        .any(|word| word == "-Ddebug" || word.starts_with("-Ddebug="))
}

#[cfg(test)]
mod tests {
    use super::*;
    use devlanes_core::contrib::{ContribMemo, DraftCommand, ScriptInfo, UnitKind};
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    /// Runs the contributor with a fake context; `runner` is the `run_script` runner id.
    async fn apply(
        preset: Option<&str>,
        options: Option<serde_json::Value>,
        runner: Option<&'static str>,
    ) -> Result<LaunchDraft, Error> {
        apply_named(preset, options, runner, "quarkus:dev").await
    }

    /// [`apply`] with `name` as the `run_script` name.
    async fn apply_named(
        preset: Option<&str>,
        options: Option<serde_json::Value>,
        runner: Option<&'static str>,
        name: &str,
    ) -> Result<LaunchDraft, Error> {
        let ports = BTreeMap::from([(DEBUG_PORT.to_owned(), 20014)]);
        let empty = BTreeMap::new();
        let memo = ContribMemo::default();
        let script = runner.map(|runner| ScriptInfo {
            source: PathBuf::from("/lanes/acme/pom.xml"),
            name: name.to_owned(),
            body: None,
            cwd: PathBuf::from("/lanes/acme"),
            runner,
        });
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
            port: Some(20013),
            ports: &ports,
            public_url: Some("https://api.acme.localhost:8443/"),
            pty: true,
            script: script.as_ref(),
            client_env: &empty,
            ca_env: &empty,
            memo: &memo,
        };
        let mut draft = LaunchDraft {
            command: DraftCommand::Script {
                source: "pom.xml".into(),
                name: "quarkus:dev".into(),
            },
            args: Vec::new(),
            cwd: PathBuf::from("/lanes/acme"),
            env: BTreeMap::new(),
            remove: BTreeSet::new(),
            files: Vec::new(),
            notes: Vec::new(),
        };
        QuarkusPreset.apply(&mut draft, &cx).await?;
        Ok(draft)
    }

    /// Maven and Gradle services get the variables and the debug property.
    #[tokio::test]
    async fn applies_to_maven_and_gradle_services() {
        for runner in RUNNERS {
            let draft = apply(Some(NAME), None, Some(runner)).await.expect("apply");
            assert_eq!(
                draft.args,
                ["-Ddebug=20014", "-Dquarkus.analytics.disabled=true"]
            );
            let env: Vec<_> = draft.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
            assert_eq!(
                env,
                [
                    "QUARKUS_HTTP_HOST=127.0.0.1",
                    "QUARKUS_HTTP_HOST_VALIDATION_ALLOWED_HOSTS=api.acme.localhost,localhost,127.0.0.1",
                    "QUARKUS_HTTP_PORT=20013",
                    "QUARKUS_HTTP_TEST_PORT=0",
                ]
            );
        }
        let other = apply(None, None, Some("npm")).await.expect("apply");
        assert!(other.args.is_empty() && other.env.is_empty());
    }

    /// Hosts come from URLs with or without a port or path.
    #[test]
    fn url_hosts() {
        assert_eq!(url_host("https://a.localhost:8443/x"), Some("a.localhost"));
        assert_eq!(url_host("https://a.localhost"), Some("a.localhost"));
        assert_eq!(url_host("https://"), None);
    }

    /// A `-Ddebug` in the script name replaces the preset's; the analytics flag stays.
    #[tokio::test]
    async fn keeps_the_users_debug_property() {
        for name in [
            "quarkus:dev -Ddebug=false",
            "quarkus:dev -Ddebug",
            "quarkus:dev '-Ddebug=5006'",
        ] {
            let draft = apply_named(Some(NAME), None, Some("maven"), name)
                .await
                .expect("apply");
            assert_eq!(draft.args, ["-Dquarkus.analytics.disabled=true"], "{name}");
        }
        let draft = apply_named(Some(NAME), None, Some("maven"), "quarkus:dev -Ddebugger=1")
            .await
            .expect("apply");
        assert_eq!(draft.args[0], "-Ddebug=20014");
    }

    /// Other runners cannot place `-Ddebug`, so the launch fails with the fix.
    #[tokio::test]
    async fn refuses_other_runners() {
        for runner in [Some("make"), None] {
            let Err(error) = apply(Some(NAME), None, runner).await else {
                panic!("runner {runner:?} must be refused");
            };
            assert!(error.to_string().contains("name: quarkus:dev"), "{error}");
        }
    }

    /// Only quarkus services get the `debug` port.
    #[test]
    fn debug_port_only_for_quarkus_services() {
        let view = |preset| ServiceView {
            name: "api",
            platform: None,
            preset,
            tags: &[],
        };
        assert_eq!(QuarkusPreset.required_ports(&view(Some(NAME))), ["debug"]);
        assert!(
            QuarkusPreset
                .required_ports(&view(Some("spring-boot")))
                .is_empty()
        );
    }

    /// The options reject unknown keys so typos surface in `config check`.
    #[test]
    fn options_reject_unknown_keys() {
        assert!(parse_options(Some(&serde_json::json!({}))).is_ok());
        let error = parse_options(Some(&serde_json::json!({"debug": false}))).expect_err("unknown");
        assert!(error.to_string().contains("quarkus options"), "{error}");
    }
}
