---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-daemon: patch
cargo/devlanes-proto: patch
---

Changed the summary of `devlanes with` and `devlanes stop --logs-dir`: it now names the command, lane and exit code, lists each unit's state, run time and average and peak CPU and memory, and in the GitHub step summary adds the last log lines of each failed unit in a collapsed block.

The `with.summary` and `stop.summary` JSON events carry the lane and the new fields (`state`, `exit_code`, `started_at_ms`, `finished_at_ms`, `avg_cpu_percent`, `avg_memory_bytes`). With an older daemon the averages and states show as `–`.
