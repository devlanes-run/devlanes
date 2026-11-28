# devlanes-gui

The second devlanes binary, installed next to `devlanes`: it holds the GUI hosts and the viewer, so `devlanes` itself never links the GUI libraries and keeps starting on a box without them. `devlanes-gui host` runs one GUI host for one GUI service and serves its windows with `devlanes-gui-host`; `devlanes-gui view` shows a GUI service's windows on this machine with `devlanes-gui-viewer`.

## Usage

```sh
devlanes-gui version
# devlanes-gui 0.17.0 build=release

DEVLANES_GUI_TOKEN=... devlanes-gui host --backend test-pattern --listen 127.0.0.1:0 --service acme:desktop
# {"ready":1,"devlanes":"0.17.0","protocol":1,"address":"127.0.0.1:41234","set":{},"remove":[],"real_desktop":false}
```

`version` prints the name and the devlanes release version first, then the build kind.

`host` starts the backend, binds the address and prints one line on standard output once it listens, then serves the GUI protocol until it gets SIGINT, SIGTERM or SIGHUP (on Windows Ctrl-C, Ctrl-Break or the console closing) or the backend stops. Logs go to standard error, filtered by `DEVLANES_LOG` (default `info`). It exits with 0 after a stop, 1 when it cannot run (an unknown backend, an address that is not loopback, a port in use), 2 for a malformed command line and 3 when the backend cannot run on this machine, after printing an unready line instead of the ready line. It also ends with 0 once the daemon says the unit exited, after showing viewers its windows Closed. A host started with a token watches for its daemon going away: it exits with 1 when the control connection closes before the unit's identity came, or when no identity came within 60 seconds of the ready line (a daemon that could not tell it, or one that died without closing the connection; on Windows the clock keeps counting while the machine sleeps). Once it has the identity and the control connection closes, it checks the unit by pid and start time every 2 seconds and exits with 0 when the unit is gone.

## Viewer

```sh
devlanes-gui view --address 127.0.0.1:26012 --service acme@2:desktop
devlanes-gui view --address 127.0.0.1:26012 --service acme@2:desktop \
  --instance-dir ~/.devlanes/run/gui-viewers --devlanes /usr/local/bin/devlanes --unit acme@2:desktop
```

`view` opens a local window for each window of the GUI host at `--address` and runs until the service is gone: 3 seconds after the host showed every window Closed, or after `--devlanes` says the unit stopped (it runs `devlanes gui ls UNIT --json` with `DEVLANES_NO_SPAWN=1` once the host has not answered for 30 seconds; a daemon that is not running counts only when it still is not 30 seconds later, so a daemon restart ends nothing). `devlanes gui open` and the daemon's auto-open start it with `--instance-dir`, where it holds the lock that makes it the one viewer of the service on this machine; a second `view` of the same service asks the first for its windows and exits with 0. `--auto` is the daemon's: windows open without taking focus, and the daemon ends the viewer once its service stopped. It exits with 3 on a machine without a display (neither `WAYLAND_DISPLAY` nor `DISPLAY` set on Linux) and 1 when the first connection fails. `version` and `host` never start the windowing library.

## Ready line

One JSON object, written and read with `devlanes_gui_proto::ready::Ready` (feature `serde`):

- `ready`: the format version, 1. The reader refuses another version and ignores fields it does not know, which later versions may add.
- `devlanes` and `protocol`: the devlanes release and the GUI protocol version of the host.
- `address`: where the host listens, such as `127.0.0.1:41234`.
- `set`: the variables the unit gets, as names and string values.
- `remove`: the names of variables the unit must not inherit.
- `real_desktop`: whether the display is the machine's real desktop, as on a Windows host.

A backend that cannot run here prints `{"unready":1,"devlanes":"0.17.0","protocol":1,"reason":"no_desktop_session","message":"..."}` instead, read with `devlanes_gui_proto::ready::HostLine`. The reasons are `no_desktop_session`, `elevated`, `missing_library`, `unsupported_os` and `other`.

## Control token

The daemon's control connection needs the token in `DEVLANES_GUI_TOKEN`; without it the control role is refused. The token keeps other accounts on the machine, which can reach the loopback port, from taking the control role. It does not protect against processes of the same account, which can read the host's environment and can already do everything through the daemon. It is never a flag, since other accounts can read a process's arguments. It stays in the host's environment, so a process the host starts should be started without it where the spawning API allows it.

## License

Apache-2.0
