# devlanes-gui-host

The GUI host core: the `Backend` trait an OS backend implements, and `serve`, which serves a backend's windows to viewers, tools and the daemon over the devlanes GUI protocol. The only backend so far is the test pattern in this crate; it builds on `devlanes-gui-proto` for the messages and state machines and `devlanes-gui-codec` for encoding, and its tests drive it with `devlanes-gui-client`.

## Usage

```rust
use devlanes_gui_host::{ServeOptions, TestPattern, TestPatternOptions, serve, start_backend};
use devlanes_gui_proto::Token;
use tokio::net::TcpListener;

async fn host_test_pattern(port: u16, token: String) -> Result<(), Box<dyn std::error::Error>> {
    let (backend, _pattern) = TestPattern::new(TestPatternOptions::default());
    let backend = start_backend(backend).await?;
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    // The display is up and the port listens: report what the unit needs.
    for (name, value) in backend.env() {
        println!("{name}={value:?}");
    }
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

use devlanes_gui_host::{Backend, BackendControl, BackendError, Command, EventSink, Started};

struct Acme;

struct AcmeControl(mpsc::Sender<Command>);

impl BackendControl for AcmeControl {
    fn send(&mut self, command: Command) {
        // Queue it for the backend's thread; never block here.
        let _ = self.0.send(command);
    }
}

impl Backend for Acme {
    fn start(self: Box<Self>, events: EventSink) -> Result<Started, BackendError> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for command in rx {
                // Apply the command, then report windows, frames and resizes.
                let _ = (&events, command);
            }
        });
        Ok(Started {
            control: Box::new(AcmeControl(tx)),
            env: vec![("WAYLAND_DISPLAY".into(), Some("devlanes-acme".into()))],
            real_desktop: false,
        })
    }
}
```

## API

- `Backend`, `Started`, `BackendControl`, `EventSink`: how a backend starts, what it hands back (its control, the unit's variables, the real-desktop flag), takes commands and reports.
- `Event`, `Command`: what a backend reports (windows, frames with damage, cursor, moves, clipboard, applied resizes) and what it is asked to do (input, resize, close, focus, clipboard, a frame, whether a window is watched, the unit's identity, exit, freeze, thaw).
- `start_backend`, `RunningBackend`, `serve`, `ServeOptions`, `Listener`: start the backend, then serve it over a `TcpListener` or a channel of accepted streams.
- `TestPattern`, `TestPatternHandle`: a backend that draws a colour grid and a counter and records what it received.
- `pattern`: the grid, for test apps and checks.
- `conformance` (feature `test-support`): the checks every backend runs against itself.

## Threads and timing

`Backend::start` runs on the blocking pool and `BackendControl::send` must return at once, so a compositor's event loop or a capture thread stays on its own thread. Encoding and screenshots run on the blocking pool, one frame at a time per window and codec. Everything else runs on one task, driven by the clock of `tokio::time`, so tests on a paused clock are deterministic. The test pattern draws inside `send`, which is why its windows are at most 2048 pixels on a side.

## Limits

At most 64 connections at once; a connection that does not read is closed past its write queue cap, and one with more than 4 screenshots or 16 syncs unanswered is closed too.

## License

Apache-2.0
