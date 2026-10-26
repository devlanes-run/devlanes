//! Box-to-client sync subscriptions over dedicated mux streams.
//!
//! The transport frames messages and, with [`crate::SYNC_ZSTD`], compresses
//! chunks and client input on the way. Manifest contents, chunk ordering and
//! path validation belong to the sync crate on each side.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::{BoxStream, SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::mux::MuxStream;
use crate::wire::{
    self, Channel, MAX_FRAME_LEN, PayloadEncoding, SyncClientFrame, SyncFrame, WireError,
};

/// zstd's own default level; higher levels cost a box serving several clients
/// much more CPU for a few percent less data.
const ZSTD_LEVEL: i32 = 3;

/// Data shorter than this goes raw without trying: compressing it saves at most
/// a couple of hundred bytes, little next to the header each frame carries
/// anyway, and a tree of small files would pay a blocking-pool trip per file.
const MIN_COMPRESSED: usize = 256;

/// The write half of a sync stream.
type SyncSink = SplitSink<Channel<MuxStream>, wire::Frame>;

/// One sync message from the box.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncMessage {
    /// An opaque manifest, sent first and again whenever the sync source wants.
    Manifest(Value),
    /// File bytes at `offset`; `last` marks the end of that file.
    ///
    /// `path` is untrusted on the client: validate it before writing.
    Chunk {
        /// Relative path chosen by the box.
        path: String,
        /// Byte offset of `data` within the file.
        offset: u64,
        /// File bytes.
        data: Bytes,
        /// Whether this chunk completes the file.
        last: bool,
    },
    /// A file or directory deleted on the box.
    Delete {
        /// Relative path chosen by the box.
        path: String,
    },
}

/// A sync source failure, forwarded verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct SyncError {
    /// Human-readable description.
    pub message: String,
}

impl SyncError {
    /// Builds an error from a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Messages for one subscription; ending the stream ends the subscription.
pub type SyncFeed = BoxStream<'static, Result<SyncMessage, SyncError>>;

/// Box-side supplier of sync feeds, implemented by the sync crate.
#[async_trait]
pub trait SyncSource: Send + Sync {
    /// Starts a feed for an opaque subscription request. `input` carries what the
    /// client sends after the request, such as its manifest in batches.
    async fn subscribe(&self, request: Value, input: SyncInput) -> Result<SyncFeed, SyncError>;
}

/// One item of client input on a sync stream.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncInputEvent {
    /// An opaque value from the client.
    Value(Value),
    /// The client said no more input follows; the subscription goes on.
    End,
}

/// Client input of one subscription.
///
/// Values are queued with backpressure: a client sending faster than the source
/// reads waits. Dropping the input discards later values.
pub struct SyncInput {
    receiver: mpsc::Receiver<SyncInputEvent>,
}

impl SyncInput {
    /// Wraps a queue; for sources tested without a transport.
    pub fn from_receiver(receiver: mpsc::Receiver<SyncInputEvent>) -> Self {
        Self { receiver }
    }

    /// The next input. `None` after [`SyncInputEvent::End`], or when the client
    /// left without ending its input.
    pub async fn recv(&mut self) -> Option<SyncInputEvent> {
        self.receiver.recv().await
    }
}

/// Client input values queued per subscription before the client waits.
const INPUT_QUEUE: usize = 4;

/// A [`SyncSource`] that refuses every subscription, for boxes without sync.
pub struct NoSync;

#[async_trait]
impl SyncSource for NoSync {
    async fn subscribe(&self, _request: Value, _input: SyncInput) -> Result<SyncFeed, SyncError> {
        Err(SyncError::new("sync is not available on this box"))
    }
}

