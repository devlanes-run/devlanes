use devlanes_core::contrib::{ScriptRequest, ScriptRunner};
use devlanes_run_script_go::{GoError, GoPackages, PackageRequest, plan};
use std::path::Path;

/// Creates `files` (path, contents) under `root`, with parent directories.
fn write_files(root: &Path, files: &[(&str, &str)]) {
    for (path, contents) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().expect("file has a parent")).expect("create dirs");
        std::fs::write(path, contents).expect("write file");
    }
}

/// `text` with `/` for `\`, so the snapshots hold on Windows.
fn slashes(text: String) -> String {
    text.replace('\\', "/")
}

fn request<'a>(
    worktree: &'a Path,
    source: &'a str,
    name: &'a str,
    extra_args: &'a [String],
) -> PackageRequest<'a> {
    PackageRequest {
        worktree,
        source: Path::new(source),
        name,
        extra_args,
    }
}

/// A library module with a `go generate` program at the root, programs under
/// `cmd/` whose package clause follows comments, and a program module nested
/// in a subdirectory.
fn fixture(root: &Path) {
    write_files(
        root,
        &[
            ("go.mod", "module example.invalid/acme\n\ngo 1.22\n"),
            ("acme.go", "package acme\n"),
            (
                "gen.go",
                "// Code generator.\r\n\r\n//go:build ignore\r\n\r\npackage main\r\n",
            ),
            ("cmd/README.md", "# Programs\n"),
            (
                "cmd/api/main.go",
                "\u{feff}// Copyright Acme.\n/* Licensed under\n   Apache-2.0. */ /* x */\n\n//go:build !windows\n\npackage main // the server\n",
            ),
            ("cmd/api/main_test.go", "package main\n"),
            ("cmd/web/web.go", "/* one line */ package main\n"),
            // Tests and files go ignores do not make a main package.
            ("cmd/tool/tool_test.go", "package main\n"),
            ("cmd/tool/_scratch.go", "package main\n"),
            ("cmd/tool/tool.go", "package mainly\n"),
            ("internal/store/store.go", "package store\n"),
            ("services/worker/go.mod", "module example.invalid/worker\n"),
            ("services/worker/main.go", "package main\n"),
        ],
    );
}

/// Name forms map to local package paths; arguments follow the package.
#[test]
fn renders_go_run() {
    let temp = tempfile::tempdir().expect("tempdir");
    fixture(temp.path());
    let args = ["--port".to_owned(), "20010".to_owned()];
    let mut rendered = String::new();
    for (source, name, args) in [
        ("go.mod", "api", &args[..]),
        ("go.mod", "cmd/web", &[][..]),
        ("./go.mod", "./cmd/api/", &[][..]),
        ("go.mod", "cmd/../cmd/web", &[][..]),
        ("services/worker/go.mod", ".", &args[..]),
    ] {
        let plan = plan(&request(temp.path(), source, name, args)).expect("plan");
        let cwd = slashes(
            plan.cwd
                .strip_prefix(temp.path())
                .expect("inside worktree")
                .display()
                .to_string(),
        );
        rendered.push_str(&format!(
            "{source}#{name}: cwd={cwd:?} argv={}\n",
            plan.argv.join(" ")
        ));
    }
    insta::assert_snapshot!(rendered, @r#"
    go.mod#api: cwd="" argv=go run ./cmd/api --port 20010
    go.mod#cmd/web: cwd="" argv=go run ./cmd/web
    ./go.mod#./cmd/api/: cwd="" argv=go run ./cmd/api
    go.mod#cmd/../cmd/web: cwd="" argv=go run ./cmd/web
    services/worker/go.mod#.: cwd="services/worker" argv=go run . --port 20010
    "#);
}

/// Missing and non-main packages list the main packages that exist.
#[test]
fn missing_main_package() {
    let temp = tempfile::tempdir().expect("tempdir");
    fixture(temp.path());
    write_files(
        temp.path(),
        &[
            ("lib/go.mod", "module example.invalid/lib\n"),
            ("lib/lib.go", "package lib\n"),
        ],
    );
    let message = |source: &str, name: &str| {
        slashes(
            plan(&request(temp.path(), source, name, &[]))
                .expect_err("error")
                .to_string()
                .replace(&temp.path().display().to_string(), "<lane>"),
        )
    };
    insta::assert_snapshot!(
        [
            message("go.mod", "."),
            message("go.mod", "worker"),
            message("go.mod", "tool"),
            message("go.mod", "internal/store"),
            message("lib/go.mod", "."),
        ]
        .join("\n"),
        @r"
    `<lane>/go.mod` has no main package `.` (main packages: ./cmd/api, ./cmd/web)
    `<lane>/go.mod` has no main package `./cmd/worker` (main packages: ./cmd/api, ./cmd/web)
    `<lane>/go.mod` has no main package `./cmd/tool` (main packages: ./cmd/api, ./cmd/web)
    `<lane>/go.mod` has no main package `./internal/store` (main packages: ./cmd/api, ./cmd/web)
    `<lane>/lib/go.mod` has no main package `.` (no main package at the module root or under cmd/)
    "
    );
}

/// Names `go run` would read as something other than one package of this module.
#[test]
fn rejects_names() {
    let temp = tempfile::tempdir().expect("tempdir");
    fixture(temp.path());
    for name in [
        "",
        "-race",
        "./cmd/...",
        "example.invalid/tool@v1.2.3",
        "/srv/api",
        "..",
        "cmd/../../api",
        "cmd\\api",
        "c:api",
        "cmd/c:api",
    ] {
        assert!(
            matches!(
                plan(&request(temp.path(), "go.mod", name, &[])),
                Err(GoError::InvalidName { .. })
            ),
            "{name}"
        );
    }
    assert!(matches!(
        plan(&request(temp.path(), "../go.mod", "api", &[])),
        Err(GoError::OutsideWorktree { .. })
    ));
}

/// The core adapter matches go.mod and maps a bad name to invalid input.
#[tokio::test]
async fn adapter_resolves_through_core_trait() {
    let temp = tempfile::tempdir().expect("tempdir");
    fixture(temp.path());
    let runner = GoPackages;
    assert!(runner.matches(Path::new("services/worker/go.mod")));
    assert!(!runner.matches(Path::new("go.work")));
    assert!(!runner.matches(Path::new("go.sum")));
    let args = vec!["--port".to_owned(), "1".to_owned()];
    let request = ScriptRequest {
        worktree: temp.path(),
        source: Path::new("go.mod"),
        name: "api",
        extra_args: &args,
    };
    let plan = runner.resolve(&request).await.expect("resolve");
    assert_eq!(plan.argv, ["go", "run", "./cmd/api", "--port", "1"]);
    assert_eq!(plan.cwd, temp.path());
    let missing = ScriptRequest {
        name: "worker",
        ..request
    };
    let error = runner.resolve(&missing).await.expect_err("missing package");
    assert!(
        matches!(error, devlanes_core::Error::InvalidInput(_)),
        "{error}"
    );
}
