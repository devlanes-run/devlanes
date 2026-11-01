//! Release checks against a local release server: the daemon reads the
//! registry of its install method, reports a newer release with
//! its bump and changelog notes, caches both, checks again on `update.check`
//! only when the last check is an hour old unless forced, and makes no request
//! when checks are off. Needs `curl`.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use devlanes_config::UpdateMode;
use devlanes_daemon::{
    Engine, EngineOptions, Handler, Session, integrations::Integrations, update::UpdateCheckOptions,
};
use devlanes_proto::{Context, Method, Request};
use devlanes_state::Store;
use devlanes_update::{InstallMethod, check::Urls};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::FakeRunner;

/// Paths a [`release_server`] was asked for, in order.
type Hits = Arc<Mutex<Vec<String>>>;

/// Serves each path of `routes` with its body (`404` for others) and records
/// the paths requested.
async fn release_server(routes: &[(&str, String)]) -> (String, Hits) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    let routes: Arc<BTreeMap<String, String>> = Arc::new(
        routes
            .iter()
            .map(|(path, body)| ((*path).to_owned(), body.clone()))
            .collect(),
    );
    let hits = Hits::default();
    let recorded = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let (routes, recorded) = (routes.clone(), recorded.clone());
            tokio::spawn(async move {
                let mut request = [0u8; 2048];
                let read = stream.read(&mut request).await.unwrap_or(0);
                let text = String::from_utf8_lossy(&request[..read]);
                let path = text.split_whitespace().nth(1).unwrap_or("").to_owned();
                recorded.lock().expect("hits").push(path.clone());
                let (status, body) = match routes.get(&path) {
                    Some(body) => ("200 OK", body.clone()),
                    None => ("404 Not Found", String::new()),
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (base, hits)
}

/// How often `path` was requested.
fn hits_of(hits: &Hits, path: &str) -> usize {
    hits.lock()
        .expect("hits")
        .iter()
        .filter(|p| *p == path)
        .count()
}

/// Options that check at once against `base` as installed by `method`.
fn checks(base: &str, method: InstallMethod) -> UpdateCheckOptions {
    UpdateCheckOptions {
        urls: Urls {
            github: format!("{base}/stable.json"),
            npm: format!("{base}/npm"),
            pypi: format!("{base}/pypi"),
            homebrew: format!("{base}/devlanes.rb"),
            changelog: format!("{base}/changelog/{{version}}"),
            releases: format!("{base}/releases"),
        },
        method: Some(method),
        off: None,
        first_delay: Duration::ZERO,
        interval: Duration::from_secs(3600),
        recheck_after: Duration::from_secs(3600),
        // Includes starting `curl`; see `CHECK_PATIENCE`.
        timeout: Duration::from_secs(30),
        exe_retained: true,
    }
}

/// A script install.
fn script() -> InstallMethod {
    InstallMethod::Script(PathBuf::from("/opt/acme/bin"))
}

/// An engine over `home` with `checks` and `updates.mode`, reconciled so
/// background work runs.
async fn engine(home: &Path, checks: UpdateCheckOptions, mode: UpdateMode) -> Engine {
    std::fs::create_dir_all(home).expect("home");
    let store = Store::open(home.join("state.db")).await.expect("store");
    let mut options = EngineOptions::new(home, home.join("d.sock"));
    options.watch = false;
    options.update_check = checks;
    options.user.updates.mode = mode;
    let integ = Integrations::with_runner(Arc::new(FakeRunner::default()));
    let engine = Engine::new(options, store, integ, CancellationToken::new());
    engine.reconcile().await.expect("reconcile");
    engine
}

/// `method` with `params` on a fresh local session, which must succeed.
async fn call(engine: &Engine, method: Method, params: Value) -> Value {
    try_call(engine, method, params).await.expect("call")
}

/// `method` with `params` on a fresh local session.
async fn try_call(
    engine: &Engine,
    method: Method,
    params: Value,
) -> Result<Value, devlanes_proto::RpcError> {
    let (session, _frames) = Session::local(1);
    let request = Request {
        id: 1,
        method,
        params,
        context: Context::default(),
    };
    engine.handle(request, session).await
}

/// `daemon.status` on a fresh local session.
async fn daemon_status(engine: &Engine) -> Value {
    call(engine, Method::DaemonStatus, json!({})).await
}

/// How long a test waits for the background check. Starting `curl` has taken
/// over 10 s on a Windows CI runner busy with the rest of the suite; the bound
/// only ends a test that is broken.
const CHECK_PATIENCE: Duration = Duration::from_secs(40);

/// Every test's own limit, above [`CHECK_PATIENCE`].
const LIMIT: Duration = Duration::from_secs(90);

/// Polls `daemon.status` until `done` holds, for up to [`CHECK_PATIENCE`].
async fn status_until(engine: &Engine, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + CHECK_PATIENCE;
    while tokio::time::Instant::now() < deadline {
        let status = daemon_status(engine).await;
        if done(&status) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    daemon_status(engine).await
}

/// The next patch release after the running version.
fn next_patch() -> String {
    let core = devlanes_version::VERSION
        .split(['-', '+'])
        .next()
        .expect("core");
    let parts: Vec<u64> = core
        .split('.')
        .map(|p| p.parse().expect("number"))
        .collect();
    format!("{}.{}.{}", parts[0], parts[1], parts[2] + 1)
}

/// A changelog with releases 99.0.0 and 98.0.0, and one older than any build.
fn changelog() -> String {
    "# devlanes\n\n## 99.0.0 — 2026-11-01\n\n### Minor changes\n\n- [abc1234](https://example.invalid/c) Removed `--acme`.\n  \n  Use `--other`.\n\n### Patch changes\n\n- [abc1234](https://example.invalid/c) Fixed a crash.\n\n## 98.0.0 — 2026-10-30\n\n### Patch changes\n\n- Added `acme`.\n\n## 0.0.1 — 2020-01-01\n\n### Patch changes\n\n- Old.\n".into()
}

#[tokio::test]
async fn newer_release_is_reported_with_notes_and_cached() {
    tokio::time::timeout(LIMIT, async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (base, hits) = release_server(&[
            ("/stable.json", json!({"version": "99.0.0"}).to_string()),
            ("/changelog/99.0.0", changelog()),
        ])
        .await;
        let engine = engine(dir.path(), checks(&base, script()), UpdateMode::Notify).await;
        let status = status_until(&engine, |s| s.get("update").is_some()).await;
        let update = &status["update"];
        assert_eq!(update["latest"], "99.0.0", "{status}");
        assert_eq!(update["bump"], "minor");
        assert_eq!(update["state"], "available");
        assert!(update["checked_at"].is_u64());
        assert_eq!(
            update["notes"],
            json!([
                {"version": "99.0.0", "level": "minor", "line": "Removed `--acme`."},
                {"version": "99.0.0", "level": "patch", "line": "Fixed a crash."},
                {"version": "98.0.0", "level": "patch", "line": "Added `acme`."},
            ])
        );
        let check = &status["release_check"];
        assert_eq!(check["state"], "ok");
        assert_eq!(check["mode"], "notify");
        assert_eq!(check["source"], "github");
        assert_eq!(check["latest"], "99.0.0");
        assert_eq!(check["bump"], "minor");
        assert_eq!(hits_of(&hits, "/stable.json"), 1);
        assert_eq!(hits_of(&hits, "/changelog/99.0.0"), 1);

        let cache: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("update-check.json")).expect("cache"),
        )
        .expect("cache json");
        assert_eq!(cache["latest"], "99.0.0");
        assert_eq!(cache["source"], format!("{base}/stable.json"));
        assert_eq!(cache["notes_for"], "99.0.0");
        assert_eq!(cache["notes"].as_array().map(Vec::len), Some(3));
    })
    .await
    .expect("timed out");
}

