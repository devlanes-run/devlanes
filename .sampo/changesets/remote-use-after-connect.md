---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
cargo/devlanes-cli: patch
cargo/devlanes-tui: patch
---

Fixed `devlanes remote use` right after `devlanes remote connect` saying the lane is chosen but not mirrored yet.

A choice that applies later, once its box connects or its lane is mirrored, now sends the same `lane_switched` notice as one that applies at once, and `remote use` and the TUI say which of the two it waits for. Choosing a box lane that only has tasks now says so, instead of pointing to `remote ls`.
