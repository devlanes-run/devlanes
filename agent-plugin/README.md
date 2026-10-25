# devlanes agent plugin

Instructions that teach coding agents to run dev services through the devlanes CLI instead of backgrounding them in a shell, to write `devlanes.yaml`, and to migrate other process-manager setups to it.

| Path | Content |
|---|---|
| `skills/devlanes/SKILL.md` | The skill: daily use. Claude Code, the skills CLI and Codex all use this file. |
| `skills/devlanes/references/configuration.md` | How to write a good `devlanes.yaml`; the skill tells agents to read it before editing the config. |
| `skills/devlanes/references/migration.md` | Mapping from mprocs, dekit, process-compose, Procfile runners, pm2, tmuxinator, tmuxp, package.json parallel scripts, mise and Docker Compose. |
| `.claude-plugin/plugin.json` | Claude Code plugin manifest. |
| `hooks/hooks.json`, `scripts/block-background.sh` | Optional Claude Code `PreToolUse` hooks that reject backgrounded dev servers: the script for the Bash tool (`scripts/test-block-background.sh` tests it), and `devlanes __agent-hook` for the PowerShell tool on Windows (tests in `crates/cli/tests/it/agent_hook.rs` and `bins/devlanes/tests/it/agent_hook.rs`). |
| `codex/AGENTS.md` | Short form of the skill for an `AGENTS.md` file. |

When the skill changes, update `codex/AGENTS.md` to match. A `devlanes-cli` test fails when a command in it no longer parses.

## Usage

### Claude Code

Install from the repository's marketplace:

```sh
claude plugin marketplace add devlanes-run/devlanes
claude plugin install devlanes@devlanes
```

For one session from a checkout, without installing:

```sh
claude --plugin-dir ./agent-plugin
```

The plugin adds the `devlanes` skill and two hooks. The Bash hook runs `scripts/block-background.sh` before every Bash tool call. It blocks the call (exit 2) when all of these hold:

- a command is detached: it ends with a lone `&` outside quotes (`pnpm dev &`, `nohup ... &`, `... &>/dev/null &`, `... & disown`), runs under `setsid -f`, `tmux new -d` or `screen -dm`, or the call sets `run_in_background`;
- that command looks like a dev server: a `dev`, `start`, `serve`, `watch`, `preview` or `up` argument, a `server.js`-style script, or a tool such as `vite`, `next`, `wrangler` or `uvicorn` without `build` or `test`;
- the directory it runs in, after any `cd DIR &&` before it, or one of that directory's parents contains `devlanes.yaml`.

A detached `devlanes` command, such as `devlanes logs api -f &`, passes. The block message tells Claude to use `devlanes start` or `devlanes exec --name NAME -- COMMAND`. The script uses `jq` when it is installed and otherwise reads the `command` and `cwd` values from the JSON input with awk. To turn the hook off, set `DEVLANES_BLOCK_BACKGROUND=0` in Claude Code's environment, for example under `env` in `~/.claude/settings.json`. That turns the PowerShell hook off too.

### PowerShell on Windows

On Windows Claude Code may run shell commands through its PowerShell tool instead of Bash, and a hook on `Bash` never sees those. A second hook runs before every PowerShell tool call:

```sh
devlanes __agent-hook --shell powershell; exit 0
```

It needs devlanes of the plugin's version or later on Claude Code's `PATH` (each release ships both with the same version). When devlanes is missing or older, the shell's "not found" error or devlanes' usage error still goes to the hook's stderr, but the `; exit 0` makes the call pass: the usage error exits 2, which Claude Code would read as a block of every PowerShell call. The line works the same in Git Bash and in PowerShell, the two shells Claude Code runs hook commands with on Windows, and `onFailure: "continue"` lets the call go ahead if the hook times out. On macOS and Linux, where the PowerShell tool is opt-in, devlanes does not answer `__agent-hook` and the hook passes every call.

It blocks the call (a `permissionDecision: "deny"` answer with the same message) under the same three conditions as the Bash hook, with PowerShell's ways of detaching a command:

- a trailing `&` outside quotes (`pnpm dev &`, `pnpm dev | Tee-Object dev.log &`), or the call sets `run_in_background`;
- `Start-Process` (`start`, `saps`) without `-Wait`, with `-FilePath`, `-ArgumentList` and `-WorkingDirectory` read;
- `Start-Job` (`sajb`) and `Start-ThreadJob`, judged by the commands in their script block;
- `cmd /c start` without `/wait`.

The dev-server and project checks are the Bash hook's, after `cd`, `Set-Location`, `sl` or `pushd`. Quotes, backtick escapes, here-strings, `#` and `<# #>` comments and the call operator (`& 'C:\tools\vite.cmd'`) are read, so `Write-Output 'pnpm dev &'` passes.

It does not catch:

- abbreviated parameter names (`-Wa` for `-Wait`, `-Wo` for `-WorkingDirectory`) and splatting (`Start-Process @params`);
- commands built at run time: arguments in variables (`Start-Process npm $arguments`), `Invoke-Expression`, `-EncodedCommand`, a nested `pwsh -Command "pnpm dev &"`, and scripts started with `-FilePath` or `.\dev.ps1`;
- .NET calls such as `[Diagnostics.Process]::Start(...)`, scheduled tasks and services;
- Codex, which reports PowerShell commands on Windows as the `Bash` tool, so only the Bash hook sees them.

It can also block too much: `Start-Process server.js` opens the file in its associated program, which may be an editor, and still counts as starting a server.

### Skills CLI

```sh
npx skills add devlanes-run/devlanes --skill devlanes
```

Add `-g` to install for every project, and `-a claude-code` or `-a codex` to pick the agent. This installs the skill only, without the hook.

The skills CLI finds the skill through the repository's `.claude-plugin/marketplace.json`, which lists `agent-plugin` as a plugin.

### Codex

Install the skill into `.agents/skills/`, where Codex reads skills:

```sh
npx skills add devlanes-run/devlanes --skill devlanes -a codex
```

Or copy `codex/AGENTS.md` into the repository's `AGENTS.md`, or into `~/.codex/AGENTS.md` for every repository.

## Behaviour

The CLI starts the daemon on first use, except inside a sandbox. It treats the process as sandboxed when `CODEX_SANDBOX` or `APP_SANDBOX_CONTAINER_ID` is set, or when it cannot bind a test Unix socket next to the daemon socket. It then fails with `auto-spawn refused in sandbox` instead of starting a daemon whose processes the sandbox would confine.

Run this once, outside the sandbox:

```sh
devlanes daemon install
```

It writes a per-user service (`~/Library/LaunchAgents/run.devlanes.daemon.plist` on macOS, `~/.config/systemd/user/devlanes.service` on Linux) that starts the daemon now and at every login. `devlanes daemon uninstall` removes it.

A sandboxed CLI still has to connect to the daemon's Unix socket. Whether the Codex sandbox allows that connection has not been tested yet. If it does not, approve running `devlanes` commands outside the sandbox.

## License

Apache-2.0
