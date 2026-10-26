---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
cargo/devlanes-transport: patch
cargo/devlanes-tunnel: patch
---

Added the URLs connected clients open to `devlanes url` and `devlanes url ls` on a box: in a terminal `url` prints `on <client>: <url>` for each machine connected with `devlanes remote connect`, or why the unit does not open there, and `status --json` shows what each client mirrors under `clients[].mirror`.
