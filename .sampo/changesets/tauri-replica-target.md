---
cargo/devlanes: patch
cargo/devlanes-preset-tauri: patch
---

Fixed `preset: tauri` replicas rebuilding each other's app, which failed on Windows.

Replicas 2 and up ignore a `CARGO_TARGET_DIR` from the shell, mise or direnv. Set one in the service's `env` to choose the folder.
