//! The tasks behind a connection: the writer with its keep-alives, the
//! reader that tracks windows and lock states, and one decoder per window.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use bytes::{Bytes, BytesMut};
use devlanes_gui_codec::{DecoderBackend, Packet, VideoDecoder};
use devlanes_gui_proto::{
    ClientMessage, CodecId, HEARTBEAT_INTERVAL, HostMessage, VideoFrame, WindowId,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::{mpsc, oneshot};

use crate::events::Slot;
use crate::{ClientError, DecodedFrame, Event, Shared, WindowState};

/// The current time on tokio's clock, which tests pause and advance.
pub(crate) fn now() -> Instant {
    tokio::time::Instant::now().into_std()
}

/// Sleeps until `at`, or forever when there is nothing to wait for.
async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at.into()).await,
        None => std::future::pending().await,
    }
}

/// Writes queued messages, and a keep-alive whenever the client holds a
/// button or key and sent nothing for [`HEARTBEAT_INTERVAL`]. Ends when the
/// connection is closed or a write fails.
pub(crate) async fn write_loop<S: AsyncWrite>(
    mut writer: WriteHalf<S>,
    mut rx: mpsc::UnboundedReceiver<Bytes>,
    shared: Arc<Shared>,
) {
    let mut keep_alive = BytesMut::new();
    // A keep-alive has no fields, so encoding cannot fail.
    let _ = ClientMessage::KeepAlive.encode(&mut keep_alive);
    let keep_alive = keep_alive.freeze();
    let mut last_sent = now();
    loop {
        let holding = shared.state().held.any();
        let due = holding.then(|| last_sent + HEARTBEAT_INTERVAL);
        let bytes = tokio::select! {
            _ = shared.close.cancelled() => break,
            next = rx.recv() => match next {
                Some(bytes) => bytes,
                None => break,
            },
            _ = sleep_until(due) => keep_alive.clone(),
        };
        if writer.write_all(&bytes).await.is_err() {
            break;
        }
        last_sent = now();
    }
    let _ = writer.shutdown().await;
}

/// Reads and handles host messages until the connection ends, then reports
/// every window locked and [`Event::Closed`].
pub(crate) async fn read_loop<S: AsyncRead>(
    mut reader: ReadHalf<S>,
    mut buf: BytesMut,
    shared: Arc<Shared>,
    backends: Vec<Arc<dyn DecoderBackend>>,
) {
    let mut reader_state = Reader {
        shared: shared.clone(),
        backends,
        decoders: HashMap::new(),
    };
    let error = loop {
        // Messages that arrived with the welcome are already in `buf`.
        match reader_state.drain(&mut buf) {
            Ok(()) => {}
            Err(error) => break Some(error),
        }
        let next = {
            let state = shared.state();
            let at = now();
            state
                .windows
                .values()
                .filter_map(|w| w.lock.next_change(at))
                .min()
        };
        tokio::select! {
            _ = shared.close.cancelled() => break None,
            read = reader.read_buf(&mut buf) => match read {
                Ok(0) => break None,
                Ok(_) => {}
                Err(error) => break Some(error.into()),
            },
            _ = sleep_until(next) => {}
            _ = shared.wake.notified() => {}
        }
        reader_state.refresh_all();
    };
    let at = now();
    {
        let mut state = shared.state();
        for (window, win) in state.windows.iter_mut() {
            win.lock.link_closed();
            shared.refresh_lock(*window, win, at);
        }
        // Dropping the senders fails the waiting requests with `Closed`.
        state.screenshots.clear();
        state.syncs.clear();
    }
    shared.emit(Event::Closed(error));
}

/// The reader's own state.
struct Reader {
    shared: Arc<Shared>,
    backends: Vec<Arc<dyn DecoderBackend>>,
    /// Frames to each window's decoder task.
    decoders: HashMap<WindowId, mpsc::UnboundedSender<Job>>,
}

impl Reader {
    /// Handles every complete message in `buf`.
    fn drain(&mut self, buf: &mut BytesMut) -> Result<(), ClientError> {
        while let Some(message) = HostMessage::decode(buf)? {
            self.handle(message);
        }
        Ok(())
    }

    /// Reports lock states that changed with time.
    fn refresh_all(&self) {
        let at = now();
        let mut state = self.shared.state();
        for (window, win) in state.windows.iter_mut() {
            self.shared.refresh_lock(*window, win, at);
        }
    }

