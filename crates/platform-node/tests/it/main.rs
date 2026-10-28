//! Node and Cloudflare contributor behavior on fake launch drafts.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use devlanes_core::contrib::{
    CA_CERT_ENV, ContribCx, ContribMemo, Contributor, DETECT_MANIFEST, DETECT_SCRIPT_SOURCE,
    DetectCx, DraftCommand, LaunchDraft, ScriptInfo, UnitKind,
};
use devlanes_platform_node::{CloudflareContributor, NodeContributor, REGISTRY_ENVS};

/// Owned data behind a [`ContribCx`].
struct Fixture {
    lane: PathBuf,
    data: PathBuf,
    client: BTreeMap<String, String>,
    ca: BTreeMap<String, String>,
    ports: BTreeMap<String, u16>,
    memo: ContribMemo,
}

impl Fixture {
    /// Fixture for a lane at `lane` with a devlanes CA.
    fn new(lane: PathBuf) -> Self {
        Self {
            lane,
            data: PathBuf::from("/data/acme/3"),
            client: BTreeMap::new(),
            ca: BTreeMap::from([(CA_CERT_ENV.to_owned(), "/ca/root.pem".to_owned())]),
            ports: BTreeMap::new(),
            memo: ContribMemo::default(),
        }
    }

    /// Context for a service with `platform`.
    fn cx(&self, platform: Option<&'static str>) -> ContribCx<'_> {
        ContribCx {
            kind: UnitKind::Service,
            project: "acme",
            name: "web",
            replica: Some(1),
            lane_index: 1,
            lane_name: "lane1",
            lane_path: &self.lane,
            lane_data_path: &self.data,
            primary_path: &self.lane,
            platform,
            preset: None,
            preset_options: None,
            port: Some(21_000),
            ports: &self.ports,
            public_url: None,
            pty: true,
            script: None,
            client_env: &self.client,
            ca_env: &self.ca,
            memo: &self.memo,
        }
    }
}

/// An empty draft.
fn draft() -> LaunchDraft {
    LaunchDraft {
        command: DraftCommand::Shell("node server.js".into()),
        args: Vec::new(),
        cwd: PathBuf::from("/w"),
        env: BTreeMap::new(),
        remove: BTreeSet::new(),
        files: Vec::new(),
        notes: Vec::new(),
    }
}

#[tokio::test]
/// `package.json` claims: strongly as a script source, at manifest level in the cwd.
async fn node_detection() {
    /// Detection context for a task running in `dir`.
    fn cx<'a>(dir: &'a Path, script: Option<&'a ScriptInfo>) -> DetectCx<'a> {
        DetectCx {
            kind: UnitKind::Task,
            name: "build",
            lane_path: dir,
            cwd: dir,
            script,
        }
    }
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(NodeContributor.detect(&cx(dir.path(), None)).await, None);
    std::fs::write(dir.path().join("package.json"), "{}").expect("write");
    assert_eq!(
        NodeContributor.detect(&cx(dir.path(), None)).await,
        Some(DETECT_MANIFEST)
    );
    let script = ScriptInfo {
        source: dir.path().join("web/package.json"),
        name: "dev".into(),
        body: None,
        cwd: dir.path().join("web"),
        runner: "npm",
    };
    assert_eq!(
        NodeContributor.detect(&cx(dir.path(), Some(&script))).await,
        Some(DETECT_SCRIPT_SOURCE)
    );
    assert_eq!(NodeContributor.platform(), Some("node"));
    assert_eq!(CloudflareContributor.platform(), None);
}

#[tokio::test]
/// Node units get the CA and color; other platforms and `platform: none` are untouched.
async fn node_env() {
    let fixture = Fixture::new("/w".into());
    let mut node = draft();
    NodeContributor
        .apply(&mut node, &fixture.cx(Some("node")))
        .await
        .expect("apply");
    assert_eq!(node.env["NODE_EXTRA_CA_CERTS"], "/ca/root.pem");
    assert_eq!(node.env["FORCE_COLOR"], "1");
    assert!(node.notes.is_empty());

    for platform in ["python", "none"] {
        let mut other = draft();
        NodeContributor
            .apply(&mut other, &fixture.cx(Some(platform)))
            .await
            .expect("apply");
        assert!(other.env.is_empty(), "{platform}");
    }
}

#[tokio::test]
/// A shell color choice wins; a shell CA is replaced with a note.
async fn node_respects_client_choices() {
    let mut fixture = Fixture::new("/w".into());
    fixture.client.insert("NO_COLOR".into(), "1".into());
    fixture
        .client
        .insert("NODE_EXTRA_CA_CERTS".into(), "/other.pem".into());
    let mut node = draft();
    NodeContributor
        .apply(&mut node, &fixture.cx(Some("node")))
        .await
        .expect("apply");
    assert!(!node.env.contains_key("FORCE_COLOR"));
    assert_eq!(node.env["NODE_EXTRA_CA_CERTS"], "/ca/root.pem");
    assert_eq!(node.notes.len(), 1);
}

/// Expected draft env: every registry variable set to `<data>/wrangler-registry`.
fn registry_env(data: &str) -> BTreeMap<String, String> {
    let registry = Path::new(data)
        .join("wrangler-registry")
        .display()
        .to_string();
    REGISTRY_ENVS
        .iter()
        .map(|name| ((*name).to_owned(), registry.clone()))
        .collect()
}

#[tokio::test]
/// Every unit gets the lane's registry in every registry variable, whatever its platform
/// and whether or not the worktree holds a Wrangler config.
async fn cloudflare_registry() {
    assert_eq!(
        REGISTRY_ENVS,
        [
            "WRANGLER_REGISTRY_PATH",
            "CLOUDFLARE_DEV_REGISTRY_PATH",
            "MINIFLARE_REGISTRY_PATH"
        ]
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let fixture = Fixture::new(dir.path().to_path_buf());
    for platform in [None, Some("node"), Some("python"), Some("none")] {
        let mut unit = draft();
        CloudflareContributor
            .apply(&mut unit, &fixture.cx(platform))
            .await
            .expect("apply");
        assert_eq!(unit.env, registry_env("/data/acme/3"), "{platform:?}");
    }

    // A second lane on the same folder gets its own registry.
    let mut other = Fixture::new(dir.path().to_path_buf());
    other.data = PathBuf::from("/data/acme/4");
    let mut second = draft();
    CloudflareContributor
        .apply(&mut second, &other.cx(None))
        .await
        .expect("apply");
    assert_eq!(second.env, registry_env("/data/acme/4"));
}

#[tokio::test]
/// A registry exported in the shell does not leak into the lane: the draft value is the
/// one the launch uses, since contributor env wins over the client env.
async fn cloudflare_registry_replaces_shell_values() {
    let mut fixture = Fixture::new("/w".into());
    for name in REGISTRY_ENVS {
        fixture
            .client
            .insert(name.to_owned(), "/home/acme/.config/cloudflare".into());
    }
    let mut unit = draft();
    CloudflareContributor
        .apply(&mut unit, &fixture.cx(None))
        .await
        .expect("apply");
    assert_eq!(unit.env, registry_env("/data/acme/3"));
}
