---
cargo/devlanes: minor
cargo/devlanes-config: minor
cargo/devlanes-daemon: minor
---

Removed `updates.check`; use `updates.mode: disable | notify | auto` in the user config.

Replace `updates.check: false` with `updates.mode: disable`; a config that still sets `updates.check` fails to load until it is changed.
