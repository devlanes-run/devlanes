//! The login shell's environment: how a call without one from a shell uses it,
//! and reading it from a real shell.

use std::{
    collections::BTreeMap,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

use devlanes_mcp::shell_env::{ShellEnv, ShellEnvError};
use serde_json::{Value, json};

use crate::{LIMIT, Lifecycle, TestClient, connect, failed, text};

/// How often [`profile_resolver`] ran.
static RESOLVED: AtomicUsize = AtomicUsize::new(0);

/// A zsh whose profile adds `FAKE_PROFILE=1`.
fn profile_resolver(
    _exe: &Path,
    env: &BTreeMap<String, String>,
) -> Result<ShellEnv, ShellEnvError> {
    RESOLVED.fetch_add(1, Ordering::SeqCst);
    let mut env = env.clone();
    env.insert("FAKE_PROFILE".into(), "1".into());
    Ok(ShellEnv {
        shell: "zsh".into(),
        env,
    })
}

#[tokio::test]
/// `needs_env`: the call runs again with the login shell's environment, says
/// so, and reads the shell once for the life of the server.
async fn a_lane_without_env_gets_the_login_shells() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(TestClient::new(), Lifecycle::Initialize, |config| {
            config.env.insert("FAKE_NEEDS_ENV".into(), "1".into());
            config.shell_env = profile_resolver;
        })
        .await;
        for _ in 0..2 {
            let result = harness.call("devlanes", &["start"]).await;
            assert!(!failed(&result), "{}", text(&result));
            assert!(
                text(&result).contains(
                    "note: the lane had no environment from a shell, so devlanes mcp read it \
                     from your"
                ),
                "{}",
                text(&result)
            );
        }
        let runs: Vec<(Value, Value)> = harness
            .runs()
            .iter()
            .map(|run| (run["from_lane"].clone(), run["profile"].clone()))
            .collect();
        let lane_then_profile = [(json!("1"), Value::Null), (Value::Null, json!("1"))];
        assert_eq!(
            runs,
            [lane_then_profile.clone(), lane_then_profile].concat()
        );
        assert_eq!(RESOLVED.load(Ordering::SeqCst), 1);
    })
    .await
    .expect("in time");
}

#[tokio::test]
/// When the login shell could not be read and a program is then missing from
/// the lane's PATH, the result says how to fix it and why the shell failed.
async fn a_missing_program_gets_the_fix() {
    tokio::time::timeout(LIMIT, async {
        let harness = connect(TestClient::new(), Lifecycle::Initialize, |config| {
            config.env.insert("FAKE_NEEDS_ENV".into(), "1".into());
            config.env.insert("FAKE_NOT_FOUND".into(), "1".into());
            config.shell_env = |_, _| {
                Err(ShellEnvError::Timeout {
                    shell: "zsh".into(),
                    seconds: 10,
                })
            };
        })
        .await;
        let result = harness.call("devlanes", &["start"]).await;
        assert!(failed(&result));
        let text = text(&result);
        assert!(
            text.contains("got the environment of the devlanes mcp server"),
            "{text}"
        );
        assert!(
            text.contains(
                "run devlanes restart --lane acme@1 in a terminal once, or check why the login \
                 shell failed: zsh did not finish within 10 s"
            ),
            "{text}"
        );
    })
    .await
    .expect("in time");
}

/// A server environment for reading a shell with `home` as its home.
#[cfg(unix)]
fn unix_env(home: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("HOME".to_owned(), home.display().to_string()),
        ("SHELL".to_owned(), "/bin/bash".to_owned()),
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("PWD".to_owned(), "/nonexistent/acme".to_owned()),
        ("SHLVL".to_owned(), "0".to_owned()),
    ])
}

