---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
---

Fixed CI checkouts of a pull request or a non-primary branch getting lane 1: when a CI variable such as `CI` or `GITHUB_ACTIONS` is set, the checkout now takes lane 0 when it is free (`acme@0` on `acme.localhost`).
