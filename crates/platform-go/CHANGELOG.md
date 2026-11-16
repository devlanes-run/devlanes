# devlanes-platform-go

## 0.2.0 — 2026-11-16

### Minor changes

- [a54919df](https://github.com/devlanes-run/devlanes/commit/a54919df3b992916a3eb01acee5aba909120d678) Added `platform: go`, detected from `go.mod` or `go.work`, which gives Go services the devlanes CA on Linux.
  
  <!-- highlight
  title: Run Go services
  body: Point `run_script` at a `go.mod` to run a main package with `go run`. Go services that devlanes starts trust its CA on Linux.
  -->

