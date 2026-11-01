//! The live fake daemon: each lane prints its own database, Redis index and
//! ports, the attached unit or container prints new lines as seconds pass,
//! tasks run and print, and stop and start change unit states.

use devlanes_tui::app::DaemonState;
use devlanes_tui_web::Session;

/// A session at the website's wide size.
fn session() -> Session {
    Session::new(120, 40)
}

/// Presses `key` with no modifiers.
fn press(session: &mut Session, key: &str) {
    session.key(key, false, false, false, false);
}

/// What the attached pane shows.
fn pane(session: &Session) -> String {
    match session.app().attached.as_ref() {
        Some(attached) => attached.shown().contents(),
        None => panic!("nothing is attached"),
    }
}

/// The state of unit `name` in lane `lane`.
fn state(session: &Session, lane: &str, name: &str) -> String {
    session
        .app()
        .lanes
        .iter()
        .find(|l| l.reference == lane)
        .and_then(|l| l.units.iter().find(|u| u.name == name))
        .map_or_else(|| "absent".into(), |unit| unit.state.clone())
}

/// Moves down the list until `name` is attached, at most 12 rows.
fn down_to(session: &mut Session, name: &str) {
    for _ in 0..12 {
        if session
            .app()
            .attached
            .as_ref()
            .is_some_and(|a| a.unit == name)
        {
            return;
        }
        press(session, "j");
    }
    panic!("{name} never got attached");
}

#[test]
fn each_lane_prints_its_own_database_and_port() {
    let mut session = session();
    let main = pane(&session);
    assert!(
        main.contains("myapp_main") && main.contains(":41000"),
        "{main}"
    );
    press(&mut session, "4");
    let agent = pane(&session);
    assert!(
        agent.contains("myapp_lane3") && agent.contains(":41300"),
        "{agent}"
    );
    assert!(agent.contains("limit=500"), "{agent}");
}

#[test]
fn the_attached_unit_and_container_print_as_seconds_pass() {
    let mut session = session();
    press(&mut session, "4");
    let before = pane(&session);
    for _ in 0..3 {
        session.tick();
    }
    assert_ne!(pane(&session), before);

    press(&mut session, "2");
    press(&mut session, "i");
    down_to(&mut session, "mailpit");
    let mail = pane(&session);
    assert!(
        mail.contains("myapp1.localhost") || mail.contains("lane1"),
        "{mail}"
    );
    for _ in 0..20 {
        session.tick();
    }
    assert_ne!(pane(&session), mail);
}

#[test]
fn a_task_runs_prints_and_exits() {
    let mut session = session();
    press(&mut session, "2");
    press(&mut session, "t");
    press(&mut session, "j");
    press(&mut session, "s");
    assert_eq!(state(&session, "myapp@1", "migrate"), "running");
    for _ in 0..8 {
        session.tick();
    }
    assert_eq!(state(&session, "myapp@1", "migrate"), "exited");
    let output = pane(&session);
    assert!(output.contains("myapp_lane1 is up to date"), "{output}");
}

/// Whether the TUI sees the daemon up.
fn online(session: &Session) -> bool {
    session.app().daemon_state == DaemonState::Online
}

/// Answers the open confirmation with `y`, which the daemon's stop and
/// restart ask for.
fn confirm(session: &mut Session) {
    assert!(session.app().dialog.is_some(), "no confirmation open");
    press(session, "y");
}

#[test]
fn the_daemon_restarts_and_comes_back() {
    let mut session = session();
    session.key("D", false, false, true, false);
    session.key("R", false, false, true, false);
    confirm(&mut session);
    assert!(!online(&session));
    for _ in 0..6 {
        session.tick();
    }
    assert!(online(&session), "{:?}", session.app().daemon_state);
    assert_eq!(state(&session, "myapp@0", "api"), "ready");
    // The daemon page shows the new process.
    let status = session.app().daemon.status.as_ref().expect("daemon status");
    assert_eq!(status["pid"], 4243);
}

#[test]
fn a_stopped_daemon_stays_down_until_started() {
    let mut session = session();
    session.key("D", false, false, true, false);
    session.key("S", false, false, true, false);
    confirm(&mut session);
    for _ in 0..6 {
        session.tick();
    }
    assert!(
        matches!(session.app().daemon_state, DaemonState::Stopped { .. }),
        "{:?}",
        session.app().daemon_state
    );
    press(&mut session, "s");
    for _ in 0..6 {
        session.tick();
    }
    assert!(online(&session), "{:?}", session.app().daemon_state);
    // A stop ends every unit; starting the daemon does not bring them back.
    assert_eq!(state(&session, "myapp@0", "api"), "stopped");
}

#[test]
fn stop_and_start_change_the_unit() {
    let mut session = session();
    press(&mut session, "x");
    assert_eq!(state(&session, "myapp@0", "api"), "stopped");
    press(&mut session, "s");
    assert_eq!(state(&session, "myapp@0", "api"), "starting");
    session.tick();
    assert_eq!(state(&session, "myapp@0", "api"), "ready");
}
