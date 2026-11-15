---
title: Install
description: Install devlanes, let it set up HTTPS for your machine and check that everything works.
kind: how-to
---

This page gets devlanes onto your machine and ready for a first project. You end with the command installed, its certificates trusted by your browser and a check that says so.

Your coding agent can do all of it, and set up a project as well.

```prompt
Install the devlanes skill with `npx skills add devlanes-run/devlanes --skill devlanes`. Then install devlanes on this machine and tell me which steps I have to run myself.
```

## Install the command

devlanes runs on macOS, Linux and Windows 11.

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

Windows needs version 24H2 or later and [Git for Windows](https://gitforwindows.org).
:::
::::

The script installs the latest release and checks the download against the release's checksums.

## If you prefer a package manager

Each of these installs the same release build.

::::tabs{sync="install"}
:::tab[Homebrew]
```console
$ brew install devlanes-run/tap/devlanes
```

Homebrew has no Windows build.
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
```
:::
::::

## Trust the certificates

devlanes serves every hostname over HTTPS with certificates from its own authority, which exists only on your machine. Your browser accepts them once you trust that authority.

```console
$ devlanes setup
```

It lists what your machine needs, asks once and then does it. Your operating system may ask for your password or a confirmation, which is why an agent cannot run this step for you.

On Linux, browsers keep their own certificate stores, and [Trust the CA](how-to/hostnames/trust-the-ca.md) covers those.

## Check the machine

`devlanes doctor` looks at everything a first start depends on and prints a fix next to anything that is off.

```console
$ devlanes doctor
proxy
  ✓ http on port 80
  ✓ https on port 443
tls
  ✓ CA at ~/.devlanes/ca/ca.pem
  ✓ CA trusted by system
hosts
  ✓ *.localhost resolves to loopback; no hosts file entries needed
```

The lines above are part of its output. A project with containers also needs Docker running, and `doctor` says when it is not.

## Turn on shell completions

Completions fill in commands, lanes and unit names. Add one line to your shell's startup file.

| Shell | File | Line |
|---|---|---|
| bash | `~/.bashrc` | `source <(devlanes completions bash)` |
| zsh | `~/.zshrc`, after `compinit` | `source <(devlanes completions zsh)` |
| fish | `~/.config/fish/config.fish` | `devlanes completions fish \| source` |
| PowerShell | `$PROFILE` | `devlanes completions powershell \| Out-String \| Invoke-Expression` |

## Next

- [A service, a port and a hostname](learn/first-service.md) starts the tour of what devlanes does.
- [Add devlanes to a project](how-to/projects/add-devlanes.md) puts your own app on it.
- [Update and uninstall](how-to/update.md) covers new releases and removing devlanes.
