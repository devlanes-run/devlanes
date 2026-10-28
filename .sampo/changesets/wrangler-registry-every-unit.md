---
cargo/devlanes: patch
cargo/devlanes-config: patch
cargo/devlanes-daemon: patch
cargo/devlanes-platform-node: patch
cargo/devlanes-preset-wrangler: patch
---

Changed the lane's Wrangler dev registry (`WRANGLER_REGISTRY_PATH`, `CLOUDFLARE_DEV_REGISTRY_PATH` and `MINIFLARE_REGISTRY_PATH`) to be set for every process of a lane, so workers started by `wrangler dev`, a script, a task or the Cloudflare Vite or a Next plugin use the lane's registry even when the shell exports one, and starts no longer scan the worktree for a Wrangler config. `platform: none` now skips platform detection without a `config check` warning.
