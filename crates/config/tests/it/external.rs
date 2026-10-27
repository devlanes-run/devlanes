//! `infra.external`: loopback addresses only, one port per entry, hostnames
//! like a service's, and per-key merging from devlanes.local.yaml.

use devlanes_config::*;

/// The message of the error `source` fails with.
fn refused(source: &str) -> String {
    parse_project(source, None).expect_err("refused").message
}

/// `127.0.0.1:<port>` and `[::1]:<port>` parse, with their port; hostnames
/// may be expressions and route mappings with `fallback` and `host_header`.
#[test]
fn loopback_entries_parse() {
    let config = parse_project(
        "project: acme\ninfra:\n  external:\n    pgadmin:\n      address: 127.0.0.1:5050\n      hosts: \"pgadmin.${{ lane.domain }}\"\n    docs:\n      address: '[::1]:8000'\n      hidden: true\n      hosts:\n        - docs.localhost\n        - { host: '*.docs.${{ lane.domain }}', fallback: true, host_header: upstream }\n",
        None,
    )
    .expect("valid");
    let pgadmin = &config.infra.external["pgadmin"];
    assert_eq!(
        pgadmin.socket_address(),
        Ok("127.0.0.1:5050".parse().expect("v4"))
    );
    assert_eq!(pgadmin.hidden, None);
    let docs = &config.infra.external["docs"];
    assert_eq!(docs.port(), Ok(8000));
    assert_eq!(docs.socket_address(), Ok("[::1]:8000".parse().expect("v6")));
    assert_eq!(docs.hidden, Some(true));
    assert_eq!(docs.hosts.as_deref().map(<[_]>::len), Some(2));
}

/// Anything but a loopback address with a port is a config error at the
/// address, and so is a missing one.
#[test]
fn only_loopback_addresses_are_accepted() {
    for (address, expected) in [
        ("0.0.0.0:5050", "not a loopback address"),
        ("192.168.1.20:5050", "not a loopback address"),
        ("'[::]:5050'", "not a loopback address"),
        ("127.0.0.2:5050", "not a loopback address"),
        (
            "localhost:5050",
            "`localhost:5050` names a host. Write `127.0.0.1:5050` or `[::1]:5050`",
        ),
        ("'[::1%2]:5050'", "has an IPv6 scope id"),
        ("'5050'", "not an address with a port"),
        ("127.0.0.1", "not an address with a port"),
        ("127.0.0.1:0", "needs a port from 1 to 65535"),
    ] {
        let message = refused(&format!(
            "project: acme\ninfra:\n  external:\n    tool:\n      address: {address}\n"
        ));
        assert!(
            message.starts_with("infra.external.tool.address: ") && message.contains(expected),
            "{address}: {message}"
        );
    }
    let message = refused("project: acme\ninfra:\n  external:\n    tool: { hidden: true }\n");
    assert!(message.contains("address is required"), "{message}");
}

