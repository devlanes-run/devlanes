# Remote boxes

A box is any machine you can reach with `ssh`, or with a command such as `docker exec`, that runs devlanes for its own lanes. Another machine connects to it and gets the box's lanes locally: the same hostnames, the same port numbers and the synced folders. Remote is mirroring only. You start, stop and inspect lanes on the box itself (over `ssh`, or in an editor's remote terminal); the connection never runs commands there and never carries environments or secrets.

## Connect

```sh
devlanes remote pair                              # on the box: a one-time code such as K7QM-3XWD
devlanes remote connect hetzner                   # any ~/.ssh/config host; asks for the code the first time
devlanes remote connect hetzner --code K7QM-3XWD  # or pass the code
devlanes remote connect hetzner --background      # returns once connected; the connection stays up
devlanes remote ls                                # boxes, state, mirrored ports, skipped lanes, syncs, warnings
devlanes remote sync                              # synced folders of running box lanes
devlanes remote disconnect hetzner                # or `devlanes remote disconnect` for every box
```

The connection belongs to your local daemon, not to the terminal. Ctrl-C on `remote connect` only detaches the CLI. The daemon reconnects with backoff after network loss or sleep. While it is down, mirrored ports stay bound; requests to the box's hostnames wait up to 15 seconds for the reconnect, then get a [box reconnecting](errors/box-reconnecting.md) page. After a reconnect the box's current state replaces what the client knew. When the connection fails for good (on the first connection, rejected credentials or a host key, or a host name that does not resolve; a protocol mismatch at any time), its ports and hostnames are removed and `remote ls` shows the error.

A box stays connected across restarts of your local daemon until `remote disconnect`: the next daemon connects it again in the background, with no terminal attached, and keeps retrying it until you disconnect it, also when the box is gone for good. When a reconnect needs you, because SSH asks for a password, a passphrase or a host key confirmation with nobody there to answer, or rejects credentials that worked before, the box waits in `needs_auth` instead of retrying. While the daemon keeps running, its ports stay bound and its hostnames show a [sign-in needed](errors/box-sign-in-needed.md) page; after a daemon restart they appear once the box connects. `remote ls` and `status --json` show the error with `devlanes remote connect <box>` as the `fix`. Running that command shows the prompts in your terminal and resumes the connection. A box that no longer accepts this machine's pairing parks the connection the same way, in `needs_pairing` ([pairing](#pairing)).

