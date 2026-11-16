---
title: devlanes mcp
description: Serve devlanes to an AI agent as an MCP server on stdin and stdout.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

The agent gets a tool per common command, such as start, exec, logs, stop and debug_why, whose arguments are this command's options, and a devlanes tool that takes any other command line. Each call runs this binary as a separate process with --output agent, so the server itself never connects to the daemon or starts one. Commands that need a terminal, such as ui, attach, env shell and setup, are refused, and so are follows without --timeout.

A call runs in the cwd it names, else in --path, else in $CLAUDE_PROJECT_DIR, else in the directory the agent started the server in. --lane adds that lane to calls that name no cwd.

A lane no shell started gets your login shell's environment on macOS and Linux; on Windows set DEVLANES_SHELL_ENV=pwsh (or powershell) in the server's env to read your PowerShell profile. https://devlanes.run/docs/how-to/agents/set-up/ has the details.

```text
devlanes mcp [OPTIONS]
```

- `--read-only`: Offer only the tools of commands that change nothing; the devlanes tool refuses the others.
- `--path <DIR>`: Run calls that name no cwd in this directory (the global --lane picks a lane instead).

```text
Register it with your agent:
  Claude Code     claude mcp add devlanes -- devlanes mcp
  Codex           codex mcp add devlanes -- devlanes mcp
  OpenCode        in opencode.json:
                  "mcp": {"servers": {"devlanes": {"type": "local", "command": ["devlanes", "mcp"]}}}
  Cursor          in ~/.cursor/mcp.json or .cursor/mcp.json:
                  "mcpServers": {"devlanes": {"type": "stdio", "command": "devlanes", "args": ["mcp"]}}
  VS Code         code --add-mcp '{"name":"devlanes","command":"devlanes","args":["mcp"]}'
  Claude Desktop  in claude_desktop_config.json, with the full path (the app does not read your shell's PATH):
                  "mcpServers": {"devlanes": {"command": "/path/to/devlanes", "args": ["mcp"]}}
  Goose           goose session --with-extension 'devlanes mcp'
                  or in config.yaml under extensions:
                  devlanes: {type: stdio, name: devlanes, cmd: devlanes, args: [mcp], enabled: true}

Examples:
  devlanes mcp --read-only             only commands that change nothing
  devlanes mcp --path /src/acme        calls without a cwd use this worktree's lane
```

Every command also takes the [global options](README.md#global-options).
