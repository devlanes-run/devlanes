---
title: devlanes exec
description: Run an ad-hoc command as a unit of the lane instead of backgrounding it with &.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

The daemon owns the process: logs, stop, attach and status work on it by its --name. It gets the shell's environment, then the lane's: devlanes variables, DEVLANES_PUBLIC_URL_\<SERVICE>, and the global_env and profile env entries that read neither self nor replica; one that reads infra that is not running is left out and named. NODE_EXTRA_CA_CERTS, DENO_CERT, SSL_CERT_FILE and REQUESTS_CA_BUNDLE point at the devlanes CA unless the shell sets them, and --env wins over all of these.

```text
devlanes exec [OPTIONS] --name <NAME> -- <COMMAND>...
```

- `--name <NAME>`: Unit name for the command, such as web, used by logs, stop and attach.
- `--no-pty`: Use pipes instead of a pseudoterminal.
- `<COMMAND>...`: Command and arguments to run.

Every command also takes the [global options](README.md#global-options).
