---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
---

Changed lane numbering in CI: when devlanes runs with a CI variable such as `CI` or `GITHUB_ACTIONS` set, the checkout takes lane 0 while it is free on any branch, so a detached pull request checkout is `acme@0` on `acme.localhost` instead of lane 1.