/// Serves one sync stream until the feed ends or the client goes away;
/// `compress` when the client negotiated [`crate::SYNC_ZSTD`].
pub(crate) async fn serve_subscription(
    channel: Channel<MuxStream>,
    request: Value,
    source: Arc<dyn SyncSource>,
    compress: bool,
) {
    let (mut sink, mut client) = channel.split();
    let (input_tx, receiver) = mpsc::channel(INPUT_QUEUE);
    let gone = CancellationToken::new();
    let reader_gone = gone.clone();
    // Why the reader gave up on a client that is still there, for its last frame.
    let (failed_tx, mut failed) = oneshot::channel();
    let reader = tokio::spawn(async move {
        let mut input_tx = Some(input_tx);
        loop {
            match wire::recv::<_, SyncClientFrame>(&mut client).await {
                Ok(Some((SyncClientFrame::Input { value, encoding }, payload))) => {
                    let value = match input_value(value, encoding, payload).await {
                        Ok(value) => value,
                        Err(error) => {
                            let _ = failed_tx.send(error);
                            reader_gone.cancel();
                            return;
                        }
                    };
                    // Waiting is the backpressure for manifest batches. A source that
                    // dropped its input makes the send fail and reading resumes, so
                    // the client leaving is still noticed.
                    if let Some(tx) = &input_tx
                        && tx.send(SyncInputEvent::Value(value)).await.is_err()
                    {
                        input_tx = None;
                    }
                }
                Ok(Some((SyncClientFrame::EndInput, _))) => {
                    if let Some(tx) = input_tx.take() {
                        let _ = tx.send(SyncInputEvent::End).await;
                    }
                }
                // EOF, a reset or a malformed frame: the client is gone.
                Ok(None) | Err(_) => {
                    reader_gone.cancel();
                    return;
                }
            }
        }
    });
    let _reader = AbortOnDrop(reader);
    let input = SyncInput { receiver };
    let feed = tokio::select! {
        feed = source.subscribe(request, input) => feed,
        () = gone.cancelled() => return abandon(&mut sink, &mut failed).await,
    };
    let mut feed = match feed {
        Ok(feed) => feed,
        Err(error) => {
            let _ = wire::send(&mut sink, &SyncFrame::Error { error }).await;
            let _ = sink.close().await;
            return;
        }
    };
    let mut chunks = ChunkEncoder {
        enabled: compress,
        raw_file: None,
    };
    loop {
        let item = tokio::select! {
            // The reader cancels before it drops the input, so a source that
            // fails because its input closed never hides the reader's reason.
            biased;
            () = gone.cancelled() => return abandon(&mut sink, &mut failed).await,
            item = feed.next() => item,
        };
        let sent = match item {
            Some(Ok(SyncMessage::Manifest(manifest))) => {
                wire::send(&mut sink, &SyncFrame::Manifest { manifest }).await
            }
            Some(Ok(SyncMessage::Chunk {
                path,
                offset,
                data,
                last,
            })) => {
                let (encoding, payload) = chunks.encode(&path, offset, data).await;
                let header = SyncFrame::Chunk {
                    path,
                    offset,
                    last,
                    encoding,
                };
                wire::send_with_payload(&mut sink, &header, payload).await
            }
            Some(Ok(SyncMessage::Delete { path })) => {
                wire::send(&mut sink, &SyncFrame::Delete { path }).await
            }
            Some(Err(error)) => {
                let _ = wire::send(&mut sink, &SyncFrame::Error { error }).await;
                break;
            }
            None => {
                let _ = wire::send(&mut sink, &SyncFrame::End).await;
                break;
            }
        };
        if sent.is_err() {
            return;
        }
    }
    let _ = sink.close().await;
}

/// Tells a client whose input was invalid why its subscription ends; a client
/// that left hears nothing.
async fn abandon(sink: &mut SyncSink, failed: &mut oneshot::Receiver<SyncError>) {
    if let Ok(error) = failed.try_recv() {
        let _ = wire::send(sink, &SyncFrame::Error { error }).await;
        let _ = sink.close().await;
    }
}

/// Picks each chunk's encoding for one subscription.
struct ChunkEncoder {
    /// Whether the client negotiated [`crate::SYNC_ZSTD`].
    enabled: bool,
    /// A file whose chunk did not get smaller. Its later chunks go raw without
    /// trying: archives, images and APKs don't turn compressible halfway, and
    /// trying every chunk would only spend the box's CPU.
    raw_file: Option<String>,
}

impl ChunkEncoder {
    /// The chunk of `path` at `offset` as it goes on the wire.
    async fn encode(&mut self, path: &str, offset: u64, data: Bytes) -> (PayloadEncoding, Bytes) {
        // A file sent again starts over.
        if offset == 0 {
            self.raw_file = None;
        }
        if !self.enabled || data.len() < MIN_COMPRESSED || self.raw_file.as_deref() == Some(path) {
            return (PayloadEncoding::Raw, data);
        }
        match deflate(data.clone()).await {
            Some(compressed) => (
                PayloadEncoding::Zstd {
                    size: data.len() as u64,
                },
                compressed,
            ),
            None => {
                self.raw_file = Some(path.to_owned());
                (PayloadEncoding::Raw, data)
            }
        }
    }
}

/// `data` as zstd when that is smaller; `None` when it isn't or the compressor
/// failed, so the raw bytes go instead. Compression runs on the blocking pool: a
/// 1 MiB chunk takes milliseconds of CPU.
async fn deflate(data: Bytes) -> Option<Bytes> {
    let input = data.clone();
    let compressed =
        tokio::task::spawn_blocking(move || zstd::bulk::compress(&input, ZSTD_LEVEL)).await;
    match compressed {
        Ok(Ok(compressed)) if compressed.len() < data.len() => Some(Bytes::from(compressed)),
        _ => None,
    }
}

