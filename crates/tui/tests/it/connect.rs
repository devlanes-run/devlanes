//! Connecting, signing in, pairing and disconnecting from the boxes view:
//! the always-shown `Boxes` button, the new connection form, the prompts of
//! an attached `connect`, the pairing code modal and a command box written
//! to config.yaml.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use devlanes_proto::{
    Event, RpcError,
    remote::{PAIRING_CODE_TTL_MS, PROMPT_EVENT, STATE_EVENT},
};
use devlanes_tui::{
    ApiError, DaemonApi,
    app::{App, Dialog, Effect, Msg, View},
    connect::{PairState, Step},
};
use ratatui::style::Modifier;
use serde_json::{Value, json};

use crate::daemon::{Step as Script, presses, run_keys, shifted};
use crate::errors::{assert_clean, key, render, row, screen};
use crate::fake::{FakeApi, mixed_lanes};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The boxes view with `hosts` as `host.list` items and no clients.
async fn boxes_app(width: u16, height: u16, hosts: Value) -> App {
    let api = FakeApi::with_lanes(mixed_lanes());
    let mut app = App::new(width, height);
    app.update(Msg::Status(api.status().await));
    app.update(Msg::Subscribed(Ok(())));
    app.update(shifted('B'));
    app.update(Msg::Boxes(Ok(json!({"items": hosts}))));
    app.update(Msg::Clients(Ok(json!({"items": [], "now_ms": 0}))));
    app
}

/// `remote.pair`'s reply for `code`, built as the daemon's `pair_method`
/// builds it.
pub fn pair_reply(code: &str) -> Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        });
    json!({
        "code": code,
        "expires_at_ms": now.saturating_add(PAIRING_CODE_TTL_MS),
        "valid_ms": PAIRING_CODE_TTL_MS,
    })
}

/// Types `text` one key at a time.
fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.update(key(KeyCode::Char(c)));
    }
}

/// An event of the attached `connect` `generation`.
fn connect_event(generation: u64, name: &str, data: Value) -> Msg {
    Msg::ConnectEvent {
        generation,
        event: Event {
            seq: 0,
            event: name.into(),
            data,
        },
    }
}

/// The `connect` that `effects` open: its generation and box.
fn opened(effects: &[Effect]) -> Option<(u64, String)> {
    effects.iter().find_map(|effect| match effect {
        Effect::Connect { generation, host } => Some((*generation, host.clone())),
        _ => None,
    })
}

/// The step of the open connect modal.
fn step(app: &App) -> Option<&Step> {
    match &app.dialog {
        Some(Dialog::Connect(dialog)) => Some(&dialog.step),
        _ => None,
    }
}

/// A left click at `(column, row)`.
fn click(column: u16, row: u16) -> Msg {
    Msg::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

/// The notice's text, empty without one.
fn notice(app: &App) -> String {
    app.notice
        .as_ref()
        .map(|n| n.text.clone())
        .unwrap_or_default()
}

/// The view buttons are always there, before any box is known: in full
/// while they fit, then Monitor, Daemon and last Boxes drop to their key
/// letter.
#[tokio::test]
async fn the_header_always_shows_boxes() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let mut app = App::new(120, 40);
    app.update(Msg::Status(api.status().await));
    assert!(app.boxes.hosts.is_empty());
    for (width, buttons) in [
        (120, "Monitor  Daemon  Boxes"),
        (80, "Monitor  Daemon  Boxes"),
        (62, "  M  Daemon  Boxes"),
        (57, "  M  D  Boxes"),
        (50, "  M  D  B"),
    ] {
        app.update(Msg::Resize(width, 24));
        let header = row(&render(&app)?, 0);
        assert!(header.trim_end().ends_with(buttons), "{width}: {header}");
    }
    Ok(())
}

