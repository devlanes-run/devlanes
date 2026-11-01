---
cargo/devlanes: minor
cargo/devlanes-cli: minor
cargo/devlanes-daemon: minor
cargo/devlanes-config: minor
cargo/devlanes-resolver-hosts: minor
cargo/devlanes-resolver-nrpt: minor
cargo/devlanes-trust-linux: minor
cargo/devlanes-trust-windows: minor
---

Replaced `devlanes certs trust` and `certs untrust` with `devlanes setup`, which works out what this machine needs and asks once.

Run `devlanes setup` instead of `certs trust --hosts --resolver`, and `devlanes setup --remove` instead of `certs untrust --hosts --resolver`.
