---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-cli: patch
cargo/devlanes-tui: patch
cargo/devlanes-mcp-ui: patch
---

Changed `devlanes status` to report a running exec command, task or infra command as `running`, JSON included.

Scripts and MCP consumers that matched `ready` for exec units or `starting` for running tasks should match `running`. A task stopped by its own `timeout:` now reads `failed` with reason `timed_out` everywhere. `run` answers and `task` events use the same words as `status`: `exited`, `failed` or `stopped` (events said `done`, and a stopped task `failed`).
