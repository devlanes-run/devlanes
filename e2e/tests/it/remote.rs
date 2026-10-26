//! `devlanes remote connect` to a box reached by a command. The box is a second
//! lab on this machine, and the command runs a real `devlanes tunnel --stdio`
//! with that lab's home. Unix only: `devlanes tunnel` is not supported on
//! Windows yet.

use std::time::Duration;

use devlanes_e2e::{Lab, Result, lane, port, unit};
use serde_json::{Value, json};

use crate::{BUDGET, need};

/// How long the client gets to connect and see the box's ports.
const CONNECT: Duration = Duration::from_secs(60);

/// The `remote ls` entry of `name` in `lab`, or `None` when it lists no such box.
fn host(lab: &Lab, name: &str) -> Result<Option<Value>> {
    let list = lab.json(lab.repo(), &["remote", "ls", "--json"])?;
    Ok(list["items"]
        .as_array()
        .and_then(|items| items.iter().find(|item| item["host"] == name))
        .cloned())
}

/// A box in `remote.boxes` connects through its command: the client mirrors
/// the box lane's ports, `remote ls` shows the command transport, the box
/// lists the client, and `disconnect` ends the tunnel.
#[test]
fn a_command_box_mirrors_its_lane() -> Result<()> {
    let boxed = Lab::new("two-services", BUDGET)?;
    let started = boxed.json(boxed.repo(), &["start"])?;
    assert_eq!(started["ready"], true, "{started}");
    let lanes = boxed.lanes(boxed.repo())?;
    let api = need(
        lane(&lanes, "fixture-two@0")
            .and_then(|lane| unit(lane, "api"))
            .and_then(port),
        "the box's api port",
    )?;

    // Without a devlanes.yaml, so no lane here competes for the box's hostnames.
    let client = Lab::without_config(BUDGET)?;
    let devlanes = devlanes_e2e::bin_dir()?.join("devlanes");
    let command = json!([
        "env",
        format!("DEVLANES_HOME={}", boxed.home().display()),
        devlanes,
        "tunnel",
        "--stdio",
    ]);
    let config = client.home().join("config.yaml");
    let mut text = std::fs::read_to_string(&config)?;
    text.push_str(&format!(
        "remote:\n  boxes:\n    acme:\n      command: {command}\n"
    ));
    std::fs::write(&config, text)?;

    client.json(
        client.repo(),
        &["remote", "connect", "acme", "--background"],
    )?;
    let mirrored = client.eventually("acme's api port in remote ls", CONNECT, || {
        Ok(host(&client, "acme")?.filter(|host| {
            host["state"] == "connected"
                && host["ports"]
                    .as_array()
                    .is_some_and(|ports| ports.iter().any(|entry| entry["port"] == api))
        }))
    })?;
    assert_eq!(mirrored["transport"], "command", "{mirrored}");
    assert_eq!(mirrored["lanes"], 1, "{mirrored}");

    let status = boxed.json(boxed.repo(), &["status", "--json"])?;
    let clients = status["clients"].as_array().cloned().unwrap_or_default();
    assert_eq!(clients.len(), 1, "the box lists one client: {status}");

    client.json(client.repo(), &["remote", "disconnect", "acme"])?;
    boxed.eventually("the box's tunnel to end", CONNECT, || {
        let status = boxed.json(boxed.repo(), &["status", "--json"])?;
        Ok(status.get("clients").is_none().then_some(()))
    })?;
    Ok(())
}
