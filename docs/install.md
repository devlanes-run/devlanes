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

Then install devlanes on this machine if it is missing, set up its
HTTPS certificates and hostnames, and check the machine with
`devlanes doctor`. Tell me which steps I have to do myself.
```

## Install the command

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

The Linux build needs glibc, so Alpine and other musl systems are not supported.
:::
:::tab[Windows]
```console
PS> irm https://devlanes.run/install.ps1 | iex
```

Open a new terminal afterwards. devlanes needs Windows 11 version 24H2 or later and [Git for Windows](https://gitforwindows.org).
:::
::::

## Set up HTTPS and hostnames

devlanes serves each hostname over HTTPS with certificates signed by the devlanes CA, a certificate authority it creates on your machine. The CA can only sign names meant for local development, such as those ending in `.localhost`.

```console
$ devlanes setup
```

The command lists what it is about to change, asks once and adds the CA to your system's trust store. Your operating system then asks for a password or a confirmation. `devlanes setup --remove` undoes it.

Browsers on Linux and some command-line tools keep certificate stores of their own. [Trust the devlanes CA](how-to/hostnames/trust-the-ca.md) covers those.

## Check the machine

```console
$ devlanes doctor
```

The doctor checks what a first start depends on and prints a fix under each line that is off. A project with containers also needs Docker running.

## If you use a package manager

::::tabs{sync="install"}
:::tab[Homebrew]
```console
$ brew install devlanes-run/tap/devlanes
```
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
