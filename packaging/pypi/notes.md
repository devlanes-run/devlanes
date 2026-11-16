## The PyPI package

`uv tool install devlanes` and `pipx install devlanes` install the command for your user. `pip install devlanes` installs into the active environment, so the command disappears with that environment.

This package holds the native devlanes binaries; it has no Python code. pip, uv and pipx pick the wheel for your platform: macOS arm64 or x86_64, Linux arm64 or x86_64 with glibc 2.35 or later, Windows 11 x64 or arm64. Other platforms, including Linux with musl, have no wheel.

`devlanes daemon install` records the path of the installed binary. After installing into a different environment, run it again. Update with `devlanes update`. On Windows the daemon and the TUI keep a second hard link to the running `devlanes.exe`, which lets uv and pipx replace it; where they can't make one, `devlanes update` says to run `devlanes daemon stop` first.
