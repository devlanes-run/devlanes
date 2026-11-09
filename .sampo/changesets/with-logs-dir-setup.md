---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Fixed `devlanes with --logs-dir` leaving out the logs of `infra.setup` entries and failed tasks the start ran.

`ran` in the `start --json` result now also lists the tasks and setup entries that failed.