#[tokio::test]
async fn each_install_method_reads_its_registry() {
    tokio::time::timeout(LIMIT, async {
        let dir = tempfile::tempdir().expect("tempdir");
        let formula = "class Devlanes < Formula\n  on_macos do\n    url \"https://github.com/devlanes-run/devlanes/releases/download/v97.0.0/devlanes-97.0.0-aarch64-apple-darwin.tar.gz\"\n";
        let (base, hits) = release_server(&[
            ("/stable.json", json!({"version": "96.0.0"}).to_string()),
            ("/next.json", json!({"version": "99.0.0-next.3"}).to_string()),
            ("/npm", json!({"name": "devlanes", "version": "95.0.0"}).to_string()),
            ("/pypi", json!({"info": {"version": "94.0.0rc1"}}).to_string()),
            ("/devlanes.rb", formula.to_owned()),
        ])
        .await;
        let cases = [
            // The manifest a `next` build reads.
            ("script-next", script(), "/next.json", "github", "99.0.0-next.3"),
            ("cargo", InstallMethod::CargoInstall, "/stable.json", "github", "96.0.0"),
            ("pnpm", InstallMethod::Pnpm, "/stable.json", "npm", "95.0.0"),
            ("pipx", InstallMethod::Pipx, "/stable.json", "pypi", "94.0.0-rc.1"),
            ("brew", InstallMethod::Homebrew, "/stable.json", "homebrew", "97.0.0"),
        ];
        for (name, method, manifest, source, latest) in cases {
            let mut options = checks(&base, method);
            options.urls.github = format!("{base}{manifest}");
            let engine = engine(&dir.path().join(name), options, UpdateMode::Notify).await;
            let status = status_until(&engine, |s| s["release_check"]["state"] == "ok").await;
            let check = &status["release_check"];
            assert_eq!(check["source"], source, "{name}: {status}");
            assert_eq!(check["latest"], latest, "{name}: {status}");
            assert_eq!(status["update"]["latest"], latest, "{name}: {status}");
            // No changelog at the server: the release shows without notes.
            assert_eq!(status["update"]["notes"], json!([]), "{name}: {status}");
        }
        // A `next` build has no tag whose changelog could be read.
        assert_eq!(hits_of(&hits, "/changelog/99.0.0-next.3"), 0);
        assert_eq!(hits_of(&hits, "/changelog/96.0.0"), 1);
    })
    .await
    .expect("timed out");
}

