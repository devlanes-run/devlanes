//! The command transport against `fake-box`, this crate's stand-in for a box
//! command (`src/bin/fake-box.rs`), which cargo builds for these tests.

use std::time::Duration;

use devlanes_transport::{PreambleError, Session, SessionOptions, Transport};
use devlanes_transport_command::{
    CommandError, CommandOptions, CommandTransport, EXIT_NOT_FOUND, INTERNAL_ENV, Ready, run,
};
use tokio::io::AsyncReadExt;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// Every test's limit; the fake answers in milliseconds.
const LIMIT: Duration = Duration::from_secs(30);

/// `fake-box` with `steps` as its arguments.
fn fake(steps: &[&str]) -> Vec<String> {
    std::iter::once(env!("CARGO_BIN_EXE_fake-box"))
        .chain(steps.iter().copied())
        .map(str::to_owned)
        .collect()
}

/// A transport running `fake-box` with `steps` for the box `acme`.
fn transport(steps: &[&str]) -> CommandTransport {
    CommandTransport::new(fake(steps), CommandOptions::new("acme"))
}

/// The error of a connect that must fail.
async fn failure(transport: &CommandTransport) -> CommandError {
    match transport.open().await {
        Ok(opened) => panic!("connected: {:?}", String::from_utf8_lossy(&opened.header)),
        Err(error) => error,
    }
}

/// A command that prints a banner and then serves a box connects: the banner
/// is the header, and a session runs over its stdio.
#[tokio::test]
async fn a_box_command_connects_and_keeps_the_banner() -> TestResult {
    tokio::time::timeout(LIMIT, async {
        let transport = transport(&[
            "--print",
            "Last login: today\nmotd ",
            "--preamble",
            "--serve",
        ]);
        let opened = transport.open().await?;
        assert_eq!(opened.header, b"Last login: today\nmotd ");
        drop(opened);
        let session = Session::connect(&transport, SessionOptions::new("test")).await?;
        assert_eq!(session.server(), "fake-box");
        session.close();
        Ok(())
    })
    .await?
}

/// A program that cannot start and an empty command stop the link.
#[tokio::test]
async fn a_missing_program_is_permanent() -> TestResult {
    tokio::time::timeout(LIMIT, async {
        let missing = CommandTransport::new(
            ["devlanes-no-such-program", "tunnel"],
            CommandOptions::new("acme"),
        );
        let error = failure(&missing).await;
        assert!(
            matches!(&error, CommandError::Spawn { program, .. } if program == "devlanes-no-such-program"),
            "{error:?}"
        );
        assert!(error.is_permanent());
        let message = error.to_string();
        assert!(
            message.starts_with("cannot run `devlanes-no-such-program` for acme: "),
            "{message}"
        );
        let Err(error) = missing.connect().await else {
            panic!("a missing program connected");
        };
        assert!(error.is_permanent());

        let empty = CommandTransport::new(Vec::<String>::new(), CommandOptions::new("acme"));
        let error = failure(&empty).await;
        assert!(matches!(error, CommandError::Empty { .. }), "{error:?}");
        assert!(error.is_permanent());
        Ok(())
    })
    .await?
}

/// Status 127 (devlanes missing inside the box) stops the link before the
/// first connection and is retried after one; the stderr is in the message.
#[tokio::test]
async fn exit_127_is_permanent_before_the_first_connection() -> TestResult {
    tokio::time::timeout(LIMIT, async {
        let steps = [
            "--stderr",
            "exec: \"devlanes\": executable file not found in $PATH\n",
            "--exit",
            "127",
        ];
        let error = failure(&transport(&steps)).await;
        let CommandError::Exited { exit, .. } = &error else {
            panic!("{error:?}");
        };
        assert_eq!(exit.code(), Some(EXIT_NOT_FOUND));
        assert!(error.is_permanent());
        let message = error.to_string();
        assert!(message.contains("executable file not found"), "{message}");
        assert!(
            message.contains("check that devlanes is installed on acme"),
            "{message}"
        );

        let mut options = CommandOptions::new("acme");
        options.connected_before = true;
        let restored = CommandTransport::new(fake(&steps), options);
        assert!(!failure(&restored).await.is_permanent());
        Ok(())
    })
    .await?
}

