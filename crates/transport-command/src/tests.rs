use super::*;

/// An exit status built from a process exit code.
#[cfg(unix)]
fn status(code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt as _;
    ExitStatus::from_raw(code << 8)
}

/// An exit status built from a process exit code.
#[cfg(windows)]
fn status(code: i32) -> ExitStatus {
    use std::os::windows::process::ExitStatusExt as _;
    ExitStatus::from_raw(code.cast_unsigned())
}

/// An exit with `code` and `stderr`.
fn exit(code: i32, stderr: &str) -> Exit {
    Exit {
        status: Ok(status(code)),
        stderr: stderr.to_owned(),
    }
}

/// The daemon-stopped status becomes a temporary error that names the box
/// and the fix; other failures keep the stderr; a clean exit is no error.
#[test]
fn a_stopped_box_daemon_is_transient() {
    let stopped = exit_error(
        "acme",
        "the command for acme",
        &exit(
            EXIT_DAEMON_STOPPED,
            "devlanes tunnel: box daemon is not running",
        ),
    )
    .expect("an error");
    assert!(matches!(&stopped, CommandError::DaemonStopped { name } if name == "acme"));
    assert!(!stopped.is_permanent());
    let message = stopped.to_string();
    assert!(message.contains("acme"), "{message}");
    assert!(message.contains("`devlanes daemon start`"), "{message}");
    assert!(message.contains("`devlanes daemon install`"), "{message}");
    assert!(!TransportError::from(stopped).is_permanent());

    let failed = exit_error(
        "acme",
        "the command for acme",
        &exit(1, "Error: No such container: acme"),
    )
    .expect("an error");
    assert!(!failed.is_permanent());
    let message = failed.to_string();
    assert!(
        message.starts_with("the command for acme exited with "),
        "{message}"
    );
    assert!(
        message.ends_with(": Error: No such container: acme"),
        "{message}"
    );
    assert!(exit_error("acme", "x", &exit(0, "noise")).is_none());
}

/// Status 127 stops the link only before the first connection, and says what
/// it means.
#[test]
fn a_missing_program_on_the_box_is_permanent_only_at_first() {
    let missing = || {
        exit_error(
            "acme",
            "the command for acme",
            &exit(EXIT_NOT_FOUND, "sh: devlanes: not found"),
        )
        .expect("an error")
    };
    let first = with_output(missing(), b"motd\n".to_vec(), true);
    assert!(first.is_permanent());
    let message = first.to_string();
    assert!(message.contains("was not found"), "{message}");
    assert!(
        message.contains("check that devlanes is installed on acme"),
        "{message}"
    );
    assert!(message.contains("sh: devlanes: not found"), "{message}");
    assert!(message.ends_with("; the box printed: motd"), "{message}");
    assert_eq!(first.output(), b"motd\n");
    assert!(TransportError::from(first).is_permanent());
    let later = with_output(missing(), Vec::new(), false);
    assert!(!later.is_permanent());
}

/// Another protocol stops the link at any time; silence only before the
/// first connection; a pipe that ends retries.
#[test]
fn preamble_failures_follow_the_first_connection() {
    let transport = CommandTransport::new(["acme"], CommandOptions::new("acme"));
    let none = Mutex::new(None);
    let protocol = || PreambleError::Protocol {
        found: devlanes_transport::PROTOCOL_VERSION + 1,
        output: Vec::new(),
    };
    let silent = || PreambleError::Silent {
        waited: COMMAND_WAIT,
        output: Vec::new(),
    };
    for first in [true, false] {
        let mismatch = transport.classify(protocol(), first, &none);
        assert!(mismatch.is_permanent());
        assert!(
            mismatch.to_string().contains("install the same release"),
            "{mismatch}"
        );
    }
    assert!(transport.classify(silent(), true, &none).is_permanent());
    assert!(!transport.classify(silent(), false, &none).is_permanent());
    let ended = Mutex::new(Some(exit(0, "warning: no tty")));
    let error = transport.classify(
        PreambleError::Ended {
            output: b"bye\n".to_vec(),
        },
        true,
        &ended,
    );
    assert!(!error.is_permanent());
    let message = error.to_string();
    assert!(message.contains("the box printed: bye"), "{message}");
    assert!(message.ends_with(": warning: no tty"), "{message}");
}

