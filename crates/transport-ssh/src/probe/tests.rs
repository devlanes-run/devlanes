use super::*;

/// The platform part of a probe for `os` and `arch`.
fn platform(os: &str, arch: &str) -> RemotePlatform {
    RemotePlatform {
        os: os.into(),
        arch: arch.into(),
    }
}

#[test]
fn probe_output_parses_missing_and_present_binaries() {
    let missing = parse_probe(
        "\ndevlanes-probe:os=Linux\ndevlanes-probe:arch=x86_64\ndevlanes-probe:program=\n",
    );
    assert_eq!(
        missing,
        Some(RemoteProbe {
            platform: platform("linux", "x86_64"),
            program: None,
            shell: BoxShell::Sh,
        })
    );
    let present = parse_probe(
        "devlanes-probe:os=Darwin\r\ndevlanes-probe:arch=arm64\r\n\
         devlanes-probe:program=/usr/local/bin/devlanes\r\n",
    );
    assert_eq!(
        present.as_ref().map(|p| p.platform.arch.as_str()),
        Some("aarch64")
    );
    assert_eq!(
        present.and_then(|p| p.program).as_deref(),
        Some("/usr/local/bin/devlanes")
    );
    assert_eq!(parse_probe("devlanes-probe:os=Linux\n"), None);
}

/// Output a login shell or profile file prints around the probe (Tailscale
/// SSH runs `su -l`) is not the probe's.
#[test]
fn probe_output_ignores_other_lines() {
    let probe = parse_probe(
        "Last login: Mon Oct 26\nLinux\nwelcome to acme\n\
         devlanes-probe:os=Linux\ndevlanes-probe:arch=aarch64\n\
         devlanes-probe:program=/home/acme/.devlanes/bin/devlanes\nlogout\n",
    );
    assert_eq!(
        probe,
        Some(RemoteProbe {
            platform: platform("linux", "aarch64"),
            program: Some("/home/acme/.devlanes/bin/devlanes".into()),
            shell: BoxShell::Sh,
        })
    );
    assert_eq!(parse_probe("Linux\nx86_64\n/usr/bin/devlanes\n"), None);
    let script = probe_script("devlanes");
    assert!(
        script.contains("printf '\\ndevlanes-probe:os=%s\\n"),
        "{script}"
    );
}

/// Windows' own names and the unames of Git Bash, MSYS2 and Cygwin are all
/// Windows; Unix names keep their mapping.
#[test]
fn windows_platform_names_map_to_windows() {
    for (sysname, machine, expected) in [
        ("Windows", "AMD64", platform("windows", "x86_64")),
        ("Windows_NT", "ARM64", platform("windows", "aarch64")),
        (
            "MINGW64_NT-10.0-26100",
            "x86_64",
            platform("windows", "x86_64"),
        ),
        (
            "MSYS_NT-10.0-26100",
            "x86_64",
            platform("windows", "x86_64"),
        ),
        ("CYGWIN_NT-10.0", "x86_64", platform("windows", "x86_64")),
        ("Linux", "x86_64", platform("linux", "x86_64")),
        ("FreeBSD", "amd64", platform("freebsd", "x86_64")),
        ("Windows", "x86", platform("windows", "x86")),
    ] {
        assert_eq!(RemotePlatform::from_uname(sysname, machine), expected);
    }
}

/// What the `sh` probe leaves on stdout when a Windows box's shell cannot
/// run it: nothing from cmd.exe and PowerShell, which report the missing `sh`
/// on stderr (their messages are checked too, in case a box merges the
/// streams), and Git Bash's uname when an `sh` on `PATH` ran it. Each calls
/// for the Windows probe; a Unix box's answer does not.
#[test]
fn a_failed_or_windows_sh_probe_calls_for_the_windows_probe() {
    let refused = [
        "",
        "'sh' is not recognized as an internal or external command,\r\n\
         operable program or batch file.\r\n",
        "sh : The term 'sh' is not recognized as the name of a cmdlet, function, script file, or \
         operable program.\r\nAt line:1 char:1\r\n+ sh -c 'if command -v ...\r\n\
         + CategoryInfo          : ObjectNotFound: (sh:String) [], CommandNotFoundException\r\n\
         + FullyQualifiedErrorId : CommandNotFoundException\r\n",
    ];
    for output in refused {
        let probe = parse_probe(output);
        assert_eq!(probe, None, "{output:?}");
        assert!(needs_windows_probe(probe.as_ref()), "{output:?}");
    }
    let git_bash = parse_probe(
        "devlanes-probe:os=MINGW64_NT-10.0-26100\ndevlanes-probe:arch=x86_64\n\
         devlanes-probe:program=/c/Users/acme/.devlanes/bin/devlanes\n",
    );
    assert!(needs_windows_probe(git_bash.as_ref()), "{git_bash:?}");
    let linux = parse_probe(
        "devlanes-probe:os=Linux\ndevlanes-probe:arch=x86_64\ndevlanes-probe:program=\n",
    );
    assert!(!needs_windows_probe(linux.as_ref()), "{linux:?}");
}

