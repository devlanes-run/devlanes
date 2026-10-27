---
cargo/devlanes: patch
cargo/devlanes-tui: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
cargo/devlanes-cli: patch
---

Added remote boxes to the TUI: `B` opens the boxes view, where `n` connects a box by its SSH name or a command and Enter connects, resumes or disconnects one, with ssh's prompts and the pairing code asked in the TUI. Connected boxes' projects get tabs after the local ones (`acme @hetzner`), read-only, and a box unit's pane shows its ports, mirror state and the URLs that open it here.

`devlanes remote ls --json` entries also carry the box's `registry`, each port's `state` and each route's `url`.
