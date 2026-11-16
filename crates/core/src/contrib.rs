//! Launch contributors and `run_script` runners.
//!
//! Lane resolution calls these while it plans a process launch. They return data
//! only; the daemon spawns processes and writes files. Integrations implement the
//! traits and the binary registers them.

use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;

use crate::Error;

/// `ca_env` key holding the path of the devlanes root certificate (PEM).
/// Runtimes that append roots, such as Node's `NODE_EXTRA_CA_CERTS`, point here.
pub const CA_CERT_ENV: &str = "DEVLANES_CA_CERT";

/// `ca_env` key holding the path of a bundle of the system roots plus the devlanes
/// root. Runtimes that replace their roots, such as Python's `SSL_CERT_FILE`, point here.
pub const CA_BUNDLE_ENV: &str = "DEVLANES_CA_BUNDLE";

/// `ca_env` key holding the path of a PKCS#12 trust store with the same roots as
/// [`CA_BUNDLE_ENV`], for JVMs, which cannot read PEM bundles. Its password is
/// [`CA_TRUSTSTORE_PASSWORD`]. Absent when the store could not be written.
pub const CA_TRUSTSTORE_ENV: &str = "DEVLANES_CA_TRUSTSTORE";

/// Password of the [`CA_TRUSTSTORE_ENV`] store. Public on purpose: the store holds
/// only public root certificates, and Java needs some password to check it.
pub const CA_TRUSTSTORE_PASSWORD: &str = "changeit";

/// Variables runtimes read for their roots, with the devlanes CA variable each
/// points at: the certificate where a runtime adds roots, the bundle where it
/// replaces them.
pub const RUNTIME_CA_ENV: [(&str, &str); 4] = [
    ("NODE_EXTRA_CA_CERTS", CA_CERT_ENV),
    ("DENO_CERT", CA_CERT_ENV),
    ("SSL_CERT_FILE", CA_BUNDLE_ENV),
    ("REQUESTS_CA_BUNDLE", CA_BUNDLE_ENV),
];

/// Options every JVM reads at startup, build tool JVMs included. Quotes inside
/// the value are honoured, so a quoted path may contain spaces.
pub const JAVA_TOOL_OPTIONS: &str = "JAVA_TOOL_OPTIONS";

/// System property whose presence in [`JAVA_TOOL_OPTIONS`] means the trust
/// store is already chosen.
const TRUST_STORE_PROPERTY: &str = "javax.net.ssl.trustStore=";

/// Whether a `JAVA_TOOL_OPTIONS` value already names a trust store.
pub fn java_options_set_trust_store(options: &str) -> bool {
    options.contains(TRUST_STORE_PROPERTY)
}

/// `base` (the `JAVA_TOOL_OPTIONS` value already set, if any) followed by the
/// flags that make a JVM trust the PKCS#12 store at `truststore`. `None` when
/// `base` already names a trust store: the user chose it, and it may hold roots
/// imported into the JDK's own `cacerts`.
pub fn java_trust_options(base: Option<&str>, truststore: &str) -> Option<String> {
    let base = base.map(str::trim).unwrap_or_default();
    if java_options_set_trust_store(base) {
        return None;
    }
    // HotSpot splits JAVA_TOOL_OPTIONS on whitespace and strips quotes inside a
    // token, without backslash escapes, so Windows paths stay intact.
    let path = if !truststore.contains(char::is_whitespace) {
        truststore.to_owned()
    } else if truststore.contains('"') {
        format!("'{truststore}'")
    } else {
        format!("\"{truststore}\"")
    };
    let flags = format!(
        "-D{TRUST_STORE_PROPERTY}{path} -Djavax.net.ssl.trustStoreType=PKCS12 \
         -Djavax.net.ssl.trustStorePassword={CA_TRUSTSTORE_PASSWORD}"
    );
    Some(if base.is_empty() {
        flags
    } else {
        format!("{base} {flags}")
    })
}

