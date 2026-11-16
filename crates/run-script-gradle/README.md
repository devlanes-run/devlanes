# devlanes-run-script-gradle

Runs `run_script: { file: build.gradle.kts, name: <arguments> }` as `gradle <arguments>`, or the project's Gradle wrapper, in that file's directory. It handles `build.gradle`, `build.gradle.kts`, `settings.gradle` and `settings.gradle.kts`. Lane resolution calls it through `GradleTasks`, a `devlanes-core` `ScriptRunner`.

## Usage

```rust
use std::path::Path;
use devlanes_run_script_gradle::{TasksRequest, plan};

let args = vec!["--no-daemon".to_owned()];
let launch = plan(&TasksRequest {
    worktree: Path::new("/lanes/acme-1"),
    source: Path::new("settings.gradle.kts"),
    name: ":web:bootRun --offline",
    extra_args: &args,
})?;
// Without a gradlew between the file and the worktree root:
assert_eq!(launch.argv, ["gradle", ":web:bootRun", "--offline", "--no-daemon"]);
```

The composition root registers `Arc::new(GradleTasks)` as a script runner. `plan` looks for the wrapper, so the adapter runs it on a blocking thread.

## API

- `plan`: splits the arguments and picks the wrapper or `gradle`.
- `split_arguments` and `tasks`: the shell-style split of `name` and its task tokens.
- `handles`: accepts the four Gradle build and settings file names.

## Config example

```yaml
services:
  api:
    run_script:
      file: build.gradle.kts
      name: quarkusDev
```

## Behaviour

`name` is split like a shell command line and must name at least one task. Arguments from contributors and presets come after it as Gradle arguments.

The program is the nearest `gradlew` (`gradlew.bat` on Windows) from the file's directory up to the worktree root, else `gradle` from `PATH`. Gradle finds the settings file itself by walking up from the working directory.

Task names are not checked, because listing them needs a Gradle configuration run. Gradle fails at once with "Task 'x' not found", and devlanes shows the unit as failed.

## License

Apache-2.0
