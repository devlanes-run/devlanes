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

// On the host: append what the socket delivered, then take whole messages.
// The first one must be the hello; the role decides what may follow.
fn on_bytes(
    buf: &mut BytesMut,
    read: &[u8],
    role: &mut Option<Role>,
) -> Result<Vec<ClientMessage>, DecodeError> {
    buf.extend_from_slice(read);
    let mut messages = Vec::new();
    if role.is_none() {
        match ClientMessage::decode_hello(buf)? {
            Some(hello) => *role = Some(hello.role),
            None => return Ok(messages),
        }
    }
    while let Some(message) = ClientMessage::decode(buf)? {
        if role.is_some_and(|role| message.allowed_for(role)) {
            messages.push(message);
        }
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

## Wire format

Each message is a type byte, a big-endian `u32` body length and the body. Every type has a body limit, checked from the 5-byte header before the body is buffered, and every string and list has a limit of its own. Frames of unknown types are skipped, readers ignore bytes after the fields they know, and unknown enumeration values fall back to a generic one, so a version can add messages and fields without breaking older peers. The crate docs list the rules and what never changes.

## License

Apache-2.0
