# devlanes-run-script-npm

## 0.7.0 — 2026-11-09

### Minor changes

- [d5747169](https://github.com/devlanes-run/devlanes/commit/d5747169e4c7d2f579e00d773f8c00a0105db59a) Changed `run_script` npm scripts on Windows to run without npm, so a stop reports the program's own exit code instead of `exit=1`.
  
  Scripts get npm's package and lifecycle variables and `node_modules\.bin` on `PATH`, but not npm's `npm_config_*` settings.

## 0.4.0 ... 0.6.0 — 2026-11-07

Version bump only, to follow other packages.

