---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-resolve: minor
cargo/devlanes-config: patch
cargo/devlanes-cli: patch
---

Changed `devlanes exec` and `env show` to fail on a misspelled `infra.<name>`, which `config check` now reports too.

`exec` and `env show` leave out a value only while its container is stopped or Docker is not running, and say which. They refuse when Docker does not answer in time, and with compose files on a machine without the Docker CLI; pass the value with `--env KEY=...` or start Docker. `env export` and `env shell` now include `NODE_EXTRA_CA_CERTS`, `DENO_CERT`, `SSL_CERT_FILE` and `REQUESTS_CA_BUNDLE` unless the shell sets them.
