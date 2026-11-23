---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-cli: patch
---

Fixed `start`, `stop`, `restart`, `reload`, `logs` and `with` sending units named without `lane:` to another unit's lane instead of the current one.

To act on another lane, prefix every unit with it or use `--lane`. One lane written two ways, such as `1:api myapp@1:web`, now works.
