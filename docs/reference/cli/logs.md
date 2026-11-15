---
title: devlanes logs
description: Read unit logs; each line carries a cursor to continue from.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

Lines of several units are merged by time. Each line starts with a cursor such as api#120 that --cursor and --around accept. In agent mode and without a terminal the output defaults to the last 100 lines within the output budget.

```text
devlanes logs [OPTIONS] [UNIT]...
```

- `[UNIT]...`: Units to read ([lane:]unit, a glob or +tag, such as api, 2:web, api-\* or +backend) [default: pick from a list].
- `--all`: Read every unit of the lane, grouped by unit.
- `--tail <N>`: Show the last N lines [default: 100 in agent mode or without a terminal].
- `--head <N>`: Show the first N lines of the selected range.
- `-f, --follow`: Keep streaming new lines; agent mode requires --timeout.
- `--since <WHEN>`: Only lines newer than a duration (5m), a time in milliseconds, or start (the lane's last start).
- `--grep <REGEX>`: Only lines matching this regular expression, such as timeout|refused.
- `--level <LEVEL>`: Only lines at this level or above: debug, info, warn or error.
- `--around <CURSOR>`: Show the lines around this cursor, such as api#120.
- `-C, --context <N>`: Lines before and after --around, or around each --grep or --level match [default: 3 with --around, else 0].
- `--cursor <CURSOR>`: Continue after this cursor, as printed by an earlier logs call.
- `--mine`: Only units this agent session started.

```text
Examples:
  devlanes logs api --tail 200
  devlanes logs acme@2:api --since 5m --grep 'timeout|refused'
  devlanes logs api -f --timeout 30s
  devlanes logs --all --since start
```

Every command also takes the [global options](README.md#global-options).
