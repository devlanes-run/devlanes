# Updates

The daemon looks for new devlanes releases, and `devlanes update` or `U` in the TUI installs them the same way devlanes was installed, then restarts the daemon into the new version.

## Settings

```yaml
# ~/.devlanes/config.yaml
updates:
  mode: notify # disable, notify (the default) or auto
```

`notify` shows a newer release in command output and the TUI. `auto` does the same and also installs patch releases in the background ([background installs](#background-installs)). `disable` never checks. Checks are also off in debug builds, when the daemon starts with CI variables set, and with `DEVLANES_NO_UPDATE_CHECK=1`. devlanes installed as a project dependency and source builds never check, since the project's manifest or the checkout decides their version. Changes to `updates.*` apply after `devlanes daemon restart`.

## Release checks

The daemon reads the latest version 30 seconds after it starts, then every 4 hours, and when a TUI opens if the last check is over an hour old. It asks the registry devlanes was installed from, with `curl`:

- `install.sh`, `install.ps1`, standalone binaries and cargo: the GitHub release manifest (`releases/latest/download/release-manifest.json`, or `releases/download/next/release-manifest.json` for a [build of main](#builds-of-main)).
- npm, pnpm, Bun and Yarn: `https://registry.npmjs.org/devlanes/latest`.
- uv, uvx, pipx and pip: `https://pypi.org/pypi/devlanes/json`.
- Homebrew: the formula in `devlanes-run/homebrew-tap`.

For a new release it also reads that release's `CHANGELOG.md` and keeps the entries since the running version. Answers are cached in `~/.devlanes/update-check.json`, and a failed request keeps the cached answer. `devlanes update --check` checks at once and shows the result; it never starts a daemon, and when no daemon of this version's protocol runs, or its checks are off, it reads the registry itself. `devlanes update` does the same, so `updates.mode: disable` turns off the background checks and notices, not an update you ask for. When the registry cannot be read, both fail instead of answering from the cache.

## Notices

In human output, a command prints one line on stderr, at most once a day per version:

```text
devlanes 0.9.1 is available (this is 0.9.0): devlanes update
devlanes 0.10.0 is available with breaking changes (this is 0.9.0): devlanes update
```

Installs devlanes cannot update itself (cargo, `uvx`, `pipx run`, pip, a script install in a directory this user cannot write to) name their own command instead. When the daemon runs another version than the CLI, the line says to run `devlanes daemon restart` when convenient, since a restart stops services. Agent, JSON, plain and CI output never get these lines.

The TUI header shows `0.9.1 available · U update` (`available, breaking` for a minor release), `0.9.1 installing…` and `0.9.1 ready · U restart`, and the summary printed when the TUI closes has an `Update` line. `devlanes doctor` shows the version, build, install method and the path that runs it, the update mode and registry, and the latest release with its command. Shown notices and the version the release highlights last showed are kept in `~/.devlanes/notices.json`.

## devlanes update

`devlanes update` checks, then asks: `[Y/n]` for a patch release, while a minor release, which can break what worked before 1.0, lists its breaking changes and asks `[y/N]`. It installs the release, then the new binary restarts the daemon, which starts again the lanes that were running, and prints a link to the release notes. Without a terminal, pass `--yes`.

devlanes installs and updates to the latest release only, and never to one older than the running build. Installing an older release is not recommended, since it may not open the state database a newer build migrated. If you need one, install the npm or PyPI package with that version: `npm install -g devlanes@X.Y.Z`, `uv tool install devlanes==X.Y.Z` or `pipx install devlanes==X.Y.Z`.

How it installs depends on how devlanes was installed:

- `install.sh`, `install.ps1` and standalone binaries in a directory you can write to: it downloads the release archive into `~/.devlanes/update/`, checks it against the release's `SHA256SUMS`, its exact entries and the version the new binary reports, and renames the binaries into place.
- npm, pnpm, Bun, Yarn, uv, pipx and Homebrew: it runs the package manager, with its output in `~/.devlanes/logs/update.log`.
- cargo, `uvx`, `pipx run`, pip and script installs in a directory you cannot write to: it prints the command to run yourself.
- A project dependency: it says to update devlanes in the project's `package.json`, `pyproject.toml` or requirements file, since a global command would not change the version the project pins.

One install runs at a time (`~/.devlanes/update/lock`); a `devlanes update` started during a background install waits for it.

In the TUI, `U` asks the same question with the release's changelog lines and the number of lanes the restart starts again. Confirming closes the TUI, runs the update in the terminal, and opens the TUI again on the same lane and view.

## Builds of main

Every commit on `main` is built as a `next` build (such as `0.9.1-next.14`), published only as release archives on the rolling `next` prerelease. Install one with the scripts:

```sh
curl -fsSL https://devlanes.run/install.sh | sh -s -- --channel next
```

```powershell
iex "& {$(irm https://devlanes.run/install.ps1)} -Channel next"
```

Over an existing script install in the same directory, this installs the build of main in its place. A `next` build then updates along `next` builds by itself: `devlanes update` and `updates.mode: auto` read the `next` manifest, which only holds the latest build of main, so it never moves to a stable release on its own. To go back to stable releases, run `devlanes uninstall --keep-data` (or delete the installed files) and install the usual way. To move a Homebrew, npm or PyPI install to `next` builds, uninstall it first, since the scripts stop when another installer's `devlanes` comes first on `PATH`.

## Background installs

With `updates.mode: auto`, the daemon prepares patch releases in the background for the install methods `devlanes update` installs itself. For `install.sh`, `install.ps1` and standalone installs it downloads and checks the release into `~/.devlanes/update/` and leaves the installed binary alone, so commands and the daemon keep matching; the release is put in place the next time you open devlanes at a terminal or run `devlanes update`. Package managers install in the background, and the daemon switches at that same point. Nothing restarts on its own. A background install that failed shows in `devlanes update --check` and in the TUI header, and is tried again at the next scheduled check, by `devlanes update --check` or by `devlanes update`. Minor releases always wait for you.

## Restart when the TUI opens

When `devlanes` opens the TUI, or a start command opens it, and the daemon runs an older version than the binary, devlanes restarts the daemon into the binary first, which starts again the lanes that were running. A patch release restarts without asking. When the daemon is a minor version behind the TUI being opened, devlanes asks first. A project's own devlanes never restarts the shared daemon; it prints a warning. Agent, JSON, plain and CI output never restart the daemon.

A TUI that is open while another one updates reconnects to the new daemon and shows `daemon 0.9.1, TUI 0.9.0 · U reload`; `U` opens the new TUI on the same lane. When the binary that would open is not newer than this TUI (another install comes first, or a project's own devlanes), the TUI stays open and says so.

## Release highlights

Each build carries the highlights of its `CHANGELOG.md`. The first TUI opened on a newer stable release shows them after the welcome, for every release since the version it last showed. The dialog lists every breaking change, then at most five highlights that the release authors picked, and `o` opens the release page for the rest. A release with no breaking change and no highlight shows nothing, which is most patch releases. A fresh home records its version without showing anything, and `next` and local builds never show it. The Esc menu's `highlights` opens the dialog again, and `updates.highlights: false` in the user config stops it from opening by itself.

After `devlanes update` and the install scripts restart the daemon, the terminal prints one line with the link to the release page. Agent, JSON, plain and CI output print nothing.

## Windows

Windows refuses to replace an executable while a process runs from it, unless the file has a second hard link. The daemon, the TUI and `devlanes update` keep one in `%USERPROFILE%\.devlanes\run\exe\` while they run, so npm, pnpm, Bun, Yarn, uv and pipx can replace `devlanes.exe`. The link needs `devlanes.exe` on the same drive as `%USERPROFILE%`; otherwise `devlanes update` says to run `devlanes daemon stop` first, which stops every unit. An `install.ps1` install renames the running `devlanes.exe` to `devlanes.exe.old-<timestamp>` and installs next to it; `devlanes doctor`, or the next `install.ps1` run, deletes the old file once nothing runs it. [Windows](windows.md#updating) has the rest.

## Install scripts

`install.sh` and `install.ps1` install the latest stable release, or with `--channel next` (`-Channel next`) the latest [build of main](#builds-of-main); `--channel stable` is the default. They refuse `--version` (`-Version`) and `DEVLANES_VERSION`, naming the npm and PyPI packages instead. Run on a machine that already has devlanes in the install directory, they hand over to `devlanes update --yes` and exit with its status. They install by themselves instead with `--channel next`, when the directory is not writable (with `DEVLANES_INSTALL_SUDO=1` on macOS and Linux) or when the devlanes there predates `devlanes update`. After installing a new binary while a daemon runs, they restart it with `devlanes update --finish`, which also restarts a daemon of an older release. When the first `devlanes` on `PATH` comes from Homebrew, npm, pnpm, Bun, Yarn, uv, pipx or cargo, they stop and name it, since that one would keep running; `--force` (`-Force`) installs anyway.

## Uninstall

`devlanes uninstall` asks once (naming the processes the daemon would stop), then stops the daemon, removes the login service, runs `devlanes setup --remove` with sudo asked for once, and removes this install: the `install.sh` and `install.ps1` files and, on Windows, the user `PATH` entry; for npm, pnpm, Bun, Yarn, uv, pipx, pip, Homebrew and cargo installs, their remove command. A project's own dependency, `uvx`, `pipx run` and source builds stay, with a line saying why. A step that fails is reported and the rest still run. Last it lists `~/.devlanes`, lane data under a `lanes.data_dir` outside it and a daemon socket outside it, with their sizes, and deletes them only if you answer yes. `--remove-data` deletes them without asking, `--keep-data` keeps them. Without a terminal the command needs `--yes`, and the data then stays unless `--remove-data` is given. `--dry-run` prints the plan and changes nothing. On Windows a running `devlanes.exe` cannot delete itself, so a detached `cmd.exe` deletes it once the command exits. `install.sh --uninstall` and `install.ps1 -Uninstall` run it from the install directory.

```sh
devlanes uninstall --dry-run
devlanes uninstall --yes --keep-data   # no questions, data kept
```
