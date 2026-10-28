# devlanes-runner-windows

## 0.6.1 — 2026-10-28

### Patch changes

- [effb343](https://github.com/devlanes-run/devlanes/commit/effb343649781d8c516285391605f27d192c66c0) Fixed `pty: true` units on Windows whose output leaves a color active stopping to follow the TUI pane's size, which left a gap of empty rows above the cursor and blank lines in the log, and a scrolled-back pane jumping up when the unit's screen got shorter.

## 0.6.0 — 2026-10-28

### Patch changes

- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Fixed `devlanes stop` on Windows sometimes waiting out the stop timeout for a service run through a `.cmd` shim such as `npm.cmd`, when its `Terminate batch job (Y/N)?` prompt missed the answer.

## 0.5.0 — 2026-10-26

### Minor changes

- [8015a60](https://github.com/devlanes-run/devlanes/commit/8015a60fe48fae958e0fd0c22e0aa260c7c3acbc) Added idle lanes: after `lanes.freeze_after` (default 15m) without activity, devlanes freezes a lane's processes and its own containers, and the next request, connection or command resumes them.
  
  On macOS and Windows, `lanes.throttle_after` (default 5m) first runs an idle lane at background priority. Set `services.<name>.keep_awake: true` on workers that must keep running. `0s` turns either setting off. See `docs/guides/lifecycle.md`.

## 0.4.0 — 2026-10-25

### Patch changes

- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Added native support for Windows 11 24H2 or later, on x64 and arm64.
  
  Install with `irm https://devlanes.run/install.ps1 | iex`. `*.localhost` resolves without a hosts file, `daemon install` starts the daemon at logon, and `certs trust --resolver` writes an NRPT rule for custom domains. See `docs/guides/windows.md`.

