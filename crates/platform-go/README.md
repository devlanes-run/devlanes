# devlanes-platform-go

Launch contributor for Go units. The binary registers it and `devlanes-resolve` applies it to units whose platform is `go`, set in the config or detected from a `go.mod` or `go.work`.

## Usage

```rust
use std::sync::Arc;
use devlanes_core::contrib::Contributor;
use devlanes_platform_go::GoContributor;

let go: Arc<dyn Contributor> = Arc::new(GoContributor);
// Pass in `ResolveInput::contributors`.
```

## Config example

```yaml
services:
  api:
    cwd: services/api                    # has go.mod: platform inferred
    run: go run ./cmd/api
  worker:
    platform: go
    run: ./bin/worker
```

Go units get `SSL_CERT_FILE`: the combined bundle of system roots plus the devlanes root. A different value from the shell is replaced and reported as a note.

Go reads `SSL_CERT_FILE` on Linux and the other Unix systems except macOS. macOS and Windows use the OS verifier and ignore it; `devlanes setup` adds the CA there.

## Behaviour

Without `platform`, Go claims a unit whose `run_script` source is `go.mod`, or whose working directory contains `go.mod` or `go.work`. Only the working directory itself is checked; parent directories are not searched. The claim sits below node's and rust's, so a directory that also has `package.json` or `Cargo.toml` stays with that platform. It sits above python's, so a directory with both `pyproject.toml` and `go.mod` is Go.

## License

Apache-2.0
