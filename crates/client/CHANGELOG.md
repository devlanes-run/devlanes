# devlanes-client

## 0.13.0 — 2026-11-09

### Patch changes

- [40afbabe](https://github.com/devlanes-run/devlanes/commit/40afbabe21bbc5431a65a080085d9e023dfbe5ce) Fixed the unit picker and the TUI choosing a `lane new` lane instead of the folder's own lane.

## 0.12.0 — 2026-11-07

### Patch changes

- [5db5cde](https://github.com/devlanes-run/devlanes/commit/5db5cdec69bc61915e31237b81abf0456a061bfc) Added `devlanes mcp`, an MCP server that lets AI agents run devlanes commands as tools.
  
  Register it with your agent, such as `claude mcp add devlanes -- devlanes mcp` for Claude Code. See [coding agents](https://github.com/devlanes-run/devlanes/blob/main/docs/guides/mcp.md) for Codex, OpenCode, Cursor, VS Code, Claude Desktop and Goose.
  
  <!-- highlight
  title: Use devlanes from your agent over MCP
  body: Run `claude mcp add devlanes -- devlanes mcp` (or see `devlanes mcp --help` for other agents) and the agent starts dev servers with `exec`, reads logs and stops what it started, asking you before it deletes lane data.
  -->

## 0.11.2 — 2026-11-06

### Patch changes

- [91c4f2b](https://github.com/devlanes-run/devlanes/commit/91c4f2b60a70695afc248f0b045a499292336a43) Fixed starting the daemon reporting no reason when the daemon stopped during startup.

## 0.11.0 ... 0.11.1 — 2026-11-01

Version bump only, to follow other packages.

## 0.10.0 — 2026-11-01

### Patch changes

- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Fixed `devlanes daemon install` and restarts from a TUI that outlived an update starting a removed binary (a Homebrew Cellar path, or a replaced file on Linux); they now use the install's stable path.

## 0.7.1 ... 0.9.0 — 2026-10-29

Version bump only, to follow other packages.

## 0.7.0 — 2026-10-28

### Minor changes

- [76f6716](https://github.com/devlanes-run/devlanes/commit/76f67163f90f6857092f8e42e44f1cbdc1acaf6d) Changed `devlanes daemon restart` to finish as soon as the new daemon answers, with the services that ran coming back as ordinary starts; once the old daemon was asked to stop, the restart completes even if the terminal or TUI that asked goes away.
  
  `daemon restart --json` now lists the lanes it starts again in `resuming` instead of their start results in `resumed`; use `devlanes status` or `devlanes wait` to follow them.

### Patch changes

- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Added Windows boxes for `devlanes remote connect`: a Windows machine with Windows' OpenSSH Server and devlanes installed can be a box, with cmd.exe, PowerShell or Git Bash as its SSH shell.
  
  See "Windows boxes" in `docs/guides/remote-setup.md`.

## 0.6.0 — 2026-10-26

### Patch changes

- [a7472f6](https://github.com/devlanes-run/devlanes/commit/a7472f6affae0e0d4e86ccfd7a26bb614a6c40b0) Fixed the first `devlanes` command on a busy machine, or one run while another started the daemon, failing with "daemon is unreachable" while the daemon was still starting.
  
  The command now waits as long as the daemon makes progress. When one startup step hangs for 30 seconds it fails with "the daemon has not finished starting", names the step and hints `devlanes daemon logs`.

## 0.4.2 ... 0.5.0 — 2026-10-26

Version bump only, to follow other packages.

## 0.4.1 — 2026-10-26

### Patch changes

- [546ea51](https://github.com/devlanes-run/devlanes/commit/546ea511256266e54b0b56fd05ecbab7953c1051) Added PyPI packages: `uv tool install devlanes`, `pipx install devlanes` or `pip install devlanes` installs the release build, and the update notice and `devlanes doctor` give the matching upgrade command.

## 0.4.0 — 2026-10-25

### Patch changes

- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Fixed an empty `HOME` being used as the home directory; devlanes now reports it as missing, like an unset one.

