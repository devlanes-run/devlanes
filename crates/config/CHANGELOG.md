# devlanes-config

## 0.12.0 — 2026-11-09

### Minor changes

- [087f74bb](https://github.com/devlanes-run/devlanes/commit/087f74bb4a2cfd4da7f7333eb7434172531a61dd) Changed `devlanes with --profile` to refuse a running lane whose profiles give its services other settings, before starting anything.
  
  Stop the lane, run `with` in another worktree, or name the services instead of the profile. A profile that only picks services works on a running lane whose profiles set no `env`, `run_scripts` or `infra`.

### Patch changes

- [9d77691d](https://github.com/devlanes-run/devlanes/commit/9d77691d7aaefc4a5910d08afa5012da3bd24dd0) Fixed `after: services` tasks running when a service exits within half a second of starting.
  
  Every service now reads `starting` until it has run half a second and its `ready` check passed, so `devlanes status` and `devlanes wait` agree with `devlanes start`.
- [4a4678ec](https://github.com/devlanes-run/devlanes/commit/4a4678ec5ac2014623f664c14b2ac83db7bd96c1) Changed `devlanes exec` and `env show` to fail on a misspelled `infra.<name>`, which `config check` now reports too.
  
  `exec` and `env show` leave out a value only while its container is stopped or Docker is not running, and say which. They refuse when Docker does not answer in time, and with compose files on a machine without the Docker CLI; pass the value with `--env KEY=...` or start Docker. `env export` and `env shell` now include `NODE_EXTRA_CA_CERTS`, `DENO_CERT`, `SSL_CERT_FILE` and `REQUESTS_CA_BUNDLE` unless the shell sets them.
- [f322ca79](https://github.com/devlanes-run/devlanes/commit/f322ca79aee9a20312c079c4a40709ce871fd32f) Changed `ready.log` to match the lines `devlanes logs` shows, so a line drawn over a spinner or progress text now matches.
  
  Text a program draws only on the alternate screen (full-screen terminal apps) no longer matches, and a prompt or a line still being redrawn matches once the program moves past it or pauses. Match a line the program prints before it switches to full screen, or use a `tcp` or `http` check. Trailing spaces are not part of a logged line, so end a pattern before them: `^password>` or `^password>$` instead of `"^password> "`.

## 0.11.0 — 2026-11-07

### Patch changes

- [6090b6e](https://github.com/devlanes-run/devlanes/commit/6090b6e6e954c332aab772ac7bf9383226ab9971) Fixed `run` lists failing to start when an item is one `${{ }}` expression that gives a number, such as `self.port`.

## 0.10.2 — 2026-11-02

### Patch changes

- [fef7c86](https://github.com/devlanes-run/devlanes/commit/fef7c863cf1a266174c8bed86a23767e0a30dee4) Changed `daemon.exit_when_idle` to stop the daemon about 15 seconds after the last TUI closes instead of 30.
  
  The summary printed when the TUI closes now says `stopping in 15s` instead of `running` when nothing keeps the daemon up.
- [36c4aef](https://github.com/devlanes-run/devlanes/commit/36c4aefc33e33d3e7af468e65af7d27762ad866f) Changed a deleted worktree's lane to keep its data and ports until `devlanes lane gc --yes` cleans it.
  
  Set `lanes.removed_worktrees: clean` to have the daemon clean these lanes by itself, as before.

## 0.10.1 — 2026-11-01

### Patch changes

- [22d6f20](https://github.com/devlanes-run/devlanes/commit/22d6f2063df1696a4511831d010c63b4a5a0fa6a) Changed the dialog after an update to show breaking changes and up to five highlights.
  
  Press `o` in it to open the release notes. `devlanes update` and the install scripts now end with a link to them instead of the full text. Set `updates.highlights: false` in the user config to stop the dialog from opening by itself.

## 0.10.0 — 2026-11-01

### Minor changes

- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Removed `updates.check`; use `updates.mode: disable | notify | auto` in the user config.
  
  Replace `updates.check: false` with `updates.mode: disable`; a config that still sets `updates.check` fails to load until it is changed.

### Patch changes

- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Removed `devlanes certs trust` and `certs untrust`; use `devlanes setup`, which works out what this machine needs and asks once.
  
  Run `devlanes setup` instead of `certs trust --hosts --resolver`, and `devlanes setup --remove` instead of `certs untrust --hosts --resolver`.
  The TUI offers to run it when HTTPS or hostnames need it.

## 0.9.0 — 2026-10-29

### Minor changes

- [31c5a3f](https://github.com/devlanes-run/devlanes/commit/31c5a3f75f97742e5b5f96d22567608c9929e1b3) Changed default lane domains to `<project><N>.localhost`, such as `acme2.localhost` instead of `acme-2.localhost`.
  
  Lane 0 keeps `acme.localhost`, and the `resolver` strategy gives `acme2.test`. A project that sets `lanes.vars.domain` keeps its own. Replace `acme-2.localhost` wherever you wrote it, such as `.env` files and OAuth redirect URIs. Lane 2 of `acme` and lane 0 of a project named `acme2` now share `acme2.localhost`, and devlanes warns about it with `host_conflict`.

### Patch changes

- [31c5a3f](https://github.com/devlanes-run/devlanes/commit/31c5a3f75f97742e5b5f96d22567608c9929e1b3) Added the `domain(name)` expression function, which gives a name its own site in each lane, such as `pages2.localhost` in lane 2, for user content that must not share the app's cookies.

## 0.8.0 — 2026-10-28

### Minor changes

- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Removed `devlanes lane new <branch>` and the `lanes.worktree_dir` config key; devlanes no longer creates worktrees.
  
  Create one with `git worktree add <path> <branch>` and run `devlanes start` in it. Configs that still set `lanes.worktree_dir` fail to load until it is removed. `devlanes lane new --name` still adds another lane for the current folder.
- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Changed projects to be identified by their `project:` name, so one repository can hold several projects: a `devlanes.yaml` in a subdirectory, or another file such as `devlanes.prod.yaml` named once with the new `--file` flag and then with `-p`/`--project` (which also reads `DEVLANES_PROJECT`).
  
  Renaming `project:` now makes a new project; the old one's lanes stay until `devlanes lane gc --yes` cleans them. Another repository can no longer use a name already taken. In a project file below the worktree root, relative `cwd`, `run_script.file` and `sync` paths now start at that file's directory. `lanes.max` comes from the primary worktree's file, and a worktree whose file sets it, `lanes.primary` or shared infra differently gets a warning. Commands run inside a process devlanes started now default to that lane's project; pass `-p` to choose another. See [several projects in one repository](https://github.com/devlanes-run/devlanes/blob/main/docs/guides/services.md).

### Patch changes

- [0befd1d](https://github.com/devlanes-run/devlanes/commit/0befd1d1c48705308b4dbd6c9759f8ee6d71d05c) Added a top-level `extends` key that builds a project file on one or more others, each merged with its own `.local.yaml`, so a `devlanes.prod.yaml` can reuse `devlanes.yaml` and change only what differs.
  
  ```yaml
  # devlanes.prod.yaml
  extends: devlanes.yaml
  project: acme-prod
  profiles:
    default: { include: ["*"], run_scripts: { dev: start } }
  ```

## 0.7.0 — 2026-10-28

### Minor changes

- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Changed `devlanes remote connect` so a box serves only machines it paired with, and added `devlanes daemon pair`, `devlanes daemon clients`, `devlanes daemon revoke`, `remote.require_pairing` and `remote.allow_connections`.
  
  A machine's first connection to a box needs a one-time code: run `devlanes daemon pair` on the box, then `devlanes remote connect <box> --code <code>` here, or enter the code when `devlanes remote connect <box>` asks. Later connections, including those after a daemon restart, need none. To let every machine that gets in over SSH connect without a code, as before, set `remote.require_pairing: false` in the box's user config. To turn every machine away, set `remote.allow_connections: false`. See "Pairing" in `docs/remote.md`.

### Patch changes

- [76f6716](https://github.com/devlanes-run/devlanes/commit/76f67163f90f6857092f8e42e44f1cbdc1acaf6d) Changed `daemon.exit_when_idle` so a daemon that only commands used, with no TUI ever open, also exits once nothing has run for about 5 minutes; before, it kept running until stopped.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Added `remote.start_daemon` to the user config of a box: set to `false`, `devlanes remote connect` no longer starts the box's daemon, and the client reports the stopped daemon and retries until it runs.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Added `infra.external.<name>` for a server devlanes does not run: every lane lists it as a unit of kind `external`, the proxy routes its `hosts` to its loopback `address`, and machines connected with `devlanes remote connect` mirror it while a lane of the project runs.
  
  A server only you run goes in `devlanes.local.yaml`; see `docs/guides/infra.md#external-ports`:
  
  ```yaml
  infra:
    external:
      pgadmin:
        address: 127.0.0.1:5050
        hosts: "pgadmin.${{ lane.domain }}"
  ```
- [01fb93f](https://github.com/devlanes-run/devlanes/commit/01fb93f56d3265cfe7756d4742dba08c93cc1fd0) Fixed `devlanes start` of services on a lane that runs or is being brought up waiting for the lane's other starts: each start now shows its services as queued, with what they wait for, the moment it arrives, and joins the bring-up instead of running its tasks again.
- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Added `remote.boxes.<name>.command` to the user config, so `devlanes remote connect <name>` reaches a container, pod, WSL distribution or codespace by running a command such as `docker exec` instead of SSH.
  
  ```yaml
  remote:
    boxes:
      acme:
        command: [docker, exec, -i, acme, devlanes, tunnel, --stdio]
  ```
  
  See `docs/guides/remote-command.md`.
- [01fb93f](https://github.com/devlanes-run/devlanes/commit/01fb93f56d3265cfe7756d4742dba08c93cc1fd0) Changed the lane's Wrangler dev registry (`WRANGLER_REGISTRY_PATH`, `CLOUDFLARE_DEV_REGISTRY_PATH` and `MINIFLARE_REGISTRY_PATH`) to be set for every process of a lane, so workers started by `wrangler dev`, a script, a task or the Cloudflare Vite or a Next plugin use the lane's registry even when the shell exports one, and starts no longer scan the worktree for a Wrangler config. `platform: none` now skips platform detection without a `config check` warning.

## 0.6.0 — 2026-10-26

### Minor changes

- [8015a60](https://github.com/devlanes-run/devlanes/commit/8015a60fe48fae958e0fd0c22e0aa260c7c3acbc) Added idle lanes: after `lanes.freeze_after` (default 15m) without activity, devlanes freezes a lane's processes and its own containers, and the next request, connection or command resumes them.
  
  On macOS and Windows, `lanes.throttle_after` (default 5m) first runs an idle lane at background priority. Set `services.<name>.keep_awake: true` on workers that must keep running. `0s` turns either setting off. See `docs/guides/lifecycle.md`.

## 0.5.0 — 2026-10-26

### Minor changes

- [edc8274](https://github.com/devlanes-run/devlanes/commit/edc8274b1423f7370cbaaae258f617037857edf3) Changed a plain `devlanes start` (and `s` on a lane in the TUI) to bring back the services the lane ran before, after a reboot, a daemon crash or `daemon stop`, including ones started by name.
  
  `devlanes stop` without units now also forgets the lane's profiles, so the next plain start uses the `default` profile. A plain start names the other lanes that ran so `devlanes lane resume` can bring them back, and `devlanes with` no longer changes what a lane brings back. Set `start.restore: false` in `~/.devlanes/config.yaml` to always start the `default` profile.

### Patch changes

- [6b60828](https://github.com/devlanes-run/devlanes/commit/6b608289d00c02d7f6181fad56def7e8546e3977) Added `devlanes config get`, `set` and `unset` to read and change the user config.yaml.
- [488f5be](https://github.com/devlanes-run/devlanes/commit/488f5bee3ef717386e6ea4ece47870af144ce613) Added `ready.http.host` to set the Host header an http readiness check sends, for apps that answer their health route only on their own hostname, such as `host: ${{ self.hosts[0] }}`.

## 0.4.1 — 2026-10-26

### Patch changes

- [f81274b](https://github.com/devlanes-run/devlanes/commit/f81274b9c8094c87886180d81da6ed5a88187ec0) Changed the proxy's error pages: each names an error code documented at devlanes.run/docs/errors, lists commands to diagnose it, and comes as Markdown to clients that do not ask for HTML. Requests to a starting service or a reconnecting box now wait for it, refused connections are retried for a few seconds, and forwarded responses carry `Via: 1.1 devlanes.run` unless `proxy.via: false`.

## 0.4.0 — 2026-10-25

### Minor changes

- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Removed the fnox integration: the `fnox` config key and `devlanes secrets unlock`.
  
  Configs with a `fnox:` key fail to load. Run the tool in the command instead, for example `fnox exec -- npm run dev`. `${{ env.X }}` now reads the client environment, then `.env`.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Removed the `DEVLANES_PUBLIC_URL` variable that services received.
  
  Apps that read it get nothing now. Set it in the service's `env` instead:
  
  ```yaml
  env:
    DEVLANES_PUBLIC_URL: ${{ self.public_url }}
  ```
  
  `DEVLANES_PUBLIC_URL_<SERVICE>` in `with`, `exec` and `env show` is unchanged.

### Patch changes

- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Added an idle exit for daemons that a client started on demand, and the `daemon.exit_when_idle` user config key to turn it off.
  
  Such a daemon now exits about 30 seconds after the last TUI left, when nothing ran or was starting for that whole time. Containers keep running. Daemons installed with `devlanes daemon install` never exit when idle. Set `daemon.exit_when_idle: false` to keep the old behavior.
- [771b030](https://github.com/devlanes-run/devlanes/commit/771b030878fefcde77a4d3b35bd007c17c9b9b30) Added WSL support: devlanes detects WSL and adapts to it.
  
  `devlanes url open` and `o` in the TUI open URLs in the Windows browser. A new `windows` target for `tls.trust` adds the CA to the Windows user store. Starting in a worktree under `/mnt/<drive>` warns, `doctor` gets a `wsl` group, and `daemon install` without systemd fails with the `wsl.conf` fix. See `docs/guides/wsl.md`.

