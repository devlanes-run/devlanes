---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-daemon: patch
cargo/devlanes-mcp: patch
---

Added `devlanes mcp`, an MCP server that lets AI agents run devlanes commands as tools.

Register it with your agent, for example `claude mcp add devlanes -- devlanes mcp` for Claude Code. `devlanes mcp --help` lists the command or config for Codex, OpenCode, Cursor, VS Code, Claude Desktop and Goose.

<!-- highlight
title: Use devlanes from your agent over MCP
body: Run `claude mcp add devlanes -- devlanes mcp` (or see `devlanes mcp --help` for other agents) and the agent starts dev servers with `exec`, reads logs and stops what it started, asking you before it deletes lane data.
-->