#[tokio::test]
async fn patch_and_older_releases() {
    tokio::time::timeout(LIMIT, async {
        let dir = tempfile::tempdir().expect("tempdir");
        let patch = next_patch();
        let (base, _) = release_server(&[
            ("/npm", json!({"version": patch}).to_string()),
            ("/stable.json", json!({"version": "0.0.1"}).to_string()),
        ])
        .await;
        // npm installs devlanes itself, so `auto` mode would run it; notify
        // mode reads the same registry.
        let npm = engine(
            &dir.path().join("npm"),
            checks(&base, InstallMethod::Npm),
            UpdateMode::Notify,
        )
        .await;
        let status = status_until(&npm, |s| s.get("update").is_some()).await;
        assert_eq!(status["update"]["latest"], patch.as_str(), "{status}");
        assert_eq!(status["update"]["bump"], "patch");
        assert_eq!(status["update"]["state"], "available");
        assert_eq!(status["release_check"]["mode"], "notify");

        let older = engine(
            &dir.path().join("older"),
            checks(&base, script()),
            UpdateMode::Notify,
        )
        .await;
        let status = status_until(&older, |s| s["release_check"]["state"] == "ok").await;
        assert_eq!(status["release_check"]["latest"], "0.0.1", "{status}");
        assert!(status["release_check"].get("bump").is_none(), "{status}");
        assert!(status.get("update").is_none(), "{status}");
    })
    .await
    .expect("timed out");
}

/// Writes `home`'s cache file: release 98.0.0, read from `source` at
/// `checked_at`.
fn cache(home: &Path, checked_at: u64, source: &str) {
    std::fs::create_dir_all(home).expect("home");
    std::fs::write(
        home.join("update-check.json"),
        json!({"checked_at": checked_at, "latest": "98.0.0", "source": source}).to_string(),
    )
    .expect("cache");
}