/// `n` opens the form; a name is an SSH destination, so Enter connects at
/// once, writes nothing and follows the link.
#[tokio::test]
async fn a_new_ssh_box_connects_by_its_name() -> TestResult {
    let mut app = boxes_app(100, 30, json!([])).await;
    app.update(key(KeyCode::Char('n')));
    assert!(matches!(step(&app), Some(Step::Form)));
    let effects = app.update(key(KeyCode::Enter));
    assert!(opened(&effects).is_none(), "{effects:?}");
    assert!(screen(&render(&app)?).contains("Type the box's name."));
    // A placeholder is dim and slanted, typed text is neither.
    let style_of = |buffer: &ratatui::buffer::Buffer, needle: &str| {
        (0..buffer.area.height).find_map(|y| {
            let line = row(buffer, y);
            let at = line.find(needle)?;
            let x = u16::try_from(line[..at].chars().count()).ok()?;
            Some(buffer[(x, y)].style())
        })
    };
    let hint = style_of(&render(&app)?, "hetzner or me@").ok_or("no placeholder")?;
    assert!(
        hint.add_modifier.contains(Modifier::DIM | Modifier::ITALIC),
        "{hint:?}"
    );
    type_text(&mut app, "hetzner");
    let buffer = render(&app)?;
    assert_clean(&buffer, "the form");
    let typed = style_of(&buffer, "hetzner▏").ok_or("no typed name")?;
    assert!(
        !typed
            .add_modifier
            .intersects(Modifier::DIM | Modifier::ITALIC),
        "{typed:?}"
    );
    let text = screen(&buffer);
    assert!(text.contains("connect a box"), "{text}");
    assert!(
        text.contains("devlanes runs ssh hetzner, so your SSH config applies."),
        "{text}"
    );
    let effects = app.update(key(KeyCode::Enter));
    assert_eq!(
        opened(&effects).map(|(_, host)| host).as_deref(),
        Some("hetzner")
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::SaveBoxCommand { .. })),
        "{effects:?}"
    );
    assert!(matches!(step(&app), Some(Step::Waiting)));
    assert!(screen(&render(&app)?).contains("connecting…"));
    Ok(())
}

/// The attached `connect` asks for a pairing code, then ssh asks for a
/// passphrase, shown masked; each answer goes back on its stream, and once
/// the link connects the modal closes and lets the `connect` go.
#[tokio::test]
async fn prompts_are_answered_in_the_modal() -> TestResult {
    let mut app = boxes_app(100, 30, json!([])).await;
    app.update(key(KeyCode::Char('n')));
    type_text(&mut app, "hetzner");
    let (generation, _) = opened(&app.update(key(KeyCode::Enter))).ok_or("no connect")?;
    app.update(connect_event(
        generation,
        PROMPT_EVENT,
        json!({"host": "hetzner", "kind": "code", "stream_id": 3,
               "prompt": "pairing code from `devlanes daemon pair` on the box:"}),
    ));
    let text = screen(&render(&app)?);
    assert!(text.contains("hetzner needs a pairing code."), "{text}");
    assert!(
        text.contains("On hetzner, press p in its Boxes view."),
        "{text}"
    );
    assert!(
        text.contains("Or run devlanes daemon pair there."),
        "{text}"
    );
    assert!(!text.contains('`'), "{text}");
    type_text(&mut app, "K7QM-3XWD");
    let effects = app.update(key(KeyCode::Enter));
    assert!(
        effects.iter().any(|e| matches!(e,
            Effect::ConnectAnswer { stream_id: 3, answer } if answer.0 == "K7QM-3XWD\n")),
        "{effects:?}"
    );
    assert!(matches!(step(&app), Some(Step::Waiting)));

    app.update(connect_event(
        generation,
        PROMPT_EVENT,
        json!({"host": "hetzner", "kind": "secret", "stream_id": 4,
               "prompt": "Enter passphrase for key '/home/acme/.ssh/id_ed25519': "}),
    ));
    type_text(&mut app, "hunter2");
    let buffer = render(&app)?;
    assert_clean(&buffer, "a secret prompt");
    let text = screen(&buffer);
    assert!(text.contains("Enter passphrase for key"), "{text}");
    assert!(text.contains("•••••••"), "{text}");
    assert!(!text.contains("hunter2"), "the answer is masked:\n{text}");
    let effects = app.update(key(KeyCode::Enter));
    assert!(
        effects.iter().any(|e| matches!(e,
            Effect::ConnectAnswer { stream_id: 4, answer } if answer.0 == "hunter2\n")),
        "{effects:?}"
    );

    // A notice is answered at once and shown while waiting.
    let effects = app.update(connect_event(
        generation,
        PROMPT_EVENT,
        json!({"host": "hetzner", "kind": "notify", "stream_id": 5,
               "prompt": "Warning: Permanently added 'hetzner' to the list of known hosts."}),
    ));
    assert!(
        effects.iter().any(|e| matches!(e,
            Effect::ConnectAnswer { stream_id: 5, answer } if answer.0 == "\n")),
        "{effects:?}"
    );
    assert!(screen(&render(&app)?).contains("Permanently added"));

    let effects = app.update(connect_event(
        generation,
        STATE_EVENT,
        json!({"host": "hetzner", "state": "connected"}),
    ));
    assert!(app.dialog.is_none());
    assert!(
        effects.iter().any(|e| matches!(e, Effect::ConnectDetach)),
        "{effects:?}"
    );
    assert_eq!(notice(&app), "connected to hetzner");
    Ok(())
}

