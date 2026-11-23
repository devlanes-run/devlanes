//! `preset: tauri`: runs several Tauri dev apps side by side on one machine.
//!
//! Each lane and replica gets its own bundle `identifier`, which keys the app
//! data, webview data and keychain entries, plus its own `build.devUrl`. Both
//! go into a JSON overlay passed with `tauri dev --config <file>`, which Tauri
//! v2 merges over `tauri.conf.json` (<https://v2.tauri.app/reference/cli/#dev>).
//! Each lane and replica also builds into its own Cargo target folder in the
//! lane data folder (`CARGO_TARGET_DIR`); `isolate_target_dir: false` turns
//! that off.
//!
//! Local only: the app runs on the machine that runs the lane. Remote boxes
//! need GUI streaming, which is out of scope for v1.

use async_trait::async_trait;
use devlanes_core::{
    Error,
    contrib::{ContribCx, Contributor, LaunchDraft, PreparedFile},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// Preset name, also the key that holds [`TauriOptions`] in a service.
pub const NAME: &str = "tauri";

/// Also set to the overlay path, for wrapper scripts that start `tauri dev`
/// themselves and do not forward extra arguments.
pub const CONFIG_ENV: &str = "DEVLANES_TAURI_CONFIG";

/// Cargo's target folder variable, set per lane and replica unless
/// `isolate_target_dir: false`.
pub const TARGET_DIR_ENV: &str = "CARGO_TARGET_DIR";

/// Options for `preset: tauri`, which lets several Tauri v2 dev apps run side by side, one per
/// lane and replica. The values go into a config overlay at
/// `${{ lane.data_path }}/tauri/<service>.<replica>.conf.json`, passed with `--config` and
/// in `DEVLANES_TAURI_CONFIG`, which Tauri merges over `tauri.conf.json`. Each instance builds
/// into its own Cargo target folder unless `isolate_target_dir` is `false`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("examples" = [{"identifier":"io.acme.app.lane${{ lane.index }}.client${{ replica.index }}","dev_url":"${{ services.desktop_web.url }}","before_dev_command":"","isolate_target_dir":true}]))]
pub struct TauriOptions {
    /// Bundle identifier for this lane and replica, which keys the app data, webview data and
    /// keychain entries. Required and must differ per lane and replica, for example
    /// `com.acme.app.dev.lane${{ lane.index }}.client${{ replica.index }}`; two instances with
    /// the same value are refused at start. Only letters, digits, `-` and `.` are allowed, with
    /// no empty part between dots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("examples" = ["io.acme.app.lane${{ lane.index }}.client${{ replica.index }}"]))]
    pub identifier: Option<String>,
    /// Replaces the app's `productName` from `tauri.conf.json` for this instance, for example
    /// to tell lanes apart. Default: the value in `tauri.conf.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("examples" = ["Acme Dev"]))]
    pub product_name: Option<String>,
    /// Frontend address the app loads, written to `build.devUrl`. Usually another service's
    /// `url`, such as `${{ services.desktop_web.url }}`, so each lane's app loads that lane's
    /// frontend. Must start with `http://` or `https://`; when unset, `tauri.conf.json` decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("examples" = ["${{ services.desktop_web.url }}"]))]
    pub dev_url: Option<String>,
    /// Replaces `build.beforeDevCommand`, the command Tauri runs before the app. Set it to `""`
    /// when the frontend runs as its own devlanes service; otherwise every replica starts its
    /// own frontend on the port from `tauri.conf.json`. Default: the command in
    /// `tauri.conf.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("examples" = ["","pnpm dev"]))]
    pub before_dev_command: Option<String>,
    /// Points `CARGO_TARGET_DIR` at `tauri/target/<service>.<replica>` in the lane's data
    /// folder (`${{ lane.data_path }}`), so each lane and replica builds its own copy of the
    /// app. Tauri compiles the identifier and dev URL into the app, so copies that share a
    /// target folder rebuild each other's app, and on Windows the rebuild fails while another
    /// copy runs. The cost is about one full Cargo target folder on disk per lane and replica,
    /// and the first start of each is a full build. This replaces a `CARGO_TARGET_DIR` from
    /// the shell, mise or direnv. One set in the project file's `env` (the service's, a
    /// profile's or `global_env`) wins; it must still differ per lane and replica. Build
    /// scripts and `externalBin` or `resources` paths that point into `../target/` need the
    /// same folder. Set `false` to leave the target folder to your shell or Cargo's config.
    /// Default: `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("examples" = [true,false]))]
    pub isolate_target_dir: Option<bool>,
}

impl TauriOptions {
    /// Applies a local override or `extends` overlay; set fields replace.
    pub fn merge(&mut self, overlay: Self) {
        let Self {
            identifier,
            product_name,
            dev_url,
            before_dev_command,
            isolate_target_dir,
        } = overlay;
        if identifier.is_some() {
            self.identifier = identifier;
        }
        if product_name.is_some() {
            self.product_name = product_name;
        }
        if dev_url.is_some() {
            self.dev_url = dev_url;
        }
        if before_dev_command.is_some() {
            self.before_dev_command = before_dev_command;
        }
        if isolate_target_dir.is_some() {
            self.isolate_target_dir = isolate_target_dir;
        }
    }
}

/// Arguments and environment a preset adds to a service's launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contribution {
    /// Appended to the command; runners decide whether a `--` precedes them.
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// Everything [`contribution`] needs for one replica.
#[derive(Debug, Clone, Copy)]
pub struct TauriInput<'a> {
    pub options: &'a TauriOptions,
    /// Absolute path the overlay is written to; one per lane and replica.
    pub overlay_path: &'a Path,
    /// Cargo target folder for this lane and replica, from [`target_dir`].
    pub target_dir: &'a Path,
}

