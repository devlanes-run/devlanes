---
cargo/devlanes: minor
cargo/devlanes-cli: minor
cargo/devlanes-config: minor
cargo/devlanes-daemon: minor
cargo/devlanes-pages: minor
cargo/devlanes-proto: minor
cargo/devlanes-proxy: minor
cargo/devlanes-state: minor
cargo/devlanes-transport: minor
cargo/devlanes-tui: patch
cargo/devlanes-tunnel: minor
---

Changed `devlanes remote connect` so a box serves only machines it paired with, and added `devlanes remote pair`, `devlanes remote clients`, `devlanes remote revoke` and `remote.require_pairing`.

Before a machine's first connection, run `devlanes remote pair` on the box and enter the code on the machine, or pass it with `devlanes remote connect <box> --code <code>`; later connections need no code. A box whose `~/.devlanes/config.yaml` sets `remote.require_pairing: false` serves every machine that reaches it, as before.
