---
title: devlanes status
description: Show lanes and units with state, ports, URLs, CPU and memory.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

Exits 1 while a lane or unit it shows has failed, a task or exec command that exited non-zero included. devlanes stop NAME clears a failed service or exec command once you have read why; running a unit again also replaces its failure.

```text
devlanes status [OPTIONS] [LANE]
```

- `[LANE]`: Lane to show ([project@]lane, such as 2, feat/checkout or acme@2) [default: every lane of the current project].
- `--watch`: Keep printing lane changes until interrupted.
- `--mine`: Only show units this agent session started.
- `--statusline`: Print one line for this directory's lane, for an agent's status line (reads Claude Code's JSON on stdin and never starts the daemon).

```text
Examples:
  devlanes status                      lanes of this project
  devlanes status acme@2               one lane with its units
  devlanes status --statusline         one line, such as: acme@1 3/3 ready · web acme1.localhost

Claude Code shows that line under its prompt with this in ~/.claude/settings.json:
  "statusLine": {"type": "command", "command": "devlanes status --statusline"}
```

Every command also takes the [global options](README.md#global-options).
