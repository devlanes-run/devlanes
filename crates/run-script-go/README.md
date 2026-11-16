# devlanes-run-script-go

Runs `run_script: { file: go.mod, name: <package> }` as `go run <package> <args>` in the module's directory. Lane resolution calls it through `GoPackages`, a `devlanes-core` `ScriptRunner`.

## Usage

```rust
use std::path::Path;
use devlanes_run_script_go::{PackageRequest, plan};

let args = vec!["--port".to_owned(), "20010".to_owned()];
let launch = plan(&PackageRequest {
    worktree: Path::new("/lanes/acme-1"),
    source: Path::new("go.mod"),
    name: "api",
    extra_args: &args,
})?;
assert_eq!(launch.argv, ["go", "run", "./cmd/api", "--port", "20010"]);
```

The composition root registers `Arc::new(GoPackages)` as a script runner. `plan` lists the package directory and reads its Go files, so the adapter runs it on a blocking thread.

## API

- `plan`: checks that the name is a main package of the module and renders the command.
- `handles`: accepts `go.mod`.

## Config example

```yaml
services:
  api:
    # Runs ./cmd/api with go run.
    run_script:
      file: go.mod
      name: api
```

A bare name is a directory under `cmd/`. `.` or a name with a `/` (`cmd/api`, `./tools/seed`) is a path relative to `go.mod`. Paths that leave the module, `...` patterns and `@version` suffixes are rejected.

## Behaviour

A package is a main package when one of its `.go` files, other than `_test.go` files, declares `package main`. Build constraints are not evaluated. Arguments follow the package without `--`, because `go run` hands everything after the package to the program. `go.work` is not a `run_script` file: `go run` finds it by walking up from the module's directory.

`go run` exits with status 1 when the program fails, so the unit's exit code is not the program's own. The first `go run` in a lane compiles the program and can be slow; raise the service's ready timeout if it fails before the build finishes.

## License

Apache-2.0
