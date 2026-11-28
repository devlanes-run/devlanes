---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-cli: minor
cargo/devlanes-config: minor
cargo/devlanes-core: minor
cargo/devlanes-proto: minor
cargo/devlanes-transport: minor
cargo/devlanes-transport-iroh: minor
cargo/devlanes-state: minor
cargo/devlanes-mcp: minor
---

Added `devlanes remote connect <ticket>`, which pairs with a box and connects to it peer to peer, without SSH.

Run `devlanes daemon pair` on the box, then `devlanes remote connect` here with the ticket it prints. The box is reachable only while its daemon runs, and `devlanes daemon install` starts it at login. [Connect to a box with a ticket](https://devlanes.run/docs/how-to/remote/connect-by-ticket/) has the steps. On a terminal, `devlanes daemon pair` now prints a ticket before the code. The state database gains tables for these pairings, so an older devlanes cannot open it once this release has run.

<!-- highlight
title: Connect to a box without SSH
body: Run `devlanes daemon pair` on the box, then `devlanes remote connect <ticket>` here with the ticket it prints. The two machines connect peer to peer, with no SSH account on the box.
-->
