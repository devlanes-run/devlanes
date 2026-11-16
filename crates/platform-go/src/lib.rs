//! Go launch contributor (`platform: go`, or detected from `go.mod` or `go.work`).
//!
//! `SSL_CERT_FILE` points at the combined bundle (system roots plus the devlanes
//! root). Go's `crypto/x509` reads it on Linux and the other Unix systems except
//! macOS, where it replaces the default bundle file list (`SSL_CERT_DIR` still
//! applies). On macOS and Windows Go uses the OS verifier and ignores the
//! variable; `devlanes setup` covers those. On Linux, `devlanes setup` with system
//! trust already adds the CA to the distro bundle; the variable covers lanes where
//! it did not (a trust list without `system`, boxes, CI).

use async_trait::async_trait;
use devlanes_core::{
    Error,
    contrib::{
        CA_BUNDLE_ENV, ContribCx, Contributor, DETECT_MANIFEST, DETECT_SCRIPT_SOURCE, DetectCx,
        LaunchDraft,
    },
};

/// Platform name matched against `platform:` (explicit or inferred).
pub const PLATFORM: &str = "go";

/// Manifest that makes a directory a Go module.
pub const MANIFEST: &str = "go.mod";

/// Workspace file that makes a directory a Go workspace.
pub const WORKSPACE: &str = "go.work";

/// Claim for a `go.mod` or `go.work` in the working directory. Below node's,
/// and below rust's, so a directory with both `Cargo.toml` and `go.mod` stays rust.
const CWD_CONFIDENCE: u8 = DETECT_MANIFEST - 6;

/// Contributor for `platform: go` units.
#[derive(Debug, Default, Clone, Copy)]
pub struct GoContributor;

#[async_trait]
impl Contributor for GoContributor {
    fn id(&self) -> &'static str {
        "platform-go"
    }

    fn priority(&self) -> i32 {
        50
    }

    fn platform(&self) -> Option<&'static str> {
        Some(PLATFORM)
    }

    /// Claims `go.mod` script sources and working directories with a `go.mod`
    /// or `go.work`.
    async fn detect(&self, cx: &DetectCx<'_>) -> Option<u8> {
        let source = cx.script.and_then(|s| s.source.file_name());
        if source.is_some_and(|name| name == MANIFEST) {
            return Some(DETECT_SCRIPT_SOURCE);
        }
        for file in [MANIFEST, WORKSPACE] {
            if tokio::fs::try_exists(cx.cwd.join(file))
                .await
                .unwrap_or(false)
            {
                return Some(CWD_CONFIDENCE);
            }
        }
        None
    }

    /// Sets `SSL_CERT_FILE` to the combined bundle when the CA provides one.
    async fn apply(&self, draft: &mut LaunchDraft, cx: &ContribCx<'_>) -> Result<(), Error> {
        if cx.platform != Some(PLATFORM) {
            return Ok(());
        }
        if let Some(bundle) = cx.ca_env.get(CA_BUNDLE_ENV) {
            // SSL_CERT_FILE replaces the trust store, so a shell value is dropped; say so.
            if let Some(previous) = cx.client_env.get("SSL_CERT_FILE")
                && previous != bundle
            {
                draft.notes.push(format!(
                    "SSL_CERT_FILE from the shell ({previous}) replaced by the devlanes bundle"
                ));
            }
            draft.env.insert("SSL_CERT_FILE".to_owned(), bundle.clone());
        }
        Ok(())
    }
}
