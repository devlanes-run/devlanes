//! Stand-in for an external program in tests. A copy named after the tool reads
//! its behaviour from the file next to it named like itself plus `.json`
//! (`mise.exe.json`); see `devlanes_testing::fake_tool::sidecar`.

use std::{fs::OpenOptions, io::Write, process::ExitCode, time::Duration};

use devlanes_testing::fake_tool::{Behaviour, expand, expand_json, sidecar};

/// Runs the behaviour; a fake that cannot read it exits 101 with the reason.
fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("fake-tool: {error}");
            ExitCode::from(101)
        }
    }
}

/// Checks arguments, records them, sleeps, prints and returns the exit code.
fn run() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let behaviour: Behaviour =
        serde_json::from_slice(&std::fs::read(sidecar(&std::env::current_exe()?))?)?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    if behaviour
        .args
        .as_ref()
        .is_some_and(|expected| *expected != args)
    {
        eprintln!("bad args: {}", args.join(" "));
        return Ok(ExitCode::from(2));
    }
    if let Some(record) = &behaviour.record {
        let mut file = OpenOptions::new().create(true).append(true).open(record)?;
        writeln!(file, "{}", serde_json::to_string(&args)?)?;
    }
    if behaviour.sleep_ms > 0 {
        std::thread::sleep(Duration::from_millis(behaviour.sleep_ms));
    }
    let cwd = std::fs::canonicalize(std::env::current_dir()?)?
        .to_string_lossy()
        .into_owned();
    let env = |name: &str| std::env::var(name).ok();
    let mut stderr = std::io::stderr().lock();
    stderr.write_all(expand(&behaviour.stderr, &cwd, &env).as_bytes())?;
    stderr.flush()?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(expand(&behaviour.stdout, &cwd, &env).as_bytes())?;
    if let Some(json) = &behaviour.stdout_json {
        serde_json::to_writer(&mut stdout, &expand_json(json, &cwd, &env))?;
    }
    stdout.flush()?;
    if behaviour.echo_stdin {
        std::io::copy(&mut std::io::stdin().lock(), &mut stdout)?;
        stdout.flush()?;
    }
    drop(stdout);
    if let Some(shell) = &behaviour.run_with {
        let command = args.last().ok_or("run_with needs an argument to run")?;
        return run_like_sshd(shell, command, &behaviour.run_env);
    }
    Ok(ExitCode::from(behaviour.exit))
}

/// Runs `command` as Win32-OpenSSH's sshd runs a session's command with the
/// default shell `shell`: cmd.exe gets the raw line `/c "<command>"`,
/// PowerShell the arguments `-c <command>`. Returns the shell's status.
#[cfg(windows)]
fn run_like_sshd(
    shell: &str,
    command: &str,
    env: &[(String, String)],
) -> Result<ExitCode, Box<dyn std::error::Error>> {
    use std::os::windows::process::CommandExt as _;

    let system = std::path::PathBuf::from(std::env::var_os("SystemRoot").ok_or("no SystemRoot")?)
        .join("System32");
    let mut child = match shell {
        "cmd" => {
            let mut child = std::process::Command::new(system.join("cmd.exe"));
            child.raw_arg(format!("/c \"{command}\""));
            child
        }
        "powershell" => {
            let mut child =
                std::process::Command::new(system.join(r"WindowsPowerShell\v1.0\powershell.exe"));
            child.args(["-c", command]);
            child
        }
        other => return Err(format!("unknown shell {other}").into()),
    };
    child.envs(env.iter().map(|(name, value)| (name, value)));
    let status = child.status()?;
    let code = status.code().and_then(|code| u8::try_from(code).ok());
    Ok(ExitCode::from(code.unwrap_or(1)))
}

/// Only Windows shells are emulated.
#[cfg(not(windows))]
fn run_like_sshd(
    shell: &str,
    _command: &str,
    _env: &[(String, String)],
) -> Result<ExitCode, Box<dyn std::error::Error>> {
    Err(format!("run_with {shell} needs Windows").into())
}
