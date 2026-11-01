---
cargo/devlanes: patch
cargo/devlanes-client: patch
---

Fixed `devlanes daemon install` and daemon restarts pointing at a versioned Homebrew path that `brew upgrade` removes; they now use the install's stable path.
