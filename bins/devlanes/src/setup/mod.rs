//! `devlanes setup`: the machine-level steps lanes rely on, run in the user's
//! terminal and never in the daemon, because they prompt for a password.
//!
//! - `ca`: trust the CA `tls.ca` selects in the stores `tls.trust` names.
//! - `hosts`: the `/etc/hosts` helper (macOS and Linux) when `*.localhost` does
//!   not resolve to loopback, the user config asks for the hosts file, or the
//!   daemon reports that a lane needs it.
//! - `resolver`: the resolver file (NRPT rule on Windows) for `hostnames.tld`
//!   under `hostnames.resolver: resolver`.
//! - `browser`: the native messaging host of the devlanes browser extension.
//!   It is never missing, so it never asks for setup on its own.
//!
//! [`plan`] decides from read-only [`Facts`] which steps are needed and done;
//! `setup` asks once and runs the missing ones, `--check` only reports, and
//! `--remove` undoes all of them.

mod browser;
mod ca;
mod hosts;
#[cfg(windows)]
mod nrpt;
#[cfg(unix)]
mod resolver;
#[cfg(unix)]
mod sudo;

use std::time::Duration;

use devlanes_cli::{OutputMode, Rendered, Runtime, SetupAction};
use devlanes_client::{Client, Paths};
use devlanes_config::UserConfig;
use devlanes_proto::{Context, Method, RpcError};
use devlanes_resolver_hosts::LocalhostResolution;
use serde_json::{Value, json};

pub(crate) use browser::doctor_report as browser_report;
pub(crate) use ca::{
    UNCONSTRAINED_CA, ca_tld, certificate, doctor_report, mkcert_caroot, mkcert_selected,
    recreate_ca_command, report,
};
pub(crate) use hosts::installed_helper;

use crate::output::emit;

/// What to run from a terminal to set this machine up.
pub(crate) const APPLY: &str = "devlanes setup";

/// What to run from a terminal to undo it.
pub(crate) const REMOVE: &str = "devlanes setup --remove";

/// Bound on the `*.localhost` probe, as at daemon start.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Longest the start notice and the TUI's setup prompt wait for the trust
/// stores; past it the CA is not mentioned rather than delaying the shell
/// prompt.
const CA_CHECK_TIMEOUT: Duration = Duration::from_millis(1500);

/// `{path, installed, matches, ...}` for the file (or NRPT rule) that sends
/// `tld` to the DNS server at `server`, as `doctor` and the plan read it.
/// Blocking.
pub(crate) fn resolver_status(tld: &str, server: std::net::SocketAddr) -> Value {
    #[cfg(unix)]
    {
        resolver::status(tld, server)
    }
    #[cfg(windows)]
    {
        nrpt::status(tld, server)
    }
}

/// One setup step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepKind {
    Ca,
    Hosts,
    Resolver,
    BrowserHost,
}

impl StepKind {
    /// The name `setup` prints and `daemon.status` reports in `setup.missing`.
    fn name(self) -> &'static str {
        match self {
            Self::Ca => "ca",
            Self::Hosts => "hosts",
            Self::Resolver => "resolver",
            Self::BrowserHost => "browser",
        }
    }

    /// Whether the step runs sudo on this OS with `config`, so `setup` asks
    /// for the password once up front. The CA needs it only for Linux's
    /// system trust store.
    fn sudo(self, config: &UserConfig) -> bool {
        match self {
            Self::Ca => ca::needs_root(config),
            Self::Hosts | Self::Resolver => cfg!(unix),
            Self::BrowserHost => false,
        }
    }

    /// Whether the step counts as missing when it isn't done. The browser
    /// host never does: lanes work without it, and agents rely on `--check`
    /// exiting 0 once the rest is done.
    fn counts(self) -> bool {
        self != Self::BrowserHost
    }
}

/// A step of the plan: `needed` on this machine, and `done` when it is.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Step {
    kind: StepKind,
    /// Whether the step applies to this machine. For the browser host,
    /// whether some browser holds or can take this install's registration.
    /// With all of them registered it is `done` and shows `ok`.
    needed: bool,
    done: bool,
    /// Why the step is needed and what is missing, what is in place, or why
    /// it is skipped.
    reason: String,
}

impl Step {
    /// `ok`, `todo` or `skip`, and `info` for a step that never counts but
    /// that setup still applies.
    fn state(&self) -> &'static str {
        match (self.needed, self.done) {
            (false, _) => "skip",
            (true, true) => "ok",
            (true, false) if self.kind.counts() => "todo",
            (true, false) => "info",
        }
    }

    /// Whether this step is missing, so `setup` asks for it and `--check`
    /// exits 1.
    fn todo(&self) -> bool {
        self.kind.counts() && self.applies()
    }

    /// Whether `setup` runs this step when it runs.
    fn applies(&self) -> bool {
        self.needed && !self.done
    }
}

/// Whether the configured stores trust the CA.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Trust {
    Trusted,
    /// The stores that do not.
    Untrusted(Vec<String>),
    /// The CA could not be read or a store could not tell.
    Unknown(String),
    /// devlanes' own CA does not exist yet.
    NoCa,
}

/// What the user config says about the hosts file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostsWanted {
    /// Only when `*.localhost` fails or a project needs it.
    Auto,
    /// `hostnames.hosts_file: never`.
    Never,
    /// Always, because of this setting.
    Always(&'static str),
}

/// The installed `/etc/hosts` helper.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Helper {
    Absent,
    Current,
    /// Installed but needs replacing, and why.
    Stale(String),
}

/// The `resolver` strategy and whether its TLD reaches the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Dns {
    /// Another strategy.
    Off,
    /// `hostnames.resolver: resolver` without `hostnames.tld`.
    NoTld,
    Routed {
        tld: String,
    },
    Unrouted {
        tld: String,
        reason: String,
    },
}

/// Everything [`plan`] decides from, gathered read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Facts {
    windows: bool,
    mkcert: bool,
    trust: Trust,
    localhost: LocalhostResolution,
    hosts: HostsWanted,
    helper: Helper,
    /// A running daemon reports `hosts` in `setup.missing`.
    daemon_hosts: bool,
    dns: Dns,
    browser: browser::Facts,
}

/// The steps for `facts`, in the order they run.
fn plan(facts: &Facts) -> Vec<Step> {
    vec![
        ca_step(facts),
        hosts_step(facts),
        resolver_step(&facts.dns),
        browser_step(&facts.browser),
    ]
}

