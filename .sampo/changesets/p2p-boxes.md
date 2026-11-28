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
cargo/devlanes-tui: minor
---

Added `devlanes remote connect <ticket>`, which pairs with a box and connects to it peer to peer, without SSH.

Run `devlanes daemon pair` on the box, then `devlanes remote connect <ticket>` here. [Connect to a box with a ticket](https://devlanes.run/docs/how-to/remote/connect-by-ticket/) has the steps. On a terminal, `devlanes daemon pair` now prints a ticket before the code and starts peer-to-peer networking, which stops soon after the ticket expires, unless a machine pairs with it. A `remote.boxes` name longer than 63 characters now fails the config. An older devlanes cannot open the state database once this release has run.

<!-- highlight
title: Connect to a box without SSH
body: On the box, press p in the Boxes view (or run `devlanes daemon pair`); here, press n and paste the ticket. The two machines connect peer to peer, without SSH.
-->
