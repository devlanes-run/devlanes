---
cargo/devlanes: patch
cargo/devlanes-cli: patch
cargo/devlanes-config: patch
cargo/devlanes-daemon: patch
cargo/devlanes-resolve: patch
cargo/devlanes-start-gate: patch
---

Fixed `devlanes start` of services on a lane that runs or is being brought up waiting for the lane's other starts: each start now shows its services as queued, with what they wait for, the moment it arrives, and joins the bring-up instead of running its tasks again.