/// Milliseconds since the epoch, an hour ago.
fn hour_ago() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("ms")
        - 3_600_000
}

#[tokio::test]
async fn failures_keep_the_cached_release() {
    tokio::time::timeout(LIMIT, async {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path()).expect("home");
        // Checked an hour ago: the daemon waits out the rest of the interval
        // unless the interval is shorter, as here.
        let checked_at = hour_ago();
        // Nothing is served: every request fails with 404.
        let (base, hits) = release_server(&[]).await;
        cache(dir.path(), checked_at, &format!("{base}/stable.json"));
        let mut options = checks(&base, script());
        options.interval = Duration::from_millis(100);
        let engine = engine(dir.path(), options, UpdateMode::Notify).await;
        let status = status_until(&engine, |s| s.get("update").is_some()).await;
        assert_eq!(status["update"]["latest"], "98.0.0", "{status}");
        let deadline = tokio::time::Instant::now() + CHECK_PATIENCE;
        while hits_of(&hits, "/stable.json") == 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(hits_of(&hits, "/stable.json") >= 1, "the check never ran");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let status = daemon_status(&engine).await;
        assert_eq!(status["update"]["latest"], "98.0.0", "{status}");
        assert_eq!(status["release_check"]["checked_at"], checked_at);
    })
    .await
    .expect("timed out");
}

/// A forced `update.check` that cannot read the registry fails instead of
/// answering with the cached release, or with none; an unforced one keeps
/// the cache.
#[tokio::test]
async fn a_forced_check_that_cannot_read_the_registry_fails() {
    tokio::time::timeout(LIMIT, async {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path()).expect("home");
        let (base, hits) = release_server(&[]).await;
        cache(
            dir.path(),
            hour_ago() - 1_000,
            &format!("{base}/stable.json"),
        );
        let mut options = checks(&base, script());
        options.first_delay = Duration::from_secs(3600);
        let engine = engine(dir.path(), options, UpdateMode::Notify).await;
        status_until(&engine, |s| s.get("update").is_some()).await;

        let error = try_call(&engine, Method::UpdateCheck, json!({"force": true}))
            .await
            .expect_err("a forced check fails");
        assert_eq!(error.code, "update_failed", "{error:?}");
        let title = error.data["title"].as_str().unwrap_or_default();
        assert!(
            title.starts_with("cannot read the latest release"),
            "{title}"
        );
        assert_eq!(error.data["hint"], "retry with devlanes update");
        assert_eq!(hits_of(&hits, "/stable.json"), 1);

        let update = call(&engine, Method::UpdateCheck, json!({})).await;
        assert_eq!(update["latest"], "98.0.0", "{update}");
        assert_eq!(hits_of(&hits, "/stable.json"), 2);
    })
    .await
    .expect("timed out");
}

#[tokio::test]
async fn a_cache_of_another_source_is_ignored() {
    tokio::time::timeout(LIMIT, async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (base, _) = release_server(&[]).await;
        let mut options = checks(&base, script());
        options.first_delay = Duration::from_secs(3600);
        // The answer of another registry, and of the `next` manifest, which a
        // stable build does not read.
        for other in ["npm", "next.json"] {
            let home = dir.path().join(other);
            cache(&home, hour_ago(), &format!("{base}/{other}"));
            let engine = engine(&home, options.clone(), UpdateMode::Notify).await;
            let status = status_until(&engine, |s| s["release_check"]["source"] == "github").await;
            assert_eq!(
                status["release_check"]["state"], "pending",
                "{other}: {status}"
            );
            assert!(status.get("update").is_none(), "{other}: {status}");
        }
    })
    .await
    .expect("timed out");
}