/// Output read before the preamble wait comes first in the error's output.
#[test]
fn headers_lead_the_output_of_a_failure() {
    let error = with_header(
        PreambleError::Ended {
            output: b"rest".to_vec(),
        },
        b"probe\nready\n".to_vec(),
    );
    assert_eq!(error.output(), b"probe\nready\nrest");
    let error = CommandError::Preamble {
        error,
        permanent: false,
        stderr: String::new(),
    }
    .skip_output(b"probe\nready\n".len());
    assert_eq!(error.output(), b"rest");
    let gone = with_output(
        exit_error("acme", "x", &exit(1, "")).expect("an error"),
        b"short".to_vec(),
        false,
    );
    assert!(gone.skip_output(99).output().is_empty());
}

/// The first connect yields a pipe that fails like a tunnel whose box daemon
/// is stopped; later connects reach a working box.
struct StoppedThenRunning {
    box_side: devlanes_transport::memory::MemoryTransport,
    attempts: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl Transport for StoppedThenRunning {
    async fn connect(&self) -> Result<BoxDuplex, TransportError> {
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(Box::new(Stopped));
        }
        self.box_side.connect().await
    }
}

/// A tunnel that exited with the daemon-stopped status: every read fails.
struct Stopped;

impl AsyncRead for Stopped {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let error = exit_error("acme", "x", &exit(EXIT_DAEMON_STOPPED, ""));
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            error.expect("an error"),
        )))
    }
}

impl AsyncWrite for Stopped {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// A stopped box daemon keeps the link retrying with the hint as its error,
/// and the link connects once the box serves again.
#[tokio::test]
async fn a_stopped_box_daemon_retries_until_it_runs() -> Result<(), Box<dyn std::error::Error>> {
    use devlanes_transport::{
        Backoff, BoxServices, Link, LinkState, LoopbackDialer, NoSync, RegistrySnapshot,
        ServeOptions, SessionOptions, SharedRegistry,
    };

    tokio::time::timeout(Duration::from_secs(20), async {
        let (box_side, mut listener) = devlanes_transport::memory::memory_transport();
        let serving = tokio::spawn(async move {
            while let Some(stream) = listener.accept().await {
                let services = BoxServices {
                    registry: Arc::new(SharedRegistry::new(RegistrySnapshot::default(), 4)),
                    dialer: Arc::new(LoopbackDialer),
                    sync: Arc::new(NoSync),
                    pairing: Arc::new(devlanes_transport::NoPairing),
                };
                tokio::spawn(devlanes_transport::serve(
                    stream,
                    services,
                    ServeOptions::new("box"),
                ));
            }
        });
        let transport = Arc::new(StoppedThenRunning {
            box_side,
            attempts: std::sync::atomic::AtomicUsize::new(0),
        });
        let link = Link::spawn(
            transport,
            SessionOptions::new("client"),
            Backoff::new(Duration::from_millis(200), Duration::from_millis(200)),
        );
        let mut state = link.state();
        let mut retried = None;
        loop {
            match &*state.borrow_and_update() {
                LinkState::Retrying { error, .. } if retried.is_none() => {
                    retried = Some(error.clone());
                }
                LinkState::Connected(_) => break,
                LinkState::Failed { error } => panic!("the link gave up: {error}"),
                _ => {}
            }
            state.changed().await?;
        }
        let error = retried.ok_or("connected without retrying")?;
        assert!(
            error.contains("the devlanes daemon is not running on acme"),
            "{error}"
        );
        assert!(error.contains("`devlanes daemon start`"), "{error}");
        link.shutdown().await;
        serving.abort();
        Ok(())
    })
    .await?
}