/// Esc detaches: the connection stays in the daemon, the modal says so, and
/// what the old `connect` sends later changes nothing.
#[tokio::test]
async fn esc_detaches_and_leaves_the_connection_to_the_daemon() -> TestResult {
    let mut app = boxes_app(100, 30, json!([])).await;
    app.update(key(KeyCode::Char('n')));
    type_text(&mut app, "hetzner");
    let (generation, _) = opened(&app.update(key(KeyCode::Enter))).ok_or("no connect")?;
    app.update(connect_event(
        generation,
        STATE_EVENT,
        json!({"host": "hetzner", "state": "reconnecting", "retry_in_ms": 4_000,
               "error": "ssh: connect to host hetzner port 22: Connection refused"}),
    ));
    let text = screen(&render(&app)?);
    assert!(text.contains("next try in 4s"), "{text}");
    assert!(text.contains("Connection refused"), "{text}");
    let effects = app.update(key(KeyCode::Esc));
    assert!(app.dialog.is_none());
    assert!(
        effects.iter().any(|e| matches!(e, Effect::ConnectDetach)),
        "{effects:?}"
    );
    assert_eq!(notice(&app), "hetzner keeps connecting in the daemon");
    app.update(connect_event(
        generation,
        PROMPT_EVENT,
        json!({"host": "hetzner", "kind": "secret", "stream_id": 9, "prompt": "Password:"}),
    ));
    assert!(app.dialog.is_none());
    // Esc on the form only closes it.
    app.update(key(KeyCode::Char('n')));
    let effects = app.update(key(KeyCode::Esc));
    assert!(app.dialog.is_none());
    assert!(effects.is_empty(), "{effects:?}");
    Ok(())
}

/// Enter acts on the selected box by its state: a box waiting for a sign-in
/// or a pairing code opens the same flow, a connected one asks before it
/// disconnects; `d` on a box that is not connected says so.
#[tokio::test]
async fn enter_acts_on_the_box_by_its_state() -> TestResult {
    let hosts = json!([
        {"host": "hetzner", "state": "connected", "transport": "ssh"},
        {"host": "gpu", "state": "needs_pairing", "transport": "ssh"},
        {"host": "old", "state": "disconnected", "transport": "ssh"},
    ]);
    let mut app = boxes_app(100, 30, hosts).await;
    app.update(key(KeyCode::Enter));
    match &app.dialog {
        Some(Dialog::Confirm {
            title,
            prompt,
            detail,
            ..
        }) => {
            assert_eq!(title, "disconnect hetzner");
            assert_eq!(prompt, "Its ports and hostnames stop working here.");
            assert_eq!(detail, &["Its lanes keep running on the box.".to_owned()]);
        }
        other => return Err(format!("no confirm dialog: {other:?}").into()),
    }
    assert!(
        app.update(key(KeyCode::Enter)).is_empty(),
        "enter confirms nothing"
    );
    app.update(key(KeyCode::Enter));
    let effects = app.update(key(KeyCode::Char('y')));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Disconnect { host } if host == "hetzner")),
        "{effects:?}"
    );
    app.update(Msg::Disconnected {
        host: "hetzner".into(),
        result: Ok(json!({"disconnected": ["hetzner"]})),
    });
    assert_eq!(notice(&app), "disconnected hetzner");

    app.update(key(KeyCode::Char('j')));
    let effects = app.update(key(KeyCode::Enter));
    assert_eq!(
        opened(&effects).map(|(_, host)| host).as_deref(),
        Some("gpu")
    );
    assert!(matches!(step(&app), Some(Step::Waiting)));
    app.update(key(KeyCode::Esc));

    app.update(key(KeyCode::Char('j')));
    app.update(key(KeyCode::Char('d')));
    assert!(app.dialog.is_none());
    assert_eq!(notice(&app), "old is not connected");
    Ok(())
}