/// The launch additions plus the overlay the caller writes with
/// [`write_overlay`] before spawning.
#[derive(Debug, Clone, PartialEq)]
pub struct TauriLaunch {
    pub contribution: Contribution,
    pub overlay: Value,
}

/// Errors that stop a Tauri replica from starting.
#[derive(Debug, thiserror::Error)]
pub enum TauriError {
    #[error(
        "preset tauri needs `tauri.identifier`, distinct per lane and replica, e.g. `com.acme.app.dev.lane${{{{ lane.index }}}}.client${{{{ replica.index }}}}`"
    )]
    MissingIdentifier,
    #[error("tauri identifier `{identifier}` is invalid: {reason}")]
    InvalidIdentifier {
        identifier: String,
        reason: &'static str,
    },
    #[error("tauri dev_url `{url}` must start with http:// or https://")]
    InvalidDevUrl { url: String },
    #[error(
        "`{first}` and `{second}` both use tauri identifier `{identifier}`; include lane.index and replica.index in it so their app data stays apart"
    )]
    DuplicateIdentifier {
        identifier: String,
        first: String,
        second: String,
    },
    #[error("cannot write the tauri config overlay `{}`", .path.display())]
    Write {
        path: PathBuf,
        #[source]
        error: std::io::Error,
    },
}

/// Returns the named ports to allocate before expressions are evaluated. The
/// app itself listens on nothing devlanes routes.
pub fn required_ports() -> &'static [&'static str] {
    &[]
}

/// Returns the overlay path for `service` replica `replica`: `tauri/` in the lane data
/// folder, which no other lane shares, even one running from the same worktree.
/// The service name is encoded on Windows, where it may hold `:` or name a device.
pub fn overlay_path(lane_data: &Path, service: &str, replica: u32) -> PathBuf {
    let service = devlanes_core::paths::name_on_disk(service);
    lane_data
        .join("tauri")
        .join(format!("{service}.{replica}.conf.json"))
}

