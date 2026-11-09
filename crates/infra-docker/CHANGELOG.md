# devlanes-infra-docker

## 0.6.0 — 2026-11-09

### Patch changes

- [a5309033](https://github.com/devlanes-run/devlanes/commit/a5309033e8d296987a2dd797dc81c0be182ae5b0) Fixed the proxy on Windows taking 2 seconds per connection to services that listen only on `::1`, such as Vite on `localhost`.
  
  Readiness checks of such services, port checks of infra containers that are not listening yet and connections a Windows box makes to its own services no longer wait either.
- [28a6b933](https://github.com/devlanes-run/devlanes/commit/28a6b933ea008732ffc669eebbb0d3d615a159cb) Fixed container events, states and stats staying unavailable on macOS and Linux when Docker started after the devlanes daemon.
- [a5113517](https://github.com/devlanes-run/devlanes/commit/a5113517be39affa095fab0364748fb863a080db) Fixed infra containers that restarted while devlanes reconnected to Docker keeping their old ports.

## 0.5.0 — 2026-11-07

### Patch changes

- [5db5cde](https://github.com/devlanes-run/devlanes/commit/5db5cdec69bc61915e31237b81abf0456a061bfc) Added `devlanes lane clean --dry-run`, which lists what a clean would stop, run and delete, with sizes.
- [2e0091c](https://github.com/devlanes-run/devlanes/commit/2e0091c226381bb1794d99f52d6db471ee498efc) Fixed services, routes and `devlanes status` keeping an infra container's old port after it restarted.

## 0.4.0 — 2026-10-26

### Minor changes

- [8015a60](https://github.com/devlanes-run/devlanes/commit/8015a60fe48fae958e0fd0c22e0aa260c7c3acbc) Added idle lanes: after `lanes.freeze_after` (default 15m) without activity, devlanes freezes a lane's processes and its own containers, and the next request, connection or command resumes them.
  
  On macOS and Windows, `lanes.throttle_after` (default 5m) first runs an idle lane at background priority. Set `services.<name>.keep_awake: true` on workers that must keep running. `0s` turns either setting off. See `docs/guides/lifecycle.md`.

## 0.3.1 — 2026-10-26

### Patch changes

- [b3a2ee0](https://github.com/devlanes-run/devlanes/commit/b3a2ee03f66aa8c7515008f37031468fbc92c2ae) Added `devlanes infra logs <container> [-f]` and Docker logs in the TUI pane when an infra container is selected, read from Docker and followed across container restarts.

