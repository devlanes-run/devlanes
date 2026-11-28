//! `devlanes gui`: its arguments, its results in every output mode and the
//! wording of its errors. The binary builds the result data from what the
//! GUI host and the daemon said; these fixtures have its shape.

use clap::Parser;
use devlanes_cli::*;
use devlanes_proto::RpcError;
use serde_json::{Value, json};

/// Runtime for `mode` with color off.
fn runtime(mode: OutputMode, quiet: bool) -> Runtime {
    let mut args = vec!["devlanes"];
    if quiet {
        args.push("--quiet");
    }
    Runtime::resolve(
        &Cli::parse_from(args),
        Signals {
            env_output: Some(mode.as_str().into()),
            stdout_tty: mode == OutputMode::Human,
            no_color: true,
            ..Signals::default()
        },
    )
    .expect("runtime")
}

/// The command a `devlanes gui ...` line parses to.
fn parse(args: &[&str]) -> Result<GuiCommand, String> {
    let cli = Cli::try_parse_from(std::iter::once("devlanes").chain(args.iter().copied()))
        .map_err(|error| error.to_string())?;
    match cli.command {
        Some(Command::Gui { command }) => Ok(command),
        _ => Err("not a gui command".into()),
    }
}

/// Sizes, negative scroll deltas, `--at` and the button parse; bad sizes and
/// buttons are refused before anything runs.
#[test]
fn gui_arguments_parse() {
    let Ok(GuiCommand::Resize { size, .. }) = parse(&["gui", "resize", "desktop", "1280x800"])
    else {
        panic!("resize");
    };
    assert_eq!((size.width, size.height), (1280, 800));
    let Ok(GuiCommand::Scroll { dx, dy, at, .. }) =
        parse(&["gui", "scroll", "desktop", "-2", "-3", "--at", "10", "20"])
    else {
        panic!("scroll");
    };
    assert_eq!((dx, dy, at), (-2, -3, Some(vec![10, 20])));
    let Ok(GuiCommand::Click { button, double, .. }) = parse(&[
        "gui", "click", "desktop", "1", "2", "--button", "right", "--double",
    ]) else {
        panic!("click");
    };
    assert_eq!((button, double), (GuiButton::Right, true));
    let Ok(GuiCommand::Type { text, .. }) = parse(&["gui", "type", "desktop", "-n 1"]) else {
        panic!("type");
    };
    assert_eq!(text, "-n 1");
    for bad in [
        &["gui", "resize", "desktop", "1280"][..],
        &["gui", "resize", "desktop", "0x800"],
        &["gui", "resize", "desktop", "800x-1"],
        &["gui", "click", "desktop", "1", "2", "--button", "back"],
        &["gui", "key", "desktop"],
        &["gui", "scroll", "desktop", "0", "1", "--at", "10"],
    ] {
        assert!(parse(bad).is_err(), "{bad:?} parsed");
    }
}

