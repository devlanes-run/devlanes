---
title: devlanes wait
description: Wait until a unit or lane is ready or failed.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

Exits 0 when ready, 1 when it failed (with the reason and log tail) and 124 when --timeout passes first [default: 180s]. A task or exec unit is waited on until its command exits: 0 when it exited 0, 1 when it failed. A server started with exec never exits; read its logs instead. A lane counts only its services.

```text
devlanes wait [OPTIONS] [TARGET]
```

- `[TARGET]`: Unit ([lane:]unit) or lane ([project@]lane) to wait for, such as api, 2:web or acme@2 [default: pick from a list].

Every command also takes the [global options](README.md#global-options).
