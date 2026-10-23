---
cargo/devlanes: patch
cargo/devlanes-cli: patch
---

Added a hook for Claude Code's PowerShell tool on Windows to the agent plugin, which blocks dev servers started in the background inside a devlanes project (`pnpm dev &`, `Start-Process`, `Start-Job`, `cmd /c start`) as the Bash hook does.
