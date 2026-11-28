# devlanes-gui

The second devlanes binary. It holds the GUI hosts, so `devlanes` itself never links the GUI libraries and keeps starting on a box without them. It ships next to `devlanes` in every install. `devlanes-gui host` is one GUI host for one GUI service and serves its windows with `devlanes-gui-host`; nothing in devlanes starts it yet.

## Usage

```sh
devlanes-gui version
# devlanes-gui 0.17.0 build=release

DEVLANES_GUI_TOKEN=... devlanes-gui host --backend test-pattern --listen 127.0.0.1:0 --service acme:desktop
# {"ready":1,"address":"127.0.0.1:41234","set":{},"remove":[],"real_desktop":false}
```

`version` prints the name and the devlanes release version first; installers and `devlanes update` read those two words.

`host` starts the backend, binds the address and prints one line on standard output once it listens, then serves the GUI protocol until it gets SIGINT, SIGTERM or SIGHUP (on Windows Ctrl-C, Ctrl-Break or the console closing) or the backend stops. Logs go to standard error, filtered by `DEVLANES_LOG` (default `info`). It exits with 0 after a stop, 1 when it cannot run (an unknown or unavailable backend, an address that is not loopback, a port in use) and 2 for a malformed command line.

## Ready line

One JSON object:

- `ready`: the format version, 1. A reader refuses another version and ignores fields it does not know, which later versions may add.
- `address`: where the host listens, such as `127.0.0.1:41234`.
- `set`: the variables the unit gets, as names and string values.
- `remove`: the names of variables the unit must not inherit.
- `real_desktop`: whether the display is the machine's real desktop, as on a Windows host.

## Control token

The daemon's control connection needs the token in `DEVLANES_GUI_TOKEN`; without it the control role is refused. It is never a flag, since other users can read a process's arguments. It stays in the host's environment, so any process the host starts must not inherit it.

## License

Apache-2.0