#[tokio::test]
async fn update_check_rechecks_after_an_hour_or_when_forced() {
    tokio::time::timeout(LIMIT, async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (base, hits) = release_server(&[
            ("/stable.json", json!({"version": "99.0.0"}).to_string()),
            ("/changelog/99.0.0", changelog()),
        ])
        .await;
        let mut options = checks(&base, script());
        // Only `update.check` reads the server.
        options.first_delay = Duration::from_secs(3600);
        let engine = engine(dir.path(), options, UpdateMode::Notify).await;

        let update = call(&engine, Method::UpdateCheck, json!({})).await;
        assert_eq!(update["latest"], "99.0.0", "{update}");
        assert_eq!(update, daemon_status(&engine).await["update"]);
        assert_eq!(hits_of(&hits, "/stable.json"), 1);

        // Checked moments ago: no request.
        let again = call(&engine, Method::UpdateCheck, json!({"force": false})).await;
        assert_eq!(again, update);
        assert_eq!(hits_of(&hits, "/stable.json"), 1);

        let forced = call(&engine, Method::UpdateCheck, json!({"force": true})).await;
        assert_eq!(forced["latest"], "99.0.0", "{forced}");
        assert_eq!(hits_of(&hits, "/stable.json"), 2);
        // The notes are read once per release.
        assert_eq!(hits_of(&hits, "/changelog/99.0.0"), 1);
        assert_eq!(forced["notes"], update["notes"]);
    })
    .await
    .expect("timed out");
}

#[tokio::test]
async fn update_check_with_an_hour_old_cache_checks_again() {
    tokio::time::timeout(LIMIT, async {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path()).expect("home");
        let (base, hits) =
            release_server(&[("/stable.json", json!({"version": "99.0.0"}).to_string())]).await;
        cache(
            dir.path(),
            hour_ago() - 1_000,
            &format!("{base}/stable.json"),
        );
        let mut options = checks(&base, script());
        options.first_delay = Duration::from_secs(3600);
        let engine = engine(dir.path(), options, UpdateMode::Notify).await;
        status_until(&engine, |s| s.get("update").is_some()).await;
        let update = call(&engine, Method::UpdateCheck, json!({})).await;
        assert_eq!(update["latest"], "99.0.0", "{update}");
        assert_eq!(hits_of(&hits, "/stable.json"), 1);
    })
    .await
    .expect("timed out");
}

#[tokio::test(flavor = "multi_thread")]
/// Through the socket, `daemon.status` carries the engine's release check for
/// a client of the same protocol, and the frozen fields alone for another.
async fn daemon_status_through_the_server_reports_the_release() {
    use devlanes_proto::{Hello, Outcome};

    tokio::time::timeout(LIMIT, async {
        let tmp = crate::temp();
        let (base, _) =
            release_server(&[("/stable.json", json!({"version": "99.0.0"}).to_string())]).await;
        let engine = engine(
            &tmp.path.join("home"),
            checks(&base, script()),
            UpdateMode::Notify,
        )
        .await;
        status_until(&engine, |s| s.get("update").is_some()).await;
        let options = devlanes_daemon::Options {
            home: tmp.path.join("srv"),
            socket: crate::endpoint(&tmp.path, "srv/run/d.sock"),
        };
        let socket = options.socket.clone();
        let server = devlanes_daemon::Server::bind(options).await.expect("bind");
        let token = server.shutdown_token();
        let handler: Arc<dyn Handler> = Arc::new(engine.clone());
        let running = tokio::spawn(server.run(handler, || Ok(())));

        let mut same = crate::server::client(&socket).await;
        let Outcome::Ok { data } = crate::server::request(
            &mut same,
            1,
            Method::DaemonStatus,
            json!({}),
            Context::default(),
        )
        .await
        else {
            panic!("daemon.status");
        };
        assert_eq!(data["update"]["latest"], "99.0.0", "{data}");
        assert_eq!(data["release_check"]["state"], "ok", "{data}");
        assert!(data["managed_processes"].is_u64() && data["socket"].is_string());

        let mut newer = Hello::current(0);
        newer.protocol += 1;
        let mut other = crate::server::client_with(&socket, newer).await;
        let Outcome::Ok { data } = crate::server::request(
            &mut other,
            2,
            Method::DaemonStatus,
            json!({}),
            Context::default(),
        )
        .await
        else {
            panic!("frozen daemon.status");
        };
        assert!(data.get("release_check").is_none(), "{data}");
        assert!(data.get("update").is_none(), "{data}");
        assert!(data["version"].is_string() && data["pid"].is_u64());

        token.cancel();
        running.await.expect("join").expect("clean shutdown");
    })
    .await
    .expect("timed out");
}