/// A box that turns this machine away shows its reason in plain words in
/// the modal, and Enter tries again.
#[tokio::test]
async fn a_refusal_shows_in_the_modal() -> TestResult {
    let mut app = boxes_app(100, 30, json!([])).await;
    app.update(key(KeyCode::Char('n')));
    type_text(&mut app, "hetzner");
    let (generation, _) = opened(&app.update(key(KeyCode::Enter))).ok_or("no connect")?;
    app.update(Msg::ConnectDone {
        generation,
        result: Err(ApiError::Rpc(RpcError::new(
            "remote_failed",
            json!({"host": "hetzner",
                   "message": "the box does not accept connections. Set remote.allow_connections to true there"}),
            4,
        ))),
    });
    let buffer = render(&app)?;
    assert_clean(&buffer, "a refused connection");
    let text = screen(&buffer);
    assert!(
        text.contains("hetzner does not accept connections."),
        "{text}"
    );
    assert!(
        text.contains("remote.allow_connections to true there"),
        "{text}"
    );
    let effects = app.update(key(KeyCode::Enter));
    let (again, host) = opened(&effects).ok_or("no second connect")?;
    assert_eq!(host, "hetzner");
    assert!(again > generation);
    // The old `connect`'s end is stale now.
    app.update(Msg::ConnectDone {
        generation,
        result: Err(ApiError::Rpc(RpcError::new("remote_failed", json!({}), 4))),
    });
    assert!(matches!(step(&app), Some(Step::Waiting)));
    Ok(())
}

/// `p` asks for a code and shows it with its countdown and the command for
/// the other machine; a pause and connections being off have words of their
/// own.
#[tokio::test]
async fn the_pairing_code_modal() -> TestResult {
    let mut app = boxes_app(100, 30, json!([])).await;
    let effects = app.update(key(KeyCode::Char('p')));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::Pair)),
        "{effects:?}"
    );
    app.update(Msg::Paired(Ok(pair_reply("K7QM-3XWD"))));
    let buffer = render(&app)?;
    assert_clean(&buffer, "the pairing code");
    // The code stands alone on its row, centered in the dialog, bold and in
    // the attention color; without color it is bold only.
    let code_style =
        |app: &App| -> Result<(u16, ratatui::style::Style), Box<dyn std::error::Error>> {
            let buffer = render(app)?;
            let area = app.dialog_area.borrow().ok_or("no dialog")?;
            let y = (area.y..area.bottom())
                .find(|y| {
                    row(&buffer, *y).contains("K7QM-3XWD") && !row(&buffer, *y).contains("--code")
                })
                .ok_or("no code row")?;
            let line = row(&buffer, y);
            let at = line.find("K7QM-3XWD").ok_or("no code")?;
            let x = u16::try_from(line[..at].chars().count())?;
            let left = x - area.x - 1;
            let right = area.right() - 1 - (x + 9);
            assert!(
                left.abs_diff(right) <= 1,
                "not centered: {left} and {right}\n{line}"
            );
            Ok((x, buffer[(x, y)].style()))
        };
    let (_, style) = code_style(&app)?;
    assert_eq!(style.fg, Some(ratatui::style::Color::Yellow), "{style:?}");
    assert!(style.add_modifier.contains(Modifier::BOLD), "{style:?}");
    app.theme.tier = devlanes_tui::theme::Tier::Plain;
    let (_, plain) = code_style(&app)?;
    assert!(
        matches!(plain.fg, None | Some(ratatui::style::Color::Reset)),
        "{plain:?}"
    );
    assert!(plain.add_modifier.contains(Modifier::BOLD), "{plain:?}");
    app.theme.tier = devlanes_tui::theme::Tier::Ansi;
    let text = screen(&buffer);
    for expected in [
        "K7QM-3XWD",
        "It works once and expires in 10m.",
        "devlanes remote connect <box> --code K7QM-3XWD",
    ] {
        assert!(text.contains(expected), "missing {expected:?}:\n{text}");
    }
    app.update(Msg::Tick);
    app.update(Msg::Tick);
    assert!(screen(&render(&app)?).contains("expires in 9m 58s"));
    let effects = app.update(key(KeyCode::Char('y')));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Copy(text) if text == "K7QM-3XWD")),
        "{effects:?}"
    );
    // At 10 minutes the code is gone and `y` copies nothing.
    for _ in 0..598 {
        app.update(Msg::Tick);
    }
    let text = screen(&render(&app)?);
    assert!(text.contains("This code expired."), "{text}");
    assert!(!text.contains("K7QM-3XWD"), "{text}");
    let effects = app.update(key(KeyCode::Char('y')));
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Copy(_))),
        "{effects:?}"
    );
    // A reply without `valid_ms` still counts down from 10 minutes.
    app.update(key(KeyCode::Char('p')));
    app.update(Msg::Paired(Ok(json!({"code": "K7QM-3XWD"}))));
    app.update(Msg::Tick);
    assert!(screen(&render(&app)?).contains("expires in 9m 59s"));

    let paused = RpcError::new(
        "pairing_paused",
        json!({"message": "the box paused pairing", "retry_after_ms": 252_000}),
        4,
    );
    app.update(key(KeyCode::Char('p')));
    app.update(Msg::Paired(Err(ApiError::Rpc(paused))));
    assert!(matches!(
        &app.dialog,
        Some(Dialog::Pair(dialog)) if matches!(dialog.state, PairState::Paused { .. })
    ));
    let text = screen(&render(&app)?);
    assert!(
        text.contains("Pairing is paused after 5 wrong codes."),
        "{text}"
    );
    assert!(text.contains("again in 4m 12s"), "{text}");

    let off = RpcError::new("connections_off", json!({"invalid": false}), 4);
    app.update(key(KeyCode::Char('p')));
    app.update(Msg::Paired(Err(ApiError::Rpc(off))));
    let text = screen(&render(&app)?);
    assert!(
        text.contains("Connections to this machine are off."),
        "{text}"
    );
    assert!(
        text.contains("Turn on remote.allow_connections to pair another machine."),
        "{text}"
    );
    app.update(key(KeyCode::Esc));
    assert!(app.dialog.is_none());
    assert_eq!(app.view, View::Boxes);
    Ok(())
}

