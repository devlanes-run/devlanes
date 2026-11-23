# Setting devlanes up

Read this to take a repository without devlanes to a running lane and a URL the user can open. Some steps only the user can do; this file says which, so ask for them instead of working around them.

## 1. Check whether devlanes is installed

```sh
devlanes --version
```

A version line means it is installed; go to step 3. A command that is not found means it is not, or the shell has not picked up its folder yet.

A project can pin devlanes instead: `devlanes` under `devDependencies` of its `package.json`. Then the command is not on `PATH`. Run the package manager's install and call it as `npx devlanes`, `pnpm devlanes`, `yarn devlanes` or `bunx devlanes`, in every command of this skill. One daemon per user serves every project, so a pinned CLI can meet a daemon of another release, started by a global install or by another project:

- `devlanes doctor` prints `warn release: the daemon runs X, this CLI is Y` with `fix: devlanes daemon restart when convenient (it stops services)`. Commands work; leave it.
- A command that fails with `the running daemon is devlanes X (protocol N, this CLI speaks M) and cannot serve this CLI`, exit 4, cannot work until the daemon restarts. `devlanes daemon restart` from the project moves the daemon to the pinned release and restarts every service on the machine, so ask the user first.
- A pinned devlanes does not update itself. `devlanes update` fails with `error: devlanes cannot update this install (a project dependency) itself` and `hint: update devlanes in PATH/package.json`: change the version there when the user asks.

## 2. Install the command

Install only when the user asked for devlanes on this machine. Use a package manager the machine already has. Each installs the same release build:

| Package manager | Command |
|---|---|
| Homebrew (macOS, Linux) | `brew install devlanes-run/tap/devlanes` |
| npm | `npm install -g devlanes` |
| uv | `uv tool install devlanes` |

Use the install script when the machine has none of them, or when the user asks for it. It installs the latest release and checks the download against its published checksums.

| OS | Command | Notes |
|---|---|---|
| macOS, Linux | `curl -fsSL https://devlanes.run/install.sh \| sh` | Installs to `~/.local/bin` and says when that folder is not on `PATH`. |
| Windows | `irm https://devlanes.run/install.ps1 \| iex` in PowerShell | A shell opened before the install does not see the command; call `$env:LOCALAPPDATA\Programs\devlanes\devlanes.exe` by its path, or ask the user to open a new terminal. |

Linux needs glibc; musl systems such as Alpine are not supported. Windows needs Windows 11 24H2 or later and Git for Windows. The project also needs Git, and Docker running when it has containers.

## 3. HTTPS and hostnames: `devlanes setup`

`devlanes setup` trusts the devlanes certificate authority (created in `~/.devlanes/ca`, signs only local development names such as `*.localhost`) in the system store, and, on macOS and Linux, installs a helper for `/etc/hosts` when the system does not resolve `*.localhost` itself. It changes nothing in the repository. `devlanes setup --remove` undoes it.

Check first. This reads and changes nothing:

```sh
devlanes setup --check
```

Each line is `ok`, `todo`, `info` or `skip` with a step (`ca`, `hosts`, `resolver`, `browser`). It exits 0 when nothing is `todo` and 1 otherwise.

The `browser` step registers devlanes as the native host of the devlanes browser extension in each installed browser on its list (Chrome and its channels, Chromium, Edge and its channels, Brave, Vivaldi, and Arc on macOS), and keeps a copy of the manifest at `~/.devlanes/browser/run.devlanes.browser.json` for any other Chromium browser. It is `info` when setup would write or refresh a registration or the copy, and it is never `todo`: it never counts in `missing` or the exit code. With nothing else `todo`, plain `devlanes setup` applies it without asking and exits 0, also without a terminal. It needs no `sudo` and no dialog. It is `skip` when setup can't register anything, for example:

- devlanes runs from `uvx` or `pipx run`, whose cache paths a browser can't rely on
- setup runs from a source checkout or a project dependency, and another install holds every registration
- devlanes can't tell where the home folder is

The line says why. `devlanes setup --remove` deletes only the registrations that start this install, and the copy. A link or key the user made to the copy for another browser stays.

When something is `todo`:

- Without a terminal, which includes agent output, a plain `devlanes setup` refuses with `this needs confirmation; rerun with --yes` and exit 2.
- Run `devlanes setup --yes` only when the user asked you to set devlanes up. Tell them first that their system will ask them:

| OS | What the user does |
|---|---|
| macOS | Confirms the keychain dialog for the `ca` step. |
| Windows | Confirms the dialog Windows shows for each change, such as adding the certificate to the root store. A `resolver` step (only with `hostnames.resolver: resolver`) needs administrator approval your session cannot answer: ask the user to run `devlanes setup`. |
| macOS, Linux, steps that run `sudo` (`hosts`, `resolver`, and `ca` on Linux) | Nothing, when `sudo` works without a password. Otherwise the step fails with `sudo needs a password and this session cannot prompt; run it from a terminal` and `hint: devlanes setup`: ask the user to run `devlanes setup` in their own terminal, where it asks once and `sudo` asks for the password. |

