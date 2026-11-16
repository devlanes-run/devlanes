//! Java launch contributor (`platform: java`, or detected from a Maven or
//! Gradle build file).
//!
//! JVMs read neither PEM bundles nor the OS store by default, so the trust store
//! flags go into `JAVA_TOOL_OPTIONS`, which every JVM reads, the Maven and
//! Gradle JVMs and JDK 8 included. Build tool flags that depend on the unit
//! (pty, service or task) are added here so the runners stay pure functions of
//! the build file and script name.

use async_trait::async_trait;
use devlanes_core::{
    Error,
    contrib::{
        CA_TRUSTSTORE_ENV, ContribCx, Contributor, DETECT_MANIFEST, DETECT_SCRIPT_SOURCE, DetectCx,
        JAVA_TOOL_OPTIONS, LaunchDraft, ScriptInfo, UnitKind, java_trust_options,
    },
};

/// Platform name matched against `platform:` (explicit or inferred).
pub const PLATFORM: &str = "java";

/// Files that make a directory a Maven or Gradle project.
pub const MANIFESTS: &[&str] = &[
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
];

/// Claim for a manifest in the working directory. Below node's, rust's, go's
/// and python's (`DETECT_MANIFEST - 10`), so a directory with a Java build
/// file next to `package.json`, `Cargo.toml`, `go.mod` or `pyproject.toml`
/// keeps that platform.
const CWD_CONFIDENCE: u8 = DETECT_MANIFEST - 12;

/// [`devlanes_core::contrib::ScriptRunner::id`] of the Maven runner.
const MAVEN: &str = "maven";

/// [`devlanes_core::contrib::ScriptRunner::id`] of the Gradle runner.
const GRADLE: &str = "gradle";

/// Contributor for `platform: java` units.
#[derive(Debug, Default, Clone, Copy)]
pub struct JavaContributor;

#[async_trait]
impl Contributor for JavaContributor {
    fn id(&self) -> &'static str {
        "platform-java"
    }

    fn priority(&self) -> i32 {
        50
    }

    fn platform(&self) -> Option<&'static str> {
        Some(PLATFORM)
    }

    /// Claims Maven and Gradle build files as script sources, and working
    /// directories that contain one.
    async fn detect(&self, cx: &DetectCx<'_>) -> Option<u8> {
        let source = cx.script.and_then(|s| s.source.file_name());
        if source.is_some_and(|name| MANIFESTS.iter().any(|m| name == *m)) {
            return Some(DETECT_SCRIPT_SOURCE);
        }
        for manifest in MANIFESTS {
            if tokio::fs::try_exists(cx.cwd.join(manifest))
                .await
                .unwrap_or(false)
            {
                return Some(CWD_CONFIDENCE);
            }
        }
        None
    }

    /// Adds the devlanes trust store to `JAVA_TOOL_OPTIONS` when the CA provides
    /// one, and the non-interactive flags of the Maven and Gradle runners.
    async fn apply(&self, draft: &mut LaunchDraft, cx: &ContribCx<'_>) -> Result<(), Error> {
        if cx.platform != Some(PLATFORM) {
            return Ok(());
        }
        if let Some(store) = cx.ca_env.get(CA_TRUSTSTORE_ENV) {
            trust_store(draft, cx, store);
        }
        if let Some(script) = cx.script {
            build_tool_args(draft, script, cx.kind, cx.pty);
        }
        Ok(())
    }
}

/// Appends the trust store flags to the options an env manager or the client
/// already set, or keeps those options with a note when they name a trust store.
fn trust_store(draft: &mut LaunchDraft, cx: &ContribCx<'_>, store: &str) {
    let base = match draft.env.get(JAVA_TOOL_OPTIONS) {
        Some(value) => Some(value.as_str()),
        None if draft.remove.contains(JAVA_TOOL_OPTIONS) => None,
        None => cx.client_env.get(JAVA_TOOL_OPTIONS).map(String::as_str),
    };
    match java_trust_options(base, store) {
        Some(options) => {
            draft.env.insert(JAVA_TOOL_OPTIONS.to_owned(), options);
        }
        // A shell profile that exports the devlanes flags, as the docs show, is not news.
        None if base.is_some_and(|options| options.contains(store)) => {}
        // Only an options value that already names a trust store gives `None`.
        None => draft.notes.push(
            "JAVA_TOOL_OPTIONS already sets javax.net.ssl.trustStore; keeping that trust store instead of the devlanes one"
                .to_owned(),
        ),
    }
}

/// Flags for a build tool that runs without a terminal or as a long-lived
/// service, leaving out the ones the script name already chooses.
fn build_tool_args(draft: &mut LaunchDraft, script: &ScriptInfo, kind: UnitKind, pty: bool) {
    match script.runner {
        GRADLE => {
            let words = shell_words::split(&script.name)
                .unwrap_or_else(|_| script.name.split_whitespace().map(str::to_owned).collect());
            // A daemon-owned application JVM is not a descendant of the unit, so
            // stop, freeze and metrics would miss it, and a stop could kill a
            // daemon shared with the IDE or other lanes.
            if kind == UnitKind::Service
                && !words.iter().any(|w| w == "--daemon" || w == "--no-daemon")
            {
                draft.args.push("--no-daemon".to_owned());
            }
            if !pty && !words.iter().any(|w| w.starts_with("--console")) {
                draft.args.push("--console=plain".to_owned());
            }
        }
        MAVEN if !pty => {
            draft.args.extend(["-B".to_owned(), "-ntp".to_owned()]);
        }
        _ => {}
    }
}
