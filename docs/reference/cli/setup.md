---
title: devlanes setup
description: Trust the devlanes CA and set up hostnames for this machine; asks once.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

Works out what this machine needs and lists each step as ok, todo, info or skip:

```text
ca        trust the CA in the stores tls.trust names (mkcert's CA with tls.ca: mkcert)
hosts     install the /etc/hosts helper (macOS and Linux) when *.localhost does not
          resolve to loopback, the user config asks for the hosts file, or the
          daemon needs it for a project
resolver  send hostnames.tld to the devlanes DNS server (a resolver file, or an
          NRPT rule on Windows) when hostnames.resolver is resolver
browser   register the browser extension's host for the installed browsers on
          its list, such as Chrome, Edge and Brave, and keep a copy of its
          manifest in ~/.devlanes/browser for other Chromium browsers. It shows
          as info, never todo, and doesn't change --check's exit code
```

Then it asks once before doing the todo steps, and registers the browser host with them. With nothing else to do, it registers the host without asking. sudo asks for your password at most once. Without a terminal, pass --yes.

```text
devlanes setup [OPTIONS]
```

- `--check`: Only report each step; exit 0 when nothing is missing, 1 otherwise.
- `--remove`: Undo everything: the resolver file or NRPT rule, the /etc/hosts helper, this install's browser host registrations and the CA trust.
- `--keep-ca`: With --remove, keep the CA trusted.

```text
Examples:
  devlanes setup                       set up what is missing, asking once
  devlanes setup --check               exit 1 when something is missing
  devlanes setup --remove --keep-ca    remove the hosts helper, resolver and browser host, keep the CA
```

Every command also takes the [global options](README.md#global-options).