/// A result of each kind, as the binary builds it.
fn fixtures() -> Vec<(&'static str, GuiOutput, Value)> {
    let window = |id: u32, title: &str, lock: &str| {
        json!({"id": id, "title": title, "app_id": "run.devlanes.test-pattern", "width": 320,
               "height": 200, "kind": "toplevel", "lock": lock, "parent": null})
    };
    let target = |action: &str| {
        json!({"lane": "acme@2", "unit": "desktop", "box": null,
               "window": {"id": 1, "title": "Test pattern 1"}, "action": action,
               "window_after": "open"})
    };
    let mut click = target("click");
    click["button"] = json!("left");
    click["double"] = json!(true);
    click["at"] = json!({"x": 120, "y": 48});
    click["window_after"] = json!("closed");
    let mut typed = target("type");
    typed["characters"] = json!(11);
    let mut key = target("key");
    key["keys"] = json!(["ctrl+s", "enter"]);
    key["window_after"] = json!("frozen");
    let mut scroll = target("scroll");
    scroll["box"] = json!("hetzner");
    scroll["delta"] = json!({"x": 0, "y": -3});
    scroll["at"] = json!({"x": 160, "y": 100});
    let mut resized = target("resize");
    resized["requested"] = json!({"width": 2000, "height": 900});
    resized["size"] = json!({"width": 1920, "height": 900});
    vec![
        (
            "ls",
            GuiOutput::Ls,
            json!({"lane": "acme@2", "box": null, "truncated": false, "next_cursor": null, "items": [
                {"unit": "desktop", "state": "ready", "host": "running", "address": "127.0.0.1:26012",
                 "real_desktop": false, "unmirrored": null, "error": null,
                 "windows": [window(1, "Test pattern 1", "unlocked"), window(2, "Settings", "frozen")]},
                {"unit": "viewer.2", "state": "stopped", "host": "stopped", "address": null,
                 "real_desktop": null, "unmirrored": null, "error": null, "windows": []},
                {"unit": "editor", "state": "ready", "host": "unreachable", "address": "127.0.0.1:26013",
                 "real_desktop": null, "unmirrored": null,
                 "error": "the GUI host did not answer the connection in time", "windows": []},
            ]}),
        ),
        (
            "ls_box",
            GuiOutput::Ls,
            json!({"lane": "acme@2", "box": "hetzner", "truncated": false, "next_cursor": null, "items": [
                {"unit": "desktop", "state": "ready", "host": "running", "address": null,
                 "real_desktop": null, "error": null,
                 "unmirrored": {"port": 26012, "state": "local_lane", "message": null},
                 "windows": []},
            ]}),
        ),
        (
            "ls_empty",
            GuiOutput::Ls,
            json!({"lane": "acme@2", "box": null, "items": [], "truncated": false, "next_cursor": null}),
        ),
        (
            "screenshot",
            GuiOutput::Screenshot,
            json!({"lane": "acme@2", "unit": "desktop", "box": null,
                   "window": {"id": 1, "title": "Test pattern 1"},
                   "path": "/home/ada/.devlanes/screenshots/acme@2-desktop-1.png", "width": 320, "height": 200}),
        ),
        ("click", GuiOutput::Input, click),
        ("type", GuiOutput::Input, typed),
        ("key", GuiOutput::Input, key),
        ("scroll", GuiOutput::Input, scroll),
        ("resize", GuiOutput::Input, resized),
    ]
}

/// JSON prints the data unchanged, `--quiet` prints nothing in text modes,
/// and each text mode has its lines.
#[test]
fn gui_results_in_every_output_mode() {
    let mut text = String::new();
    for (name, output, data) in fixtures() {
        let json = runtime(OutputMode::Json, true)
            .reporter()
            .gui(output, &data)
            .expect("json");
        assert_eq!(
            serde_json::from_str::<Value>(&json.stdout).expect("json output"),
            data,
            "{name}"
        );
        let quiet = runtime(OutputMode::Plain, true)
            .reporter()
            .gui(output, &data)
            .expect("quiet");
        assert!(quiet.stdout.is_empty(), "{name}: {}", quiet.stdout);
        for mode in [OutputMode::Human, OutputMode::Plain, OutputMode::Agent] {
            let rendered = runtime(mode, false)
                .reporter()
                .gui(output, &data)
                .expect("text");
            assert_eq!(rendered.exit_code, 0, "{name}");
            text.push_str(&format!(
                "--- {name} {}\n{}",
                mode.as_str(),
                rendered.stdout
            ));
        }
    }
    insta::assert_snapshot!("gui_results", text);
}

