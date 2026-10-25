//! The agent plugin's PowerShell hook (`devlanes __agent-hook --shell
//! powershell`): which PowerShell commands it blocks, and that it says what
//! the Bash hook says.

use std::path::Path;

use devlanes_cli::agent_hook::{BLOCK_MESSAGE, Verdict, decide, deny_output};

/// A project at `<tmp>/project` with a `web` folder, and `<tmp>/other` outside it.
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("project/web")).expect("project");
    std::fs::create_dir_all(dir.path().join("other")).expect("other");
    std::fs::write(dir.path().join("project/devlanes.yaml"), "project: acme\n").expect("config");
    dir
}

/// The verdict for `command` run from `cwd` by the PowerShell tool.
fn verdict(cwd: &Path, home: &Path, command: &str, background: bool) -> Verdict {
    let payload = serde_json::json!({
        "session_id": "s",
        "cwd": cwd,
        "tool_name": "PowerShell",
        "tool_input": {"command": command, "run_in_background": background},
    });
    decide(&payload.to_string(), Path::new("/unused"), Some(home))
}

#[test]
fn blocks_backgrounded_dev_servers_in_powershell() {
    let tmp = workspace();
    let project = tmp.path().join("project/web");
    let home = tmp.path().join("project");
    for command in [
        "pnpm dev &",
        "npm run dev & Start-Sleep 5; curl http://localhost:5173",
        "pnpm dev | Tee-Object dev.log &",
        "pnpm dev 2>&1 &",
        "& 'C:\\tools\\vite.cmd' --port 5173 &",
        "$job = pnpm dev &",
        "Start-Process npm -ArgumentList 'run','dev'",
        "Start-Process -FilePath pnpm -ArgumentList \"dev\" -NoNewWindow -PassThru",
        "start npm 'run dev'",
        "saps node server.js",
        "Start-Process python -ArgumentList '-m','http.server','8000' -RedirectStandardOutput out.log",
        "Start-Process pwsh -ArgumentList '-NoExit','-Command','pnpm dev'",
        "Start-Job { pnpm dev }",
        "$job = Start-Job -ScriptBlock { Set-Location web; npm start }",
        "Start-ThreadJob -ScriptBlock { vite }",
        "sajb { & pnpm dev }",
        "cmd /c start pnpm dev",
        "cmd.exe /c \"start /b npm run dev\"",
        "cmd /c start \"\" /d web pnpm dev",
        "cmd /c start \"Dev Server\" pnpm dev",
        "cmd /c 'start \"Dev\" pnpm dev'",
        "if ($true) { pnpm dev & }",
        "Write-Output 'a & b'; pnpm dev &",
        "# start the server\npnpm dev &",
        "<# a comment with { #> pnpm dev &",
        "pnpm `\n  dev &",
        "Start-Process npm -ArgumentList 'run','dev' -Wait:$false",
        "Start-Process -ErrorAction Stop vite",
        "Start-Process -ea:Stop -OutVariable p vite",
        "& \"C:\\Program Files\\nodejs\\npm.cmd\" run dev &",
    ] {
        assert_eq!(
            verdict(&project, &home, command, false),
            Verdict::Block,
            "{command}"
        );
    }
    assert_eq!(
        verdict(&project, &home, "pnpm dev", true),
        Verdict::Block,
        "run_in_background"
    );
    let other = tmp.path().join("other");
    for command in [
        "cd ../project; pnpm dev &",
        "Set-Location -Path ../project/web; Start-Process npm -ArgumentList 'run','dev'",
        "Start-Process npm -ArgumentList 'run','dev' -WorkingDirectory ../project",
        "Start-Job -WorkingDirectory ../project { pnpm dev }",
        "Start-Process npm -ArgumentList 'run','dev' -WorkingDirectory:../project",
        "Start-Job -WorkingDirectory:../project { pnpm dev }",
        "cmd /c start \"Dev Server\" /d ../project pnpm dev",
        "cd ~; pnpm dev &",
    ] {
        assert_eq!(
            verdict(&other, &home, command, false),
            Verdict::Block,
            "{command}"
        );
    }
}

