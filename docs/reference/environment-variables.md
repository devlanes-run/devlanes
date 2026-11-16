---
title: Environment variables
description: The variables devlanes sets for the processes it starts, and the variables you set to change what devlanes does.
kind: reference
---

devlanes sets variables in the processes it starts, and reads a few that you set. The order in which a unit's variables are layered, and which layer wins, is on [Environment and expressions](../concepts/environment.md).

## Variables devlanes sets

### For every unit

Services, tasks, and `infra.setup` and `infra.teardown` entries get these.

| Variable | Value |
|---|---|
| `DEVLANES` | `1`. |
| `DEVLANES_CA_BUNDLE` | The path of `~/.devlanes/ca/bundle.pem`: the system's roots and the devlanes root in one file. |
| `DEVLANES_CA_CERT` | The path of the devlanes root certificate: `~/.devlanes/ca/ca.pem`, or mkcert's `rootCA.pem` with `tls.ca: mkcert`. |
| `DEVLANES_LANE` | The lane number: `0`, `1`, `2` and so on. |
| `DEVLANES_LANE_DATA` | The path of the [lane data folder](files.md#lane-data-folder). |
| `DEVLANES_LANE_NAME` | `main` for lane 0, `lane<N>` for lane N. |
| `DEVLANES_LANE_PROJECT` | `project` from the project file, such as `myapp`. devlanes commands run by the unit use this project. |

### For services

| Variable | Value |
|---|---|
| `DEVLANES_ORIGINS` | The lane's origins, separated by commas: `https://` and each hostname of each service, then `http://localhost:<port>` and `http://127.0.0.1:<port>` for each service. |
| `DEVLANES_SERVICE` | The service name from the project file. |
| `HOST` | `127.0.0.1`. |
| `PORT` | The service's port. |

### For each runtime

| Runtime | Variable | Value |
|---|---|---|
| Node, Bun | `NODE_EXTRA_CA_CERTS` | The path in `DEVLANES_CA_CERT`. |
| Deno | `DENO_CERT` | The path in `DEVLANES_CA_CERT`. |
| Python | `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE` | The path in `DEVLANES_CA_BUNDLE`. |
| Rust | `SSL_CERT_FILE` | The path in `DEVLANES_CA_BUNDLE`. |
| Go | `SSL_CERT_FILE` | The path in `DEVLANES_CA_BUNDLE`. |

### For commands run with exec and with

`devlanes exec` and `devlanes with` give the command the lane's variables: the ones for every unit, the `global_env` and profile `env` entries that read neither `self` nor `replica`, and one variable per service:

| Variable | Value |
|---|---|
| `DEVLANES_PUBLIC_URL_<SERVICE>` | The service's `public_url`, or its `url` when it has no hostname. `<SERVICE>` is the service name in uppercase with each other character replaced by `_`: `crm-api` gives `DEVLANES_PUBLIC_URL_CRM_API`. |

## Variables you set

| Variable | Values | Effect |
|---|---|---|
| `CLICOLOR_FORCE` | Any value except empty and `0`. | Color in output that does not go to a terminal. |
| `DEVLANES_AGENT` | `0` | The command counts as a person's, even when a coding agent runs it. |
| `DEVLANES_HOME` | A path. | The [devlanes home](files.md#the-devlanes-home), instead of `~/.devlanes`. Each home has its own daemon. |
| `DEVLANES_INSTALL_DIR` | A folder. | Where `install.sh` and `install.ps1` install. |
| `DEVLANES_LOG` | A level such as `debug`. Default `info`. | The level of the daemon's log, read when the daemon starts. |
| `DEVLANES_NO_UI` | Any value except empty and `0`. | A start on a terminal prints its progress instead of opening the TUI, as `--no-ui` does. |
| `DEVLANES_NO_UPDATE_CHECK` | Any value except empty, `0` and `false`. | Turns the release check off. |
| `DEVLANES_OUTPUT` | `human`, `plain`, `agent` or `json`. | The [output mode](states-and-exit-codes.md#output-modes), unless `--output` or `--json` is given. |
| `DEVLANES_PROJECT` | A project name. | The same as `--project`. |
| `DEVLANES_SHELL_ENV` | `0` or `1`; `pwsh` or `powershell` on Windows. | Whether `devlanes mcp` reads your login shell's environment ([MCP tools](mcp-tools.md)). |
| `DEVLANES_TERM_LINKS`, `DEVLANES_TERM_MARKS`, `DEVLANES_TERM_NOTIFICATIONS`, `DEVLANES_TERM_PROGRESS`, `DEVLANES_TERM_TITLE` | `0` or `1`. | Turns off or on the terminal feature of the same name under `terminal` in the user config. |
| `DEVLANES_VERBOSE` | `1` or `2`. | The same as `-v` or `-vv`. |
| `NO_COLOR` | Any value except empty. | No color. |

## Variables a new daemon keeps

A daemon that a command starts keeps only these variables of that command's shell: `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `LANG`, `TZ`, `TMPDIR`, `TERM`, `SSH_AUTH_SOCK`, `KUBECONFIG`, the proxy variables such as `HTTPS_PROXY`, the CI markers such as `CI`, and those that start with `LC_`, `XDG_`, `DOCKER_` or `DEVLANES_`. On Windows it also keeps the system's own variables, such as `USERPROFILE`, `APPDATA` and `TEMP`. Each unit gets the environment of the command that started it.
