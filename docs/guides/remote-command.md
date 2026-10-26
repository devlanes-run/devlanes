# Boxes reached by a command

Some boxes have no SSH server you would connect to: a Docker container, a Kubernetes pod, a WSL distribution, a GitHub codespace. Each of them has a command that runs a program inside and passes its standard input and output through, and that is all `devlanes remote connect` needs. Give the box a name and that command in your user config, and the name connects like any SSH host. Mirroring, hostnames, ports and synced folders then work as [Remote boxes](../remote.md) describes; this page covers what differs from SSH. Preparing the box itself is the same as in [remote setup](remote-setup.md#on-the-box).

## Set up

On the client, in `~/.devlanes/config.yaml` (`%USERPROFILE%\.devlanes\config.yaml` on Windows):

```yaml
remote:
  boxes:
    acme:
      command: [docker, exec, -i, acme, devlanes, tunnel, --stdio]
```

```sh
devlanes remote connect acme
devlanes remote ls                  # transport: command
```

Or without editing the file:

```sh
devlanes config set remote.boxes.acme.command '[docker, exec, -i, acme, devlanes, tunnel, --stdio]'
```

The key under `boxes` is the name you connect with. It may hold letters, digits, `-` and `_`. A name listed there is never treated as an `ssh` destination, and every other name still is. The command is a list: no shell runs it, so each argument is its own item and nothing is expanded (`~`, `$HOME`, globs). The daemon reads the list at each `remote connect` and when it restores the box after a restart, so a change needs no daemon restart; a box that is already connected keeps its command until `devlanes remote disconnect`.

The command must end in `devlanes tunnel --stdio` inside the box, with stdin and stdout passed through as they are, without a terminal. The tunnel writes one line first, so whatever the command prints before it (a banner, a "starting codespace" line) is skipped, up to 64 KiB.

## Inside the box

devlanes has to be installed in the box, the same release as on the client. Unlike SSH, devlanes does not probe the box for the binary: the command names it, so it must be on the `PATH` the command gets, or written as a full path such as `/home/acme/.devlanes/bin/devlanes`. When it is missing, the command usually exits with status 127, and the first `remote connect` fails with `exited with exit status: 127, so a program it runs was not found` (`exit code: 127` on Windows) and stops retrying.

The tunnel starts the box's daemon when none runs, unless the box's own user config sets `remote.start_daemon: false`; the client then reports the stopped daemon and keeps retrying, as with SSH. The daemon belongs to the user the command runs as, with that user's `~/.devlanes`. It outlives the command, but not the container, pod or distribution it runs in.

Services in the box listen on its loopback or a wildcard address. The tunnel dials them on the box's `127.0.0.1`, so the box's network setup (published ports, a pod's network) plays no part.

## Examples

Docker, with the container already running:

```yaml
remote:
  boxes:
    acme:
      command: [docker, exec, -i, acme, devlanes, tunnel, --stdio]
```

Use `-i` and never `-t`: a terminal would mangle the binary stream. Add `-u <user>` to run as another user than the image's default, and `-w <dir>` when it matters where the tunnel starts. Docker Compose has the same with `docker compose exec -T <service> devlanes tunnel --stdio`, where `-T` turns off the terminal Compose allocates by default.

Kubernetes:

```yaml
remote:
  boxes:
    acme:
      command: [kubectl, exec, -i, acme-0, -c, app, --, devlanes, tunnel, --stdio]
```

`-i` passes stdin; leave out `-t`. Add `--context <name>` or `-n <namespace>` before `--` for another context or namespace than the current one. The pod's name is fixed in the list, so a Deployment's pods, whose names change, need a name you can predict (a StatefulSet's `acme-0`) or `deploy/acme`, which runs in the Deployment's first pod.

A WSL distribution, from Windows:

```yaml
remote:
  boxes:
    ubuntu:
      command: [wsl.exe, -d, Ubuntu, --exec, /home/acme/.devlanes/bin/devlanes, tunnel, --stdio]
```

`--exec` runs the program without the distribution's shell, so the path must be absolute. With devlanes on the system `PATH` (`/usr/local/bin`), `[wsl.exe, -d, Ubuntu, devlanes, tunnel, --stdio]` works too. WSL may stop the distribution, and its daemon with it, some seconds after the last connection ends ([WSL](wsl.md#keep-the-daemon-running) has the setting). WSL also forwards ports that programs in the distribution bind to Windows localhost by default; such a port may then be taken on Windows, and `remote ls` reports it as in use instead of mirroring it. Setting `localhostForwarding=false` under `[wsl2]` in `%UserProfile%\.wslconfig` leaves those ports to devlanes. With `networkingMode=mirrored`, `localhostForwarding` has no effect and ports bound in the distribution are bound on Windows too, so they show as in use; `ignoredPorts` under `[wsl2]` lists ports the distribution may bind anyway.

A GitHub codespace:

```yaml
remote:
  boxes:
    codespace:
      command: [gh, codespace, ssh, -c, acme-x9q7, --, devlanes, tunnel, --stdio]
```

`gh` starts a stopped codespace first, which can take a minute; the command gets two minutes to reach the tunnel. The codespace needs an SSH server (`gh codespace ssh` says how to add one), and `gh` must be signed in (`gh auth login`) before the daemon runs it. Alternatively, `gh codespace ssh --config > ~/.ssh/codespaces` writes an OpenSSH `Host` entry for each codespace to that file. With `Include ~/.ssh/codespaces` at the top of `~/.ssh/config`, connect to the codespace by its host name, and devlanes uses SSH, with a ControlMaster and prompts.

## Differences from SSH

The command runs as a child of your local daemon, with the daemon's environment except its own `DEVLANES_HOME`, and no terminal. Nothing relays its prompts: a password prompt, a device-code login or a "do you trust this host" question has nobody to answer it, so the connection fails or waits until it gives up. Sign in to the tool beforehand (`gh auth login`, `kubectl` credentials, `az login`), and make sure the daemon sees the same environment, such as `KUBECONFIG` or `DOCKER_HOST`; after changing it, `devlanes daemon restart` from the shell that has it.

There is no ControlMaster. Every connection and every reconnect runs the command again, and a reconnect after sleep or a network change is a new `docker exec` or `kubectl exec`.

Failures are reported with the command's exit status and the end of its stderr. The command not starting at all (the program is not installed or not on the daemon's `PATH`) stops the link, and so does a box of another devlanes protocol. On the first connection, status 127, a command that prints no tunnel line within two minutes and one that prints more than 64 KiB before it stop it too; after a first connection they are retried like other failures. Everything else is retried with backoff, including a container that is stopped or does not exist yet, so a typo in its name shows up as an error in `remote ls` that repeats rather than one that stops.

The command ends with the connection, together with whatever it started: on Linux and macOS it leads a process group of its own, and on Windows it runs inside a job. On Windows a program without a directory is looked up on `PATH` with `.exe`, then `.com` added, unless its name already ends in one of them; for a batch file, run it through `cmd`: `[cmd, /c, acme-box.cmd]`.

## Checking

| Message | Cause | Fix |
| --- | --- | --- |
| ``cannot run `docker` for acme: ...`` | the program is not on the daemon's `PATH` | install it, or write its full path in the list |
| `the command for acme exited with exit status: 127, so a program it runs was not found` (`exit code: 127` on Windows) | devlanes is not installed in the box, or not on the `PATH` the command gets | install it there, or write its full path |
| `the command for acme exited with exit status: 1: Error response from daemon: container ... is not running` | the container is stopped | start it; the client keeps retrying |
| `the devlanes daemon is not running on acme` | `remote.start_daemon: false` in the box and its daemon is stopped | `devlanes daemon start` in the box |
| `` `devlanes tunnel --stdio` did not answer within 120s `` | the command waits for something, often a prompt or a codespace that is still starting, or the box runs a devlanes release from before protocol 4 | sign in to the tool first, or install the same release in the box, then connect again |
| `remote.boxes.acme.command: must list the program and its arguments` | an empty command in the user config | fill in the list |
