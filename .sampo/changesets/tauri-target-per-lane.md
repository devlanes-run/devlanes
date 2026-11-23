---
cargo/devlanes-preset-tauri: patch
cargo/devlanes: patch
---

Fixed `preset: tauri` lanes and replicas sharing one Cargo target folder, which failed to start on Windows.

Each lane and replica builds in its own folder, which overrides a shell or Cargo-config target folder. Set `CARGO_TARGET_DIR` in the service's `env` to pick the folder, or `tauri: { isolate_target_dir: false }` to leave it to your shell and Cargo.
