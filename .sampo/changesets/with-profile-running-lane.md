---
cargo/devlanes: minor
cargo/devlanes-cli: minor
cargo/devlanes-daemon: minor
---

Changed `devlanes with --profile` to exit 1 when the lane's running services lack the profile's settings.

Stop the lane first, or run `with` in another worktree. `devlanes start --profile` on such a lane now warns about those services, and `devlanes status` shows them as changed until `devlanes reload`.