/// Points the [`RUNTIME_CA_ENV`] variables at the devlanes CA paths in `ca`, for
/// commands no platform contributor plans (`devlanes exec`, `devlanes with`).
/// `shell` returns the value a variable already has. Those variables are
/// skipped, since a value the shell set may name a CA the command needs as
/// well, except [`JAVA_TOOL_OPTIONS`], which keeps the shell's options and gains
/// the trust store flags unless it already names a trust store.
pub fn runtime_ca_env<'v>(
    ca: &BTreeMap<String, String>,
    shell: impl Fn(&str) -> Option<&'v str>,
) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = RUNTIME_CA_ENV
        .iter()
        .filter(|(name, _)| shell(name).is_none())
        .filter_map(|(name, source)| Some(((*name).to_owned(), ca.get(*source)?.clone())))
        .collect();
    if let Some(options) = ca
        .get(CA_TRUSTSTORE_ENV)
        .and_then(|store| java_trust_options(shell(JAVA_TOOL_OPTIONS), store))
    {
        env.insert(JAVA_TOOL_OPTIONS.to_owned(), options);
    }
    env
}

/// Static service facts available before any expression is evaluated.
#[derive(Debug, Clone, Copy)]
pub struct ServiceView<'a> {
    pub name: &'a str,
    /// Explicit `platform`; inference happens later, at launch planning.
    pub platform: Option<&'a str>,
    pub preset: Option<&'a str>,
    pub tags: &'a [String],
}

/// Kind of process being planned. Contributors that only make sense for
/// long-running services check it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    Service,
    Task,
    /// `infra.setup` or `infra.teardown` entry.
    InfraCommand,
}

/// Command before contributor arguments are appended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftCommand {
    /// Shell text, run with `/bin/sh -c`.
    Shell(String),
    /// Executable and arguments, run without a shell.
    Argv(Vec<String>),
    /// `run_script` reference; a [`ScriptRunner`] renders it after contributors run.
    Script { source: PathBuf, name: String },
}

/// A file the daemon writes before spawning, such as a framework config overlay.
/// Contents must not contain secrets: the daemon writes them to disk. The path must lie
/// in the lane worktree or the lane data folder; prefer the data folder, since lanes
/// that share a worktree would overwrite each other's files there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedFile {
    pub path: PathBuf,
    pub contents: Vec<u8>,
}

/// Mutable launch plan shared by contributors, applied in priority order.
/// Neither `Debug` nor serializable: `env` can hold values from env managers.
pub struct LaunchDraft {
    pub command: DraftCommand,
    /// Appended to the command. Script runners place them (for example after `--`).
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Contributor layer of the environment (above the client environment).
    pub env: BTreeMap<String, String>,
    /// Variables to drop from the client environment, such as direnv unsets.
    pub remove: BTreeSet<String>,
    pub files: Vec<PreparedFile>,
    /// Non-fatal diagnostics shown with the unit, such as an untrusted mise config.
    pub notes: Vec<String>,
}

/// `run_script` facts resolved before contributors run, so presets can inspect
/// the script body (for example to reject a hardcoded `--port`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptInfo {
    /// Absolute path of the source file.
    pub source: PathBuf,
    /// Script name after profile remapping.
    pub name: String,
    pub body: Option<String>,
    /// Directory the runner executes in.
    pub cwd: PathBuf,
    /// [`ScriptRunner::id`] of the runner that handles the source.
    pub runner: &'static str,
}

/// Read-only facts for one unit (one replica of a service, a task or an infra command).
pub struct ContribCx<'a> {
    pub kind: UnitKind,
    pub project: &'a str,
    /// Service, task or infra entry name as written in the config.
    pub name: &'a str,
    /// One-based replica index for services.
    pub replica: Option<u16>,
    pub lane_index: u32,
    pub lane_name: &'a str,
    pub lane_path: &'a Path,
    /// The lane's data folder outside the worktree (`lane.data_path`), unique per lane even
    /// when several lanes share one folder. Generated files and per-lane state go here.
    pub lane_data_path: &'a Path,
    pub primary_path: &'a Path,
    /// Explicit or inferred platform (`node`, `python`, ...).
    pub platform: Option<&'a str>,
    pub preset: Option<&'a str>,
    /// Preset options after expression evaluation, as JSON.
    pub preset_options: Option<&'a serde_json::Value>,
    pub port: Option<u16>,
    /// Named ports (`self.ports`), without the primary port.
    pub ports: &'a BTreeMap<String, u16>,
    pub public_url: Option<&'a str>,
    pub pty: bool,
    pub script: Option<&'a ScriptInfo>,
    /// Environment of the client that started the lane.
    pub client_env: &'a BTreeMap<String, String>,
    /// Trust variables from the CA, keyed by [`CA_CERT_ENV`] and [`CA_BUNDLE_ENV`].
    pub ca_env: &'a BTreeMap<String, String>,
    /// Per-resolution cache shared by all units of one lane.
    pub memo: &'a ContribMemo,
}

