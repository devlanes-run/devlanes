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

After upgrading the box and this machine, a box connected before waits in `needs_pairing` (`devlanes remote ls`). Run `devlanes remote pair` on the box, then `devlanes remote connect <box> --code <code>` here, or enter the code when `devlanes remote connect <box>` asks; later connections need no code. To keep the earlier behaviour, set `remote.require_pairing: false` in the box's user config. See "Pairing" in `docs/remote.md`.
