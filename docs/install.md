---
title: Install devlanes
description: Install devlanes, set up HTTPS for your machine and check that it is ready.
kind: how-to
---

devlanes is a single command. Its background daemon starts by itself the first time you use it, so installing means getting the command and then letting it set up HTTPS on your machine.

Your coding agent can do this for you, apart from one dialog your operating system shows you.

```prompt
Install the devlanes skill with `npx skills add devlanes-run/devlanes --skill devlanes`. Then install devlanes on this machine and tell me which steps I have to do myself.
```

## Install the command

::::tabs{sync="os"}
:::tab[macOS]
```console
$ curl -fsSL https://devlanes.run/install.sh | sh
```

The command goes into `~/.local/bin`, and the script tells you when that folder is not on your `PATH`.
:::
:::tab[Linux]
```console
$ curl -fsSL https://devlanes.run/install.sh | sh
```

The command goes into `~/.local/bin`, and the script tells you when that folder is not on your `PATH`. The Linux build needs glibc, so Alpine and other musl systems are not supported.
:::
:::tab[Windows]
```console
PS> irm https://devlanes.run/install.ps1 | iex
```

Open a new terminal afterwards so it finds the command. devlanes needs Windows 11 version 24H2 or later and [Git for Windows](https://gitforwindows.org).
:::
::::

Both scripts install the latest release and compare the download with the checksums published for it.

## Set up HTTPS and hostnames

devlanes serves each hostname over HTTPS with certificates signed by the devlanes CA, a certificate authority it creates on your machine in `~/.devlanes/ca`. The CA can only sign names meant for local development, such as those ending in `.localhost`.

```console
$ devlanes setup
```

The command lists what it is about to change and asks once. It adds the CA to your system's trust store. On macOS and Linux it also installs a small helper for the hosts file, when your system does not resolve `.localhost` hostnames by itself. Your operating system then asks for a password or a confirmation, and that part needs you.

`devlanes setup --remove` undoes all of it.

On Linux, browsers keep certificate stores of their own. [Trust the devlanes CA](how-to/hostnames/trust-the-ca.md) covers those, along with command-line tools and language runtimes.

## Check the machine

The doctor command checks what a first start depends on and prints a fix under each line that is off. The proxy, the certificates and hostname lookups are the parts to look at.

```console title="on Windows 11"
$ devlanes doctor
proxy
  ✓ http on port 80
  ✓ https on port 443
tls
  ✓ CA at C:\Users\you\.devlanes\ca\ca.pem
  ✓ CA trusted by system
hosts
  ✓ *.localhost resolves to loopback; no hosts file entries needed
```

When the proxy cannot bind port 443 or 80, because another program holds it or because Linux keeps ports below 1024 for root, it uses 8443 and 8080 instead, and URLs carry the port. [Choose the proxy's ports](how-to/hostnames/proxy-ports.md) changes that. A project with containers also needs Docker running.

## Turn on shell completions

Completions fill in commands, lanes and units as you type. Add the lines for your shell to its startup file.

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

## With a package manager

Homebrew, npm and uv install the release build the scripts install.

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

This needs cargo-binstall. The second line adds the hosts helper, which the other installs bring with them.
:::
::::

## Related

- [A service, a port and a hostname](learn/first-service.md)
- [Add devlanes to a project](how-to/projects/add-devlanes.md)
- [Update and uninstall](how-to/update.md)
