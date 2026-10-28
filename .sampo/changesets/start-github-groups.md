---
cargo/devlanes: patch
cargo/devlanes-cli: patch
---

Fixed `devlanes start` on GitHub Actions printing `::endgroup::` lines without the `::group::` that opens them; each start phase now gets its own log group.
