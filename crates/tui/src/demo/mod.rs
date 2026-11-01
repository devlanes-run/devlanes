//! A fake daemon that answers every effect of the [`App`] at once, from the
//! mock data in [`data`]. The `screens` example draws every page against it,
//! and the web demo (crates/tui-web) runs the TUI in the browser on it.

pub mod data;

use std::collections::VecDeque;

use devlanes_proto::{AttachResult, Event, Page, RpcError};
use serde_json::json;

use crate::{
    ApiError,
    app::{App, Effect, Msg, RowId, Selected},
    keys::{KeyCode, KeyEvent, KeyModifiers},
    logfile::{LogChunk, LogPosition},
    settings,
};

/// Messages one step may cause before the fake daemon stops answering.
const STEP_BUDGET: usize = 2_000;

/// The user config the options page reads.
pub const USER_CONFIG: &str = "\
# machine settings for devlanes
tui:
  theme: dark
proxy:
  https_port: 8443 # 443 needs the helper
  http_port: 8080
history:
  retention: 14d
";

/// The TUI with a fake daemon that answers every effect at once.
pub struct Fake {
    pub app: App,
    queue: VecDeque<Msg>,
    /// What `status` answers; a scene may swap it, as the idle scene does.
    pub lanes: fn() -> Vec<devlanes_proto::LaneSummary>,
    /// What `host.list` answers: no boxes until a scene connects them.
    pub boxes: serde_json::Value,
    /// What `remote.clients` answers: nobody until a scene pairs some.
    pub clients: serde_json::Value,
    /// What `remote.pair` answers: a code unless a scene says otherwise.
    pub pairing: Result<serde_json::Value, RpcError>,
    /// What an attached `connect` to a box sends: the link connecting
    /// unless a scene has it ask something or fail.
    pub connect: fn(u64, &str) -> Vec<Msg>,
    /// Seconds the fake daemon's clock moved on from [`data::NOW_MS`];
    /// `metrics` answers [`data::metrics_at`] this step.
    pub step: u64,
}

/// A `remote.state` or `remote.prompt` event of the `connect` `generation`.
pub fn connect_event(generation: u64, name: &str, data: serde_json::Value) -> Msg {
    Msg::ConnectEvent {
        generation,
        event: Event {
            seq: 0,
            event: name.into(),
            data,
        },
    }
}

/// The link to `host` connecting, as `connect` reports it first.
pub fn connecting(generation: u64, host: &str) -> Vec<Msg> {
    vec![connect_event(
        generation,
        devlanes_proto::remote::STATE_EVENT,
        json!({"host": host, "state": "connecting", "attempt": 1}),
    )]
}

impl Fake {
    /// A TUI of `width` x `height` that subscribed and loaded the lanes.
    pub fn new(width: u16, height: u16) -> Self {
        let mut app = App::new(width, height);
        app.version = data::VERSION;
        // Step times count from here, so they draw the same on every run.
        app.clock = Some(web_time::Instant::now());
        // Opened from the monorepo's main worktree.
        app.prefer_path("/work/acme".into());
        let mut fake = Self {
            app,
            queue: VecDeque::new(),
            lanes: data::lanes,
            boxes: json!({"items": []}),
            clients: data::no_clients(),
            pairing: Ok(data::pair_code()),
            connect: connecting,
            step: 0,
        };
        for effect in fake.app.start() {
            fake.answer(effect);
        }
        fake.drain();
        fake
    }

    /// Applies `msg` and everything the fake daemon answers to its effects.
    pub fn send(&mut self, msg: Msg) {
        self.queue.push_back(msg);
        self.drain();
    }

    /// One second later: the daemon's clock and the app's pinned clock move
    /// on, then the app ticks, which refetches metrics on the monitor.
    pub fn tick(&mut self) {
        self.step += 1;
        self.later(1_000);
        self.send(Msg::Tick);
    }

    /// Moves the app's pinned clock on by `ms`, for step times.
    pub fn later(&mut self, ms: u64) {
        self.app.clock = self
            .app
            .clock
            .map(|now| now + std::time::Duration::from_millis(ms));
    }

    /// Presses `code`.
    pub fn key(&mut self, code: KeyCode) {
        self.send(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    /// Presses Shift and `c`.
    pub fn shifted(&mut self, c: char) {
        self.send(Msg::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::SHIFT,
        )));
    }

    /// Selects `row` of lane `lane`, unfolding what hides it.
    pub fn select(&mut self, lane: &str, row: RowId) {
        let mut effects = Vec::new();
        self.app.select(
            Selected {
                lane: lane.into(),
                row,
            },
            &mut effects,
        );
        for effect in effects {
            self.answer(effect);
        }
        self.drain();
    }

    /// Selects the unit `name` of lane `lane`.
    pub fn unit(&mut self, lane: &str, name: &str) {
        self.select(lane, RowId::Unit(name.into()));
    }

    /// Selects the whole-lane row of `lane`.
    pub fn lane(&mut self, lane: &str) {
        self.select(lane, RowId::WholeLane);
    }

    /// Connects the fake's boxes: `host.list` lists them from now on, and a
    /// `remote.mirror.changed` makes the TUI ask.
    pub fn connect_boxes(&mut self) {
        self.show_boxes(data::boxes());
    }