/// Any other failure is retried, with the command's stderr and what it
/// printed in the message.
#[tokio::test]
async fn other_failures_are_transient_with_their_stderr() -> TestResult {
    tokio::time::timeout(LIMIT, async {
        let error = failure(&transport(&[
            "--print",
            "starting\n",
            "--stderr",
            "Error response from daemon: container acme is not running\n",
            "--exit",
            "1",
        ]))
        .await;
        assert!(matches!(error, CommandError::Exited { .. }), "{error:?}");
        assert!(!error.is_permanent());
        let message = error.to_string();
        assert!(
            message.starts_with("the command for acme exited with "),
            "{message}"
        );
        assert!(
            message.contains("container acme is not running"),
            "{message}"
        );
        assert!(
            message.ends_with("; the box printed: starting"),
            "{message}"
        );

        // A clean exit without a preamble is retried too, with its stderr.
        let error = failure(&transport(&["--stderr", "nothing to do\n"])).await;
        assert!(
            matches!(
                &error,
                CommandError::Preamble {
                    error: PreambleError::Ended { .. },
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(!error.is_permanent());
        assert!(error.to_string().ends_with(": nothing to do"), "{error}");
        Ok(())
    })
    .await?
}

/// A tunnel whose box daemon is stopped exits 75 after its preamble: the
/// stream reports the temporary daemon-stopped error with the fix.
#[tokio::test]
async fn exit_75_is_a_stopped_box_daemon() -> TestResult {
    tokio::time::timeout(LIMIT, async {
        let transport = transport(&[
            "--preamble",
            "--stderr",
            "devlanes tunnel: box daemon is not running\n",
            "--exit",
            "75",
        ]);
        let mut stream = transport.open().await?.stream;
        let mut rest = Vec::new();
        let error = stream
            .read_to_end(&mut rest)
            .await
            .expect_err("the tunnel exited 75");
        let inner = error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<CommandError>())
            .ok_or("no command error inside")?;
        assert!(
            matches!(inner, CommandError::DaemonStopped { name } if name == "acme"),
            "{inner:?}"
        );
        assert!(!inner.is_permanent());
        let message = error.to_string();
        assert!(
            message.contains("the devlanes daemon is not running on acme"),
            "{message}"
        );
        assert!(message.contains("`devlanes daemon start`"), "{message}");

        // Before the preamble, as from a wrapper script, it is the same error.
        let error = failure(&self::transport(&["--exit", "75"])).await;
        assert!(
            matches!(error, CommandError::DaemonStopped { .. }),
            "{error:?}"
        );
        Ok(())
    })
    .await?
}

/// A box of another protocol stops the link at once; a silent one stops it
/// before the first connection only.
#[tokio::test]
async fn preamble_problems_follow_the_protocol_rules() -> TestResult {
    tokio::time::timeout(LIMIT, async {
        let error = failure(&transport(&[
            "--line",
            "devlanes-tunnel 99",
            "--sleep",
            "30",
        ]))
        .await;
        assert!(
            matches!(
                &error,
                CommandError::Preamble {
                    error: PreambleError::Protocol { found: 99, .. },
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(error.is_permanent());

        for (connected_before, permanent) in [(false, true), (true, false)] {
            let mut options = CommandOptions::new("acme");
            options.preamble_wait = Duration::from_millis(300);
            options.connected_before = connected_before;
            let silent = CommandTransport::new(fake(&["--sleep", "30"]), options);
            let error = failure(&silent).await;
            assert!(
                matches!(
                    &error,
                    CommandError::Preamble {
                        error: PreambleError::Silent { .. },
                        ..
                    }
                ),
                "{error:?}"
            );
            assert_eq!(error.is_permanent(), permanent, "{error}");
        }
        Ok(())
    })
    .await?
}

/// With a ready line, output up to it gets its own wait and stays in the
/// header; a command that never prints it fails as not ready.
#[tokio::test]
async fn a_ready_line_has_its_own_wait() -> TestResult {
    tokio::time::timeout(LIMIT, async {
        let mut options = CommandOptions::new("acme");
        options.ready = Some(Ready {
            line: "ready-7".to_owned(),
            wait: Duration::from_secs(10),
        });
        let ready = CommandTransport::new(
            fake(&[
                "--print",
                "probe output\nready-7\r\n",
                "--print",
                "after\n",
                "--preamble",
                "--serve",
            ]),
            options.clone(),
        );
        let opened = ready.open().await?;
        assert_eq!(opened.header, b"probe output\nready-7\r\nafter\n");

        options.ready = Some(Ready {
            line: "ready-7".to_owned(),
            wait: Duration::from_millis(300),
        });
        let stuck =
            CommandTransport::new(fake(&["--print", "Password: ", "--sleep", "30"]), options);
        let error = failure(&stuck).await;
        assert!(matches!(error, CommandError::NotReady { .. }), "{error:?}");
        assert!(!error.is_permanent());
        assert_eq!(error.output(), b"Password: ");
        Ok(())
    })
    .await?
}

/// A stopped token ends the commands started with it.
#[tokio::test]
async fn stopping_ends_running_commands() -> TestResult {
    tokio::time::timeout(LIMIT, async {
        let options = CommandOptions::new("acme");
        let stop = options.stop.clone();
        let transport = CommandTransport::new(fake(&["--preamble", "--sleep", "60"]), options);
        let mut stream = transport.open().await?.stream;
        stop.cancel();
        let mut rest = Vec::new();
        let _ = stream.read_to_end(&mut rest).await;
        assert!(rest.is_empty());
        Ok(())
    })
    .await?
}

/// `run` collects a short command's output and status, and ends one that
/// takes too long.
#[tokio::test]
async fn run_collects_output_and_ends_slow_commands() -> TestResult {
    tokio::time::timeout(LIMIT, async {
        let argv = fake(&["--print", "hello\n", "--stderr", "warn\n", "--exit", "3"]);
        let options = CommandOptions::new("acme");
        let output = run(argv, &options, Duration::from_secs(20))
            .await?
            .ok_or("did not finish")?;
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(output.stdout, b"hello\n");
        assert_eq!(output.stderr, b"warn\n");

        let slow = fake(&["--sleep", "30"]);
        let limit = Duration::from_millis(300);
        assert!(run(slow, &options, limit).await?.is_none());
        Ok(())
    })
    .await?
}

/// Set for the copy of this test binary that runs [`helper_internal_variables`].
const HELPER_ENV: &str = "DEVLANES_TRANSPORT_COMMAND_TEST_HELPER";

/// devlanes' own variables never reach a command, even when this process
/// has them. Setting them here would need `unsafe`, so a copy of this test
/// binary gets them and runs the checks in [`helper_internal_variables`].
#[tokio::test]
async fn internal_variables_stay_out() -> TestResult {
    let mut helper = tokio::process::Command::new(std::env::current_exe()?);
    #[cfg(windows)]
    devlanes_testing::no_window(helper.as_std_mut());
    helper
        .args([
            "--exact",
            "helper_internal_variables",
            "--ignored",
            "--nocapture",
        ])
        .env(HELPER_ENV, "1")
        .env("DEVLANES_HOME", "/acme/home")
        .env("DEVLANES_READY_FD", "1")
        .env("DEVLANES_READY_STEPS", "1")
        .env("DEVLANES_ASKPASS_SOCKET", "/acme/askpass")
        .env("ACME_KEPT", "kept")
        .env("ACME_REMOVED", "removed")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(LIMIT, helper.output()).await??;
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("internal variables checked"),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// [`internal_variables_stay_out`]'s checks, in the copy of this binary that
/// has the variables: a command gets neither [`INTERNAL_ENV`] nor
/// `env_remove`, keeps the rest, and still gets what `env` sets.
#[tokio::test]
#[ignore = "run by internal_variables_stay_out"]
async fn helper_internal_variables() -> TestResult {
    if std::env::var_os(HELPER_ENV).is_none() {
        return Ok(());
    }
    tokio::time::timeout(LIMIT, async {
        let names = INTERNAL_ENV
            .into_iter()
            .chain(["ACME_KEPT", "ACME_REMOVED"]);
        let mut steps: Vec<&str> = names.flat_map(|name| ["--env", name]).collect();
        steps.extend(["--preamble", "--sleep", "60"]);
        let mut options = CommandOptions::new("acme");
        options.env_remove = vec!["ACME_REMOVED".into()];
        let opened = CommandTransport::new(fake(&steps), options.clone())
            .open()
            .await?;
        assert_eq!(
            String::from_utf8_lossy(&opened.header),
            "DEVLANES_HOME unset\nDEVLANES_READY_FD unset\nDEVLANES_READY_STEPS unset\n\
             DEVLANES_ASKPASS_SOCKET unset\nACME_KEPT=kept\nACME_REMOVED unset\n"
        );
        drop(opened);

        options.env = vec![("DEVLANES_ASKPASS_SOCKET".into(), "/acme/relay".into())];
        let output = run(fake(&["--env", "DEVLANES_ASKPASS_SOCKET"]), &options, LIMIT)
            .await?
            .ok_or("did not finish")?;
        assert_eq!(output.stdout, b"DEVLANES_ASKPASS_SOCKET=/acme/relay\n");
        println!("internal variables checked");
        Ok(())
    })
    .await?
}

/// Dropping the stream ends the command and what it started, through its job
/// on Windows and its process group on Unix. The child counts as gone once
/// its pid is free, a zombie, or names a process started later.
#[tokio::test]
async fn dropping_the_stream_ends_the_whole_command() -> TestResult {
    tokio::time::timeout(LIMIT, async {
        let transport = transport(&["--child", "--preamble", "--sleep", "60"]);
        let opened = transport.open().await?;
        let header = String::from_utf8_lossy(&opened.header).into_owned();
        let pid: u32 = header
            .trim()
            .strip_prefix("child ")
            .ok_or("no child pid")?
            .parse()?;
        let child = devlanes_sys::identity(pid)?.ok_or("the child is not running")?;
        assert!(devlanes_sys::matches(&child)?, "the child runs");
        drop(opened);
        while devlanes_sys::matches(&child)? {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    })
    .await?
}