    /// Handles one message from the host.
    fn handle(&mut self, message: HostMessage) {
        let at = now();
        let shared = self.shared.clone();
        let mut state = shared.state();
        for win in state.windows.values_mut() {
            win.lock.data(at);
        }
        let event = match message {
            HostMessage::WindowAdded(info) => {
                state
                    .windows
                    .insert(info.id, WindowState::new(info.lock, at));
                Event::WindowAdded(info)
            }
            HostMessage::WindowChanged(info) => {
                if let Some(win) = state.windows.get_mut(&info.id) {
                    win.lock.host_state(info.lock);
                    // The description carries the host's state; report the
                    // combined one only when it differs from that.
                    win.shown = win.lock.state(at);
                }
                Event::WindowChanged(info)
            }
            HostMessage::WindowRemoved { window } => {
                state.windows.remove(&window);
                self.decoders.remove(&window);
                Event::WindowRemoved(window)
            }
            HostMessage::VideoFrame(frame) => {
                let window = frame.window;
                let Some(win) = state.windows.get_mut(&window) else {
                    drop(state);
                    // Discarded, and acknowledged so the host's pacing goes on.
                    let _ = self.shared.send(&ClientMessage::FrameAck {
                        window,
                        seq: frame.seq,
                    });
                    return;
                };
                win.lock.frame(frame.seq, at);
                self.shared.refresh_lock(window, win, at);
                drop(state);
                self.decode(frame);
                return;
            }
            HostMessage::LockChanged { window, state: host } => {
                if let Some(win) = state.windows.get_mut(&window) {
                    win.lock.host_state(host);
                    self.shared.refresh_lock(window, win, at);
                }
                return;
            }
            HostMessage::ResizeApplied {
                window,
                request,
                from_seq,
            } => {
                if let Some(win) = state.windows.get_mut(&window) {
                    win.lock.resize_applied(request, from_seq);
                }
                Event::ResizeApplied {
                    window,
                    request,
                    from_seq,
                }
            }
            HostMessage::Screenshot { request, result } => {
                if let Some(reply) = state.screenshots.remove(&request) {
                    let _ = reply.send(result);
                }
                return;
            }
            HostMessage::Synced { request } => {
                if let Some(reply) = state.syncs.remove(&request) {
                    let _ = reply.send(());
                }
                return;
            }
            HostMessage::Cursor(cursor) => Event::Cursor(cursor),
            HostMessage::MoveResizeStarted(started) => Event::MoveResizeStarted(started),
            HostMessage::Clipboard { text } => Event::Clipboard(text),
            HostMessage::InputHolder(holder) => Event::InputHolder(holder),
            HostMessage::Activity => Event::Activity,
            // A heartbeat only proves the link, which `data` above recorded.
            // A second welcome or refusal has no meaning mid-connection.
            HostMessage::Heartbeat | HostMessage::Welcome(_) | HostMessage::Refused(_) => return,
        };
        self.shared.emit(event);
    }

    /// Hands `frame` to its window's decoder task, starting one if needed,
    /// and reserves its place among the events.
    fn decode(&mut self, frame: VideoFrame) {
        let (slot, pending) = oneshot::channel();
        let _ = self.shared.events.send(Slot::Frame(pending));
        let window = frame.window;
        let tx = self.decoders.entry(window).or_insert_with(|| {
            let (tx, rx) = mpsc::unbounded_channel();
            tokio::spawn(decode_loop(
                window,
                rx,
                self.backends.clone(),
                self.shared.clone(),
            ));
            tx
        });
        // The task ends only when this sender is dropped.
        let _ = tx.send((frame, slot));
    }
}

/// A frame to decode and the place its event fills.
type Job = (VideoFrame, oneshot::Sender<DecodedFrame>);

/// A decoder and the codec it decodes.
type Decoder = (CodecId, Box<dyn VideoDecoder>);

/// Decodes one window's frames in order on the blocking pool. A frame that
/// fails is acknowledged as discarded and followed by a keyframe request; a
/// decoded one fills its event's place and is acknowledged when read.
async fn decode_loop(
    window: WindowId,
    mut rx: mpsc::UnboundedReceiver<Job>,
    backends: Vec<Arc<dyn DecoderBackend>>,
    shared: Arc<Shared>,
) {
    let mut decoder: Option<Decoder> = None;
    while let Some((frame, slot)) = rx.recv().await {
        let (seq, keyframe) = (frame.seq, frame.keyframe);
        let backends = backends.clone();
        let current = decoder.take();
        let job = move || decode_one(current, &backends, &frame);
        let (kept, outcome) = match tokio::task::spawn_blocking(job).await {
            Ok(result) => result,
            Err(_) => (None, Err(())),
        };
        decoder = kept;
        match outcome {
            Ok(Some(picture)) => {
                let _ = slot.send(DecodedFrame {
                    window,
                    seq,
                    keyframe,
                    frame: picture,
                });
            }
            Ok(None) => {
                let _ = shared.send(&ClientMessage::FrameAck { window, seq });
            }
            Err(()) => {
                let _ = shared.send(&ClientMessage::FrameAck { window, seq });
                let _ = shared.send(&ClientMessage::KeyframeRequest { window });
            }
        }
    }
}

/// Decodes `frame` with `decoder`, or a new decoder when it is missing or of
/// another codec. Returns the decoder to keep (none after an error, so the
/// next keyframe starts clean) and the picture.
fn decode_one(
    decoder: Option<Decoder>,
    backends: &[Arc<dyn DecoderBackend>],
    frame: &VideoFrame,
) -> (
    Option<Decoder>,
    Result<Option<devlanes_gui_codec::Frame>, ()>,
) {
    let decoder = match decoder {
        Some(decoder) if decoder.0 == frame.codec => Some(decoder),
        _ => backends
            .iter()
            .filter(|backend| backend.offer().codec == frame.codec)
            .max_by_key(|backend| backend.offer().hardware)
            .and_then(|backend| match backend.decoder() {
                Ok(decoder) => Some((frame.codec, decoder)),
                Err(error) => {
                    tracing::debug!(%error, "cannot start a GUI decoder");
                    None
                }
            }),
    };
    let Some((codec, mut decoder)) = decoder else {
        return (None, Err(()));
    };
    let packet = Packet {
        payload: &frame.payload,
        width: frame.size.width,
        height: frame.size.height,
        keyframe: frame.keyframe,
        timestamp: std::time::Duration::from_micros(frame.timestamp_us),
    };
    match decoder.decode(&packet) {
        Ok(picture) => (Some((codec, decoder)), Ok(picture)),
        Err(error) => {
            tracing::debug!(%error, window = frame.window.0, seq = frame.seq, "GUI frame did not decode");
            (None, Err(()))
        }
    }
}
