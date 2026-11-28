---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-cli: minor
cargo/devlanes-config: minor
cargo/devlanes-transport: minor
cargo/devlanes-transport-iroh: minor
cargo/devlanes-state: minor
cargo/devlanes-mcp: minor
---

Added `devlanes remote connect <ticket>`, which pairs with a box and connects to it peer to peer, without SSH.

Run `devlanes daemon pair` on the box, then `devlanes remote connect` here with the ticket it prints. The box needs `devlanes daemon install` to stay reachable. The state database gains tables for these pairings, so an older devlanes cannot open it once this release has run.

<!-- highlight
title: Connect to a box without SSH
body: Run `devlanes daemon pair` on the box, then `devlanes remote connect <ticket>` here with the ticket it prints. The two machines connect peer to peer, with no SSH account on the box.
-->
