---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
---

Fixed `devlanes status` saying `config changed` with nothing to reload after some project file edits.

`status` also shows a service as changed when a value it reads from your shell through `${{ env.NAME }}` differs after a start from another shell.