#[tokio::test]
async fn disabled_checks_make_no_request() {
    tokio::time::timeout(LIMIT, async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (base, hits) =
            release_server(&[("/stable.json", json!({"version": "99.0.0"}).to_string())]).await;
        let by_config = engine(
            &dir.path().join("config"),
            checks(&base, script()),
            UpdateMode::Disable,
        )
        .await;
        let mut options = checks(&base, script());
        options.off = Some("env");
        let by_env = engine(&dir.path().join("env"), options, UpdateMode::Notify).await;
        let local = engine(
            &dir.path().join("local"),
            checks(
                &base,
                InstallMethod::Local {
                    manifest: PathBuf::from("package.json"),
                },
            ),
            UpdateMode::Notify,
        )
        .await;
        let source = engine(
            &dir.path().join("source"),
            checks(&base, InstallMethod::Source),
            UpdateMode::Notify,
        )
        .await;
        for engine in [&by_config, &by_env, &local, &source] {
            let reply = call(engine, Method::UpdateCheck, json!({"force": true})).await;
            assert_eq!(reply, Value::Null);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(hits.lock().expect("hits").is_empty());
        let status = daemon_status(&by_config).await;
        assert_eq!(
            status["release_check"],
            json!({"state": "off", "reason": "config", "mode": "disable"})
        );
        assert!(status.get("update").is_none());
        assert_eq!(
            daemon_status(&by_env).await["release_check"]["reason"],
            "env"
        );
        assert_eq!(
            daemon_status(&local).await["release_check"]["reason"],
            "local"
        );
        assert_eq!(
            daemon_status(&source).await["release_check"]["reason"],
            "source"
        );
    })
    .await
    .expect("timed out");
}

/// `file:///...` for a local directory.
fn file_url(path: &Path) -> String {
    let path = path.display().to_string().replace('\\', "/");
    format!("file:///{}", path.trim_start_matches('/'))
}

/// Publishes `version` under `site/releases/download/v<version>/`: its
/// release archive, whose `devlanes` reports `version`, and `SHA256SUMS`. On
/// Windows that `devlanes.exe` is `fake-tool`, whose behaviour goes where
/// staging unpacks it under `home`, since the archive holds only release files.
fn publish(site: &Path, home: &Path, version: &str) {
    use devlanes_update::install::{TARGET, sha256_file};
    let top = format!("devlanes-{version}-{TARGET}");
    let work = site.join("work");
    let package = work.join(&top);
    std::fs::create_dir_all(&package).expect("package");
    std::fs::write(package.join("LICENSE"), "Apache-2.0").expect("license");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = package.join("devlanes");
        std::fs::write(&script, format!("#!/bin/sh\necho 'devlanes {version}'\n")).expect("script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("mode");
        std::fs::write(package.join("devlanes-hosts"), "helper").expect("helper");
    }
    #[cfg(windows)]
    {
        use devlanes_testing::fake_tool::{Behaviour, install, sidecar};
        let behaviour = Behaviour::new().stdout(format!("devlanes {version}\n"));
        let program = install(&work, "devlanes", &behaviour).expect("fake-tool");
        std::fs::rename(&program, package.join("devlanes.exe")).expect("move");
        let staged = home.join("update").join(version).join(&top);
        std::fs::create_dir_all(&staged).expect("staged");
        std::fs::rename(sidecar(&program), staged.join("devlanes.exe.json")).expect("behaviour");
    }
    #[cfg(unix)]
    let _ = home;
    let folder = site.join("releases/download").join(format!("v{version}"));
    std::fs::create_dir_all(&folder).expect("folder");
    #[cfg(unix)]
    let (suffix, tar, flags) = (".tar.gz", PathBuf::from("tar"), &["-czf"][..]);
    #[cfg(windows)]
    let (suffix, tar, flags) = (
        ".zip",
        PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
            .join(r"System32\tar.exe"),
        &["-a", "-cf"][..],
    );
    let name = format!("{top}{suffix}");
    let archive = folder.join(&name);
    let status = std::process::Command::new(tar)
        .args(flags)
        .arg(&archive)
        .arg("-C")
        .arg(&work)
        .arg(&top)
        .status()
        .expect("tar");
    assert!(status.success(), "tar");
    let sha = sha256_file(&archive).expect("sha");
    std::fs::write(folder.join("SHA256SUMS"), format!("{sha}  {name}\n")).expect("sums");
}

