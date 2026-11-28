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
    app.update(Msg::Subscribed(Ok(Vec::new())));
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

/// The keys from a new form, which opens on the ticket row, to the name row
/// of SSH name.
fn ssh_keys() -> Vec<Msg> {
    vec![
        Msg::Key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
        key(KeyCode::Char(' ')),
        key(KeyCode::Tab),
    ]
}

/// `n`, then SSH name at "Reach by", with the name row taking keys.
fn new_ssh_box(app: &mut App) -> Vec<Effect> {
    let effects = app.update(key(KeyCode::Char('n')));
    for msg in ssh_keys() {
        app.update(msg);
    }
    effects
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

/// The open pairing modal's generation, 0 without one.
fn pair_generation(app: &App) -> u64 {
    match &app.dialog {
        Some(Dialog::Pair(dialog)) => dialog.generation,
        _ => 0,
    }
}

/// The answer to the open pairing modal's `remote.pair`.
fn paired(app: &App, result: Result<Value, ApiError>) -> Msg {
    Msg::Paired {
        generation: pair_generation(app),
        result,
    }
}

/// Moves the app's clock past the wait after a pairing answer.
fn after_a_while(app: &mut App) {
    app.clock = Some(app.now() + devlanes_tui::connect::PAIR_AGAIN_AFTER);
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

/// A ticket-shaped test string, `dlp1` and 98 base32 letters and digits.
fn test_ticket() -> String {
    devlanes_transport::Ticket {
        addr: devlanes_transport::PeerAddr {
            id: devlanes_transport::PeerId([7; 32]),
            relay: Some("https://relay.example.invalid".into()),
            direct: vec![std::net::SocketAddr::from(([192, 0, 2, 4], 4242))],
        },
        secret: devlanes_transport::TicketSecret([9; 16]),
        ephemeral: false,
    }
    .encode()
}

/// The ticket the connect form sends, with the name: `None` when nothing
/// was sent.
fn sent_ticket(effects: &[Effect]) -> Option<(String, Option<String>)> {
    effects.iter().find_map(|effect| match effect {
        Effect::ConnectTicket { ticket, name, .. } => {
            Some((ticket.expose().to_owned(), name.clone()))
        }
        _ => None,
    })
}

/// The form opens on the ticket row. A ticket copied from a wrapped
/// terminal or from Markdown is sent whole, without the line breaks,
/// spaces, quotes and backticks around and inside it, with the name when
/// one was typed; once sent it is no longer on screen. One cut short or
/// that is not a ticket is refused without being repeated.
#[tokio::test]
async fn the_form_sends_a_pasted_ticket() -> TestResult {
    let ticket = test_ticket();
    let (head, tail) = ticket.split_at(50);
    let pasted = format!(" `{head}\n  {tail}`\n");
    for (name, sent_name) in [("", None), ("acme", Some("acme"))] {
        let mut app = boxes_app(100, 30, json!([])).await;
        app.update(key(KeyCode::Char('n')));
        app.update(Msg::Paste(pasted.clone()));
        let text = screen(&render(&app)?);
        assert!(text.contains("dlp1… "), "{text}");
        assert!(!text.contains(&ticket[..20]), "{text}");
        app.update(key(KeyCode::Tab));
        type_text(&mut app, name);
        let effects = app.update(key(KeyCode::Enter));
        assert_eq!(
            sent_ticket(&effects),
            Some((ticket.clone(), sent_name.map(str::to_owned))),
            "{effects:?}"
        );
        assert!(opened(&effects).is_none(), "{effects:?}");
        let text = screen(&render(&app)?);
        assert!(
            text.contains("pairing with the box in the ticket"),
            "{text}"
        );
        assert!(!text.contains("dlp1"), "{text}");
    }
    let cut = &ticket[..50];
    let other = if ticket.ends_with('a') { 'b' } else { 'a' };
    let changed = format!("{}{other}", &ticket[..ticket.len() - 1]);
    for (typed, refusal) in [
        (cut, "This ticket is incomplete or damaged."),
        (changed.as_str(), "This ticket is incomplete or damaged."),
        (
            "dlp1box",
            "This starts like a pairing ticket but is too short for one.",
        ),
    ] {
        let mut app = boxes_app(100, 30, json!([])).await;
        app.update(key(KeyCode::Char('n')));
        type_text(&mut app, typed);
        let effects = app.update(key(KeyCode::Enter));
        assert_eq!(sent_ticket(&effects), None, "{effects:?}");
        let text = screen(&render(&app)?);
        assert!(text.contains(refusal), "{text}");
        assert!(
            !text.contains(&typed[4..20.min(typed.len())]) || typed.len() < 20,
            "{text}"
        );
    }
    // A typed SSH name moves to its row, and Enter then connects over SSH.
    let mut app = boxes_app(100, 30, json!([])).await;
    app.update(key(KeyCode::Char('n')));
    type_text(&mut app, "hetzner");
    let effects = app.update(key(KeyCode::Enter));
    assert!(effects.is_empty(), "{effects:?}");
    assert!(
        screen(&render(&app)?).contains("That is not a ticket, so it is now the SSH name."),
        "{}",
        screen(&render(&app)?)
    );
    let effects = app.update(key(KeyCode::Enter));
    assert_eq!(
        opened(&effects).map(|(_, host)| host).as_deref(),
        Some("hetzner")
    );
    // A name the daemon would refuse is refused here, before the ticket goes.
    let mut app = boxes_app(100, 30, json!([])).await;
    app.update(key(KeyCode::Char('n')));
    app.update(Msg::Paste(ticket.clone()));
    app.update(key(KeyCode::Tab));
    type_text(&mut app, "my box");
    let effects = app.update(key(KeyCode::Enter));
    assert_eq!(sent_ticket(&effects), None, "{effects:?}");
    assert!(screen(&render(&app)?).contains("A box name is 1 to 63 letters"));
    Ok(())
}

/// The keys of a ticket as Windows delivers a paste: the first as a key, the
/// rest as keys of the paste. Copied with `y` it is the ticket alone, which
/// starts with `d`; copied with the mouse from the pair dialog it is three
/// bordered lines, each line break an Enter.
fn pasted_keys(ticket: &str, bordered: bool) -> Vec<Msg> {
    let third = ticket.len() / 3;
    let text = if bordered {
        [
            &ticket[..third],
            &ticket[third..2 * third],
            &ticket[2 * third..],
        ]
        .map(|line| format!("│ {line} │"))
        .join("\r")
    } else {
        format!("{ticket}\r")
    };
    text.chars()
        .enumerate()
        .map(|(index, c)| {
            let code = if c == '\r' {
                KeyCode::Enter
            } else {
                KeyCode::Char(c)
            };
            let key = KeyEvent::new(code, KeyModifiers::NONE);
            if index == 0 {
                Msg::Key(key)
            } else {
                Msg::PastedKey(key)
            }
        })
        .collect()
}

/// Effects that act on a box, a client or pairing, which a paste must
/// never produce.
fn acts(effects: &[Effect]) -> Vec<&Effect> {
    effects
        .iter()
        .filter(|e| {
            matches!(
                e,
                Effect::Disconnect { .. }
                    | Effect::Forget { .. }
                    | Effect::RevokeClient { .. }
                    | Effect::Pair { .. }
                    | Effect::MintTicket { .. }
                    | Effect::Connect { .. }
                    | Effect::ConnectTicket { .. }
            )
        })
        .collect()
}

/// A ticket pasted as key presses into the bare Boxes view, with a connected
/// box selected and a box paired by ticket listed, opens at most what its
/// first letter opens, the disconnect question for `d`, and its other
/// letters (`y`, `n`, `p`, `f`, `x` and digits among them) answer nothing
/// and run no binding. The same paste into an open question confirms
/// nothing.
#[tokio::test]
async fn a_paste_as_key_presses_runs_no_bindings() -> TestResult {
    let hosts = json!([
        {"host": "hetzner", "state": "connected", "transport": "ssh"},
        {"host": "studio", "state": "failed", "transport": "p2p"},
    ]);
    let ticket = test_ticket();
    let mut app = boxes_app(100, 30, hosts.clone()).await;
    let mut effects = Vec::new();
    for msg in pasted_keys(&ticket, false) {
        effects.extend(app.update(msg));
    }
    assert!(acts(&effects).is_empty(), "{effects:?}");
    // The question its `d` opened closes again, and the notice says why.
    assert!(app.dialog.is_none(), "{:?}", app.dialog);
    assert_eq!(
        notice(&app),
        "Pasted text goes only into a text field. Press n, then paste the ticket."
    );
    assert_eq!(app.view, View::Boxes);

    let mut app = boxes_app(100, 30, hosts).await;
    app.update(key(KeyCode::Char('j')));
    app.update(key(KeyCode::Char('f')));
    let mut effects = Vec::new();
    for msg in pasted_keys(&ticket, false) {
        effects.extend(app.update(msg));
    }
    assert!(acts(&effects).is_empty(), "{effects:?}");
    assert!(
        matches!(&app.dialog, Some(Dialog::Confirm { prompt, .. }) if prompt == "Forget studio?")
    );
    // Keys pressed one by one still answer it.
    let effects = app.update(key(KeyCode::Char('y')));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Forget { host } if host == "studio")),
        "{effects:?}"
    );
    Ok(())
}

