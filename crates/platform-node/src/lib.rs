//! Node.js launch contributors.
//!
//! - [`NodeContributor`] (`platform: node`, or detected from `package.json`):
//!   trusts the devlanes CA through `NODE_EXTRA_CA_CERTS` and forces color, since
//!   output often goes through pipes.
//!   Node needs no unbuffering switch: `process.stdout` writes are not held back
//!   in a userspace buffer.
//! - [`CloudflareContributor`]: points every process of a lane at a dev registry in
//!   the lane data folder, so same-named workers in two lanes do not bind to each
//!   other, even when both lanes run from one folder.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use devlanes_core::{
    Error,
    contrib::{
        CA_CERT_ENV, ContribCx, Contributor, DETECT_MANIFEST, DETECT_SCRIPT_SOURCE, DetectCx,
        LaunchDraft,
    },
};

/// Platform name matched against `platform:` (explicit or inferred).
pub const PLATFORM: &str = "node";

/// Manifest that makes a directory a Node project.
pub const MANIFEST: &str = "package.json";

/// Returns whether the unit's `run_script` source is named `file`.
fn script_file_is(cx: &DetectCx<'_>, file: &str) -> bool {
    cx.script
        .and_then(|s| s.source.file_name())
        .is_some_and(|name| name == file)
}

/// Non-blocking existence check; unreadable paths count as absent.
async fn exists(path: &Path) -> bool {
    tokio::fs::try_exists(path).await.unwrap_or(false)
}

/// Wrangler's dev registry folder, read by `wrangler dev` and passed to Miniflare
/// (`getRegistryPath` in cloudflare/workers-sdk `packages/workers-utils`).
pub const WRANGLER_REGISTRY_ENV: &str = "WRANGLER_REGISTRY_PATH";

/// Miniflare 5's registry override. It wins over the folder Wrangler passes, and the
/// Cloudflare Vite plugin (1.49.1 and later) reads it through Miniflare's
/// `getDefaultDevRegistryPath`, so a value left in the shell would bypass the lane.
pub const CLOUDFLARE_REGISTRY_ENV: &str = "CLOUDFLARE_DEV_REGISTRY_PATH";

/// Miniflare 4's registry override, read by Cloudflare Vite plugin releases up to 1.49.0.
pub const MINIFLARE_REGISTRY_ENV: &str = "MINIFLARE_REGISTRY_PATH";

/// Every registry variable Wrangler, Miniflare or the Vite plugin reads, all set to the
/// same lane folder so workers started by any of them find each other.
pub const REGISTRY_ENVS: [&str; 3] = [
    WRANGLER_REGISTRY_ENV,
    CLOUDFLARE_REGISTRY_ENV,
    MINIFLARE_REGISTRY_ENV,
];

/// Registry directory in the lane data folder, shared by the lane's workers.
pub const REGISTRY_DIR: &str = "wrangler-registry";

/// Contributor for `platform: node` units.
#[derive(Debug, Default, Clone, Copy)]
pub struct NodeContributor;

#[async_trait]
impl Contributor for NodeContributor {
    fn id(&self) -> &'static str {
        "platform-node"
    }

    fn priority(&self) -> i32 {
        50
    }

    fn platform(&self) -> Option<&'static str> {
        Some(PLATFORM)
    }

    /// Claims `package.json` scripts and working directories with a `package.json`.
    async fn detect(&self, cx: &DetectCx<'_>) -> Option<u8> {
        if script_file_is(cx, MANIFEST) {
            return Some(DETECT_SCRIPT_SOURCE);
        }
        exists(&cx.cwd.join(MANIFEST))
            .await
            .then_some(DETECT_MANIFEST)
    }

    /// Sets `NODE_EXTRA_CA_CERTS` from the CA and `FORCE_COLOR=1` unless the
    /// client already chose a color policy.
    async fn apply(&self, draft: &mut LaunchDraft, cx: &ContribCx<'_>) -> Result<(), Error> {
        if cx.platform != Some(PLATFORM) {
            return Ok(());
        }
        if let Some(cert) = cx.ca_env.get(CA_CERT_ENV) {
            // Node reads one extra file only, so a user-set value is replaced and reported.
            if let Some(previous) = cx.client_env.get("NODE_EXTRA_CA_CERTS")
                && previous != cert
            {
                draft.notes.push(format!(
                    "NODE_EXTRA_CA_CERTS from the shell ({previous}) replaced by the devlanes CA; add that CA to the devlanes bundle or set it in the service env"
                ));
            }
            draft
                .env
                .insert("NODE_EXTRA_CA_CERTS".to_owned(), cert.clone());
        }
        if !cx.client_env.contains_key("FORCE_COLOR") && !cx.client_env.contains_key("NO_COLOR") {
            draft.env.insert("FORCE_COLOR".to_owned(), "1".to_owned());
        }
        Ok(())
    }
}

/// Sets every variable in [`REGISTRY_ENVS`] to `<lane data>/wrangler-registry` for
/// every process of every lane.
///
/// Wrangler and Miniflare often run inside processes devlanes cannot recognise: Vite or
/// Next plugins, scripts that call `wrangler`, nested package scripts, and tasks such as
/// `wrangler d1 migrations apply`. All of them must share the lane's registry, so the
/// variables go on every unit instead of on detected ones. Only Wrangler and Miniflare
/// read them, so they change nothing for other programs.
#[derive(Debug, Default, Clone, Copy)]
pub struct CloudflareContributor;

#[async_trait]
impl Contributor for CloudflareContributor {
    fn id(&self) -> &'static str {
        "cloudflare"
    }

    fn priority(&self) -> i32 {
        60
    }

    /// Sets the lane's registry path in every registry variable, replacing values from
    /// the shell. Wrangler and Miniflare create the folder when they register a worker,
    /// so nothing touches the disk here.
    async fn apply(&self, draft: &mut LaunchDraft, cx: &ContribCx<'_>) -> Result<(), Error> {
        let registry = registry_path(cx.lane_data_path).display().to_string();
        for name in REGISTRY_ENVS {
            draft.env.insert(name.to_owned(), registry.clone());
        }
        Ok(())
    }
}

/// Returns `<lane data>/wrangler-registry`.
pub fn registry_path(lane_data: &Path) -> PathBuf {
    lane_data.join(REGISTRY_DIR)
}