/// Returns the Cargo target folder for `service` replica `replica`:
/// `tauri/target/<service>.<replica>` in the lane data folder.
pub fn target_dir(lane_data: &Path, service: &str, replica: u32) -> PathBuf {
    let service = devlanes_core::paths::name_on_disk(service);
    lane_data
        .join("tauri")
        .join("target")
        .join(format!("{service}.{replica}"))
}

/// Builds the Tauri v2 config overlay: top-level `identifier` and
/// `productName`, `build.devUrl` and `build.beforeDevCommand`.
pub fn overlay(options: &TauriOptions) -> Result<Value, TauriError> {
    let identifier = options
        .identifier
        .as_deref()
        .ok_or(TauriError::MissingIdentifier)?;
    validate_identifier(identifier)?;
    let mut root = Map::new();
    root.insert("identifier".into(), identifier.into());
    if let Some(name) = &options.product_name {
        root.insert("productName".into(), name.as_str().into());
    }
    let mut build = Map::new();
    if let Some(url) = &options.dev_url {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(TauriError::InvalidDevUrl { url: url.clone() });
        }
        build.insert("devUrl".into(), url.as_str().into());
    }
    if let Some(command) = &options.before_dev_command {
        build.insert("beforeDevCommand".into(), command.as_str().into());
    }
    if !build.is_empty() {
        root.insert("build".into(), Value::Object(build));
    }
    Ok(Value::Object(root))
}

/// Renders `--config <overlay_path>`, the overlay to write there and, unless
/// `isolate_target_dir` is `false`, `CARGO_TARGET_DIR`.
pub fn contribution(input: &TauriInput<'_>) -> Result<TauriLaunch, TauriError> {
    let overlay = overlay(input.options)?;
    let path = input.overlay_path.to_string_lossy().into_owned();
    let mut env = BTreeMap::new();
    if input.options.isolate_target_dir != Some(false) {
        env.insert(
            TARGET_DIR_ENV.to_owned(),
            devlanes_core::paths::forward_slashes(input.target_dir),
        );
    }
    env.insert(CONFIG_ENV.to_owned(), path.clone());
    Ok(TauriLaunch {
        contribution: Contribution {
            args: vec!["--config".to_owned(), path],
            env,
        },
        overlay,
    })
}

/// Writes `overlay` to `path` through a temporary file and a rename, so a
/// starting replica never reads a half-written file. Creates parent
/// directories. Blocking.
pub fn write_overlay(path: &Path, overlay: &Value) -> Result<(), TauriError> {
    let write_error = |error| TauriError::Write {
        path: path.to_owned(),
        error,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(write_error)?;
    }
    let mut bytes =
        serde_json::to_vec_pretty(overlay).map_err(|error| write_error(error.into()))?;
    bytes.push(b'\n');
    let file_name = path
        .file_name()
        .map_or_else(Default::default, |name| name.to_string_lossy());
    let temp = path.with_file_name(format!(".{file_name}.tmp"));
    std::fs::write(&temp, bytes).map_err(write_error)?;
    std::fs::rename(&temp, path).map_err(write_error)
}

/// Fails when two instances share an identifier. `instances` pairs a label
/// for messages (such as `desktop#2 in lane 1`) with the evaluated identifier.
/// Call it across every lane and replica of a project before starting any.
pub fn ensure_distinct<'a>(
    instances: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<(), TauriError> {
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for (label, identifier) in instances {
        if let Some(first) = seen.insert(identifier, label) {
            return Err(TauriError::DuplicateIdentifier {
                identifier: identifier.to_owned(),
                first: first.to_owned(),
                second: label.to_owned(),
            });
        }
    }
    Ok(())
}

/// Applies Tauri's rule ("only alphanumeric characters, hyphens and periods",
/// <https://v2.tauri.app/reference/config/#identifier>) plus non-empty dot
/// segments, which macOS bundle identifiers require.
fn validate_identifier(identifier: &str) -> Result<(), TauriError> {
    let reason = if identifier.is_empty() {
        "it is empty"
    } else if !identifier
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        "only A-Z, a-z, 0-9, `-` and `.` are allowed"
    } else if identifier.split('.').any(str::is_empty) {
        "it has an empty segment between dots"
    } else {
        return Ok(());
    };
    Err(TauriError::InvalidIdentifier {
        identifier: identifier.to_owned(),
        reason,
    })
}