#[test]
#[cfg(unix)]
/// bash's login profile reaches the environment; the variables of the shell
/// process keep the server's values.
fn reads_a_bash_profile() {
    if !Path::new("/bin/bash").exists() {
        eprintln!("skipped: no /bin/bash");
        return;
    }
    let home = tempfile::tempdir().expect("home");
    let bin = home.path().join("acme-bin");
    std::fs::write(
        home.path().join(".bash_profile"),
        "export DEVLANES_TEST_FROM_PROFILE=1\nexport PATH=\"$HOME/acme-bin:$PATH\"\n",
    )
    .expect("profile");
    let env = unix_env(home.path());
    let resolved =
        devlanes_mcp::shell_env::resolve(Path::new(crate::FAKE), &env).expect("bash's environment");
    assert_eq!(resolved.shell, "bash");
    let get = |name: &str| resolved.env.get(name).map(String::as_str);
    assert_eq!(get("DEVLANES_TEST_FROM_PROFILE"), Some("1"));
    let path = get("PATH").unwrap_or_default();
    assert!(path.starts_with(&format!("{}:", bin.display())), "{path}");
    assert_eq!(get("SHLVL"), Some("0"));
    assert_eq!(get("PWD"), Some("/nonexistent/acme"));
    assert_eq!(get(devlanes_mcp::shell_env::RESOLVING_ENV), None);
}

#[test]
#[cfg(unix)]
/// A profile that hangs ends at the timeout, with the shell and what it
/// started killed.
fn a_hanging_profile_times_out() {
    if !Path::new("/bin/bash").exists() {
        eprintln!("skipped: no /bin/bash");
        return;
    }
    let home = tempfile::tempdir().expect("home");
    std::fs::write(home.path().join(".bash_profile"), "sleep 30\n").expect("profile");
    let mut env = unix_env(home.path());
    env.insert(devlanes_mcp::shell_env::TIMEOUT_ENV.into(), "1".into());
    let started = std::time::Instant::now();
    let resolved = devlanes_mcp::shell_env::resolve(Path::new(crate::FAKE), &env);
    assert_eq!(
        resolved,
        Err(ShellEnvError::Timeout {
            shell: "bash".into(),
            seconds: 1
        })
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
#[cfg(windows)]
/// With `DEVLANES_SHELL_ENV=pwsh` or `=powershell` the payload's environment
/// comes back through that PowerShell, when it is installed. Its profile
/// cannot be pointed at a test file (PowerShell finds it through the Documents
/// folder, not an environment variable), so this runs whatever profile the
/// machine has and checks only the round trip.
fn reads_through_powershell() {
    for shell in ["pwsh", "powershell"] {
        let mut env: BTreeMap<String, String> = std::env::vars().collect();
        env.insert(devlanes_mcp::shell_env::SWITCH_ENV.into(), shell.into());
        env.insert(devlanes_mcp::shell_env::TIMEOUT_ENV.into(), "60".into());
        env.insert("DEVLANES_TEST_ROUNDTRIP".into(), "acme".into());
        match devlanes_mcp::shell_env::resolve(Path::new(crate::FAKE), &env) {
            Err(ShellEnvError::NotFound(shell)) => eprintln!("skipped: no {shell}"),
            resolved => round_trips(&resolved.expect("PowerShell's environment"), shell),
        }
    }
}

/// `resolved` came from `shell` with the test's variable, `PATH` under that
/// name only, and without the resolver's marker.
#[cfg(windows)]
fn round_trips(resolved: &ShellEnv, shell: &str) {
    assert_eq!(resolved.shell, shell);
    assert_eq!(
        resolved
            .env
            .get("DEVLANES_TEST_ROUNDTRIP")
            .map(String::as_str),
        Some("acme")
    );
    assert!(resolved.env.contains_key("PATH"));
    assert!(
        !resolved
            .env
            .keys()
            .any(|name| name != "PATH" && name.eq_ignore_ascii_case("path"))
    );
    assert!(
        !resolved
            .env
            .keys()
            .any(|name| name.eq_ignore_ascii_case(devlanes_mcp::shell_env::RESOLVING_ENV))
    );
}
