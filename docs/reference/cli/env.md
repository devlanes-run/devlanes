---
title: devlanes env
description: Show, export or enter the environment of a lane or unit.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

```text
devlanes env [OPTIONS] <COMMAND>
```

## devlanes env show

Print the environment of the lane or of one unit as KEY=VALUE lines; agent output hides secret-looking values.

```text
devlanes env show [OPTIONS] [UNIT]
```

- `[UNIT]`: Unit whose environment to print ([lane:]unit, such as api or 2:api) [default: the lane's variables].
- `--explain`: Show where each variable comes from; secret-looking values are hidden. With a unit, also prints the unit's launch notes.

## devlanes env export

Print export KEY=VALUE lines for eval in a shell.

```text
devlanes env export [OPTIONS] [UNIT]
```

- `[UNIT]`: Unit whose environment to export ([lane:]unit) [default: the lane's variables].
- `--shell <SHELL>`: Shell syntax of the printed lines.
  - Values: `posix` (`export NAME='value'`, for sh, bash, zsh and Git Bash), `pwsh` (`$env:NAME = 'value'`, for Windows PowerShell 5.1 and PowerShell 7; the lines are ASCII and other text is decoded from Base64).
  - Default: `posix`.

```text
Examples:
  eval "$(devlanes env export api)"
  devlanes env export api --shell pwsh | Out-String | Invoke-Expression

Windows PowerShell 5.1 and PowerShell before 7.5 remove a variable that is
assigned an empty value, so variables with empty values end up unset there.
```

## devlanes env shell

Open a subshell with the lane's or one unit's environment.

```text
devlanes env shell [OPTIONS] [UNIT]
```

- `[UNIT]`: Unit whose environment to use ([lane:]unit) [default: the lane's variables].

Every command also takes the [global options](README.md#global-options).
