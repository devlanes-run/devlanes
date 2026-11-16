---
title: Trust the devlanes CA
description: Make browsers, command-line tools and language runtimes accept the certificates devlanes signs for lane hostnames, on each operating system.
kind: how-to
---

Follow these steps when a browser or a program on your machine rejects the certificate of a lane hostname.

## Choose the stores

devlanes signs each certificate with the devlanes CA, a certificate authority devlanes creates on your machine. The setup command adds the CA to the stores your user config lists, and to the system store when it lists none.

::::tabs{sync="os"}
:::tab[macOS]
The system store is your login keychain, which Safari, Chrome and Edge read, and Firefox with its default settings. Nothing to add.
:::
:::tab[Linux]
Browsers on Linux keep stores of their own, so list them next to the system store. Both need the NSS tools, `libnss3-tools` on Debian and Ubuntu.

```yaml devlanes-user
# ~/.devlanes/config.yaml
tls:
  trust: [system, chrome, firefox]
```
:::
:::tab[Windows]
The system store is your user's store, which Chrome and Edge read, and Firefox with its default settings. Nothing to add.
:::
::::

[`tls.trust`](/docs/reference/config/user/tls/#tls-trust) lists every store. Inside WSL the store to add is `windows`, as [Run devlanes in WSL](../wsl.md) shows.

## Add the CA

Run the setup command in your own terminal, since your system asks for a password or a confirmation:

```console
$ devlanes setup
```

Then check that each store trusts the CA. The doctor prints a fix under any store that does not.

```console
$ devlanes doctor
```

## Give your own commands the CA

Node, Bun, Deno, Python, Rust and Go services that devlanes starts already trust the CA. For commands you start yourself, open a shell with the same variables from the worktree:

```console
$ devlanes env shell
```

Or set them in your shell profile:

::::tabs{sync="shell"}
:::tab[bash]
```sh title="~/.bashrc"
export NODE_EXTRA_CA_CERTS="$HOME/.devlanes/ca/ca.pem"
export DENO_CERT="$HOME/.devlanes/ca/ca.pem"
export SSL_CERT_FILE="$HOME/.devlanes/ca/bundle.pem"
export REQUESTS_CA_BUNDLE="$HOME/.devlanes/ca/bundle.pem"
```
:::
:::tab[zsh]
```sh title="~/.zshrc"
export NODE_EXTRA_CA_CERTS="$HOME/.devlanes/ca/ca.pem"
export DENO_CERT="$HOME/.devlanes/ca/ca.pem"
export SSL_CERT_FILE="$HOME/.devlanes/ca/bundle.pem"
export REQUESTS_CA_BUNDLE="$HOME/.devlanes/ca/bundle.pem"
```
:::
:::tab[PowerShell]
```powershell title="$PROFILE"
$env:NODE_EXTRA_CA_CERTS = "$env:USERPROFILE\.devlanes\ca\ca.pem"
$env:DENO_CERT = "$env:USERPROFILE\.devlanes\ca\ca.pem"
$env:SSL_CERT_FILE = "$env:USERPROFILE\.devlanes\ca\bundle.pem"
$env:REQUESTS_CA_BUNDLE = "$env:USERPROFILE\.devlanes\ca\bundle.pem"
```
:::
::::

`bundle.pem` holds the system's root certificates plus the devlanes CA. Go programs read `SSL_CERT_FILE` on Linux and the system store on macOS and Windows. The curl of macOS and Linux uses the system store and needs nothing.

## If curl.exe rejects the certificate on Windows

`curl.exe` checks for a revocation list, which the devlanes CA does not publish. Add a flag that skips that check and still verifies the certificate:

```console
$ curl.exe --ssl-revoke-best-effort https://myapp.localhost/
```

## If you already trust mkcert

devlanes can sign with mkcert's CA instead of its own, so browsers that trust mkcert need nothing new:

```yaml devlanes-user
# ~/.devlanes/config.yaml
tls:
  ca: mkcert
```

```console
$ devlanes daemon restart
```

mkcert's CA can sign any name, unlike the devlanes CA; [Security](../../concepts/security.md) has what that changes.

## Related

- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
- [Install devlanes](../../install.md)
- [Run devlanes in WSL](../wsl.md)