/// The browser extension's host, registered whenever setup runs.
fn browser_step(facts: &browser::Facts) -> Step {
    let (needed, done, reason) = browser::plan(facts);
    Step {
        kind: StepKind::BrowserHost,
        needed,
        done,
        reason,
    }
}

/// The CA is always needed; mkcert's root is trusted the same way as devlanes' own.
fn ca_step(facts: &Facts) -> Step {
    let what = if facts.mkcert {
        "mkcert's CA"
    } else {
        "the devlanes CA"
    };
    let (done, reason) = match &facts.trust {
        Trust::Trusted => (
            true,
            format!("{what} is trusted by every store in tls.trust"),
        ),
        Trust::Untrusted(stores) => (
            false,
            format!("{what} is not trusted by {}", stores.join(", ")),
        ),
        Trust::Unknown(reason) => (
            false,
            format!("cannot tell whether {what} is trusted: {reason}"),
        ),
        Trust::NoCa => (
            false,
            "there is no devlanes CA yet; setup creates and trusts it".into(),
        ),
    };
    Step {
        kind: StepKind::Ca,
        needed: true,
        done,
        reason,
    }
}

/// The helper is needed where devlanes writes the hosts file (never on
/// Windows) and something asks for it; an installed one is kept current.
fn hosts_step(facts: &Facts) -> Step {
    let step = |needed, done, reason: String| Step {
        kind: StepKind::Hosts,
        needed,
        done,
        reason,
    };
    if facts.windows {
        let reason = if facts.localhost.works() {
            "Windows answers *.localhost itself"
        } else {
            "devlanes writes no hosts file on Windows"
        };
        return step(false, false, reason.into());
    }
    if facts.hosts == HostsWanted::Never {
        return step(false, false, "hostnames.hosts_file is never".into());
    }
    let why = match facts.hosts {
        HostsWanted::Always(setting) => Some(setting),
        _ if !facts.localhost.works() => Some("*.localhost does not resolve to loopback"),
        _ if facts.daemon_hosts => Some("a lane needs its hostnames in /etc/hosts"),
        _ => None,
    };
    match (why, &facts.helper) {
        (None, Helper::Stale(stale)) => step(true, false, stale.clone()),
        (None, _) => step(false, false, "*.localhost resolves to loopback".into()),
        (Some(why), Helper::Absent) => step(
            true,
            false,
            format!("{why}; the /etc/hosts helper is not installed"),
        ),
        (Some(why), Helper::Stale(stale)) => step(true, false, format!("{why}; {stale}")),
        (Some(why), Helper::Current) if facts.daemon_hosts => step(
            true,
            false,
            format!("{why}; the daemon cannot run the /etc/hosts helper"),
        ),
        (Some(why), Helper::Current) => step(
            true,
            true,
            format!("{why}; the /etc/hosts helper is installed"),
        ),
    }
}

/// The resolver file or NRPT rule, under the `resolver` strategy only.
fn resolver_step(dns: &Dns) -> Step {
    let (needed, done, reason) = match dns {
        Dns::Off => (false, false, "hostnames.resolver is not resolver".into()),
        Dns::NoTld => (
            false,
            false,
            "hostnames.resolver is resolver but hostnames.tld is not set".into(),
        ),
        Dns::Routed { tld } => (
            true,
            true,
            format!("*.{tld} goes to the devlanes DNS server"),
        ),
        Dns::Unrouted { tld, reason } => (
            true,
            false,
            format!("*.{tld} does not reach the devlanes DNS server: {reason}"),
        ),
    };
    Step {
        kind: StepKind::Resolver,
        needed,
        done,
        reason,
    }
}

/// The trust of a loaded CA from [`ca::store_report`]'s entries.
fn trust_of(report: &Value) -> Trust {
    let entries = report.as_array().map(Vec::as_slice).unwrap_or_default();
    let store = |entry: &Value| entry["store"].as_str().unwrap_or("?").to_owned();
    let untrusted: Vec<String> = entries
        .iter()
        .filter(|entry| entry["state"] == "untrusted")
        .map(store)
        .collect();
    if !untrusted.is_empty() {
        return Trust::Untrusted(untrusted);
    }
    let unknown: Vec<String> = entries
        .iter()
        .filter(|entry| entry["state"] != "trusted")
        .map(|entry| {
            format!(
                "{}: {}",
                store(entry),
                entry["reason"].as_str().unwrap_or("unknown")
            )
        })
        .collect();
    if unknown.is_empty() {
        Trust::Trusted
    } else {
        Trust::Unknown(unknown.join("; "))
    }
}

/// The hosts file setting of the user config; `resolver: hosts_file` wins
/// over `hosts_file`, as in the daemon.
fn hosts_wanted(config: &UserConfig) -> HostsWanted {
    let hostnames = &config.hostnames;
    if hostnames.resolver == devlanes_config::Resolver::HostsFile {
        return HostsWanted::Always("hostnames.resolver is hosts_file");
    }
    match hostnames.hosts_file {
        Some(devlanes_config::HostsFile::Always) => {
            HostsWanted::Always("hostnames.hosts_file is always")
        }
        Some(devlanes_config::HostsFile::Never) => HostsWanted::Never,
        _ => HostsWanted::Auto,
    }
}

/// The installed helper from `devlanes_daemon::hosts::helper_check`, `None`
/// when no helper is installed.
fn helper_of(check: Option<&Value>) -> Helper {
    let Some(check) = check else {
        return Helper::Absent;
    };
    match check["state"].as_str() {
        Some("ok") => Helper::Current,
        Some("missing") => Helper::Absent,
        Some("outdated") => Helper::Stale(format!(
            "the installed helper is revision {} and devlanes needs {}",
            check["revision"], check["required"]
        )),
        Some("incompatible") => {
            Helper::Stale("the installed helper speaks another protocol".into())
        }
        _ => Helper::Stale("the installed helper cannot run".into()),
    }
}

/// `hostnames.tld` under the `resolver` strategy.
fn resolver_tld(config: &UserConfig) -> Option<String> {
    let hostnames = &config.hostnames;
    (hostnames.resolver == devlanes_config::Resolver::Resolver)
        .then(|| hostnames.tld.clone())
        .flatten()
        .filter(|tld| !tld.is_empty())
}