/// `o` in the pair-off modal and in the view opens the options on
/// `remote.allow_connections`, and closing them asks for the clients again,
/// whose note then says whether connections are on.
#[tokio::test]
async fn o_opens_the_options_on_allow_connections() -> TestResult {
    let mut app = boxes_app(100, 30, json!([])).await;
    app.update(Msg::Clients(Ok(json!({"items": [], "now_ms": 0,
        "require_pairing": true, "allow_connections": false}))));
    let text = screen(&render(&app)?);
    assert!(
        text.contains("Connections to this machine are off. Press o to turn them on."),
        "{text}"
    );
    assert!(row(&render(&app)?, 29).contains("o options"), "the footer");
    app.update(key(KeyCode::Char('p')));
    let off = RpcError::new("connections_off", json!({"invalid": false}), 4);
    app.update(Msg::Paired(Err(ApiError::Rpc(off))));
    assert!(screen(&render(&app)?).contains("o options"));
    app.update(key(KeyCode::Char('o')));
    match &app.dialog {
        Some(Dialog::Settings(settings)) => assert_eq!(
            settings.selected().map(|item| item.key.as_str()),
            Some("remote.allow_connections")
        ),
        other => return Err(format!("no options page: {other:?}").into()),
    }
    // Esc leaves the options for the menu.
    let effects = app.update(key(KeyCode::Esc));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::FetchClients)),
        "{effects:?}"
    );
    app.update(key(KeyCode::Esc));
    assert!(app.dialog.is_none());

    // Invalid remote settings say so instead.
    app.update(Msg::Clients(Ok(json!({"items": [], "now_ms": 0,
        "require_pairing": true, "allow_connections": false, "remote_invalid": true}))));
    let text = screen(&render(&app)?);
    assert!(
        text.contains("because config.yaml has invalid remote settings"),
        "{text}"
    );
    assert!(text.contains("doctor to see them."), "{text}");
    app.update(key(KeyCode::Char('o')));
    assert!(matches!(app.dialog, Some(Dialog::Settings(_))));
    Ok(())
}