/// Two entries on one port, an entry on a service's literal `PORT`, a name
/// another unit has, a per-unit expression or an unknown container in a
/// hostname, and an entry named in the container keys are refused, each at
/// its key; a route with `port` and an unknown key are parse errors.
#[test]
fn collisions_and_bad_hosts_are_refused() {
    let cases = [
        (
            "infra:\n  external:\n    a: { address: '127.0.0.1:5050' }\n    b: { address: '[::1]:5050' }\n",
            "infra.external.b.address: port 5050 is already infra.external.a's port",
        ),
        (
            "services:\n  api: { run: api, env: { PORT: 5050 } }\ninfra:\n  external:\n    a: { address: '127.0.0.1:5050' }\n",
            "services.api.env.PORT: port 5050 is already infra.external.a's port",
        ),
        (
            "services:\n  api: { run: api }\ninfra:\n  external:\n    api: { address: '127.0.0.1:5050' }\n",
            "infra.external.api: `api` is also a service",
        ),
        (
            "tasks:\n  seed: { run: seed }\ninfra:\n  external:\n    seed: { address: '127.0.0.1:5050' }\n",
            "infra.external.seed: `seed` is also a task",
        ),
        (
            "infra:\n  containers:\n    db: { image: postgres }\n  external:\n    db: { address: '127.0.0.1:5050' }\n",
            "infra.external.db: `db` is also a container",
        ),
        (
            "infra:\n  setup:\n    db: echo\n  external:\n    setup.db: { address: '127.0.0.1:5050' }\n",
            "infra.external.setup.db: `setup.db` is also an infra.setup entry's unit",
        ),
        (
            "infra:\n  teardown:\n    db: echo\n  external:\n    teardown.db: { address: '127.0.0.1:5050' }\n",
            "infra.external.teardown.db: `teardown.db` is also an infra.teardown entry's unit",
        ),
        (
            "infra:\n  external:\n    a: { address: '127.0.0.1:5050', hosts: 'a-${{ infra.nope.port }}.localhost' }\n",
            "infra.external.a.hosts: unknown container `nope`",
        ),
        (
            "infra:\n  external:\n    a: { address: '127.0.0.1:5050', hosts: 'a-${{ self.port }}.localhost' }\n",
            "infra.external.a.hosts: `self.port` is not defined for an external entry",
        ),
        (
            "infra:\n  external:\n    a: { address: '127.0.0.1:5050', hosts: 'bad_host.localhost' }\n",
            "infra.external.a.hosts:",
        ),
        (
            "infra:\n  hidden: [a]\n  external:\n    a: { address: '127.0.0.1:5050' }\n",
            "infra.hidden: `a` is an infra.external entry. Set `hidden: true` on it instead",
        ),
        (
            "infra:\n  hosts: { a: a.localhost }\n  external:\n    a: { address: '127.0.0.1:5050' }\n",
            "infra.hosts.a: `a` is an infra.external entry. Set its hostnames in its own `hosts`",
        ),
        (
            "infra:\n  per_lane: [a]\n  external:\n    a: { address: '127.0.0.1:5050' }\n",
            "infra.per_lane: `a` is an infra.external entry. per_lane only takes Compose services",
        ),
        (
            "infra:\n  exclude: [a]\n  external:\n    a: { address: '127.0.0.1:5050' }\n",
            "infra.exclude: `a` is an infra.external entry. exclude only takes Compose services",
        ),
    ];
    for (body, expected) in cases {
        let message = refused(&format!("project: acme\n{body}"));
        assert!(message.starts_with(expected), "{body}\n{message}");
    }
    for parse_error in [
        "a: { address: '127.0.0.1:5050', port: 5050 }",
        "a: { address: '127.0.0.1:5050', hosts: [{ host: a.localhost, port: http }] }",
    ] {
        let unknown = refused(&format!(
            "project: acme\ninfra:\n  external:\n    {parse_error}\n"
        ));
        assert!(unknown.contains("unknown"), "{parse_error}: {unknown}");
    }
}

/// devlanes.local.yaml cannot rename the project.
#[test]
fn local_files_keep_the_project_name() {
    let error =
        parse_project("project: acme\n", Some("project: other\n")).expect_err("renamed locally");
    assert_eq!(error.location.file, "devlanes.local.yaml", "{error:?}");
    assert_eq!(
        error.message,
        "project: set the project name in devlanes.yaml, not in devlanes.local.yaml"
    );
}

/// devlanes.local.yaml adds entries and changes single keys of entries the
/// base declares, keeping the rest; its errors point at the local file.
#[test]
fn local_files_add_and_change_entries() {
    let base = "project: acme\ninfra:\n  external:\n    pgadmin:\n      address: 127.0.0.1:5050\n      hosts: pgadmin.${{ lane.domain }}\n";
    let local = "infra:\n  external:\n    pgadmin:\n      hidden: true\n    docs:\n      address: 127.0.0.1:8000\n";
    let config = parse_project(base, Some(local)).expect("merged");
    let names: Vec<&str> = config.infra.external.keys().map(String::as_str).collect();
    assert_eq!(names, ["pgadmin", "docs"]);
    let pgadmin = &config.infra.external["pgadmin"];
    assert_eq!(pgadmin.port(), Ok(5050), "the address stays");
    assert!(pgadmin.hosts.is_some(), "the hostnames stay");
    assert_eq!(pgadmin.hidden, Some(true));

    let error = parse_project(
        base,
        Some("infra:\n  external:\n    pgadmin:\n      address: 10.0.0.1:5050\n"),
    )
    .expect_err("not loopback");
    assert_eq!(error.location.file, "devlanes.local.yaml", "{error:?}");
    assert!(
        error.message.contains("not a loopback address"),
        "{error:?}"
    );
}
