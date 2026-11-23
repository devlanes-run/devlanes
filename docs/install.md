---
title: Install devlanes
description: Install devlanes, set up HTTPS for your machine and check that it is ready.
kind: how-to
---

devlanes is a single command. Its background daemon starts by itself the first time you use it.

Your coding agent can install it for you, apart from one dialog your operating system shows you.

```prompt
Install the devlanes skill:
npx skills add devlanes-run/devlanes --skill devlanes

Then install devlanes on this machine, set up everything it needs,
and help me port this project to devlanes from whatever runs it today.
```

## Install the command

devlanes runs on macOS, on Linux with glibc, and on Windows 11 version 24H2 or later with [Git for Windows](https://gitforwindows.org). Install it with the package manager you already use.

::::tabs{sync="install"}
:::tab[Homebrew]
```console
$ brew install devlanes-run/tap/devlanes
```

Homebrew has builds for macOS and Linux.
:::
:::tab[npm]
```console
$ npm install -g devlanes
```
:::
:::tab[uv]
```console
$ uv tool install devlanes
```
:::
:::tab[cargo]
```console
$ cargo binstall --git https://github.com/devlanes-run/devlanes devlanes
$ cargo binstall --git https://github.com/devlanes-run/devlanes devlanes-hosts-helper
```
:::
::::

You install it once. devlanes then keeps itself on the latest patch release in the background, and [Update and uninstall](how-to/update.md) covers the rest.

## Or use the install script

::::tabs{sync="os"}
:::tab[macOS]
```console
$ curl -fsSL https://devlanes.run/install.sh | sh
```
:::
:::tab[Linux]
```console
$ curl -fsSL https://devlanes.run/install.sh | sh
```
:::
:::tab[Windows]
```console
PS> irm https://devlanes.run/install.ps1 | iex
```

Open a new terminal afterwards.
:::
::::

## Set up HTTPS and hostnames

devlanes serves each hostname over HTTPS with certificates signed by the devlanes CA, a certificate authority it creates on your machine. The CA can only sign names meant for local development, such as those ending in `.localhost`.

```console
$ devlanes setup
```

The command lists what it is about to change, asks once and adds the CA to your system's trust store. Your operating system then asks for a password or a confirmation. `devlanes setup --remove` undoes it.

Setup also registers devlanes as the native host of the devlanes browser extension, for Chrome, Chromium, Brave and Edge. On macOS and Linux it writes a small file into the folder of each of these browsers you have. On Windows it writes one file in `~\.devlanes\browser` and a key per browser under `HKCU\Software`. The registration only lets the extension start devlanes, and it needs no password. `devlanes setup --remove` deletes the registrations that start this install.

Browsers on Linux and some command-line tools keep certificate stores of their own. [Trust the devlanes CA](how-to/hostnames/trust-the-ca.md) covers those.

## Check the machine

```console
$ devlanes doctor
```

The doctor checks what a first start depends on and prints a fix under each line that is off. A project with containers also needs Docker running.

## If a project pins devlanes

A JavaScript project can list devlanes as a dependency, so everyone who works on it runs the same release:

```console title="~/dev/myapp"
$ npm install --save-dev devlanes
$ npx devlanes start
```

The command is then not on your `PATH`, so you run it through the package manager, as the second line does; pnpm, Yarn and Bun have their own form, such as `pnpm devlanes`.

One daemon serves every project on the machine, so it can run another release than the one the project pins. The doctor shows both versions when they differ. To move the daemon to the pinned release, restart it from the project, which stops your running services and starts them again:

```console title="~/dev/myapp"
$ npx devlanes daemon restart
```

## If you want shell completions

Add the line for your shell to its startup file.

::::tabs{sync="shell"}
:::tab[bash]
```sh title="~/.bashrc"
eval "$(devlanes completions bash)"
```
:::
:::tab[zsh]
```sh title="~/.zshrc"
autoload -Uz compinit && compinit
source <(devlanes completions zsh)
```
:::
:::tab[fish]
```fish title="~/.config/fish/config.fish"
devlanes completions fish | source
```
:::
:::tab[PowerShell]
```powershell title="$PROFILE"
devlanes completions powershell | Out-String | Invoke-Expression
```
:::
::::

## Related

- [A service, a port and a hostname](learn/first-service.md)
- [Add devlanes to a project](how-to/projects/add-devlanes.md)
- [Update and uninstall](how-to/update.md)