/// `auto` mode stages a patch release of a script install in the background
/// and reports it ready; a failed download is reported with its error. An
/// `update.check` a TUI sends on open does not try it again, even when it
/// reads the registry, nor does a forced one with `install: false`; a forced
/// one does. The installed binary stays as it is.
#[tokio::test]
async fn auto_mode_stages_a_patch_release_and_retries_a_failure() {
    tokio::time::timeout(LIMIT, async {
        let dir = tempfile::tempdir().expect("tempdir");
        let (home, site, bin) = (
            dir.path().join("home"),
            dir.path().join("site"),
            dir.path().join("bin"),
        );
        std::fs::create_dir_all(&bin).expect("bin");
        let installed = bin.join(format!("devlanes{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&installed, "old").expect("installed");
        let patch = next_patch();
        let (base, _) =
            release_server(&[("/stable.json", json!({"version": patch}).to_string())]).await;
        let mut options = checks(&base, InstallMethod::Script(bin.clone()));
        options.urls.releases = format!("{}/releases", file_url(&site));
        // Only `update.check` reads the server, every time it is called.
        options.first_delay = Duration::from_secs(3600);
        options.recheck_after = Duration::ZERO;
        let engine = engine(&home, options, UpdateMode::Auto).await;

        let update = call(&engine, Method::UpdateCheck, json!({"force": true})).await;
        assert_eq!(update["latest"], patch.as_str(), "{update}");
        let status = status_until(&engine, |s| s["update"]["state"] == "failed").await;
        let error = status["update"]["error"].as_str().unwrap_or_default();
        assert!(error.contains("SHA256SUMS"), "{status}");

        publish(&site, &home, &patch);
        let again = call(&engine, Method::UpdateCheck, json!({"force": false})).await;
        assert_eq!(again["state"], "failed", "{again}");
        tokio::time::sleep(Duration::from_secs(1)).await;
        let status = daemon_status(&engine).await;
        assert_eq!(status["update"]["state"], "failed", "{status}");
        assert!(devlanes_update::install::Staged::load(&home).is_none());

        // `devlanes update` checks with `install: false` and installs itself.
        let forced = json!({"force": true, "install": false});
        let held = call(&engine, Method::UpdateCheck, forced).await;
        assert_eq!(held["state"], "failed", "{held}");
        tokio::time::sleep(Duration::from_secs(1)).await;
        let status = daemon_status(&engine).await;
        assert_eq!(status["update"]["state"], "failed", "{status}");
        assert!(devlanes_update::install::Staged::load(&home).is_none());

        call(&engine, Method::UpdateCheck, json!({"force": true})).await;
        let status = status_until(&engine, |s| s["update"]["state"] == "ready").await;
        assert_eq!(status["update"]["installed"], patch.as_str(), "{status}");
        let staged = devlanes_update::install::Staged::load(&home).expect("staged");
        assert_eq!(staged.version, patch);
        assert_eq!(staged.target, bin);
        assert_eq!(std::fs::read(&installed).expect("installed"), b"old");
    })
    .await
    .expect("timed out");
}
