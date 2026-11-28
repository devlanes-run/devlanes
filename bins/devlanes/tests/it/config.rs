//! `devlanes config get|set|unset` on the user config of an isolated
//! `DEVLANES_HOME`, run as the real binary without a daemon.

use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Exit status, stdout and stderr of a finished command.
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// `devlanes <args>` with `home` as its devlanes home and plain output. The
/// child is killed if it outlives the deadline.
fn devlanes(home: &Path, args: &[&str]) -> Run {
    let mut child = Command::new(env!("CARGO_BIN_EXE_devlanes"))
        .args(args)
        .env("DEVLANES_HOME", home)
        .env("DEVLANES_OUTPUT", "plain")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start devlanes");
    let mut stdout = child.stdout.take().expect("stdout");
    let mut stderr = child.stderr.take().expect("stderr");
    // Read on threads so a full pipe cannot stall the child; channels, so a
    // timeout can collect what arrived without waiting on a pipe that a
    // child of the child may keep open.
    let (out_sent, out) = std::sync::mpsc::channel();
    let (err_sent, err) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stdout.read_to_end(&mut text);
        let _ = out_sent.send(String::from_utf8_lossy(&text).into_owned());
    });
    std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stderr.read_to_end(&mut text);
        let _ = err_sent.send(String::from_utf8_lossy(&text).into_owned());
    });
    let started = Instant::now();
    let deadline = started + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            let collect = |text: &std::sync::mpsc::Receiver<String>| {
                text.recv_timeout(Duration::from_secs(2))
                    .unwrap_or_else(|_| "(still open after the kill)".into())
            };
            panic!(
                "devlanes {args:?} did not exit within 60 s (killed after {:?})\nstdout:\n{}\nstderr:\n{}",
                started.elapsed(),
                collect(&out),
                collect(&err)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Run {
        code: status.code(),
        stdout: out.recv().expect("stdout reader"),
        stderr: err.recv().expect("stderr reader"),
    }
}

/// Asserts a zero exit and returns stdout.
fn ok(run: Run) -> String {
    assert_eq!(run.code, Some(0), "{}{}", run.stdout, run.stderr);
    run.stdout
}

#[test]
fn set_get_and_unset_round_trip_without_a_daemon() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path();
    let file = home.join("config.yaml");
    std::fs::write(&file, "# mine\nproxy:\n  http_port: 8080 # kept\n").expect("config");

    assert_eq!(
        ok(devlanes(home, &["config", "get", "tui.theme"])),
        "auto\n"
    );
    let set = ok(devlanes(home, &["config", "set", "tui.theme", "dark"]));
    assert!(set.starts_with("set tui.theme to dark in "), "{set}");
    assert!(set.contains("the next devlanes command"), "{set}");
    let set = ok(devlanes(
        home,
        &["config", "set", "proxy.https_port", "8443"],
    ));
    assert!(set.contains("`devlanes daemon restart`"), "{set}");
    ok(devlanes(
        home,
        &["config", "set", "tls.trust", "[system, firefox]"],
    ));
    let text = std::fs::read_to_string(&file).expect("read");
    assert!(
        text.starts_with("# mine\nproxy:\n  http_port: 8080 # kept\n"),
        "{text}"
    );

    assert_eq!(
        ok(devlanes(home, &["config", "get", "tui.theme"])),
        "dark\n"
    );
    assert_eq!(
        ok(devlanes(home, &["config", "get", "proxy.https_port"])),
        "8443\n"
    );
    let tui = ok(devlanes(home, &["config", "get", "tui"]));
    assert!(
        tui.contains("theme: dark\n") && tui.contains("glyphs: auto\n"),
        "{tui}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&ok(devlanes(home, &["config", "get", "--json"]))).expect("JSON");
    assert_eq!(
        json["value"]["tls"]["trust"],
        serde_json::json!(["system", "firefox"])
    );
    assert_eq!(json["value"]["metrics"]["retention"], "7d");

    let unset = ok(devlanes(home, &["config", "unset", "tui.theme"]));
    assert!(unset.starts_with("removed tui.theme from "), "{unset}");
    assert_eq!(
        ok(devlanes(home, &["config", "get", "tui.theme"])),
        "auto\n"
    );
    let again = ok(devlanes(home, &["config", "unset", "tui.theme"]));
    assert!(again.contains("is not set"), "{again}");
    let text = std::fs::read_to_string(&file).expect("read");
    assert!(!text.contains("tui"), "{text}");
    // Nothing but the config: no daemon was started.
    let entries: Vec<_> = std::fs::read_dir(home)
        .expect("home")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(entries, ["config.yaml"]);
}

#[test]
fn invalid_values_and_unknown_keys_change_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path();
    let file = home.join("config.yaml");
    let original = "proxy:\n  https_port: 443\n";
    std::fs::write(&file, original).expect("config");
    for (args, expected) in [
        (
            &["config", "set", "proxy.https_port", "nope"][..],
            "was not changed: proxy.https_port:",
        ),
        (
            &["config", "set", "proxy.http_port", "443"],
            "proxy.https_port: must differ from proxy.http_port",
        ),
        (
            &["config", "set", "proxy.https_port", "-1"],
            "was not changed",
        ),
        (
            &["config", "set", "tui.theme", "[unclosed"],
            "was not changed",
        ),
    ] {
        let run = devlanes(home, args);
        assert_eq!(run.code, Some(1), "{args:?}: {}", run.stderr);
        assert!(run.stderr.contains(expected), "{args:?}: {}", run.stderr);
        assert_eq!(std::fs::read_to_string(&file).expect("read"), original);
    }
    for args in [
        &["config", "get", "proxy.htps_port"][..],
        &["config", "set", "proxy.htps_port", "8443"],
        &["config", "unset", "proxy.htps_port"],
    ] {
        let run = devlanes(home, args);
        assert_eq!(run.code, Some(2), "{args:?}: {}", run.stderr);
        assert!(
            run.stderr.starts_with(
                "error: unknown setting `proxy.htps_port`; did you mean `proxy.https_port`?"
            ),
            "{args:?}: {}",
            run.stderr
        );
        assert_eq!(std::fs::read_to_string(&file).expect("read"), original);
    }
}

#[test]
fn a_broken_file_still_reads_and_a_misspelled_key_can_be_removed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path();
    let file = home.join("config.yaml");
    std::fs::write(&file, "tui:\n  theme: dark\n  colour: red\n").expect("config");
    let run = devlanes(home, &["config", "get", "tui.theme"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    // Only the bad key is left out, with a warning naming it.
    assert_eq!(run.stdout, "dark\n");
    assert!(
        run.stderr.starts_with("warning: ") && run.stderr.contains("colour"),
        "{}",
        run.stderr
    );
    ok(devlanes(home, &["config", "unset", "tui.colour"]));
    assert_eq!(
        std::fs::read_to_string(&file).expect("read"),
        "tui:\n  theme: dark\n"
    );
    let run = devlanes(home, &["config", "get", "tui.theme"]);
    assert_eq!((run.stdout.as_str(), run.stderr.as_str()), ("dark\n", ""));
}
