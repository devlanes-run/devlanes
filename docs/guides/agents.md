# Coding agents

Agents use devlanes through its CLI, like a person in a terminal. When devlanes detects an agent it prints short output meant for one, and the devlanes skill teaches the agent to run dev servers and watchers through devlanes instead of in the background of its shell. `devlanes mcp` is an optional MCP server that offers the same commands as tools, for hosts without a shell, sandboxes whose shell cannot reach the daemon, and hosts that show MCP Apps.

## The skill and agent output

```sh
npx skills add devlanes-run/devlanes --skill devlanes
```

The [skill](../../skills/devlanes/SKILL.md) covers starting and checking a lane, `exec` for long-running processes, tests with `devlanes with`, reading output and errors, and writing or migrating a `devlanes.yaml`.

devlanes detects an agent from the variables it sets in its shell and switches to agent output: terse lines, failures first with the end of their log and the command that shows more, and at most 16 KiB per command (`--budget` changes that). Agent output never prompts. A command that needs confirmation exits 2 and names the flag, usually `--yes`. `--json` prints the full data instead, `--output agent` forces agent output, and `DEVLANES_AGENT=0` turns detection off.

## Registering devlanes mcp

`devlanes mcp` serves MCP on stdin and stdout. devlanes never edits a host's config, so register it yourself. `devlanes mcp --help` lists the same commands.

Claude Code, for the current project, or with `--scope user` for every project:

```sh
claude mcp add devlanes -- devlanes mcp
```

Codex:

```sh
codex mcp add devlanes -- devlanes mcp
```

OpenCode, in the project's `opencode.json` or in `~/.config/opencode/opencode.json`:

```json
{
  "mcp": {
    "devlanes": {"type": "local", "command": ["devlanes", "mcp"]}
  }
}
```

Cursor, in `~/.cursor/mcp.json` or the project's `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "devlanes": {"command": "devlanes", "args": ["mcp"]}
  }
}
```

VS Code, for GitHub Copilot's agent mode:

```sh
code --add-mcp '{"name":"devlanes","command":"devlanes","args":["mcp"]}'
```

or for one workspace, in `.vscode/mcp.json`:

```json
{
  "servers": {
    "devlanes": {"type": "stdio", "command": "devlanes", "args": ["mcp"]}
  }
}
```

Claude Desktop, in `claude_desktop_config.json` (Settings, Developer, Edit Config). The app does not read your shell's `PATH`, so give the full path that `command -v devlanes` prints, or `(Get-Command devlanes).Source` in PowerShell:

```json
{
  "mcpServers": {
    "devlanes": {"command": "/opt/homebrew/bin/devlanes", "args": ["mcp"]}
  }
}
```

On Windows the path needs doubled backslashes, such as `"C:\\Users\\acme\\AppData\\Local\\Programs\\devlanes\\devlanes.exe"`.

Goose, for one session:

```sh
goose session --with-extension 'devlanes mcp'
```

or for every session, in Goose's `config.yaml` (`~/.config/goose/config.yaml` on macOS and Linux):

```yaml
extensions:
  devlanes:
    type: stdio
    name: devlanes
    cmd: devlanes
    args: [mcp]
    enabled: true
