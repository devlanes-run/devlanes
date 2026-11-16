//! Java detection on tempdir projects and env and arguments on fake launch drafts.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use devlanes_core::contrib::{
    CA_TRUSTSTORE_ENV, ContribCx, ContribMemo, Contributor, DETECT_MANIFEST, DETECT_SCRIPT_SOURCE,
    DetectCx, DraftCommand, JAVA_TOOL_OPTIONS, LaunchDraft, ScriptInfo, UnitKind,
};
use devlanes_platform_java::JavaContributor;

/// Detection context for a service running in `dir`.
fn detect_cx<'a>(dir: &'a Path, script: Option<&'a ScriptInfo>) -> DetectCx<'a> {
    DetectCx {
        kind: UnitKind::Service,
        name: "api",
        lane_path: dir,
        cwd: dir,
        script,
    }
}

/// A `run_script` handled by `runner` from `file` in `dir`.
fn script(dir: &Path, file: &str, runner: &'static str) -> ScriptInfo {
    named_script(dir, file, runner, "run")
}

/// A `run_script` named `name`, handled by `runner` from `file` in `dir`.
fn named_script(dir: &Path, file: &str, runner: &'static str, name: &str) -> ScriptInfo {
    ScriptInfo {
        source: dir.join(file),
        name: name.into(),
        body: None,
        cwd: dir.to_path_buf(),
        runner,
    }
}

#[tokio::test]
/// Build files claim strongly as script sources and, in the cwd, below every
/// other platform's manifest claim.
async fn detection() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(
        JavaContributor.detect(&detect_cx(dir.path(), None)).await,
        None
    );
    std::fs::write(dir.path().join("settings.gradle.kts"), "").expect("write");
    let cwd = JavaContributor
        .detect(&detect_cx(dir.path(), None))
        .await
        .expect("claim");
    // platform-python claims `pyproject.toml` with `DETECT_MANIFEST - 10` at the
    // same priority, and ties go to the smaller id, which is java's.
    assert!(cwd < DETECT_MANIFEST - 10);
    for (file, runner) in [("pom.xml", "maven"), ("build.gradle", "gradle")] {
        let script = script(dir.path(), file, runner);
        assert_eq!(
            JavaContributor
                .detect(&detect_cx(dir.path(), Some(&script)))
                .await,
            Some(DETECT_SCRIPT_SOURCE)
        );
    }
}

/// One launch to plan: the unit's facts and what earlier layers set.
struct Unit<'a> {
    platform: &'a str,
    kind: UnitKind,
    pty: bool,
    script: Option<&'a ScriptInfo>,
    client: BTreeMap<String, String>,
    draft_env: BTreeMap<String, String>,
    ca: BTreeMap<String, String>,
}

impl Default for Unit<'_> {
    /// A Java service without a pty, script or client env, with a trust store.
    fn default() -> Self {
        Self {
            platform: "java",
            kind: UnitKind::Service,
            pty: false,
            script: None,
            client: BTreeMap::new(),
            draft_env: BTreeMap::new(),
            ca: BTreeMap::from([(
                CA_TRUSTSTORE_ENV.to_owned(),
                "/ca/truststore.p12".to_owned(),
            )]),
        }
    }
}

/// Applies the contributor to `unit` and returns the draft.
async fn apply(unit: Unit<'_>) -> LaunchDraft {
    let lane = PathBuf::from("/w");
    let memo = ContribMemo::default();
    let ports = BTreeMap::new();
    let cx = ContribCx {
        kind: unit.kind,
        project: "acme",
        name: "api",
        replica: Some(1),
        lane_index: 1,
        lane_name: "lane1",
        lane_path: &lane,
        lane_data_path: Path::new("/data/acme/1"),
        primary_path: &lane,
        platform: Some(unit.platform),
        preset: None,
        preset_options: None,
        port: Some(21_000),
        ports: &ports,
        public_url: None,
        pty: unit.pty,
        script: unit.script,
        client_env: &unit.client,
        ca_env: &unit.ca,
        memo: &memo,
    };
    let mut draft = LaunchDraft {
        command: DraftCommand::Shell("java -jar app.jar".into()),
        args: Vec::new(),
        cwd: lane.clone(),
        env: unit.draft_env,
        remove: BTreeSet::new(),
        files: Vec::new(),
        notes: Vec::new(),
    };
    JavaContributor.apply(&mut draft, &cx).await.expect("apply");
    draft
}