/// Where pastes arrive as keys, a question takes `y` only once it has been
/// on screen a third of a second and the key before came that long before,
/// since a paste read in pieces starts each piece with a key that reads as
/// typed: a `y` 50 ms after the `d`, `f` or `x` that opened the question
/// leaves it open, a `y` after the wait answers it. Where pastes are paste
/// events, the first `y` answers.
#[tokio::test]
async fn a_question_waits_before_it_takes_an_answer() -> TestResult {
    let hosts = json!([
        {"host": "hetzner", "state": "connected", "transport": "ssh"},
        {"host": "studio", "state": "connected", "transport": "p2p"},
    ]);
    let client = json!({"items": [{
        "id": format!("dl_cid_{}", "3f".repeat(32)), "name": "laptop", "paired": true,
        "pairings": 1, "connections": [],
    }], "now_ms": 0});
    let wait = |app: &mut App, ms: u64| {
        app.clock = Some(app.now() + std::time::Duration::from_millis(ms));
    };
    for pastes_are_keys in [true, false] {
        // The key that opens each question, after the moves that select its row.
        for (moves, opener, answered) in [
            (0, 'd', "disconnect"),
            (1, 'f', "forget"),
            (2, 'x', "revoke"),
        ] {
            let mut app = boxes_app(100, 30, hosts.clone()).await;
            app.update(Msg::Clients(Ok(client.clone())));
            app.clock = Some(app.now());
            app.paste.on = pastes_are_keys;
            for _ in 0..moves {
                app.update(key(KeyCode::Char('j')));
                wait(&mut app, 500);
            }
            app.update(key(KeyCode::Char(opener)));
            assert!(
                matches!(app.dialog, Some(Dialog::Confirm { .. })),
                "{opener}: {:?}",
                app.dialog
            );
            render(&app)?;
            wait(&mut app, 50);
            let first = app.update(key(KeyCode::Char('y')));
            let acted = |effects: &[Effect]| {
                effects.iter().any(|e| match e {
                    Effect::Disconnect { .. } => answered == "disconnect",
                    Effect::Forget { .. } => answered == "forget",
                    Effect::RevokeClient { .. } => answered == "revoke",
                    _ => false,
                })
            };
            if pastes_are_keys {
                assert!(!acted(&first), "{opener}: {first:?}");
                assert!(matches!(app.dialog, Some(Dialog::Confirm { .. })));
                wait(&mut app, 400);
                let second = app.update(key(KeyCode::Char('y')));
                assert!(acted(&second), "{opener}: {second:?}");
            } else {
                assert!(acted(&first), "{opener}: {first:?}");
            }
        }
    }
    Ok(())
}

