//! The `ca` step: the CA `tls.ca` selects (devlanes' own, or mkcert's) and its
//! trust in the stores `tls.trust` names. Also what the daemon's wiring and
//! `doctor` need to load and judge that CA.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use devlanes_cli::{Rendered, Runtime};
use devlanes_client::Paths;
use devlanes_config::UserConfig;
use devlanes_proto::RpcError;
use devlanes_trust::{CaCertificate, TrustStatus, TrustStore, TrustTarget};
use serde_json::{Value, json};

use crate::output::emit;

/// Name shown in keychains and NSS databases.
const CA_NAME: &str = "devlanes development CA";

/// Why an unconstrained devlanes CA is worth replacing.
pub(crate) const UNCONSTRAINED_CA: &str = "the devlanes CA has no name constraints (it predates them), so a leaked key could sign certificates for any site";

/// The CAROOT from `mkcert -CAROOT`: it must exit 0 and print a non-empty
/// absolute path. Anything else would make the CA load from a path relative to
/// the current directory.
pub(crate) fn mkcert_caroot(output: &std::process::Output) -> Result<PathBuf, String> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`mkcert -CAROOT` failed: {}", stderr.trim()));
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if root.is_empty() {
        return Err("`mkcert -CAROOT` printed no directory".into());
    }
    let root = PathBuf::from(root);
    if !root.is_absolute() {
        return Err(format!(
            "`mkcert -CAROOT` printed a relative path: {}",
            root.display()
        ));
    }
    Ok(root)
}

/// Loads the CA `config` selects (devlanes' own, or mkcert's).
///
/// Without `create`, devlanes' own CA is only read: `ca.pem` alone is enough,
/// and `None` means there is none. Checking and removing trust never create a CA.
pub(crate) async fn certificate(
    paths: &Paths,
    config: &UserConfig,
    create: bool,
) -> Result<Option<CaCertificate>, RpcError> {
    let mkcert = mkcert_selected(config);
    let tld = ca_tld(config);
    let home = paths.home.clone();
    let pem = tokio::task::spawn_blocking(move || -> Result<Option<String>, String> {
        if mkcert {
            let output = std::process::Command::new("mkcert")
                .arg("-CAROOT")
                .stdin(std::process::Stdio::null())
                .output()
                .map_err(|error| format!("mkcert: {error}"))?;
            let root = mkcert_caroot(&output)?;
            let ca = devlanes_pki::CertificateAuthority::from_mkcert(&root)
                .map_err(|error| error.to_string())?;
            return Ok(Some(ca.certificate_pem().to_owned()));
        }
        let directory = home.join("ca");
        if create {
            let ca = devlanes_pki::CertificateAuthority::load_or_create_with_tld(
                &directory,
                tld.as_deref(),
            )
            .map_err(|error| error.to_string())?;
            return Ok(Some(ca.certificate_pem().to_owned()));
        }
        match std::fs::read_to_string(directory.join("ca.pem")) {
            Ok(pem) => Ok(Some(pem)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("reading {}: {error}", directory.display())),
        }
    })
    .await
    .map_err(|error| RpcError::new("io", json!({"message": error.to_string()}), 1))?
    .map_err(|message| RpcError::new("ca_unavailable", json!({"message": message}), 1))?;
    pem.map(|pem| {
        CaCertificate::from_pem(&pem, CA_NAME)
            .map_err(|error| RpcError::new("ca_invalid", json!({"message": error.to_string()}), 1))
    })
    .transpose()
}

/// The TLD a new devlanes CA should also permit: `hostnames.tld` when it passes the
/// resolver's special-use check. Anything else is dropped rather than widening the
/// CA's name constraints to a name that could belong to a public site.
pub(crate) fn ca_tld(config: &UserConfig) -> Option<String> {
    let tld = config.hostnames.tld.as_deref()?;
    devlanes_resolver_dns::Zone::new(tld)
        .ok()
        .map(|zone| zone.name().to_owned())
}

/// How to replace a CA created before devlanes added name constraints; `None`
/// when `ca` is constrained. `setup --remove` keeps `ca.pem`, so this never
/// fixes itself.
pub(crate) fn unconstrained_ca_hint(ca: &CaCertificate, home: &Path) -> Option<String> {
    (!ca.is_name_constrained()).then(|| {
        format!(
            "{UNCONSTRAINED_CA}; recreate it: {}",
            recreate_ca_command(home)
        )
    })
}

