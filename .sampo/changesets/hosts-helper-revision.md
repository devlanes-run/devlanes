---
cargo/devlanes: patch
cargo/devlanes-hosts-helper: patch
cargo/devlanes-resolver-hosts: patch
cargo/devlanes-daemon: patch
---

Fixed every devlanes release asking to reinstall the /etc/hosts helper; it now asks only when the helper itself changed.
