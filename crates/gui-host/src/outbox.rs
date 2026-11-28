//! The write queue of one connection, with a cap on the bytes it holds.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};

use bytes::Bytes;
use tokio::sync::Notify;

/// The queue was over its cap or closed; the message was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rejected;

#[derive(Debug, Default)]
struct State {
    queue: VecDeque<Bytes>,
    bytes: usize,
    closed: bool,
}

/// Encoded messages waiting for the connection's writer.
#[derive(Debug)]
pub(crate) struct Outbox {
    state: Mutex<State>,
    notify: Notify,
    cap: usize,
}

impl Outbox {
    /// An empty queue that holds at most `cap` bytes.
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            state: Mutex::new(State::default()),
            notify: Notify::new(),
            cap,
        }
    }

    /// The state; a panic while it was held cannot leave it inconsistent.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    /// Queues `message`. A message larger than the cap still goes into an
    /// empty queue, so one big frame never closes a connection that keeps up.
    pub(crate) fn push(&self, message: Bytes) -> Result<(), Rejected> {
        let mut state = self.state();
        let fits = state.queue.is_empty() || state.bytes + message.len() <= self.cap;
        if state.closed || !fits {
            return Err(Rejected);
        }
        state.bytes += message.len();
        state.queue.push_back(message);
        drop(state);
        self.notify.notify_one();
        Ok(())
    }

    /// Closes the queue. With `flush` the writer still sends what is queued;
    /// without, it stops at once.
    pub(crate) fn close(&self, flush: bool) {
        let mut state = self.state();
        state.closed = true;
        if !flush {
            state.queue.clear();
            state.bytes = 0;
        }
        drop(state);
        self.notify.notify_one();
    }

    /// The next message for the writer, or `None` once closed and empty.
    pub(crate) async fn pop(&self) -> Option<Bytes> {
        loop {
            {
                let mut state = self.state();
                if let Some(message) = state.queue.pop_front() {
                    state.bytes -= message.len();
                    return Some(message);
                }
                if state.closed {
                    return None;
                }
            }
            // `notify_one` keeps a permit when nobody waits, so a push between
            // the check above and this await is not missed.
            self.notify.notified().await;
        }
    }
}