/// The commands that replace the CA under `home` with a constrained one
/// (`crates/pki/README.md`, "Name constraints").
#[cfg(unix)]
pub(crate) fn recreate_ca_command(home: &Path) -> String {
    format!(
        "devlanes setup --remove && rm -r {} && devlanes setup && devlanes daemon restart",
        home.join("ca").display()
    )
}

/// The steps that replace the CA under `home` with a constrained one
/// (`crates/pki/README.md`, "Name constraints"), in words: Windows shells
/// disagree on both `&&` and recursive delete.
#[cfg(windows)]
pub(crate) fn recreate_ca_command(home: &Path) -> String {
    format!(
        "devlanes setup --remove, delete the folder {}, then devlanes setup and devlanes daemon restart",
        home.join("ca").display()
    )
}

/// Whether the user config selects `tls.ca: mkcert`.
pub(crate) fn mkcert_selected(config: &UserConfig) -> bool {
    matches!(config.tls.ca, devlanes_config::CaKind::Mkcert)
}

/// Configured trust targets, defaulting to the system store; mobile targets are refused.
fn targets(config: &UserConfig) -> Vec<TrustTarget> {
    let configured: Vec<TrustTarget> = config
        .tls
        .trust
        .iter()
        .filter_map(|target| serde_json::to_value(target).ok())
        .filter_map(|value| {
            value
                .as_str()
                .and_then(|name| TrustTarget::from_str(name).ok())
        })
        .collect();
    if configured.is_empty() {
        vec![TrustTarget::System]
    } else {
        configured
    }
}

/// Whether trusting or untrusting the CA runs sudo: only Linux's system
/// store needs root. The NSS databases of Firefox and Chrome belong to the
/// user, and macOS and Windows confirm changes in their own dialogs.
pub(super) fn needs_root(config: &UserConfig) -> bool {
    cfg!(target_os = "linux") && targets(config).contains(&TrustTarget::System)
}

/// What `setup --remove` does to the CA, read-only: `None` when nothing
/// changes (mkcert's CA stays trusted, or there is no devlanes CA).
pub(super) async fn removal(
    paths: &Paths,
    config: &UserConfig,
) -> Result<Option<String>, RpcError> {
    if mkcert_selected(config) {
        return Ok(None);
    }
    let stores: Vec<&str> = targets(config)
        .into_iter()
        .map(TrustTarget::as_str)
        .collect();
    Ok(certificate(paths, config, false)
        .await?
        .map(|_| format!("untrust the devlanes CA in {}", stores.join(", "))))
}

/// The OS decides the store implementation at compile time. `own_ca` lets the
/// macOS keychain and the Windows root store also remove older devlanes CAs by
/// label or subject; mkcert's CA is not ours.
/// On Linux, sudo may ask for a password only when this invocation may prompt.
fn store(
    target: TrustTarget,
    own_ca: bool,
    can_prompt: bool,
) -> Result<Box<dyn TrustStore>, devlanes_trust::TrustError> {
    #[cfg(target_os = "macos")]
    {
        let _ = can_prompt;
        let trust = devlanes_trust_macos::MacosTrust::from_env()?;
        let trust = if own_ca {
            trust.with_ca_label(devlanes_trust_macos::DEVLANES_CA_LABEL)
        } else {
            trust
        };
        trust.store(target)
    }
    // Windows confirms each change in its own dialog, as macOS does, so
    // `can_prompt` changes nothing; without a desktop the store refuses first.
    #[cfg(windows)]
    {
        let _ = can_prompt;
        let trust = devlanes_trust_windows::WindowsTrust::default();
        let trust = if own_ca {
            trust.with_ca_subject(devlanes_trust_windows::DEVLANES_CA_SUBJECT)
        } else {
            trust
        };
        trust.store(target)
    }
    #[cfg(target_os = "linux")]
    {
        use devlanes_trust_linux::{LinuxTrust, Privilege};
        let _ = own_ca;
        // A terminal on stdin is not enough: agents and CI have one but must not prompt.
        let privilege = match Privilege::detect() {
            Privilege::Sudo { .. } => Privilege::Sudo {
                interactive: can_prompt,
            },
            root => root,
        };
        let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
            devlanes_trust::TrustError::io(
                "finding the home directory",
                std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is not set"),
            )
        })?;
        LinuxTrust::new(Arc::new(devlanes_trust::SystemExecutor), home, privilege).store(target)
    }
}

