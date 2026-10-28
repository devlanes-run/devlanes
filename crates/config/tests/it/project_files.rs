//! Project files on disk: file-level `extends`, local overlays per file, the files a load
//! reads, and where errors point when several files are merged.

use devlanes_config::*;
use std::path::Path;

/// Writes `files` (name, YAML) into `dir`.
fn write(dir: &Path, files: &[(&str, &str)]) {
    for (name, text) in files {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("dir");
        }
        std::fs::write(path, text).expect("write");
    }
}

/// The service's `run_script` name.
fn script(config: &ProjectConfig, service: &str) -> String {
    config.services[service]
        .run_script
        .as_ref()
        .expect("run_script")
        .file_and_name()
        .expect("script")
        .1
        .to_owned()
}

/// A prod file extends the dev file: it names its own project, and each file's local
/// overlay applies right after it, in order.
#[test]
fn extends_merges_files_and_their_local_overlays_in_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        &[
            (
                "devlanes.yaml",
                "project: acme\nservices:\n  api: { run_script: { file: package.json, name: dev }, tags: [api] }\n  web: { run: web }\n",
            ),
            (
                "devlanes.local.yaml",
                "services:\n  web: { autostart: false }\n",
            ),
            (
                "devlanes.prod.yaml",
                "project: acme-prod\nextends: devlanes.yaml\nservices:\n  api: { run_script: { name: start } }\n",
            ),
            (
                "devlanes.prod.local.yaml",
                "services:\n  api: { tags: [local] }\n",
            ),
        ],
    );
    let entry = dir.path().join("devlanes.prod.yaml");
    let load = load_project_files::<OpaqueExtensions>(&entry, None);
    let (config, _) = load.result.expect("loaded");
    assert_eq!(config.project, "acme-prod");
    assert_eq!(script(&config, "api"), "start");
    assert_eq!(config.services["api"].tags, Some(vec!["local".into()]));
    assert_eq!(
        config.services["web"].autostart,
        Some(Templated::Literal(false))
    );
    assert_eq!(
        config.extends,
        Some(ProjectExtends::File("devlanes.yaml".into()))
    );
    let names: Vec<_> = load
        .files
        .iter()
        .map(|file| {
            file.file_name()
                .expect("name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        names,
        [
            "devlanes.prod.yaml",
            "devlanes.yaml",
            "devlanes.local.yaml",
            "devlanes.prod.local.yaml",
        ]
    );
    let merged: Vec<_> = config.sources.files().map(str::to_owned).collect();
    assert_eq!(merged.len(), 4);
    assert!(merged[3].ends_with("devlanes.prod.local.yaml"));
}

/// A list of bases merges left to right, so the later one wins; paths are relative to
/// the file that names them, and a base may leave out `project`.
#[test]
fn extends_lists_merge_left_to_right_relative_to_the_naming_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        &[
            ("shared/a.yaml", "services:\n  api: { run: one }\n"),
            ("shared/b.yaml", "services:\n  api: { run: two }\n"),
            (
                "devlanes.yaml",
                "project: acme\nextends: [shared/a.yaml, shared/b.yaml]\n",
            ),
        ],
    );
    let config = load_project(dir.path()).expect("loaded");
    assert!(matches!(
        &config.services["api"].run,
        Some(Run::Shell(Templated::Literal(run))) if run == "two"
    ));
}

/// Two bases that extend one shared file merge it once, so the first base's change to
/// it survives the second base.
#[test]
fn a_shared_base_merges_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        &[
            ("shared.yaml", "services:\n  api: { run: shared }\n"),
            (
                "a.yaml",
                "extends: shared.yaml\nservices:\n  api: { run: from-a }\n",
            ),
            (
                "b.yaml",
                "extends: shared.yaml\nservices:\n  web: { run: web }\n",
            ),
            (
                "devlanes.yaml",
                "project: acme\nextends: [a.yaml, b.yaml]\n",
            ),
        ],
    );
    let config = load_project(dir.path()).expect("loaded");
    assert!(matches!(
        &config.services["api"].run,
        Some(Run::Shell(Templated::Literal(run))) if run == "from-a"
    ));
    assert!(config.services.contains_key("web"));
}

/// The entry file must name the project; a name in an extended file is ignored.
#[test]
fn extended_files_do_not_name_the_project() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        &[
            (
                "devlanes.yaml",
                "project: acme\nservices:\n  api: { run: api }\n",
            ),
            ("devlanes.prod.yaml", "extends: devlanes.yaml\n"),
        ],
    );
    let error = load_project(dir.path().join("devlanes.prod.yaml")).expect_err("no name");
    assert!(
        error.message.contains("must set the project name"),
        "{error}"
    );
    assert!(error.location.file.ends_with("devlanes.prod.yaml"));
}

