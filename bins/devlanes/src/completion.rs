//! Dynamic completion that asks an already-running daemon and never spawns one.
//! User config keys come from the config schema instead.

use std::{sync::Arc, time::Duration};

use devlanes_cli::{CompletionKind, CompletionSource, complete_environment};
use devlanes_client::{Client, Paths};
use devlanes_proto::{Context, LaneSummary, Method, Page};
use serde_json::{Value, json};

/// Completion runs once per keypress; anything slower than this feels broken.
const BUDGET: Duration = Duration::from_millis(300);

struct DaemonCandidates;

impl CompletionSource for DaemonCandidates {
    /// Builds a tiny runtime per call because completion runs before main's runtime exists.
    fn existing_daemon_candidates(&self, kind: CompletionKind, _prefix: &str) -> Vec<String> {
        if kind == CompletionKind::Setting {
            return devlanes_config::user_keys();
        }
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return Vec::new();
        };
        runtime
            .block_on(async { tokio::time::timeout(BUDGET, candidates(kind)).await.ok() })
            .flatten()
            .unwrap_or_default()
    }
}

/// Names from `items[].key` of a list result.
fn item_names(data: &Value, key: &str) -> Vec<String> {
    data.get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get(key).and_then(Value::as_str).map(str::to_owned))
        .collect()
}

/// Status with `all` knows every lane and unit name; tasks and profiles come from
/// the lane the daemon answers as the directory's own (`here`), named by
/// reference. `all` and `here` register nothing, so completing in a new
/// repository never adds it as a project.
async fn candidates(kind: CompletionKind) -> Option<Vec<String>> {
    let paths = Paths::discover().ok()?;
    let mut client = Client::connect(&paths).await.ok()?;
    let mut names = if kind == CompletionKind::Host {
        let data = client
            .request(Method::HostList, json!({}), Context::default())
            .await
            .ok()?;
        item_names(&data, "host")
    } else {
        let method = match kind {
            CompletionKind::Task => Some(Method::TaskList),
            CompletionKind::Profile => Some(Method::ProfileList),
            _ => None,
        };
        let context = Context {
            cwd: std::env::current_dir().ok()?,
            ..Context::default()
        };
        let data = client
            .request(
                Method::Status,
                json!({"all": true, "here_for": method}),
                context,
            )
            .await
            .ok()?;
        let here = data["here"].as_str().map(str::to_owned);
        let page: Page<LaneSummary> = serde_json::from_value(data).ok()?;
        let mut names = Vec::new();
        // Tasks and profiles belong to the directory's lane, named by reference.
        if let Some(method) = method {
            let context = Context {
                lane: Some(here?),
                ..Context::default()
            };
            let data = client.request(method, json!({}), context).await.ok()?;
            names = item_names(&data, "name");
        }
        for lane in &page.items {
            match kind {
                CompletionKind::Lane => names.push(lane.reference.clone()),
                CompletionKind::Project => names.push(lane.project.clone()),
                CompletionKind::Unit => {
                    for unit in &lane.units {
                        names.push(unit.name.clone());
                        names.push(format!("{}:{}", lane.reference, unit.name));
                    }
                }
                CompletionKind::Task
                | CompletionKind::Host
                | CompletionKind::Profile
                | CompletionKind::Setting => {}
            }
        }
        names
    };
    names.sort();
    names.dedup();
    Some(names)
}

/// Installs the hook; returns normally unless the shell is asking for completions.
pub fn install() {
    complete_environment(Arc::new(DaemonCandidates));
}
