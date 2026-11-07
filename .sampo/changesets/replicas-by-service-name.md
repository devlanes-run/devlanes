---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Fixed `devlanes stop`, `restart` and `logs` rejecting the name of a service with `replicas` instead of selecting every replica.