#[test]
fn allows_foreground_one_shot_and_quoted_commands() {
    let tmp = workspace();
    let project = tmp.path().join("project/web");
    let home = tmp.path().join("project");
    for command in [
        "pnpm dev",
        "pnpm build; pnpm test",
        "Start-Sleep 10 &",
        "Get-Content -Wait dev.log &",
        "vite build &",
        "Write-Output 'pnpm dev &'",
        "Write-Output \"npm start `& more\"",
        "echo 'it''s pnpm dev &'",
        "# pnpm dev &",
        "<# pnpm dev & #> Get-ChildItem",
        "git commit -m @'\nStart the dev server's & watcher\nStart-Process npm 'run dev'\n'@",
        "curl.exe \"https://dev.example.invalid/?a=1&b=2\"",
        "cargo build 2>&1 | Select-Object -Last 20",
        "Start-Process npm -ArgumentList 'run','dev' -Wait",
        "Start-Process npm -ArgumentList 'run','dev' -Wait:$true",
        "Start-Process http://localhost:5173",
        "& \"C:\\Users\\Jo Smith\\AppData\\Local\\Programs\\devlanes\\devlanes.exe\" start &",
        "Start-Job { pnpm build } | Wait-Job | Receive-Job",
        "Start-Job -FilePath .\\tasks.ps1",
        "cmd /c start /wait pnpm dev",
        "cmd /c start \"Dev Server\" /wait pnpm dev",
        "cmd /c 'start \"Dev\" /wait pnpm dev'",
        "cmd /c \"start \"\"Dev\"\" /wait pnpm dev\"",
        "cmd /c pnpm dev",
        "Invoke-Command { pnpm dev }",
        "devlanes exec --name web -- pnpm dev",
        "devlanes logs web -f --timeout 30s &",
        "Start-Process devlanes -ArgumentList 'logs','web','-f'",
        "$env:PORT = 3001; pnpm dev",
    ] {
        assert_eq!(
            verdict(&project, &home, command, false),
            Verdict::Allow,
            "{command}"
        );
    }
    assert_eq!(
        verdict(&project, &home, "devlanes start", true),
        Verdict::Allow
    );
    let other = tmp.path().join("other");
    for command in [
        "pnpm dev &",
        "Start-Job { pnpm dev }",
        "cd ../project; cd ../other; pnpm dev &",
        "Start-Process npm -ArgumentList 'run','dev' -WorkingDirectory ../other",
        "Start-Process npm -ArgumentList 'run','dev' -WorkingDirectory:../other",
    ] {
        assert_eq!(
            verdict(&other, &home, command, false),
            Verdict::Allow,
            "{command}"
        );
    }
    for payload in ["", "not json", "{\"tool_input\":{}}", "[]"] {
        assert_eq!(
            decide(payload, &project, Some(&home)),
            Verdict::Allow,
            "{payload}"
        );
    }
}

#[test]
fn falls_back_to_the_process_directory_without_cwd() {
    let tmp = workspace();
    let payload = r#"{"tool_name":"PowerShell","tool_input":{"command":"pnpm dev &"}}"#;
    assert_eq!(
        decide(payload, &tmp.path().join("project"), None),
        Verdict::Block
    );
    assert_eq!(
        decide(payload, &tmp.path().join("other"), None),
        Verdict::Allow
    );
}

#[test]
fn block_message_matches_the_bash_hook() {
    let script = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../agent-plugin/scripts/block-background.sh"
    ))
    .expect("block-background.sh");
    let message = script
        .split_once("<<'EOF'\n")
        .and_then(|(_, rest)| rest.split_once("\nEOF\n"))
        .map(|(message, _)| message)
        .expect("the script's message heredoc");
    assert_eq!(message, BLOCK_MESSAGE);
    let output: serde_json::Value = serde_json::from_str(&deny_output()).expect("JSON");
    assert_eq!(
        output,
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": BLOCK_MESSAGE,
            }
        })
    );
}
