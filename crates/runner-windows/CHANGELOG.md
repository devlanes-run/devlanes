# devlanes-runner-windows

## 0.9.0 — 2026-11-09

### Patch changes

- [96ac235b](https://github.com/devlanes-run/devlanes/commit/96ac235bb12d1e92be5a0d1e526f690e45e0727f) Fixed Windows stops of `pty: false` units running Git's `sh` or another MSYS program waiting for `stop.timeout`.
- [96ac235b](https://github.com/devlanes-run/devlanes/commit/96ac235bb12d1e92be5a0d1e526f690e45e0727f) Fixed Windows units left by a previous daemon getting Ctrl-Break on `stop`, which skipped Node's `SIGINT` handlers.
- [d5747169](https://github.com/devlanes-run/devlanes/commit/d5747169e4c7d2f579e00d773f8c00a0105db59a) Fixed `run:` lists and `devlanes with --` commands of MSYS programs on Windows expanding `*`, `?`, `[...]`, `{a,b}` and `~` in arguments.
  
  This covers Git's `usr\bin` shells and tools and MSYS2 and Cygwin programs. Arguments now arrive as written, as on macOS and Linux.
- [96ac235b](https://github.com/devlanes-run/devlanes/commit/96ac235bb12d1e92be5a0d1e526f690e45e0727f) Changed Windows stops to type Ctrl-C into `pty: true` units that run Git's `sh`, so their programs shut down cleanly.
  
  A program that reads its terminal raw and ignores Ctrl-C now runs until `stop.timeout`. `run:` strings with `stop.signal: SIGINT` no longer get a launch note.
- [a4108390](https://github.com/devlanes-run/devlanes/commit/a4108390ab5093951c1075317bc93a2000030196) Fixed Windows units ended by a stop showing exit codes such as -1073741510, 255 or 1 instead of a signal, as on macOS and Linux.
  
  just recipes and Git's `bin\bash.exe` now show the code they return, `512` when their shell ended on Ctrl-C.

## 0.8.0 — 2026-11-07

### Patch changes

- [077b5b0](https://github.com/devlanes-run/devlanes/commit/077b5b0ed890510e4b3417c26d621fc41a97a5cc) Fixed stopped units on Windows showing exit codes such as 255 or 512 instead of reading as interrupted.
- [077b5b0](https://github.com/devlanes-run/devlanes/commit/077b5b0ed890510e4b3417c26d621fc41a97a5cc) Changed the default Windows stop (`stop.signal: SIGTERM`) to send Ctrl-C, so Node programs such as `wrangler dev` run their shutdown code.
  
  Units where Git's `sh` or `bash` runs, and `run:` strings, still get Ctrl-Break.

## 0.7.0 — 2026-11-01

### Patch changes

- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Fixed `devlanes attach` and the TUI's unit pane on Windows showing a `pty: true` unit's lines with gaps, and missing later resizes, after a resize that came while the unit was printing.

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

