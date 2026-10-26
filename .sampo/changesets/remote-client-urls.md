---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
cargo/devlanes-transport: patch
cargo/devlanes-tunnel: patch
---

Added the URLs connected clients open to `devlanes url` on a box: it prints `on <client>: <url>` for each machine connected with `devlanes remote connect`, or why the unit does not open there, and `devlanes url --json` lists them under `clients`.

`devlanes url ls` counts the hostnames each client reaches, and `devlanes status --json` shows what each client mirrors under `clients[].mirror`.
