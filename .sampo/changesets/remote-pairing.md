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

Changed `devlanes remote connect` so a box serves only machines it paired with, and added `devlanes daemon pair`, `devlanes daemon clients`, `devlanes daemon revoke` and `remote.require_pairing`.

A machine's first connection to a box needs a one-time code: run `devlanes daemon pair` on the box, then `devlanes remote connect <box> --code <code>` here, or enter the code when `devlanes remote connect <box>` asks. Later connections, including those after a daemon restart, need none. To let every machine that gets in over SSH connect without a code, as before, set `remote.require_pairing: false` in the box's user config. See "Pairing" in `docs/remote.md`.
