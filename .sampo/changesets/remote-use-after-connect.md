---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
---

Fixed `devlanes remote use` right after `devlanes remote connect` saying the lane is chosen but not mirrored yet.

A choice that applies later, once its box connects or its lane is mirrored, now sends the same `lane_switched` notice as one that applies at once.
