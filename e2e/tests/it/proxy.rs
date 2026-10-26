//! HTTPS through the daemon's proxy to each lane's hostnames.

use std::process::Stdio;

use devlanes_e2e::{Lab, Result, lane, pid, unit};
use serde_json::Value;

use crate::{BUDGET, need};

/// Every routed hostname of two lanes answers 200 over HTTPS with the lab's CA, and the
/// answer comes from that lane's process.
#[test]
fn https_reaches_each_lane_by_hostname() -> Result<()> {
    if devlanes_e2e::curl()
        .arg("--version")
        .stdout(Stdio::null())
        .status()
        .map_or(true, |s| !s.success())
    {
        eprintln!("skipped: curl is not installed");
        return Ok(());
    }
    let lab = Lab::new("two-services", BUDGET)?;
    let feature = lab.add_worktree("feat/x")?;
    for cwd in [lab.repo().to_owned(), feature] {
        let started = lab.json(&cwd, &["start"])?;
        assert_eq!(started["ready"], true, "{started}");
    }

    let routes = lab.json(lab.repo(), &["url", "ls", "--json"])?;
    let https = &routes["proxy"]["https"];
    let Some(port) = https["port"].as_u64().filter(|_| https["bound"] == true) else {
        eprintln!(
            "skipped: the proxy could not bind HTTPS on {}, 443 or 8443: {https}",
            lab.https_port()
        );
        return Ok(());
    };
    let lanes = lab.lanes(lab.repo())?;
    let items = routes["items"].as_array().cloned().unwrap_or_default();
    assert_eq!(items.len(), 4, "{routes}");
    let ca = lab.home().join("ca/ca.pem");
    for route in &items {
        let host = need(route["host"].as_str(), "route host")?;
        let mut curl = devlanes_e2e::curl();
        curl.args(["-sS", "--max-time", "10", "--cacert"])
            .arg(&ca)
            .arg("--resolve")
            .arg(format!("{host}:{port}:127.0.0.1"))
            .args(["-w", "\n%{http_code}"])
            .arg(format!("https://{host}:{port}/healthz"));
        let output = lab.run(curl, devlanes_e2e::COMMAND_TIMEOUT)?;
        output.expect_code(0)?;
        let (body, code) = output
            .stdout
            .rsplit_once('\n')
            .unwrap_or(("", output.stdout.as_str()));
        assert_eq!(code, "200", "{host}: {output}");

        let reflected: Value = serde_json::from_str(body)?;
        let lane_ref = need(route["lane"].as_str(), "route lane")?;
        let unit_name = need(route["unit"].as_str(), "route unit")?;
        let target = need(
            lane(&lanes, lane_ref).and_then(|l| unit(l, unit_name)),
            "routed unit",
        )?;
        assert_eq!(
            reflected["pid"].as_i64(),
            pid(target).map(i64::from),
            "{host} must reach {lane_ref}:{unit_name}"
        );
        assert_eq!(reflected["headers"]["host"], format!("{host}:{port}"));
    }
    Ok(())
}

/// A port `devlanes.local.yaml` declares in `infra.external`, here an `exec`
/// unit's server, is listed as a unit of kind `external` and routed by the
/// proxy under the lane's domain; taking the entry out of the file takes the
/// route and the unit away again.
#[test]
fn an_external_port_is_routed_by_its_hostname() -> Result<()> {
    if devlanes_e2e::curl()
        .arg("--version")
        .stdout(Stdio::null())
        .status()
        .map_or(true, |s| !s.success())
    {
        eprintln!("skipped: curl is not installed");
        return Ok(());
    }
    let lab = Lab::new("two-services", BUDGET)?;
    let started = lab.json(lab.repo(), &["start"])?;
    assert_eq!(started["ready"], true, "{started}");
    let served = lab.spare_port(0);
    let served_text = served.to_string();
    lab.json(
        lab.repo(),
        &[
            "exec",
            "--name",
            "docs-server",
            "--",
            "fixture-server",
            "--port",
            &served_text,
        ],
    )?;
    lab.eventually(
        "the exec unit listens",
        devlanes_e2e::COMMAND_TIMEOUT,
        || Ok(devlanes_e2e::listening(served).then_some(())),
    )?;
    let local = lab.repo().join("devlanes.local.yaml");
    std::fs::write(
        &local,
        format!(
            "infra:\n  external:\n    docs:\n      address: 127.0.0.1:{served}\n      hosts: \"docs.${{{{ lane.domain }}}}\"\n"
        ),
    )?;
    let host = "docs.fixture-two.localhost";
    // The daemon reloads the config once its watcher sees the new file.
    let listed = lab.eventually(
        "the external unit to be listed",
        devlanes_e2e::COMMAND_TIMEOUT,
        || {
            let lanes = lab.lanes(lab.repo())?;
            Ok(lane(&lanes, "fixture-two@0")
                .and_then(|l| unit(l, "docs"))
                .cloned())
        },
    )?;
    let lanes = lab.lanes(lab.repo())?;
    assert_eq!(listed["kind"], "external", "{listed}");
    assert_eq!(listed["state"], "external", "{listed}");
    assert_eq!(listed["ports"]["default"], served, "{listed}");

    let routes = lab.json(lab.repo(), &["url", "ls", "--json"])?;
    let https = &routes["proxy"]["https"];
    let Some(port) = https["port"].as_u64().filter(|_| https["bound"] == true) else {
        eprintln!("skipped: the proxy could not bind HTTPS: {https}");
        return Ok(());
    };
    let ca = lab.home().join("ca/ca.pem");
    let mut curl = devlanes_e2e::curl();
    curl.args(["-sS", "--max-time", "10", "--cacert"])
        .arg(&ca)
        .arg("--resolve")
        .arg(format!("{host}:{port}:127.0.0.1"))
        .args(["-w", "\n%{http_code}"])
        .arg(format!("https://{host}:{port}/healthz"));
    let output = lab.run(curl, devlanes_e2e::COMMAND_TIMEOUT)?;
    output.expect_code(0)?;
    let (body, code) = output
        .stdout
        .rsplit_once('\n')
        .unwrap_or(("", output.stdout.as_str()));
    assert_eq!(code, "200", "{output}");
    // The answer comes from the exec unit's process, not from a service.
    let reflected: Value = serde_json::from_str(body)?;
    let docs = need(
        lane(&lanes, "fixture-two@0").and_then(|l| unit(l, "docs-server")),
        "exec unit",
    )?;
    assert_eq!(
        reflected["pid"].as_i64(),
        pid(docs).map(i64::from),
        "{host} must reach the exec unit"
    );

    std::fs::remove_file(&local)?;
    lab.eventually(
        "the external unit to go",
        devlanes_e2e::COMMAND_TIMEOUT,
        || {
            let lanes = lab.lanes(lab.repo())?;
            let gone = lane(&lanes, "fixture-two@0")
                .and_then(|l| unit(l, "docs"))
                .is_none();
            Ok(gone.then_some(()))
        },
    )?;
    let routes = lab.json(lab.repo(), &["url", "ls", "--json"])?;
    let still = routes["items"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["host"] == host));
    assert!(!still, "removed but still routed: {routes}");
    Ok(())
}
