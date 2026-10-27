# Security

devlanes runs as your user, and on every machine its daemon answers only processes of the account that runs it. Between machines, a box hands a connected client only what its lanes run or declare, and the client binds and routes only box ports and names that leave its own lanes alone. Lanes keep ports, hostnames and data apart; they are not a sandbox, and every unit runs with your permissions.

## On one machine

### The daemon's endpoint

On Linux and macOS the daemon listens on a Unix socket, `run/daemon.sock` in the devlanes home (`~/.devlanes`, or `$DEVLANES_HOME` when set). On Linux it uses `$XDG_RUNTIME_DIR/devlanes/daemon-<hash>.sock` instead when that directory belongs to you and has mode 0700. A path too long for a socket moves it to `devlanes-<uid>/` under the temp directory, or under `/tmp`. Every directory that holds the socket is created with mode 0700, an existing one of yours is tightened to 0700, and the daemon refuses to start when one is a symlink, not a directory or another user's. On each connection the daemon compares the peer's uid with its own and drops a connection from any other user without answering. The client makes the same check the other way and refuses a socket that another user's process serves (`the daemon endpoint belongs to another user`).

On Windows the endpoint is a named pipe, `\\.\pipe\devlanes-<hash>`, where the hash covers your account's SID and the devlanes home. Its security descriptor names your account as owner, also when the daemon runs elevated, grants access to your account alone, and carries a medium mandatory label, so lower-integrity processes of your account (sandboxed or low-integrity programs) can neither read nor write it. The pipe refuses clients from other machines. The daemon creates the name with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so when another process created the name first, the daemon refuses to start instead of sharing it, and the client checks that the pipe belongs to your account before it sends anything.

The endpoint has no token. The peer check already limits it to your account, and a token file readable only by you would be readable by exactly the same processes.

### Private directories

The devlanes home and its `run` directory are private, and everything devlanes stores lives inside the home. On Unix new ones are created with mode 0700; the daemon tightens an existing one of yours to 0700 and logs that it did, and it refuses a symlink, a non-directory or another user's directory. Its lock file, `run/daemon.lock`, is opened without following symlinks. On Windows they are created owned by you, with a protected access list for you, SYSTEM and Administrators that their contents inherit. An existing empty directory that others can reach is tightened the same way; the daemon refuses to start in a non-empty one, because the entries inside would keep their own grants.

The home holds `state.db` (lanes, port blocks, the services and profiles each lane ran, a box's pairings and a client's remote tokens, never environments), the daemon's and units' logs (whatever your services print), the CA, the client id and the folders synced from boxes. Root on Unix, and an administrator on Windows, can read all of it.

### Processes running as you

Any process running as your user can connect to the daemon and use every method: start, stop and restart units, run commands in a lane with `exec` and `run`, attach to a unit's terminal and type into it, read logs and a lane's resolved environment (`devlanes env`), stop the daemon or reset its state, connect to boxes, and, on a box, mint pairing codes and revoke clients. Units run as you too, so a dev server or a dependency's install script can do the same.