/// The flags pointing a JVM at `/ca/truststore.p12`.
const FLAGS: &str = "-Djavax.net.ssl.trustStore=/ca/truststore.p12 \
                     -Djavax.net.ssl.trustStoreType=PKCS12 -Djavax.net.ssl.trustStorePassword=changeit";

#[tokio::test]
/// The trust store flags extend an env manager's options before the client's,
/// and stay out with a note when either already names a trust store.
async fn trust_store_options() {
    let draft = apply(Unit::default()).await;
    assert_eq!(draft.env[JAVA_TOOL_OPTIONS], FLAGS);
    assert!(draft.notes.is_empty());

    let client = BTreeMap::from([(JAVA_TOOL_OPTIONS.to_owned(), "-Xmx1g".to_owned())]);
    let draft = apply(Unit {
        client: client.clone(),
        ..Unit::default()
    })
    .await;
    assert_eq!(draft.env[JAVA_TOOL_OPTIONS], format!("-Xmx1g {FLAGS}"));

    let draft = apply(Unit {
        client,
        draft_env: BTreeMap::from([(JAVA_TOOL_OPTIONS.to_owned(), "-Xss4m".to_owned())]),
        ..Unit::default()
    })
    .await;
    assert_eq!(draft.env[JAVA_TOOL_OPTIONS], format!("-Xss4m {FLAGS}"));

    let mine = "-Djavax.net.ssl.trustStore=/mine.jks";
    let draft = apply(Unit {
        client: BTreeMap::from([(JAVA_TOOL_OPTIONS.to_owned(), mine.to_owned())]),
        ..Unit::default()
    })
    .await;
    assert!(!draft.env.contains_key(JAVA_TOOL_OPTIONS));
    assert_eq!(draft.notes.len(), 1, "{:?}", draft.notes);

    // The devlanes flags exported from a shell profile are kept without a note.
    let draft = apply(Unit {
        client: BTreeMap::from([(JAVA_TOOL_OPTIONS.to_owned(), FLAGS.to_owned())]),
        ..Unit::default()
    })
    .await;
    assert!(!draft.env.contains_key(JAVA_TOOL_OPTIONS));
    assert!(draft.notes.is_empty(), "{:?}", draft.notes);

    let without_store = apply(Unit {
        ca: BTreeMap::new(),
        ..Unit::default()
    })
    .await;
    assert!(without_store.env.is_empty());
    let other = apply(Unit {
        platform: "node",
        ..Unit::default()
    })
    .await;
    assert!(other.env.is_empty());
}

#[tokio::test]
/// Gradle services run without a daemon, and both build tools drop their
/// interactive output when the unit has no terminal.
async fn build_tool_arguments() {
    let dir = Path::new("/w");
    let gradle = script(dir, "build.gradle.kts", "gradle");
    let maven = script(dir, "pom.xml", "maven");
    let other = script(dir, "package.json", "npm");
    let cases = [
        (
            &gradle,
            UnitKind::Service,
            false,
            vec!["--no-daemon", "--console=plain"],
        ),
        (&gradle, UnitKind::Service, true, vec!["--no-daemon"]),
        (&gradle, UnitKind::Task, false, vec!["--console=plain"]),
        (&gradle, UnitKind::Task, true, vec![]),
        (&maven, UnitKind::Service, false, vec!["-B", "-ntp"]),
        (&maven, UnitKind::Task, true, vec![]),
        (&other, UnitKind::Service, false, vec![]),
    ];
    for (script, kind, pty, expected) in cases {
        let draft = apply(Unit {
            kind,
            pty,
            script: Some(script),
            ..Unit::default()
        })
        .await;
        assert_eq!(draft.args, expected, "{} {kind:?} pty={pty}", script.runner);
    }
}

#[tokio::test]
/// Gradle flags the script name already sets are not added a second time.
async fn gradle_arguments_from_the_script_name() {
    let dir = Path::new("/w");
    let cases = [
        ("bootRun --daemon", vec!["--console=plain"]),
        ("bootRun --no-daemon", vec!["--console=plain"]),
        ("bootRun --console=rich", vec!["--no-daemon"]),
        ("bootRun --console verbose", vec!["--no-daemon"]),
        (
            "bootRun '--args=--console x'",
            vec!["--no-daemon", "--console=plain"],
        ),
        // An unmatched quote falls back to splitting on whitespace.
        ("bootRun --daemon 'x", vec!["--console=plain"]),
    ];
    for (name, expected) in cases {
        let gradle = named_script(dir, "build.gradle.kts", "gradle", name);
        let draft = apply(Unit {
            script: Some(&gradle),
            ..Unit::default()
        })
        .await;
        assert_eq!(draft.args, expected, "{name}");
    }
}