/// JSON Schema of [`TauriOptions`], for the config's `Extensions` registration.
pub fn options_schema() -> schemars::Schema {
    schemars::schema_for!(TauriOptions)
}

/// Parses evaluated options. A service without a `tauri:` key gets defaults,
/// which then fail on the missing identifier with an example.
pub fn parse_options(value: Option<&Value>) -> Result<TauriOptions, Error> {
    match value {
        None | Some(Value::Null) => Ok(TauriOptions::default()),
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|e| Error::InvalidInput(format!("{NAME} options: {e}"))),
    }
}

/// Identifiers claimed by replicas resolved so far in one lane, keyed by
/// identifier with the claiming replica's label.
type Claims = Mutex<BTreeMap<String, String>>;

/// Core contributor for services with `preset: tauri`.
///
/// The overlay goes into `draft.files` as a [`PreparedFile`] rather than being
/// written here: contributors only plan, and the daemon writes prepared files
/// right before it spawns, so a failed resolution leaves no stale overlay.
#[derive(Debug, Default, Clone, Copy)]
pub struct TauriPreset;

#[async_trait]
impl Contributor for TauriPreset {
    fn id(&self) -> &'static str {
        "preset-tauri"
    }

    fn priority(&self) -> i32 {
        100
    }

    /// Adds `--config <overlay>`, the overlay file and, unless `isolate_target_dir: false`,
    /// this replica's `CARGO_TARGET_DIR`, after checking that no other replica in the lane
    /// uses the same identifier.
    async fn apply(&self, draft: &mut LaunchDraft, cx: &ContribCx<'_>) -> Result<(), Error> {
        if cx.preset != Some(NAME) {
            return Ok(());
        }
        let options = parse_options(cx.preset_options)?;
        let replica = cx.replica.unwrap_or(1);
        let path = overlay_path(cx.lane_data_path, cx.name, u32::from(replica));
        let target = target_dir(cx.lane_data_path, cx.name, u32::from(replica));
        let launch = contribution(&TauriInput {
            options: &options,
            overlay_path: &path,
            target_dir: &target,
        })
        .map_err(|e| Error::InvalidInput(e.to_string()))?;
        if let Some(identifier) = options.identifier.as_deref() {
            claim(cx, identifier, &format!("{}#{replica}", cx.name))?;
        }
        let mut contents = serde_json::to_vec_pretty(&launch.overlay)
            .map_err(|e| Error::Integration(format!("cannot render the tauri overlay: {e}")))?;
        contents.push(b'\n');
        draft.files.push(PreparedFile { path, contents });
        // A folder from the shell, mise or direnv is replaced; say so.
        if let Some(folder) = launch.contribution.env.get(TARGET_DIR_ENV)
            && let Some(previous) = draft
                .env
                .get(TARGET_DIR_ENV)
                .or_else(|| cx.client_env.get(TARGET_DIR_ENV))
            && previous != folder
        {
            draft.notes.push(format!(
                "CARGO_TARGET_DIR ({previous}) replaced by this replica's own folder; set `tauri: {{ isolate_target_dir: false }}` to keep it, or set CARGO_TARGET_DIR per lane and replica in the service env"
            ));
        }
        // Presets require `run_script`; its runner places the arguments for the tool.
        draft.args.extend(launch.contribution.args);
        draft.env.extend(launch.contribution.env);
        Ok(())
    }
}