/// A client input value, decompressed and parsed on the blocking pool when it
/// came compressed.
async fn input_value(
    value: Value,
    encoding: PayloadEncoding,
    payload: Bytes,
) -> Result<Value, SyncError> {
    let PayloadEncoding::Zstd { size } = encoding else {
        return Ok(value);
    };
    let decoded = tokio::task::spawn_blocking(move || {
        let json = inflate(&payload, size)?;
        serde_json::from_slice(&json).map_err(|error| format!("is not JSON: {error}"))
    })
    .await;
    match decoded {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(reason)) => Err(SyncError::new(format!("compressed sync input {reason}"))),
        Err(error) => Err(SyncError::new(format!(
            "compressed sync input could not be decompressed: {error}"
        ))),
    }
}

/// Decompresses data declared to be `size` bytes long. Allocates at most `size`,
/// itself bounded by the frame limit, so a peer cannot make this end hold more
/// than a raw frame would; data of any other length is an error.
fn inflate(payload: &[u8], size: u64) -> Result<Bytes, String> {
    let size = usize::try_from(size)
        .ok()
        .filter(|size| *size <= MAX_FRAME_LEN)
        .ok_or_else(|| format!("declares {size} bytes, more than a frame holds"))?;
    let data = zstd::bulk::decompress(payload, size)
        .map_err(|error| format!("does not decompress to its declared {size} bytes: {error}"))?;
    if data.len() != size {
        return Err(format!(
            "decompresses to {} bytes instead of its declared {size}",
            data.len()
        ));
    }
    Ok(Bytes::from(data))
}

/// An input frame carrying `value` in its header.
fn raw_input(value: Value) -> (SyncClientFrame, Bytes) {
    let frame = SyncClientFrame::Input {
        value,
        encoding: PayloadEncoding::Raw,
    };
    (frame, Bytes::new())
}

/// An input frame carrying `value`'s JSON as zstd in its payload when that is
/// smaller. A manifest batch is hundreds of kilobytes of JSON, so it is
/// serialized and compressed on the blocking pool.
async fn input_frame(value: Value) -> Result<(SyncClientFrame, Bytes), WireError> {
    let encoded = tokio::task::spawn_blocking(move || {
        let json = serde_json::to_vec(&value)?;
        if json.len() >= MIN_COMPRESSED
            && let Ok(compressed) = zstd::bulk::compress(&json, ZSTD_LEVEL)
            && compressed.len() < json.len()
        {
            let frame = SyncClientFrame::Input {
                value: Value::Null,
                encoding: PayloadEncoding::Zstd {
                    size: json.len() as u64,
                },
            };
            return Ok((frame, Bytes::from(compressed)));
        }
        Ok(raw_input(value))
    })
    .await;
    encoded.unwrap_or_else(|error| Err(WireError::Io(std::io::Error::other(error))))
}