/// Whether each configured trust store trusts the CA:
/// `[{store, state: trusted|untrusted|unknown, reason?}]`, `null` without a CA.
/// Read-only and never prompts: a store that would need a password is `unknown`.
pub(crate) async fn report(paths: &Paths, config: &UserConfig) -> Value {
    match certificate(paths, config, false).await {
        Ok(Some(certificate)) => store_report(config, &certificate).await,
        _ => Value::Null,
    }
}

/// [`report`] for a loaded `certificate`: one entry per configured store.
pub(super) async fn store_report(config: &UserConfig, certificate: &CaCertificate) -> Value {
    let mkcert = mkcert_selected(config);
    let mut done = BTreeSet::new();
    let mut report = Vec::new();
    for target in targets(config) {
        let status = async {
            target.ensure_supported()?;
            let store: Arc<dyn TrustStore> = Arc::from(store(target, !mkcert, false)?);
            if !done.insert(store.id()) {
                return Ok(None);
            }
            store.status(certificate).await.map(Some)
        }
        .await;
        report.push(match status {
            Ok(None) => continue,
            Ok(Some(status)) => trust_entry(target, &status),
            Err(error) => {
                json!({"store": target.as_str(), "state": "unknown", "reason": error.to_string()})
            }
        });
    }
    Value::Array(report)
}

/// [`report`] for `doctor`, with the config read as the daemon reads it;
/// doctor lists its problems itself.
pub(crate) async fn doctor_report(paths: &Paths) -> Value {
    report(paths, &crate::user_config::lenient(paths).await).await
}

/// One store's line of [`report`].
fn trust_entry(target: TrustTarget, status: &TrustStatus) -> Value {
    match status {
        TrustStatus::Installed => json!({"store": target.as_str(), "state": "trusted"}),
        TrustStatus::Missing => json!({"store": target.as_str(), "state": "untrusted"}),
        TrustStatus::Unknown { reason } => {
            json!({"store": target.as_str(), "state": "unknown", "reason": reason})
        }
    }
}

/// Creates devlanes' CA when there is none yet and trusts it (or mkcert's) in
/// every configured store; one failure doesn't skip the rest.
pub(super) async fn install(
    runtime: &Runtime,
    paths: &Paths,
    config: &UserConfig,
) -> Result<u8, RpcError> {
    let mkcert = mkcert_selected(config);
    let certificate = certificate(paths, config, true)
        .await?
        .ok_or_else(|| RpcError::new("ca_unavailable", json!({"message": "no CA"}), 1))?;
    let code = apply(runtime, config, &certificate, true).await;
    // mkcert roots are never constrained and not ours to replace.
    if !mkcert && let Some(hint) = unconstrained_ca_hint(&certificate, &paths.home) {
        emit(&Rendered {
            stderr: format!("warning: {hint}\n"),
            ..Rendered::default()
        });
    }
    Ok(code)
}

/// Removes devlanes' CA from every configured store. mkcert's CA stays trusted:
/// `mkcert -install` may have added that trust for other tools.
pub(super) async fn remove(
    runtime: &Runtime,
    paths: &Paths,
    config: &UserConfig,
) -> Result<u8, RpcError> {
    let say = |text: String| {
        if !runtime.quiet {
            emit(&Rendered {
                stdout: text,
                ..Rendered::default()
            });
        }
    };
    if mkcert_selected(config) {
        say(
            "kept mkcert's CA trusted (tls.ca: mkcert); run `mkcert -uninstall` to remove it\n"
                .into(),
        );
        return Ok(0);
    }
    let Some(certificate) = certificate(paths, config, false).await? else {
        say(format!(
            "no devlanes CA under {}; nothing to untrust\n",
            paths.home.join("ca").display()
        ));
        return Ok(0);
    };
    Ok(apply(runtime, config, &certificate, false).await)
}

