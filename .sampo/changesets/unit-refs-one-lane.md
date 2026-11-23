---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Fixed `start`, `stop`, `restart`, `reload`, `logs` and `with` applying one unit's `lane:` to units named without one.

Units that name one lane in different words, such as `1:api myapp@1:web`, work.
