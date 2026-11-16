---
title: devlanes start
description: "Bring up a lane: infra, setup, tasks and services, then wait until ready."
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

Starts the lane of the current worktree, or only the named units. Units that already run are left alone, so starting twice is safe. Waits until every started unit is ready or one fails; Ctrl-C detaches and leaves everything running.

Without units or --profile it brings back the services the lane ran before, such as after a reboot or daemon crash. A lane stopped with devlanes stop has nothing to bring back and starts its default profile; start.restore: false in \~/.devlanes/config.yaml always does that.

```text
devlanes start [OPTIONS] [UNIT]...
```

- `[UNIT]...`: Units to start ([lane:]unit, a glob or +tag, such as api, 2:web, api-\* or +backend) [default: what the lane ran before, else its default profile].
- `--profile <PROFILE>`: Start the units of this profile from devlanes.yaml; repeat to combine profiles (later ones win on env).
- `--attach`: Run the start in the TUI even when DEVLANES_NO_UI or tui.open_on_start turn that off [default on a terminal].
- `--no-ui`: Print the start as text instead of running it in the TUI.
- `--all-at-once`: Start every selected service immediately, bypassing the machine-wide start gate for this run.

```text
Examples:
  devlanes start                       what this worktree's lane ran before
                                       (its default profile after devlanes stop)
  devlanes start api web               only these services
  devlanes start --profile e2e         the units of the e2e profile
  devlanes start 2:+api --timeout 5m   units tagged api in lane 2
```

Every command also takes the [global options](README.md#global-options).