```

### Options

Flags go after `mcp`, as in `claude mcp add devlanes -- devlanes mcp --read-only` or `"args": ["mcp", "--read-only"]`.

- `--read-only` offers only the `devlanes_read` tool, so the agent can read status, logs and errors and cannot start, stop or change anything.
- `--path DIR` runs calls that name no `cwd` in DIR, which picks that worktree's lane. No shell reads a JSON config, so `~` does not expand there; give an absolute path.
- `--lane REF` adds `--lane REF` to calls that name neither a `cwd` nor a lane. With `--path` it does nothing.

A call runs in the `cwd` it names (relative to the server's directory), else in `--path`, else in `$CLAUDE_PROJECT_DIR` when the host sets it, else in the directory the host started the server in. Claude Desktop has no project directory, so give it `--path` or `--lane REF` with a `project@` reference, or let the agent pass a `cwd`.

## Tools

`devlanes` runs any command, and `devlanes_read` only commands that change nothing. Both take `{"args": [...], "cwd": "..."}`, where `args` is the command line after `devlanes`, with the same arguments the CLI takes: `["start"]`, `["logs", "web", "--tail", "100"]`, `["stop", "--mine"]`. The server checks the line with the CLI's parser and runs the devlanes binary with `--output agent`. The result's text is the command's agent output; `structuredContent` holds the JSON that `--json` would print, and a nonzero exit marks the result as an error. `--help` works in both tools.

`devlanes_read` accepts `status`, `logs`, `wait`, `doctor`, `run --list`, `lane ls`, `lane clean --dry-run`, the `debug` commands except `freeze`, `thaw` and `bundle`, `env show`, `env export`, `url`, `infra status`, `infra logs`, `config check`, `config get`, `remote ls`, `remote sync`, `daemon status`, `daemon logs` and `daemon clients`. For anything else it tells the agent to call `devlanes` with the same args. `env show` and `env export` print values as the CLI does, so a read-only agent can still read a lane's environment.

Both tools refuse commands that need a terminal or never end, and say what to run instead: `ui`, `attach`, `start --attach`, `env shell`, `setup`, `update`, `uninstall`, `completions`, `status --statusline`, `daemon run`, `daemon logs -f`, and `logs -f`, `status --watch`, `debug events -f` or `infra logs -f` without `--timeout`.

`start`, `restart`, `reload`, `wait`, `with` and `lane resume` without `--timeout` get one a little under the time the host waits for a tool call:

| Host | Timeout |
|---|---|
| Claude Code | 170 s |
| Codex | 50 s |
| OpenCode and other hosts | 25 s |

Codex waits 60 s by default (`tool_timeout_sec` in [its MCP docs](https://developers.openai.com/codex/extend/mcp)). If you raise `tool_timeout_sec` or the host's own limit, the agent can pass a longer `--timeout` itself.

When it passes, the call returns what is ready so far and the units keep starting; `devlanes wait` continues from there. Hosts that send a progress token get the lane's phases, such as `acme@1 infra started`, as progress notifications. Cancelling a call is like Ctrl-C in a terminal: the command detaches and services keep running.

## Long-running processes

Dev servers, watchers, workers, tunnels and other processes that belong to the lane's work and should keep running after the command returns go through `devlanes exec` instead of `&`, `nohup` or a background shell:

```sh
devlanes exec --name storybook -- pnpm storybook
```

Through MCP that is `["exec", "--name", "storybook", "--", "pnpm", "storybook"]`. The daemon owns the process and its logs and gives it the lane's environment, so it survives the agent's shell, and `devlanes logs storybook --tail 100`, `devlanes status` and `devlanes stop storybook` find it by name. The result says once how to read and stop it. Builds, tests and other commands that exit run as usual, or with `devlanes with -- COMMAND` when they need the lane running. The skill and the MCP server's instructions teach the agent the same rules.

## Consent prompts

Through MCP, these commands ask the user before they run:

- `lane clean`, with what the clean would remove (`lane clean --dry-run`), and `lane gc`
- `daemon pair`, `daemon stop`, `daemon restart`, `daemon reset-state` and `daemon revoke`
- `remote connect` and `remote disconnect`
- `config set` and `config unset` of a `remote.*` key

Any other command that needs confirmation asks too, such as `run` of a task with `confirm:`. When the user accepts, the server runs the command with `--yes`; when they decline, nothing runs. A host without MCP elicitation cannot ask, so there the call fails with the question and `ask the user, then rerun with --yes`, as the CLI does for an agent in a shell.

The prompts help the user catch an agent's mistakes and are not a security boundary: an agent that passes `--yes` is not asked, and any process running as you can reach the daemon without `devlanes mcp` ([processes running as you](../security.md#processes-running-as-you)).

## Environment

Units get the environment of the shell that started their lane ([environment](ports-and-env.md#environment)). An MCP call first sends no environment, so a lane started from a shell keeps that shell's environment for `start`, `restart`, `exec` and the other commands that start processes. When the lane has none, because no shell started it since the daemon started, the call runs again with the environment the host gave the MCP server, and the result ends with:

```text
note: the lane had no environment from a shell, so it got the environment of the devlanes mcp server
```

The lane keeps that environment for later calls. It is often thinner than a shell's: apps opened from the desktop, such as Claude Desktop, get a short `PATH`, and Codex passes MCP servers only a few variables, so services can fail with `command not found on PATH`. Run `devlanes restart` in the worktree from a terminal once; the lane then keeps that terminal's environment, and MCP calls use it until the daemon stops ([bringing a lane back](lifecycle.md#bringing-a-lane-back)).

## Who started what

Each command tells the daemon who runs it, a person or an agent with its session, and the daemon records that on the units it starts (`actor` in `status --json`). `devlanes stop --mine` stops only what the current agent session started, and `status --mine` and `logs --mine` show only those units. MCP calls detect the agent from the server's environment the same way, so when a host gives its MCP servers the variables it gives its shell, `--mine` covers what the session started through either path. When the environment names no agent, the host's client name does (`claude-code`, `codex`, `cursor`, `goose` and others).

Codex gives MCP servers no thread id, so units its MCP calls start carry no session. `stop --mine` from a Codex shell leaves them alone, and through MCP it matches what any Codex session started through MCP. The actor is a label the client sets; the daemon never checks it.

## The daemon

`devlanes mcp` never connects to the daemon or starts one by itself, so a host with it registered costs one idle process. Each call runs one devlanes command, which starts the daemon when the CLI would (`doctor` and `daemon status` never do) and closes its connection when it ends, so `daemon.exit_when_idle` still stops a daemon nothing uses.

## Codex

On Windows a shell inside the Codex sandbox cannot use devlanes. Tested with codex-cli 0.166.0 on Windows 11: with the daemon running, sandboxed `devlanes daemon status` fails with `client I/O: Access is denied` and `devlanes status` with `auto-spawn refused in sandbox`; without a daemon, `devlanes daemon start` fails with `Access is denied` too. `devlanes daemon install` does not help there. Codex starts MCP servers outside its sandbox, so on Windows register `devlanes mcp` and let the agent use its tools.

The macOS and Linux sandboxes were not tested. When shell commands there fail with `auto-spawn refused in sandbox`, run `devlanes daemon install` outside the sandbox, which starts the daemon now and at login, or use `devlanes mcp`.

## Claude Code status line

`devlanes status --statusline` prints one line for the lane of Claude Code's directory, such as `acme@1 3/3 ready · web acme1.localhost`, or `acme@1 ✕ api failed · 2/3` when a unit failed. Add it to `~/.claude/settings.json`, or a project's `.claude/settings.json`:

```json
{
  "statusLine": {"type": "command", "command": "devlanes status --statusline"}
}
```

It reads the directory and session id from the JSON Claude Code writes to its stdin and lists the session's own units first among failures and URLs. Without that JSON it uses the current directory. It never starts the daemon and prints nothing when no daemon runs or the directory has no lane.

## MCP Apps

Hosts that show MCP Apps, such as Claude Desktop, GitHub Copilot in VS Code, Cursor and Goose, show a lane card with results such as `start` and `status`. Other hosts, terminal agents among them, get the same text without the card.
