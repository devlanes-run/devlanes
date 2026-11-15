---
title: devlanes update
description: Install the latest devlanes release the way devlanes was installed, then restart the daemon.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

Checks for a newer release, asks once (a minor release before 1.0 may break what worked, so it lists those changes and defaults to no), installs it, and restarts the daemon, which starts again the lanes that were running. install.sh, install.ps1 and standalone binaries download the release archive and check it against the release's SHA256SUMS; npm, pnpm, Bun, Yarn, uv, pipx and Homebrew installs run their package manager. Other installs, such as cargo or a project's own dependency, print the command to run instead. Without a terminal, pass --yes. A build of main, installed with install.sh --channel next or install.ps1 -Channel next, updates to the latest build of main.

```text
devlanes update [OPTIONS]
```

- `--check`: Only check: show the install method, the running and latest versions and what changed.

```text
Examples:
  devlanes update                      install the latest release, asking first
  devlanes update --yes                install it without asking
  devlanes update --check              show the latest release and what changed
```

Every command also takes the [global options](README.md#global-options).