/// The Windows probe's lines, after whatever a PowerShell profile printed,
/// give the platform, the Windows path and the shell; a shell it does not
/// know is taken as cmd.exe.
#[test]
fn windows_probe_output_parses() {
    let output = |shell: &str| {
        format!(
            "profile says hi\r\n\ndevlanes-probe:os=Windows\ndevlanes-probe:arch=AMD64\n\
             devlanes-probe:program=C:\\Program Files (x86)\\devlanes\\devlanes.exe\n\
             devlanes-probe:shell={shell}\n"
        )
    };
    assert_eq!(
        parse_probe(&output("powershell")),
        Some(RemoteProbe {
            platform: platform("windows", "x86_64"),
            program: Some(r"C:\Program Files (x86)\devlanes\devlanes.exe".into()),
            shell: BoxShell::PowerShell,
        })
    );
    for (name, shell) in [
        ("cmd", BoxShell::Cmd),
        ("pwsh", BoxShell::PowerShell),
        ("bash", BoxShell::Sh),
        ("sh", BoxShell::Sh),
        ("nu", BoxShell::Cmd),
        ("", BoxShell::Cmd),
    ] {
        assert_eq!(
            parse_probe(&output(name)).map(|probe| probe.shell),
            Some(shell),
            "{name}"
        );
    }
    let missing = parse_probe(
        "devlanes-probe:os=Windows\ndevlanes-probe:arch=ARM64\ndevlanes-probe:program=\n\
         devlanes-probe:shell=cmd\n",
    );
    assert_eq!(missing.and_then(|probe| probe.program), None);
}

