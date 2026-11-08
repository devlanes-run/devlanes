---
cargo/devlanes: minor
cargo/devlanes-daemon: minor
cargo/devlanes-config: patch
cargo/devlanes-cli: patch
---

Changed `ready.log` to match the lines `devlanes logs` shows, so a line drawn over a spinner or progress text now matches.

Text a program draws only on the alternate screen (full-screen terminal apps) no longer matches, and a prompt or a line still being redrawn matches once the program moves past it or pauses. Match a line the program prints before it switches to full screen, or use a `tcp` or `http` check. Trailing spaces are not part of a logged line, so end a pattern before them: `^password>` or `^password>$` instead of `"^password> "`.
