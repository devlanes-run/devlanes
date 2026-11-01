//! The website's live scene (`demo::Scene::web`): each lane prints its own
//! database, Redis index and ports, attached units and containers print new
//! lines as seconds pass, tasks run and print, and stop and start change
//! unit states.

use devlanes_tui::{
    app::RowId,
    demo::{Fake, Scene},
    keys::KeyCode,
};

/// The website's scene at the demo's size.
fn fake() -> Fake {
    Fake::with_scene(120, 40, Scene::web())
}

/// What the attached pane shows.
fn pane(fake: &Fake) -> String {
    match fake.app.attached.as_ref() {
        Some(attached) => attached.shown().contents(),
        None => panic!("nothing is attached"),
    }
}

/// The state of unit `name` in lane `lane`.
fn state(fake: &Fake, lane: &str, name: &str) -> String {
    fake.app
        .lanes
        .iter()
        .find(|l| l.reference == lane)
        .and_then(|l| l.units.iter().find(|u| u.name == name))
        .map_or_else(|| "absent".into(), |unit| unit.state.clone())
}

#[test]
fn each_lane_prints_its_own_database_and_port() {
    let mut fake = fake();
    fake.unit("myapp@0", "api");
    let main = pane(&fake);
    assert!(
        main.contains("myapp_main") && main.contains(":41000"),
        "{main}"
    );
    fake.unit("myapp@3", "api");
    let agent = pane(&fake);
    assert!(
        agent.contains("myapp_lane3") && agent.contains(":41300"),
        "{agent}"
    );
    assert!(agent.contains("limit=500"), "{agent}");
}

#[test]
fn attached_units_and_containers_print_as_seconds_pass() {
    let mut fake = fake();
    fake.unit("myapp@3", "api");
    let before = pane(&fake);
    for _ in 0..3 {
        fake.tick();
    }
    assert_ne!(pane(&fake), before);

    fake.select("myapp@1", RowId::Container("mailpit".into()));
    let mail = pane(&fake);
    assert!(
        mail.contains("myapp1.localhost") || mail.contains("lane1") || mail.contains("18125"),
        "{mail}"
    );
    let before = pane(&fake);
    for _ in 0..20 {
        fake.tick();
    }
    assert_ne!(pane(&fake), before);
}

#[test]
fn a_task_runs_prints_and_exits() {
    let mut fake = fake();
    // The lane's task list loads once the lane shows.
    fake.lane("myapp@1");
    fake.select("myapp@1", RowId::Task("migrate".into()));
    fake.key(KeyCode::Char('s'));
    assert_eq!(state(&fake, "myapp@1", "migrate"), "running");
    for _ in 0..8 {
        fake.tick();
    }
    assert_eq!(state(&fake, "myapp@1", "migrate"), "exited");
    let output = pane(&fake);
    assert!(output.contains("myapp_lane1 is up to date"), "{output}");
}

#[test]
fn stop_and_start_change_the_unit() {
    let mut fake = fake();
    fake.unit("myapp@0", "api");
    fake.key(KeyCode::Char('x'));
    assert_eq!(state(&fake, "myapp@0", "api"), "stopped");
    fake.key(KeyCode::Char('s'));
    assert_eq!(state(&fake, "myapp@0", "api"), "starting");
    fake.tick();
    assert_eq!(state(&fake, "myapp@0", "api"), "ready");
}