/// Aborts the client reader when the subscription ends.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    /// Stops reading the client.
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Failure of a subscription as seen by the client.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SyncStreamError {
    /// The box's sync source failed.
    #[error("remote sync error: {0}")]
    Remote(#[from] SyncError),
    /// The stream broke or carried invalid frames.
    #[error(transparent)]
    Wire(#[from] WireError),
    /// The stream ended without an end marker, usually because the session died.
    #[error("sync stream interrupted")]
    Interrupted,
    /// A compressed chunk did not decompress to the size it declared.
    #[error("compressed chunk of {path} {reason}")]
    BadChunk {
        /// Relative path chosen by the box.
        path: String,
        /// What was wrong with it.
        reason: String,
    },
}

/// Client side of a subscription. Dropping it cancels the feed on the box.
pub struct SyncSubscription {
    // Closing the write side would read as cancellation, so it stays open.
    sink: SyncSink,
    stream: SplitStream<Channel<MuxStream>>,
    /// Whether input values may go compressed, as negotiated with the box.
    compress: bool,
    /// The path of a compressed chunk being decompressed off the executor, and
    /// that task; kept across cancelled [`Self::next`] calls so the chunk is not
    /// lost.
    inflating: Option<(String, JoinHandle<Result<SyncMessage, SyncStreamError>>)>,
    done: bool,
    input_ended: bool,
}

impl SyncSubscription {
    /// Wraps a stream whose subscription frame was already sent; `compress`
    /// when the session negotiated [`crate::SYNC_ZSTD`].
    pub(crate) fn new(channel: Channel<MuxStream>, compress: bool) -> Self {
        let (sink, stream) = channel.split();
        Self {
            sink,
            stream,
            compress,
            inflating: None,
            done: false,
            input_ended: false,
        }
    }

    /// Sends one input value to the box's sync source, its JSON compressed
    /// when that is smaller; waits while the box is behind.
    pub async fn send(&mut self, value: Value) -> Result<(), SyncStreamError> {
        let (frame, payload) = if self.compress {
            input_frame(value).await?
        } else {
            raw_input(value)
        };
        wire::send_with_payload(&mut self.sink, &frame, payload).await?;
        Ok(())
    }

    /// Tells the box no more input follows; messages keep flowing.
    pub async fn finish_input(&mut self) -> Result<(), SyncStreamError> {
        if !self.input_ended {
            wire::send(&mut self.sink, &SyncClientFrame::EndInput).await?;
            self.input_ended = true;
        }
        Ok(())
    }

    /// Next message, chunks decompressed; `None` once the box ended the feed
    /// normally. Every error ends the subscription.
    ///
    /// Cancel-safe: a chunk still decompressing when the call is dropped is
    /// returned by the next call.
    pub async fn next(&mut self) -> Option<Result<SyncMessage, SyncStreamError>> {
        if self.done {
            return None;
        }
        let result = self.receive().await;
        if !matches!(result, Some(Ok(_))) {
            self.done = true;
        }
        result
    }

    /// Reads the next frame, or finishes the pending decompression first.
    async fn receive(&mut self) -> Option<Result<SyncMessage, SyncStreamError>> {
        let (path, task) = match &mut self.inflating {
            Some(inflating) => inflating,
            slot @ None => {
                let (frame, payload) = match wire::recv::<_, SyncFrame>(&mut self.stream).await {
                    Ok(Some(received)) => received,
                    Ok(None) => return Some(Err(SyncStreamError::Interrupted)),
                    Err(error) => return Some(Err(error.into())),
                };
                match frame {
                    SyncFrame::Manifest { manifest } => {
                        return Some(Ok(SyncMessage::Manifest(manifest)));
                    }
                    SyncFrame::Chunk {
                        path,
                        offset,
                        last,
                        encoding: PayloadEncoding::Raw,
                    } => {
                        return Some(Ok(SyncMessage::Chunk {
                            path,
                            offset,
                            data: payload,
                            last,
                        }));
                    }
                    SyncFrame::Chunk {
                        path,
                        offset,
                        last,
                        encoding: PayloadEncoding::Zstd { size },
                    } => {
                        let name = path.clone();
                        let task =
                            tokio::task::spawn_blocking(move || match inflate(&payload, size) {
                                Ok(data) => Ok(SyncMessage::Chunk {
                                    path,
                                    offset,
                                    data,
                                    last,
                                }),
                                Err(reason) => Err(SyncStreamError::BadChunk { path, reason }),
                            });
                        slot.insert((name, task))
                    }
                    SyncFrame::Delete { path } => return Some(Ok(SyncMessage::Delete { path })),
                    SyncFrame::Error { error } => return Some(Err(error.into())),
                    SyncFrame::End => return None,
                }
            }
        };
        let inflated = task.await.unwrap_or_else(|error| {
            Err(SyncStreamError::BadChunk {
                path: path.clone(),
                reason: format!("could not be decompressed: {error}"),
            })
        });
        self.inflating = None;
        Some(inflated)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::FutureExt;

    use super::*;
    use crate::mux;
    use crate::registry::{RegistrySnapshot, SharedRegistry};
    use crate::server::{BoxServices, LoopbackDialer, ServeOptions, serve};
    use crate::session::{Session, SessionOptions};
    use crate::wire::{BoxControl, BoxInfo, FEATURES, OpenRequest, PROTOCOL_VERSION, SYNC_ZSTD};

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

    /// Fails the test instead of hanging when a future never completes.
    async fn bounded<T>(future: impl Future<Output = TestResult<T>>) -> TestResult<T> {
        tokio::time::timeout(Duration::from_secs(10), future).await?
    }

    /// Bytes zstd cannot shrink, the same on every run (xorshift).
    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 32) as u8
            })
            .collect()
    }

    /// Source text, which zstd shrinks many times over.
    fn text(len: usize) -> Vec<u8> {
        b"console.log(\"hello from the box\");\n"
            .iter()
            .copied()
            .cycle()
            .take(len)
            .collect()
    }

    /// Names as owned strings.
    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    /// Files as paths with their chunks, in the order the box sends them.
    type Files = Vec<(&'static str, Vec<Bytes>)>;

    /// A file whose chunks compress, don't, and would compress again.
    fn mixed() -> Vec<Bytes> {
        vec![
            Bytes::from(text(64 * 1024)),
            Bytes::from(noise(64 * 1024)),
            Bytes::from(text(64 * 1024)),
        ]
    }

    /// A sync source that sends `files` for every request except
    /// `{"echo": true}`, which collects the client's input up to its end marker
    /// and sends it back as one manifest.
    struct TestSource {
        files: Files,
    }

    #[async_trait]
    impl SyncSource for TestSource {
        /// Echoes input or sends the files.
        async fn subscribe(
            &self,
            request: Value,
            mut input: SyncInput,
        ) -> Result<SyncFeed, SyncError> {
            if request["echo"] == true {
                let echo = async move {
                    let mut values = Vec::new();
                    loop {
                        match input.recv().await {
                            Some(SyncInputEvent::Value(value)) => values.push(value),
                            Some(SyncInputEvent::End) => break,
                            None => return Err(SyncError::new("client left without ending input")),
                        }
                    }
                    Ok(SyncMessage::Manifest(Value::Array(values)))
                };
                return Ok(futures_util::stream::once(echo).boxed());
            }
            let mut messages = Vec::new();
            for (path, chunks) in &self.files {
                let count = chunks.len();
                let mut offset = 0;
                for (index, data) in chunks.iter().enumerate() {
                    messages.push(Ok(SyncMessage::Chunk {
                        path: (*path).into(),
                        offset,
                        data: data.clone(),
                        last: index + 1 == count,
                    }));
                    offset += data.len() as u64;
                }
            }
            Ok(futures_util::stream::iter(messages).boxed())
        }
    }

    /// A box listing `box_features` that serves `files` on `io`.
    fn serve_box(
        io: tokio::io::DuplexStream,
        box_features: &[&str],
        files: Files,
    ) -> JoinHandle<()> {
        let services = BoxServices {
            registry: Arc::new(SharedRegistry::new(RegistrySnapshot::default(), 4)),
            dialer: Arc::new(LoopbackDialer),
            sync: Arc::new(TestSource { files }),
            pairing: Arc::new(crate::NoPairing),
        };
        let mut options = ServeOptions::new("box");
        options.features = names(box_features);
        tokio::spawn(async move {
            let _ = serve(io, services, options).await;
        })
    }

    /// The path, encoding and payload length of every chunk a client listing
    /// `client_features` receives from a box listing `box_features` that sends
    /// `files`, read off the wire.
    async fn chunks_on_the_wire(
        box_features: &[&str],
        client_features: &[&str],
        files: Files,
    ) -> TestResult<Vec<(String, PayloadEncoding, usize)>> {
        let (client_io, box_io) = tokio::io::duplex(1024 * 1024);
        let server = serve_box(box_io, box_features, files);
        let mux = mux::client(Box::new(client_io));
        let mut control = wire::channel(mux.open().await?);
        let hello = OpenRequest::Control {
            protocol: PROTOCOL_VERSION,
            client: "client".into(),
            features: names(client_features),
            client_id: String::new(),
            client_name: String::new(),
            token: None,
        };
        wire::send(&mut control, &hello).await?;
        // The welcome, then the registry snapshot.
        for _ in 0..2 {
            wire::recv::<_, BoxControl>(&mut control)
                .await?
                .ok_or("the box closed the control stream")?;
        }
        let mut stream = wire::channel(mux.open().await?);
        wire::send(
            &mut stream,
            &OpenRequest::Sync {
                request: Value::Null,
            },
        )
        .await?;
        let mut chunks = Vec::new();
        loop {
            match wire::recv::<_, SyncFrame>(&mut stream).await? {
                Some((SyncFrame::Chunk { path, encoding, .. }, payload)) => {
                    chunks.push((path, encoding, payload.len()));
                }
                Some((SyncFrame::End, _)) => break,
                other => return Err(format!("unexpected {other:?}").into()),
            }
        }
        mux.shutdown();
        server.abort();
        Ok(chunks)
    }

    /// A zstd encoding for `len` bytes of data.
    fn zstd(len: usize) -> PayloadEncoding {
        PayloadEncoding::Zstd { size: len as u64 }
    }

    /// Chunks zstd makes smaller go out compressed. Once a chunk doesn't get
    /// smaller, the rest of its file goes raw; the next file, and the same file
    /// sent again from the start, try again.
    #[tokio::test]
    async fn the_box_compresses_chunks_until_one_does_not_shrink() -> TestResult {
        bounded(async {
            let text_chunk = || Bytes::from(text(64 * 1024));
            let files: Files = vec![
                ("a.js", vec![text_chunk(), text_chunk()]),
                ("app.bin", mixed()),
                ("b.js", vec![text_chunk()]),
                ("app.bin", mixed()),
            ];
            let chunks = chunks_on_the_wire(FEATURES, &[SYNC_ZSTD], files).await?;
            let encodings: Vec<(&str, PayloadEncoding)> = chunks
                .iter()
                .map(|(path, encoding, _)| (path.as_str(), *encoding))
                .collect();
            let (full, raw) = (64 * 1024, PayloadEncoding::Raw);
            assert_eq!(
                encodings,
                [
                    ("a.js", zstd(full)),
                    ("a.js", zstd(full)),
                    ("app.bin", zstd(full)),
                    ("app.bin", raw),
                    ("app.bin", raw),
                    ("b.js", zstd(full)),
                    ("app.bin", zstd(full)),
                    ("app.bin", raw),
                    ("app.bin", raw),
                ]
            );
            assert!(chunks[0].2 * 10 < full, "{chunks:?}");
            assert_eq!(chunks[3].2, full);
            Ok(())
        })
        .await
    }

    /// Chunks under 256 bytes go raw without trying, empty ones included.
    #[tokio::test]
    async fn chunks_under_the_threshold_go_raw() -> TestResult {
        bounded(async {
            let files: Files = vec![
                ("short.js", vec![Bytes::from(text(MIN_COMPRESSED - 1))]),
                ("at.js", vec![Bytes::from(text(MIN_COMPRESSED))]),
                ("empty.js", vec![Bytes::new()]),
            ];
            let chunks = chunks_on_the_wire(FEATURES, &[SYNC_ZSTD], files).await?;
            let encodings: Vec<PayloadEncoding> =
                chunks.iter().map(|(_, encoding, _)| *encoding).collect();
            assert_eq!(
                encodings,
                [
                    PayloadEncoding::Raw,
                    zstd(MIN_COMPRESSED),
                    PayloadEncoding::Raw
                ]
            );
            Ok(())
        })
        .await
    }

    /// Every chunk goes raw unless both ends list `sync_zstd`.
    #[tokio::test]
    async fn chunks_go_raw_unless_both_ends_list_the_feature() -> TestResult {
        bounded(async {
            for (box_features, client_features) in [(FEATURES, &[][..]), (&[], &[SYNC_ZSTD])] {
                let files: Files = vec![("app.bin", mixed())];
                let chunks = chunks_on_the_wire(box_features, client_features, files).await?;
                let expected: Vec<(String, PayloadEncoding, usize)> = mixed()
                    .iter()
                    .map(|part| ("app.bin".to_owned(), PayloadEncoding::Raw, part.len()))
                    .collect();
                assert_eq!(
                    chunks, expected,
                    "box {box_features:?}, client {client_features:?}"
                );
            }
            Ok(())
        })
        .await
    }

    /// A session between a box listing `box_features` and a client listing
    /// `client_features`, which negotiated `sync_zstd` when both list it.
    async fn session_with(
        box_features: &[&str],
        client_features: &[&str],
        files: Files,
    ) -> TestResult<(Session, JoinHandle<()>)> {
        let (client_io, box_io) = tokio::io::duplex(1024 * 1024);
        let server = serve_box(box_io, box_features, files);
        let mut options = SessionOptions::new("client");
        options.features = names(client_features);
        let session = Session::establish(Box::new(client_io), options).await?;
        assert_eq!(
            session.has_feature(SYNC_ZSTD),
            box_features.contains(&SYNC_ZSTD) && client_features.contains(&SYNC_ZSTD),
            "box {box_features:?}, client {client_features:?}"
        );
        Ok((session, server))
    }

    /// The file a client assembles from [`mixed`] chunks, between ends listing
    /// these features.
    async fn file_through_a_session(
        box_features: &[&str],
        client_features: &[&str],
    ) -> TestResult<Vec<u8>> {
        let files: Files = vec![("app.bin", mixed())];
        let (session, server) = session_with(box_features, client_features, files).await?;
        let mut sync = session.subscribe_sync(Value::Null).await?;
        let file = assemble(&mut sync).await?;
        session.close();
        server.abort();
        Ok(file)
    }

    /// The one file a subscription sends, checking offsets and the last mark.
    async fn assemble(sync: &mut SyncSubscription) -> TestResult<Vec<u8>> {
        let mut file = Vec::new();
        let mut ended = false;
        while let Some(message) = sync.next().await {
            let SyncMessage::Chunk {
                offset, data, last, ..
            } = message?
            else {
                return Err("expected only chunks".into());
            };
            assert_eq!(offset, file.len() as u64);
            file.extend_from_slice(&data);
            ended = last;
        }
        assert!(ended, "the last chunk was not marked last");
        Ok(file)
    }

    /// Compressed and raw chunks of one file arrive as the box sent them, with
    /// or without compression.
    #[tokio::test]
    async fn mixed_chunks_round_trip_through_a_session() -> TestResult {
        bounded(async {
            let expected: Vec<u8> = mixed().concat();
            for (box_features, client_features) in [
                (FEATURES, &[SYNC_ZSTD][..]),
                (FEATURES, &[]),
                (&[], &[SYNC_ZSTD]),
            ] {
                let file = file_through_a_session(box_features, client_features).await?;
                assert_eq!(
                    file, expected,
                    "box {box_features:?}, client {client_features:?}"
                );
            }
            Ok(())
        })
        .await
    }

    /// The client decodes whatever mix of encodings a box sends for one file.
    #[tokio::test]
    async fn the_client_reassembles_alternating_encodings() -> TestResult {
        bounded(async {
            let parts = [text(1000), text(2000), noise(500), text(3000)];
            let mut frames = Vec::new();
            let mut offset = 0;
            for (index, part) in parts.iter().enumerate() {
                let compressed = index % 2 == 1;
                let (encoding, payload) = if compressed {
                    let payload = zstd::bulk::compress(part, ZSTD_LEVEL)?;
                    (zstd(part.len()), Bytes::from(payload))
                } else {
                    (PayloadEncoding::Raw, Bytes::from(part.clone()))
                };
                let frame = SyncFrame::Chunk {
                    path: "app.bin".into(),
                    offset,
                    last: index + 1 == parts.len(),
                    encoding,
                };
                frames.push((frame, payload));
                offset += part.len() as u64;
            }
            frames.push((SyncFrame::End, Bytes::new()));
            let (_session, mut sync) = fake_box(frames).await?;
            assert_eq!(assemble(&mut sync).await?, parts.concat());
            Ok(())
        })
        .await
    }

    /// One manifest batch's worth of file entries, as the client daemon sends.
    fn manifest_batch(files: usize) -> Value {
        let files: Vec<Value> = (0..files)
            .map(|index| {
                serde_json::json!({
                    "path": format!("d{:03}/f{index:05}.txt", index % 500),
                    "size": index * 37,
                    "mode": 420,
                    "hash": format!("{:064x}", index * 7919),
                })
            })
            .collect();
        serde_json::json!({ "files": files })
    }

    /// Large input values go compressed and small ones raw; both reach the box's
    /// source intact, with and without the feature.
    #[tokio::test]
    async fn large_manifests_round_trip_compressed() -> TestResult {
        bounded(async {
            let (big, small) = (manifest_batch(2000), serde_json::json!({"files": []}));
            let (frame, payload) = input_frame(big.clone()).await?;
            let json_len = serde_json::to_vec(&big)?.len();
            let encoding = match &frame {
                SyncClientFrame::Input { encoding, .. } => *encoding,
                SyncClientFrame::EndInput => PayloadEncoding::Raw,
            };
            assert_eq!(encoding, zstd(json_len), "{frame:?}");
            assert!(
                payload.len() * 4 < json_len,
                "{} of {json_len}",
                payload.len()
            );
            let (frame, payload) = input_frame(small.clone()).await?;
            assert!(matches!(
                frame,
                SyncClientFrame::Input {
                    encoding: PayloadEncoding::Raw,
                    ..
                }
            ));
            assert!(payload.is_empty());

            for client_features in [&[SYNC_ZSTD][..], &[]] {
                let (session, server) = session_with(FEATURES, client_features, Vec::new()).await?;
                let mut sync = session
                    .subscribe_sync(serde_json::json!({"echo": true}))
                    .await?;
                for value in [&big, &small, &big] {
                    sync.send(value.clone()).await?;
                }
                sync.finish_input().await?;
                let echoed = sync.next().await.ok_or("no echo")??;
                let sent = vec![big.clone(), small.clone(), big.clone()];
                let expected = SyncMessage::Manifest(Value::Array(sent));
                assert_eq!(echoed, expected, "client {client_features:?}");
                session.close();
                server.abort();
            }
            Ok(())
        })
        .await
    }

    /// The box ends a subscription whose compressed input breaks its declared
    /// size or is not JSON, and tells the client why.
    #[tokio::test]
    async fn bad_compressed_input_is_rejected() -> TestResult {
        bounded(async {
            let compressed = |data: &[u8]| -> TestResult<Bytes> {
                Ok(Bytes::from(zstd::bulk::compress(data, ZSTD_LEVEL)?))
            };
            let json = serde_json::to_vec(&manifest_batch(100))?;
            let cases = [
                ("inflates past", 100, compressed(&json)?),
                ("over a frame", MAX_FRAME_LEN as u64 + 1, compressed(&json)?),
                ("shorter", json.len() as u64 + 1, compressed(&json)?),
                ("corrupt", 10, Bytes::from_static(b"not zstd")),
                ("not json", 9, compressed(b"not json!")?),
            ];
            for (case, size, payload) in cases {
                let (session, server) = session_with(FEATURES, &[SYNC_ZSTD], Vec::new()).await?;
                let mut sync = session
                    .subscribe_sync(serde_json::json!({"echo": true}))
                    .await?;
                let frame = SyncClientFrame::Input {
                    value: Value::Null,
                    encoding: PayloadEncoding::Zstd { size },
                };
                wire::send_with_payload(&mut sync.sink, &frame, payload).await?;
                let result = sync.next().await;
                assert!(
                    matches!(
                        &result,
                        Some(Err(SyncStreamError::Remote(error)))
                            if error.message.starts_with("compressed sync input")
                    ),
                    "{case}: {result:?}"
                );
                session.close();
                server.abort();
            }
            Ok(())
        })
        .await
    }

    /// A box that sends `frames` on the first sync stream, then holds it open.
    async fn fake_box(frames: Vec<(SyncFrame, Bytes)>) -> TestResult<(Session, SyncSubscription)> {
        let (client_io, box_io) = tokio::io::duplex(1024 * 1024);
        tokio::spawn(async move {
            let mut mux = mux::server(Box::new(box_io));
            let mut control = wire::channel(mux.accept().await.ok_or("no control stream")?);
            let _hello = wire::recv::<_, OpenRequest>(&mut control).await?;
            let welcome = BoxControl::Welcome {
                protocol: PROTOCOL_VERSION,
                server: "box".into(),
                features: vec![SYNC_ZSTD.into()],
                box_info: BoxInfo::default(),
            };
            wire::send(&mut control, &welcome).await?;
            let snapshot = BoxControl::Snapshot {
                snapshot: RegistrySnapshot::default(),
            };
            wire::send(&mut control, &snapshot).await?;
            let mut stream = wire::channel(mux.accept().await.ok_or("no sync stream")?);
            let _open = wire::recv::<_, OpenRequest>(&mut stream).await?;
            for (frame, payload) in frames {
                wire::send_with_payload(&mut stream, &frame, payload).await?;
            }
            // Keeps both streams until the client leaves.
            while let Ok(Some(_)) = wire::recv::<_, Value>(&mut control).await {}
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        });
        let session =
            Session::establish(Box::new(client_io), SessionOptions::new("client")).await?;
        let sync = session.subscribe_sync(Value::Null).await?;
        Ok((session, sync))
    }

    /// A zstd chunk frame of `app.bin` declaring `size` decoded bytes.
    fn zstd_chunk(size: u64) -> SyncFrame {
        SyncFrame::Chunk {
            path: "app.bin".into(),
            offset: 0,
            last: true,
            encoding: PayloadEncoding::Zstd { size },
        }
    }

    /// A box cannot make the client decode more than a chunk declares, declare
    /// more than a frame holds, or pass off a shorter or corrupt payload; each
    /// ends the subscription.
    #[tokio::test]
    async fn chunks_that_break_their_declared_size_are_rejected() -> TestResult {
        bounded(async {
            let compressed = |data: Vec<u8>| -> TestResult<Bytes> {
                Ok(Bytes::from(zstd::bulk::compress(&data, ZSTD_LEVEL)?))
            };
            let cases = [
                (
                    "inflates past",
                    zstd_chunk(1000),
                    compressed(text(64 * 1024))?,
                ),
                (
                    "over a frame",
                    zstd_chunk(MAX_FRAME_LEN as u64 + 1),
                    compressed(text(1000))?,
                ),
                ("shorter", zstd_chunk(2000), compressed(text(1000))?),
                ("corrupt", zstd_chunk(10), Bytes::from_static(b"not zstd")),
            ];
            for (case, frame, payload) in cases {
                let (_session, mut sync) = fake_box(vec![(frame, payload)]).await?;
                let result = sync.next().await;
                assert!(
                    matches!(
                        &result,
                        Some(Err(SyncStreamError::BadChunk { path, .. })) if path == "app.bin"
                    ),
                    "{case}: {result:?}"
                );
                assert!(
                    sync.next().await.is_none(),
                    "{case}: the subscription went on"
                );
            }
            Ok(())
        })
        .await
    }

    /// Dropping `next` while a chunk decompresses keeps the chunk for the next call.
    #[tokio::test(flavor = "multi_thread")]
    async fn next_is_cancel_safe_while_a_chunk_decompresses() -> TestResult {
        bounded(async {
            // Large enough that decompressing takes several polls.
            let data = text(4 * 1024 * 1024);
            let frame = zstd_chunk(data.len() as u64);
            let payload = Bytes::from(zstd::bulk::compress(&data, ZSTD_LEVEL)?);
            let frames = vec![(frame, payload), (SyncFrame::End, Bytes::new())];
            let (_session, mut sync) = fake_box(frames).await?;
            // Each attempt polls `next` once and drops it.
            let mut dropped_while_inflating = false;
            let message = loop {
                if let Some(message) = sync.next().now_or_never() {
                    break message;
                }
                dropped_while_inflating |= sync.inflating.is_some();
                tokio::task::yield_now().await;
            };
            assert!(
                dropped_while_inflating,
                "no call was dropped mid-decompression"
            );
            let received = match &message {
                Some(Ok(SyncMessage::Chunk { data, .. })) => data.as_ref(),
                _ => panic!("expected the chunk, got {message:?}"),
            };
            assert!(received == data.as_slice(), "the chunk changed");
            assert!(sync.next().await.is_none());
            Ok(())
        })
        .await
    }
}
