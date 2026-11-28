# devlanes-gui-client

A headless async client of the devlanes GUI protocol: it connects to a GUI host, says hello with a role, sends input and requests, and turns what the host sends into events with decoded frames. The conformance checks of `devlanes-gui-host` drive hosts with it; it uses `devlanes-gui-proto` for the messages and `devlanes-gui-codec` for decoding, and draws nothing itself.

## Usage

```rust
use devlanes_gui_client::{ClientError, ClientOptions, Event, connect};
use devlanes_gui_proto::{Buttons, Point, Role};

async fn click_then_type(port: u16) -> Result<(), ClientError> {
    let options = ClientOptions::new(Role::Tool, "acme-laptop:4242", "ada");
    let (client, _events) = connect(("127.0.0.1", port), options).await?;
    let window = client.welcome().windows.first().ok_or(ClientError::NoSuchWindow)?.id;
    client.click(window, Point::new(40, 20), Buttons::LEFT)?;
    // Waits until the click was applied and the app drew after it.
    client.sync().await?;
    client.text(window, "hello")?;
    let shot = client.screenshot(Some(window)).await?;
    println!("{}x{}", shot.frame.width(), shot.frame.height());
    Ok(())
}

async fn watch(port: u16) -> Result<(), ClientError> {
    let options = ClientOptions::new(Role::Viewer, "acme-laptop:4243", "ada");
    let (_client, mut events) = connect(("127.0.0.1", port), options).await?;
    while let Some(event) = events.next().await {
        match event {
            // Acknowledged as it is returned, so a slow caller slows the stream.
            Event::Frame(frame) => println!("frame {} of window {:?}", frame.seq, frame.window),
            Event::Lock { window, state } => println!("{window:?} is {state:?}"),
            Event::Closed(_) => break,
            _ => {}
        }
    }
    Ok(())
}
```

## API

- `connect`, `connect_stream`, `ClientOptions`: TCP or any stream, with a role, decoders and a control token.
- `Client`: typed input (`pointer`, `click`, `key`, `text`, `wheel`, `resize`, `focus`, `clipboard`), `screenshot`, `sync`, and the control messages.
- `Events`, `Event`: windows, decoded frames, lock states, the input holder, cursor, clipboard, activity, and the end of the connection.
- `ClientError`: typed failures, including refusals and `DecodeError::VersionMismatch`.
- `instance`: the lock that keeps one viewer process per GUI service on a machine (`claim`, `running`, `holder`, `ask_to_show`), shared by the viewer, `devlanes gui open` and the daemon's auto-open.

## Behaviour

Each window decodes on the blocking pool in order, and events keep the order the host sent them in. A frame that fails to decode is acknowledged as discarded and followed by a keyframe request. While the client holds a button or key it sends a keep-alive every heartbeat interval, so the host does not release them. Lock states combine the host's state with the client's own resizes and its link, as `ViewerLock` does, for one connection only. At most `MAX_SCREENSHOTS_IN_FLIGHT` screenshots are asked for at once; further calls wait. Once the connection closes, every call returns `ClientError::Closed`.

## License

Apache-2.0
