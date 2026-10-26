//! The command transport against `fake-box`, this crate's stand-in for a box
//! command (`src/bin/fake-box.rs`), which cargo builds for these tests.

use std::time::Duration;

use devlanes_transport::{PreambleError, Session, SessionOptions, Transport};
use devlanes_transport_command::{
    CommandError, CommandOptions, CommandTransport, EXIT_NOT_FOUND, Ready, run,
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

/// Dropping the stream ends the command and, through its job, what it
/// started.
#[cfg(windows)]
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
        assert!(devlanes_sys::process(pid)?.is_some(), "the child runs");
        drop(opened);
        loop {
            if devlanes_sys::process(pid)?.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    })
    .await?
}
