//! Event bus (plan 3.4). Every subscriber reads from its own bounded broadcast
//! position; a subscriber that falls behind gets one `resync` event instead of the
//! events it missed, then continues with live events.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use devlanes_proto::{Event, events};
use serde_json::{Value, json};
use tokio::sync::{broadcast, watch};

use crate::Session;

/// Per-subscriber queue length before overflow turns into `resync`.
pub const SUBSCRIBER_QUEUE: usize = 1024;
/// Recent events kept for non-following `devlanes debug events`.
const RECENT: usize = 200;

/// Sequenced publisher shared by the whole engine.
pub struct EventBus {
    sender: broadcast::Sender<Event>,
    seq: AtomicU64,
    recent: Mutex<VecDeque<Event>>,
    /// Each client forward by id, with the sequence number of the last event it
    /// sent or skipped, for [`Self::flushed`].
    forwarded: watch::Sender<BTreeMap<u64, u64>>,
    next_forward: AtomicU64,
    /// Subscribers that asked not to keep the daemon awake, for
    /// [`Self::awake_subscribers`].
    quiet: AtomicUsize,
}

/// A forward's receiver, counted in [`EventBus::quiet`] when it is quiet.
struct Subscription<'a> {
    bus: &'a EventBus,
    /// Taken out only by [`Drop`], so it can drop before the count does.
    receiver: Option<broadcast::Receiver<Event>>,
    quiet: bool,
}

impl<'a> Subscription<'a> {
    /// Subscribes to `bus`. The caller holds [`EventBus::recent`], so
    /// [`EventBus::awake_subscribers`] sees the receiver and the quiet count
    /// change together.
    fn new(bus: &'a EventBus, quiet: bool) -> Self {
        if quiet {
            bus.quiet.fetch_add(1, Ordering::Relaxed);
        }
        Self {
            bus,
            receiver: Some(bus.sender.subscribe()),
            quiet,
        }
    }
}

impl Drop for Subscription<'_> {
    /// Drops the receiver and the quiet count under one lock, so
    /// [`EventBus::awake_subscribers`] never sees one without the other.
    fn drop(&mut self) {
        let _counts = crate::model::lock(&self.bus.recent);
        self.receiver.take();
        if self.quiet {
            self.bus.quiet.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// Removes a forward from [`EventBus::forwarded`] when it ends.
struct Forwarding<'a> {
    bus: &'a EventBus,
    id: u64,
}

impl Forwarding<'_> {
    /// Records that the forward handled every event up to `seq`.
    fn handled(&self, seq: u64) {
        self.bus.forwarded.send_modify(|forwards| {
            forwards.insert(self.id, seq);
        });
    }
}

impl Drop for Forwarding<'_> {
    /// A forward that ended holds up no [`EventBus::flushed`].
    fn drop(&mut self) {
        self.bus.forwarded.send_modify(|forwards| {
            forwards.remove(&self.id);
        });
    }
}

impl Default for EventBus {
    /// Uses [`SUBSCRIBER_QUEUE`] as the per-subscriber bound.
    fn default() -> Self {
        Self::with_capacity(SUBSCRIBER_QUEUE)
    }
}