/// Confidence for a claim based on the `run_script` source file, such as a
/// `package.json` script for node. See [`Contributor::detect`].
pub const DETECT_SCRIPT_SOURCE: u8 = 90;

/// Confidence for a claim based on the platform's manifest in the unit's
/// working directory, such as `package.json` for node.
pub const DETECT_MANIFEST: u8 = 60;

/// Added to a base confidence by a platform that refines another platform's
/// manifest: a bun lockfile next to a `package.json` beats plain node.
pub const DETECT_REFINE: u8 = 5;

/// Files a platform contributor may inspect when a unit omits `platform`.
#[derive(Debug, Clone, Copy)]
pub struct DetectCx<'a> {
    pub kind: UnitKind,
    /// Service, task or infra entry name as written in the config.
    pub name: &'a str,
    /// Lane worktree root; detection must not look above it.
    pub lane_path: &'a Path,
    /// Directory the unit runs in (the script's directory for `run_script`).
    pub cwd: &'a Path,
    /// The `run_script` source, when the unit uses one.
    pub script: Option<&'a ScriptInfo>,
}

/// Cache for work that is per lane rather than per unit, such as `mise env`.
/// It lives for one lane resolution, so config edits apply at the next start.
#[derive(Default)]
pub struct ContribMemo {
    entries: Mutex<HashMap<String, Arc<dyn Any + Send + Sync>>>,
}

impl ContribMemo {
    /// Returns a cached value when `key` holds a value of type `T`.
    pub fn get<T: Any + Send + Sync>(&self, key: &str) -> Option<Arc<T>> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.get(key).cloned()?.downcast::<T>().ok()
    }

    /// Stores `value` under `key`, replacing an earlier entry.
    pub fn insert<T: Any + Send + Sync>(&self, key: impl Into<String>, value: Arc<T>) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.insert(key.into(), value);
    }
}

/// Adds environment, arguments or files to a launch. Platform integrations,
/// env managers and framework presets implement it.
#[async_trait]
pub trait Contributor: Send + Sync {
    /// Stable identifier used in errors and `env --explain`.
    fn id(&self) -> &'static str;

    /// Lower priorities run first; later contributors override earlier values.
    /// Env managers use 10-20, platforms 50, presets 100.
    fn priority(&self) -> i32 {
        0
    }

    /// Named ports to allocate for `service` before expressions are evaluated,
    /// so they exist in `self.ports` (wrangler's `inspector`, for example).
    fn required_ports(&self, _service: &ServiceView<'_>) -> Vec<String> {
        Vec::new()
    }

    /// Platform name this contributor provides (`node`, `python`, ...). Only
    /// contributors that return one are asked to [`Self::detect`].
    fn platform(&self) -> Option<&'static str> {
        None
    }

    /// Claims a unit whose config omits `platform`, with a confidence. Resolution
    /// picks the highest claim; ties go to the higher [`Self::priority`], then the
    /// smaller [`Self::id`]. Use [`DETECT_SCRIPT_SOURCE`], [`DETECT_MANIFEST`] and
    /// [`DETECT_REFINE`] so claims from different crates compare sensibly.
    async fn detect(&self, _cx: &DetectCx<'_>) -> Option<u8> {
        None
    }

    /// Edits the draft. Errors fail the unit's launch; use `draft.notes` for
    /// problems that should not.
    async fn apply(&self, draft: &mut LaunchDraft, cx: &ContribCx<'_>) -> Result<(), Error>;
}

/// One `run_script` invocation to render.
#[derive(Debug, Clone, Copy)]
pub struct ScriptRequest<'a> {
    /// Lane worktree root. Runners must not read above it.
    pub worktree: &'a Path,
    /// Source file, relative to `worktree` or absolute inside it.
    pub source: &'a Path,
    /// Script, target or recipe name.
    pub name: &'a str,
    /// Arguments to forward to the script, typically preset flags. Runners add
    /// whatever separator their tool needs.
    pub extra_args: &'a [String],
}

