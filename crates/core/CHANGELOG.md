# devlanes-core

## 0.6.0 — 2026-11-07

### Patch changes

- [a0aca99](https://github.com/devlanes-run/devlanes/commit/a0aca991de5bac6b10fff605302f1df06858055e) Fixed `devlanes env show --explain` printing the values of secret-looking variables in full.
- [077b5b0](https://github.com/devlanes-run/devlanes/commit/077b5b0ed890510e4b3417c26d621fc41a97a5cc) Changed the default Windows stop (`stop.signal: SIGTERM`) to send Ctrl-C, so Node programs such as `wrangler dev` run their shutdown code.
  
  Units where Git's `sh` or `bash` runs, and `run:` strings, still get Ctrl-Break.
- [64657fc](https://github.com/devlanes-run/devlanes/commit/64657fc4cd1d67da95bcb9065962d3eff5870931) Fixed `devlanes exec` commands missing `global_env` and CA variables such as `NODE_EXTRA_CA_CERTS`.
  
  `devlanes env show` without a unit and the command `devlanes with` runs now include `global_env` and the profile's `env` too, except entries that read `self` or `replica`. Until the lane's infra is up, entries that read `infra.*` are left out and named.
  `devlanes exec` now refuses while another of those values does not evaluate (a `${{ env.X }}` the shell does not set, for example), or while the project file does not load and nothing in the lane runs.

## 0.5.0 — 2026-10-26

### Minor changes

- [8015a60](https://github.com/devlanes-run/devlanes/commit/8015a60fe48fae958e0fd0c22e0aa260c7c3acbc) Added idle lanes: after `lanes.freeze_after` (default 15m) without activity, devlanes freezes a lane's processes and its own containers, and the next request, connection or command resumes them.
  
  On macOS and Windows, `lanes.throttle_after` (default 5m) first runs an idle lane at background priority. Set `services.<name>.keep_awake: true` on workers that must keep running. `0s` turns either setting off. See `docs/guides/lifecycle.md`.

## 0.4.0 — 2026-10-25

### Minor changes

- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Removed the fnox integration: the `fnox` config key and `devlanes secrets unlock`.
  
  Configs with a `fnox:` key fail to load. Run the tool in the command instead, for example `fnox exec -- npm run dev`. `${{ env.X }}` now reads the client environment, then `.env`.

### Patch changes

- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Added a per-core CPU view to the TUI monitor. `m` puts memory in the large graph box and CPU in a small box with a square per core, and `c` switches back.

