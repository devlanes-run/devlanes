# devlanes-version

## 0.4.1 — 2026-11-16

### Patch changes

- [9f3f2500](https://github.com/devlanes-run/devlanes/commit/9f3f2500338b7170deeb1ae232b9c92eb6099b17) Changed the hint for a missing `devlanes.yaml` to link the docs page on adding devlanes to a project.

## 0.4.0 — 2026-10-28

### Minor changes

- [9c1090e](https://github.com/devlanes-run/devlanes/commit/9c1090e6d6e5bc7557453415dc9732bcab1dfe84) Removed `devlanes remote connect --install`; a box without devlanes now fails with `devlanes is not installed on <box>` and a link to the remote setup guide.
  
  Install devlanes on the box yourself, the same release as on this machine, with `install.sh` (`install.ps1` on Windows), Homebrew, npm or cargo binstall, so that it is on the `PATH` of a non-interactive SSH session or in `~/.devlanes/bin` (`install.sh --dir ~/.devlanes/bin`; `%USERPROFILE%\.devlanes\bin` on Windows). See `docs/guides/remote-setup.md`.

