use devlanes_core::contrib::{self, ScriptRunner};
use devlanes_run_script_gradle::{
    GradleError, GradleTasks, PROGRAM, TasksRequest, WRAPPER, handles, plan,
};
use std::path::Path;

/// Creates `files` (path, contents) under `root`, with parent directories.
/// Wrappers get the executable bit, as a fresh checkout gives them on Unix.
fn write_files(root: &Path, files: &[(&str, &str)]) {
    for (path, contents) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().expect("file has a parent")).expect("create dirs");
        std::fs::write(&path, contents).expect("write file");
        #[cfg(unix)]
        if path.file_name().is_some_and(|name| name == "gradlew") {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
}

fn request<'a>(
    worktree: &'a Path,
    source: &'a str,
    name: &'a str,
    extra_args: &'a [String],
) -> TasksRequest<'a> {
    TasksRequest {
        worktree,
        source: Path::new(source),
        name,
        extra_args,
    }
}

/// Build and settings files in both DSLs are handled; other files are not.
#[test]
fn handled_files() {
    for file in [
        "build.gradle",
        "web/build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ] {
        assert!(handles(Path::new(file)), "{file}");
    }
    for file in ["gradle.properties", "build.gradle.bak", "pom.xml"] {
        assert!(!handles(Path::new(file)), "{file}");
    }
}

/// The nearest wrapper wins, lookup stops at the worktree root, and `gradle`
/// runs from `PATH` without one.
#[test]
fn wrapper_lookup_stays_in_the_worktree() {
    let dir = tempfile::tempdir().expect("tempdir");
    let worktree = dir.path().join("lane");
    write_files(
        dir.path(),
        &[
            (WRAPPER, "outside the worktree"),
            ("lane/web/build.gradle.kts", ""),
        ],
    );
    let launch = plan(&request(&worktree, "web/build.gradle.kts", "bootRun", &[])).expect("plans");
    assert_eq!(launch.argv, [PROGRAM, "bootRun"]);
    assert_eq!(launch.detail(), "gradle (no wrapper)");
    assert_eq!(launch.cwd, worktree.join("web"));

    write_files(&worktree, &[(WRAPPER, "")]);
    let launch = plan(&request(&worktree, "web/build.gradle.kts", "bootRun", &[])).expect("plans");
    assert_eq!(launch.argv[0], worktree.join(WRAPPER).display().to_string());
    assert_eq!(launch.detail(), format!("{WRAPPER} (wrapper in .)"));
    assert_eq!(launch.cwd, worktree.join("web"));

    write_files(&worktree, &[(&format!("web/{WRAPPER}"), "")]);
    let launch = plan(&request(&worktree, "web/build.gradle.kts", "bootRun", &[])).expect("plans");
    assert_eq!(launch.wrapper, Some(worktree.join("web").join(WRAPPER)));
    assert_eq!(launch.detail(), format!("{WRAPPER} (wrapper in web)"));
    assert!(launch.notes.is_empty(), "{:?}", launch.notes);
}

/// Windows runs `gradlew.bat`; the shell script next to it is not a wrapper there.
#[cfg(windows)]
#[test]
fn windows_uses_the_batch_wrapper() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_files(dir.path(), &[("settings.gradle", ""), ("gradlew", "")]);
    let launch = plan(&request(dir.path(), "settings.gradle", "bootRun", &[])).expect("plans");
    assert_eq!(launch.argv[0], "gradle");
    write_files(dir.path(), &[("gradlew.bat", "")]);
    let launch = plan(&request(dir.path(), "settings.gradle", "bootRun", &[])).expect("plans");
    assert_eq!(WRAPPER, "gradlew.bat");
    assert_eq!(
        launch.argv[0],
        dir.path().join("gradlew.bat").display().to_string()
    );
    assert_eq!(launch.detail(), "gradlew.bat (wrapper in .)");
}

/// Unix runs `gradlew`; a wrapper without the executable bit gets a note.
#[cfg(unix)]
#[test]
fn unix_uses_the_shell_wrapper() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    write_files(dir.path(), &[("settings.gradle", ""), ("gradlew.bat", "")]);
    let launch = plan(&request(dir.path(), "settings.gradle", "bootRun", &[])).expect("plans");
    assert_eq!(launch.argv[0], "gradle");
    write_files(dir.path(), &[("gradlew", "")]);
    let launch = plan(&request(dir.path(), "settings.gradle", "bootRun", &[])).expect("plans");
    assert_eq!(
        launch.argv[0],
        dir.path().join("gradlew").display().to_string()
    );
    assert!(launch.notes.is_empty(), "{:?}", launch.notes);
    std::fs::set_permissions(
        dir.path().join("gradlew"),
        std::fs::Permissions::from_mode(0o644),
    )
    .expect("chmod");
    let launch = plan(&request(dir.path(), "settings.gradle", "bootRun", &[])).expect("plans");
    assert!(
        launch.notes.iter().any(|n| n.contains("chmod +x gradlew")),
        "{:?}",
        launch.notes
    );
}

