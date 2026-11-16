---
title: Update and uninstall
description: Install new devlanes releases, choose how devlanes checks for them, try builds of the main branch, and remove devlanes from a machine.
kind: how-to
---

devlanes keeps itself up to date. It installs patch releases in the background and they apply the next time you start devlanes at a terminal. Use this for a release that waits for you, to change how devlanes updates, or to remove it.

An install made with cargo, or as a dependency of a project, does not update itself. `devlanes update` says what to run or change instead.

## Update

A minor release can change how things work, so it waits for you. devlanes tells you when one is out.

```console
$ devlanes update
```

The command asks once, installs the latest release the way you installed devlanes, and restarts the daemon, which stops your running services and starts them again on the new version. A minor release lists its breaking changes first. Without a terminal, add `--yes`.

A box, a machine whose lanes are mirrored to yours, needs its own update: run the same command there. Two machines on releases with different protocols do not connect.

## Change how devlanes updates

Set [`updates.mode`](/docs/reference/config/user/updates/#updates-mode) in the user config, then restart the daemon. With `notify` devlanes only tells you about a new release, and with `disable` it never checks.

```yaml devlanes-user
updates:
  mode: notify
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
