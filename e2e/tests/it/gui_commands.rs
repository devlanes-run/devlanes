//! The `devlanes gui` commands against the real `devlanes-gui` and its test
//! pattern, on a lane of this machine and on a box's lane through its
//! mirrored port.
//!
//! The pattern shows what it got in its picture: a click leaves a black
//! square where it landed, and every redraw counts up in the bottom row (one
//! pixel per bit, white for 1). It redraws once per frame the host asks for
//! (each sync and each screenshot ask for one), per press, per text and per
//! key going down, so the counter tells how much input arrived. Which keys
//! and which text arrived is checked against the pattern's own list in
//! `devlanes-gui-tool`'s tests; a second window can only be opened there too,
//! since nothing outside the host process can tell the pattern to open one.

use std::{path::Path, process::Command, time::Duration};

use devlanes_e2e::{Lab, Output, Result};
use devlanes_gui_host::pattern;
use devlanes_gui_proto::Size;
use serde_json::Value;

use crate::{BUDGET, need, remote};

/// A GUI service and a plain one.
const CONFIG: &str = "\
project: fixture-gui
services:
  desktop:
    run: [fixture-server, --log-line, desktop-ready]
    ready: { http: { path: /healthz } }
    gui: true
  api:
    run: [fixture-server, --log-line, api-ready]
";

/// `command` with GUI services on and the test-pattern backend.
fn with_gui(mut command: Command) -> Command {
    command.env("DEVLANES_GUI_BACKEND", "test-pattern");
    command
}

/// `devlanes <args>` in `lab`'s repository with GUI services on.
fn devlanes(lab: &Lab, args: &[&str]) -> Result<Output> {
    let command = with_gui(lab.command(lab.repo(), args)?);
    lab.run(command, Duration::from_secs(60))
}

/// A decoded PNG: its size and RGBA pixels.
struct Picture {
    size: Size,
    rgba: Vec<u8>,
}

impl Picture {
    /// Reads the PNG at `path`.
    fn read(path: &Path) -> Result<Self> {
        let file = std::io::BufReader::new(std::fs::File::open(path)?);
        let mut reader = png::Decoder::new(file)
            .read_info()
            .map_err(|error| devlanes_e2e::Error::Unexpected(format!("not a PNG: {error}")))?;
        let mut rgba = vec![0; reader.output_buffer_size().unwrap_or_default()];
        let info = reader
            .next_frame(&mut rgba)
            .map_err(|error| devlanes_e2e::Error::Unexpected(format!("not a PNG: {error}")))?;
        assert_eq!(info.color_type, png::ColorType::Rgba, "an RGBA PNG");
        Ok(Self {
            size: Size::new(info.width, info.height),
            rgba,
        })
    }

    /// The RGB colour at `x`, `y`.
    fn at(&self, x: u32, y: u32) -> [u8; 3] {
        let index = (y as usize * self.size.width as usize + x as usize) * 4;
        [self.rgba[index], self.rgba[index + 1], self.rgba[index + 2]]
    }

    /// Whether the centre of every cell has the grid's colour.
    fn shows_grid(&self) -> bool {
        (0..pattern::GRID_ROWS).all(|row| {
            (0..pattern::GRID_COLUMNS).all(|column| {
                let centre = pattern::cell_centre(column, row, self.size);
                let (x, y) = (centre.x as u32, centre.y as u32);
                self.at(x, y) == pattern::grid_colour(x, y, self.size)
            })
        })
    }

    /// The pattern's redraw counter from the bottom row.
    fn counter(&self) -> u32 {
        (0..32).fold(0, |counter, bit| {
            let on = self.at(bit, self.size.height - 1) == [0xff, 0xff, 0xff];
            counter | (u32::from(on) << bit)
        })
    }
}

/// `gui screenshot <unit> --to <dir>/<name>.png --json` in `lab`, its JSON and picture.
fn screenshot(lab: &Lab, dir: &Path, unit: &str, name: &str) -> Result<(Value, Picture)> {
    let file = dir.join(format!("{name}.png"));
    let to = file.to_string_lossy().into_owned();
    let shot = devlanes(lab, &["gui", "screenshot", unit, "--to", &to, "--json"])?
        .expect_code(0)?
        .json()?;
    assert_eq!(
        Path::new(need(shot["path"].as_str(), "the screenshot path")?),
        std::path::absolute(&file)?,
        "{shot}"
    );
    Ok((shot, Picture::read(&file)?))
}

