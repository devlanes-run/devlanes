---
title: Environment variables
description: The variables devlanes sets for the processes it starts, the variables a new daemon keeps, and the variables devlanes itself reads.
kind: reference
---

devlanes sets variables in the processes it starts and reads others from the environment it runs in. The order in which a unit's variables are layered, and which layer wins, is on [Environment and expressions](../concepts/environment.md).

## Variables devlanes sets

### For every unit

Services, tasks, and `infra.setup` and `infra.teardown` entries get these.

| Variable | Value |
|---|---|
| `DEVLANES` | `1`. |
| `DEVLANES_CA_BUNDLE` | The path of `~/.devlanes/ca/bundle.pem`: the system's roots and the devlanes root in one file. Not set when the CA could not be loaded. |
| `DEVLANES_CA_CERT` | The path of the root certificate the proxy's certificates are signed with: `~/.devlanes/ca/ca.pem`, or mkcert's `rootCA.pem` with `tls.ca: mkcert`. Not set when the CA could not be loaded. |
| `DEVLANES_LANE` | The lane number: `0`, `1`, `2` and so on. |
| `DEVLANES_LANE_DATA` | The path of the [lane data folder](files.md#lane-data-folder). On Windows it is written with `/`. |
| `DEVLANES_LANE_NAME` | `main` for lane 0, `lane<N>` for lane N. |
| `DEVLANES_LANE_PROJECT` | `project` from the project file, such as `myapp`. devlanes commands run by the unit read it ([Picking the project](lane-and-unit-references.md#picking-the-project)). |
| `CLOUDFLARE_DEV_REGISTRY_PATH`, `MINIFLARE_REGISTRY_PATH`, `WRANGLER_REGISTRY_PATH` | `<lane data folder>/wrangler-registry`, replacing a value from the shell. |

### For services

| Variable | Value |
|---|---|
| `DEVLANES_ORIGINS` | Every origin of the lane, separated by commas: for each service, `https://` and each hostname without a wildcard, with the proxy's HTTPS port unless it is 443, then `http://localhost:<port>` and `http://127.0.0.1:<port>`. |
| `DEVLANES_SERVICE` | The service name from the project file, the same for every replica. |
| `HOST` | `127.0.0.1`. |
| `PORT` | The service's port, the same as `${{ self.port }}`. |

### For tasks and infra entries of a project with Compose

Tasks, and `infra.setup` and `infra.teardown` entries, get these when the project has `infra.compose`.

| Variable | Value |
|---|---|
| `COMPOSE_FILE` | The absolute paths of the Compose files of the primary worktree, separated by `:`, or by `;` on Windows. |
| `COMPOSE_PROFILES` | The active Compose profiles, separated by commas. Set only when there are some. |
| `COMPOSE_PROJECT_NAME` | The name of the shared Compose project. |
| `DEVLANES_COMPOSE_LANE_PROJECT` | The name of the lane's own Compose project. Set only when the lane has `per_lane` services. |

### For commands run with exec and with

`devlanes exec`, `devlanes with`, and `devlanes env show`, `env export` and `env shell` without a unit, get the lane's variables, which differ from a unit's.

| Variable | Value |
|---|---|
| `DEVLANES`, `DEVLANES_CA_BUNDLE`, `DEVLANES_CA_CERT`, `DEVLANES_LANE`, `DEVLANES_LANE_DATA`, `DEVLANES_LANE_NAME`, `DEVLANES_LANE_PROJECT` | As for every unit. |
| `DEVLANES_PUBLIC_URL_<SERVICE>` | One per service: its `public_url`, or its `url` when it has no hostname. `<SERVICE>` is the service name in uppercase, with each character other than a letter or a digit replaced by `_`: `crm-api` gives `DEVLANES_PUBLIC_URL_CRM_API`. |
| `NODE_EXTRA_CA_CERTS`, `DENO_CERT` | The path in `DEVLANES_CA_CERT`, unless the shell or `--env` sets the variable. |
| `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE` | The path in `DEVLANES_CA_BUNDLE`, unless the shell or `--env` sets the variable. |

These commands also get the `global_env` and profile `env` entries that read neither `self` nor `replica`. An entry that reads a container that is not running is left out, and `exec` and `env show` name it. Units do not get `DEVLANES_PUBLIC_URL_<SERVICE>`.

### By platform

A unit's platform is its `platform` setting, or the one devlanes detects from its `run_script` file or its working directory.

| Platform | Variable | Value |
|---|---|---|
| `bun` | `FORCE_COLOR` | `1`, unless the shell sets `FORCE_COLOR` or `NO_COLOR`. |
| `bun` | `NODE_EXTRA_CA_CERTS` | The path in `DEVLANES_CA_CERT`. It replaces a value from the shell, and the unit's notes say so. |
| `deno` | `DENO_CERT` | The path in `DEVLANES_CA_CERT`, else in `DEVLANES_CA_BUNDLE`. It replaces a value from the shell, and the unit's notes say so. |
| `deno` | `DENO_NO_UPDATE_CHECK` | `1`. |
| `deno` | `FORCE_COLOR` | `1`, unless the shell sets `FORCE_COLOR` or `NO_COLOR`. |
| `node` | `FORCE_COLOR` | `1`, unless the shell sets `FORCE_COLOR` or `NO_COLOR`. |
| `node` | `NODE_EXTRA_CA_CERTS` | The path in `DEVLANES_CA_CERT`. It replaces a value from the shell, and the unit's notes say so. |
| `python` | `PYTHONUNBUFFERED` | `1`. |
| `python` | `REQUESTS_CA_BUNDLE`, `SSL_CERT_FILE` | The path in `DEVLANES_CA_BUNDLE`. |
| `rust` | `CARGO_TERM_COLOR` | `always`, unless the shell sets `CARGO_TERM_COLOR` or `NO_COLOR`. |
| `rust` | `SSL_CERT_FILE` | The path in `DEVLANES_CA_BUNDLE`. It replaces a value from the shell, and the unit's notes say so. |

### By preset

| Preset | Variable | Value |
|---|---|---|
| `tauri` | `DEVLANES_TAURI_CONFIG` | The path of the config overlay for this lane and replica, `<lane data folder>/tauri/<service>.<replica>.conf.json`, also passed with `--config`. |
| `wrangler` | `CLOUDFLARE_DEV_REGISTRY_PATH`, `MINIFLARE_REGISTRY_PATH`, `WRANGLER_REGISTRY_PATH` | With `isolate_registry: false`, not set to the lane's folder. The service keeps a value from the shell, if it has one. |

The `vite` and `next` presets set no variables.

### By run script on Windows

A `run_script` from a `package.json` on Windows gets the variables npm gives a script, since devlanes runs the script's steps without npm.

| Variable | Value |
|---|---|
| `INIT_CWD` | The package's folder. |
| `npm_command` | `run-script`. |
| `npm_lifecycle_event` | The script name. |
| `npm_lifecycle_script` | The script's command. |
| `npm_package_bin*`, `npm_package_config_*`, `npm_package_engines_*`, `npm_package_name`, `npm_package_version` | The fields of `package.json`, flattened as npm does. |
| `npm_package_json` | The path of `package.json`. |

### For Git's shell on Windows

Each command devlanes runs through Git for Windows' `sh.exe` gets these.

| Variable | Value |
|---|---|
| `MSYS2_ARG_CONV_EXCL` | `*`. |
| `MSYS_NO_PATHCONV` | `1`. |

The folder of `sh.exe` is added to the end of `PATH`.

## Variables a new daemon keeps

A command that finds no daemon running starts one. That daemon keeps only these variables from the command's environment and drops the rest:

| Kept | Variables |
|---|---|
| By name | `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `LANG`, `TZ`, `TMPDIR`, `TERM`, `SSH_AUTH_SOCK`, `KUBECONFIG`, `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, `http_proxy`, `https_proxy`, `no_proxy` |
| CI markers | `CI`, `GITHUB_ACTIONS`, `GITLAB_CI`, `BUILDKITE`, `CIRCLECI`, `TF_BUILD`, `JENKINS_URL`, `TEAMCITY_VERSION`, `BITBUCKET_BUILD_NUMBER` |
| By prefix | Every variable that starts with `LC_`, `XDG_`, `DOCKER_` or `DEVLANES_` |
| On Windows, by name | `SystemRoot`, `SystemDrive`, `windir`, `ComSpec`, `PATHEXT`, `USERPROFILE`, `USERNAME`, `USERDOMAIN`, `APPDATA`, `LOCALAPPDATA`, `ProgramData`, `ProgramFiles`, `ProgramFiles(x86)`, `ProgramW6432`, `CommonProgramFiles`, `TEMP`, `TMP`, `PROCESSOR_ARCHITECTURE`, `NUMBER_OF_PROCESSORS`, `OS`, `HOMEDRIVE`, `HOMEPATH`, `PSModulePath` |

A daemon started by the login service of `devlanes daemon install` gets the service manager's environment. On macOS and Linux the service sets `DEVLANES_HOME` and nothing else. On Windows it sets nothing, so `devlanes daemon install` refuses to install while `DEVLANES_HOME` names another home than the default.

Units do not get the daemon's environment. Each unit gets the environment of the command that started it ([Environment and expressions](../concepts/environment.md)).

## Variables devlanes reads

### devlanes

| Variable | Read by | Values | Effect |
|---|---|---|---|
| `DEVLANES_AGENT` | every command | `0`. | Turns agent detection off: the command counts as a person's. |
| `DEVLANES_HOME` | every command, the daemon, the install scripts | A path. Empty counts as unset. | The [devlanes home](files.md#the-devlanes-home), instead of `~/.devlanes`. Each home has its own daemon. |
| `DEVLANES_INSTALL_DIR` | `install.sh`, `install.ps1` | A folder. | Where the scripts install, when no folder is passed. |
| `DEVLANES_INSTALL_SUDO` | `install.sh` | `1`. | Lets the script use sudo when the install folder is not writable. |
| `DEVLANES_LANE_PROJECT` | every command | A project name. Set by devlanes in every unit. | Picks the project after `--file`, `--project` and `DEVLANES_PROJECT` ([Picking the project](lane-and-unit-references.md#picking-the-project)). |
| `DEVLANES_LOG` | the daemon, when it starts | A filter in `RUST_LOG` syntax, such as `debug` or `devlanes_daemon=trace`. Default `info`. | The level of `logs/daemon.log`. |
| `DEVLANES_NO_UI` | `start` | Any value except empty and `0`. | A start on a terminal prints its progress instead of opening the TUI, as `--no-ui` does. |
| `DEVLANES_NO_UPDATE_CHECK` | the daemon | Any value except empty, `0` and `false`. | Turns the release check off. |
| `DEVLANES_OUTPUT` | every command | `human`, `plain`, `agent`, `json`. Another value is a usage error. | The [output mode](states-and-exit-codes.md#output-modes), unless `--output` or `--json` is given. |
| `DEVLANES_PROJECT` | every command | A project name. | The same as `--project`. |
| `DEVLANES_RESOLVING_ENVIRONMENT` | your shell profile | Set by `devlanes mcp` in the login shell whose environment it reads. | A profile can test it to skip slow setup that only an interactive shell needs. |
| `DEVLANES_SHELL_ENV` | `devlanes mcp` | `0` or `1` on macOS and Linux; `pwsh` or `powershell` on Windows. | `0` stops the MCP server from reading the login shell's environment, `1` makes it read it when a shell started the server. On Windows, it names the PowerShell to read; reading is off without it. |
| `DEVLANES_SHELL_ENV_TIMEOUT` | `devlanes mcp` | Seconds, 1 to 120. Default 10. | How long the login shell may take. |
| `DEVLANES_TERM_LINKS` | commands writing to a terminal | `0`, `false`, `off`, `never`; or `1`, `true`, `on`, `always`. | Overrides `terminal.links`: clickable links. |
| `DEVLANES_TERM_MARKS` | commands writing to a terminal | As `DEVLANES_TERM_LINKS`. | Overrides `terminal.marks`: prompt and command marks. |
| `DEVLANES_TERM_NOTIFICATIONS` | commands writing to a terminal | As `DEVLANES_TERM_LINKS`. | Overrides `terminal.notifications`: desktop notifications. |
| `DEVLANES_TERM_PROGRESS` | commands writing to a terminal | As `DEVLANES_TERM_LINKS`. | Overrides `terminal.progress`: the progress bar in the terminal's tab or taskbar. |
| `DEVLANES_TERM_TITLE` | commands writing to a terminal | As `DEVLANES_TERM_LINKS`. | Overrides `terminal.title`: the terminal's title. |
| `DEVLANES_VERBOSE` | every command | `1` or `2`. | The same as `-v` or `-vv`. |

### Output and terminals

| Variable | Read by | Values | Effect |
|---|---|---|---|
| `CI` | every command, the daemon | Any value except empty, `0` and `false`. | CI detection: plain output, no prompts, no TUI, terminal features set to `auto` stay off, no release check. The daemon gives the clone the job made lane 0, on any branch. |
| `CLICOLOR_FORCE` | every command | Any value except empty and `0`. | Color in human and plain output when it does not go to a terminal. |
| `COLORFGBG`, `COLORTERM` | the TUI | Set by terminals. | With `tui.theme: auto`: a dark or light background, and true color. |
| `GITHUB_ACTIONS`, `GITLAB_CI`, `BUILDKITE`, `CIRCLECI`, `TF_BUILD`, `JENKINS_URL`, `TEAMCITY_VERSION`, `BITBUCKET_BUILD_NUMBER` | every command, the daemon | Any value except empty, `0` and `false`. | CI detection, as `CI`. On GitHub Actions and GitLab CI, plain output keeps its colors. |
| `GITHUB_STEP_SUMMARY` | `devlanes with` | A file. | `with` adds its table of units, run times, CPU and memory to it. |
| `LANG`, `LC_ALL`, `LC_CTYPE` | the TUI | A locale. | With `tui.glyphs: auto`: the first one set of `LC_ALL`, `LC_CTYPE`, `LANG` decides between Unicode and ASCII symbols. |
| `NO_COLOR` | every command, the TUI | Any value except empty. | No color. Node, Bun, Deno and Rust units then get no `FORCE_COLOR` or `CARGO_TERM_COLOR`. |
| `TERM` | commands writing to a terminal | `dumb`, or the terminal's name. | `dumb` turns terminal features off. Other names identify the terminal. |
| `TERM_PROGRAM`, `TERM_PROGRAM_VERSION`, `TERM_FEATURES`, `WT_SESSION`, `KITTY_WINDOW_ID`, `ALACRITTY_WINDOW_ID`, `KONSOLE_VERSION`, `VTE_VERSION`, `PTYXIS_VERSION`, `XTERM_VERSION` | commands writing to a terminal | Set by terminals. | Identify the terminal, for the features set to `auto`. |
| `TMUX`, `STY` | commands writing to a terminal | Set by tmux and screen. | Terminal features set to `auto` stay off. |

### Agents and sandboxes

| Variable | Read by | Values | Effect |
|---|---|---|---|
| `AGENT` | every command | A known agent's name, or a true value. | As `AI_AGENT`. Another value is ignored. |
| `AI_AGENT` | every command | An agent's name, or any value. | Marks the caller as a coding agent: agent output, no prompts, and units it starts count as the agent's for `--mine`. |
| `CLAUDE_PROJECT_DIR` | `devlanes mcp` | A folder. | The working directory of a tool call that names none, after `--path`. |
| `CODEX_SANDBOX`, `APP_SANDBOX_CONTAINER_ID` | every command | Set by a sandbox. | When no daemon runs, the command does not start one and fails with `auto-spawn refused in sandbox`. A daemon that already runs is used. |
| `MCP_TOOL_TIMEOUT` | `devlanes mcp` under Claude Code | Milliseconds, 1000 or more. | Sets the timeout given to blocking commands, a little under this value ([MCP tools](mcp-tools.md)). |
| `SHLVL` | `devlanes mcp` on macOS and Linux | A number. | Above 0, the server counts as started from a shell and does not read the login shell, unless `DEVLANES_SHELL_ENV=1`. |
| Variables coding agents set in their shells, such as `CLAUDECODE`, `CODEX_SANDBOX`, `CURSOR_AGENT`, `GEMINI_CLI` and `OPENCODE` | every command | Set by the agent. | As `AI_AGENT`, for that agent. |

### System

| Variable | Read by | Values | Effect |
|---|---|---|---|
| `APPDATA`, `CARGO_HOME`, `LOCALAPPDATA`, `UV_TOOL_DIR`, `XDG_DATA_HOME` | `devlanes update`, `devlanes uninstall` | Folders. | Where cargo, uv and `install.ps1` put the command, to tell how devlanes was installed. |
| `DOCKER_CONFIG`, `DOCKER_CONTEXT`, `DOCKER_HOST` | the daemon | As for the Docker CLI. | The Docker engine devlanes uses for containers. |
| `HOME` | every command, the daemon | A folder. | On macOS and Linux, the user's home: the default `~/.devlanes` and `~/` in settings. On Windows the profile folder takes its place for both. Lane references that start with `~`, and the default `sync` destination `~/devlanes/sync`, read the `HOME` of the command on every system. |
| `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, and the same names in lowercase | the daemon | Proxy settings. | Used by the release check and passed to `docker compose`. |
| `KUBECONFIG` | the daemon | A path. | The cluster of a box reached with `kubectl exec`. |
| `SHELL` | `devlanes env shell`, `devlanes mcp` | A shell's path. | The shell `env shell` opens: without it `/bin/sh`, or `%ComSpec%` on Windows. The login shell `devlanes mcp` reads on macOS and Linux: without it, the one in the user database. |
| `SSH_AUTH_SOCK` | the daemon | Set by an SSH agent. | The agent `devlanes remote connect` authenticates with. |
| `TMPDIR` | every command, the daemon, on macOS and Linux | A folder. | Where the daemon's socket goes when its path in the home is too long ([Socket and pipe](files.md#socket-and-pipe)). |
| `USERPROFILE` | `run_script` on Windows | A folder. | Where the `.npmrc` with a `script-shell` setting is looked for last. |
| `WSL_DISTRO_NAME` | every command, the daemon, on Linux | Set by WSL. | Detects that devlanes runs in WSL. |
| `XDG_RUNTIME_DIR` | every command, the daemon, on Linux | A folder owned by you with mode 0700. | Where the daemon's socket goes ([Socket and pipe](files.md#socket-and-pipe)). |
