---
cargo/devlanes: patch
cargo/devlanes-preset-tauri: patch
---

Fixed `preset: tauri` replicas after the first rebuilding the app the first one runs, which failed on Windows.

Replicas 2 and up build in their own folder in the lane data folder, replacing a `CARGO_TARGET_DIR` from the shell; one in the project file's `env` still wins.
