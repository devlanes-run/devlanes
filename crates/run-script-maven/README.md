# devlanes-run-script-maven

Runs `run_script: { file: pom.xml, name: <arguments> }` as `mvn <arguments>`, or the project's Maven wrapper, in the pom's directory. Lane resolution calls it through `MavenGoals`, a `devlanes-core` `ScriptRunner`.

## Usage

```rust
use std::path::Path;
use devlanes_run_script_maven::{GoalsRequest, plan};

let args = vec!["-B".to_owned(), "-ntp".to_owned()];
let launch = plan(&GoalsRequest {
    worktree: Path::new("/lanes/acme-1"),
    source: Path::new("pom.xml"),
    name: "-pl web -am spring-boot:run",
    extra_args: &args,
})?;
// Without a mvnw between the pom and the worktree root:
assert_eq!(launch.argv, ["mvn", "-pl", "web", "-am", "spring-boot:run", "-B", "-ntp"]);
```

The composition root registers `Arc::new(MavenGoals)` as a script runner. `plan` looks for the wrapper and reads poms, so the adapter runs it on a blocking thread.

## API

- `plan`: splits the arguments, picks the wrapper or `mvn` and checks plugin prefixes.
- `split_arguments` and `goals`: the shell-style split of `name` and its goal and phase tokens.
- `handles`: accepts `pom.xml`.

## Config example

```yaml
services:
  web:
    run_script:
      file: pom.xml
      name: spring-boot:run "-Dspring-boot.run.jvmArguments=-Xmx1g"
```

## Behaviour

`name` is split like a shell command line, so quotes group an argument with spaces and a backslash escapes the next character. It must name at least one goal or phase. Arguments from contributors and presets come after it as Maven arguments.

The program is the nearest `mvnw` (`mvnw.cmd` on Windows) from the pom's directory up to the worktree root, else `mvn` from `PATH`. `-pl` and `-am` are never added: `-am` runs the goal in upstream modules too, which fails unless the parent pom skips the plugin there.

For a `prefix:goal` token, the pom and its parents reachable through `relativePath` inside the worktree are searched for a `<prefix>-maven-plugin` or `maven-<prefix>-plugin`. A miss is a note, since parents outside the worktree and `settings.xml` plugin groups are not read. Arguments with `-f` or `--file` skip the check, because Maven builds that other pom. A pom that does not parse is a note too; Maven reports the error itself.

## License

Apache-2.0