`devlanes setup` is refused by the MCP server; through MCP, ask the user to run it. Firefox and Chrome on Linux keep their own stores: `tls.trust` in the user config adds them, and [Trust the devlanes CA](https://devlanes.run/docs/how-to/hostnames/trust-the-ca.md) covers browsers, tools and runtimes per OS.

## 4. Check the machine: `devlanes doctor`

```sh
devlanes doctor
```

It never starts the daemon. Each line is a check; a line that is not `ok` is followed by a `fix:` line. It exits 1 when a check fails. Shortened real output in agent mode on Windows 11, with the daemon running:

```text
ok daemon: devlanes 0.13.0, pid 49012, pipe \\.\pipe\devlanes-723741743a863791
ok sandbox: not sandboxed
ok docker: Docker 29.9.3
ok proxy: http on port 80
ok proxy: https on port 443
ok tls: CA at C:\Users\you\.devlanes\ca\ca.pem
ok tls: CA trusted by system
ok hosts: *.localhost resolves to loopback; no hosts file entries needed
ok output: mode agent (agent detection)
```

Lines to act on:

- `daemon` not running: normal before the first start; the first command that needs the daemon starts it.
- `sandbox`: your shell is sandboxed. See "When the shell cannot reach the daemon" below.
- `terminal: this terminal ends the processes it started when it closes, so a daemon started here stops with it` (Windows): a daemon your shell starts dies with that terminal. Ask the user to run `devlanes daemon install` and `devlanes daemon start` in their own terminal.
- `proxy` on 8443 or 8080: URLs carry that port. [Choose the proxy's ports](https://devlanes.run/docs/how-to/hostnames/proxy-ports.md) changes it.
- `tls` not trusted: step 3 is not done.

## 5. Write the project file

Read [configuration.md](configuration.md), then write `devlanes.yaml` at the repository root. For a project that already runs under another tool, follow [migration.md](migration.md). A small one, for an API and a Vite app with a `dev` script each:

```yaml
# yaml-language-server: $schema=https://devlanes.run/v1/schema.json
project: myapp
tasks:
  install:
    run: [npm, install]
    when: lane.create
services:
  api:
    run_script:
      file: api/package.json
      name: dev
    hosts: "api.${{ lane.domain }}"
    ready:
      http:
        path: /healthz
  web:
    preset: vite
    run_script:
      file: web/package.json
      name: dev
    hosts: "${{ lane.domain }}"
    env:
      VITE_API_URL: "${{ services.api.public_url }}"
```

Each service must listen on the port devlanes gives it in `PORT`; [configuration.md](configuration.md) says how. `/healthz` stands for a route the app really serves; a check on a missing route never passes.

## 6. Check the file

```sh
devlanes config check
```

It prints `ok myapp (PATH)` and exits 0, or an `error:` naming the file, line and column with a `hint:`. It does not check that a script named in `run_script` exists; the start does.

## 7. Start the lane

```sh
devlanes start
```

The first command that needs the daemon starts it. The start runs containers, setup entries, `lane.create` and `lane.start` tasks, then the services, and returns when every service is ready or one failed. Output in agent mode is a summary line, then one line per service with its loopback URL and its HTTPS URL. A failure comes first with the end of its log; [troubleshooting.md](troubleshooting.md) goes by message.

Commit `devlanes.yaml` when the user agrees, so every worktree has it.

## 8. Report to the user

- The HTTPS URL of each service from the start, as printed.
- What they have to do themselves: the system dialog or `devlanes setup` from step 3, `devlanes daemon install` when the doctor or a sandbox asked for it, and a browser on Linux that needs the CA.
- That they can watch the lanes by running `devlanes` in their terminal, which opens the TUI.
- That the services keep running after your session, and `devlanes stop` in the worktree stops the lane.
- Anything you left out of `devlanes.yaml`, and why.

## When the shell cannot reach the daemon

Some agent hosts run shell commands in a sandbox. devlanes does not start a daemon from a sandboxed shell, and a sandbox may also forbid connecting to a running one. The command exits 4 with:

```text
error: auto-spawn refused in sandbox (REASON); run devlanes daemon install outside the sandbox, then retry
hint: devlanes daemon install
```

REASON is `CODEX_SANDBOX` or `APP_SANDBOX_CONTAINER_ID` (the variable that marked the sandbox), `connecting to the daemon socket is not permitted`, `creating the daemon directory is not permitted`, `Unix socket bind probe failed` or `named pipe probe failed`. `devlanes doctor` runs in the sandbox and reports it on its `sandbox` line. You cannot fix this from inside; ask the user to run in their own terminal:

| OS | The user runs | Effect |
|---|---|---|
| macOS, Linux | `devlanes daemon install` | Registers a login service (launchd or systemd) that starts the daemon now and at each login. |
| Windows | `devlanes daemon install`, then `devlanes daemon start` | The first adds a start at each logon and starts nothing now; the second starts the daemon for this session. |

Then retry. When the sandbox also forbids connecting to the running daemon, the error says `connecting to the daemon socket is not permitted`. Most sandboxes on Windows, Codex's included, are refused by the daemon's pipe. Then the way in is the MCP server, which the host starts outside its sandbox: [mcp.md](mcp.md). What you start through the daemon runs outside the sandbox with the user's rights; say so to the user. [Use devlanes from an agent sandbox](https://devlanes.run/docs/how-to/agents/sandboxes.md) is the page to send them.