/// Each shell gets the program as one word: sh in single quotes, cmd.exe in
/// double quotes (refusing what it would expand or cannot quote), PowerShell
/// through the call operator with its quotes doubled and the exit status
/// passed on.
#[test]
fn tunnel_commands_quote_the_program_for_each_shell() {
    let unix = "/home/acme/my tools/it's/devlanes";
    assert_eq!(
        BoxShell::Sh.tunnel_command(unix).as_deref(),
        Some(r"'/home/acme/my tools/it'\''s/devlanes' tunnel --stdio")
    );
    let windows = r"C:\Program Files (x86)\R&D's tools\devlanes.exe";
    assert_eq!(
        BoxShell::Cmd.tunnel_command(windows).as_deref(),
        Some(r#""C:\Program Files (x86)\R&D's tools\devlanes.exe" tunnel --stdio"#)
    );
    for refused in [
        r"C:\100%\devlanes.exe",
        r"C:\Wow!\devlanes.exe",
        "C:\\a\"b\\devlanes.exe",
        "C:\\a\nb.exe",
    ] {
        assert_eq!(BoxShell::Cmd.tunnel_command(refused), None, "{refused:?}");
    }
    assert_eq!(
        BoxShell::Cmd
            .command(r"C:\acme\devlanes.exe", "daemon status --json")
            .as_deref(),
        Some(r#""C:\acme\devlanes.exe" daemon status --json"#)
    );
    assert_eq!(
        BoxShell::PowerShell.tunnel_command(windows).as_deref(),
        Some(
            r"& 'C:\Program Files (x86)\R&D''s tools\devlanes.exe' tunnel --stdio; exit $LASTEXITCODE"
        )
    );
    assert_eq!(
        BoxShell::PowerShell
            .tunnel_command("C:\\it\u{2019}s\\100%\\devlanes.exe")
            .as_deref(),
        Some("& 'C:\\it\u{2019}\u{2019}s\\100%\\devlanes.exe' tunnel --stdio; exit $LASTEXITCODE")
    );
}

/// The script inside an `-EncodedCommand` probe.
fn decoded(command: &str) -> String {
    let encoded = command
        .strip_prefix("powershell -NoProfile -NonInteractive -EncodedCommand ")
        .expect("an encoded PowerShell command");
    assert!(
        encoded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')),
        "only base64 characters, which no shell quotes or expands: {encoded}"
    );
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("base64");
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    String::from_utf16(&units).expect("UTF-16")
}

/// The Windows probe travels as base64, so cmd.exe, PowerShell and sh pass it
/// on unchanged, and the script looks the program up by its quoted name.
#[test]
fn the_windows_probe_is_encoded_powershell() {
    let script = decoded(&windows_probe_command("it's-devlanes"));
    assert!(
        script.contains("Get-Command -Name 'it''s-devlanes' -CommandType Application"),
        "{script}"
    );
    assert!(script.contains(r"'.devlanes\bin\devlanes.exe'"), "{script}");
    assert!(
        script.contains("$e -contains $_.Extension"),
        "only PATHEXT extensions count: {script}"
    );
    assert!(
        script.contains("`ndevlanes-probe:os=Windows`ndevlanes-probe:arch=$a"),
        "{script}"
    );
    assert!(script.ends_with("Out-Probe $t\n"), "{script}");
    assert!(
        !script.contains("@NAME@") && !script.contains("@TAG@"),
        "every placeholder filled: {script}"
    );
}

/// The `sh` scripts hold none of cmd.exe's operators, which cmd.exe would
/// act on while parsing `sh -c '<script>'` on a Windows box: no
/// redirection that creates `\dev\null`, no `&&` that splits the line.
#[test]
fn sh_scripts_give_cmd_nothing_to_act_on() {
    let scripts = [
        probe_script("devlanes"),
        #[cfg(windows)]
        session_script("devlanes", "start-1", "ready-1"),
    ];
    for script in scripts {
        assert!(!script.contains(['<', '>', '&', '|', '^']), "{script}");
    }
}

/// A probe error shows what the `sh` probe printed and the end of the
/// PowerShell probe's stderr, where a locked-down box says why.
#[test]
fn probe_errors_keep_the_powershell_stderr() {
    assert_eq!(probe_failure(" motd\n", None), "motd");
    assert_eq!(probe_failure("motd", Some(b" \r\n")), "motd");
    let long = format!(
        "{}Cannot invoke method. Method invocation is supported only on core types in this language mode.",
        "x".repeat(4096)
    );
    let message = probe_failure("", Some(long.as_bytes()));
    assert!(
        message.starts_with("\nthe PowerShell probe printed on stderr: x"),
        "{message}"
    );
    assert!(message.ends_with("in this language mode."), "{message}");
    assert!(message.len() < 2200, "only the tail: {}", message.len());
}

/// A Windows client's session on a Windows box prints its lines in the
/// order the `sh` session does, then runs the tunnel and passes on its exit
/// status, also through a PowerShell that sshd runs it with.
#[cfg(windows)]
#[test]
fn windows_sessions_print_the_probe_between_the_lines() {
    let command = windows_session_command("devlanes", "start-1", "ready-1", BoxShell::Cmd);
    let script = decoded(&command);
    assert!(
        script.contains("Out-Probe (\"`nstart-1`n\"+$t+\"ready-1`n\")\n"),
        "{script}"
    );
    assert!(
        script.ends_with("if($p){& $p tunnel --stdio; exit $LASTEXITCODE}\n"),
        "{script}"
    );
    assert_eq!(
        windows_session_command("devlanes", "start-1", "ready-1", BoxShell::PowerShell),
        format!("{command}; exit $LASTEXITCODE")
    );
    assert_eq!(
        windows_session_command("devlanes", "start-1", "ready-1", BoxShell::Sh),
        command
    );
}

/// Windows sessions read the probe from the lines after the start line,
/// whatever a login shell printed first.
#[cfg(windows)]
#[test]
fn session_headers_use_the_lines_after_the_start() {
    let lines = |text: &str| text.lines().map(str::to_owned).collect::<Vec<_>>();
    let present = parse_session_header(
        &lines(
            "welcome from .bashrc\nno newline here\nstart-1\n\ndevlanes-probe:os=Linux\n\
             devlanes-probe:arch=x86_64\ndevlanes-probe:program=/home/acme/.devlanes/bin/devlanes",
        ),
        "start-1",
    );
    assert_eq!(
        present.and_then(|probe| probe.program).as_deref(),
        Some("/home/acme/.devlanes/bin/devlanes")
    );
    let missing = parse_session_header(
        &lines(
            "start-1\ndevlanes-probe:os=Linux\ndevlanes-probe:arch=aarch64\ndevlanes-probe:program=",
        ),
        "start-1",
    );
    assert_eq!(
        missing,
        Some(RemoteProbe {
            platform: platform("linux", "aarch64"),
            program: None,
            shell: BoxShell::Sh,
        })
    );
    // Login output glued to the start line is not the start line.
    assert_eq!(
        parse_session_header(
            &lines("motdstart-1\ndevlanes-probe:os=Linux\ndevlanes-probe:arch=x86_64"),
            "start-1"
        ),
        None
    );
    let script = session_script("devlanes", "start-1", "ready-1");
    assert!(
        script.starts_with("printf '\\n%s\\n' 'start-1'; p=$(command -v 'devlanes');"),
        "{script}"
    );
    assert!(script.contains("printf '%s\\n' 'ready-1'"), "{script}");
    assert!(
        script.ends_with("exec \"$p\" tunnel --stdio; fi"),
        "{script}"
    );
}

/// The probe and the tunnel command run in the real shells, started the way
/// Win32-OpenSSH's sshd starts a session's command (`fake-tool`'s
/// `run_with`): cmd.exe, Windows PowerShell, PowerShell 7 and Git Bash. A
/// shell this machine lacks is skipped with a note; the CI runners have all.
#[cfg(windows)]
mod shells {
    use std::path::{Path, PathBuf};
    use std::process::{ExitStatus, Stdio};
    use std::time::Duration;

    use devlanes_testing::fake_tool::{Behaviour, SshdShell, install};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The shells installed here, each with its executable and the shell the
    /// probe should name.
    fn shells() -> Vec<(SshdShell, PathBuf, BoxShell)> {
        SshdShell::ALL
            .into_iter()
            .filter_map(|shell| {
                let Some(program) = shell.program() else {
                    eprintln!("skipping {shell:?}: not installed here");
                    return None;
                };
                let expected = match shell {
                    SshdShell::Cmd => BoxShell::Cmd,
                    SshdShell::WindowsPowerShell | SshdShell::Pwsh => BoxShell::PowerShell,
                    SshdShell::GitBash => BoxShell::Sh,
                };
                Some((shell, program, expected))
            })
            .collect()
    }

    /// Every byte value, with the line breaks and the Ctrl-Z that text-mode
    /// pipes would change.
    fn binary_payload() -> Vec<u8> {
        let mut payload: Vec<u8> = (0..=255).collect();
        payload.extend_from_slice(b"\r\n\n\r\x1a\x00\x1a\r\nend");
        payload.repeat(64)
    }

    /// `System32`, where cmd.exe and Windows PowerShell live.
    fn system32() -> PathBuf {
        devlanes_sys_windows::file::system_directory().expect("System32")
    }

    /// A `PATH` of `dirs`, then System32 and Windows PowerShell, so neither a
    /// developer's `sh` nor their devlanes is found.
    fn path_of(dirs: &[&Path]) -> std::ffi::OsString {
        let system = system32();
        let mut all: Vec<PathBuf> = dirs.iter().map(|dir| dir.to_path_buf()).collect();
        all.push(system.join(r"WindowsPowerShell\v1.0"));
        all.push(system);
        std::env::join_paths(all).expect("PATH")
    }

    /// Runs `command` as sshd runs it with `shell` at `program`, with `env`
    /// set for the shell, writing `input` to its stdin; the status and stdout
    /// once it ended. The emulation is a fake in `dir`.
    async fn run_like_sshd(
        dir: &Path,
        (shell, program): (SshdShell, &Path),
        command: &str,
        env: &[(&str, std::ffi::OsString)],
        input: &[u8],
    ) -> Result<(ExitStatus, Vec<u8>), Box<dyn std::error::Error>> {
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.to_string_lossy().into_owned()))
            .collect();
        let sshd = install(
            dir,
            &format!("sshd-{}", shell.kind()),
            &Behaviour::new().run_with(shell.kind(), Some(program), env),
        )?;
        let mut child = tokio::process::Command::new(sshd)
            .arg(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdin = child.stdin.take().ok_or("stdin")?;
        let mut stdout = child.stdout.take().ok_or("stdout")?;
        let input = input.to_vec();
        let writing = tokio::spawn(async move {
            let written = stdin.write_all(&input).await;
            drop(stdin);
            written
        });
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).await?;
        writing.await??;
        Ok((child.wait().await?, output))
    }

    /// The Windows probe finds devlanes on `PATH` (an `.exe` in a folder with
    /// spaces and parentheses, not the extensionless file of that name
    /// earlier on `PATH`), else in `%USERPROFILE%\.devlanes\bin`, and names
    /// the shell that ran it.
    #[tokio::test]
    async fn the_windows_probe_finds_devlanes_and_the_shell() -> TestResult {
        tokio::time::timeout(Duration::from_secs(300), async {
            let dir = tempfile::tempdir()?;
            let plain = dir.path().join("plain");
            std::fs::create_dir_all(&plain)?;
            std::fs::write(plain.join("devlanes"), b"#!/bin/sh\n")?;
            let on_path = dir.path().join("Program Files (x86)").join("acme tools");
            std::fs::create_dir_all(&on_path)?;
            let program = install(&on_path, "devlanes", &Behaviour::new())?;
            let profile = dir.path().join("profile");
            let installed = profile.join(r".devlanes\bin");
            std::fs::create_dir_all(&installed)?;
            let fallback = install(&installed, "devlanes", &Behaviour::new())?;
            let empty = dir.path().join("empty");
            std::fs::create_dir_all(&empty)?;
            let command = windows_probe_command("devlanes");
            for (shell, sshd_shell, expected) in shells() {
                let sshd = (shell, sshd_shell.as_path());
                let env = [
                    ("PATH", path_of(&[&plain, &on_path])),
                    ("USERPROFILE", empty.clone().into_os_string()),
                ];
                let (status, stdout) = run_like_sshd(dir.path(), sshd, &command, &env, b"").await?;
                let text = String::from_utf8_lossy(&stdout);
                assert!(status.success(), "{shell:?}: {status} {text}");
                let probe = parse_probe(&text).ok_or_else(|| format!("{shell:?}: {text}"))?;
                assert_eq!(probe.platform.os, WINDOWS, "{shell:?}");
                assert_eq!(probe.shell, expected, "{shell:?}: {text}");
                assert_eq!(
                    probe.program.as_deref().map(Path::new),
                    Some(program.as_path()),
                    "{shell:?}"
                );

                let env = [
                    ("PATH", path_of(&[])),
                    ("USERPROFILE", profile.clone().into_os_string()),
                ];
                let (_, stdout) = run_like_sshd(dir.path(), sshd, &command, &env, b"").await?;
                let text = String::from_utf8_lossy(&stdout);
                let probe = parse_probe(&text).ok_or_else(|| format!("{shell:?}: {text}"))?;
                assert_eq!(
                    probe.program.as_deref().map(Path::new),
                    Some(fallback.as_path()),
                    "{shell:?}"
                );

                let env = [
                    ("PATH", path_of(&[])),
                    ("USERPROFILE", empty.clone().into_os_string()),
                ];
                let (_, stdout) = run_like_sshd(dir.path(), sshd, &command, &env, b"").await?;
                let text = String::from_utf8_lossy(&stdout);
                let probe = parse_probe(&text).ok_or_else(|| format!("{shell:?}: {text}"))?;
                assert_eq!(probe.program, None, "{shell:?}: {text}");
            }
            Ok(())
        })
        .await?
    }

    /// The tunnel command reaches a program whose folder holds spaces,
    /// parentheses, `&` and `'`, with exactly `tunnel --stdio`, through each
    /// shell; every byte crosses stdin and stdout unchanged (whatever a
    /// profile prints before the preamble aside), and the program's exit
    /// status (the stopped box daemon's 75) comes back.
    #[tokio::test]
    async fn tunnel_commands_reach_the_program_byte_for_byte() -> TestResult {
        tokio::time::timeout(Duration::from_secs(300), async {
            let dir = tempfile::tempdir()?;
            let folder = dir.path().join("Program Files (x86)").join("R&D's tools");
            std::fs::create_dir_all(&folder)?;
            let echo = install(
                &folder,
                "devlanes",
                &Behaviour::new()
                    .args(["tunnel", "--stdio"])
                    .stdout("devlanes-tunnel 4\n")
                    .echo_stdin(),
            )?;
            let stopped = install(
                &folder,
                "stopped",
                &Behaviour::new().args(["tunnel", "--stdio"]).exit(75),
            )?;
            let payload = binary_payload();
            let preamble = b"devlanes-tunnel 4\n";
            for (shell, program, box_shell) in shells() {
                let sshd = (shell, program.as_path());
                let command = box_shell
                    .tunnel_command(&echo.to_string_lossy())
                    .ok_or("quotable")?;
                let (status, stdout) =
                    run_like_sshd(dir.path(), sshd, &command, &[], &payload).await?;
                assert!(status.success(), "{shell:?}: {status}");
                let at = stdout
                    .windows(preamble.len())
                    .position(|window| window == preamble)
                    .ok_or_else(|| format!("{shell:?}: no preamble"))?;
                let after = &stdout[at + preamble.len()..];
                assert!(
                    after == payload.as_slice(),
                    "{shell:?}: {} bytes differ",
                    after.len()
                );

                let command = box_shell
                    .tunnel_command(&stopped.to_string_lossy())
                    .ok_or("quotable")?;
                let (status, _) = run_like_sshd(dir.path(), sshd, &command, &[], b"").await?;
                assert_eq!(status.code(), Some(75), "{shell:?}");
            }
            Ok(())
        })
        .await?
    }
}
