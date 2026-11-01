# devlanes-update

## 0.2.1 — 2026-11-01

### Patch changes

- [22d6f20](https://github.com/devlanes-run/devlanes/commit/22d6f2063df1696a4511831d010c63b4a5a0fa6a) Changed the dialog after an update to show breaking changes and up to five highlights.
  
  Press `o` in it to open the release notes. `devlanes update` and the install scripts now end with a link to them instead of the full text. Set `updates.highlights: false` in the user config to stop the dialog from opening by itself.

## 0.2.0 — 2026-11-01

### Patch changes

- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Changed devlanes installed as a project dependency to skip release checks; `devlanes update` and `devlanes doctor` say to update it in the project's package.json, pyproject.toml or requirements file.
- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Changed release checks to run every 4 hours and when the TUI opens, reading the version from the registry devlanes was installed from.
- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Added `devlanes update` and the TUI's `U` key, which install the latest release the way devlanes was installed, restart the daemon and bring its lanes back.
  
  `u` no longer hides the release notice.
- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Fixed `devlanes daemon install` and restarts from a TUI that outlived an update starting a removed binary (a Homebrew Cellar path, or a replaced file on Linux); they now use the install's stable path.
- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Added background installs of patch releases with `updates.mode: auto`; devlanes switches to them the next time you open it.
- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Added a What's new dialog the first time the TUI opens after an update, and `devlanes update --notes`.

