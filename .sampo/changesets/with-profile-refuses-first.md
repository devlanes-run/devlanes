---
cargo/devlanes: minor
cargo/devlanes-cli: minor
cargo/devlanes-daemon: minor
cargo/devlanes-proto: patch
cargo/devlanes-resolve: minor
cargo/devlanes-config: minor
---

Changed `devlanes with --profile` to refuse a running lane whose profiles give its services other settings, before starting anything.

Stop the lane, run `with` in another worktree, or name the services instead of the profile. A profile that only picks services works on a running lane whose profiles set no `env`, `run_scripts` or `infra`.
