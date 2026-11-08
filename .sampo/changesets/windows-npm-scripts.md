---
cargo/devlanes: minor
cargo/devlanes-run-script-npm: minor
cargo/devlanes-core: patch
cargo/devlanes-resolve: patch
cargo/devlanes-shell-windows: patch
---

Changed `run_script` npm scripts on Windows to run without npm, so a stop reports the program's own exit code instead of `exit=1`.

Scripts get npm's package and lifecycle variables and `node_modules\.bin` on `PATH`, but not npm's `npm_config_*` settings.
