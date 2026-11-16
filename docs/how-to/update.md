---
title: Update and uninstall
description: Install new devlanes releases, choose how devlanes checks for them, try builds of the main branch, and remove devlanes from a machine.
kind: how-to
---

devlanes tells you when a newer release is out. Use this to install it, to stop the checks, or to remove devlanes.

## Update

```console
$ devlanes update
```

The command asks once, installs the latest release the way you installed devlanes, and restarts the daemon, which stops your running services and starts them again on the new version. A minor release lists its breaking changes first. Without a terminal, add `--yes`.

A box, a machine whose lanes are mirrored to yours, needs its own update: run the same command there. Two machines on releases with different protocols do not connect.

## Let devlanes update itself

Set [`updates.mode`](/docs/reference/config/user/updates/#updates-mode) to `auto` in the user config, then restart the daemon:

```yaml devlanes-user
updates:
  mode: auto
```

```console
$ devlanes daemon restart
```

Patch releases then install in the background, with nothing to confirm, where the way you installed devlanes allows it. They apply the next time you start devlanes at a terminal. A minor release still waits for `devlanes update`.

## Turn the update check off

Set [`updates.mode`](/docs/reference/config/user/updates/) in the user config, then restart the daemon:

```yaml devlanes-user
updates:
  mode: disable
```

```console
$ devlanes daemon restart
```

An update you ask for still works.

## Uninstall

```console
$ devlanes uninstall
```

The command stops the daemon, undoes what `devlanes setup` changed, including the CA trust, and removes the install. It then lists the devlanes home and the lane data, and deletes them only if you answer yes. `--dry-run` shows the plan without changing anything.

Containers stay in Docker with their volumes, and so do the folders services sync build output to, the MCP server registered with your agents and the skill. To remove each lane's own containers first, see [Remove lanes](lanes/clean-up.md).

## If you want to try builds of the main branch

The newest build of main that passed CI is published as the `next` build, and the install script installs it with `--channel next`. From then on, updates stay on `next` builds.

::::tabs{sync="os"}
:::tab[macOS]
```console
$ curl -fsSL https://devlanes.run/install.sh | sh -s -- --channel next
```
:::
:::tab[Linux]
```console
$ curl -fsSL https://devlanes.run/install.sh | sh -s -- --channel next
```
:::
:::tab[Windows]
```console
PS> iex "& {$(irm https://devlanes.run/install.ps1)} -Channel next"
```
:::
::::

## If you want to go back to releases

Uninstall with `--keep-data`, then install and run the setup again, as [Install devlanes](../install.md) shows.

## Related

- [Install devlanes](../install.md)
- [Remove lanes](lanes/clean-up.md)
- [Set up a coding agent](agents/set-up.md)