/// Installs (or removes) `certificate` in every configured store and prints
/// each store's state; 1 when a store failed.
async fn apply(
    runtime: &Runtime,
    config: &UserConfig,
    certificate: &CaCertificate,
    install: bool,
) -> u8 {
    let own_ca = !mkcert_selected(config);
    let mut done = BTreeSet::new();
    let mut failed = false;
    let mut lines = String::new();
    for target in targets(config) {
        let result = async {
            target.ensure_supported()?;
            let store: Arc<dyn TrustStore> =
                Arc::from(store(target, own_ca, runtime.can_prompt())?);
            // System and Chrome share the macOS login keychain; touch it once.
            if !done.insert(store.id()) {
                return Ok(None);
            }
            if install {
                store.install(certificate).await?;
            } else {
                store.uninstall(certificate).await?;
            }
            store.status(certificate).await.map(Some)
        }
        .await;
        match result {
            Ok(None) => {}
            Ok(Some(status)) => {
                let word = match status {
                    TrustStatus::Installed => "trusted",
                    TrustStatus::Missing => "untrusted",
                    TrustStatus::Unknown { .. } => "unknown",
                };
                lines.push_str(&format!("{word} {target}\n"));
            }
            Err(error) => {
                failed = true;
                lines.push_str(&format!("failed {target}: {error}\n"));
            }
        }
    }
    // --quiet keeps only the failures, on stderr, so a command that prints
    // JSON on stdout (`devlanes uninstall --json`) keeps it parseable.
    if runtime.quiet {
        let failures = lines
            .lines()
            .filter(|line| line.starts_with("failed "))
            .map(|line| format!("{line}\n"))
            .collect();
        emit(&Rendered {
            stderr: failures,
            ..Rendered::default()
        });
        return u8::from(failed);
    }
    emit(&Rendered {
        stdout: lines,
        ..Rendered::default()
    });
    u8::from(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Doctor's trust entries name the store and its state.
    fn trust_entries_name_store_and_state() {
        assert_eq!(
            trust_entry(TrustTarget::System, &TrustStatus::Missing),
            json!({"store": "system", "state": "untrusted"})
        );
        assert_eq!(
            trust_entry(
                TrustTarget::Firefox,
                &TrustStatus::Unknown {
                    reason: "certutil is missing".into()
                }
            ),
            json!({"store": "firefox", "state": "unknown", "reason": "certutil is missing"})
        );
    }

    /// An `Output` with exit `code` and `stdout`.
    #[cfg(unix)]
    fn output(code: i32, stdout: &str) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: b"boom".to_vec(),
        }
    }

    /// An `Output` with exit `code` and `stdout`.
    #[cfg(windows)]
    fn output(code: u32, stdout: &str) -> std::process::Output {
        use std::os::windows::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: b"boom".to_vec(),
        }
    }

    /// A CA without name constraints, as devlanes created before it added them.
    const UNCONSTRAINED_CA: &str = "-----BEGIN CERTIFICATE-----
