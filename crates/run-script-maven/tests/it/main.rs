use devlanes_core::contrib::{self, ScriptRunner};
use devlanes_run_script_maven::{
    GoalsRequest, MavenError, MavenGoals, PROGRAM, WRAPPER, goals, plan, split_arguments,
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
        if path.file_name().is_some_and(|name| name == "mvnw") {
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
) -> GoalsRequest<'a> {
    GoalsRequest {
        worktree,
        source: Path::new(source),
        name,
        extra_args,
    }
}

const BOOT_POM: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<project xmlns="http://maven.apache.org/POM/4.0.0">
  <artifactId>web</artifactId>
  <build>
    <plugins>
      <plugin>
        <groupId>org.springframework.boot</groupId>
        <artifactId>spring-boot-maven-plugin</artifactId>
      </plugin>
    </plugins>
  </build>
</project>
"#;

const BARE_POM: &str = "<project><artifactId>acme</artifactId></project>";

/// The nearest wrapper wins, lookup stops at the worktree root, and `mvn`
/// runs from `PATH` without one.
#[test]
fn wrapper_lookup_stays_in_the_worktree() {
    let dir = tempfile::tempdir().expect("tempdir");
    let worktree = dir.path().join("lane");
    write_files(
        dir.path(),
        &[
            (WRAPPER, "outside the worktree"),
            ("lane/web/pom.xml", BARE_POM),
        ],
    );
    let launch = plan(&request(&worktree, "web/pom.xml", "package", &[])).expect("plans");
    assert_eq!(launch.argv, [PROGRAM, "package"]);
    assert_eq!(launch.wrapper, None);
    assert_eq!(launch.detail(), "mvn (no wrapper)");
    assert_eq!(launch.cwd, worktree.join("web"));

    write_files(&worktree, &[(WRAPPER, "")]);
    let launch = plan(&request(&worktree, "web/pom.xml", "package", &[])).expect("plans");
    assert_eq!(launch.argv[0], worktree.join(WRAPPER).display().to_string());
    assert_eq!(launch.detail(), format!("{WRAPPER} (wrapper in .)"));
    assert_eq!(launch.cwd, worktree.join("web"));

    write_files(&worktree, &[(&format!("web/{WRAPPER}"), "")]);
    let launch = plan(&request(&worktree, "web/pom.xml", "package", &[])).expect("plans");
    assert_eq!(launch.wrapper, Some(worktree.join("web").join(WRAPPER)));
    assert_eq!(launch.detail(), format!("{WRAPPER} (wrapper in web)"));
    assert!(launch.notes.is_empty(), "{:?}", launch.notes);
}

/// Windows runs `mvnw.cmd`; the shell script next to it is not a wrapper there.
#[cfg(windows)]
#[test]
fn windows_uses_the_batch_wrapper() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_files(dir.path(), &[("pom.xml", BARE_POM), ("mvnw", "")]);
    let launch = plan(&request(dir.path(), "pom.xml", "package", &[])).expect("plans");
    assert_eq!(launch.argv[0], "mvn");
    write_files(dir.path(), &[("mvnw.cmd", "")]);
    let launch = plan(&request(dir.path(), "pom.xml", "package", &[])).expect("plans");
    assert_eq!(WRAPPER, "mvnw.cmd");
    assert_eq!(
        launch.argv[0],
        dir.path().join("mvnw.cmd").display().to_string()
    );
    assert_eq!(launch.detail(), "mvnw.cmd (wrapper in .)");
}

/// Unix runs `mvnw`; a wrapper without the executable bit gets a note.
#[cfg(unix)]
#[test]
fn unix_uses_the_shell_wrapper() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    write_files(dir.path(), &[("pom.xml", BARE_POM), ("mvnw.cmd", "")]);
    let launch = plan(&request(dir.path(), "pom.xml", "package", &[])).expect("plans");
    assert_eq!(launch.argv[0], "mvn");
    write_files(dir.path(), &[("mvnw", "")]);
    let launch = plan(&request(dir.path(), "pom.xml", "package", &[])).expect("plans");
    assert_eq!(
        launch.argv[0],
        dir.path().join("mvnw").display().to_string()
    );
    assert!(launch.notes.is_empty(), "{:?}", launch.notes);
    std::fs::set_permissions(
        dir.path().join("mvnw"),
        std::fs::Permissions::from_mode(0o644),
    )
    .expect("chmod");
    let launch = plan(&request(dir.path(), "pom.xml", "package", &[])).expect("plans");
    assert!(
        launch.notes.iter().any(|n| n.contains("chmod +x mvnw")),
        "{:?}",
        launch.notes
    );
}

