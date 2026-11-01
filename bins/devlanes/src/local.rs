//! Actions that never talk to the daemon: reading or editing the user config,
//! and the box side of a remote connection (`tunnel`, `askpass`).

use devlanes_cli::{Action, Runtime};
use devlanes_client::Paths;
use devlanes_proto::RpcError;
use serde_json::json;

/// Routes the daemon-free actions.
pub async fn run(runtime: &Runtime, action: Action) -> Result<u8, RpcError> {
    match action {
        Action::ConfigGet(key) => crate::user_config::get(runtime, &paths()?, key.as_deref()).await,
        Action::ConfigSet { key, value } => {
            crate::user_config::set(runtime, &paths()?, &key, &value).await
        }
        Action::ConfigUnset(key) => crate::user_config::unset(runtime, &paths()?, &key).await,
        action @ (Action::Tunnel | Action::Askpass(_)) => crate::remote::run(action).await,
        _ => Err(RpcError::new(
            "usage",
            json!({"message": "unexpected action"}),
            2,
        )),
    }
}

/// The devlanes home's paths.
fn paths() -> Result<Paths, RpcError> {
    Paths::discover().map_err(|error| RpcError::new("io", json!({"message": error.to_string()}), 1))
}
