---
title: devlanes completions
description: Print shell completions; lane and unit names come from the running daemon.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

```text
devlanes completions [OPTIONS] <SHELL>
```

- `<SHELL>`: Shell to generate completions for.
  - Values: `bash`, `elvish`, `fish`, `powershell`, `zsh`.

Every command also takes the [global options](README.md#global-options).
