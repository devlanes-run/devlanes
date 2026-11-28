# devlanes-gui-proto

The binary protocol between a GUI host and its viewers, tools and the daemon, plus the pure state machines both ends run on it: frame pacing, input arbitration and window lock states. The GUI host, the headless client and the viewer build on it; it has no I/O of its own, so callers read bytes from their socket into a buffer and take messages out of it.

## Usage

```rust
use bytes::BytesMut;
use devlanes_gui_proto::{ClientMessage, DecodeError, EncodeError, Hello, Role};

fn send_hello(out: &mut BytesMut) -> Result<(), EncodeError> {
    ClientMessage::Hello(Hello {
        role: Role::Tool,
        client_id: "acme-laptop:4242".into(),
        client_name: "ada".into(),
        decoders: Vec::new(),
        features: Vec::new(),
        token: None,
    })
    .encode(out)
}

// Why the host ends a connection.
#[derive(Debug)]
enum Close {
    Decode(DecodeError),
    // Answered with a refusal (RoleNotSupported) before closing.
    Role(Role),
    NotAllowed,
}

// On the host: append what the socket delivered, then take whole messages.
// The first one must be the hello; the role decides what may follow, and the
// first message it may not send ends the connection.
fn on_bytes(
    buf: &mut BytesMut,
    read: &[u8],
    role: &mut Option<Role>,
) -> Result<Vec<ClientMessage>, Close> {
    buf.extend_from_slice(read);
    let mut messages = Vec::new();
    let role = match *role {
        Some(role) => role,
        None => match ClientMessage::decode_hello(buf).map_err(Close::Decode)? {
            Some(hello) => match hello.role {
                // A real host also compares `hello.token` with its own for
                // `Control` and refuses with `BadToken` when they differ.
                Role::Viewer | Role::Tool | Role::Control => *role.insert(hello.role),
                other => return Err(Close::Role(other)),
            },
            None => return Ok(messages),
        },
    };
    while let Some(message) = ClientMessage::decode(buf).map_err(Close::Decode)? {
        if !message.allowed_for(role) {
            return Err(Close::NotAllowed);
        }
        messages.push(message);
    }
    Ok(messages)
}
```

## API

- `ClientMessage` and `HostMessage`: every message of each direction, with `encode` and `decode`; `ClientMessage::allowed_for` says which role may send it.
- `Hello`, `Welcome`, `Refusal`: the handshake, read with `decode_hello` and `decode_answer`; a version mismatch decodes as `DecodeError::VersionMismatch` on both ends.
- `WindowInfo`, `CodecOffer`, `CursorShape`, `LockState`: what the host describes.
- `Pacer`: per window and codec, decides when to encode, which viewers get the frame and at what bitrate, frame rate and quality.
- `InputArbiter`: per display, decides whose input reaches the app and releases what a silent client holds.
- `HostLock` and `ViewerLock`: per window, the lock state from its causes on each end (resizing, waiting for the app, reconnecting, frozen, closed).
- `ready::Ready` (feature `serde`, which brings serde and serde_json): the JSON line a GUI host prints once it listens, with `Ready::new`, `to_line` and the strict reader `Ready::parse`; read at most `MAX_READY_LINE` bytes of it.

## Wire format

Each message is a type byte, a big-endian `u32` body length and the body. Every type has a body limit, checked from the 5-byte header before the body is buffered, and every string and list has a limit of its own. Frames of unknown types are skipped, readers ignore bytes after the fields they know, and unknown enumeration values fall back to a generic one, so a version can add messages and fields without breaking older peers. The crate docs list the rules and what never changes.

## License

Apache-2.0