/// The same paste into the ticket row is one text: its line breaks are
/// dropped instead of submitting, the row is masked and says the ticket is
/// whole, and only an Enter pressed afterwards sends it, once.
#[tokio::test]
async fn a_ticket_pasted_as_key_presses_waits_for_enter() -> TestResult {
    let ticket = test_ticket();
    let mut app = boxes_app(100, 30, json!([])).await;
    app.update(key(KeyCode::Char('n')));
    let mut effects = Vec::new();
    for msg in pasted_keys(&ticket, true) {
        effects.extend(app.update(msg));
    }
    assert!(acts(&effects).is_empty(), "{effects:?}");
    assert!(matches!(step(&app), Some(Step::Form)));
    let text = screen(&render(&app)?);
    assert!(text.contains("Press enter to pair."), "{text}");
    assert!(!text.contains(&ticket[..20]), "{text}");
    let effects = app.update(key(KeyCode::Enter));
    assert_eq!(sent_ticket(&effects), Some((ticket.clone(), None)));

    // Half of one, as a person may press Enter on, says it is not whole.
    let mut app = boxes_app(100, 30, json!([])).await;
    app.update(key(KeyCode::Char('n')));
    type_text(&mut app, &ticket[..ticket.len() / 2]);
    assert!(screen(&render(&app)?).contains("This ticket is incomplete or damaged."));
    let effects = app.update(key(KeyCode::Enter));
    assert_eq!(sent_ticket(&effects), None, "{effects:?}");

    let lines = [&ticket[..40], &ticket[40..80], &ticket[80..]];
    let mut app = boxes_app(100, 30, json!([])).await;
    app.update(key(KeyCode::Char('n')));
    let bordered: Vec<String> = lines.iter().map(|line| format!("│ {line} │")).collect();
    app.update(Msg::Paste(bordered.join("\n")));
    let effects = app.update(key(KeyCode::Enter));
    assert_eq!(sent_ticket(&effects), Some((ticket.clone(), None)));
    Ok(())
}