/// The resolver fact for `tld` from [`resolver_status`].
fn dns_of(tld: String, status: &Value) -> Dns {
    if status["matches"] == true {
        return Dns::Routed { tld };
    }
    let path = status["path"].as_str().unwrap_or("the resolver file");
    let reason = match status["error"].as_str() {
        Some(error) => error.to_owned(),
        None if status["group_policy"] == true => {
            "Group Policy sets NRPT rules, so Windows ignores local ones".into()
        }
        None if status["installed"] == true => format!("{path} points elsewhere"),
        None => format!("{path} is missing"),
    };
    Dns::Unrouted { tld, reason }
}

/// Reads everything the plan needs without changing or prompting for anything.
/// The browser registrations are read at `location`.
async fn gather(paths: &Paths, config: &UserConfig, location: Option<&browser::Location>) -> Facts {
    let hosts = hosts_wanted(config);
    let trust = async {
        match ca::certificate(paths, config, false).await {
            Ok(Some(certificate)) => trust_of(&ca::store_report(config, &certificate).await),
            Ok(None) => Trust::NoCa,
            Err(error) => Trust::Unknown(
                error.data["message"]
                    .as_str()
                    .unwrap_or(&error.code)
                    .to_owned(),
            ),
        }
    };
    // `never` makes the probe irrelevant on macOS and Linux; Windows probes
    // for its warning.
    let probe = async {
        if hosts == HostsWanted::Never && cfg!(unix) {
            LocalhostResolution::Loopback
        } else {
            devlanes_resolver_hosts::probe_localhost(PROBE_TIMEOUT).await
        }
    };
    let dns = async {
        let Some(tld) = resolver_tld(config) else {
            return if config.hostnames.resolver == devlanes_config::Resolver::Resolver {
                Dns::NoTld
            } else {
                Dns::Off
            };
        };
        let lookup = tld.clone();
        let status = tokio::task::spawn_blocking(move || {
            resolver_status(&lookup, devlanes_resolver_dns::default_listen())
        })
        .await
        .unwrap_or_else(|error| json!({"error": error.to_string()}));
        dns_of(tld, &status)
    };
    let browser = {
        let location = location.cloned();
        async move {
            tokio::task::spawn_blocking(move || {
                browser::facts(location.as_ref(), browser::Install::current())
            })
            .await
            .unwrap_or(browser::Facts::NoHome)
        }
    };
    let (trust, localhost, helper, missing, dns, browser) = tokio::join!(
        trust,
        probe,
        hosts::installed_helper(),
        daemon_missing(paths),
        dns,
        browser
    );
    Facts {
        windows: cfg!(windows),
        mkcert: ca::mkcert_selected(config),
        trust,
        localhost,
        hosts,
        helper: helper_of(helper.as_ref()),
        daemon_hosts: missing.iter().any(|step| step == "hosts"),
        dns,
        browser,
    }
}

/// `setup.missing` of a running daemon; nothing when none runs. Never starts one.
async fn daemon_missing(paths: &Paths) -> Vec<String> {
    let Ok(mut client) = Client::connect(paths).await else {
        return Vec::new();
    };
    missing_of(
        &client
            .request(Method::DaemonStatus, json!({}), Context::default())
            .await
            .unwrap_or_default(),
    )
}