On Windows an elevated daemon still accepts your unelevated processes ([elevated terminals](guides/windows.md#elevated-terminals)). A daemon started from an elevated terminal runs every unit as administrator, so any process of yours can run commands elevated through it. `devlanes doctor` warns when the daemon is elevated, and `devlanes daemon restart` from an unelevated terminal fixes it.

### Sandboxes

A command that finds no daemon starts one, except when it looks sandboxed. It refuses with `auto-spawn refused in sandbox (...)` when `CODEX_SANDBOX` or `APP_SANDBOX_CONTAINER_ID` is set, when connecting to an existing socket or pipe is denied, when it may not create the daemon's directories, or when it cannot create a throwaway socket next to the real one (a throwaway pipe on Windows). The message says to start devlanes outside the sandbox or to run `devlanes daemon install`.

A daemon started outside the sandbox stays outside it. A sandboxed process that is allowed to reach that daemon's endpoint can use every method, and what it asks for runs with the daemon's rights, outside the sandbox. On Windows the pipe's label refuses low-integrity processes; on Unix it depends on whether the sandbox lets the process connect to the socket.

### The daemon's environment

A daemon started on demand keeps only an allowlist of the starting shell's variables: `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `LANG`, `TZ`, `TMPDIR`, `TERM`, `SSH_AUTH_SOCK` (for `remote connect`), `KUBECONFIG` (for boxes reached through `kubectl exec`), the `HTTP_PROXY`, `HTTPS_PROXY` and `NO_PROXY` variables in either case, common CI markers, and anything starting with `LC_`, `XDG_`, `DOCKER_` or `DEVLANES_`. Windows adds the system variables its programs need, such as `SystemRoot`, `USERPROFILE`, `APPDATA`, `TEMP` and `PATHEXT`. Tokens and project variables in your shell stay out of a process that serves every later shell.

Units get the environment of the shell that sent the request (`start`, `lane resume`, `daemon restart`), layered with the lane's config and `.env` ([environment](guides/ports-and-env.md#environment)). The daemon keeps it in memory and never writes it to `state.db`, so after a reboot, a crash or `daemon stop` each lane needs one start from a shell ([bringing a lane back](guides/lifecycle.md#bringing-a-lane-back)).

## The CA

devlanes creates a CA per user at `~/.devlanes/ca/ca.pem`, with its key in `ca-key.pem`. The directory is created with mode 0700 and the key with 0600; an existing directory is checked, never changed. Before loading the key, the daemon refuses it when it is a symlink or when its group or others have any access (on Windows: another owner, other accounts with access, a link or a second hard link, with the `icacls` command that fixes it). The CA is valid for 10 years and name-constrained to `localhost`, `test`, `internal`, `home.arpa` and the `hostnames.tld` configured when it was created, so a client that checks name constraints refuses a certificate it signs for any other name.

The proxy mints a leaf during the TLS handshake for the name the browser asked for (its SNI), plus the lane's domain, and only when its route table has a route for that name. A handshake without SNI or for an unknown name is refused. A leaf is valid for 30 days, never past the CA's expiry, and cached for 24 hours. The daemon never writes a trust store; `devlanes certs trust` runs in your terminal ([trust the CA once](guides/https.md#trust-the-ca-once)). Anyone who can read `ca-key.pem` can mint certificates your browsers trust for the names the CA allows. `tls.ca: mkcert` loads mkcert's root instead, which has no name constraints.

### Hosts file and resolver

`devlanes certs trust --hosts` installs a root-owned helper that keeps one block of `/etc/hosts` in sync with the routed names. The [hosts helper README](../crates/hosts-helper/README.md#security-model) documents what it accepts and refuses, and what any process running as you can do with it. `devlanes certs trust --resolver` writes `/etc/resolver/<tld>` on macOS, a systemd-resolved delegate on Linux, or an NRPT rule on Windows (after an administrator prompt), which sends only that TLD to the daemon's DNS server on `127.0.0.1:15353` (`127.53.0.1:53` on Windows). Both ask for your password, or an administrator's approval, once ([programs that cannot resolve *.localhost](guides/https.md#programs-that-cannot-resolve-localhost)).

## Remote connections

### Roles

Roles come from the endpoint a request arrives on, never from a token. The local socket or pipe is the local role, with every method. `devlanes tunnel --stdio`, which sshd (or a [box command](guides/remote-command.md)) starts once per client connection, is itself a local client of the box daemon, and offers the client on its standard input and output only the mirror protocol: the registry, data streams to authorized ports, sync streams, mirror reports, pings and the pairing exchange. The protocol has no stream that carries a daemon method, so a client cannot start or stop anything, read logs or environments, or run a command on the box through it. The box opens no streams to the client; the client daemon drops any it tries to open.

Over SSH the client runs two things on the box with your SSH login: a probe through the box's shell that reports its OS, its shell and where devlanes is installed, and `devlanes tunnel --stdio`. A box command runs only what its list names.

### Data on the connection

The box sends a client its version label, its protocol features, its proxy's HTTPS port and its `ports.range`, then its registry: project names, lane indexes and domains, and for each service, container, task and external port that is not hidden its name, kind, state, port names and numbers, hostnames and, for an external port, its declared loopback address. Hidden units never reach the client: the tunnel keeps them for its own checks and sends neither their names nor their ports or hostnames, and removes a unit from the client's copy when it becomes hidden. Tasks carry their state only. Data streams carry the bytes of each mirrored connection, and sync streams carry the sync entries of services that are not hidden (lane, unit, the source folder's path on the box and its ignore globs) and the contents and permission bits of the synced files.

The client sends its protocol version, its release, its client id, its host name, its features and, once paired, its remote token. It sends the list of files it holds in each synced folder, and a mirror report after its mirror changes: the name it connects by, its HTTPS port, each mirrored port with its state and, when something else holds it, that program's name and pid or the local lane's name, and the lanes and hostnames it skips. The box keeps the latest report in memory only, with at most 1,024 ports, 256 skipped lanes and 256 skipped hostnames, each text cut to 256 bytes and stripped of control characters, and shows it in `status --json` and `devlanes url`. It keeps the client id and name the same way, cut to 256 bytes. The box also records the client's address from `SSH_CONNECTION`.

Environments, `.env` values, secrets, logs, terminal output, commands and either machine's CA never cross the connection. Everything travels inside the SSH session, or inside whatever the box command uses. Between the tunnel and the box's services the traffic is plain TCP on the box's loopback.

### Dial authorization on the box

The tunnel checks every data stream against its own copy of the registry, which keeps the hidden units the client never sees. The port must belong to a non-hidden service or container that is starting, ready, running or left running by an earlier daemon (`orphaned`), or to an `infra.external` entry of a project with a running lane. A port that any hidden unit lists is refused, whatever else lists it too, and task ports are never dialable. The tunnel then dials `127.0.0.1:<port>`, then `[::1]:<port>`; for an external port it dials only the declared address. When a unit stops, becomes hidden or goes away, the tunnel closes the streams already open to its ports. A server you start on the box outside devlanes is not reachable until the box's config declares it. The box proxy's and DNS server's ports belong to no unit, and `infra.external` refuses to declare them.

Every connection a box service gets from a client comes from the box's own loopback. A service that trusts loopback connections without authentication, such as an admin page, a debugger port or a database without a password, trusts every client the box serves.

### Limits on the client

The client binds a box's ports on `127.0.0.1` and `::1` only. A box port inside one of this machine's lane port blocks is never bound, and a port something here already holds is reported instead of taken (`port 5432 is in use on this machine by postgres (pid 812); not mirrored`). A port held on only one of the two addresses is bound on neither, since a program that reaches `localhost` over the other family would get the box instead of what holds the port. Windows lets a loopback bind succeed next to another program's listener on the wildcard address (`0.0.0.0` or `[::]`), so there the client also reads the system's listener table before it binds a port and leaves such a port alone. A program that starts listening on the wildcard address after the client bound the port gets no connections to `localhost` on it. The box chooses which port numbers it mirrors, so a free port a local program connects to later may lead to the box; `remote ls` lists every mirrored port.

The box's hostnames join the client's proxy and get certificates from the client's CA only when they lie below `localhost`, or below the client's `hostnames.tld` when the client uses the resolver strategy with an accepted TLD: a name needs at least one label in front of that suffix and a wildcard two, so `localhost` itself and `*.localhost` are refused. Other names are listed under `skipped_hosts` and get neither a route nor a certificate, so a box cannot make the client's CA sign a name of its choosing outside those, nor catch every name the client does not route itself. When a box lane's hostname overlaps a route of a local lane, by the same name or through a wildcard on either side (a box `api.acme.localhost` under a local `*.acme.localhost`, or a box `*.acme.localhost` over a local `web.acme.localhost`), the whole box lane is skipped, its ports too. Between two boxes the one connected first keeps a name both serve, by the same rule. When the box's HTTPS port differs from this machine's, the client also listens on loopback at the box's port, for that box's hostnames only ([proxy port](remote.md#proxy-port)).

Mirrored ports and the proxy listen on loopback, so every process on the client machine can connect to them, other users' included, as it can to local services.

### Synced folders

Files from a box's `services.<name>.sync` entries land only in `~/.devlanes/sync/<box>/<project>/<lane>/<folder>` on the client, and the box's `to:` is never used. The project, lane and folder names the box sends must each be a single plain path component, and each file path must be relative, without empty, `.` or `..` components or NUL, and on Windows without names Windows cannot hold (device names such as `con`, `:` and the other reserved characters, a trailing dot or space). The client never follows a symlink in the destination, refuses to write into an existing folder it did not create, and skips files over 256 MiB. Only permission bits up to `0o777` travel, so a synced file can be executable, but devlanes never runs it. A client follows at most 16 entries per box, and a box serves at most 16 per client. There is no limit on a folder's total size or file count, and the box can delete any file in a folder it syncs.

## Pairing

A box serves only clients it paired with ([pairing](remote.md#pairing)). On its first connection a client sends a one-time code from `devlanes daemon pair` on the box and gets a remote token back; every later connection presents its client id and that token.

A code is 8 characters from Crockford's base32 alphabet (no I, L, O or U), 40 random bits, shown as `XXXX-XXXX`. It works once, expires after 10 minutes and lives only in the box daemon's memory; the box keeps at most 16 outstanding codes. Every wrong code voids every outstanding code. After 5 wrong codes within 10 minutes the box refuses every code, and `daemon pair` hands out none, until the oldest of them is 10 minutes old. Failures count across all tunnels of the box. A code is never logged.

A remote token is `dl_rt_` and 256 random bits as 64 lowercase hex digits. The client keeps it per box name in its `state.db`; the box stores only its SHA-256, with the client's id, name, pairing time and last connection. The box compares the presented token's hash with each of that client's pairings in constant time, and compares codes the same way. It logs every refused code and token with the client's id, name and address. A client keeps one pairing per name it connects by, and the box keeps up to 8 pairings per client, dropping the oldest.

A client id is `dl_cid_` and 256 random bits as hex, kept in `client-id` in the client's devlanes home. Clients choose their own id and name and the box shows them in `status` and `daemon clients`, so neither is a credential: the box accepts a client only with the token of one of that id's pairings, or a valid code. Two clients can send the same name; `daemon revoke` then answers that the name is ambiguous, and takes an id or the start of one.

`devlanes daemon revoke <client>` deletes every pairing of that client and ends its sessions at once. `devlanes daemon reset-state` on the box deletes every pairing, voids every code and ends every session; on a client it deletes every token.

### Unpaired clients

Until the box accepts its token or code, a client gets no version label, registry or box info, and every stream it opens other than the control stream is reset. It can still see the tunnel's first line (`devlanes-tunnel 4`), the box's protocol revision when the two differ, what the tunnel writes on stderr before the session (that the box daemon is not running, that the box's remote settings are invalid, that a Windows box daemon stays inside the ssh session), and whether the box asks for a code or refuses one, with the wait after too many. Its connection starts the box daemon when none runs, unless `remote.start_daemon` is false. Over SSH the probe has already reported the box's OS, shell and devlanes path.

### Limits of pairing

Pairing sits behind SSH, or behind whatever the box command uses. SSH decides who reaches the box at all; pairing decides which of the machines that get in see its lanes. Anyone with a shell on the box as you can run `devlanes daemon pair`, or ask the box daemon for a code directly, and so can an agent working there: the bundled devlanes skill tells an agent on a box with no client to run `daemon pair` and give you the code. A code pairs whichever machine sends it first.

A token is a bearer secret. Anyone who can read the client's `state.db`, that is any process running as you there, can copy it with the client id and connect as that machine from anywhere SSH lets them reach the box. Revoking a client does not revoke its SSH access; remove its key from the box's `authorized_keys` as well. A client that can run the tunnel but has no code can still send wrong codes, which voids your outstanding codes and can pause pairing for 10 minutes.

Pairing protects against machines that can run the tunnel but nothing else, such as a key restricted to `devlanes tunnel --stdio` in `authorized_keys` and used through a box command like `[ssh, -T, acme-box]`; `remote connect` over SSH runs a probe first, which a forced command would replace ([pairing](remote.md#pairing)). With `remote.require_pairing: false`, every machine SSH lets run the tunnel gets the registry, the data streams and the syncs.

## SSH and other transports

devlanes uses the system `ssh` with one ControlMaster per box on Linux and macOS, and one `ssh.exe` per connection on Windows. The master reads your `~/.ssh/config`, so host key checking (`known_hosts`, `StrictHostKeyChecking`), keys, agents, certificates, `ProxyJump` and `ProxyCommand` work as they do for `ssh` in a terminal, and so does a setting that weakens them, such as `StrictHostKeyChecking no`. devlanes adds only keepalives, a connect timeout, `ControlPersist=600` for the master and `BatchMode=yes` when it has no way to relay prompts. Passphrase, one-time password and host key prompts go to the terminal running `remote connect`; with none attached, a reconnect that needs one waits in `needs_auth` instead of retrying. The master's socket lives in `run/ssh` in the devlanes home, a 0700 directory; while the master runs, any process of yours can open sessions on the box through it without authenticating. The tunnel needs only a session channel, so `AllowTcpForwarding no` on the box works.

A daemon that the tunnel starts on the box drops `SSH_AUTH_SOCK`, `SSH_CONNECTION`, `SSH_CLIENT`, `SSH_TTY` and `SSH_ORIGINAL_COMMAND` on top of the allowlist above, so that daemon, which outlives the session, keeps neither a forwarded agent's socket nor the session's details. devlanes needs neither agent nor X11 forwarding.

A box command (`docker exec`, `kubectl exec`, `wsl.exe`, `gh codespace ssh`) authenticates however its program does, and its connection is as private as that program's: access to the Docker socket, Kubernetes RBAC on `pods/exec`, a `gh` login ([boxes reached by a command](guides/remote-command.md)). [Tailscale](guides/remote-tailscale.md) can carry the connection, and Tailscale SSH can authenticate it by tailnet identity, which makes the tailnet policy the boundary. [Cloudflare Tunnel](guides/remote-cloudflare.md) puts an Access policy in front of the box's sshd.

## Settings that matter

`remote.require_pairing` in the box's user config, `true` by default, makes the box serve only paired clients; `false` serves every machine SSH lets in. `remote.start_daemon`, also `true` by default, lets any connection start the box daemon, paired or not; with `false` only you start it, and clients report it as stopped and retry. The tunnel reads both at each connection. Any problem under `remote` in that file, other than under `remote.boxes`, turns pairing on and starting off.

`hidden` on a service (`services.<name>.hidden`), in `infra.hidden` for containers, or on an `infra.external` entry keeps that unit on the box: clients never receive its name, ports or hostnames, a hidden service's `sync` folders are not synced, and the box refuses streams to its ports, also when a unit that is not hidden lists the same port ([hidden](remote.md#hidden)).

`infra.external.<name>`, in `devlanes.yaml` for the team or in `devlanes.local.yaml` on the box for one developer ([local overrides](guides/ports-and-env.md#local-overrides)), makes a server devlanes does not run reachable from clients while a lane of the project runs. Its `address` must be `127.0.0.1:<port>` or `[::1]:<port>`; host names and other addresses are a config error, and so is a port that devlanes' own proxy or DNS server uses, or that a lane's service or container uses. devlanes does not know what listens there: whatever holds that address when a client connects gets the connection ([external ports](remote.md#external-ports)).

`proxy.lan` in the user config, `false` by default, makes the proxy listen on every address and accept peers from the network. Every hostname it routes is then reachable by anyone on the network who resolves the name to your machine, the hostnames of connected boxes included, since they go through the same proxy; the error pages show service logs to loopback peers only. Mirrored ports, the extra listener for a box's HTTPS port and the DNS server stay on loopback. On macOS the proxy listens on every address for ports below 1024 even without `proxy.lan`, and drops every peer that is not loopback.

`tls.ca: mkcert` replaces the name-constrained CA with mkcert's unconstrained root.

## Recommendations

- Keep `remote.require_pairing` on, and run `devlanes daemon pair` yourself rather than leaving it to an agent when you can.
- Revoke a machine you retire with `devlanes daemon revoke`, and remove its key from `authorized_keys` too.
- Mark services and containers `hidden` on a box when their ports should stay there, such as admin pages, debuggers and databases without passwords, and give box services authentication rather than trusting loopback.
- Let services on a box listen on loopback; devlanes dials them there, so a wildcard address only adds the box's own network.
- Set `remote.start_daemon: false` on a box where you want to decide when its lanes run.
- Keep `proxy.lan` off unless another device needs your hostnames.
- Keep the devlanes home out of shared backups and synced folders, and do not copy it between machines: it holds the CA key, remote tokens and the client id.
- Leave host key checking on for boxes, and keep `ForwardAgent` and `ForwardX11` off in their `Host` blocks.
- On Windows, start the daemon from an unelevated terminal.
- Run untrusted code in a container or a separate account, not in a lane.

## Reporting a vulnerability

Follow the devlanes-run [security policy](https://github.com/devlanes-run/.github/blob/main/SECURITY.md): email security@arthur.place with the affected version, steps to reproduce and the impact, and for remote connections which machine an attacker needs access to. Do not open a public issue or pull request with exploit details.
