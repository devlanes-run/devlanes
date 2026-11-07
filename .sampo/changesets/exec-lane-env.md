---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-core: patch
cargo/devlanes-daemon: patch
cargo/devlanes-resolve: patch
---

Fixed `devlanes exec` commands missing `global_env` and CA variables such as `NODE_EXTRA_CA_CERTS`.

`devlanes env show` without a unit and the command `devlanes with` runs now include `global_env` and the profile's `env` too, except entries that read `self` or `replica`.
`devlanes exec` now refuses while one of those values does not evaluate (an `infra.*` value before infra is up, for example), or while the project file does not load and nothing in the lane runs.
