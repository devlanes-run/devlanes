---
cargo/devlanes: patch
cargo/devlanes-cli: patch
---

Added `devlanes uninstall`, which asks once, then stops the daemon, removes its login service, undoes `devlanes setup` and removes the binary, and asks again before deleting `~/.devlanes`; `install.sh --uninstall` and `install.ps1 -Uninstall` run it.