    /// Makes `host.list` answer `boxes` and announces it.
    pub fn show_boxes(&mut self, boxes: serde_json::Value) {
        self.boxes = boxes;
        self.send(Msg::Event(Event {
            seq: 1,
            event: devlanes_proto::remote::MIRROR_EVENT.into(),
            data: json!({"host": "hetzner"}),
        }));
    }

    /// Types `text` one key at a time.
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.key(KeyCode::Char(c));
        }
    }

    /// Runs queued messages until none are left.
    fn drain(&mut self) {
        for _ in 0..STEP_BUDGET {
            let Some(msg) = self.queue.pop_front() else {
                return;
            };
            for effect in self.app.update(msg) {
                self.answer(effect);
            }
        }
    }

    /// What the fake daemon sends back for one effect.
    fn answer(&mut self, effect: Effect) {
        let unit_name = |reference: &str| {
            reference
                .rsplit_once(':')
                .map_or(reference, |(_, unit)| unit)
                .to_owned()
        };
        let reply = match effect {
            Effect::Subscribe => vec![Msg::Subscribed(Ok(()))],
            Effect::Refresh => vec![Msg::Status(Ok((self.lanes)()))],
            Effect::Attach {
                generation,
                lane,
                unit,
                columns,
                rows,
                ..
            } => vec![
                Msg::AttachOpened {
                    generation,
                    result: Ok(AttachResult {
                        stream_id: 1,
                        unit: unit.clone(),
                        lane,
                    }),
                },
                Msg::AttachView {
                    generation,
                    view: Box::new(screen_view(&unit_name(&unit), columns, rows)),
                },
            ],
            // The daemon redraws a unit's stream at its viewer's new size.
            Effect::ResizePty { columns, rows } => match &self.app.attached {
                Some(attached) if !attached.container => vec![Msg::AttachView {
                    generation: attached.generation,
                    view: Box::new(screen_view(&attached.unit, columns, rows)),
                }],
                _ => Vec::new(),
            },
            Effect::FollowContainer {
                generation,
                container,
                ..
            } => vec![
                Msg::ContainerLogsOpened {
                    generation,
                    result: Ok(()),
                },
                Msg::AttachOutput {
                    generation,
                    bytes: data::container_logs(&container),
                },
            ],
            Effect::FetchLogs {
                generation, query, ..
            } => {
                let unit = query
                    .units
                    .first()
                    .map(|u| unit_name(u))
                    .unwrap_or_default();
                vec![Msg::Logs {
                    generation,
                    result: Ok(Page::complete(data::log_page(&unit))),
                }]
            }
            Effect::FetchMetrics(_) => vec![Msg::Metrics(Ok(data::metrics_at(self.step)))],
            Effect::FetchDaemon => vec![Msg::DaemonInfo {
                status: Ok(data::daemon_status()),
                doctor: Ok(data::doctor()),
            }],
            Effect::ReadDaemonLog { .. } => vec![Msg::DaemonLog(Ok(LogChunk {
                lines: data::daemon_log(),
                position: LogPosition {
                    offset: 4_096,
                    file: (1, 1),
                },
                reset: true,
            }))],
            Effect::FetchProfiles { lane } => vec![Msg::Profiles {
                result: Ok(data::profiles(&lane)),
                lane,
            }],
            Effect::FetchTasks { lane } => vec![Msg::Tasks {
                result: Ok(data::tasks(&lane)),
                lane,
            }],
            Effect::FetchBoxes => vec![Msg::Boxes(Ok(self.boxes.clone()))],
            Effect::FetchClients => vec![Msg::Clients(Ok(self.clients.clone()))],
            Effect::Pair => vec![Msg::Paired(self.pairing.clone().map_err(ApiError::Rpc))],
            Effect::Connect { generation, host } => (self.connect)(generation, &host),
            Effect::SaveBoxCommand { generation, .. } => vec![Msg::BoxSaved {
                generation,
                result: Ok(()),
            }],
            // The file starts as USER_CONFIG; a save writes its changes over it.
            Effect::Settings(request) => {
                let mut source = USER_CONFIG.to_owned();
                if let settings::Request::Save { changes } = request {
                    for (key, value) in changes {
                        if let Ok(text) =
                            devlanes_config::edit_user_source(&source, &key, value.as_ref())
                        {
                            source = text;
                        }
                    }
                }
                vec![Msg::Settings(settings::Reply::Loaded {
                    path: "~/.devlanes/config.yaml".into(),
                    source: Ok(source),
                    error: None,
                })]
            }
            _ => Vec::new(),
        };
        self.queue.extend(reply);
    }
}

/// The daemon's whole redraw of `unit`'s stream at `columns`x`rows`: its
/// screen, with older lines in the log as `log_page` has them.
fn screen_view(unit: &str, columns: u16, rows: u16) -> devlanes_proto::AttachView {
    devlanes_proto::AttachView {
        stream_id: 1,
        from: 0,
        lines: Vec::new(),
        log_unit: unit.to_owned(),
        before_seq: 60,
        columns,
        rows,
        screen: String::from_utf8_lossy(&data::screen(unit)).into_owned(),
        whole: true,
    }
}
