---
title: devlanes run
description: Run a task from devlanes.yaml and exit with its exit code.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

The task's output streams to this terminal. Ctrl-C asks before stopping a foreground task on a terminal and stops it in agent mode. Tasks with confirm: need --yes when no terminal can prompt.

```text
devlanes run [OPTIONS] [TASK]
```

- `[TASK]`: Task to run, as named under tasks: in devlanes.yaml, or one infra entry as infra.setup.NAME or infra.teardown.NAME [default: pick from a list].
- `--list`: List the tasks and the script sources they use.
- `--detach`: Start the task in the background and return.

Every command also takes the [global options](README.md#global-options).
