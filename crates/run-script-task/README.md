# devlanes-run-script-task

Runs `run_script: { file: Taskfile.yml, name: <task> }` with [Task](https://taskfile.dev) (go-task) in the Taskfile's directory. Lane resolution calls it through `TaskfileTasks`, a `devlanes-core` `ScriptRunner`.

## Usage

```rust
use std::path::Path;
use devlanes_run_script_task::{TaskRequest, plan};

// Taskfile.yml:  tasks:
//                  dev: vite {{.CLI_ARGS}}
let args = vec!["--port".to_owned(), "20012".to_owned()];
let launch = plan(&TaskRequest {
    worktree: Path::new("/lanes/acme-1"),
    source: Path::new("Taskfile.yml"),
    name: "dev",
    extra_args: &args,
})?;
assert_eq!(
    launch.argv,
    ["task", "--taskfile", "Taskfile.yml", "dev", "--", "--port", "20012"]
);
```

The composition root registers `Arc::new(TaskfileTasks)` as a script runner. `plan` reads the Taskfile, so the adapter runs it on a blocking thread.

## API

- `plan`: checks the task, then renders the command. It always passes `--taskfile`, because task picks by its own priority when a directory has several Taskfiles.
- `parse` and `Tasks::check`: the task check on its own, following aliases and `*` wildcards.
- `handles`: accepts the eight names task finds by itself, from `Taskfile.yml` to `taskfile.dist.yaml`.

## Behaviour

Extra arguments, such as preset flags, go after `--`, and task hands them to the task as `{{.CLI_ARGS}}`. A task that never mentions `CLI_ARGS` ignores them; `plan` adds a note to the launch instead of failing.

A task is reported missing only when the file has no `includes`; with any, the check is `Unverifiable` and task decides at run time. Internal tasks fail, since task refuses to run them from the command line.

## The `task` binary

devlanes runs `task`. Arch Linux installs go-task as `go-task`, because Taskwarrior owns `task` there. devlanes does not look for `go-task`; put a `task` symlink to it on `PATH`, since a shell alias does not reach processes devlanes starts.

## License

Apache-2.0
