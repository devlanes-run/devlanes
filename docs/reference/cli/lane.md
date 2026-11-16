---
title: devlanes lane
description: Create, list, clean and resume worktree lanes.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

```text
devlanes lane [OPTIONS] <COMMAND>
```

## devlanes lane ls

List lanes after rereading the project's Git worktrees, including moved, unassigned and missing ones.

```text
devlanes lane ls [OPTIONS]
```

## devlanes lane new

Add another lane for the current folder. It has its own index, ports, domain and data folder. Its name is a lane reference: `--lane <name>`, `<name>:<unit>` and `lane clean <name>` reach it, as its index does. A cleaned lane's name can be used again and gives a new lane with empty data. Inside the lane, `lane.name` and DEVLANES_LANE_NAME stay `lane<N>`. Commands run in the folder keep using the folder's first lane. With --lane, adds the lane on that lane's folder.

For a lane on another branch, create a worktree with `git worktree add <path> <branch>` and run `devlanes start` in it.

Without --start, a terminal asks whether to start the new lane right away. --start then starts it as start --lane \<name> does, with this shell's environment, --env and --timeout.

```text
devlanes lane new [OPTIONS]
```

- `--name <NAME>`: Name of the extra lane, a lane reference for --lane and \<name>:unit; not a number, main or lane\<N> [default: \<folder>-\<n>].
- `--start`: Start the new lane right away.
- `--no-ui`: With --start, print the start as text instead of running it in the TUI.

## devlanes lane clean

Stop a lane, run its clean tasks and teardown, delete its data and logs, and free its ports (needs --yes).

The worktree gets a new lane, with a new, empty data folder, no run history and possibly other ports, the next time `start`, `status` or another command that acts on lanes runs in it. The primary worktree's lane comes back on the next such command for its project. Reads such as `logs` and `url ls` leave it cleaned. A lane added with `lane new` does not come back; `lane new --name <name>` creates a new one. A `lane new` lane that waits for `lanes.max` is cleaned by its name. Cleaning a project's last lane also stops its shared containers, keeping their data.

```text
devlanes lane clean [OPTIONS] [LANE]
```

- `[LANE]`: Lane to clean ([project@]lane, such as 4 or acme@chore/deps) [default: pick from a list].
- `--dry-run`: Print what the clean would stop, run and delete, with sizes, and change nothing; needs no --yes.

## devlanes lane gc

Clean lanes whose worktree or project file is gone or names another project, drop unused port offsets, and delete lane data folders a clean could not (needs --yes).

```text
devlanes lane gc [OPTIONS]
```

## devlanes lane resume

Start the lanes that were running before a reboot or daemon restart.

The daemon never stores a shell's environment, so after a restart each lane needs one start or resume from a shell. A plain start brings back only its own lane; resume uses this shell's environment for every lane that was not stopped by hand, and starts the services each one ran.

```text
devlanes lane resume [OPTIONS]
```

- `--all-at-once`: Start every selected service immediately, bypassing the machine-wide start gate for this run.
- `--no-ui`: Print the starts as text instead of running them in the TUI.

Every command also takes the [global options](README.md#global-options).
