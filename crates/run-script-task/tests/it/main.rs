use devlanes_core::contrib::{ScriptRequest, ScriptRunner};
use devlanes_run_script_task::{TaskCheck, TaskError, TaskRequest, TaskfileTasks, plan};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TASKFILE: &str = "\
version: '3'

tasks:
  dev:
    cmds:
      - echo \"args=[{{.CLI_ARGS}}]\"

  fixtures: ./load.sh

  migrate:
    internal: true
    cmds:
      - ./migrate.sh
";

/// Creates `files` (path, contents) under `root`, with parent directories.
fn write_files(root: &Path, files: &[(&str, &str)]) {
    for (path, contents) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().expect("file has a parent")).expect("create dirs");
        std::fs::write(path, contents).expect("write file");
    }
}

/// `text` with the temp dir as `<lane>` and `/` separators, so snapshots hold
/// on every OS.
fn shown(text: String, root: &Path) -> String {
    text.replace(&root.display().to_string(), "<lane>")
        .replace('\\', "/")
}

fn request<'a>(
    worktree: &'a Path,
    source: &'a str,
    name: &'a str,
    extra_args: &'a [String],
) -> TaskRequest<'a> {
    TaskRequest {
        worktree,
        source: Path::new(source),
        name,
        extra_args,
    }
}

/// The file is always named, and preset arguments go after `--` only when
/// there are some.
#[test]
fn renders_argv() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_files(
        temp.path(),
        &[
            ("Taskfile.yml", TASKFILE),
            ("apps/web/Taskfile.dist.yaml", TASKFILE),
        ],
    );
    let args = ["--port".to_owned(), "20012".to_owned()];
    let mut rendered = String::new();
    for source in ["Taskfile.yml", "apps/web/Taskfile.dist.yaml"] {
        for extra in [&[][..], &args[..]] {
            let plan = plan(&request(temp.path(), source, "dev", extra)).expect("plan");
            rendered.push_str(&format!(
                "{} in {}\n",
                plan.argv.join(" "),
                shown(plan.cwd.display().to_string(), temp.path())
            ));
            assert!(plan.notes.is_empty());
        }
    }
    insta::assert_snapshot!(rendered, @r"
    task --taskfile Taskfile.yml dev in <lane>
    task --taskfile Taskfile.yml dev -- --port 20012 in <lane>
    task --taskfile Taskfile.dist.yaml dev in <lane>/apps/web
    task --taskfile Taskfile.dist.yaml dev -- --port 20012 in <lane>/apps/web
    ");
}

/// Missing and internal tasks fail before starting; arguments a task never
/// reads only add a note.
#[test]
fn checks_task() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_files(
        temp.path(),
        &[
            ("Taskfile.yml", TASKFILE),
            ("broken/Taskfile.yml", "tasks: [dev]\n"),
            ("shared/Taskfile.yml", "includes:\n  db: ./db\n"),
        ],
    );
    let args = vec!["--port".to_owned(), "1".to_owned()];
    let messages: Vec<String> = [
        plan(&request(temp.path(), "Taskfile.yml", "seed", &[])),
        plan(&request(temp.path(), "Taskfile.yml", "migrate", &[])),
        plan(&request(temp.path(), "broken/Taskfile.yml", "dev", &[])),
        plan(&request(temp.path(), "Taskfile.yml", "NAME=x", &[])),
    ]
    .into_iter()
    .map(|result| shown(result.expect_err("task error").to_string(), temp.path()))
    .collect();
    insta::assert_snapshot!(messages.join("\n"), @r"
    `<lane>/Taskfile.yml` has no task `seed` (tasks: dev, fixtures)
    task `migrate` in `<lane>/Taskfile.yml` is internal, so task refuses to run it from the command line; run a task that calls it, or remove `internal: true`
    cannot parse `<lane>/broken/Taskfile.yml`: `tasks` is not a mapping
    invalid task name `NAME=x`: it contains `=` and would be read as a variable assignment
    ");
    let fixtures = plan(&request(temp.path(), "Taskfile.yml", "fixtures", &args)).expect("plan");
    insta::assert_snapshot!(fixtures.notes.join("\n"), @"task `fixtures` never reads {{.CLI_ARGS}}, so it ignores the arguments after `--` (--port 1); add {{.CLI_ARGS}} to the command that should get them");
    let included =
        plan(&request(temp.path(), "shared/Taskfile.yml", "db:up", &args)).expect("plan");
    assert_eq!(
        included.check,
        TaskCheck::Unverifiable {
            reason: "it includes other Taskfiles"
        }
    );
    assert!(included.notes.is_empty());
    assert!(matches!(
        plan(&request(temp.path(), "../Taskfile.yml", "dev", &[])),
        Err(TaskError::OutsideWorktree { .. })
    ));
}

/// The core adapter matches Taskfile names, keeps the notes and maps errors.
#[tokio::test]
async fn adapter_resolves_through_core_trait() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_files(temp.path(), &[("taskfile.yaml", TASKFILE)]);
    let runner = TaskfileTasks;
    assert_eq!(runner.id(), "task");
    assert!(runner.matches(Path::new("apps/api/taskfile.yaml")));
    assert!(!runner.matches(Path::new("Taskfile.ci.yml")));
    assert!(!runner.matches(Path::new("TASKFILE.YML")));
    let args = vec!["--port".to_owned(), "1".to_owned()];
    let request = ScriptRequest {
        worktree: temp.path(),
        source: Path::new("taskfile.yaml"),
        name: "fixtures",
        extra_args: &args,
    };
    let plan = runner.resolve(&request).await.expect("resolve");
    assert_eq!(
        plan.argv,
        [
            "task",
            "--taskfile",
            "taskfile.yaml",
            "fixtures",
            "--",
            "--port",
            "1"
        ]
    );
    assert_eq!(plan.notes.len(), 1);
    let missing = ScriptRequest {
        name: "seed",
        ..request
    };
    let error = runner.resolve(&missing).await.expect_err("missing task");
    assert!(
        matches!(error, devlanes_core::Error::InvalidInput(_)),
        "{error}"
    );
}

/// Preset flags reach `{{.CLI_ARGS}}` through the real binary.
#[test]
#[ignore = "needs task"]
fn real_task_forwards_port() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_files(temp.path(), &[("Taskfile.yml", TASKFILE)]);
    let args = vec!["--port".to_owned(), "4321".to_owned()];
    let plan = plan(&request(temp.path(), "Taskfile.yml", "dev", &args)).expect("plan");
    let mut child = Command::new(&plan.argv[0])
        .args(&plan.argv[1..])
        .current_dir(&plan.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn task");
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll task") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("task timed out");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut output = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        std::io::Read::read_to_string(&mut stdout, &mut output).expect("read stdout");
    }
    assert!(status.success(), "task failed: {status}");
    assert_eq!(output.trim(), "args=[--port 4321]");
}
