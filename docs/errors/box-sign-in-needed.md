---
sidebar:
  order: 205
---

# DL205 Box sign-in needed

`503`. The hostname belongs to a lane on a remote box, and reconnecting to the box needs you. Either SSH asked for a password, a key passphrase or a host key confirmation while no `devlanes remote connect` was attached to answer, or SSH rejected the credentials or the host key of a box that connected before. The daemon stopped retrying. While the daemon keeps running, the box's ports stay bound on this machine and its hostnames stay routed, so requests get this page right away instead of waiting. After a daemon restart, the box's ports and hostnames appear once it connects, so until then its hostnames are unknown here ([DL101](unknown-host.md)).

## Fix

```sh
devlanes remote ls                  # the box shows needs_auth with SSH's error
devlanes remote connect my-box      # shows SSH prompts in this terminal, then reconnects
```

When SSH reports a changed host key, check the key with whoever runs the box, update `~/.ssh/known_hosts` (`ssh-keygen -R <host>`), then run `devlanes remote connect` again. A key in an SSH agent, or one without a passphrase, lets later reconnects run without a prompt. See [remote boxes](../remote.md).
