---
title: Trust the devlanes CA
description: Make browsers, command-line tools and language runtimes accept the certificates devlanes signs for lane hostnames, on each operating system.
kind: how-to
---

Follow these steps when a program on your machine rejects the certificate of a lane hostname, or before you open lanes in a browser that keeps its own store.

## Choose the stores

devlanes signs each certificate with the devlanes CA, a certificate authority devlanes creates on your machine. The user config lists the stores that the setup command adds the CA to. Without the setting it uses the system store, which covers Chrome, Edge and Safari on macOS and Chrome and Edge on Windows.

::::tabs{sync="os"}
:::tab[macOS]
The system store is your login keychain, which Safari, Chrome, Edge and the curl in `/usr/bin` read.

Firefox reads it while its `security.enterprise_roots.enabled` preference is on. If Firefox still rejects the certificate, add `firefox` to the list; it needs the NSS tools from Homebrew's nss package.
:::
:::tab[Linux]
Browsers on Linux do not read the system store, so list their stores next to it:

```yaml devlanes-user
# ~/.devlanes/config.yaml
tls:
  trust: [system, chrome, firefox]
```

`chrome` is the store in `~/.pki/nssdb`, which Chrome and Playwright's Chromium read.

`firefox` covers each Firefox profile, including those of the snap and Flatpak packages. It reaches only the profiles that exist when you run the setup command, so run it again after you create one.

Both need the NSS tools from your distribution:

- Debian and Ubuntu: `libnss3-tools`
- Fedora: `nss-tools`
- Arch: `nss`
:::
:::tab[Windows]
On Windows every entry in the list is the same store, the one your Windows user keeps trusted certificate authorities in, which Chrome and Edge read. Adding to it needs no administrator rights. Firefox reads it while its `security.enterprise_roots.enabled` preference is on, so check that preference if Firefox rejects the certificate.
:::
::::

[`tls.trust`](/docs/reference/config/user/tls/) lists every store name. Inside WSL, where the browser runs on Windows, the store to add is `windows`, as [Run devlanes in WSL](../wsl.md) shows.

## Add the CA to them

Run the setup command in your own terminal, since the operating system asks you for a password or a confirmation.

```console
$ devlanes setup
```

On Windows a security warning asks you to confirm each change to the store, and the command fails over SSH or anywhere without a desktop. [Install devlanes](../../install.md) says what else the command changes and how to undo it.

## Check each store

The doctor command reports the CA's file and, for each store in the list, whether that store trusts the CA. A store that does not gets a line with a fix under it.

::::tabs{sync="os"}
:::tab[macOS]
```console
$ devlanes doctor
```
:::
:::tab[Linux]
```console
$ devlanes doctor
```
:::
:::tab[Windows]
```console
$ devlanes doctor
tls
  ✓ CA at C:\Users\you\.devlanes\ca\ca.pem
  ✓ CA trusted by system
```
:::
::::

When the doctor reports every store as trusting the CA and a browser still rejects the certificate, that browser reads a store the list leaves out, such as Firefox on Linux without `firefox`.

## Run your own commands with the lane's variables

Services that devlanes runs as Node, Bun, Deno, Python or Rust get variables that point their runtime at the CA. Commands you start yourself get the same variables when you run them through `devlanes exec` or `devlanes with`, or inside the shell that this command opens when you run it in the worktree:

```console
$ devlanes env shell
```

## Set the variables in your shell profile

To have them in every shell, point runtimes that add to their trusted certificates at `ca.pem`, and runtimes that replace them at `bundle.pem`, which holds the system's root certificates plus the devlanes CA.

::::tabs{sync="shell"}
:::tab[bash and zsh]
```sh title="~/.bashrc"
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

Node and Bun read the first variable and Deno the second. Go programs use the system store, so the system entry covers them.

Python's ssl module and httpx read `SSL_CERT_FILE`, and requests reads `REQUESTS_CA_BUNDLE`.

Rust programs read `SSL_CERT_FILE` when they use OpenSSL or rustls-native-certs. Programs with certificates compiled in ignore it.

## If curl rejects the certificate

::::tabs{sync="os"}
:::tab[macOS]
The curl in `/usr/bin` reads the keychain. Homebrew's curl reads a bundle of its own, so give it the devlanes bundle through `SSL_CERT_FILE` as above.
:::
:::tab[Linux]
The curl of your distribution reads the system store, so the system entry covers it. A curl built against another bundle takes `SSL_CERT_FILE` as above.
:::
:::tab[Windows]
`curl.exe` reads the Windows store but also checks for a revocation list, which the devlanes CA does not publish. Add `--ssl-revoke-best-effort`, which still verifies the chain:

```console
$ curl.exe -s -i --ssl-revoke-best-effort https://myapp.localhost/
HTTP/1.1 200 OK
content-type: text/html
via: 1.1 devlanes.run
```
:::
::::

## If you use Java

A JDK from your Linux distribution reads the system store, so the system entry covers it. Other JDKs, and every JDK on macOS and Windows, keep their own store. Import the CA into the store of the JDK you run, answer yes when keytool asks whether to trust it, and give the store's password, `changeit` unless someone changed it:

::::tabs{sync="shell"}
:::tab[bash and zsh]
```console
$ keytool -importcert -cacerts -alias devlanes -file "$HOME/.devlanes/ca/ca.pem"
```
:::
:::tab[PowerShell]
```console
PS> keytool -importcert -cacerts -alias devlanes -file "$env:USERPROFILE\.devlanes\ca\ca.pem"
```
:::
::::

A JDK installed for every user needs sudo or an administrator shell for this.

## If the program runs in a container

A container has its own store and does not resolve lane hostnames to your machine; [Fix programs that cannot resolve *.localhost](resolve-localhost.md#if-the-program-runs-in-a-container) covers containers.

## If you already trust mkcert

devlanes can sign with mkcert's CA instead of creating its own, so browsers that trust mkcert's CA need nothing new. Set it in the user config:

```yaml devlanes-user
# ~/.devlanes/config.yaml
tls:
  ca: mkcert
```

Then restart the daemon so it loads mkcert's CA:

```console
$ devlanes daemon restart
```

mkcert has to be on the daemon's `PATH`, or HTTPS stays off. mkcert's CA has no limit on the names it can sign, unlike the devlanes CA, and `devlanes setup --remove` leaves it trusted; [Security](../../concepts/security.md) has what that changes.

## Related

- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
- [Security](../../concepts/security.md)
- [Run devlanes in WSL](../wsl.md)
