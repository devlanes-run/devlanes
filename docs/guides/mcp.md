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

- `--read-only` offers only the tools that change nothing, so the agent can read status, logs and errors and cannot start, stop or change anything.
- `--path DIR` runs calls that name no `cwd` in DIR, which picks that worktree's lane. No shell reads a JSON config, so `~` does not expand there; give an absolute path.
- `--lane REF` adds `--lane REF` to calls that name neither a `cwd` nor a lane. With `--path` it does nothing.

A call runs in the `cwd` it names (relative to the server's directory), else in `--path`, else in `$CLAUDE_PROJECT_DIR` when the host sets it, else in the directory the host started the server in. Claude Desktop has no project directory, so give it `--path` or `--lane REF` with a `project@` reference, or let the agent pass a `cwd`. When that directory holds several projects, such as the root of a monorepo with a `devlanes.yaml` per app, calls that need one project fail with `project_ambiguous` and `status` lists them all; a `cwd` inside one project or a `project@` lane reference picks it.

## Tools

Each common command has a tool of its own, named after the command with `_` between words: `status`, `start`, `exec`, `logs`, `stop`, `restart`, `reload`, `wait`, `with`, `run`, `debug_why`, `debug_errors`, `debug_top`, `url_print`, `url_ls`, `env_show`, `lane_ls`, `lane_clean`, `daemon_pair` and `doctor`. The server builds them from the CLI's own definition: a tool's description is the command's one-line help plus when to pick that tool, and its arguments are the command's arguments and options with their help, with `_` in place of `-`. So `devlanes logs web --tail 100` is the `logs` tool with `{"units": ["web"], "tail": 100}`, and `devlanes exec --name web -- pnpm dev` is `exec` with `{"name": "web", "command": ["pnpm", "dev"]}`.

Every tool also takes `cwd`, and every tool whose command uses the global `--lane` takes `lane`; `status` and `lane_clean` have a `lane` of their own, and `debug_top`, `url_ls`, `lane_ls`, `daemon_pair` and `doctor` take none. `start`, `restart`, `reload`, `wait`, `with` and `logs` take `timeout`, and `start`, `restart`, `exec` and `run` take `env`, a list of `KEY=VALUE` items. The tools leave out what only matters in a terminal or to the server: the output modes, `--quiet`, `--verbose`, `--budget`, `start --attach`, `--no-ui`, `status --watch` and `status --statusline`. `--project` and `--file` are left out too, since a `project@` lane reference or a `cwd` does the same.

The `devlanes` tool runs every other command, such as `infra status`, `debug config`, `config get` or `lane new`. It takes `{"args": [...], "cwd": "..."}`, where `args` is the command line after `devlanes`, with the same arguments the CLI takes: `["infra", "status"]`, `["lane", "new", "--start"]`. `--help` works there too. Its description tells the agent to use the other tools first.

Each call becomes a devlanes command line, which the server checks with the CLI's parser and runs with the devlanes binary and `--output agent`. The result's text is the command's agent output; `structuredContent.data` holds the JSON that `--json` would print, and a nonzero exit marks the result as an error. Tools of commands that change nothing carry `readOnlyHint`, the others `destructiveHint`, by the same rule `--read-only` uses.

With `--read-only` the server offers only `status`, `logs`, `wait`, `debug_why`, `debug_errors`, `debug_top`, `url_print`, `url_ls`, `env_show`, `lane_ls`, `doctor` and the `devlanes` tool, which then runs only commands that change nothing: besides those with a tool, `run --list`, `lane clean --dry-run`, the `debug` commands except `freeze`, `thaw` and `bundle`, `url open`, `infra status`, `infra logs`, `config check`, `config get`, `remote ls`, `remote sync`, `daemon status`, `daemon logs` and `daemon clients`. For anything else it tells the agent to ask the user. `env show` hides secret-looking values, as it does in agent output, so a read-only server cannot read a secret.

Every tool refuses commands that need a terminal or never end, and says what to run instead: `ui`, `attach`, `start --attach`, `env shell`, `env export` (its lines are for `eval` in a shell), `setup`, `update`, `uninstall`, `completions`, `status --statusline`, `daemon run`, `daemon logs -f`, and `logs -f`, `status --watch`, `debug events -f` or `infra logs -f` without `--timeout`.

Hosts that send a progress token get the phases of the lane a call starts (`start`, `restart`, `reload`, `with`, `lane resume`, `lane new --start`), such as `acme@1 infra started`, as progress notifications. Cancelling a call is like Ctrl-C in a terminal: the command detaches and services keep running. On Windows cancelling also ends every process the command started, such as the program `devlanes with` ran. When the host closes the server's stdin or stops it with SIGTERM, or on Windows closes its console, the server ends its running commands the same way right away.

## Tool call timeouts

Most calls answer in milliseconds. `start`, `restart`, `reload`, `wait`, `with` and `lane resume` wait for the project instead: containers turning healthy, setup steps, first compiles, a test run. Every host also gives up on a tool call after a while, and when the host gives up first the agent gets a bare timeout with no devlanes output, so it cannot tell what came up or what failed.

When one of those commands names no `--timeout`, the server adds one that fits the host, chosen from the name the host sends when it connects:

| Host | Host's limit | `--timeout` added |
|---|---|---|
| Claude Code | about 28 hours, or `MCP_TOOL_TIMEOUT` | 170 s, or a little under `MCP_TOOL_TIMEOUT` |
| Codex | 60 s (`tool_timeout_sec`) | 50 s |
| OpenCode | 30 s (`timeout`) | 25 s |
| any other host | not known, often 60 s | 55 s |

When the timeout passes, the call returns what is ready so far, such as `2/3 ready, web starting`, and the units keep starting in the daemon; the agent calls the `wait` tool to continue. A timeout that is too short costs an extra `wait` call, and one longer than the host's limit brings back the bare timeout, so the values lean short.

Claude Code reads `MCP_TOOL_TIMEOUT`, in milliseconds, for every MCP tool call ([Claude Code's time limits on MCP tool calls](https://code.claude.com/docs/en/mcp#time-limits-on-mcp-tool-calls)), and its MCP servers inherit Claude Code's environment, so `devlanes mcp` sees the same value and stays a tenth under it (between 1 s and 10 s under). A per-server `timeout` in `.mcp.json` is not visible to the server; set `MCP_TOOL_TIMEOUT` instead.

Codex documents a 60 s default for `tool_timeout_sec` ([Codex MCP docs](https://developers.openai.com/codex/extend/mcp)), and OpenCode's per-server `timeout` in `opencode.json` sets its limit. Raising either leaves the 50 s or 25 s the server adds as they are. A `--timeout` the agent passes is never changed, so it can always ask for longer.

## Long-running processes

Dev servers, watchers, workers, tunnels and other processes that belong to the lane's work and should keep running after the command returns go through `devlanes exec` instead of `&`, `nohup` or a background shell:

```sh
devlanes exec --name storybook -- pnpm storybook
```

Through MCP that is the `exec` tool with `{"name": "storybook", "command": ["pnpm", "storybook"]}`. The daemon owns the process and its logs and gives it the lane's environment, so it survives the agent's shell, and `devlanes logs storybook --tail 100`, `devlanes status` and `devlanes stop storybook` find it by name. The result says once how to read and stop it. Builds, tests and other commands that exit run as usual, or with `devlanes with -- COMMAND` when they need the lane running. The skill and the MCP server's instructions teach the agent the same rules.

## Consent prompts

Through MCP, these commands ask the user before they run:

- `lane clean`, with what the clean would remove (`lane clean --dry-run`), and `lane gc`
- `daemon pair`, `daemon stop`, `daemon restart`, `daemon reset-state` and `daemon revoke`
- `remote connect` and `remote disconnect`
- `config set` and `config unset` of a `remote.*` key

Any other command that needs confirmation asks too, such as `run` of a task with `confirm:`. When the user accepts, the server runs the command with `--yes`; when they decline, nothing runs. A host without MCP elicitation cannot ask, so there the call fails with the question and tells the agent to ask the user, then call the tool again with `yes: true` (`--yes` through the `devlanes` tool), as the CLI does for an agent in a shell. `lane_clean`, `daemon_pair` and `run` take `yes` for that.

The prompts help the user catch an agent's mistakes and are not a security boundary: an agent that passes `yes: true` or `--yes` is not asked, and any process running as you can reach the daemon without `devlanes mcp` ([processes running as you](../security.md#processes-running-as-you)).

## Environment

Units get the environment of the shell that started their lane ([environment](ports-and-env.md#environment)). An MCP call first sends no environment, so a lane started from a shell keeps that shell's environment for `start`, `restart`, `exec` and the other commands that start processes. When the lane has none, because no shell started it since the daemon started, the call runs again with the environment described below, and the lane keeps it for later calls. `lane new --start` and `daemon restart` start lanes that have no environment of their own, so they run once, with that environment from the start.

The environment the host gives the MCP server is often thinner than a shell's: apps opened from the Dock or the Start menu, such as Claude Desktop, get a `PATH` without what your shell profile adds (nvm, fnm or mise directories, `export` lines), and Codex passes MCP servers only a few variables. So on macOS and Linux the server first reads the environment of your login shell, the way VS Code, JetBrains IDEs and Zed do when opened from the desktop: it runs `$SHELL` as an interactive login shell in your home directory, which runs your profile and then has the devlanes binary print the environment. What the profile itself prints never reaches a result. The server does this once per process, on the first call that needs it, and keeps the environment or the failure in memory only. A failure stays until the host restarts the server. When reading worked, the result says so:

```text
note: the lane had no environment from a shell, so devlanes mcp read it from your login shell (zsh)
```

The server skips this when `SHLVL` is set and above 0 in its own environment, which means a terminal started the host and its environment already went through your profile. Two variables in the host's MCP config change this:

- On macOS and Linux, `DEVLANES_SHELL_ENV=0` never reads the login shell and `DEVLANES_SHELL_ENV=1` reads it even when `SHLVL` is set.
- `DEVLANES_SHELL_ENV_TIMEOUT` is how many seconds the shell may take, 10 by default, from 1 to 120. A shell that takes longer is stopped.

When the login shell cannot be read, the call uses the server's own environment and the result ends with:

```text
note: the lane had no environment from a shell, so it got the environment of the devlanes mcp server
```

If a service then fails with `command not found on PATH`, the result adds a line with the fix: why the login shell failed, or on Windows without `DEVLANES_SHELL_ENV`, how to set it. Running `devlanes restart` in the worktree from a terminal once always works: the lane then keeps that terminal's environment, and MCP calls use it until the daemon stops ([bringing a lane back](lifecycle.md#bringing-a-lane-back)).

On Windows the server does not read a profile unless you ask, since apps started from the Start menu already get your user and system `PATH`. Tools that add to `PATH` only from a PowerShell profile, such as fnm with `fnm env --use-on-cd | Out-String | Invoke-Expression`, are missing then. Set `DEVLANES_SHELL_ENV` to `pwsh` (PowerShell 7) or `powershell` (Windows PowerShell 5.1) in the MCP server's `env`, and the server runs it with your profile, under your execution policy:

```json
{
  "mcpServers": {
    "devlanes": {
      "command": "C:\\Users\\acme\\AppData\\Local\\Programs\\devlanes\\devlanes.exe",
      "args": ["mcp"],
      "env": {"DEVLANES_SHELL_ENV": "pwsh"}
    }
  }
}
```

The lane gets the profile's environment except `PSModulePath`, which stays the server's so that a unit running the other PowerShell still finds its own modules.

For fnm there is also a fixed directory to add to your user `PATH` instead: `<FNM_DIR>\aliases\default` (by default `%APPDATA%\fnm\aliases\default`) always points at the default Node version, so every app finds `node` and the tools installed with it.

## Actors and --mine

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

Hosts that show MCP Apps, such as Claude Desktop, GitHub Copilot in VS Code, Cursor and Goose, show a card with results such as `start`, `status`, `logs UNIT`, `debug top`, `lane clean` and `daemon pair`; logs read with `--grep`, `--since` or another filter get none. Its buttons run commands themselves without a consent prompt, since the click is the consent: the clean button runs `lane clean --yes`, and new code on a pairing card runs `daemon pair` again. With `--read-only` the card only reads. On the board, a box lane whose link is down shows no open button. Only the newest card of a lane polls the daemon, only while it is visible, and it never starts the daemon. Other hosts, terminal agents among them, get the same text without the card. Recent Claude Desktop builds do not show cards from stdio servers, reported on Windows and macOS ([anthropics/claude-ai-mcp#1069](https://github.com/anthropics/claude-ai-mcp/issues/1069)).
