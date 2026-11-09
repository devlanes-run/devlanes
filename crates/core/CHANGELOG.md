# devlanes-core

## 0.7.0 — 2026-11-09

### Minor changes

- [e661615a](https://github.com/devlanes-run/devlanes/commit/e661615ab92e5077104877ea646e4be793c0850f) Fixed `devlanes reload 1` not saying that `1` is a lane, as `stop 1` does.
  
  `reload` takes globs and `+tag` like `stop`. A service name in `stop` or `reload` also covers a replica the file no longer has, such as `api.3` after `replicas: 2`.
- [679d435c](https://github.com/devlanes-run/devlanes/commit/679d435cefcd5ea90bbf431c141aa315dcb9ec2f) Changed `url ls`, `debug top`, `logs` and other reads to leave a project removed by `lane clean` removed.
  
  Only `start`, `restart`, `reload`, `run`, `exec`, `lane new`, `infra`, `env`, `url`, `status` and `lane ls` add the folder's project. Other reads in a project nobody started say so; `stop` and `lane gc` do nothing there. A `devlanes.yaml` that does not load now shows its error in `status` and `url ls`. `config check` warns about hostnames before the first start. A main checkout off the primary branch gets its lane on its first `status` or `start`, as other worktrees do.

### Patch changes

- [d5747169](https://github.com/devlanes-run/devlanes/commit/d5747169e4c7d2f579e00d773f8c00a0105db59a) Changed `run_script` npm scripts on Windows to run without npm, so a stop reports the program's own exit code instead of `exit=1`.
  
  Scripts get npm's package and lifecycle variables and `node_modules\.bin` on `PATH`, but not npm's `npm_config_*` settings.
- [20b17f7d](https://github.com/devlanes-run/devlanes/commit/20b17f7d42235555652ec632a2aebfa5eff929a8) Fixed `devlanes debug bundle` on Windows leaving the home directory in its files.
- [96ac235b](https://github.com/devlanes-run/devlanes/commit/96ac235bb12d1e92be5a0d1e526f690e45e0727f) Changed Windows stops to type Ctrl-C into `pty: true` units that run Git's `sh`, so their programs shut down cleanly.
  
  A program that reads its terminal raw and ignores Ctrl-C now runs until `stop.timeout`. `run:` strings with `stop.signal: SIGINT` no longer get a launch note.

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

