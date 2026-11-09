# devlanes-shell-windows

## 0.5.0 — 2026-11-09

### Patch changes

- [d5747169](https://github.com/devlanes-run/devlanes/commit/d5747169e4c7d2f579e00d773f8c00a0105db59a) Changed `run_script` npm scripts on Windows to run without npm, so a stop reports the program's own exit code instead of `exit=1`.
  
  Scripts get npm's package and lifecycle variables and `node_modules\.bin` on `PATH`, but not npm's `npm_config_*` settings.

## 0.4.0 — 2026-10-25

### Patch changes

- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Added native support for Windows 11 24H2 or later, on x64 and arm64.
  
  Install with `irm https://devlanes.run/install.ps1 | iex`. `*.localhost` resolves without a hosts file, `daemon install` starts the daemon at logon, and `certs trust --resolver` writes an NRPT rule for custom domains. See `docs/guides/windows.md`.

