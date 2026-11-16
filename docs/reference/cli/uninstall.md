---
title: devlanes uninstall
description: "Remove devlanes from this machine: stop the daemon, then remove its login service, what devlanes setup changed and this install."
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

In order, and going on when a step fails:

```text
1. stop the daemon (asked once up front, naming the processes it stops;
   without a terminal, pass --yes)
2. remove the login service (daemon install)
3. undo devlanes setup: the CA trust, the /etc/hosts helper and the resolver
   file or NRPT rule; sudo asks for your password at most once
4. remove this install: install.sh and install.ps1 files (and the user PATH
   entry on Windows), or the package manager's remove command. A project's
   own dependency and a source build stay.
5. the devlanes home (~/.devlanes, or $DEVLANES_HOME) and lane data: listed
   with their sizes and deleted only if you answer yes to a second question.
   With --yes and no terminal they stay unless --remove-data is given.
```

```text
devlanes uninstall [OPTIONS]
```

- `--remove-data`: Delete the devlanes home and lane data without asking.
- `--keep-data`: Keep the devlanes home and lane data, and print where they are.
- `--dry-run`: Print what each step would remove; change nothing.

```text
Examples:
  devlanes uninstall --dry-run         show what would be removed
  devlanes uninstall                   remove devlanes, asking first and again before deleting data
  devlanes uninstall --yes --keep-data
```

Every command also takes the [global options](README.md#global-options).