/// Each failure says what is wrong with which service and window, with the
/// exit code it carries and a command to run next where one helps.
#[test]
fn gui_errors_are_worded() {
    let windows = json!([{"id": 1, "title": "Test pattern 1"}, {"id": 2, "title": "Settings"}]);
    let errors = [
        RpcError::new("not_supported_yet", json!({"feature": "devlanes gui"}), 1),
        RpcError::new(
            "gui_not_gui_service",
            json!({"lane": "acme@2", "unit": "api", "box": null, "services": ["desktop"]}),
            3,
        ),
        RpcError::new(
            "gui_not_running",
            json!({"lane": "acme@2", "unit": "desktop", "box": null, "state": "stopped"}),
            1,
        ),
        RpcError::new(
            "gui_not_running",
            json!({"lane": "acme@2", "unit": "desktop", "box": "hetzner", "state": "failed"}),
            1,
        ),
        RpcError::new(
            "gui_no_host",
            json!({"lane": "acme@2", "unit": "desktop", "box": null, "state": "ready"}),
            1,
        ),
        RpcError::new(
            "gui_host_down",
            json!({"lane": "acme@2", "unit": "desktop", "box": null, "state": "ready"}),
            1,
        ),
        RpcError::new(
            "gui_not_mirrored",
            json!({"lane": "acme@2", "unit": "desktop", "box": "hetzner", "state": "ready",
                   "mirror": {"port": 26012, "state": "in_use",
                              "message": "port 26012 is not mirrored because node (pid 7) uses it here"}}),
            1,
        ),
        RpcError::new(
            "gui_unreachable",
            json!({"lane": "acme@2", "unit": "desktop", "box": null, "address": "127.0.0.1:26012",
                   "message": "GUI connection failed: connection refused"}),
            1,
        ),
        RpcError::new(
            "gui_window_not_found",
            json!({"lane": "acme@2", "unit": "desktop", "box": null, "window": "7", "windows": windows}),
            3,
        ),
        RpcError::new(
            "gui_window_ambiguous",
            json!({"lane": "acme@2", "unit": "desktop", "box": null, "window": "Twin",
                   "windows": [{"id": 3, "title": "Twin"}, {"id": 4, "title": "Twin"}]}),
            3,
        ),
        RpcError::new(
            "gui_no_window",
            json!({"lane": "acme@2", "unit": "desktop", "box": null, "windows": []}),
            1,
        ),
        RpcError::new(
            "gui_window_locked",
            json!({"lane": "acme@2", "unit": "desktop", "box": null,
                   "window": {"id": 1, "title": "Test pattern 1"}, "lock": "frozen"}),
            1,
        ),
        RpcError::new(
            "gui_failed",
            json!({"lane": "acme@2", "unit": "desktop", "box": null,
                   "message": "the GUI host did not answer the screenshot in time"}),
            1,
        ),
        RpcError::new(
            "gui_unknown_key",
            json!({"chord": "hyper+a", "name": "hyper", "reason": "unknown", "known": ["ctrl", "a-z", "enter"]}),
            2,
        ),
        RpcError::new(
            "gui_unknown_key",
            json!({"chord": "ctrl++", "name": null, "reason": "empty", "known": ["ctrl", "a-z", "enter"]}),
            2,
        ),
        RpcError::new(
            "gui_unknown_key",
            json!({"chord": "a+b", "name": null, "reason": "not_a_chord", "known": ["ctrl", "a-z", "enter"]}),
            2,
        ),
        RpcError::new(
            "gui_outside_window",
            json!({"lane": "acme@2", "unit": "desktop", "box": null,
                   "window": {"id": 1, "title": "Test pattern 1"},
                   "at": {"x": 5000, "y": 5000}, "size": {"width": 320, "height": 200}}),
            2,
        ),
        RpcError::new(
            "gui_size_too_large",
            json!({"lane": "acme@2", "unit": "desktop", "box": null, "max": 8192}),
            2,
        ),
        RpcError::new("gui_off_in_daemon", json!({}), 1),
        RpcError::new(
            "gui_bad_reference",
            json!({"reference": "desk*", "reason": "selector"}),
            2,
        ),
        RpcError::new(
            "gui_bad_reference",
            json!({"reference": "2:viewer", "reason": "lanes"}),
            2,
        ),
        RpcError::new(
            "gui_write_failed",
            json!({"path": "/work/out/shot.png", "message": "No such file or directory"}),
            1,
        ),
    ];
    let mut text = String::new();
    for error in &errors {
        assert!(has_wording(&error.code), "{}", error.code);
        let rendered = runtime(OutputMode::Plain, false)
            .reporter()
            .error(error, None);
        assert_eq!(rendered.exit_code, error.exit_code, "{}", error.code);
        text.push_str(&rendered.stderr);
    }
    insta::assert_snapshot!("gui_errors", text);
}
