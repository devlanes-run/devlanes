---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-state: minor
cargo/devlanes-cli: patch
cargo/devlanes-tui: patch
cargo/devlanes-mcp-ui: patch
---

Changed `devlanes status` to report a running exec command, task or infra command as `running`, JSON included.

Scripts and MCP consumers that matched `ready` for exec units or `starting` for running tasks should match `running`. A task stopped by its own `timeout:` now reads `failed` with reason `task_timeout` everywhere. `run` answers, `task` events and the last run in `run --list` use the same words as `status`: `exited`, `failed` or `stopped` (they said `done`, a stopped task `failed` in events and `interrupted` in the list).
