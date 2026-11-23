---
cargo/devlanes-preset-tauri: patch
cargo/devlanes: patch
---

Fixed `preset: tauri` lanes and replicas sharing one Cargo target folder, which failed to start on Windows.

Each lane and replica builds in its own folder, which overrides a target folder from the shell, mise, direnv or Cargo's config. A `CARGO_TARGET_DIR` in the project file's `env` still wins and must differ per lane and replica; `tauri: { isolate_target_dir: false }` leaves the folder to your shell and Cargo.