/// The step names in a `daemon.status` reply's `setup.missing`.
fn missing_of(status: &Value) -> Vec<String> {
    status
        .pointer("/setup/missing")
        .and_then(Value::as_array)
        .map(|missing| {
            missing
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Runs `devlanes setup`, `--check` or `--remove`.
pub async fn run(runtime: &Runtime, action: SetupAction) -> Result<u8, RpcError> {
    let paths = Paths::discover()
        .map_err(|error| RpcError::new("io", json!({"message": error.to_string()}), 1))?;
    run_in(
        runtime,
        &paths,
        browser::Location::current().as_ref(),
        action,
    )
    .await
}

/// [`run`] for the devlanes home at `paths`, with the browser registrations
/// at `location`.
async fn run_in(
    runtime: &Runtime,
    paths: &Paths,
    location: Option<&browser::Location>,
    action: SetupAction,
) -> Result<u8, RpcError> {
    let yes = match action {
        SetupAction::Remove { keep_ca } => {
            return remove_in(runtime, paths, location, keep_ca).await;
        }
        SetupAction::Check => None,
        SetupAction::Apply { yes } => Some(yes),
    };
    // Every step depends on the config; a partial read could pick another CA,
    // other stores or another TLD than the file names.
    let config = crate::user_config::load(paths).await?;
    let facts = gather(paths, &config, location).await;
    let steps = plan(&facts);
    #[cfg(windows)]
    if let Some(warning) = hosts::windows_warning(facts.localhost) {
        emit(&Rendered {
            stderr: warning,
            ..Rendered::default()
        });
    }
    let Some(yes) = yes else {
        show(runtime, &steps, runtime.mode == OutputMode::Json);
        return Ok(check_code(&steps));
    };
    show(runtime, &steps, false);
    let runs: Vec<&Step> = steps.iter().filter(|step| step.applies()).collect();
    let password = runs.iter().any(|step| step.kind.sudo(&config));
    match decide(&steps, yes) {
        Decision::Nothing => return Ok(0),
        Decision::Run => {}
        Decision::Ask { missing, browser } => {
            if !ask(runtime, missing, browser, password).await? {
                return Ok(1);
            }
        }
    }
    #[cfg(unix)]
    if password {
        sudo::refresh(runtime).await?;
    }
    // A failed step is reported and the next one still runs.
    let mut code = 0;
    for step in runs {
        let result = match step.kind {
            StepKind::Ca => ca::install(runtime, paths, &config).await,
            #[cfg(unix)]
            StepKind::Hosts => hosts::install(runtime).await,
            // The plan never needs the helper on Windows.
            #[cfg(windows)]
            StepKind::Hosts => Ok(0),
            StepKind::Resolver => {
                let result = match &facts.dns {
                    #[cfg(unix)]
                    Dns::Unrouted { tld, .. } => resolver::install(runtime, paths, tld).await,
                    #[cfg(windows)]
                    Dns::Unrouted { tld, .. } => nrpt::install(runtime, paths, tld).await,
                    _ => Ok(0),
                };
                recheck_resolver(paths).await;
                result
            }
            StepKind::BrowserHost => match (&facts.browser, location) {
                (
                    browser::Facts::Found {
                        install,
                        registrations,
                        ..
                    },
                    Some(location),
                ) => browser::install(runtime, location, install, registrations).await,
                // The plan applies it only with a home and an install it takes.
                _ => Ok(0),
            },
        };
        fold(runtime, &mut code, result);
    }
    Ok(code)
}

/// What a plain `setup` does with its plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    /// No step applies.
    Nothing,
    /// Run the steps that apply without a question.
    Run,
    /// Ask once about `missing` steps, and the browser host with them when
    /// `browser`.
    Ask { missing: usize, browser: bool },
}

/// Whether `setup` asks before it runs `steps`. A step that never counts is
/// asked about only with the missing ones, and runs without a question when
/// it is alone, so it never needs `--yes`. `yes` skips the question.
fn decide(steps: &[Step], yes: bool) -> Decision {
    let missing = steps.iter().filter(|step| step.todo()).count();
    let runs = steps.iter().filter(|step| step.applies()).count();
    match (runs, missing) {
        (0, _) => Decision::Nothing,
        (_, 0) => Decision::Run,
        _ if yes => Decision::Run,
        _ => Decision::Ask {
            missing,
            browser: runs > missing,
        },
    }
}

/// `--check`'s exit status: 1 while a step is missing, else 0. A step that
/// never counts doesn't change it.
fn check_code(steps: &[Step]) -> u8 {
    u8::from(steps.iter().any(Step::todo))
}

/// Has a running daemon read the resolver file (NRPT rule on Windows)
/// again, which `daemon.status` otherwise keeps from its first read; the
/// existing `hosts.retry` is that signal. Never starts a daemon.
async fn recheck_resolver(paths: &Paths) {
    if let Ok(mut client) = Client::connect(paths).await {
        let _ = client
            .request(Method::HostsRetry, json!({}), Context::default())
            .await;
    }
}

/// One change `setup --remove` makes on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Removal {
    /// `hosts`, `resolver`, `browser` or `ca`.
    pub(crate) step: &'static str,
    /// What changes, or why the step will fail.
    pub(crate) what: String,
    /// Whether it runs sudo.
    pub(crate) sudo: bool,
    /// Whether this is something `--remove` leaves in place, such as another
    /// install's browser registration, listed so the user knows it stays.
    pub(crate) kept: bool,
}

/// What `setup --remove` changes on this machine, and what it leaves in
/// place, without changing anything or prompting; steps with nothing to undo
/// are left out.
pub(crate) async fn removal_plan(paths: &Paths, keep_ca: bool) -> Vec<Removal> {
    removal_plan_in(paths, browser::Location::current().as_ref(), keep_ca).await
}

/// [`removal_plan`] with the browser registrations at `location`.
async fn removal_plan_in(
    paths: &Paths,
    location: Option<&browser::Location>,
    keep_ca: bool,
) -> Vec<Removal> {
    let config = crate::user_config::load(paths).await;
    let item = |step: &'static str, what: String, sudo: bool| Removal {
        step,
        what,
        sudo,
        kept: false,
    };
    let failed = |error: RpcError| {
        error.data["message"]
            .as_str()
            .unwrap_or(&error.code)
            .to_owned()
    };
    let mut plan = Vec::new();
    #[cfg(unix)]
    {
        if let Some(what) = hosts::removal().await {
            plan.push(item(StepKind::Hosts.name(), what, true));
        }
        let tld = config.as_ref().ok().and_then(resolver_tld);
        match resolver::removal(paths, tld).await {
            Ok(files) => plan.extend(files.into_iter().map(|file| {
                item(
                    StepKind::Resolver.name(),
                    format!("remove {}", file.display()),
                    true,
                )
            })),
            Err(error) => plan.push(item(StepKind::Resolver.name(), failed(error), true)),
        }
    }
    #[cfg(windows)]
    match nrpt::removal().await {
        Ok(keys) => plan.extend(
            keys.into_iter()
                .map(|key| item(StepKind::Resolver.name(), format!("remove {key}"), false)),
        ),
        Err(error) => plan.push(item(StepKind::Resolver.name(), failed(error), false)),
    }
    plan.extend(
        browser::removal_lines(location)
            .await
            .into_iter()
            .map(|(what, kept)| Removal {
                kept,
                ..item(StepKind::BrowserHost.name(), what, false)
            }),
    );
    if !keep_ca {
        match config {
            Ok(config) => match ca::removal(paths, &config).await {
                Ok(Some(what)) => {
                    plan.push(item(StepKind::Ca.name(), what, ca::needs_root(&config)))
                }
                Ok(None) => {}
                Err(error) => plan.push(item(StepKind::Ca.name(), failed(error), false)),
            },
            Err(error) => plan.push(item(
                StepKind::Ca.name(),
                format!("fails while config.yaml does not load: {}", failed(error)),
                false,
            )),
        }
    }
    plan
}

/// `setup --remove`: the hosts helper, the resolver file (or NRPT rule) and
/// the browser host first, so a CA removal failure (a cancelled keychain
/// prompt) still leaves them removed, then the CA unless `keep_ca`. sudo asks
/// for the password once up front when a step needs it. A failed step is
/// reported and the next one still runs.
pub(crate) async fn remove(
    runtime: &Runtime,
    paths: &Paths,
    keep_ca: bool,
) -> Result<u8, RpcError> {
    remove_in(
        runtime,
        paths,
        browser::Location::current().as_ref(),
        keep_ca,
    )
    .await
}

/// [`remove`] with the browser registrations at `location`.
async fn remove_in(
    runtime: &Runtime,
    paths: &Paths,
    location: Option<&browser::Location>,
    keep_ca: bool,
) -> Result<u8, RpcError> {
    #[cfg(unix)]
    if removal_plan_in(paths, location, keep_ca)
        .await
        .iter()
        .any(|item| item.sudo)
    {
        sudo::refresh(runtime).await?;
    }
    let config = crate::user_config::load(paths).await;
    let mut code = 0;
    #[cfg(unix)]
    {
        fold(runtime, &mut code, hosts::remove(runtime).await);
        // A broken config still lets the record of the last install name the file.
        let tld = config.as_ref().ok().and_then(resolver_tld);
        fold(
            runtime,
            &mut code,
            resolver::remove(runtime, paths, tld).await,
        );
    }
    #[cfg(windows)]
    fold(runtime, &mut code, nrpt::remove(runtime, paths).await);
    recheck_resolver(paths).await;
    fold(runtime, &mut code, browser::remove(runtime, location).await);
    if !keep_ca {
        let result = match config {
            Ok(config) => ca::remove(runtime, paths, &config).await,
            Err(error) => Err(error),
        };
        fold(runtime, &mut code, result);
    }
    Ok(code)
}

