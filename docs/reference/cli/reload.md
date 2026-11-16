---
title: devlanes reload
description: Apply project file and .env changes to the running lane.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

Stops the units the file removed, restarts the ones it changed and starts the ones it added. Without units, a change to infra or ports restarts the whole lane instead. With units, it applies only their changes. devlanes status shows what changed. A whole-lane restart stops exec units and does not run them again.

```text
devlanes reload [OPTIONS] [UNIT]...
```

- `[UNIT]...`: Apply only these units' changes ([lane:]unit, such as api or 2:web) [default: every change in the lane].
- `--all-at-once`: Start every selected service immediately, bypassing the machine-wide start gate for this run.
- `--no-ui`: Print the reload as text instead of running it in the TUI.

Every command also takes the [global options](README.md#global-options).
