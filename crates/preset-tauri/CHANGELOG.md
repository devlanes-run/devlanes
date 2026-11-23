# devlanes-preset-tauri

## 0.9.0 — 2026-11-23

### Patch changes

- [3b4ae053](https://github.com/devlanes-run/devlanes/commit/3b4ae0530e88b2c6ab845b808fa8478e94180c79) Fixed `preset: tauri` replicas rebuilding each other's app, which failed on Windows.
  
  Replicas 2 and up ignore a `CARGO_TARGET_DIR` from the shell, mise or direnv. Set one in the service's `env` to choose the folder.

## 0.4.0 ... 0.8.0 — 2026-11-16

Version bump only, to follow other packages.

