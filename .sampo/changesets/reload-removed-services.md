---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-daemon: patch
---

Fixed `devlanes reload` not starting services that were removed from the project file and added back.

`reload` no longer restarts the whole lane for an `infra.external`, `infra.hosts` or `infra.hidden` edit. `devlanes status` drops removed services once they stop, says a unit `changed since it started` instead of naming the project file, and no longer keeps a stale `config changed` note on a lane where nothing runs.
