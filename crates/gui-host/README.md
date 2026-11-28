# devlanes-gui-host

The GUI host core: the `Backend` trait each OS backend implements, and `serve`, which serves a backend's windows to viewers, tools and the daemon over the devlanes GUI protocol. The `devlanes-gui host` binary runs it with a Linux, Windows or test-pattern backend; it builds on `devlanes-gui-proto` for the messages and state machines and `devlanes-gui-codec` for encoding.

## Usage

```rust
use devlanes_gui_host::{ServeOptions, TestPattern, TestPatternOptions, serve};
use devlanes_gui_proto::Token;
use tokio::net::TcpListener;

async fn host_test_pattern(port: u16, token: String) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let (backend, _pattern) = TestPattern::new(TestPatternOptions::default());
    let mut options = ServeOptions::new("acme:desktop");
    options.control_token = Some(Token::new(token));
    // Runs until `options.shutdown` is cancelled or the backend stops.
    serve(listener, backend, options).await?;
    Ok(())
}
```

A backend runs on threads of its own and never blocks the host:

```rust
use std::sync::mpsc;

use devlanes_gui_host::{Backend, BackendControl, BackendError, Command, EventSink};

struct Acme;

struct AcmeControl(mpsc::Sender<Command>);

impl BackendControl for AcmeControl {
    fn send(&mut self, command: Command) {
        // Queue it for the backend's thread; never block here.
        let _ = self.0.send(command);
    }
}

impl Backend for Acme {
    fn real_desktop(&self) -> bool {
        false
    }

    fn start(self: Box<Self>, events: EventSink) -> Result<Box<dyn BackendControl>, BackendError> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for command in rx {
                // Apply the command, then report windows, frames and resizes.
                let _ = (&events, command);
            }
        });
        Ok(Box::new(AcmeControl(tx)))
    }
}
```

## API

- `Backend`, `BackendControl`, `EventSink`: how a backend starts, takes commands and reports.
- `Event`, `Command`: what a backend reports (windows, frames with damage, cursor, moves, clipboard, applied resizes) and what it is asked to do (input, resize, close, focus, clipboard, a frame, the unit's identity, exit, freeze, thaw).
- `serve`, `ServeOptions`, `Listener`: the server over a `TcpListener` or a channel of accepted streams.
- `TestPattern`, `TestPatternHandle`: a backend that draws a colour grid and a counter and records what it received.
- `pattern`: the grid, for test apps and checks.
- `conformance` (feature `test-support`): the checks every backend runs against itself.

## Threads and timing

`Backend::start` runs on the blocking pool and `BackendControl::send` must return at once, so a compositor's event loop or a capture thread stays on its own thread. Encoding and screenshots run on the blocking pool, one frame at a time per window and codec. Everything else runs on one task, driven by the clock of `tokio::time`, so tests on a paused clock are deterministic.

## License

Apache-2.0
