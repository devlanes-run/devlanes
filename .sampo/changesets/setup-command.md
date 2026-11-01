---
cargo/devlanes: minor
cargo/devlanes-cli: minor
cargo/devlanes-daemon: minor
cargo/devlanes-config: patch
cargo/devlanes-resolver-hosts: patch
cargo/devlanes-resolver-nrpt: patch
cargo/devlanes-trust-linux: patch
cargo/devlanes-trust-windows: patch
cargo/devlanes-tui: patch
---

Removed `devlanes certs trust` and `certs untrust`; use `devlanes setup`, which works out what this machine needs and asks once.

Run `devlanes setup` instead of `certs trust --hosts --resolver`, and `devlanes setup --remove` instead of `certs untrust --hosts --resolver`.
The TUI offers to run it when HTTPS or hostnames need it.
