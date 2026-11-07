# devlanes-platform-node

## 0.7.0 — 2026-11-07

### Patch changes

- [077b5b0](https://github.com/devlanes-run/devlanes/commit/077b5b0ed890510e4b3417c26d621fc41a97a5cc) Fixed `WRANGLER_REGISTRY_PATH` and the other Wrangler registry variables on Windows to use `/` like `DEVLANES_LANE_DATA`.

## 0.6.0 — 2026-10-28

### Patch changes

- [01fb93f](https://github.com/devlanes-run/devlanes/commit/01fb93f56d3265cfe7756d4742dba08c93cc1fd0) Changed the lane's Wrangler dev registry (`WRANGLER_REGISTRY_PATH`, `CLOUDFLARE_DEV_REGISTRY_PATH` and `MINIFLARE_REGISTRY_PATH`) to be set for every process of a lane, so workers started by `wrangler dev`, a script, a task or the Cloudflare Vite or a Next plugin use the lane's registry even when the shell exports one, and starts no longer scan the worktree for a Wrangler config. `platform: none` now skips platform detection without a `config check` warning.

## 0.4.0 ... 0.5.0 — 2026-10-26

Version bump only, to follow other packages.

