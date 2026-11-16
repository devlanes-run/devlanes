---
title: devlanes url
description: Print, open and list service URLs and routed hostnames.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

```text
devlanes url [OPTIONS] <COMMAND>
```

## devlanes url print

Print a service's URL, for scripts.

```text
devlanes url print [OPTIONS] [UNIT]
```

- `[UNIT]`: Service whose URL to print ([lane:]unit, such as web or 2:web) [default: pick from a list].

## devlanes url open

Open a service's URL in the browser; agent mode only prints it.

```text
devlanes url open [OPTIONS] [UNIT]
```

- `[UNIT]`: Service to open ([lane:]unit) [default: pick from a list].

## devlanes url ls

List the current project's routed hostnames with their lane, unit, port and state; outside a project, every project's.

```text
devlanes url ls [OPTIONS]
```

Every command also takes the [global options](README.md#global-options).
