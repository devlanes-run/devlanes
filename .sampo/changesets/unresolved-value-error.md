---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-cli: patch
cargo/devlanes-tui: patch
---

Changed the error for a config value with no value in the lane, such as an unset `${{ env.X }}`, to stop calling `devlanes.yaml` invalid.

Its JSON code is `unresolved_value` instead of `config_error` or `lane_env_unresolved`, for services, tasks, `exec` and `env show` alike. Older clients show the code and message without the variable's name.