/// While the command is saved Esc and a click outside do nothing, and a
/// save that fails shows why under the form, which stays as typed.
#[tokio::test]
async fn saving_a_command_holds_the_modal_and_reports_a_failure() -> TestResult {
    let mut app = boxes_app(100, 30, json!([])).await;
    app.update(key(KeyCode::Char('n')));
    type_text(&mut app, "acme");
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::BackTab,
        KeyModifiers::SHIFT,
    )));
    app.update(key(KeyCode::Char(' ')));
    app.update(key(KeyCode::Tab));
    app.update(key(KeyCode::Tab));
    type_text(&mut app, "docker exec -i acme devlanes tunnel --stdio");
    let effects = app.update(key(KeyCode::Enter));
    let generation = effects
        .iter()
        .find_map(|e| match e {
            Effect::SaveBoxCommand { generation, .. } => Some(*generation),
            _ => None,
        })
        .ok_or("no save")?;
    assert!(matches!(step(&app), Some(Step::Saving)));
    let effects = app.update(key(KeyCode::Esc));
    assert!(effects.is_empty(), "{effects:?}");
    assert!(matches!(step(&app), Some(Step::Saving)));
    assert_eq!(notice(&app), "");
    app.update(click(0, 0));
    assert!(matches!(step(&app), Some(Step::Saving)), "a click outside");
    app.update(Msg::BoxSaved {
        generation,
        result: Err("config.yaml is not valid YAML".into()),
    });
    assert!(matches!(step(&app), Some(Step::Form)));
    let text = screen(&render(&app)?);
    assert!(
        text.contains("config.yaml was not changed: config.yaml is not valid YAML"),
        "{text}"
    );
    match &app.dialog {
        Some(Dialog::Connect(dialog)) => {
            assert_eq!(
                dialog.form.command,
                "docker exec -i acme devlanes tunnel --stdio"
            );
        }
        other => return Err(format!("no form: {other:?}").into()),
    }
    Ok(())
}

/// The form reads the commands config.yaml has: a new command for a box
/// that has one asks first, a connected box keeps its command, and an SSH
/// name with a command says it runs that.
#[tokio::test]
async fn the_form_knows_the_configured_boxes() -> TestResult {
    let hosts = json!([{"host": "live", "state": "connected", "transport": "command"}]);
    let mut app = boxes_app(100, 30, hosts).await;
    let effects = app.update(key(KeyCode::Char('n')));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::LoadBoxCommands)),
        "{effects:?}"
    );
    let docker = |name: &str| {
        (
            name.to_owned(),
            ["docker", "exec", "-i", name].map(String::from).to_vec(),
        )
    };
    app.update(Msg::BoxCommands([docker("acme"), docker("live")].into()));
    type_text(&mut app, "acme");
    let text = screen(&render(&app)?);
    assert!(
        text.contains("acme has a command in config.yaml, so devlanes runs that"),
        "{text}"
    );
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::BackTab,
        KeyModifiers::SHIFT,
    )));
    app.update(key(KeyCode::Char(' ')));
    app.update(key(KeyCode::Tab));
    app.update(key(KeyCode::Tab));
    type_text(&mut app, "wsl.exe -d Ubuntu");
    let effects = app.update(key(KeyCode::Enter));
    assert!(effects.is_empty(), "{effects:?}");
    let text = screen(&render(&app)?);
    assert!(
        text.contains("config.yaml already runs docker exec -i acme for acme."),
        "{text}"
    );
    let effects = app.update(key(KeyCode::Enter));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::SaveBoxCommand { host, .. } if host == "acme")),
        "{effects:?}"
    );

    let hosts = json!([{"host": "live", "state": "connected", "transport": "command"}]);
    let mut app = boxes_app(100, 30, hosts).await;
    app.update(key(KeyCode::Char('n')));
    app.update(Msg::BoxCommands([docker("live")].into()));
    type_text(&mut app, "live");
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::BackTab,
        KeyModifiers::SHIFT,
    )));
    app.update(key(KeyCode::Char(' ')));
    app.update(key(KeyCode::Tab));
    app.update(key(KeyCode::Tab));
    type_text(&mut app, "wsl.exe -d Ubuntu");
    let effects = app.update(key(KeyCode::Enter));
    assert!(effects.is_empty(), "{effects:?}");
    let text = screen(&render(&app)?);
    assert!(
        text.contains("live is connected now. Press d on it to disconnect it first."),
        "{text}"
    );
    Ok(())
}