/// `name` splits with shell quoting, and contributor arguments follow it.
#[test]
fn arguments_split_with_quotes_before_extra_args() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_files(dir.path(), &[("pom.xml", BOOT_POM)]);
    let extra = vec![
        "-B".to_owned(),
        "-ntp".to_owned(),
        "-Ddebug=5005".to_owned(),
    ];
    let launch = plan(&request(
        dir.path(),
        "pom.xml",
        r#"spring-boot:run "-Dspring-boot.run.jvmArguments=-Xmx1g -Dx=y" -Pdev"#,
        &extra,
    ))
    .expect("plans");
    assert_eq!(
        launch.argv,
        [
            "mvn",
            "spring-boot:run",
            "-Dspring-boot.run.jvmArguments=-Xmx1g -Dx=y",
            "-Pdev",
            "-B",
            "-ntp",
            "-Ddebug=5005",
        ]
    );
    assert!(launch.notes.is_empty(), "{:?}", launch.notes);

    let launch = plan(&request(
        dir.path(),
        "pom.xml",
        "-pl web -am spring-boot:run",
        &[],
    ))
    .expect("plans");
    assert_eq!(
        launch.argv,
        ["mvn", "-pl", "web", "-am", "spring-boot:run"],
        "-pl/-am are kept as typed and never added"
    );
}

/// Arguments that name no goal or phase, or do not split, are refused.
#[test]
fn invalid_names_are_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_files(dir.path(), &[("pom.xml", BARE_POM)]);
    for (name, reason) in [
        ("", "empty"),
        ("   ", "empty"),
        (r#"spring-boot:run "-Dx=y"#, "unmatched quote"),
        ("-Pdev", "no goal"),
        ("-pl web -am", "no goal"),
        ("-P dev -f other/pom.xml -T 4 -D skipTests", "no goal"),
        ("-emp secret --encrypt-master-password secret", "no goal"),
        ("-ep secret --encrypt-password secret", "no goal"),
        ("--color always", "no goal"),
    ] {
        let error = plan(&request(dir.path(), "pom.xml", name, &[])).expect_err(name);
        assert!(
            matches!(error, MavenError::InvalidName { .. }),
            "{name}: {error}"
        );
        assert!(error.to_string().contains(reason), "{name}: {error}");
    }
}

/// Values of options that take one are not goals; `-c` (`--lax-checksums`)
/// takes none.
#[test]
fn option_values_are_not_goals() {
    let tokens = split_arguments("--color never -ep -emp x -c package").expect("splits");
    assert_eq!(goals(&tokens), ["package"]);
    let tokens = split_arguments("-c package --encrypt-password=x").expect("splits");
    assert_eq!(goals(&tokens), ["package"]);
}

/// The pom must exist, be a pom.xml and sit inside the worktree.
#[test]
fn file_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let worktree = dir.path().join("lane");
    write_files(dir.path(), &[("pom.xml", BARE_POM), ("lane/build.xml", "")]);
    let error = plan(&request(&worktree, "../pom.xml", "package", &[])).expect_err("outside");
    assert!(
        matches!(error, MavenError::OutsideWorktree { .. }),
        "{error}"
    );
    let error = plan(&request(&worktree, "build.xml", "package", &[])).expect_err("not a pom");
    assert!(matches!(error, MavenError::NotPom { .. }), "{error}");
    let error = plan(&request(&worktree, "web/pom.xml", "package", &[])).expect_err("missing");
    assert!(matches!(error, MavenError::Missing { .. }), "{error}");
}

/// A `prefix:goal` whose plugin is not declared gets a note; a declaration in
/// a parent reached through `relativePath` counts; phases, full coordinates
/// and default-group plugins are not checked.
#[test]
fn plugin_prefix_notes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let parent = r#"<project xmlns="http://maven.apache.org/POM/4.0.0">
  <artifactId>acme-parent</artifactId>
  <build><pluginManagement><plugins>
    <plugin><groupId>io.quarkus</groupId><artifactId>quarkus-maven-plugin</artifactId></plugin>
  </plugins></pluginManagement></build>
