---
cargo/devlanes: minor
cargo/devlanes-cli: patch
cargo/devlanes-daemon: minor
cargo/devlanes-state: minor
cargo/devlanes-transport: minor
cargo/devlanes-transport-ssh: minor
cargo/devlanes-proxy: minor
cargo/devlanes-pages: minor
---

Changed `devlanes remote connect` to reconnect boxes after a daemon restart until `devlanes remote disconnect`, and to wait for another `devlanes remote connect` instead of failing when a reconnect needs a password or new credentials.

While a box waits, `devlanes remote ls` and `devlanes status --json` show `needs_auth` with the command to run, and its hostnames show a sign-in page (DL205). The daemon upgrades its state database on the first start of this release, and a daemon of an earlier release cannot open it afterwards.