/// A ticket typed into the SSH name or the command row is masked there,
/// never in the title, and Enter moves it to the ticket row instead of
/// sending it as a host.
#[tokio::test]
async fn a_ticket_in_another_row_moves_to_the_ticket_row() -> TestResult {
    let ticket = test_ticket();
    for command in [false, true] {
        let mut app = boxes_app(100, 30, json!([])).await;
        new_ssh_box(&mut app);
        if command {
            type_text(&mut app, "acme");
            app.update(Msg::Key(KeyEvent::new(
                KeyCode::BackTab,
                KeyModifiers::SHIFT,
            )));
            app.update(key(KeyCode::Char(' ')));
            app.update(key(KeyCode::Tab));
            app.update(key(KeyCode::Tab));
        }
        type_text(&mut app, &ticket);
        let text = screen(&render(&app)?);
        assert!(!text.contains(&ticket[..20]), "{text}");
        let effects = app.update(key(KeyCode::Enter));
        assert!(opened(&effects).is_none(), "{effects:?}");
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::SaveBoxCommand { .. })),
            "{effects:?}"
        );
        let text = screen(&render(&app)?);
        assert!(text.contains("it is now in the Ticket row"), "{text}");
        assert!(!text.contains(&ticket[..20]), "{text}");
        let effects = app.update(key(KeyCode::Enter));
        let name = command.then(|| "acme".to_owned());
        assert_eq!(sent_ticket(&effects), Some((ticket.clone(), name)));
    }
    Ok(())
}

/// A ticket pasted on the SSH name row goes to the ticket row, so it is
/// never shown or sent as a host; the first state names the box, and the
/// notice says which name this machine keeps it under.
#[tokio::test]
async fn a_pairing_names_the_box_from_its_first_state() -> TestResult {
    let mut app = boxes_app(100, 30, json!([])).await;
    new_ssh_box(&mut app);
    app.update(Msg::Paste(test_ticket()));
    match &app.dialog {
        Some(Dialog::Connect(dialog)) => assert!(dialog.form.name.is_empty()),
        other => return Err(format!("no form: {other:?}").into()),
    }
    let effects = app.update(key(KeyCode::Enter));
    let generation = effects
        .iter()
        .find_map(|e| match e {
            Effect::ConnectTicket { generation, .. } => Some(*generation),
            _ => None,
        })
        .ok_or("no pairing")?;
    let paired = json!({"name": "acme", "kept": true, "name_ignored": false});
    app.update(connect_event(
        generation,
        STATE_EVENT,
        json!({"host": "acme", "state": "connecting", "attempt": 1, "paired": paired}),
    ));
    let text = screen(&render(&app)?);
    assert!(text.contains("connect acme"), "{text}");
    assert!(
        text.contains("paired with the box before and keeps its name"),
        "{text}"
    );
    app.update(connect_event(
        generation,
        STATE_EVENT,
        json!({"host": "acme", "state": "connected"}),
    ));
    assert!(app.dialog.is_none());
    assert_eq!(
        notice(&app),
        "connected to acme, its name from an earlier pairing"
    );

    // A pairing that fails goes back to the form for the ticket again.
    app.update(key(KeyCode::Char('n')));
    app.update(Msg::Paste(test_ticket()));
    let effects = app.update(key(KeyCode::Enter));
    let generation = effects
        .iter()
        .find_map(|e| match e {
            Effect::ConnectTicket { generation, .. } => Some(*generation),
            _ => None,
        })
        .ok_or("no pairing")?;
    app.update(Msg::ConnectDone {
        generation,
        result: Err(ApiError::Rpc(RpcError::new(
            "remote_failed",
            json!({"host": "p2p:1a2b3c4d", "pairing": true,
                   "message": "the box in the ticket did not answer. Check that its devlanes daemon is running"}),
            4,
        ))),
    });
    let text = screen(&render(&app)?);
    assert!(
        text.contains("the box in the ticket did not answer."),
        "{text}"
    );
    assert!(text.contains("paste again"), "{text}");
    app.update(key(KeyCode::Enter));
    assert!(matches!(step(&app), Some(Step::Form)));
    Ok(())
}