/// Rendered script command.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ScriptPlan {
    /// Program first.
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    /// Script text when the source defines one, for presets that inspect it.
    pub body: Option<String>,
    /// Non-fatal findings, such as a package manager picked from a lockfile
    /// because `packageManager` is missing. Lane resolution copies them into
    /// the launch notes shown by `env --explain`.
    #[serde(default)]
    pub notes: Vec<String>,
    /// One line on how the runner was chosen, such as
    /// `pnpm 10.4.1 (packageManager in package.json)`. Shown with the notes.
    #[serde(default)]
    pub detail: Option<String>,
    /// Variables the script runs with, as its tool would set them (npm's
    /// `npm_package_*`). Layered under the unit's own `env`.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Directories put in front of the unit's `PATH`, first wins (npm's
    /// `node_modules/.bin` directories).
    #[serde(default)]
    pub path: Vec<PathBuf>,
}

/// Renders `run_script` references for one kind of source file
/// (package.json, Makefile, justfile).
#[async_trait]
pub trait ScriptRunner: Send + Sync {
    /// Stable identifier, such as `npm` or `make`.
    fn id(&self) -> &'static str;

    /// Returns whether this runner handles `source` (usually by file name).
    fn matches(&self, source: &Path) -> bool;

    /// Renders the command. Fails when the file or script is missing. Called once
    /// without `extra_args` before contributors run and again with them when
    /// contributors added arguments.
    async fn resolve(&self, request: &ScriptRequest<'_>) -> Result<ScriptPlan, Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// The memo returns values only for the type they were stored with.
    fn memo_is_typed() {
        let memo = ContribMemo::default();
        memo.insert("k", Arc::new(5_u32));
        assert_eq!(memo.get::<u32>("k").as_deref(), Some(&5));
        assert!(memo.get::<String>("k").is_none());
        assert!(memo.get::<u32>("missing").is_none());
    }

    #[test]
    /// The trust flags extend the existing options, quote a path with spaces and
    /// stay out when the options already name a trust store.
    fn java_trust_options_compose() {
        let flags =
            "-Djavax.net.ssl.trustStoreType=PKCS12 -Djavax.net.ssl.trustStorePassword=changeit";
        assert_eq!(
            java_trust_options(None, "/ca/truststore.p12").as_deref(),
            Some(format!("-Djavax.net.ssl.trustStore=/ca/truststore.p12 {flags}").as_str())
        );
        assert_eq!(
            java_trust_options(Some("-Xmx1g "), r"C:\Users\Jane Doe\ca\truststore.p12").as_deref(),
            Some(
                format!(
                    r#"-Xmx1g -Djavax.net.ssl.trustStore="C:\Users\Jane Doe\ca\truststore.p12" {flags}"#
                )
                .as_str()
            )
        );
        assert_eq!(
            java_trust_options(Some("-Djavax.net.ssl.trustStore=/mine.jks"), "/ca/t.p12"),
            None
        );
    }

    #[test]
    /// Shell values keep the CA variables out, but `JAVA_TOOL_OPTIONS` composes
    /// onto the shell's value.
    fn runtime_ca_env_composes_java_options() {
        let ca = BTreeMap::from([
            (CA_CERT_ENV.to_owned(), "/ca/ca.pem".to_owned()),
            (CA_BUNDLE_ENV.to_owned(), "/ca/bundle.pem".to_owned()),
            (
                CA_TRUSTSTORE_ENV.to_owned(),
                "/ca/truststore.p12".to_owned(),
            ),
        ]);
        let shell = BTreeMap::from([
            ("NODE_EXTRA_CA_CERTS", "/mine.pem"),
            (JAVA_TOOL_OPTIONS, "-Xmx1g"),
        ]);
        let env = runtime_ca_env(&ca, |name| shell.get(name).copied());
        assert!(!env.contains_key("NODE_EXTRA_CA_CERTS"));
        assert_eq!(env["SSL_CERT_FILE"], "/ca/bundle.pem");
        assert!(
            env[JAVA_TOOL_OPTIONS]
                .starts_with("-Xmx1g -Djavax.net.ssl.trustStore=/ca/truststore.p12 ")
        );

        let without_store = BTreeMap::from([(CA_CERT_ENV.to_owned(), "/ca/ca.pem".to_owned())]);
        assert!(!runtime_ca_env(&without_store, |_| None).contains_key(JAVA_TOOL_OPTIONS));
    }
}
