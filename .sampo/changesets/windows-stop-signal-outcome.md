---
cargo/devlanes: patch
cargo/devlanes-runner-windows: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
cargo/devlanes-tui: patch
---

Fixed Windows units ended by a stop showing exit codes such as -1073741510, 255 or 1 instead of a signal, as on macOS and Linux.

just recipes and Git's `bin\bash.exe` now show the code they return, `512` when their shell ended on Ctrl-C.
