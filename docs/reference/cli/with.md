---
title: devlanes with
description: Start the lane, run a command against it, then stop what was started.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

Waits for ready first; when a unit fails it prints the failure and exits 1 without running the command. The command gets the lane's environment, including global_env, the profile's env and DEVLANES_PUBLIC_URL_\<SERVICE> for every service, plus NODE_EXTRA_CA_CERTS, DENO_CERT, SSL_CERT_FILE and REQUESTS_CA_BUNDLE for the devlanes CA unless the shell sets them. Afterwards devlanes writes --logs-dir, prints each unit's state, run time and average and peak CPU and memory when output is not a terminal (CI logs, pipes) and adds the same table to the GitHub step summary, stops what it started (and the daemon, if it started it) and exits with the command's exit code.

```text
devlanes with [OPTIONS] [UNIT]... -- <COMMAND>...
```

- `[UNIT]...`: Units to start ([lane:]unit, a glob or +tag, such as api, 2:web, api-\* or +backend) [default: what the lane ran before, else its default profile].
- `--profile <PROFILE>`: Start the units of this profile from devlanes.yaml; repeat to combine profiles (later ones win on env).
- `--logs-dir <DIR>`: Write the log of every unit and task the run started to DIR as text files, for CI artifacts; a unit with no output gets no file, and other files in DIR stay.
- `--keep`: Leave the lane running afterwards, for debugging.
- `--all-at-once`: Start every selected service immediately, bypassing the machine-wide start gate for this run.
- `<COMMAND>...`: Command and arguments to run once the lane is ready.

```text
Examples:
  devlanes with -- pnpm test
  devlanes with --profile e2e -- pnpm test:e2e
  devlanes with api --logs-dir devlanes-logs -- cargo test --test api
```

Every command also takes the [global options](README.md#global-options).