</project>"#;
    let child = r#"<project xmlns="http://maven.apache.org/POM/4.0.0">
  <parent>
    <artifactId>acme-parent</artifactId>
    <relativePath>../build/parent</relativePath>
  </parent>
  <artifactId>app</artifactId>
</project>"#;
    let orphan = r#"<project>
  <parent><artifactId>acme-parent</artifactId><relativePath/></parent>
  <artifactId>orphan</artifactId>
</project>"#;
    let mismatch = r#"<project>
  <parent><artifactId>spring-boot-starter-parent</artifactId></parent>
  <artifactId>web</artifactId>
</project>"#;
    write_files(
        root,
        &[
            ("build/parent/pom.xml", parent),
            ("app/pom.xml", child),
            ("orphan/pom.xml", orphan),
            ("pom.xml", parent),
            ("web/pom.xml", mismatch),
        ],
    );
    let notes = |source: &str, name: &str| {
        plan(&request(root, source, name, &[]))
            .expect("plans")
            .notes
    };

    assert_eq!(notes("app/pom.xml", "quarkus:dev"), Vec::<String>::new());
    let missing = notes("app/pom.xml", "spring-boot:run");
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert!(
        missing[0].contains("spring-boot-maven-plugin")
            && missing[0].contains("No plugin found for prefix 'spring-boot'")
            && missing[0].contains("outside the worktree"),
        "{missing:?}"
    );
    // An empty relativePath turns the local parent lookup off.
    assert_eq!(notes("orphan/pom.xml", "quarkus:dev").len(), 1);
    // The default ../pom.xml is another project, so it is not the parent.
    assert_eq!(notes("web/pom.xml", "quarkus:dev").len(), 1);
    // With another pom picked, the source pom's plugins say nothing.
    for name in [
        "-f ../app/pom.xml quarkus:dev",
        "--file ../app quarkus:dev",
        "--file=../app/pom.xml quarkus:dev",
    ] {
        assert_eq!(notes("web/pom.xml", name), Vec::<String>::new(), "{name}");
    }

    for unchecked in [
        "clean package",
        "dependency:tree",
        "io.quarkus:quarkus-maven-plugin:3.0.0:dev",
        "before:compile",
    ] {
        assert_eq!(
            notes("orphan/pom.xml", unchecked),
            Vec::<String>::new(),
            "{unchecked}"
        );
    }
}

/// A pom that does not parse is a note, not an error: Maven reports it itself.
#[test]
fn malformed_pom_is_a_note() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_files(dir.path(), &[("pom.xml", "<project><build>")]);
    let launch = plan(&request(dir.path(), "pom.xml", "quarkus:dev", &[])).expect("plans");
    assert_eq!(launch.argv, ["mvn", "quarkus:dev"]);
    assert!(
        launch
            .notes
            .iter()
            .any(|n| n.contains("cannot parse pom.xml")),
        "{:?}",
        launch.notes
    );
}

/// The adapter has the `maven` id, matches only pom.xml and fills the plan.
#[tokio::test]
async fn adapter_renders_a_script_plan() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_files(dir.path(), &[("web/pom.xml", BOOT_POM)]);
    let runner = MavenGoals;
    assert_eq!(runner.id(), "maven");
    assert!(runner.matches(Path::new("web/pom.xml")));
    assert!(!runner.matches(Path::new("web/build.gradle")));
    let extra = vec!["-B".to_owned()];
    let plan = runner
        .resolve(&contrib::ScriptRequest {
            worktree: dir.path(),
            source: Path::new("web/pom.xml"),
            name: "spring-boot:run",
            extra_args: &extra,
        })
        .await
        .expect("resolves");
    assert_eq!(plan.argv, ["mvn", "spring-boot:run", "-B"]);
    assert_eq!(plan.cwd, dir.path().join("web"));
    assert_eq!(plan.detail.as_deref(), Some("mvn (no wrapper)"));
    assert_eq!(plan.body, None);

    let error = runner
        .resolve(&contrib::ScriptRequest {
            worktree: dir.path(),
            source: Path::new("web/pom.xml"),
            name: "-Pdev",
            extra_args: &[],
        })
        .await
        .expect_err("no goal");
    assert!(error.to_string().contains("no goal"), "{error}");
}