/// `gui ls` lists the window; a screenshot is a PNG of the grid at the
/// window's size; a click, typed text and a chord arrive; a resize applies
/// and the next screenshot has the new size. Errors keep their exit codes,
/// and with GUI services off the commands say they are not supported yet.
#[test]
fn gui_commands_drive_the_test_pattern() -> Result<()> {
    let lab = Lab::with_config(CONFIG, BUDGET)?;
    let shots = tempfile::tempdir()?;
    let started = devlanes(&lab, &["start"])?.expect_code(0)?.json()?;
    assert_eq!(started["ready"], true, "{started}");

    let listed = devlanes(&lab, &["gui", "ls", "--json"])?
        .expect_code(0)?
        .json()?;
    assert_eq!(listed["lane"], "fixture-gui@0", "{listed}");
    let items = listed["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(items.len(), 1, "only desktop is a GUI service: {listed}");
    assert_eq!(items[0]["unit"], "desktop", "{listed}");
    assert_eq!(items[0]["host"], "running", "{listed}");
    assert_eq!(items[0]["real_desktop"], false, "{listed}");
    let window = &items[0]["windows"][0];
    assert_eq!(window["kind"], "toplevel", "{listed}");
    let size = Size::new(
        need(window["width"].as_u64(), "the width")? as u32,
        need(window["height"].as_u64(), "the height")? as u32,
    );

    let (shot, before) = screenshot(&lab, shots.path(), "desktop", "before")?;
    assert_eq!(before.size, size, "{shot}");
    assert_eq!(
        (shot["width"].as_u64(), shot["height"].as_u64()),
        (Some(size.width.into()), Some(size.height.into()))
    );
    assert!(before.shows_grid(), "the screenshot shows the grid");

    // Inside a cell, away from its centre, which the grid check reads.
    let (x, y) = (size.width / 8 + 10, size.height / 6 + 10);
    assert_ne!(before.at(x, y), [0, 0, 0]);
    let clicked = devlanes(
        &lab,
        &[
            "gui",
            "click",
            "desktop",
            &x.to_string(),
            &y.to_string(),
            "--json",
        ],
    )?
    .expect_code(0)?
    .json()?;
    assert_eq!(clicked["action"], "click", "{clicked}");
    let (_, after) = screenshot(&lab, shots.path(), "desktop", "clicked")?;
    assert_eq!(after.at(x, y), [0, 0, 0], "the click's marker");

    // A sync before and after the text, the text, and the screenshot.
    devlanes(&lab, &["gui", "type", "desktop", "hello"])?.expect_code(0)?;
    let (_, typed) = screenshot(&lab, shots.path(), "desktop", "typed")?;
    assert_eq!(typed.counter(), after.counter() + 4, "the text redrew once");
    // Syncs, ctrl and s going down, and the screenshot; releases draw nothing.
    devlanes(&lab, &["gui", "key", "desktop", "ctrl+s"])?.expect_code(0)?;
    let (_, keyed) = screenshot(&lab, shots.path(), "desktop", "keyed")?;
    assert_eq!(keyed.counter(), typed.counter() + 5, "two keys went down");

    let resized = devlanes(&lab, &["gui", "resize", "desktop", "200x120", "--json"])?
        .expect_code(0)?
        .json()?;
    assert_eq!(resized["size"]["width"], 200, "{resized}");
    assert_eq!(resized["size"]["height"], 120, "{resized}");
    let (_, smaller) = screenshot(&lab, shots.path(), "desktop", "resized")?;
    assert_eq!(smaller.size, Size::new(200, 120));
    assert!(smaller.shows_grid(), "the grid at the new size");

    let refused = devlanes(&lab, &["gui", "screenshot", "desktop", "--window", "9"])?;
    assert!(
        refused.expect_code(3)?.stderr.contains("has no window 9"),
        "{refused}"
    );
    let refused = devlanes(&lab, &["gui", "click", "api", "1", "1"])?;
    assert!(
        refused
            .expect_code(3)?
            .stderr
            .contains("is not a GUI service"),
        "{refused}"
    );
    devlanes(&lab, &["gui", "key", "desktop", "hyper+a"])?.expect_code(2)?;
    // The CLI without the switch, against the same daemon.
    let off = lab.run(
        lab.command(lab.repo(), &["gui", "ls"])?,
        Duration::from_secs(60),
    )?;
    assert!(
        off.expect_code(1)?
            .stderr
            .contains("devlanes gui is not supported yet"),
        "{off}"
    );

    devlanes(&lab, &["stop", "desktop"])?.expect_code(0)?;
    let stopped = devlanes(&lab, &["gui", "type", "desktop", "x"])?;
    assert!(
        stopped.expect_code(1)?.stderr.contains("is not running"),
        "{stopped}"
    );
    Ok(())
}

/// A box's GUI service, from a connected machine with no project of its
/// own, found through `remote ls`. Both labs share this machine's loopback,
/// where the box's own host holds the port, so the client cannot bind its
/// mirror: the commands find the service and its GUI port and say why it
/// cannot be reached, naming what holds the port. Reaching a host through a
/// bound mirror needs two machines (or network namespaces) and is left to
/// the address lookup's unit test in the binary.
#[test]
fn gui_commands_find_a_box_lane() -> Result<()> {
    let boxed = Lab::with_config(CONFIG, BUDGET)?;
    let config = boxed.home().join("config.yaml");
    let mut text = std::fs::read_to_string(&config)?;
    text.push_str("remote:\n  require_pairing: false\n");
    std::fs::write(&config, text)?;
    let started = devlanes(&boxed, &["start"])?.expect_code(0)?.json()?;
    assert_eq!(started["ready"], true, "{started}");

    let client = Lab::without_config(BUDGET)?;
    let config = client.home().join("config.yaml");
    let mut text = std::fs::read_to_string(&config)?;
    text.push_str(&format!(
        "remote:\n  boxes:\n    acme:\n      command: {}\n",
        remote::box_command(&boxed)?
    ));
    std::fs::write(&config, text)?;
    client.json(
        client.repo(),
        &["remote", "connect", "acme", "--background"],
    )?;
    // The box's GUI port, once the client lists it among the box's ports.
    let gui_port = |host: &Value| {
        let units = host["registry"]["units"].as_array()?;
        let desktop = units.iter().find(|unit| unit["key"]["name"] == "desktop")?;
        let port = desktop["ports"]
            .as_array()?
            .iter()
            .find(|port| port["name"] == "devlanes_gui")?["port"]
            .as_u64()?;
        host["ports"]
            .as_array()?
            .iter()
            .any(|entry| entry["port"] == port)
            .then_some(port)
    };
    let port = client.eventually("the box's GUI port in remote ls", remote::CONNECT, || {
        Ok(remote::host(&client, "acme")?.and_then(|host| gui_port(&host)))
    })?;

    let listed = devlanes(&client, &["gui", "ls", "--lane", "fixture-gui@0", "--json"])?
        .expect_code(0)?
        .json()?;
    assert_eq!(listed["box"], "acme", "{listed}");
    let item = &listed["items"][0];
    assert_eq!(item["unit"], "desktop", "{listed}");
    assert_eq!(item["address"], Value::Null, "{listed}");
    let held = format!("port {port} is not mirrored because devlanes-gui");
    assert!(
        item["error"]
            .as_str()
            .is_some_and(|error| error.contains(&held)),
        "{listed}"
    );

    let shot = devlanes(&client, &["gui", "screenshot", "fixture-gui@0:desktop"])?;
    let stderr = &shot.expect_code(1)?.stderr;
    assert!(
        stderr.contains("fixture-gui@0:desktop on box acme is not mirrored to this machine")
            && stderr.contains(&held),
        "{shot}"
    );

    client.json(client.repo(), &["remote", "disconnect", "acme"])?;
    Ok(())
}