/// Records `identifier` for `label` in the lane's memo and fails when another
/// replica already holds it. Lanes are resolved separately, so this catches
/// expressions that ignore `replica.index`; [`ensure_distinct`] covers lanes.
fn claim(cx: &ContribCx<'_>, identifier: &str, label: &str) -> Result<(), Error> {
    let key = format!("{NAME}:identifiers:{}", cx.lane_path.display());
    let claims = cx.memo.get::<Claims>(&key).unwrap_or_else(|| {
        let claims = Arc::new(Claims::default());
        cx.memo.insert(key, claims.clone());
        claims
    });
    let mut claims = claims.lock().unwrap_or_else(PoisonError::into_inner);
    match claims.get(identifier) {
        Some(first) if first != label => Err(Error::InvalidInput(
            TauriError::DuplicateIdentifier {
                identifier: identifier.to_owned(),
                first: first.clone(),
                second: label.to_owned(),
            }
            .to_string(),
        )),
        _ => {
            claims.insert(identifier.to_owned(), label.to_owned());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devlanes_core::contrib::{ContribMemo, DraftCommand, UnitKind};
    use std::collections::BTreeSet;

    /// Runs [`TauriPreset`] for replica `replica` of `desktop` in lane 1, over a draft
    /// whose environment starts as `env`.
    async fn apply_replica(
        memo: &ContribMemo,
        options: Option<Value>,
        replica: u16,
        env: BTreeMap<String, String>,
    ) -> Result<LaunchDraft, Error> {
        let ports = BTreeMap::new();
        let empty = BTreeMap::new();
        let cx = ContribCx {
            kind: UnitKind::Service,
            project: "acme",
            name: "desktop",
            replica: Some(replica),
            lane_index: 1,
            lane_name: "lane1",
            lane_path: Path::new("/lanes/acme-1"),
            lane_data_path: Path::new("/data/acme/5"),
            primary_path: Path::new("/lanes/acme"),
            platform: Some("node"),
            preset: Some(NAME),
            preset_options: options.as_ref(),
            port: Some(20015),
            ports: &ports,
            public_url: None,
            pty: true,
            script: None,
            client_env: &empty,
            ca_env: &empty,
            memo,
        };
        let mut draft = LaunchDraft {
            command: DraftCommand::Shell("pnpm --filter @acme/desktop dev".to_owned()),
            args: Vec::new(),
            cwd: PathBuf::from("/lanes/acme-1"),
            env,
            remove: BTreeSet::new(),
            files: Vec::new(),
            notes: Vec::new(),
        };
        TauriPreset.apply(&mut draft, &cx).await?;
        Ok(draft)
    }

    /// Each replica gets its own overlay file, `--config`, env pointer and Cargo
    /// target folder; a folder set earlier, for example by mise, is replaced and reported.
    #[tokio::test]
    async fn adapter_prepares_overlay_per_replica() {
        let memo = ContribMemo::default();
        for replica in [1_u16, 2] {
            let options = serde_json::json!({
                "identifier": format!("com.acme.app.dev.lane1.client{replica}"),
                "dev_url": "http://127.0.0.1:20014",
                "before_dev_command": "",
                "product_name": null,
            });
            let draft = apply_replica(&memo, Some(options), replica, BTreeMap::new())
                .await
                .expect("apply");
            assert_eq!(
                draft.env.get(TARGET_DIR_ENV).map(String::as_str),
                Some(format!("/data/acme/5/tauri/target/desktop.{replica}").as_str())
            );
            let expected = Path::new("/data/acme/5")
                .join("tauri")
                .join(format!("desktop.{replica}.conf.json"))
                .display()
                .to_string();
            assert_eq!(draft.args, ["--config", expected.as_str()]);
            assert_eq!(draft.env.get(CONFIG_ENV), Some(&expected));
            let [file] = draft.files.as_slice() else {
                panic!("expected one prepared file, got {}", draft.files.len());
            };
            assert_eq!(file.path, Path::new(&expected));
            assert!(draft.notes.is_empty(), "{:?}", draft.notes);
            let written: Value = serde_json::from_slice(&file.contents).expect("overlay json");
            assert_eq!(
                written["identifier"],
                format!("com.acme.app.dev.lane1.client{replica}")
            );
            assert_eq!(written["build"]["devUrl"], "http://127.0.0.1:20014");
        }

        // A shared folder from mise would bring the clash back, so it is replaced and reported.
        let prior = BTreeMap::from([(TARGET_DIR_ENV.to_owned(), "/set/by/mise".to_owned())]);
        let options = serde_json::json!({ "identifier": "com.acme.app.dev.lane1.client3" });
        let draft = apply_replica(&memo, Some(options), 3, prior)
            .await
            .expect("apply");
        assert_eq!(
            draft.env.get(TARGET_DIR_ENV).map(String::as_str),
            Some("/data/acme/5/tauri/target/desktop.3")
        );
        let [note] = draft.notes.as_slice() else {
            panic!("expected one note, got {:?}", draft.notes);
        };
        assert!(
            note.contains("/set/by/mise") && note.contains("isolate_target_dir: false"),
            "{note}"
        );
    }

    /// Replicas sharing an identifier, a missing identifier and unknown keys fail.
    #[tokio::test]
    async fn adapter_rejects_bad_options() {
        let memo = ContribMemo::default();
        let same = || Some(serde_json::json!({ "identifier": "com.acme.app.dev" }));
        apply_replica(&memo, same(), 1, BTreeMap::new())
            .await
            .expect("first replica");
        // `LaunchDraft` has no `Debug` (it may hold secrets), so no `expect_err`.
        let duplicate = apply_replica(&memo, same(), 2, BTreeMap::new())
            .await
            .err()
            .expect("duplicate");
        assert!(
            duplicate
                .to_string()
                .contains("`desktop#1` and `desktop#2`"),
            "{duplicate}"
        );
        let missing = apply_replica(&ContribMemo::default(), None, 1, BTreeMap::new())
            .await
            .err()
            .expect("missing");
        assert!(
            missing.to_string().contains("tauri.identifier"),
            "{missing}"
        );
        let unknown = apply_replica(
            &ContribMemo::default(),
            Some(serde_json::json!({ "bundle_id": "x" })),
            1,
            BTreeMap::new(),
        )
        .await;
        assert!(matches!(unknown, Err(Error::InvalidInput(_))));
    }

    /// The exported schema is the options struct's schema.
    #[test]
    fn schema_lists_identifier() {
        let schema = options_schema().to_value();
        assert!(schema["properties"]["identifier"].is_object(), "{schema}");
    }

    fn options(identifier: &str) -> TauriOptions {
        TauriOptions {
            identifier: Some(identifier.to_owned()),
            dev_url: Some("http://127.0.0.1:20014".to_owned()),
            before_dev_command: Some(String::new()),
            product_name: None,
            isolate_target_dir: None,
        }
    }

    /// The overlay uses Tauri v2 key names and nesting. Compared as values:
    /// key order depends on serde_json's workspace-unified `preserve_order`.
    #[test]
    fn overlay_shape() {
        let mut with_name = options("com.acme.app.dev.lane1.client2");
        with_name.product_name = Some("Acme Dev 2".to_owned());
        let value = overlay(&with_name).expect("valid options");
        let expected = serde_json::json!({
            "identifier": "com.acme.app.dev.lane1.client2",
            "productName": "Acme Dev 2",
            "build": { "devUrl": "http://127.0.0.1:20014", "beforeDevCommand": "" }
        });
        assert_eq!(value, expected);
        let minimal = TauriOptions {
            identifier: Some("com.acme".to_owned()),
            ..TauriOptions::default()
        };
        assert_eq!(
            overlay(&minimal).expect("valid options"),
            serde_json::json!({"identifier": "com.acme"})
        );
    }

    /// `--config` and the env variable point at the same file; `isolate_target_dir: false`
    /// leaves `CARGO_TARGET_DIR` unset.
    #[test]
    fn contribution_points_at_overlay() {
        let mut options = options("com.acme.app.dev.client1");
        let path = overlay_path(Path::new("/data/acme/5"), "desktop", 1);
        let target = target_dir(Path::new("/data/acme/5"), "desktop", 1);
        let launch = contribution(&TauriInput {
            options: &options,
            overlay_path: &path,
            target_dir: &target,
        })
        .expect("valid options");
        // Snapshots use `/`; Windows joins below the lane data folder with `\`.
        let args = launch.contribution.args.join(" ").replace('\\', "/");
        insta::assert_snapshot!(args, @"--config /data/acme/5/tauri/desktop.1.conf.json");
        assert_eq!(
            launch.contribution.env.get(CONFIG_ENV).map(String::as_str),
            Some(path.display().to_string().as_str())
        );

        options.isolate_target_dir = Some(false);
        let launch = contribution(&TauriInput {
            options: &options,
            overlay_path: &path,
            target_dir: &target,
        })
        .expect("valid options");
        assert!(
            !launch.contribution.env.contains_key(TARGET_DIR_ENV),
            "{:?}",
            launch.contribution.env
        );
        assert!(launch.contribution.env.contains_key(CONFIG_ENV));
    }

    /// Service names are file names as they are on Unix; Windows encodes the
    /// characters and device names it refuses.
    #[test]
    fn overlay_names_follow_the_host() {
        let path = overlay_path(Path::new("data"), "aux", 1);
        let name = path.file_name().and_then(|name| name.to_str());
        #[cfg(unix)]
        assert_eq!(name, Some("aux.1.conf.json"));
        #[cfg(windows)]
        assert_eq!(name, Some("%61ux.1.conf.json"));
    }

    /// Identifier problems are reported before anything starts.
    #[test]
    fn identifier_validation() {
        assert!(matches!(
            overlay(&TauriOptions::default()),
            Err(TauriError::MissingIdentifier)
        ));
        for bad in ["", "com.acme app", "com..acme", "com.acme.", "com_acme"] {
            assert!(
                matches!(
                    overlay(&options(bad)),
                    Err(TauriError::InvalidIdentifier { .. })
                ),
                "{bad:?} should be rejected"
            );
        }
        let mut bad_url = options("com.acme");
        bad_url.dev_url = Some("127.0.0.1:1".to_owned());
        assert!(matches!(
            overlay(&bad_url),
            Err(TauriError::InvalidDevUrl { .. })
        ));
    }

    /// Two lanes or replicas sharing an identifier would share app data.
    #[test]
    fn duplicate_identifiers_are_rejected() {
        let ok = ensure_distinct([("desktop#1", "a.client1"), ("desktop#2", "a.client2")]);
        assert!(ok.is_ok());
        let error = ensure_distinct([
            ("lane 0 desktop#1", "a.client1"),
            ("lane 1 desktop#1", "a.client1"),
        ])
        .expect_err("duplicate identifier");
        insta::assert_snapshot!(error.to_string(), @"`lane 0 desktop#1` and `lane 1 desktop#1` both use tauri identifier `a.client1`; include lane.index and replica.index in it so their app data stays apart");
    }

    /// The written overlay round-trips and leaves no temporary file behind.
    #[test]
    fn write_overlay_is_atomic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = overlay_path(dir.path(), "desktop", 2);
        let value = overlay(&options("com.acme.client2")).expect("valid options");
        write_overlay(&path, &value).expect("write overlay");
        let read: Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read overlay")).expect("json");
        assert_eq!(read, value);
        let entries = std::fs::read_dir(path.parent().expect("parent"))
            .expect("list")
            .count();
        assert_eq!(entries, 1);
    }

    /// The missing-identifier message shows a working expression.
    #[test]
    fn missing_identifier_message() {
        insta::assert_snapshot!(TauriError::MissingIdentifier.to_string(), @"preset tauri needs `tauri.identifier`, distinct per lane and replica, e.g. `com.acme.app.dev.lane${{ lane.index }}.client${{ replica.index }}`");
    }
}
