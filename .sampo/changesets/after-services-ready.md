---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Fixed `after: services` tasks running when `devlanes start` timed out.

`devlanes start` prints `services ready` once every service it started is ready, and `--json` gets a `lane.phase` event with `state: ready` before `services` `done`, which now comes after the wait. A `devlanes stop` of the lane during that wait ends the start with `start_cancelled`.
