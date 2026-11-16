---
title: devlanes debug
description: "Explain failures: why a unit is not up, errors, events and resource use."
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

```text
devlanes debug [OPTIONS] <COMMAND>
```

## devlanes debug why

Explain why a unit is not running or not ready, with a fix for each reason.

```text
devlanes debug why [OPTIONS] [UNIT]
```

- `[UNIT]`: Unit to explain ([lane:]unit, such as api or 2:web) [default: pick from a list].

## devlanes debug errors

List failed units, crash counts, recent error lines and port conflicts.

```text
devlanes debug errors [OPTIONS]
```

- `--since <DURATION>`: Count crashes and error lines from this long ago, such as 5m or 2h [default: all recorded runs].

## devlanes debug events

Print the daemon's event stream: lane, unit, task and port events.

```text
devlanes debug events [OPTIONS]
```

- `-f, --follow`: Keep streaming until interrupted [default: print recent events].

## devlanes debug top

Show CPU and memory of the running units and containers of the current project, or of every project outside one.

```text
devlanes debug top [OPTIONS]
```

## devlanes debug freeze

Freeze this lane's processes and own containers now, as if it had been idle for `lanes.freeze_after`.

```text
devlanes debug freeze [OPTIONS] [LANE]
```

- `[LANE]`: Lane to freeze ([project@]lane) [default: the current lane].

## devlanes debug thaw

Resume a throttled or frozen lane.

```text
devlanes debug thaw [OPTIONS] [LANE]
```

- `[LANE]`: Lane to thaw ([project@]lane) [default: the current lane].

## devlanes debug config

Print the lane's project config as YAML after merging its files and templates; expressions stay unevaluated.

```text
devlanes debug config [OPTIONS]
```

## devlanes debug bundle

Write a redacted archive to attach to a bug report and print its path.

The archive holds the client and daemon versions, doctor, config check and status as JSON, the last lines of the daemon's log and, with -- ARGS, the output of devlanes -vv ARGS. Values of secret-looking variables, URL passwords and the home directory are replaced before anything is written; read the files before sending them anyway.

```text
devlanes debug bundle [OPTIONS] [-- <ARGS>...]
```

- `--to <PATH>`: Write the archive here [default: devlanes-debug-\<time>.tar.gz in the temporary directory].
- `--log-lines <N>`: Lines of the daemon's log to include.
  - Default: `500`.
- `[ARGS]...`: devlanes arguments to run again with -vv and include, such as: -- start api.

```text
Examples:
  devlanes debug bundle
  devlanes debug bundle --to bug.tar.gz -- start api
```

Every command also takes the [global options](README.md#global-options).
