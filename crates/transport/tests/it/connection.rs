//! What the session and the box need from every connection type, checked
//! against the interface. Each check takes a [`Pair`], so another connection
//! type only adds a fixture function next to [`memory`].

use std::time::Duration;

use devlanes_transport::memory::memory_transport;
use devlanes_transport::{
    BoxConnection, BoxDuplex, BoxError, BoxServerConnection, ConnectionError, MAX_STREAMS,
    Transport,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{TestResult, bounded};

/// One client and box connection of a transport under test, and a way to
/// break the link under them.
struct Pair {
    client: BoxConnection,
    server: BoxServerConnection,
    sever: Box<dyn Fn() + Send + Sync>,
}

/// yamux over the memory transport's pipe.
async fn memory() -> Result<Pair, BoxError> {
    let (transport, mut listener) = memory_transport();
    let client = transport.connect().await?;
    let server = listener.accept().await.ok_or("no link")?;
    let link = transport.last_link().ok_or("no link control")?;
    Ok(Pair {
        client,
        server,
        sever: Box::new(move || link.sever()),
    })
}

/// Reads exactly `expected` from `stream`.
async fn expect(stream: &mut BoxDuplex, expected: &[u8]) -> TestResult {
    let mut read = vec![0; expected.len()];
    stream.read_exact(&mut read).await?;
    assert_eq!(read, expected);
    Ok(())
}

/// Reads `stream` to its end.
async fn rest(stream: &mut BoxDuplex) -> Result<Vec<u8>, BoxError> {
    let mut read = Vec::new();
    stream.read_to_end(&mut read).await?;
    Ok(read)
}

/// A payload larger than one mux frame, with a pattern a lost or reordered
/// byte breaks.
fn payload() -> Vec<u8> {
    (0..251u8).cycle().take(256 * 1024).collect()
}

/// Opens a stream and announces it with `hello`, as every protocol stream
/// starts, then accepts it on the box and reads the hello.
async fn opened(pair: &mut Pair, hello: &[u8]) -> Result<(BoxDuplex, BoxDuplex), BoxError> {
    let mut client = pair.client.open().await?;
    client.write_all(hello).await?;
    let mut boxed = pair.server.accept().await.ok_or("no stream")?;
    expect(&mut boxed, hello).await?;
    Ok((client, boxed))
}

/// Two streams carry bytes both ways, each on its own.
async fn streams_carry_bytes_both_ways(mut pair: Pair) -> TestResult {
    let (mut first, mut first_box) = opened(&mut pair, b"one").await?;
    first_box.write_all(b"1").await?;
    expect(&mut first, b"1").await?;
    let (mut second, mut second_box) = opened(&mut pair, b"two").await?;
    second_box.write_all(b"2").await?;
    expect(&mut second, b"2").await?;
    first.write_all(b"again").await?;
    expect(&mut first_box, b"again").await?;
    Ok(())
}

/// The side that shuts its write half down first still reads the other's
/// reply, whichever side that is: a request followed by its end, as a
/// mirrored connection forwards it.
async fn a_half_closed_stream_keeps_the_other_direction(mut pair: Pair) -> TestResult {
    for client_first in [true, false] {
        let (mut client, mut boxed) = opened(&mut pair, b"open").await?;
        let (first, second) = if client_first {
            (&mut client, &mut boxed)
        } else {
            (&mut boxed, &mut client)
        };
        first.write_all(b"request").await?;
        first.shutdown().await?;
        assert_eq!(
            rest(second).await?,
            b"request",
            "client first: {client_first}"
        );
        second.write_all(b"reply").await?;
        second.shutdown().await?;
        assert_eq!(rest(first).await?, b"reply", "client first: {client_first}");
    }
    Ok(())
}

/// A stream dropped without a shutdown ends cleanly for the reader after
/// every byte written before, whichever side drops it.
async fn a_dropped_stream_ends_cleanly_after_its_bytes(mut pair: Pair) -> TestResult {
    let payload = payload();
    for client_drops in [true, false] {
        let (client, boxed) = opened(&mut pair, b"open").await?;
        let (mut writer, mut reader) = if client_drops {
            (client, boxed)
        } else {
            (boxed, client)
        };
        let sent = payload.clone();
        let writing = tokio::spawn(async move {
            writer.write_all(&sent).await?;
            drop(writer);
            Ok::<_, std::io::Error>(())
        });
        let read = rest(&mut reader).await?;
        writing.await??;
        assert!(
            read == payload,
            "client drops: {client_drops}: {} bytes",
            read.len()
        );
    }
    Ok(())
}

/// [`MAX_STREAMS`] streams stay open at once without an open waiting on the
/// box to close one.
async fn max_streams_stay_open_at_once(pair: Pair) -> TestResult {
    let mut server = pair.server;
    // Like the real box, answer every stream: yamux stops opening streams
    // while too many wait for their first reply.
    let answering = tokio::spawn(async move {
        let mut held = Vec::new();
        while held.len() < MAX_STREAMS {
            let mut stream = server.accept().await.ok_or("the connection closed")?;
            expect(&mut stream, b"o").await?;
            stream.write_all(b"k").await?;
            held.push(stream);
        }
        Ok::<_, BoxError>((server, held))
    });
    let mut open = Vec::new();
    for _ in 0..MAX_STREAMS {
        let mut stream = pair.client.open().await?;
        stream.write_all(b"o").await?;
        open.push(stream);
    }
    for stream in &mut open {
        expect(stream, b"k").await?;
    }
    let (_server, held) = answering.await??;
    assert_eq!(held.len(), MAX_STREAMS);
    Ok(())
}

/// A message the box wrote and shut down before it dropped the connection
/// reaches the client, as a refusal does, and a clean close has no failure
/// reason.
async fn finished_bytes_survive_the_box_dropping_the_connection(mut pair: Pair) -> TestResult {
    let (mut client, mut boxed) = opened(&mut pair, b"hello").await?;
    let message = br#"{"type":"refused","message":"the box does not accept connections"}"#;
    boxed.write_all(message).await?;
    boxed.shutdown().await?;
    drop(boxed);
    drop(pair.server);
    assert_eq!(rest(&mut client).await?, message);
    pair.client.closed().await;
    let failure = pair.client.failure_within(Duration::from_secs(1)).await;
    assert_eq!(failure, None);
    Ok(())
}

/// After the client shuts the connection down, it opens nothing and the box
/// accepts nothing.
async fn a_client_shutdown_ends_the_connection(mut pair: Pair) -> TestResult {
    pair.client.shutdown();
    pair.client.closed().await;
    assert!(
        matches!(pair.client.open().await, Err(ConnectionError::Closed)),
        "a stream opened on a closed connection"
    );
    assert!(
        pair.server.accept().await.is_none(),
        "the box accepted a stream"
    );
    Ok(())
}

/// A link that breaks under the connection closes both sides, and the
/// client has a reason to show.
async fn a_broken_link_reports_why(mut pair: Pair) -> TestResult {
    (pair.sever)();
    pair.client.closed().await;
    let failure = pair.client.failure_within(Duration::from_secs(1)).await;
    assert!(failure.is_some(), "no reason for a broken link");
    assert!(
        pair.server.accept().await.is_none(),
        "the box accepted a stream"
    );
    Ok(())
}

#[tokio::test]
async fn memory_streams_carry_bytes_both_ways() -> TestResult {
    bounded(async { streams_carry_bytes_both_ways(memory().await?).await }).await
}

#[tokio::test]
async fn memory_a_half_closed_stream_keeps_the_other_direction() -> TestResult {
    bounded(async { a_half_closed_stream_keeps_the_other_direction(memory().await?).await }).await
}

#[tokio::test]
async fn memory_a_dropped_stream_ends_cleanly_after_its_bytes() -> TestResult {
    bounded(async { a_dropped_stream_ends_cleanly_after_its_bytes(memory().await?).await }).await
}

#[tokio::test]
async fn memory_max_streams_stay_open_at_once() -> TestResult {
    tokio::time::timeout(Duration::from_secs(20), async {
        max_streams_stay_open_at_once(memory().await?).await
    })
    .await?
}

#[tokio::test]
async fn memory_finished_bytes_survive_the_box_dropping_the_connection() -> TestResult {
    bounded(async { finished_bytes_survive_the_box_dropping_the_connection(memory().await?).await })
        .await
}

#[tokio::test]
async fn memory_a_client_shutdown_ends_the_connection() -> TestResult {
    bounded(async { a_client_shutdown_ends_the_connection(memory().await?).await }).await
}

#[tokio::test]
async fn memory_a_broken_link_reports_why() -> TestResult {
    bounded(async { a_broken_link_reports_why(memory().await?).await }).await
}