/// Folds one step's result into the exit status (the worst one wins) and
/// reports a failure, so the caller can go on with the next step.
fn fold(runtime: &Runtime, code: &mut u8, result: Result<u8, RpcError>) {
    match result {
        Ok(step) => *code = (*code).max(step),
        Err(error) => {
            *code = (*code).max(error.exit_code);
            let hint = crate::user_config::hint(&error);
            emit(&runtime.reporter().error(&error, hint.as_ref()));
        }
    }
}

/// Prints one line per step, or the plan as JSON; nothing with `--quiet`.
fn show(runtime: &Runtime, steps: &[Step], json: bool) {
    if runtime.quiet {
        return;
    }
    let stdout = if json {
        let missing: Vec<&str> = steps
            .iter()
            .filter(|step| step.todo())
            .map(|step| step.kind.name())
            .collect();
        let steps: Vec<Value> = steps
            .iter()
            .map(|step| json!({"step": step.kind.name(), "state": step.state(), "reason": step.reason}))
            .collect();
        format!("{}\n", json!({"steps": steps, "missing": missing}))
    } else {
        lines(steps)
    };
    emit(&Rendered {
        stdout,
        ..Rendered::default()
    });
}

/// `ok    ca        the devlanes CA is trusted by every store in tls.trust`.
fn lines(steps: &[Step]) -> String {
    steps
        .iter()
        .map(|step| {
            format!(
                "{:<4}  {:<8}  {}\n",
                step.state(),
                step.kind.name(),
                step.reason
            )
        })
        .collect()
}

/// Asks once whether to run `count` missing steps, and the browser host
/// with them when `browser`, yes by default. Without a terminal to ask on it
/// refuses, so agents and CI pass `--yes`.
async fn ask(runtime: &Runtime, count: usize, browser: bool, sudo: bool) -> Result<bool, RpcError> {
    if !runtime.can_prompt() {
        return Err(RpcError::new(
            "confirmation_required",
            json!({"flag": "--yes"}),
            2,
        ));
    }
    let question = question(count, browser, sudo);
    tokio::task::spawn_blocking(move || {
        eprint!("{question}");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer).map(|_| answer)
    })
    .await
    .map_err(|error| RpcError::new("io", json!({"message": error.to_string()}), 1))?
    .map(|answer| !matches!(answer.trim(), "n" | "N" | "no" | "No" | "NO"))
    .map_err(|error| RpcError::new("io", json!({"message": error.to_string()}), 1))
}

/// `Do the 2 missing steps? sudo may ask for your password once. [Y/n] `,
/// with `and register the browser extension host` after the count when
/// `browser`.
fn question(count: usize, browser: bool, sudo: bool) -> String {
    let steps = if count == 1 { "step" } else { "steps" };
    let steps = if browser {
        format!("{steps} and register the browser extension host")
    } else {
        steps.to_owned()
    };
    let note = if sudo {
        " sudo may ask for your password once."
    } else if cfg!(windows) {
        " Windows asks you to confirm each change."
    } else {
        ""
    };
    format!("Do the {count} missing {steps}?{note} [Y/n] ")
}

/// After a `start` on a terminal: one stderr line when this machine misses a
/// setup step that one of the lane's URLs needs, by the TUI's rules
/// ([`devlanes_client::setup::needed`]): the CA for an `https://` address,
/// the hosts helper or resolver for a hostname. At most once a day per set
/// of those steps, shared with the TUI's prompt. The daemon reports `hosts`
/// and `resolver`; the trust stores belong to this user, so the CA is
/// checked here, read-only and bounded.
pub(crate) async fn start_notice(
    client: &mut Client,
    runtime: &Runtime,
    paths: &Paths,
    data: &Value,
) {
    if runtime.mode != OutputMode::Human
        || !runtime.stdout_tty
        || runtime.quiet
        || runtime.ci.is_some()
    {
        return;
    }
    let urls = unit_urls(data);
    if urls.is_empty() {
        return;
    }
    let status = client
        .request(Method::DaemonStatus, json!({}), Context::default())
        .await
        .unwrap_or_default();
    let mut missing = client_missing(paths).await;
    missing.extend(missing_of(&status));
    let Some((key, line)) = notice(&missing, &urls) else {
        return;
    };
    let home = paths.home.clone();
    let due = tokio::task::spawn_blocking(move || {
        let mut log = devlanes_update::NoticeLog::load(&home);
        let now = devlanes_update::now_ms();
        if !log.due(&key, now) {
            return false;
        }
        log.mark_shown(&key, now);
        // A lost write only repeats the notice sooner.
        let _ = log.save(&home);
        true
    })
    .await
    .unwrap_or(false);
    if due {
        emit(&Rendered {
            stderr: line,
            ..Rendered::default()
        });
    }
}

/// The setup steps only this user can check, by the names `daemon.status`
/// gives them in `setup.missing`: `ca` when the configured stores do not
/// trust the CA. Read-only and bounded by [`CA_CHECK_TIMEOUT`]. A CA that
/// does not exist yet is not missing here: the daemon creates it at start.
pub(crate) async fn client_missing(paths: &Paths) -> Vec<String> {
    let untrusted = tokio::time::timeout(CA_CHECK_TIMEOUT, async {
        let config = crate::user_config::lenient(paths).await;
        matches!(
            trust_of(&ca::report(paths, &config).await),
            Trust::Untrusted(_)
        )
    })
    .await
    .unwrap_or(false);
    if untrusted {
        vec![StepKind::Ca.name().to_owned()]
    } else {
        Vec::new()
    }
}

