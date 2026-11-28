# devlanes-gui

The second devlanes binary, installed next to `devlanes`: it holds the GUI hosts, so `devlanes` itself never links the GUI libraries and keeps starting on a box without them. `devlanes-gui host` runs one GUI host for one GUI service and serves its windows with `devlanes-gui-host`.

## Usage

```sh
devlanes-gui version
# devlanes-gui 0.17.0 build=release

DEVLANES_GUI_TOKEN=... devlanes-gui host --backend test-pattern --listen 127.0.0.1:0 --service acme:desktop
# {"ready":1,"devlanes":"0.17.0","protocol":1,"address":"127.0.0.1:41234","set":{},"remove":[],"real_desktop":false}
```

`version` prints the name and the devlanes release version first, then the build kind.

`host` starts the backend, binds the address and prints one line on standard output once it listens, then serves the GUI protocol until it gets SIGINT, SIGTERM or SIGHUP (on Windows Ctrl-C, Ctrl-Break or the console closing) or the backend stops. Logs go to standard error, filtered by `DEVLANES_LOG` (default `info`). It exits with 0 after a stop, 1 when it cannot run (an unknown backend, an address that is not loopback, a port in use), 2 for a malformed command line and 3 when the backend cannot run on this machine, after printing an unready line instead of the ready line. A host started with a token stops by itself when no unit identity came over the control connection within 60 seconds of the ready line, since its daemon is then gone.

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
