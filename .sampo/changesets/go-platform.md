---
cargo/devlanes: minor
cargo/devlanes-platform-go: minor
---

Added `platform: go`, detected from `go.mod` or `go.work`, which gives Go services the devlanes CA on Linux.

<!-- highlight
title: Run Go services
body: Point `run_script` at a `go.mod` to run a main package with `go run`. Go services that devlanes starts trust its CA on Linux.
-->
