# devlanes-mcp

## 0.3.0 — 2026-11-09

### Minor changes

- [174ed796](https://github.com/devlanes-run/devlanes/commit/174ed79624c5188c6c4a94924d050d9e31641234) Changed `devlanes env show` in agent output and the MCP `env_show` tool to hide secret-looking values.
  
  `devlanes mcp` no longer runs `env export`. In a shell, `env show --output plain` and `eval "$(devlanes env export)"` still print real values, and `exec` or `with` pass them to a command. A `--read-only` MCP server can no longer read secret values.

### Patch changes

- [5ed93a92](https://github.com/devlanes-run/devlanes/commit/5ed93a9209ff9afe7e2c283d346eb8e8bec49fc7) Fixed `devlanes mcp` reporting another lane's start as the progress of a start call.
  
  Calls of `with` and `lane new --start` now report the phases of the lane they start too.
- [4158a194](https://github.com/devlanes-run/devlanes/commit/4158a1943384987241920e909beed7bb111b584d) Fixed `devlanes lane new --start` and `daemon restart` through `devlanes mcp` starting lanes without the login shell's environment.
- [4158a194](https://github.com/devlanes-run/devlanes/commit/4158a1943384987241920e909beed7bb111b584d) Fixed `devlanes restart <unit>` through `devlanes mcp` keeping an `--env` value from an earlier command.

## 0.2.0 — 2026-11-07

### Patch changes

- [5db5cde](https://github.com/devlanes-run/devlanes/commit/5db5cdec69bc61915e31237b81abf0456a061bfc) Added `devlanes mcp`, an MCP server that lets AI agents run devlanes commands as tools.
  
  Register it with your agent, such as `claude mcp add devlanes -- devlanes mcp` for Claude Code. See [coding agents](https://github.com/devlanes-run/devlanes/blob/main/docs/guides/mcp.md) for Codex, OpenCode, Cursor, VS Code, Claude Desktop and Goose.
  
  <!-- highlight
  title: Use devlanes from your agent over MCP
  body: Run `claude mcp add devlanes -- devlanes mcp` (or see `devlanes mcp --help` for other agents) and the agent starts dev servers with `exec`, reads logs and stops what it started, asking you before it deletes lane data.
  -->
- [5c084ba](https://github.com/devlanes-run/devlanes/commit/5c084baa01ace68db168c7a85a8dc594b55b0dd3) Fixed `devlanes env show <unit>` failing on a lane that has not started yet.

