---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
cargo/devlanes-cli: patch
---

Fixed `devlanes start` rerunning a `container:` setup entry before its restarted container is healthy.

The start result now lists the setup entries and tasks it ran instead of saying nothing started.