devlanes uses the system `ssh` with one ControlMaster per box, so `~/.ssh/config`, agents, ProxyJump and certificates apply. Passphrase, OTP and host-key prompts appear in the terminal that runs `remote connect`, prefixed with the box's name. An idle master exits after 10 minutes. Over that master devlanes runs `devlanes tunnel --stdio` on the box, which needs only a session channel: `AllowTcpForwarding no` on the box is fine. Windows' OpenSSH has no ControlMaster, so a Windows client runs one `ssh` per connection and authenticates on every reconnect; [remote setup](guides/remote-setup.md#windows-clients) has the details.

A box without SSH, such as a container, a pod, a WSL distribution or a codespace, can be reached by a command instead: `remote.boxes.<name>.command` in your user config lists a command that runs `devlanes tunnel --stdio` inside it, and `devlanes remote connect <name>` runs that command in place of `ssh`. [Boxes reached by a command](guides/remote-command.md) has examples for `docker exec`, `kubectl exec`, `wsl.exe` and `gh codespace ssh`.

A box runs Linux, macOS or Windows; a Windows box needs Windows' OpenSSH Server, and [Windows boxes](guides/remote-setup.md#windows-boxes) covers what differs there.

Install devlanes on the box yourself, with the same install methods as on any machine; it must be on the `PATH` of a non-interactive ssh session or in `~/.devlanes/bin` (`%USERPROFILE%\.devlanes\bin` on Windows). Without it, the first `remote connect` fails with `devlanes is not installed on <box>` and stops; once a connection worked, a binary that is missing for a moment, such as during an upgrade, only makes the link retry. devlanes never installs or upgrades anything on the box. A protocol mismatch fails with instructions, and a different release on the box is shown as a warning. On Linux, run `sudo loginctl enable-linger "$USER"` on the box so its daemon survives logout; `remote ls` warns when linger is off. `devlanes daemon install` adds a user unit for boot, but a user unit is also stopped with the session unless linger is on. [Remote setup](guides/remote-setup.md) walks through preparing a box.

When no daemon runs on the box, the connection starts one. Set `remote.start_daemon: false` in the box's user config to keep that under your control: the client then reports that the box daemon is not running and keeps retrying until you start it there with `devlanes daemon start`. A daemon started that way exits about 30 seconds after the last client or TUI leaves while nothing runs (`daemon.exit_when_idle`), so run `devlanes daemon install` on the box, or set `daemon.exit_when_idle: false` there, to keep it running.

`devlanes status --json` on the box lists the clients connected to it under `clients`: each with its `id`, a random id the client machine keeps in `client-id` in its devlanes home (`~/.devlanes` by default, `$DEVLANES_HOME` when set) so it stays the same across reconnects and restarts, its host `name`, the tunnel's `pid`, `ssh` address and start time (`since_ms`), and under `mirror` what that client mirrors ([client URLs on the box](#client-urls-on-the-box)). A client id is `dl_cid_` followed by 64 lowercase hex digits, 256 random bits.

## Pairing

A box serves only machines it paired with. SSH, or the command that reaches the box, still decides who gets in at all; pairing decides which of the machines that get in see the box's lanes. On a client's first connection the box asks for a one-time code, which you get on the box:

```sh
devlanes remote pair                            # on the box
# pairing code: K7QM-3XWD (works once, for 10 minutes)
# on the machine you connect from, run: devlanes remote connect <box> --code K7QM-3XWD

devlanes remote connect hetzner --code K7QM-3XWD   # on the client
devlanes remote connect hetzner                    # or this, which asks for the code
```

`--background` asks for the code the same way before it returns. A `--code` is used only by the `remote connect` that passed it, and only within the 10 minutes a code lives: a later reconnect that needs a code asks again instead of sending an old one.

The client trades the code for a remote token (`dl_rt_` and 64 lowercase hex digits) that it keeps for that box in its state database, in its devlanes home, and presents with its client id on every later connection, including the ones a restarted daemon makes in the background. The box keeps only the SHA-256 of the token, with the client's id, name, when it paired and when it last connected, in its own state database. Each pairing has its own token, so a machine that reaches one box under two names, such as two SSH aliases, pairs once per name and keeps both. The box keeps up to 8 pairings per machine and drops the oldest beyond that, which also bounds what a machine that lost its token and paired again leaves behind. Pairing does not stop someone who can run commands on the box as you, since they can run `devlanes remote pair` themselves; it keeps the box's lanes from machines that only reach the tunnel, such as one using a key restricted to `devlanes tunnel --stdio` in `authorized_keys` through a [box command](guides/remote-command.md) like `[ssh, -T, acme-box]`. That restriction works with a box command only: `remote connect` over SSH first runs a probe, which a forced command replaces.

Until a client is paired, the box sends it no version label, registry or box info, and answers no stream it opens other than the control stream. An unpaired client can still see the first line the tunnel writes (`devlanes-tunnel 4`, its protocol revision), the protocol revision the box speaks when they differ, whatever the tunnel writes on stderr before the session (that the box daemon is not running with `remote.start_daemon: false`, that the box's user config has invalid remote settings, that a Windows box daemon stays inside the ssh session), and whether the box asks for a code or refuses one, with the wait after too many wrong ones. Connecting also starts the box daemon when none runs, unless `remote.start_daemon` is false. Over SSH the client's probe reports the box's OS, shell and where devlanes is installed before any of this.

A code is 8 characters from an alphabet without I, L, O and U, works once and expires after 10 minutes. It lives only in the box daemon's memory, which stays running while a code is outstanding. Every wrong code makes every outstanding code void, so ask for a new one after a typo, and after 5 wrong codes within 10 minutes the box refuses every code, and `remote pair` hands out none, until the oldest of those is 10 minutes old; `remote pair`, `remote connect` and `remote ls` say how long that is. The box logs each refused code and token with the client's id and SSH address, and compares codes and token hashes in constant time.

```sh
devlanes remote clients             # on the box: paired clients and the ones connected now
devlanes remote revoke laptop       # by name, id, or a unique start of the id
```

`remote revoke` deletes every pairing of the client and ends its connections at once; another client's connection stays. A client connected without pairing (`remote.require_pairing: false`) has none to revoke. The revoked client then waits in `needs_pairing`, as it does when its token is refused for any other reason or when nobody was there to type a code: no retries, its ports stay bound and its hostnames show a [pairing needed](errors/box-pairing-needed.md) page while its daemon runs, and `remote ls` shows `devlanes remote pair on <box>, then devlanes remote connect <box> --code <code>` as the `fix`. A new code pairs it again. `devlanes daemon reset-state` on the box removes every pairing, voids every code and ends every client's session; on a client it removes every token.

To let every machine that can reach the box over SSH connect without a code, as before pairing existed, set `remote.require_pairing: false` in the box's user config. The tunnel reads it at each connection, so no restart is needed. An invalid value counts as `true`.

## Mirroring

A client mirrors what the box's devlanes runs or declares: services, containers and [external ports](#external-ports). A server you start on the box by hand, outside devlanes, is not reachable from the client until the box's config declares it.

Every allocated port of the box's running, non-hidden services and containers is bound on `127.0.0.1` and `::1` under the same number. A stopped unit's ports are bound once it starts. `psql -p 20012`, debugger ports, gRPC and websockets work unchanged. When a local process already holds a port, that port is not mirrored and `remote ls` says so, for example `port 5432 is in use on this machine by postgres (pid 812); not mirrored`. When another connected box's mirror holds it, the message names that box and its unit instead: `port 20012 is in use on this machine by box hetzner (acme@0:api); not mirrored`. It is tried again when the box's lanes change. The box's hostnames keep working through a private relay port.

Both machines allocate ports from the same default pool, so while a box is connected, new port blocks on this machine skip every port its lanes hold, stopped units included. A port that already belongs to a local lane's block stays local: it is not mirrored (`port 20004 is allocated to local lane acme@0; not mirrored`). Lanes allocated on both machines before they first connected often overlap like this. Two boxes know nothing of each other's ports, so lanes on both can get the same numbers, and only one box's can be mirrored. While two connected boxes have overlapping `ports.range` values, both get a `ports_range_overlap` warning in `remote connect` and `remote ls`, which goes away when one of them disconnects; giving each box its own `ports.range` in its user config keeps them apart. A local service with a literal `PORT` that the box mirrors refuses to start and names the box.

The box's hostnames join your local proxy. TLS is terminated with your local CA, so `devlanes certs trust` on the client is enough, and the traffic goes to the box as a plain stream through the connection. Only names under `.localhost` (or your `hostnames.tld`) are accepted; others are listed under `skipped_hosts` and never get a route or a certificate. Hostnames belong to the box: its services got their public URLs when they started. When one of a box lane's hostnames is already served by a local lane, `remote connect` warns and skips that whole box lane (its ports too) instead of renaming it, since a renamed host would break CORS origins and cookies.

### Several boxes

One client can connect to several boxes at once. When two of them serve the same hostname, as they do when both run lane 0 of the same project, the box connected first keeps it. The later box's lane is skipped whole with a `lane_skipped` warning naming the first box, and `remote ls` lists it under `skipped_lanes` with the reason, for example `box hetzner, connected earlier, serves its hostnames`. When a local lane serves one of those names too, the reason is `this machine serves its hostnames`, since local lanes win over every box. The lane is mirrored as soon as the first box stops serving those names or disconnects.

### Proxy port

A box generates its URLs, `DEVLANES_ORIGINS` and CORS lists with the port its own proxy serves HTTPS on. On a box that cannot bind port 443 that is the fallback 8443, so its URLs read `https://web.acme.localhost:8443`. When the box's port differs from this machine's, the client proxy also listens on `127.0.0.1` and `::1` at the box's port, with the local CA, so those URLs open here unchanged. That port serves only the hostnames of boxes using it; local hostnames and other boxes' hostnames get the unknown-host answer there (over HTTPS the handshake fails). Every box hostname also works on this machine's own HTTPS port. The extra port stays open while the box is connected or reconnecting, like mirrored ports, and closes when no connected box needs it. `remote ls --json` shows the box's port as `https_port`.

When this machine cannot bind that port, because another process holds it or it is a low port this user may not bind, `remote connect` and `remote ls` show a `proxy_port_in_use` warning, for example `the box's URLs use HTTPS port 8443, which this machine cannot serve: in use by nginx (pid 812); open its hostnames on port 443 instead`. The extra port listens on loopback only, so on macOS a port below 1024 needs the daemon to run as root even where this machine's own proxy binds one. The port is tried again every 30 seconds, when the box reconnects and when you run `remote connect` for it again. Only HTTPS needs this: the box's URLs never name its proxy's plain HTTP port.

New lanes, services and containers on the box appear live; removed ones are unbound. Connections to a unit that stops or becomes hidden are closed. Task runs of the box's lanes are listed with their state only; they are never mirrored.

A connection to a mirrored port that cannot reach the box's service is closed and reported with one of these messages:

- `the unit is not running on the box`, `the unit is hidden on the box` or `no running unit on the box has this port`: the box refused it.
- `nothing on the box accepted it: <error>`: the unit is listed but nothing listens on the port on the box's loopback.
- `the box did not answer in time`, `the link dropped while it opened` or `the link to the box was lost: <error>`.
- `box hetzner is not connected: <error>`: the link stayed down for 10 seconds.

`remote ls` shows the latest one as the port's `last_error` until a connection gets through, and an `open_failed` warning goes to `remote connect` and event subscribers, at most once a minute per port and reason. A failure on a port that `remote ls` does not list, reached only through a hostname's relay, shows only as the warning.

## Client URLs on the box

The URLs devlanes prints on the box are the box's own. Each connected client tells the box what it mirrors, so commands on the box, and agents working there, can say what opens on the client. After its mirror changes, a client sends the box a report over the connection, at most a few times a second and only when something changed: every mirrored port with its state (`bound`, `in_use` with what holds it, `local_lane`, `failed` or `unbound`), the box lanes it skips and why, the box hostnames it does not route, its own HTTPS port, whether it serves the box's HTTPS port too ([proxy port](#proxy-port)), and the name it connects to the box by (`alias`, such as the ssh host in `devlanes remote connect hetzner`). The box keeps the latest report in memory with the client's `status` entry, as `clients[].mirror` in `status --json`, and drops it when the client's connection ends. The box still opens nothing on the client.

In a terminal, `devlanes url print <unit>` on the box prints one line per connected client after the box's URL; piped and for agents, it prints the box's URL alone, as before:

```text
https://web.acme.localhost:8443
on laptop: https://web.acme.localhost
on desktop, not reachable: port 20012 is in use there by postgres (pid 812)
```

A hostname opens on the client through its proxy, on the box's HTTPS port when the client serves that one and on its own otherwise, unless the client skips the lane or the name or its proxy serves no HTTPS. A unit without a hostname, or whose hostname does not open there, opens at `http://127.0.0.1:<port>` (or `[::1]`) once the client bound its port. Hidden units and tasks open nowhere. `--json` adds the same as `clients: [{client, id, alias, url, reachable, reason}]`, an empty list when no client reports. `devlanes url ls` adds a line per client counting the hostnames that open there, and `clients` to each route in `--json`. Clients of releases without reports are listed in `status` but get no URLs.

## External ports

A server devlanes does not run, such as `python -m http.server 8000` started with `devlanes exec` or a pgAdmin you run yourself, reaches clients once the box's config declares it under `infra.external`. Put it in `devlanes.yaml` when the whole team runs it, or in `devlanes.local.yaml` on the box when only you do ([local overrides](guides/ports-and-env.md#local-overrides)):

```yaml
# devlanes.local.yaml on the box
infra:
  external:
    docs:
      address: 127.0.0.1:8000
      hosts: "docs.${{ lane.domain }}"
```

Every lane of the project lists `docs` as a unit of kind `external`, and the box's proxy routes `docs.<lane domain>` to the port. While a lane of the project runs a service, task or exec command, or has its containers up, clients mirror the port under the same number and route the hostnames like a service's; otherwise the port is only kept free on the client. Only then does the box dial the port, whether or not anything listens there; a connection then fails like any unit's when nothing does. The box dials exactly the declared address, `127.0.0.1` or `[::1]`, never the other one. `hidden: true` on the entry keeps it on the box. Clients of releases without external ports are not sent them and cannot dial them. [External ports](guides/infra.md#external-ports) covers the address and hostname rules, which are the same on a single machine.

## Hidden

Mark what must stay on the box:

```yaml
services:
  admin:
    run: ./admin-server
    hidden: true        # not mirrored, and the box refuses streams to its ports
infra:
  hidden: [postgres]    # containers by name
  external:
    pgadmin:
      address: 127.0.0.1:5050
      hidden: true      # an external port is hidden on its own entry
```

Hidden units still show in `devlanes status` on the box.

## Sync

`services.<name>.sync` entries of running box lanes stream to `~/.devlanes/sync/<box>/<project>/<lane>/<folder>` on the connected machine (under `$DEVLANES_HOME` when set); the box's `to:` is not used remotely. On every connection the client sends a list of the files it already holds, in batches of 2,000, and the box compares each batch with its source as it arrives. Once the list is complete the box sends only new and changed files, then deletes the client's files that the source lacks. The first connection copies everything; a reconnect over an unchanged folder of 50,000 files writes nothing. A dropped connection during the comparison starts over on the next one. `remote sync` shows the state as `comparing` meanwhile. After that, changes and deletions follow live. devlanes marks that folder as its own and refuses to write into an existing folder it did not create. Files larger than 256 MiB are reported and skipped. File contents travel in chunks of 1 MiB, each compressed with zstd when that makes it smaller, and so do the client's file lists. Chunks under 256 bytes go raw, and once a chunk of a file does not get smaller, the rest of that file goes raw too. For the rest of the traffic, ssh compression is a setting in your `~/.ssh/config` ([slow links](guides/remote-setup.md#slow-links)). Two entries in one box lane whose folders have the same name (replicas of one service included) would share a destination, so neither is synced and `remote sync` and `remote ls` show the error. Typical uses are a browser extension built on the box and loaded unpacked locally, or an APK to `adb install`.

## In the TUI

The TUI on the client shows each connected box's projects after the local ones, labelled with the box: `acme @hetzner`. Their lanes and units use the same list as local ones, read-only. For a box unit the pane shows its details instead of a terminal: its state on the box, each port and whether it is mirrored here (or in use here, and by what), each hostname with the URL that opens it on this machine, and its synced folders. `o` opens that URL. Keys that would start, stop, restart, attach, run, clean or read logs do nothing for a box lane; the pane's first line says what to run on the box instead, for example `r does not restart it here. On hetzner: ssh hetzner, then devlanes restart acme@1:web`. The monitor shows only this machine's units.

`B` opens the boxes view, next to the monitor and the daemon page. Each box shows its link state and transport, the error and next retry while it reconnects, and the `devlanes remote connect <box>` that resumes a box waiting for a sign-in or pairing. Below that are the box's devlanes version, proxy port and `ports.range`, any extra HTTPS port this machine serves for it, skipped lanes and hostnames, syncs, and warnings such as a different release or linger off.

The TUI reads all of it from `remote ls --json`. Its entries carry the box's `registry` (lanes and units with states, ports and hostnames), the box's handshake info under `box`, this machine's HTTPS port and the box's under `proxy`, the name `suffixes` box hostnames must end in, and `connected_since_ms`. Each mirrored port has a `state`: `bound` (listening on at least one loopback address), `in_use` (something here holds it), `local_lane` (a local lane's block has it), `failed` (the bind failed with nothing holding the port, such as a port Windows reserves) or `unbound`. Each route has the `url` that opens it here and its `state`. The client daemon announces changes to all of this with a `remote.mirror.changed` event.

## Processes and data path

Two daemons take part, one on each machine. The client daemon owns the connection: it runs `ssh`, binds the mirrored ports, adds the box's routes to its proxy and writes synced folders. The box daemon owns the box's lanes, as it would without any client. Between them, sshd on the box starts one `devlanes tunnel --stdio` per client connection. The tunnel talks to the client over its stdin and stdout, multiplexed into streams (yamux), and to the box daemon over its local socket (a named pipe on Windows) like any other devlanes command. On Unix clients all of it runs over one ssh ControlMaster per box; Windows clients run one `ssh.exe` per connection. To find devlanes, the client runs a probe through the box's shell; when that shell has no `sh` (cmd.exe or PowerShell on a Windows box), a PowerShell probe also names the shell, and the tunnel command is quoted for it.

Before anything else the tunnel writes one line, `devlanes-tunnel 4`, naming its protocol version. A login shell or profile file on the box may print to stdout first (Tailscale SSH runs `su -l`, for example); the client drops up to 64 KiB of such output before that line and logs it at debug level. When the line never comes, the connection fails with the dropped output in the error. Then the client opens a control stream: it sends the protocol version, its release, its id and host name, the protocol features it implements and, once paired, its remote token. A box that requires pairing and does not accept the token answers that the client must pair; the client then sends a code once and gets a new token, or the box ends the session. The box answers a client it serves with its version, its features, the port its proxy serves HTTPS on and its `ports.range`, then the first copy of its registry: lanes, units, states, ports and hostnames, never environments. Data streams (one per mirrored connection) and sync streams follow, all opened by the client. Mirror reports travel on the control stream. The client never sends the box a command.

Both ends must speak the same protocol version, which changes only for changes an older release cannot handle. Anything added later is a named feature that both ends list in the handshake, and a feature is used only when both list it, so releases with the same protocol version work together.

A request from a browser on the client takes this path:

```text
browser -> client proxy (TLS with the local CA)
        -> mirror listener on 127.0.0.1:<port> (or a relay port)
        -> one stream over ssh
        -> devlanes tunnel on the box
        -> 127.0.0.1:<port> on the box (then [::1]:<port>)
```

Raw TCP clients such as `psql` skip the proxy and start at the mirror listener.

The checks are split between the processes:

- The tunnel asks the box daemon whether the client's token is valid, or trades its code, before it sends the box's version or registry. With `remote.require_pairing: false`, it lets every client through. Until the welcome it resets every other stream the client opens. The box daemon ends a revoked client's registry stream, which ends its session.
- The client daemon checks the protocol version in the handshake and stops on a mismatch. A different release with the same protocol, linger off on a Linux box, and a Windows box daemon that stays inside the ssh session become warnings from the box's version label.
- The box daemon builds the registry from the same lanes as `status --all` and the same routes as `hosts`, and streams it to the tunnel: a full copy, then each change. A tunnel that falls behind gets a fresh full copy. The tunnel passes it on to the client. When the box daemon stops or sends something the tunnel cannot read, the session ends and the client reconnects, so it never mirrors a registry the box stopped reporting.
- The tunnel authorizes every stream: the port must belong to a running service or container, or to an external port of a project that runs a lane, and no hidden unit may have it. It then dials loopback on the box. It passes each mirror report to the box daemon, which keeps it bounded in size.
- The client daemon decides what is bound and routed. Local lanes win: a box port inside a local port block is not bound, and a box lane with a hostname a local lane serves is skipped whole. Between boxes, the one connected first keeps a hostname both serve. Box hostnames outside `.localhost` and `hostnames.tld` are refused. It compares the box's HTTPS port from the handshake with its own, and the box's `ports.range` with those of the other connected boxes.
- For a sync, the client sends the list of files it holds and the box compares it with its source, so only differences cross the link.

Reconnects are the client daemon's job. It pings every 10 seconds and treats 20 seconds without an answer as a lost session, then retries with a backoff from half a second up to 30 seconds. Mirrored ports stay bound meanwhile, proxy routes show "Reconnecting", and a connection accepted while the link is down waits up to 10 seconds for it to come back. Each new session starts from the box's current registry and a fresh sync comparison. The link stops retrying for failures that need you. Rejected credentials and a changed host key are permanent before the first successful connection and park the link in `needs_auth` after it, until the next `remote connect`. A rejected login after a prompt that no attached `remote connect` could answer parks it too, also before the first connection. A host name that never resolved and devlanes missing on the box are permanent before the first connection and retried after it. In `needs_auth` the box's listeners and routes stay while the daemon runs. A box that wants a pairing code nobody gives, refuses a code or no longer accepts the token parks the link in `needs_pairing`, before or after the first connection, with the same listeners and routes, until `remote connect` with a new code. A protocol mismatch and local ssh problems (no ssh client on Windows, a control socket path that is too long) are permanent at any time.

The client daemon marks a box as wanted in its state database each time it connects, and clears that on `remote disconnect`, not when the daemon stops. A starting daemon reconnects the wanted boxes in the background, as boxes that connected before, so a name that no longer resolves is retried until `remote disconnect` too. It knows nothing yet about what those boxes run, so their ports and hostnames appear once each box connects. While it restores them, and while any box is connected, reconnecting or waiting for a sign-in or pairing, a daemon started on demand does not exit when idle (`daemon.exit_when_idle`). A box whose link failed for good does not keep the daemon running; it stays wanted, and the next daemon tries it again.

## GUI tools as services

GUI streaming is not part of v1. Tools that already serve HTTP or TCP from the box work as ordinary services through `remote connect`:

```yaml
services:
  desktop:
    # Xpra's HTML5 client at https://desktop.<lane domain>
    run: "xpra start :100 --daemon=no --html=on --bind-tcp=127.0.0.1:${{ self.port }} --start=gedit"
    hosts: "desktop.${{ lane.domain }}"
  adb:
    # Android emulator on the box; locally: ANDROID_ADB_SERVER_PORT=<port> adb devices
    run: "adb -P ${{ self.port }} nodaemon server"
```

serve-sim (iOS simulators on a macOS box) and ws-scrcpy (Android screens in the browser) follow the same pattern: bind the service's port and give it a hostname. Check each tool's own flag for its listen port.

## Limitations

TCP only. UDP ports (DNS servers, WebRTC/TURN, QUIC, game servers) are not mirrored, because the connection has no datagram frames yet.
