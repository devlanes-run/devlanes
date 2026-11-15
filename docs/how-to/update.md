---
title: Update and uninstall
description: Install new devlanes releases, choose how devlanes checks for them, try builds of the main branch, and remove devlanes from a machine.
kind: how-to
---

devlanes tells you when a newer release is out, and one command installs it the way devlanes was installed and brings your lanes back on the new version. Removing devlanes is one command as well, and it asks before it deletes your data.

## Update to the latest release

Run `devlanes update`. It checks for a newer release and asks once. A patch release defaults to yes, while a minor release, which before 1.0 may break what worked, lists its breaking changes and defaults to no. Without a terminal, pass `--yes`.

It then installs the release and restarts the daemon, which stops the running services and starts them again on the new version. How it installs depends on how you installed devlanes:

- The install scripts and standalone binaries: it downloads the release and checks it against the release's checksums.
- npm, pnpm, Bun, Yarn, uv, pipx and Homebrew: it runs the package manager.
- cargo, a project's own dependency and the other installs it cannot replace: it prints the command to run yourself.

`devlanes update --check` shows the latest release and what changed, and installs nothing. devlanes installs the latest release only.

## Turn the update check off

The daemon asks the registry you installed from for the latest version every few hours, and each command in a terminal mentions a newer release on one line, at most once a day. [Security](../concepts/security.md) says what those requests carry. To stop the checks, set [`updates.mode`](/docs/reference/config/user/updates/) in the user config:

```yaml devlanes-user
updates:
  mode: disable
```

The daemon reads the setting when it starts, so run `devlanes daemon restart` afterwards. An update you ask for still works with the checks off, and in CI they are off already. The value `auto` goes the other way: it also installs patch releases in the background, and they take effect the next time you open devlanes in a terminal.

## Update a box together with your machine

A box is a machine whose lanes are mirrored to yours, and devlanes does not update it for you. Two machines that run releases with different protocols do not connect, and the error says to install the same release on both. Releases with the same protocol connect with a warning.

:::steps
1. On the box, run `devlanes update`.
2. On your machine, run `devlanes update`.
:::

## Try builds of the main branch

Each commit on the main branch of devlanes is built and published as a `next` build. The install scripts install one with the next channel:

::::tabs{sync="os"}
:::tab[macOS and Linux]
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

Over a script install, it takes the place of the release. From then on `devlanes update` installs the latest build of main and does not go back to releases by itself. A Homebrew, npm or PyPI install has to be removed first, because the scripts stop when another installer's devlanes comes first on your path.

To go back to releases, uninstall and keep your data, then install again as [Install devlanes](../install.md) shows:

```console
$ devlanes uninstall --keep-data
```

## Uninstall

`devlanes uninstall` asks once, naming the processes the daemon runs, then goes through these steps and carries on when one fails:

- It stops the daemon and removes its login service.
- It undoes what `devlanes setup` changed: the CA trust, the hosts file helper and the resolver.
- It removes the install: the files the install scripts placed and, on Windows, the user `PATH` entry, or the package manager's remove command.
- It lists the devlanes home and the lane data with their sizes, and deletes them only if you answer yes to a second question.

To see the plan for this machine first, `--dry-run` prints it and changes nothing.

Without a terminal the command needs `--yes`, and the data stays unless you add `--remove-data`.

## If you want nothing left behind

The daemon leaves containers running when it stops, so the containers your lanes started stay in Docker with their volumes. Clean the lanes before you uninstall, as [Remove lanes](lanes/clean-up.md) shows, or remove the containers with Docker afterwards. The project files in your repositories stay, and so does a devlanes that a project lists as a dependency. Remove the skill from your agents with `npx skills remove devlanes`.

## Related

- [Install devlanes](../install.md)
- [Remove lanes](lanes/clean-up.md)
- [Prepare a box](remote/prepare-a-box.md)
