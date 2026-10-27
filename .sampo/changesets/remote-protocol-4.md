---
cargo/devlanes: minor
cargo/devlanes-cli: patch
cargo/devlanes-daemon: minor
cargo/devlanes-proto: minor
cargo/devlanes-transport: minor
cargo/devlanes-transport-ssh: minor
cargo/devlanes-tunnel: minor
---

Changed the remote protocol to version 4, so `devlanes remote connect` works with boxes whose login shell or profile prints to stdout; the client and the box both need this release.

To upgrade, install this release on the box yourself (devlanes no longer installs it there) and on this machine, then run `devlanes daemon restart` on both. Each daemon upgrades its state database on that first start, and earlier releases cannot open it afterwards. Then connect each box once with a pairing code: `devlanes daemon pair` on the box, `devlanes remote connect <box> --code <code>` here.
