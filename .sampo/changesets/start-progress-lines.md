---
cargo/devlanes: patch
cargo/devlanes-cli: patch
---

Fixed `devlanes start` calling a start that hit `--timeout` failed while its services were still starting.

`start` now also prints `services ready` before the results of `after: services` tasks, and in a terminal `start` and `with` no longer leave `… tasks: <task>` lines above finished tasks.