MIICPTCCAeSgAwIBAgIJAJaVCy5tlb5jMAoGCCqGSM49BAMCMB8xHTAbBgNVBAMM
FG9sZCBkZXZsYW5lcyB0ZXN0IENBMCAXDTI2MTAyMTE0NTI1MloYDzIxMjYwOTI3
MTQ1MjUyWjAfMR0wGwYDVQQDDBRvbGQgZGV2bGFuZXMgdGVzdCBDQTCCAUswggED
BgcqhkjOPQIBMIH3AgEBMCwGByqGSM49AQECIQD/////AAAAAQAAAAAAAAAAAAAA
AP///////////////zBbBCD/////AAAAAQAAAAAAAAAAAAAAAP//////////////
/AQgWsY12Ko6k+ez671VdpiGvGUdBrDMU7D2O848PifSYEsDFQDEnTYIhucEk2pm
eOETnSa3gZ9+kARBBGsX0fLhLEJH+Lzm5WOkQPJ3A32BLeszoPShOUXYmMKWT+NC
4v4af5uO5+tKfA+eFivOM1drMV7Oy7ZAaDe/UfUCIQD/////AAAAAP//////////
vOb6racXnoTzucrC/GMlUQIBAQNCAASIxUCGuJknyIxMqf5MclGYTAa+e/giJixq
OQbGke4EXIGPJkdlXfvri5n+hgHH9Cwbuvnf4ez2zjp6+zyKCR8boxMwETAPBgNV
HRMBAf8EBTADAQH/MAoGCCqGSM49BAMCA0cAMEQCIBylpbhd3gjizW3ZngIiRU5v
O+HxDzOmVEDsdX+MjUbmAiBkD9cQf6fFQI4As7GUERYvQS7pD+2s2XYRMpLgi1MZ
oA==
-----END CERTIFICATE-----
";

    #[test]
    /// An old unconstrained CA gets recreate instructions; a new one does not.
    fn unconstrained_ca_is_reported() {
        #[cfg(unix)]
        let (home, ca) = (Path::new("/home/me/.devlanes"), "/home/me/.devlanes/ca");
        #[cfg(windows)]
        let (home, ca) = (
            Path::new(r"C:\Users\me\.devlanes"),
            r"C:\Users\me\.devlanes\ca",
        );
        let old = CaCertificate::from_pem(UNCONSTRAINED_CA, CA_NAME).expect("parse");
        let hint = unconstrained_ca_hint(&old, home).expect("hint");
        #[cfg(unix)]
        let remove = format!("rm -r {ca}");
        #[cfg(windows)]
        let remove = format!("delete the folder {ca}");
        for step in [
            "devlanes setup --remove",
            &remove,
            "devlanes daemon restart",
        ] {
            assert!(hint.contains(step), "{hint}");
        }
        assert_eq!(hint.matches("devlanes setup").count(), 2, "{hint}");
        let dir = tempfile::tempdir().expect("tempdir");
        let new = devlanes_pki::CertificateAuthority::load_or_create_with_tld(
            &dir.path().join("ca"),
            Some("dev.internal"),
        )
        .expect("ca");
        let new = CaCertificate::from_pem(new.certificate_pem(), CA_NAME).expect("parse");
        assert_eq!(unconstrained_ca_hint(&new, home), None);
    }

    #[test]
    /// sudo is asked for only when the configured stores include Linux's
    /// system store; Firefox and Chrome alone need none.
    fn only_the_linux_system_store_needs_root() {
        use devlanes_config::TrustTarget::{Chrome, Firefox, System};
        let config = |trust: Vec<devlanes_config::TrustTarget>| {
            let mut config = UserConfig::default();
            config.tls.trust = trust;
            config
        };
        let linux = cfg!(target_os = "linux");
        // No `tls.trust` means the system store.
        assert_eq!(needs_root(&config(Vec::new())), linux);
        assert_eq!(needs_root(&config(vec![System, Firefox])), linux);
        assert!(!needs_root(&config(vec![Firefox, Chrome])));
    }

    #[test]
    /// Only special-use TLDs reach the CA's name constraints.
    fn ca_tld_never_widens_constraints() {
        let mut config = UserConfig::default();
        assert_eq!(ca_tld(&config), None);
        for (tld, expected) in [
            ("Dev.Internal", Some("dev.internal")),
            ("test", Some("test")),
            ("com", None),
            ("example", None),
            ("localhost", None),
        ] {
            config.hostnames.tld = Some(tld.into());
            assert_eq!(ca_tld(&config).as_deref(), expected, "{tld}");
        }
    }

    #[test]
    /// CAROOT needs a successful exit and a non-empty absolute path.
    fn mkcert_caroot_is_validated() {
        #[cfg(unix)]
        let root = "/Users/me/Library/Application Support/mkcert";
        #[cfg(windows)]
        let root = r"C:\Users\me\AppData\Local\mkcert";
        assert_eq!(
            mkcert_caroot(&output(0, &format!("{root}\n"))),
            Ok(PathBuf::from(root))
        );
        assert!(mkcert_caroot(&output(1, "/abs\n")).is_err_and(|e| e.contains("boom")));
        assert!(mkcert_caroot(&output(0, " \n")).is_err());
        assert!(mkcert_caroot(&output(0, "mkcert-root\n")).is_err_and(|e| e.contains("relative")));
    }
}