/// `n` opens the form; a name is an SSH destination, so Enter connects at
/// once, writes nothing and follows the link.
#[tokio::test]
async fn a_new_ssh_box_connects_by_its_name() -> TestResult {
    let mut app = boxes_app(100, 30, json!([])).await;
    new_ssh_box(&mut app);
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
    new_ssh_box(&mut app);
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
    new_ssh_box(&mut app);
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

/// A box paired by ticket that needs pairing again opens the form for a new
/// ticket instead of dialing; `f` forgets only a box paired by ticket, and
/// only after asking.
#[tokio::test]
async fn a_box_paired_by_ticket_pairs_again_and_is_forgotten_by_ticket() -> TestResult {
    let hosts = json!([
        {"host": "acme", "state": "needs_pairing", "transport": "p2p"},
        {"host": "hetzner", "state": "disconnected", "transport": "ssh"},
    ]);
    let mut app = boxes_app(100, 30, hosts).await;
    let effects = app.update(key(KeyCode::Enter));
    assert!(opened(&effects).is_none(), "{effects:?}");
    assert!(matches!(step(&app), Some(Step::Form)));
    app.update(key(KeyCode::Esc));
    app.update(key(KeyCode::Char('f')));
    assert!(
        matches!(&app.dialog, Some(Dialog::Confirm { prompt, .. }) if prompt == "Forget acme?")
    );
    let effects = app.update(key(KeyCode::Char('y')));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Forget { host } if host == "acme")),
        "{effects:?}"
    );
    app.update(key(KeyCode::Char('j')));
    let effects = app.update(key(KeyCode::Char('f')));
    assert!(app.dialog.is_none(), "{:?}", app.dialog);
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(
        notice(&app),
        "hetzner is not paired by ticket; d disconnects it"
    );
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
        Some(Dialog::Confirm { prompt, detail, .. }) => {
            assert_eq!(prompt, "Disconnect hetzner?");
            assert_eq!(
                detail,
                &[
                    "Its ports and hostnames stop working here.".to_owned(),
                    "Its lanes keep running on the box.".to_owned()
                ]
            );
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
    new_ssh_box(&mut app);
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
        effects.iter().any(|e| matches!(e, Effect::Pair { .. })),
        "{effects:?}"
    );
    app.update(paired(&app, Ok(pair_reply("K7QM-3XWD"))));
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
    after_a_while(&mut app);
    app.update(key(KeyCode::Char('p')));
    app.update(paired(&app, Ok(json!({"code": "K7QM-3XWD"}))));
    app.update(Msg::Tick);
    assert!(screen(&render(&app)?).contains("expires in 9m 59s"));

    let paused = RpcError::new(
        "pairing_paused",
        json!({"message": "the box paused pairing", "retry_after_ms": 252_000}),
        4,
    );
    after_a_while(&mut app);
    app.update(key(KeyCode::Char('p')));
    app.update(paired(&app, Err(ApiError::Rpc(paused))));
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
    after_a_while(&mut app);
    app.update(key(KeyCode::Char('p')));
    app.update(paired(&app, Err(ApiError::Rpc(off))));
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

