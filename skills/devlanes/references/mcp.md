# The devlanes MCP server

`devlanes mcp` serves devlanes over MCP on standard input and output: a tool per common command and a `devlanes` tool for the rest. Read this to decide whether a host needs it and to register it.

## When to use it

| Situation | Way in |
|---|---|
| Your shell runs `devlanes` and `devlanes doctor` prints `ok sandbox: not sandboxed` | The CLI in your shell. The MCP server adds nothing. |
| The host has no shell | `devlanes mcp` |
| The host's sandbox keeps the shell from the daemon (`auto-spawn refused in sandbox`, exit 4), as Codex on Windows does | `devlanes mcp`, when the host starts MCP servers outside its sandbox, as Codex does. A server inside the sandbox fails the same way the shell does. |
| The host shows MCP Apps and the user wants result cards | `devlanes mcp` |

devlanes never edits a host's config. Registering the server changes the user's agent setup, so do it when the user asks, or ask first.

## Register it

| Host | Registration |
|---|---|
| Claude Code | `claude mcp add devlanes -- devlanes mcp` for the current project; `claude mcp add --scope user devlanes -- devlanes mcp` for every project. |
| Codex | `codex mcp add devlanes -- devlanes mcp` |
| OpenCode | `opencode mcp add devlanes -- devlanes mcp` for the current project; add `--global` for every project. By hand, in `opencode.json`: `{"mcp": {"servers": {"devlanes": {"type": "local", "command": ["devlanes", "mcp"]}}}}` |
| Cursor | In the project's `.cursor/mcp.json` or `~/.cursor/mcp.json`: `{"mcpServers": {"devlanes": {"type": "stdio", "command": "devlanes", "args": ["mcp"]}}}` |
| VS Code (GitHub Copilot agent mode) | `code --add-mcp '{"name":"devlanes","command":"devlanes","args":["mcp"]}'`, or in `.vscode/mcp.json`: `{"servers": {"devlanes": {"type": "stdio", "command": "devlanes", "args": ["mcp"]}}}` |
| Claude Desktop | In `claude_desktop_config.json`, with the full path of the binary, since the app does not read the shell's `PATH`: `{"mcpServers": {"devlanes": {"command": "/path/to/devlanes", "args": ["mcp"]}}}`. `command -v devlanes`, or `(Get-Command devlanes).Source` in PowerShell, prints the path; in JSON on Windows, double each backslash. |
| Goose | `goose session --with-extension 'devlanes mcp'` for one session, or under `extensions` in Goose's `config.yaml`: `devlanes: {type: stdio, name: devlanes, cmd: devlanes, args: [mcp], enabled: true}` |

The host starts the server; it connects to nothing until the agent calls a tool. Each call runs one devlanes command with `--output agent` in the call's directory, and that command starts the daemon when none runs, as in a shell.

## Options

Options go after `mcp`, as in `claude mcp add devlanes -- devlanes mcp --read-only` or `"args": ["mcp", "--read-only"]`.

| Option | Effect |
|---|---|
| `--read-only` | Offers only the tools of commands that change nothing; the `devlanes` tool refuses the others and tells the agent to ask the user. |
| `--path DIR` | Calls that name no `cwd` run in DIR. Give an absolute path; nothing expands `~` in a JSON config. Hosts with no project directory, such as Claude Desktop, need it. |
| `--lane LANE` | Calls that name no `cwd` and no lane get `--lane LANE`. No effect together with `--path`. |

A call runs in its `cwd` argument (relative to where the server started), else in `--path`, else in `$CLAUDE_PROJECT_DIR` when the host sets it, else where the host started the server. The directory picks the project and the lane, as in a shell. A folder with several projects makes commands that need one fail with `project_ambiguous`; pass a `cwd` inside one, or a `project@lane` reference.

On Windows, a lane that no shell started gets the server's environment, without what a PowerShell profile adds. Set `"env": {"DEVLANES_SHELL_ENV": "pwsh"}` (or `powershell` for Windows PowerShell 5.1) in the server's config to read the profile. On macOS and Linux the server reads the user's login shell for such a lane.

## Tools in short

- `status`, `start`, `exec`, `logs`, `stop`, `restart`, `reload`, `wait`, `with`, `run`, `debug_why`, `debug_errors`, `debug_top`, `url_print`, `url_ls`, `env_show`, `lane_ls`, `lane_clean`, `daemon_pair` and `doctor` run the command they are named after. Their arguments are the command's arguments and options, with `_` for `-`: `devlanes logs web --tail 100` is `logs` with `{"units": ["web"], "tail": 100}`, and `devlanes exec --name storybook -- pnpm storybook` is `exec` with `{"name": "storybook", "command": ["pnpm", "storybook"]}`.
- `devlanes` runs any other command line: `{"args": ["infra", "status"]}`.
- Every tool takes `cwd`. `lane`, `timeout`, `env` (a list of `KEY=VALUE`) and `yes` appear where the command uses them.
- The result's text is the command's agent output; `structuredContent.data` holds what `--json` prints, and an error as `{"error": {"code", "exit_code", "data"}}`. A non-zero exit marks the result as an error.
- Refused, with what to run instead: `ui`, `attach`, `start --attach`, `env shell`, `env export`, `setup`, `update`, `uninstall`, `completions`, `status --statusline`, `daemon run`, `daemon logs -f`, and `logs -f`, `status --watch`, `debug events -f` or `infra logs -f` without `--timeout`. Ask the user to run those in a terminal.
- A long call (`start`, `restart`, `reload`, `wait`, `with`, `lane resume`, `lane new --start`) without `timeout` gets one that fits the host: 170 s for Claude Code, 50 s for Codex, 25 s for OpenCode, 55 s for others. At the timeout it returns what is ready so far and the units keep starting; call `wait` to continue.
- `lane clean`, `lane gc`, `daemon pair`, `daemon stop`, `daemon restart`, `daemon reset-state`, `daemon revoke`, `remote connect`, `remote disconnect`, a `config set` or `config unset` of a `remote.*` key, and any command that needs confirmation ask the user through the host first. A host that cannot ask returns the question: ask the user, then call again with `yes: true` (`--yes` in the `devlanes` tool's `args`).
- Cancelling a call works like Ctrl-C: the command is interrupted and units the daemon runs keep running.

[MCP tools](https://devlanes.run/docs/reference/mcp-tools.md) lists every argument, result field, timeout and consent question. [Set up a coding agent](https://devlanes.run/docs/how-to/agents/set-up.md) is the page for the user.
