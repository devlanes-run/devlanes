//! What the host sends, as the caller sees it.

use std::sync::Arc;

use devlanes_gui_codec::Frame;
use devlanes_gui_proto::{
    ClientMessage, CodecId, Cursor, Holder, LockState, MoveResize, WindowId, WindowInfo,
};
use tokio::sync::{mpsc, oneshot};

use crate::{ClientError, Shared};

/// A decoded frame of a window's stream.
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    /// The window.
    pub window: WindowId,
    /// Its sequence number in this client's stream.
    pub seq: u64,
    /// Whether it was a keyframe.
    pub keyframe: bool,
    /// The picture, BGRA.
    pub frame: Frame,
}

/// Something that happened on the connection.
#[derive(Debug)]
pub enum Event {
    /// A window appeared.
    WindowAdded(WindowInfo),
    /// A window's description changed.
    WindowChanged(WindowInfo),
    /// A window closed.
    WindowRemoved(WindowId),
    /// A viewer's decoded frame. It is acknowledged when [`Events::next`]
    /// returns it, so a caller that stops reading events stops the stream.
    Frame(DecodedFrame),
    /// A window's lock state as the viewer shows it changed: the host's
    /// state combined with this client's resize and its link.
    Lock {
        /// The window.
        window: WindowId,
        /// The new state.
        state: LockState,
    },
    /// The host applied a size change; frames from `from_seq` on follow it.
    ResizeApplied {
        /// The window.
        window: WindowId,
        /// This client's request it answers, or 0.
        request: u32,
        /// The first sequence number after the change.
        from_seq: u64,
    },
    /// The app's cursor changed.
    Cursor(Cursor),
    /// The app started an interactive move or resize.
    MoveResizeStarted(MoveResize),
    /// The app's clipboard text changed.
    Clipboard(String),
    /// Who holds input now.
    InputHolder(Option<Holder>),
    /// A viewer sent input; control connections only.
    Activity,
    /// This client's decoder of a codec failed on a frame and again on the
    /// keyframe it asked for after that, so the codec does not work here.
    /// The protocol has no way to change codecs on a connection: a viewer
    /// reconnects without offering that decoder.
    CodecFailed(CodecId),
    /// The connection closed, with the error that closed it, if any. Every
    /// window was reported locked before this.
    Closed(Option<ClientError>),
}

/// One place in the order of events. A frame takes its place when it
/// arrives and is filled in once decoded, so what came after it, such as the
/// end of a lock, is not seen before it.
#[derive(Debug)]
pub(crate) enum Slot {
    /// An event.
    Event(Event),
    /// A frame being decoded; dropped unfilled when it was discarded.
    Frame(oneshot::Receiver<DecodedFrame>),
}

/// The events of one connection, in the order they happened.
#[derive(Debug)]
pub struct Events {
    rx: mpsc::UnboundedReceiver<Slot>,
    shared: Arc<Shared>,
}

impl Events {
    /// Events from `rx`, acknowledging frames on `shared`'s connection.
    pub(crate) fn new(rx: mpsc::UnboundedReceiver<Slot>, shared: Arc<Shared>) -> Self {
        Self { rx, shared }
    }

    /// The next event, or `None` after [`Event::Closed`]. A frame is
    /// acknowledged to the host as it is returned; one that failed to decode
    /// is skipped, since its decoder acknowledged it already.
    pub async fn next(&mut self) -> Option<Event> {
        loop {
            let frame = match self.rx.recv().await? {
                Slot::Event(event) => return Some(event),
                Slot::Frame(pending) => match pending.await {
                    Ok(frame) => frame,
                    Err(_) => continue,
                },
            };
            // Fails only once the connection is closed, when nobody waits for it.
            let _ = self.shared.send(&ClientMessage::FrameAck {
                window: frame.window,
                seq: frame.seq,
            });
            return Some(Event::Frame(frame));
        }
    }
}