/// The URLs of the units in a `start` result, in order.
fn unit_urls(data: &Value) -> Vec<&str> {
    data.get("units")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|unit| unit.get("urls").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// The notice log key (`setup:<steps>`) and the line for the steps of
/// `missing` that one of `urls` needs, naming the host of the first URL that
/// needs one; `None` when none does.
fn notice(missing: &[String], urls: &[&str]) -> Option<(String, String)> {
    let (steps, subject) = devlanes_client::setup::needed(missing, urls.iter().copied());
    let subject = subject?;
    Some((
        format!("setup:{}", steps.join(",")),
        notice_line(subject, &steps),
    ))
}

/// `acme.localhost needs devlanes setup (CA not trusted)`.
fn notice_line(subject: &str, missing: &[String]) -> String {
    let reasons: Vec<&str> = missing
        .iter()
        .map(|step| match step.as_str() {
            "ca" => "CA not trusted",
            "hosts" => "/etc/hosts helper missing",
            "resolver" => "DNS for hostnames.tld not set up",
            other => other,
        })
        .collect();
    format!("{subject} needs devlanes setup ({})\n", reasons.join(", "))
}

#[cfg(test)]
mod tests {
    use LocalhostResolution::{Fails, Loopback, NotLoopback};

    use super::*;

    /// Every value each fact can take that changes the plan.
    fn every_facts() -> Vec<Facts> {
        let mut all = Vec::new();
        for windows in [false, true] {
            for mkcert in [false, true] {
                for trust in [
                    Trust::Trusted,
                    Trust::Untrusted(vec!["system".into(), "firefox".into()]),
                    Trust::Unknown("firefox: certutil is missing".into()),
                    Trust::NoCa,
                ] {
                    for localhost in [Loopback, NotLoopback, Fails] {
                        for hosts in [
                            HostsWanted::Auto,
                            HostsWanted::Never,
                            HostsWanted::Always("hostnames.hosts_file is always"),
                        ] {
                            for helper in [
                                Helper::Absent,
                                Helper::Current,
                                Helper::Stale("the installed helper cannot run".into()),
                            ] {
                                for daemon_hosts in [false, true] {
                                    for dns in [
                                        Dns::Off,
                                        Dns::NoTld,
                                        Dns::Routed { tld: "test".into() },
                                        Dns::Unrouted {
                                            tld: "test".into(),
                                            reason: "/etc/resolver/test is missing".into(),
                                        },
                                    ] {
                                        all.push(Facts {
                                            windows,
                                            mkcert,
                                            trust: trust.clone(),
                                            localhost,
                                            hosts,
                                            helper: helper.clone(),
                                            daemon_hosts,
                                            dns,
                                            browser: browser::Facts::NoHome,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        all
    }

    /// Every combination of OS, CA, probe, config, helper, daemon report and
    /// resolver: what each step needs and whether it is done.
    #[test]
    fn plan_covers_every_combination() {
        for facts in every_facts() {
            let steps = plan(&facts);
            let kinds: Vec<StepKind> = steps.iter().map(|step| step.kind).collect();
            assert_eq!(
                kinds,
                [
                    StepKind::Ca,
                    StepKind::Hosts,
                    StepKind::Resolver,
                    StepKind::BrowserHost
                ],
                "{facts:?}"
            );
            let [ca, hosts, resolver, _] = &steps[..] else {
                unreachable!()
            };

            assert!(ca.needed, "{facts:?}");
            assert_eq!(ca.done, facts.trust == Trust::Trusted, "{facts:?}");

            let hosts_needed = !facts.windows
                && facts.hosts != HostsWanted::Never
                && (matches!(facts.hosts, HostsWanted::Always(_))
                    || !facts.localhost.works()
                    || facts.daemon_hosts
                    || matches!(facts.helper, Helper::Stale(_)));
            assert_eq!(hosts.needed, hosts_needed, "{facts:?}");
            assert_eq!(
                hosts.done,
                hosts_needed && facts.helper == Helper::Current && !facts.daemon_hosts,
                "{facts:?}"
            );

            assert_eq!(
                resolver.needed,
                matches!(facts.dns, Dns::Routed { .. } | Dns::Unrouted { .. }),
                "{facts:?}"
            );
            assert_eq!(
                resolver.done,
                matches!(facts.dns, Dns::Routed { .. }),
                "{facts:?}"
            );

            for step in &steps {
                assert!(!step.reason.is_empty(), "{step:?}");
                assert!(!step.reason.contains(['\n', '\u{2014}']), "{step:?}");
                assert!(!(step.done && !step.needed), "{step:?}");
            }
        }
    }

    /// A fresh macOS or Linux machine where `*.localhost` works.
    fn fresh() -> Facts {
        Facts {
            windows: false,
            mkcert: false,
            trust: Trust::NoCa,
            localhost: Loopback,
            hosts: HostsWanted::Auto,
            helper: Helper::Absent,
            daemon_hosts: false,
            dns: Dns::Off,
            browser: browser::Facts::Refused("pipx run runs devlanes from a cache".into()),
        }
    }

    #[test]
    fn lines_say_what_is_missing_and_why() {
        assert_eq!(
            lines(&plan(&fresh())),
            "todo  ca        there is no devlanes CA yet; setup creates and trusts it\n\
             skip  hosts     *.localhost resolves to loopback\n\
             skip  resolver  hostnames.resolver is not resolver\n\
             skip  browser   pipx run runs devlanes from a cache\n"
        );
        let facts = Facts {
            mkcert: true,
            trust: Trust::Untrusted(vec!["firefox".into()]),
            localhost: Fails,
            helper: Helper::Stale("the installed helper is revision 1 and devlanes needs 2".into()),
            dns: Dns::Unrouted {
                tld: "test".into(),
                reason: "/etc/resolver/test is missing".into(),
            },
            ..fresh()
        };
        assert_eq!(
            lines(&plan(&facts)),
            "todo  ca        mkcert's CA is not trusted by firefox\n\
             todo  hosts     *.localhost does not resolve to loopback; the installed helper is revision 1 and devlanes needs 2\n\
             todo  resolver  *.test does not reach the devlanes DNS server: /etc/resolver/test is missing\n\
             skip  browser   pipx run runs devlanes from a cache\n"
        );
        let done = Facts {
            trust: Trust::Trusted,
            hosts: HostsWanted::Always("hostnames.hosts_file is always"),
            helper: Helper::Current,
            dns: Dns::Routed { tld: "test".into() },
            browser: browser_facts(&[browser::Found::Current, browser::Found::NoBrowser]),
            ..fresh()
        };
        let steps = plan(&done);
        assert!(steps.iter().all(|step| step.state() == "ok"), "{steps:?}");
        assert_eq!(
            steps[1].reason,
            "hostnames.hosts_file is always; the /etc/hosts helper is installed"
        );
    }

    #[test]
    /// A project that needs the hosts file is something only the daemon knows.
    fn the_daemon_report_asks_for_the_helper() {
        let facts = Facts {
            daemon_hosts: true,
            helper: Helper::Current,
            ..fresh()
        };
        let hosts = &plan(&facts)[1];
        assert_eq!(hosts.state(), "todo");
        assert_eq!(
            hosts.reason,
            "a lane needs its hostnames in /etc/hosts; the daemon cannot run the /etc/hosts helper"
        );
        assert_eq!(
            missing_of(&json!({"setup": {"missing": ["hosts", "resolver"]}})),
            ["hosts", "resolver"]
        );
        assert!(missing_of(&json!({"version": "0.1.0"})).is_empty());
    }

    #[test]
    fn windows_never_installs_the_helper() {
        for localhost in [Loopback, Fails] {
            let facts = Facts {
                windows: true,
                localhost,
                hosts: HostsWanted::Always("hostnames.resolver is hosts_file"),
                ..fresh()
            };
            assert_eq!(plan(&facts)[1].state(), "skip");
        }
    }

    #[test]
    fn trust_reports_become_facts() {
        assert_eq!(
            trust_of(&json!([{"store": "system", "state": "trusted"}])),
            Trust::Trusted
        );
        assert_eq!(
            trust_of(&json!([
                {"store": "system", "state": "untrusted"},
                {"store": "firefox", "state": "unknown", "reason": "no profile"},
            ])),
            Trust::Untrusted(vec!["system".into()])
        );
        assert_eq!(
            trust_of(&json!([
                {"store": "system", "state": "trusted"},
                {"store": "firefox", "state": "unknown", "reason": "no profile"},
            ])),
            Trust::Unknown("firefox: no profile".into())
        );
    }

    #[test]
    fn user_config_and_helper_checks_become_facts() {
        let mut config = UserConfig::default();
        assert_eq!(hosts_wanted(&config), HostsWanted::Auto);
        config.hostnames.hosts_file = Some(devlanes_config::HostsFile::Never);
        assert_eq!(hosts_wanted(&config), HostsWanted::Never);
        config.hostnames.hosts_file = Some(devlanes_config::HostsFile::Always);
        assert!(matches!(hosts_wanted(&config), HostsWanted::Always(_)));
        // The strategy writes the hosts file whatever hosts_file says.
        config.hostnames.hosts_file = Some(devlanes_config::HostsFile::Never);
        config.hostnames.resolver = devlanes_config::Resolver::HostsFile;
        assert_eq!(
            hosts_wanted(&config),
            HostsWanted::Always("hostnames.resolver is hosts_file")
        );
        assert_eq!(resolver_tld(&config), None);
        config.hostnames.resolver = devlanes_config::Resolver::Resolver;
        assert_eq!(resolver_tld(&config), None);
        config.hostnames.tld = Some("test".into());
        assert_eq!(resolver_tld(&config).as_deref(), Some("test"));

        assert_eq!(helper_of(None), Helper::Absent);
        assert_eq!(helper_of(Some(&json!({"state": "ok"}))), Helper::Current);
        assert_eq!(
            helper_of(Some(
                &json!({"state": "outdated", "revision": 1, "required": 2})
            )),
            Helper::Stale("the installed helper is revision 1 and devlanes needs 2".into())
        );
        assert!(matches!(
            helper_of(Some(&json!({"state": "unrunnable"}))),
            Helper::Stale(_)
        ));
    }

    #[test]
    fn resolver_status_becomes_a_fact() {
        let tld = || "test".to_owned();
        assert_eq!(
            dns_of(
                tld(),
                &json!({"path": "/etc/resolver/test", "installed": true, "matches": true})
            ),
            Dns::Routed { tld: tld() }
        );
        let reason = |status: Value| match dns_of(tld(), &status) {
            Dns::Unrouted { reason, .. } => reason,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            reason(json!({"path": "/etc/resolver/test", "installed": false, "matches": false})),
            "/etc/resolver/test is missing"
        );
        assert_eq!(
            reason(json!({"path": "/etc/resolver/test", "installed": true, "matches": false})),
            "/etc/resolver/test points elsewhere"
        );
        assert_eq!(
            reason(json!({"error": "systemctl was not found"})),
            "systemctl was not found"
        );
    }

    #[test]
    fn the_question_counts_and_mentions_sudo() {
        assert_eq!(
            question(2, false, true),
            "Do the 2 missing steps? sudo may ask for your password once. [Y/n] "
        );
        assert!(question(1, false, false).starts_with("Do the 1 missing step?"));
        assert!(
            question(1, true, false)
                .starts_with("Do the 1 missing step and register the browser extension host?")
        );
    }

    /// Browser facts with Chrome, Chromium, Brave and Edge found as `found`
    /// says, in that order, for a script install.
    fn browser_facts(found: &[browser::Found]) -> browser::Facts {
        browser::Facts::Found {
            install: browser::Install::new(
                devlanes_update::InstallMethod::Script("/opt/devlanes".into()),
                "/opt/devlanes/devlanes".into(),
            ),
            registrations: browser::Browser::ALL
                .into_iter()
                .zip(found)
                .map(|(browser, found)| browser::Registration {
                    browser,
                    place: format!("/home/a/{}", browser.name()),
                    found: found.clone(),
                })
                .collect(),
            held: None,
        }
    }

    /// The browser step never counts: whether nothing, a fresh registration
    /// or a stale one is in place, `--check` exits 0 once the rest is done,
    /// `missing` stays empty, and setup asks nothing for it alone.
    #[test]
    fn the_browser_step_never_counts() {
        use browser::Found::{Current, Missing, NoBrowser, Stale};
        let stale = Stale {
            runs: Some("/gone/devlanes".into()),
            ours: false,
        };
        let done = Facts {
            trust: Trust::Trusted,
            ..fresh()
        };
        for (found, state) in [
            (vec![Missing, NoBrowser], "info"),
            (vec![Current, NoBrowser], "ok"),
            (vec![stale.clone(), Current], "info"),
            (vec![NoBrowser, NoBrowser], "skip"),
        ] {
            let facts = Facts {
                browser: browser_facts(&found),
                ..done.clone()
            };
            let steps = plan(&facts);
            let step = &steps[3];
            assert_eq!(step.kind, StepKind::BrowserHost);
            assert_eq!(step.state(), state, "{step:?}");
            assert!(!step.todo(), "{step:?}");
            assert_eq!(step.applies(), state == "info", "{step:?}");
            assert!(!step.kind.sudo(&UserConfig::default()));
            assert_eq!(check_code(&steps), 0, "{steps:?}");
            // Alone, it runs without a question, so it needs no --yes.
            let alone = if state == "info" {
                Decision::Run
            } else {
                Decision::Nothing
            };
            assert_eq!(decide(&steps, false), alone, "{steps:?}");
        }
        // With a missing step, the exit status is that step's alone, and the
        // browser host is asked about with it.
        let facts = Facts {
            browser: browser_facts(&[Current]),
            ..fresh()
        };
        let steps = plan(&facts);
        assert_eq!(check_code(&steps), 1);
        assert_eq!(
            decide(&steps, false),
            Decision::Ask {
                missing: 1,
                browser: false
            }
        );
        let steps = plan(&Facts {
            browser: browser_facts(&[Missing]),
            ..fresh()
        });
        assert_eq!(
            decide(&steps, false),
            Decision::Ask {
                missing: 1,
                browser: true
            }
        );
        assert_eq!(decide(&steps, true), Decision::Run);

        let reason = |found: &[browser::Found]| browser::plan(&browser_facts(found)).2;
        assert_eq!(
            reason(&[Missing, NoBrowser, Current, Missing]),
            "setup registers the browser extension host for Chrome and Edge, and it is registered for Brave"
        );
        assert_eq!(
            reason(&[stale.clone(), Missing]),
            "setup registers the browser extension host for Chromium and refreshes it for Chrome"
        );
        assert_eq!(
            reason(&[Current, Current, Current]),
            "the browser extension host is registered for Chrome, Chromium and Brave"
        );
        assert_eq!(
            reason(&[NoBrowser]),
            "no Chrome, Chromium, Brave or Edge folder in your home"
        );
    }

    /// Browser folders under `home` in the Linux layout, which needs no
    /// registry, so these tests run the same on every OS.
    fn temp_location(home: &std::path::Path) -> browser::Location {
        browser::Location {
            home: home.to_path_buf(),
            layout: browser::Layout::Linux,
            registry_root: String::new(),
        }
    }

    #[tokio::test]
    /// `--check` exits 1 while a step is missing (a fresh home has no CA yet)
    /// and creates nothing: no CA, and no browser manifest whether one is
    /// missing or stale. Without a terminal and without --yes, setup refuses
    /// before changing anything, the browser host included.
    async fn check_exits_1_while_something_is_missing() {
        use devlanes_cli::clap::Parser;
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(temp.path().join("home")).expect("paths");
        let user = temp.path().join("user");
        let location = temp_location(&user);
        let hosts = user
            .join(".config")
            .join("google-chrome")
            .join("NativeMessagingHosts");
        std::fs::create_dir_all(hosts.parent().expect("parent")).expect("browser folder");
        let manifest = hosts.join("run.devlanes.browser.json");
        let cli = devlanes_cli::Cli::parse_from(["devlanes", "--output", "plain", "--quiet"]);
        let runtime = Runtime::resolve(&cli, devlanes_cli::Signals::default()).expect("runtime");
        let check = || run_in(&runtime, &paths, Some(&location), SetupAction::Check);
        // A fresh home has no CA, so the exit status is 1 from the CA alone.
        assert_eq!(check().await.expect("check"), 1);
        assert!(!manifest.exists(), "--check wrote a manifest");
        let stale = r#"{"name": "run.devlanes.browser", "path": "/gone/devlanes"}"#;
        std::fs::create_dir_all(&hosts).expect("hosts folder");
        std::fs::write(&manifest, stale).expect("stale manifest");
        assert_eq!(check().await.expect("check"), 1);
        let error = run_in(
            &runtime,
            &paths,
            Some(&location),
            SetupAction::Apply { yes: false },
        )
        .await
        .expect_err("needs --yes");
        assert_eq!(error.code, "confirmation_required");
        assert_eq!(std::fs::read_to_string(&manifest).expect("manifest"), stale);
        assert!(!paths.home.join("ca").exists(), "setup created the CA");
    }

    /// The start notice follows the TUI prompt's rules: it names only the
    /// steps a URL of the lane needs, and the first URL that needs one.
    #[test]
    fn the_start_notice_names_the_steps_and_the_first_url_that_needs_them() {
        let data = json!({"units": [
            {"urls": ["http://127.0.0.1:20101"]},
            {"urls": ["http://127.0.0.1:20102", "https://web.acme.localhost:8443/path"]},
            {"urls": ["https://api.acme.localhost"]},
        ]});
        let urls = unit_urls(&data);
        let missing = |steps: &[&str]| steps.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            notice(&missing(&["ca", "hosts"]), &urls),
            Some((
                "setup:ca,hosts".into(),
                "web.acme.localhost needs devlanes setup (CA not trusted, /etc/hosts helper missing)\n"
                    .into()
            ))
        );
        // An http:// hostname with a missing helper gets a line; the CA,
        // which it does not need, is left out of it and of the key.
        let http =
            json!({"units": [{"urls": ["http://127.0.0.1:1", "http://web.acme.localhost:20102"]}]});
        assert_eq!(
            notice(&missing(&["ca", "hosts"]), &unit_urls(&http)),
            Some((
                "setup:hosts".into(),
                "web.acme.localhost needs devlanes setup (/etc/hosts helper missing)\n".into()
            ))
        );
        assert_eq!(notice(&missing(&["ca"]), &unit_urls(&http)), None);
        let plain = json!({"units": [{"urls": ["http://127.0.0.1:1", "http://localhost:2"]}]});
        assert_eq!(
            notice(&missing(&["ca", "hosts", "resolver"]), &unit_urls(&plain)),
            None
        );
        assert_eq!(notice(&missing(&[]), &urls), None);
        assert!(unit_urls(&json!({})).is_empty());
    }

    #[tokio::test]
    /// Every setup action refuses a user config that the lenient load would
    /// take with one key dropped, before any trust store, sudo or resolver
    /// file is touched; `--check` too.
    async fn setup_loads_the_user_config_strictly() {
        use devlanes_cli::clap::Parser;
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(temp.path()).expect("paths");
        std::fs::write(paths.home.join("config.yaml"), "tui:\n  theme: neon\n").expect("config");
        assert!(crate::user_config::load(&paths).await.is_err());
        let cli = devlanes_cli::Cli::parse_from(["devlanes", "--output", "plain"]);
        let runtime = Runtime::resolve(&cli, devlanes_cli::Signals::default()).expect("runtime");
        for action in [SetupAction::Apply { yes: true }, SetupAction::Check] {
            let location = temp_location(&temp.path().join("user"));
            let error = run_in(&runtime, &paths, Some(&location), action)
                .await
                .expect_err("setup refuses");
            assert_eq!(error.code, "config_error", "{error:?}");
        }
    }
}
