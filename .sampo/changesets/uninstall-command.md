---
cargo/devlanes: patch
cargo/devlanes-cli: patch
---

Added `devlanes uninstall`, which stops the daemon and removes its login service, what `devlanes setup` changed and the binary, then asks before deleting `~/.devlanes`; `install.sh --uninstall` and `install.ps1 -Uninstall` run it.
