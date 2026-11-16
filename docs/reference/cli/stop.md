---
title: devlanes stop
description: Stop units, or the whole lane when none are named.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

Stopped services are left out of what a plain devlanes start brings back. A whole-lane stop also forgets the lane's profiles, so its next plain start uses the default profile.

A failed service or exec command you name, or every one when the whole lane stops, reads stopped afterwards; failed tasks keep their state.

To stop another lane, name it with --lane: devlanes stop --lane 2.

```text
devlanes stop [OPTIONS] [UNIT]...
```

- `[UNIT]...`: Units to stop ([lane:]unit, a glob or +tag, such as api, 2:web, api-\* or +backend) [default: the whole lane].
- `--mine`: Only stop units this agent session started; with no units, those in the current project, or in every project outside one.
- `--logs-dir <DIR>`: Write each unit's log of this run to DIR as text files, for CI artifacts; a unit with no output gets no file.

Every command also takes the [global options](README.md#global-options).
