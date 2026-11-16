---
title: devlanes infra
description: Start, stop and inspect the project's containers.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

```text
devlanes infra [OPTIONS] <COMMAND>
```

## devlanes infra up

Start shared and per-lane containers and wait until they are healthy.

```text
devlanes infra up [OPTIONS]
```

## devlanes infra down

Stop the project's shared containers; their data is kept.

```text
devlanes infra down [OPTIONS]
```

## devlanes infra status

Show the containers' health and ports.

```text
devlanes infra status [OPTIONS]
```

## devlanes infra logs

Print a container's Docker logs; replicas of a compose service are merged.

```text
devlanes infra logs [OPTIONS] <CONTAINER>
```

- `<CONTAINER>`: Compose service or infra.containers name.
- `-n, --tail <N>`: Show the last N lines.
  - Default: `200`.
- `-f, --follow`: Keep streaming new lines, also across container restarts; agent mode requires --timeout.

Every command also takes the [global options](README.md#global-options).
