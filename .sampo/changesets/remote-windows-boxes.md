---
cargo/devlanes: patch
cargo/devlanes-tunnel: patch
cargo/devlanes-transport-ssh: patch
cargo/devlanes-daemon: patch
cargo/devlanes-client: patch
cargo/devlanes-sys-windows: patch
---

Added Windows boxes for `devlanes remote connect`: a Windows machine with Windows' OpenSSH Server and devlanes installed can now be a box, whether its ssh shell is cmd.exe, PowerShell or Git Bash, and the daemon the connection starts there keeps running after the ssh session ends.

See "Windows boxes" in `docs/guides/remote-setup.md`.