/// `name` splits with shell quoting, and contributor arguments follow it.
#[test]
fn arguments_split_with_quotes_before_extra_args() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_files(dir.path(), &[("settings.gradle.kts", "")]);
    let extra = vec![
        "--no-daemon".to_owned(),
        "--console=plain".to_owned(),
        "-Ddebug=5005".to_owned(),
    ];
    let launch = plan(&request(
        dir.path(),
        "settings.gradle.kts",
        r#":web:bootRun --offline '-Pgreeting=hello world'"#,
        &extra,
    ))
    .expect("plans");
    assert_eq!(
        launch.argv,
        [
            "gradle",
            ":web:bootRun",
            "--offline",
            "-Pgreeting=hello world",
            "--no-daemon",
            "--console=plain",
            "-Ddebug=5005",
        ]
    );
    assert!(launch.notes.is_empty(), "{:?}", launch.notes);

    // A consumed backslash is most likely a Windows path that lost its separators.
    let launch = plan(&request(
        dir.path(),
        "settings.gradle.kts",
        r"bootRun -I init\dev.gradle",
        &[],
    ))
    .expect("plans");
    assert_eq!(launch.argv, ["gradle", "bootRun", "-I", "initdev.gradle"]);
    assert!(
        launch.notes.iter().any(|n| n.contains("backslash")),
        "{:?}",
        launch.notes
    );
}

/// Arguments that name no task, or do not split, are refused.
#[test]
fn invalid_names_are_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_files(dir.path(), &[("build.gradle", "")]);
    for (name, reason) in [
        ("", "empty"),
        ("  ", "empty"),
        ("bootRun 'unfinished", "unmatched quote"),
        ("--offline", "no task"),
        ("-x test -p web --console plain", "no task"),
    ] {
        let error = plan(&request(dir.path(), "build.gradle", name, &[])).expect_err(name);
        assert!(
            matches!(error, GradleError::InvalidName { .. }),
            "{name}: {error}"
        );
        assert!(error.to_string().contains(reason), "{name}: {error}");
    }
}

/// The file must exist, be a Gradle build or settings file and sit inside the worktree.
#[test]
fn file_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let worktree = dir.path().join("lane");
    write_files(
        dir.path(),
        &[("build.gradle", ""), ("lane/gradle.properties", "")],
    );
    let error = plan(&request(&worktree, "../build.gradle", "bootRun", &[])).expect_err("outside");
    assert!(
        matches!(error, GradleError::OutsideWorktree { .. }),
        "{error}"
    );
    let error =
        plan(&request(&worktree, "gradle.properties", "bootRun", &[])).expect_err("not gradle");
    assert!(matches!(error, GradleError::NotGradle { .. }), "{error}");
    let error = plan(&request(&worktree, "web/build.gradle", "bootRun", &[])).expect_err("missing");
    assert!(matches!(error, GradleError::Missing { .. }), "{error}");
}

/// The adapter has the `gradle` id, matches Gradle files and fills the plan.
#[tokio::test]
async fn adapter_renders_a_script_plan() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_files(dir.path(), &[("app/build.gradle.kts", "")]);
    let runner = GradleTasks;
    assert_eq!(runner.id(), "gradle");
    assert!(runner.matches(Path::new("app/build.gradle.kts")));
    assert!(!runner.matches(Path::new("app/pom.xml")));
    let extra = vec!["--no-daemon".to_owned()];
    let plan = runner
        .resolve(&contrib::ScriptRequest {
            worktree: dir.path(),
            source: Path::new("app/build.gradle.kts"),
            name: "quarkusDev",
            extra_args: &extra,
        })
        .await
        .expect("resolves");
    assert_eq!(plan.argv, ["gradle", "quarkusDev", "--no-daemon"]);
    assert_eq!(plan.cwd, dir.path().join("app"));
    assert_eq!(plan.detail.as_deref(), Some("gradle (no wrapper)"));
    assert_eq!(plan.body, None);
}
