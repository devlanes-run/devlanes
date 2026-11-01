//! The demo's daemon: answers every effect of the TUI's [`App`] at once from
//! [`scene`], so the real app runs with nothing behind it.

use std::collections::VecDeque;

use devlanes_proto::{AttachResult, AttachView, Page};
use devlanes_tui::{
    ApiError,
    app::{App, Effect, Msg},
    logfile::{LogChunk, LogPosition},
    settings,
};
use serde_json::json;

use crate::{live, scene};

/// Messages one step may cause before the fake stops answering, so a loop
/// between the app and the fake cannot hang the page.
const STEP_BUDGET: usize = 2_000;

/// The TUI with the fake daemon behind it.
pub struct Fake {
    pub app: App,
    queue: VecDeque<Msg>,
    /// Seconds the daemon's clock moved on from [`scene::NOW_MS`].
    step: u64,
    /// What the user started, stopped and ran since the scene's lanes.
    changes: live::State,
}

impl Fake {
    /// A TUI of `width` x `height` that subscribed and loaded the lanes.
    pub fn new(width: u16, height: u16) -> Self {
        let mut app = App::new(width, height);
        app.version = devlanes_version::VERSION;
        app.prefer_path(scene::PATH.into());
        let mut fake = Self {
            app,
            queue: VecDeque::new(),
            step: 0,
            changes: live::State::default(),
        };
        for effect in fake.app.start() {
            fake.answer(effect);
        }
        fake.drain();
        fake
    }

    /// Applies `msg` and everything the daemon answers to its effects.
    pub fn send(&mut self, msg: Msg) {
        self.queue.push_back(msg);
        self.drain();
    }

    /// One second later: units, runs and the attached pane move on, then the
    /// app ticks, which refetches metrics on the monitor.
    pub fn tick(&mut self) {
        self.step += 1;
        let msgs = self.changes.tick(self.step, &self.app);
        self.queue.extend(msgs);
        self.send(Msg::Tick);
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

    /// The whole redraw of `unit`'s stream in `lane`: a task's output so
    /// far, else the service's screen.
    fn view(&self, lane: &str, unit: &str, columns: u16, rows: u16) -> AttachView {
        let screen = self
            .changes
            .run_screen(lane, unit)
            .unwrap_or_else(|| scene::screen(lane, unit));
        AttachView {
            stream_id: 1,
            from: 0,
            lines: Vec::new(),
            log_unit: unit.to_owned(),
            before_seq: 60,
            columns,
            rows,
            screen,
            whole: true,
        }
    }

    /// What the daemon sends back for one effect.
    fn answer(&mut self, effect: Effect) {
        if let Some(reply) = self.changes.answer(self.step, &effect) {
            self.queue.extend(reply);
            return;
        }
        // A stopped daemon answers nothing until the TUI starts it again;
        // the TUI's own retries are left unanswered so they do not spin.
        if self.changes.down() {
            let gone = || ApiError::Disconnected("the daemon is stopped".into());
            let reply = match effect {
                Effect::Subscribe => vec![Msg::Subscribed(Err(gone()))],
                Effect::Refresh => vec![Msg::Status(Err(gone()))],
                Effect::FetchDaemon => vec![Msg::DaemonInfo {
                    status: Err(gone()),
                    doctor: Err(gone()),
                }],
                _ => Vec::new(),
            };
            self.queue.extend(reply);
            return;
        }
        // `myapp@0:api` as `api`.
        let unit_name = |reference: &str| {
            reference
                .rsplit_once(':')
                .map_or(reference, |(_, unit)| unit)
                .to_owned()
        };
        let reply = match effect {
            Effect::Subscribe => vec![Msg::Subscribed(Ok(()))],
            Effect::Refresh => vec![Msg::Status(Ok(self.changes.lanes()))],
            Effect::Attach {
                generation,
                lane,
                unit,
                columns,
                rows,
                ..
            } => {
                let view = self.view(&lane, &unit_name(&unit), columns, rows);
                vec![
                    Msg::AttachOpened {
                        generation,
                        result: Ok(AttachResult {
                            stream_id: 1,
                            unit,
                            lane,
                        }),
                    },
                    Msg::AttachView {
                        generation,
                        view: Box::new(view),
                    },
                ]
            }
            // The daemon redraws a unit's stream at its viewer's new size.
            Effect::ResizePty { columns, rows } => match &self.app.attached {
                Some(attached) if !attached.container => vec![Msg::AttachView {
                    generation: attached.generation,
                    view: Box::new(self.view(&attached.lane, &attached.unit, columns, rows)),
                }],
                _ => Vec::new(),
            },
            Effect::FollowContainer {
                generation,
                lane,
                container,
                ..
            } => vec![
                Msg::ContainerLogsOpened {
                    generation,
                    result: Ok(()),
                },
                Msg::AttachOutput {
                    generation,
                    bytes: scene::container_logs(&lane, &container),
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
                    result: Ok(Page::complete(scene::log_page(&unit))),
                }]
            }
            Effect::FetchMetrics(_) => vec![Msg::Metrics(Ok(scene::metrics_at(
                self.step,
                self.changes.process(),
            )))],
            Effect::FetchDaemon => vec![Msg::DaemonInfo {
                status: Ok(scene::daemon_status(
                    self.changes.process(),
                    scene::NOW_MS + self.step * 1_000,
                )),
                doctor: Ok(scene::doctor(self.changes.process())),
            }],
            Effect::ReadDaemonLog { .. } => vec![Msg::DaemonLog(Ok(LogChunk {
                lines: scene::daemon_log(),
                position: LogPosition {
                    offset: 4_096,
                    file: (1, 1),
                },
                reset: true,
            }))],
            Effect::FetchProfiles { lane } => vec![Msg::Profiles {
                result: Ok(scene::profiles(&lane)),
                lane,
            }],
            Effect::FetchTasks { lane } => vec![Msg::Tasks {
                result: Ok(scene::tasks(&lane)),
                lane,
            }],
            Effect::FetchBoxes => vec![Msg::Boxes(Ok(json!({"items": []})))],
            Effect::FetchClients => vec![Msg::Clients(Ok(scene::clients()))],
            Effect::Pair => vec![Msg::Paired(Ok(scene::pair_code()))],
            // The file starts as the scene's config; a save writes its
            // changes over it.
            Effect::Settings(request) => {
                let mut source = scene::USER_CONFIG.to_owned();
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