/// Files that extend each other fail with the chain, at the `extends` that closes it.
#[test]
fn extends_cycles_are_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        &[
            ("devlanes.yaml", "project: acme\nextends: other.yaml\n"),
            ("other.yaml", "extends: devlanes.yaml\n"),
        ],
    );
    let error = load_project(dir.path()).expect_err("cycle");
    assert!(
        error
            .message
            .contains("devlanes.yaml -> other.yaml -> devlanes.yaml"),
        "{error}"
    );
    assert!(error.location.file.ends_with("other.yaml"));
}

/// A missing base is reported at the `extends` entry that names it.
#[test]
fn a_missing_base_points_at_its_extends_entry() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        &[(
            "devlanes.yaml",
            "project: acme\nextends:\n  - gone.yaml\nservices:\n  api: { run: api }\n",
        )],
    );
    let error = load_project(dir.path()).expect_err("missing");
    assert!(
        error.message.starts_with("extends.0: cannot read"),
        "{error}"
    );
    assert_eq!(error.location.line, 3);
}

/// Local overlays may not name the project or extend other files, and cannot be loaded
/// as a project themselves.
#[test]
fn local_overlays_stay_overlays() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        &[
            (
                "devlanes.yaml",
                "project: acme\nservices:\n  api: { run: api }\n",
            ),
            ("devlanes.local.yaml", "extends: devlanes.yaml\n"),
        ],
    );
    let error = load_project(dir.path()).expect_err("extends in local");
    assert_eq!(
        error.message,
        "extends: set it in devlanes.yaml, not in devlanes.local.yaml"
    );
    let error = load_project(dir.path().join("devlanes.local.yaml")).expect_err("overlay");
    assert!(error.message.contains("local overlay"), "{error}");
    assert!(is_local_overlay(Path::new("devlanes.prod.local.yaml")));
    assert_eq!(
        local_overlay_path(Path::new("x/devlanes.prod.yaml")),
        Path::new("x/devlanes.prod.local.yaml")
    );
}

/// A lane without its own overlay reads the primary worktree's, at the same place; the
/// files list names both paths so a watcher notices either appearing.
#[test]
fn missing_overlays_fall_back_to_the_primary_worktree() {
    let lane = tempfile::tempdir().expect("lane");
    let primary = tempfile::tempdir().expect("primary");
    write(
        lane.path(),
        &[
            (
                "devlanes.yaml",
                "project: acme\nservices:\n  api: { run: api }\n",
            ),
            (
                "devlanes.prod.yaml",
                "project: acme-prod\nextends: devlanes.yaml\n",
            ),
        ],
    );
    write(
        primary.path(),
        &[(
            "devlanes.local.yaml",
            "services:\n  api: { autostart: false }\n",
        )],
    );
    let load = load_project_files::<OpaqueExtensions>(
        &lane.path().join("devlanes.prod.yaml"),
        Some(LocalFallback {
            lane_dir: lane.path(),
            primary_dir: primary.path(),
        }),
    );
    let (config, _) = load.result.expect("loaded");
    assert_eq!(
        config.services["api"].autostart,
        Some(Templated::Literal(false))
    );
    assert!(
        load.files
            .contains(&primary.path().join("devlanes.local.yaml"))
    );
    assert!(
        load.files
            .contains(&primary.path().join("devlanes.prod.local.yaml"))
    );
}

/// A validation error points at the file that set the field, here the extending one.
#[test]
fn errors_point_at_the_file_that_set_the_field() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(
        dir.path(),
        &[
            (
                "devlanes.yaml",
                "project: acme\nservices:\n  api: { run: api }\n",
            ),
            (
                "devlanes.prod.yaml",
                "project: acme-prod\nextends: devlanes.yaml\nservices:\n  api:\n    extends: missing\n",
            ),
        ],
    );
    let error = load_project(dir.path().join("devlanes.prod.yaml")).expect_err("template");
    assert!(
        error.location.file.ends_with("devlanes.prod.yaml"),
        "{error:?}"
    );
    assert_eq!(error.location.line, 5);
}

/// In-memory parsing has no files to extend.
#[test]
fn parsing_text_refuses_extends() {
    let error = parse_project("project: acme\nextends: devlanes.yaml\n", None).expect_err("text");
    assert!(error.message.starts_with("extends:"), "{error}");
}