/// A click outside the modal while it asks something leaves the question
/// open; Esc still closes it.
#[tokio::test]
async fn a_click_outside_keeps_the_question() -> TestResult {
    let mut app = boxes_app(100, 30, json!([])).await;
    app.update(key(KeyCode::Char('n')));
    type_text(&mut app, "hetzner");
    let (generation, _) = opened(&app.update(key(KeyCode::Enter))).ok_or("no connect")?;
    app.update(connect_event(
        generation,
        PROMPT_EVENT,
        json!({"host": "hetzner", "kind": "secret", "stream_id": 4, "prompt": "Password:"}),
    ));
    render(&app)?;
    let effects = app.update(click(0, 0));
    assert!(effects.is_empty(), "{effects:?}");
    assert!(matches!(step(&app), Some(Step::Prompt(_))));
    let effects = app.update(key(KeyCode::Esc));
    assert!(app.dialog.is_none());
    assert!(
        effects.iter().any(|e| matches!(e, Effect::ConnectDetach)),
        "{effects:?}"
    );
    Ok(())
}

/// The whole loop with the real executor: a box reached by a command is
/// saved to config.yaml in the devlanes home before it connects.
#[tokio::test]
async fn drive_saves_a_command_box_to_the_user_config() -> TestResult {
    let home = tempfile::tempdir()?;
    let mut fake = FakeApi::with_lanes(mixed_lanes());
    fake.home = Some(home.path().to_path_buf());
    let api = Arc::new(fake);
    let mut keys = vec![shifted('B'), key(KeyCode::Char('n'))];
    keys.extend("acme".chars().map(|c| key(KeyCode::Char(c))));
    keys.push(Msg::Key(KeyEvent::new(
        KeyCode::BackTab,
        KeyModifiers::SHIFT,
    )));
    keys.push(key(KeyCode::Char(' ')));
    keys.push(key(KeyCode::Tab));
    keys.push(key(KeyCode::Tab));
    keys.extend(
        "docker exec -i acme devlanes tunnel --stdio"
            .chars()
            .map(|c| key(KeyCode::Char(c))),
    );
    keys.push(key(KeyCode::Enter));
    run_keys(
        Arc::clone(&api),
        presses(keys),
        &["connected to acme"],
        |calls| calls.iter().any(|c| c == "connect acme"),
    )
    .await?;
    let config = std::fs::read_to_string(home.path().join("config.yaml"))?;
    let parsed = devlanes_config::parse_user(&config)?;
    assert_eq!(
        parsed.remote.boxes["acme"].command,
        [
            "docker", "exec", "-i", "acme", "devlanes", "tunnel", "--stdio"
        ]
    );
    Ok(())
}

/// The whole loop with the real executor: `connect` is sent, and the
/// pairing code and the passphrase typed in the modal reach the daemon on
/// their streams.
#[tokio::test]
async fn drive_answers_the_code_and_the_passphrase() -> TestResult {
    let fake = FakeApi::with_lanes(mixed_lanes());
    *fake.prompts.lock().map_err(|e| e.to_string())? = vec![
        (
            "code".into(),
            "pairing code from `devlanes daemon pair` on the box:".into(),
        ),
        ("secret".into(), "Enter passphrase for key:".into()),
    ];
    let api = Arc::new(fake);
    let typed = |text: &str| -> Vec<Msg> { text.chars().map(|c| key(KeyCode::Char(c))).collect() };
    let mut steps = presses(vec![shifted('B'), key(KeyCode::Char('n'))]);
    steps.extend(presses(typed("hetzner")));
    steps.extend(presses(vec![key(KeyCode::Enter)]));
    steps.push(Script::Screen("needs a pairing code"));
    steps.extend(presses(typed("K7QM-3XWD")));
    steps.extend(presses(vec![key(KeyCode::Enter)]));
    steps.push(Script::Screen("Enter passphrase for key:"));
    steps.extend(presses(typed("hunter2")));
    steps.extend(presses(vec![key(KeyCode::Enter)]));
    run_keys(
        Arc::clone(&api),
        steps,
        &["connected to hetzner"],
        |calls| calls.iter().any(|c| c == "answer 4 hunter2"),
    )
    .await?;
    let calls = api.calls();
    let order: Vec<&String> = calls
        .iter()
        .filter(|c| c.starts_with("connect ") || c.starts_with("answer "))
        .collect();
    assert_eq!(
        order,
        ["connect hetzner", "answer 3 K7QM-3XWD", "answer 4 hunter2"],
        "{calls:?}"
    );
    Ok(())
}
