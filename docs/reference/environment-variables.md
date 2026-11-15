---
title: Environment variables
description: The variables devlanes sets for the processes it starts, and the variables devlanes itself reads.
kind: reference
---

devlanes sets variables in the processes it starts and reads others from the environment it runs in. The order in which a unit's variables are layered, and which layer wins, is on [Environment and expressions](../concepts/environment.md).

## Variables devlanes sets

### For every unit

Services, tasks, and `infra.setup` and `infra.teardown` entries get these.

| Variable | Value |
|---|---|
| `DEVLANES` | `1`. |
| `DEVLANES_LANE_PROJECT` | `project` from the project file, such as `myapp`. devlanes commands run by the unit read it to pick the project ([How a command picks the project](lane-and-unit-references.md#how-a-command-picks-the-project)). |
| `DEVLANES_LANE` | The lane number: `0`, `1`, `2` and so on. |
| `DEVLANES_LANE_NAME` | `main` for lane 0, `lane<N>` for lane N. |
| `DEVLANES_LANE_DATA` | The path of the [lane data folder](files.md#lane-data-folder). On Windows it is written with `/`. |
| `DEVLANES_CA_CERT` | The path of the root certificate the proxy's certificates are signed with: `~/.devlanes/ca/ca.pem`, or mkcert's `rootCA.pem` with `tls.ca: mkcert`. Not set when the CA could not be loaded. |
| `DEVLANES_CA_BUNDLE` | The path of `~/.devlanes/ca/bundle.pem`: the system's roots and the devlanes root in one file. Not set when the CA could not be loaded. |
| `WRANGLER_REGISTRY_PATH` | `<lane data folder>/wrangler-registry`. |
| `CLOUDFLARE_DEV_REGISTRY_PATH` | `<lane data folder>/wrangler-registry`. |
| `MINIFLARE_REGISTRY_PATH` | `<lane data folder>/wrangler-registry`. |

### For services

| Variable | Value |
|---|---|
| `PORT` | The service's port, the same as `${{ self.port }}`. |
| `HOST` | `127.0.0.1`. |
| `DEVLANES_SERVICE` | The service name from the project file, the same for every replica. |
| `DEVLANES_ORIGINS` | Every origin of the lane, separated by commas: for each service, `https://` and each hostname without a wildcard, with the proxy's HTTPS port unless it is 443, then `http://localhost:<port>` and `http://127.0.0.1:<port>`. |

### For tasks and infra entries of a project with Compose

Tasks, and `infra.setup` and `infra.teardown` entries, get these when the project has `infra.compose`.

| Variable | Value |
|---|---|
| `COMPOSE_PROJECT_NAME` | The name of the shared Compose project. |
| `COMPOSE_FILE` | The absolute paths of the Compose files of the primary worktree, separated by `:`, or by `;` on Windows. |
| `COMPOSE_PROFILES` | The active Compose profiles, separated by commas. Set only when there are some. |
| `DEVLANES_COMPOSE_LANE_PROJECT` | The name of the lane's own Compose project. Set only when the lane has `per_lane` services. |

### For commands run with exec and with

`devlanes exec`, `devlanes with`, and `devlanes env show`, `env export` and `env shell` without a unit, get the lane's variables, which differ from a unit's.

| Variable | Value |
|---|---|
| `DEVLANES`, `DEVLANES_LANE_PROJECT`, `DEVLANES_LANE`, `DEVLANES_LANE_NAME`, `DEVLANES_LANE_DATA`, `DEVLANES_CA_CERT`, `DEVLANES_CA_BUNDLE` | As for every unit. |
| `DEVLANES_PUBLIC_URL_<SERVICE>` | One per service: its `public_url`, or its `url` when it has no hostname. `<SERVICE>` is the service name in uppercase, with each character other than a letter or a digit replaced by `_`: `crm-api` gives `DEVLANES_PUBLIC_URL_CRM_API`. |
| `NODE_EXTRA_CA_CERTS`, `DENO_CERT` | The path in `DEVLANES_CA_CERT`, unless the shell already sets the variable. |
| `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE` | The path in `DEVLANES_CA_BUNDLE`, unless the shell already sets the variable. |

These commands also get the `global_env` and profile `env` entries that read neither `self` nor `replica`. An entry that reads a container that is not running is left out, and `exec` and `env show` name it. Units do not get `DEVLANES_PUBLIC_URL_<SERVICE>`.

### By platform

A unit's platform is its `platform` setting, or the one devlanes detects from its `run_script` file or its working directory.

| Platform | Variable | Value |
|---|---|---|
| `node` | `NODE_EXTRA_CA_CERTS` | The path in `DEVLANES_CA_CERT`. It replaces a value from the shell, and the unit's notes say so. |
| `node` | `FORCE_COLOR` | `1`, unless the shell sets `FORCE_COLOR` or `NO_COLOR`. |
| `bun` | `NODE_EXTRA_CA_CERTS` | The path in `DEVLANES_CA_CERT`. It replaces a value from the shell, and the unit's notes say so. |
| `bun` | `FORCE_COLOR` | `1`, unless the shell sets `FORCE_COLOR` or `NO_COLOR`. |
| `deno` | `DENO_CERT` | The path in `DEVLANES_CA_CERT`, else in `DEVLANES_CA_BUNDLE`. It replaces a value from the shell, and the unit's notes say so. |
| `deno` | `DENO_NO_UPDATE_CHECK` | `1`. |
| `deno` | `FORCE_COLOR` | `1`, unless the shell sets `FORCE_COLOR` or `NO_COLOR`. |
| `python` | `PYTHONUNBUFFERED` | `1`. |
| `python` | `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE` | The path in `DEVLANES_CA_BUNDLE`. |
| `rust` | `CARGO_TERM_COLOR` | `always`, unless the shell sets `CARGO_TERM_COLOR` or `NO_COLOR`. |
| `rust` | `SSL_CERT_FILE` | The path in `DEVLANES_CA_BUNDLE`. It replaces a value from the shell, and the unit's notes say so. |

### By preset

| Preset | Variable | Value |
|---|---|---|
| `tauri` | `DEVLANES_TAURI_CONFIG` | The path of the config overlay for this lane and replica, `<lane data folder>/tauri/<service>.<replica>.conf.json`, also passed with `--config`. |
| `wrangler` | `WRANGLER_REGISTRY_PATH`, `CLOUDFLARE_DEV_REGISTRY_PATH`, `MINIFLARE_REGISTRY_PATH` | `<lane data folder>/wrangler-registry`. With `isolate_registry: false` the three are removed instead. |

The `vite` and `next` presets set no variables.

### By run script on Windows

A `run_script` from a `package.json` on Windows gets the variables npm gives a script, since devlanes runs the script's steps without npm.

| Variable | Value |
|---|---|
| `DEVLANES_NPM_PRE` | The `pre<name>` step, when the package has one. |
| `DEVLANES_NPM_SCRIPT` | The script step, with its arguments. |
| `DEVLANES_NPM_POST` | The `post<name>` step, when the package has one. |
| `npm_package_name`, `npm_package_version`, `npm_package_config_*`, `npm_package_engines_*`, `npm_package_bin*` | The fields of `package.json`, flattened as npm does. |
| `npm_package_json` | The path of `package.json`. |
| `npm_lifecycle_event` | The script name. |
| `npm_lifecycle_script` | The script's command. |
| `npm_command` | `run-script`. |
| `INIT_CWD` | The package's folder. |

### For Git's shell on Windows

Each command devlanes runs through Git for Windows' `sh.exe` gets these.

| Variable | Value |
|---|---|
| `MSYS_NO_PATHCONV` | `1`. |
| `MSYS2_ARG_CONV_EXCL` | `*`. |

The folder of `sh.exe` is added to the end of `PATH`.

## Variables devlanes reads

### devlanes

| Variable | Read by | Values | Effect |
|---|---|---|---|
| `DEVLANES_HOME` | every command, the daemon, the install scripts | A path. Empty counts as unset. | The [devlanes home](files.md#the-devlanes-home), instead of `~/.devlanes`. Each home has its own daemon. |
| `DEVLANES_PROJECT` | every command | A project name. | The same as `--project`. |
| `DEVLANES_OUTPUT` | every command | `human`, `plain`, `agent`, `json`. Another value is a usage error. | The [output mode](states-and-exit-codes.md#output-modes), unless `--output` or `--json` is given. |
| `DEVLANES_NO_UI` | `start` | Any value except empty and `0`. | A start on a terminal prints its progress instead of opening the TUI, as `--no-ui` does. |
| `DEVLANES_VERBOSE` | every command | `1` or `2`. | The same as `-v` or `-vv`. |
| `DEVLANES_AGENT` | every command | `0`. | Turns agent detection off: the command counts as a person's. |
| `DEVLANES_LOG` | the daemon, when it starts | A filter in `RUST_LOG` syntax, such as `debug` or `devlanes_daemon=trace`. Default `info`. | The level of `logs/daemon.log`. |
| `DEVLANES_NO_UPDATE_CHECK` | the daemon | Any value except empty, `0` and `false`. | Turns the release check off. |
| `DEVLANES_TERM_PROGRESS` | commands writing to a terminal | `0`, `false`, `off`, `never`; or `1`, `true`, `on`, `always`. | Overrides `terminal.progress`: the progress bar in the terminal's tab or taskbar. |
| `DEVLANES_TERM_TITLE` | commands writing to a terminal | As above. | Overrides `terminal.title`: the terminal's title. |
| `DEVLANES_TERM_LINKS` | commands writing to a terminal | As above. | Overrides `terminal.links`: clickable links. |
| `DEVLANES_TERM_NOTIFICATIONS` | commands writing to a terminal | As above. | Overrides `terminal.notifications`: desktop notifications. |
| `DEVLANES_TERM_MARKS` | commands writing to a terminal | As above. | Overrides `terminal.marks`: prompt and command marks. |
| `DEVLANES_SHELL_ENV` | `devlanes mcp` | `0` or `1` on macOS and Linux; `pwsh` or `powershell` on Windows. | `0` stops the MCP server from reading the login shell's environment, `1` makes it read it when a shell started the server. On Windows, it names the PowerShell to read; reading is off without it. |
| `DEVLANES_SHELL_ENV_TIMEOUT` | `devlanes mcp` | Seconds, 1 to 120. Default 10. | How long the login shell may take. |
| `DEVLANES_INSTALL_DIR` | `install.sh`, `install.ps1` | A folder. | Where the scripts install, when no folder is passed. |
| `DEVLANES_INSTALL_SUDO` | `install.sh` | `1`. | Lets the script use sudo when the install folder is not writable. |

### Output and terminals

| Variable | Read by | Values | Effect |
|---|---|---|---|
| `NO_COLOR` | every command, the TUI | Any value except empty. | No color. Node, Bun, Deno and Rust units then get no `FORCE_COLOR` or `CARGO_TERM_COLOR`. |
| `CLICOLOR_FORCE` | every command | Any value except empty and `0`. | Color in human and plain output when it does not go to a terminal. |
| `CI` | every command, the daemon | Any value except empty, `0` and `false`. | CI detection: plain output, no prompts, no TUI, terminal features set to `auto` stay off, no release check. |
| `GITHUB_ACTIONS`, `GITLAB_CI`, `BUILDKITE`, `CIRCLECI`, `TF_BUILD`, `JENKINS_URL`, `TEAMCITY_VERSION`, `BITBUCKET_BUILD_NUMBER` | every command, the daemon | Any value except empty, `0` and `false`. | CI detection, as `CI`. On GitHub Actions and GitLab CI, plain output keeps its colors. |
| `GITHUB_STEP_SUMMARY` | `devlanes with` | A file. | `with` adds its table of units, run times, CPU and memory to it. |
| `TERM` | commands writing to a terminal | `dumb`, or the terminal's name. | `dumb` turns terminal features off. Other names identify the terminal. |
| `TMUX`, `STY` | commands writing to a terminal | Set by tmux and screen. | Terminal features set to `auto` stay off. |
| `TERM_PROGRAM`, `TERM_PROGRAM_VERSION`, `TERM_FEATURES`, `WT_SESSION`, `KITTY_WINDOW_ID`, `ALACRITTY_WINDOW_ID`, `KONSOLE_VERSION`, `VTE_VERSION`, `PTYXIS_VERSION`, `XTERM_VERSION` | commands writing to a terminal | Set by terminals. | Identify the terminal, for the features set to `auto`. |
| `COLORTERM`, `COLORFGBG` | the TUI | Set by terminals. | With `tui.theme: auto`: true color, and a dark or light background. |
| `LC_ALL`, `LC_CTYPE`, `LANG` | the TUI | A locale. | With `tui.glyphs: auto`: the first one set decides between Unicode and ASCII symbols. |
| `SHELL` | `devlanes env shell` | A shell's path. | The shell it opens. Without it, `/bin/sh`, or `%ComSpec%` on Windows. |

### Agents and sandboxes

| Variable | Read by | Values | Effect |
|---|---|---|---|
| `AI_AGENT` | every command | An agent's name, or any value. | Marks the caller as a coding agent: agent output, no prompts, and units it starts count as the agent's for `--mine`. |
| `AGENT` | every command | A known agent's name, or a true value. | As `AI_AGENT`. Another value is ignored. |
| Variables coding agents set in their shells, such as `CLAUDECODE`, `CODEX_SANDBOX`, `CURSOR_AGENT`, `GEMINI_CLI` and `OPENCODE` | every command | Set by the agent. | As `AI_AGENT`, for that agent. |
| `CODEX_SANDBOX`, `APP_SANDBOX_CONTAINER_ID` | every command | Set by a sandbox. | The command does not start the daemon and fails with `auto-spawn refused in sandbox`. |
| `MCP_TOOL_TIMEOUT` | `devlanes mcp` under Claude Code | Milliseconds, 1000 or more. | Sets the timeout given to blocking commands, a little under this value ([MCP tools](mcp-tools.md)). |
| `CLAUDE_PROJECT_DIR` | `devlanes mcp` | A folder. | The working directory of a tool call that names none, after `--path`. |
| `SHLVL` | `devlanes mcp` on macOS and Linux | A number. | Above 0, the server counts as started from a shell and does not read the login shell, unless `DEVLANES_SHELL_ENV=1`. |

### System

| Variable | Read by | Values | Effect |
|---|---|---|---|
| `HOME` | every command, the daemon | A folder. | On macOS and Linux, the user's home: the default `~/.devlanes` and `~/` in settings. On Windows the profile folder takes its place for both. Lane references that start with `~` read `HOME` on every system. |
| `XDG_RUNTIME_DIR` | every command, the daemon, on Linux | A folder owned by you with mode 0700. | Where the daemon's socket goes ([Files and folders](files.md#socket-and-pipe)). |
| `DOCKER_HOST`, `DOCKER_CONTEXT`, `DOCKER_CONFIG` | the daemon | As for the Docker CLI. | The Docker engine devlanes uses for containers. |
| `SSH_AUTH_SOCK` | the daemon | Set by an SSH agent. | The agent `devlanes remote connect` authenticates with. |
| `KUBECONFIG` | the daemon | A path. | The cluster of a box reached with `kubectl exec`. |

The daemon reads its variables from the environment it started with: the shell of the command that started it, or the login service of `devlanes daemon install`.
