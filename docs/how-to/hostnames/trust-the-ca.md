---
title: Trust the devlanes CA
description: Make browsers, command-line tools and language runtimes accept the certificates devlanes signs for lane hostnames, on each operating system.
kind: how-to
---

Follow these steps when a browser, a command-line tool or a runtime rejects the certificate of a lane hostname, or before you open lanes in a browser that keeps its own certificate store.

## Choose the stores

The user config lists the stores that the setup command adds the devlanes certificate authority to. Without the setting it uses the system store, which is enough on macOS and Windows.

::::tabs{sync="os"}
:::tab[macOS]
The system store is your login keychain. Safari, Chrome, Edge and the curl in `/usr/bin` read it, and so does Firefox from version 120. For an older Firefox, add `firefox`, which needs the NSS tools from Homebrew's nss package.
:::
:::tab[Linux]
Browsers on Linux do not read the system store, so list their stores next to it:

```yaml devlanes-user
# ~/.devlanes/config.yaml
tls:
  trust: [system, chrome, firefox]
```

`chrome` is the database in `~/.pki/nssdb` that Chrome, Chromium and Playwright's Chromium read.

`firefox` covers each Firefox profile, including those of the snap and Flatpak packages. It reaches only the profiles that exist when you run the setup command, so run it again after you create one.

Both need the NSS tools from your distribution:

- Debian and Ubuntu: `libnss3-tools`
- Fedora: `nss-tools`
- Arch: `nss`
:::
:::tab[Windows]
Every store name means your user's root store on Windows, which Chrome, Edge and Firefox 120 or later read. Adding to it needs no administrator rights.
:::
::::

[`tls.trust`](/docs/reference/config/user/tls/) lists every store name. Inside WSL, where the browser runs on Windows, the store to add is `windows`, as [Run devlanes in WSL](../wsl.md) shows.

## Add the authority to them

Run the setup command in your own terminal, since the operating system asks you for a password or a confirmation.

```console
$ devlanes setup
```

On Windows a security warning asks you to confirm each change to the store, and the command fails over SSH or anywhere without a desktop. [Install devlanes](../../install.md) lists everything the command changes and how `devlanes setup --remove` undoes it.

## Check each store

The doctor command reports the authority's file and, for each store you listed, whether that store trusts it.

```console title="on Windows 11"
$ devlanes doctor
tls
  ✓ CA at C:\Users\you\.devlanes\ca\ca.pem
  ✓ CA trusted by system
```

A store that does not trust the authority gets a line with a fix under it. A browser that still shows `ERR_CERT_AUTHORITY_INVALID` reads a store that is not in the list.

## Point runtimes at the authority

Services that devlanes starts need nothing from you: Node, Bun, Deno, Python and Rust services get variables that point their runtime at the authority. Commands you run yourself get the same variables in the shell that `devlanes env shell` opens, or when you run them through `devlanes exec`.

```console
$ devlanes env shell
```

To set them in your own shell profile instead, point runtimes that add roots at `ca.pem` and runtimes that replace their roots at `bundle.pem`, which holds the system's roots plus the devlanes authority.

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

Node and Bun read the first variable and Deno the second. Python's ssl module, requests, httpx and Rust programs read the last two. Go programs use the system store, so the system entry covers them.

## If curl rejects the certificate

::::tabs{sync="os"}
:::tab[macOS]
The curl in `/usr/bin` reads the keychain. Homebrew's curl reads a bundle of its own, so give it the devlanes bundle through `SSL_CERT_FILE` as above.
:::
:::tab[Linux]
The curl of your distribution reads the system store, so the system entry covers it. A curl built against another bundle takes `SSL_CERT_FILE` as above.
:::
:::tab[Windows]
`curl.exe` reads the Windows store but also checks for a revocation list, which the devlanes authority does not publish. Add `--ssl-revoke-best-effort`, which still verifies the chain:

```console
$ curl.exe -s -i --ssl-revoke-best-effort https://myapp.localhost/
HTTP/1.1 200 OK
content-type: text/html
via: 1.1 devlanes.run
```
:::
::::

## If you use Java

Java keeps its own store. Import the authority into the store of the JDK you run. keytool asks for the store's password, which is `changeit` unless someone changed it, and a JDK installed for every user needs sudo or an administrator shell.

```console
$ keytool -importcert -cacerts -alias devlanes -file ~/.devlanes/ca/ca.pem
```

## If the program runs in a container

A container reads only the certificates in its image. Mount `ca.pem` into it and add it to the image's store, as `update-ca-certificates` does on Debian-based images. The container also cannot resolve lane hostnames to your machine; [Fix programs that cannot resolve *.localhost](resolve-localhost.md) covers that part.

## If you already trust mkcert

devlanes can sign with mkcert's root instead of creating its own authority, so browsers that trust mkcert need nothing new. Set it in the user config, then run `devlanes daemon restart`:

```yaml devlanes-user
# ~/.devlanes/config.yaml
tls:
  ca: mkcert
```

mkcert has to be on your `PATH`, or HTTPS stays off. Its root has no limit on the names it can sign, unlike the devlanes authority; [Security](../../concepts/security.md) has what that changes.

## Related

- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
- [Security](../../concepts/security.md)
- [Run devlanes in WSL](../wsl.md)
