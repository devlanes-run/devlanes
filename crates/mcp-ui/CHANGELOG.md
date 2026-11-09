# devlanes-mcp-ui

## 0.3.0 — 2026-11-09

### Patch changes

- [4e8e5636](https://github.com/devlanes-run/devlanes/commit/4e8e56369ab3a49b4bbd993b7af345a10abcbcad) Changed `devlanes lane clean` so a lane that comes back starts empty, without the old `--env` or history.
  
  The primary and worktree lanes get a new data folder path, and `lane clean --dry-run` lists the run history. The daemon migrates its database on the next start and deletes the history of lanes cleaned before. `devlanes lane gc` deletes lane data folders a clean could not delete.
- [36cbe7b4](https://github.com/devlanes-run/devlanes/commit/36cbe7b497f773aac89902cb65a42ecfc8ec28fe) Changed `devlanes status` to report a running exec command, task or infra command as `running`, JSON included.
  
  Scripts and MCP consumers that matched `ready` for exec units or `starting` for running tasks should match `running`. A task stopped by its own `timeout:` now reads `failed` with reason `task_timeout` everywhere. `run` answers, `task` events and the last run in `run --list` use the same words as `status`: `exited`, `failed` or `stopped` (they said `done`, a stopped task `failed` in events and `interrupted` in the list).

## 0.2.0 — 2026-11-07

### Patch changes

- [5db5cde](https://github.com/devlanes-run/devlanes/commit/5db5cdec69bc61915e31237b81abf0456a061bfc) Added `devlanes mcp`, an MCP server that lets AI agents run devlanes commands as tools.
  
  Register it with your agent, such as `claude mcp add devlanes -- devlanes mcp` for Claude Code. See [coding agents](https://github.com/devlanes-run/devlanes/blob/main/docs/guides/mcp.md) for Codex, OpenCode, Cursor, VS Code, Claude Desktop and Goose.
  
  <!-- highlight
  title: Use devlanes from your agent over MCP
  body: Run `claude mcp add devlanes -- devlanes mcp` (or see `devlanes mcp --help` for other agents) and the agent starts dev servers with `exec`, reads logs and stops what it started, asking you before it deletes lane data.
  -->