impl EventBus {
    /// Bus whose subscribers may lag by `capacity` events before a resync.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            sender: broadcast::channel(capacity.max(1)).0,
            seq: AtomicU64::new(1),
            recent: Mutex::new(VecDeque::with_capacity(RECENT)),
            forwarded: watch::channel(BTreeMap::new()).0,
            next_forward: AtomicU64::new(1),
            quiet: AtomicUsize::new(0),
        }
    }

    /// Resolves once every client forward sent or skipped each event emitted
    /// before the call, so a daemon that is about to close its connections does
    /// not drop the last ones. A client that stops reading holds it up, so
    /// callers bound the wait.
    pub async fn flushed(&self) {
        let last = self.seq.load(Ordering::Relaxed).saturating_sub(1);
        let mut forwards = self.forwarded.subscribe();
        let _ = forwards
            .wait_for(|forwards| forwards.values().all(|&seq| seq >= last))
            .await;
    }

    /// Publishes an event; having no subscribers is normal. The sequence number
    /// is taken and the event sent under one lock, so subscribers receive events
    /// in sequence order even when several threads emit at once.
    pub fn emit(&self, name: &str, data: Value) {
        let mut recent = crate::model::lock(&self.recent);
        let event = Event {
            seq: self.seq.fetch_add(1, Ordering::Relaxed),
            event: name.to_owned(),
            data,
        };
        // A nudge to fetch `host.list` or `remote.clients` again says nothing
        // on its own, and a busy box would push everything else out of
        // `debug events`.
        if name != devlanes_proto::remote::MIRROR_EVENT
            && name != devlanes_proto::remote::CLIENTS_EVENT
        {
            if recent.len() == RECENT {
                recent.pop_front();
            }
            recent.push_back(event.clone());
        }
        let _ = self.sender.send(event);
    }

    /// A new subscription positioned at the next event.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }

    /// The last events, oldest first.
    pub fn recent(&self) -> Vec<Event> {
        self.recent
            .lock()
            .map(|recent| recent.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Current subscriber count, quiet ones included, for `doctor`.
    pub fn subscribers(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Counts the subscribers that keep the daemon awake, leaving out those
    /// that subscribed with `keeps_awake: false`. The idle exit and the
    /// sampler's fast tick read it.
    pub fn awake_subscribers(&self) -> usize {
        // Quiet forwards join and leave under this lock, so both reads agree.
        let _counts = crate::model::lock(&self.recent);
        self.sender
            .receiver_count()
            .saturating_sub(self.quiet.load(Ordering::Relaxed))
    }

    /// Forwards events accepted by `filter` to `session` until it disconnects.
    pub async fn forward(&self, session: &Session, filter: impl Fn(&Event) -> bool) {
        self.forward_from(session, true, |_| Vec::new(), filter)
            .await;
    }

    /// Like [`Self::forward`], but subscribes first and then sends `initial` to
    /// this session only, so a snapshot never misses a change and other
    /// subscribers never see it. `initial` gets a sequence number of its own,
    /// after every event the subscription missed and before every one it gets.
    /// Without `keeps_awake`, the subscription is left out of
    /// [`Self::awake_subscribers`].
    pub async fn forward_from(
        &self,
        session: &Session,
        keeps_awake: bool,
        initial: impl FnOnce(u64) -> Vec<Event>,
        filter: impl Fn(&Event) -> bool,
    ) {
        let (mut subscription, seq, forwarding) = {
            // Emits take their number under this lock, so none falls in between.
            let _emits = crate::model::lock(&self.recent);
            let seq = self.seq.fetch_add(1, Ordering::Relaxed);
            let forwarding = Forwarding {
                bus: self,
                id: self.next_forward.fetch_add(1, Ordering::Relaxed),
            };
            // Events before `seq` are not this forward's to send.
            forwarding.handled(seq);
            (Subscription::new(self, !keeps_awake), seq, forwarding)
        };
        let Some(receiver) = subscription.receiver.as_mut() else {
            return;
        };
        let disconnected = session.disconnected();
        for event in initial(seq) {
            if session.event(event).await.is_err() {
                return;
            }
        }
        loop {
            let next = tokio::select! {
                () = disconnected.cancelled() => return,
                next = receiver.recv() => next,
            };
            let event = match next {
                Ok(event) if filter(&event) => event,
                Ok(event) => {
                    forwarding.handled(event.seq);
                    continue;
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => Event {
                    seq: self.seq.load(Ordering::Relaxed),
                    event: events::RESYNC.into(),
                    data: json!({"missed": missed}),
                },
                Err(broadcast::error::RecvError::Closed) => return,
            };
            let handled = event.seq;
            if session.event(event).await.is_err() {
                return;
            }
            forwarding.handled(handled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Events emitted from many threads at once arrive in sequence order.
    fn concurrent_emits_arrive_in_sequence_order() {
        let bus = std::sync::Arc::new(EventBus::with_capacity(64 * 1024));
        let mut receiver = bus.subscribe();
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let bus = bus.clone();
                std::thread::spawn(move || {
                    for _ in 0..2000 {
                        bus.emit("test", Value::Null);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("emitter");
        }
        let mut last = 0;
        let mut count = 0;
        while let Ok(event) = receiver.try_recv() {
            assert!(event.seq > last, "{} after {last}", event.seq);
            last = event.seq;
            count += 1;
        }
        assert_eq!(count, 16_000);
    }

    #[tokio::test]
    /// `flushed` returns only once a forward sent every event emitted before it,
    /// counting the ones its filter skips.
    async fn flushed_waits_for_forwards_to_send_what_was_emitted() {
        let bus = std::sync::Arc::new(EventBus::default());
        let (session, mut frames) = Session::local(1);
        let forward = {
            let bus = bus.clone();
            tokio::spawn(async move { bus.forward(&session, |e| e.event != "skipped").await })
        };
        let mut forwards = bus.forwarded.subscribe();
        forwards
            .wait_for(|forwards| !forwards.is_empty())
            .await
            .expect("the forward registers");
        for name in ["first", "skipped", "last"] {
            bus.emit(name, Value::Null);
        }
        tokio::time::timeout(std::time::Duration::from_secs(10), bus.flushed())
            .await
            .expect("flushed");
        let mut sent = Vec::new();
        while let Ok(devlanes_ipc::Frame::Control(devlanes_proto::Control::Event(event))) =
            frames.try_recv()
        {
            sent.push(event.event);
        }
        assert_eq!(sent, ["first", "last"]);
        forward.abort();
    }

    /// A forward on `bus` that runs until its session closes.
    struct Joined {
        session: Session,
        forward: tokio::task::JoinHandle<()>,
        _frames: tokio::sync::mpsc::Receiver<devlanes_ipc::Frame>,
    }

    /// Starts a forward on `bus` and returns once its first frame arrived,
    /// when its subscription exists.
    async fn join(bus: &std::sync::Arc<EventBus>, id: u64, keeps_awake: bool) -> Joined {
        let (session, mut frames) = Session::local(id);
        let forward = {
            let bus = bus.clone();
            let session = session.clone();
            tokio::spawn(async move {
                let marker = |seq| {
                    vec![Event {
                        seq,
                        event: "joined".into(),
                        data: Value::Null,
                    }]
                };
                bus.forward_from(&session, keeps_awake, marker, |_| true)
                    .await;
            })
        };
        let first = tokio::time::timeout(std::time::Duration::from_secs(10), frames.recv())
            .await
            .expect("the marker arrives");
        assert!(first.is_some(), "the forward sends its marker");
        Joined {
            session,
            forward,
            _frames: frames,
        }
    }

    /// Closes the forward's session and returns once the forward ended.
    async fn leave(joined: Joined) {
        joined.session.close();
        tokio::time::timeout(std::time::Duration::from_secs(10), joined.forward)
            .await
            .expect("the forward ends")
            .expect("the forward ran");
    }

    #[tokio::test]
    /// Quiet forwards never count as awake while they join and leave, and
    /// plain ones always do.
    async fn awake_subscribers_leave_out_quiet_forwards() {
        let bus = std::sync::Arc::new(EventBus::default());
        assert_eq!((bus.subscribers(), bus.awake_subscribers()), (0, 0));

        let quiet = join(&bus, 1, false).await;
        assert_eq!((bus.subscribers(), bus.awake_subscribers()), (1, 0));
        let plain = join(&bus, 2, true).await;
        assert_eq!((bus.subscribers(), bus.awake_subscribers()), (2, 1));
        let second_quiet = join(&bus, 3, false).await;
        assert_eq!((bus.subscribers(), bus.awake_subscribers()), (3, 1));

        leave(quiet).await;
        assert_eq!((bus.subscribers(), bus.awake_subscribers()), (2, 1));
        leave(plain).await;
        assert_eq!((bus.subscribers(), bus.awake_subscribers()), (1, 0));
        leave(second_quiet).await;
        assert_eq!((bus.subscribers(), bus.awake_subscribers()), (0, 0));
    }
}
