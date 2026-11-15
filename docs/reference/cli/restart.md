---
title: devlanes restart
description: Stop and start units again with a fresh environment, or the whole lane when none are named.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

A whole-lane restart stops everything, keeps the lane's saved profiles and services, and starts it again as a plain devlanes start does: infra, setup, tasks, services and any port change in the project file. It stops exec units and does not run them again.

```text
devlanes restart [OPTIONS] [UNIT]...
```

- `[UNIT]...`: Units to restart ([lane:]unit, a glob or +tag, such as api, 2:web, api-\* or +backend) [default: the whole lane].
- `--all-at-once`: Start every selected service immediately, bypassing the machine-wide start gate for this run.
- `--no-ui`: Print the restart as text instead of running it in the TUI.

Every command also takes the [global options](README.md#global-options).
