---
cargo/devlanes: patch
cargo/devlanes-resolver-hosts: patch
---

Fixed `devlanes setup` failing to install the `/etc/hosts` helper on Ubuntu 25.10 and later, where `sudo` is sudo-rs.
