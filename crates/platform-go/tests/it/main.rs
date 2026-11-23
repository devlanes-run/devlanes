//! Go detection on tempdir projects and env on fake launch drafts.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use devlanes_core::contrib::{
    CA_BUNDLE_ENV, ContribCx, ContribMemo, Contributor, DETECT_MANIFEST, DETECT_SCRIPT_SOURCE,
    DetectCx, DraftCommand, LaunchDraft, ScriptInfo, UnitKind,
};
use devlanes_platform_go::GoContributor;

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

#[tokio::test]
/// `go.mod` and `go.work` claim below node in the cwd; `go.mod` claims strongly as a script source.
async fn detection() {
    for file in ["go.mod", "go.work"] {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            GoContributor.detect(&detect_cx(dir.path(), None)).await,
            None
        );
        std::fs::write(dir.path().join(file), "go 1.22\n").expect("write");
        let cwd = GoContributor
            .detect(&detect_cx(dir.path(), None))
            .await
            .expect("claim");
        assert!(cwd < DETECT_MANIFEST, "{file}");
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let script = ScriptInfo {
        source: dir.path().join("go.mod"),
        name: "api".into(),
        body: None,
        cwd: dir.path().to_path_buf(),
        runner: "go",
    };
    assert_eq!(
        GoContributor
            .detect(&detect_cx(dir.path(), Some(&script)))
            .await,
        Some(DETECT_SCRIPT_SOURCE)
    );
}

/// Applies the contributor for a unit with `platform`.
async fn apply(
    platform: &str,
    client: &BTreeMap<String, String>,
    ca: &BTreeMap<String, String>,
) -> LaunchDraft {
    let lane = PathBuf::from("/w");
    let memo = ContribMemo::default();
    let ports = BTreeMap::new();
    let cx = ContribCx {
        kind: UnitKind::Service,
        project: "acme",
        name: "api",
        replica: Some(1),
        lane_index: 1,
        lane_name: "lane1",
        lane_path: &lane,
        lane_data_path: std::path::Path::new("/data/demo/1"),
        primary_path: &lane,
        platform: Some(platform),
        preset: None,
        preset_options: None,
        port: Some(21_000),
        ports: &ports,
        public_url: None,
        pty: false,
        script: None,
        client_env: client,
        client_spelled: client,
        ca_env: ca,
        memo: &memo,
    };
    let mut draft = LaunchDraft {
        command: DraftCommand::Argv(vec!["go".into(), "run".into(), ".".into()]),
        args: Vec::new(),
        cwd: lane.clone(),
        env: BTreeMap::new(),
        remove: BTreeSet::new(),
        files: Vec::new(),
        notes: Vec::new(),
    };
    GoContributor.apply(&mut draft, &cx).await.expect("apply");
    draft
}

#[tokio::test]
/// The combined bundle replaces a shell `SSL_CERT_FILE` with a note; other platforms get nothing.
async fn env() {
    let ca = BTreeMap::from([(CA_BUNDLE_ENV.to_owned(), "/ca/bundle.pem".to_owned())]);
    let draft = apply("go", &BTreeMap::new(), &ca).await;
    assert_eq!(
        draft.env,
        BTreeMap::from([("SSL_CERT_FILE".to_owned(), "/ca/bundle.pem".to_owned())])
    );
    assert!(draft.notes.is_empty());

    let shell = BTreeMap::from([("SSL_CERT_FILE".to_owned(), "/corp.pem".to_owned())]);
    let draft = apply("go", &shell, &ca).await;
    assert_eq!(draft.env["SSL_CERT_FILE"], "/ca/bundle.pem");
    assert_eq!(draft.notes.len(), 1);
    assert!(draft.notes[0].contains("/corp.pem"), "{:?}", draft.notes);

    let same = BTreeMap::from([("SSL_CERT_FILE".to_owned(), "/ca/bundle.pem".to_owned())]);
    let draft = apply("go", &same, &ca).await;
    assert_eq!(draft.env["SSL_CERT_FILE"], "/ca/bundle.pem");
    assert!(draft.notes.is_empty());
    assert!(apply("rust", &BTreeMap::new(), &ca).await.env.is_empty());
}