/// The pairing modal shows the ticket before the code. A ticket that was
/// not ready leaves the code on screen, and only `t` asks for the ticket
/// alone, once; neither drawing nor ticks nor polls ask the daemon for a
/// ticket, which would start this machine's peer-to-peer networking.
#[tokio::test]
async fn the_pairing_modal_asks_for_a_ticket_only_when_told() -> TestResult {
    let mut app = boxes_app(100, 40, json!([])).await;
    let asks = |effects: &[Effect]| {
        effects
            .iter()
            .filter(|e| matches!(e, Effect::Pair { .. } | Effect::MintTicket { .. }))
            .count()
    };
    assert_eq!(asks(&app.update(key(KeyCode::Char('p')))), 1);
    // A held or repeated p while the answer is out asks nothing more.
    let held = Msg::Key(KeyEvent {
        kind: crossterm::event::KeyEventKind::Repeat,
        ..KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)
    });
    assert_eq!(asks(&app.update(held)), 0);
    assert_eq!(asks(&app.update(key(KeyCode::Char('p')))), 0);
    let first = pair_generation(&app);
    let mut reply = pair_reply("K7QM-3XWD");
    reply["ticket"] = json!(test_ticket());
    app.update(paired(&app, Ok(reply)));
    let text = screen(&render(&app)?);
    let ticket_at = text.find(&test_ticket()[..40]).ok_or("no ticket")?;
    let code_at = text.find("--code K7QM-3XWD").ok_or("no code")?;
    assert!(ticket_at < code_at, "{text}");
    assert!(text.contains("or after devlanes remote connect"), "{text}");
    let effects = app.update(key(KeyCode::Char('y')));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Copy(text) if *text == test_ticket())),
        "{effects:?}"
    );

    // Within a second of the answer, p asks nothing: a held p on Windows
    // comes as presses, not repeats.
    assert_eq!(asks(&app.update(key(KeyCode::Char('p')))), 0);
    after_a_while(&mut app);
    assert_eq!(asks(&app.update(key(KeyCode::Char('p')))), 1);
    // A late answer to the replaced modal's request is not shown.
    let mut stale = pair_reply("STAL-E000");
    stale["ticket"] = json!(test_ticket());
    app.update(Msg::Paired {
        generation: first,
        result: Ok(stale),
    });
    assert!(matches!(
        &app.dialog,
        Some(Dialog::Pair(dialog)) if dialog.state == PairState::Loading
    ));
    let mut reply = pair_reply("K7QM-3XWD");
    reply["ticket_error"] = json!({"code": "ticket_not_ready",
        "message": "this machine was still reaching its relay; run devlanes daemon pair again"});
    app.update(paired(&app, Ok(reply)));
    for _ in 0..3 {
        assert_eq!(asks(&app.update(Msg::Tick)), 0);
        render(&app)?;
        assert_eq!(asks(&app.update(Msg::Boxes(Ok(json!({"items": []}))))), 0);
    }
    let text = screen(&render(&app)?);
    assert!(text.contains("No ticket yet"), "{text}");
    assert!(text.contains("K7QM-3XWD"), "{text}");
    assert!(!text.contains("daemon pair again"), "{text}");
    assert_eq!(asks(&app.update(key(KeyCode::Char('t')))), 1);
    assert_eq!(
        asks(&app.update(key(KeyCode::Char('t')))),
        0,
        "one at a time"
    );
    assert!(screen(&render(&app)?).contains("K7QM-3XWD"));
    app.update(Msg::TicketMinted {
        generation: pair_generation(&app),
        result: Ok(json!({"ticket": test_ticket(), "valid_ms": 600_000})),
    });
    let text = screen(&render(&app)?);
    assert!(text.contains(&test_ticket()[..40]), "{text}");
    assert!(text.contains("--code K7QM-3XWD"), "{text}");
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
    app.update(paired(&app, Err(ApiError::Rpc(off))));
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
    new_ssh_box(&mut app);
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
    let effects = new_ssh_box(&mut app);
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
    new_ssh_box(&mut app);
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
    new_ssh_box(&mut app);
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
    keys.extend(ssh_keys());
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
    steps.extend(presses(ssh_keys()));
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
