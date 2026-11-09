//! The environment of a command outside a unit: `exec` and the lane's `env`.

use std::{collections::BTreeMap, sync::Arc};

use devlanes_core::{
    contrib::runtime_ca_env,
    env::{EnvLayer, Environment, assemble},
};
use devlanes_proto::RpcError;
use devlanes_resolve::{DownReason, LeftOut};
use serde_json::{Value, json};

use crate::{
    engine::Inner,
    model::{EnvMap, LaneRt, LaunchRecord, ProjectRt, Unassigned, unassigned},
    resolve::resolve_error,
};

/// What a command outside a unit brings to its environment.
pub(crate) struct CommandEnvRequest<'a> {
    /// The env the request carried: the Client layer and what the CA rule checks.
    pub shell: Option<&'a EnvMap>,
    /// `exec --env`.
    pub overrides: &'a EnvMap,
}

/// A command's environment with provenance and what was left out of it.
pub(crate) struct CommandEnv {
    pub env: Environment,
    pub left_out: BTreeMap<String, LeftOut>,
}

impl From<CommandEnv> for LaunchRecord {
    /// The launch record of an exec run.
    fn from(command: CommandEnv) -> Self {
        Self {
            env: command.env,
            notes: Vec::new(),
            left_out: command.left_out,
        }
    }
}

/// The `left_out` field of `exec`, `env` and `env <unit>` answers:
/// `{key, infra}` with the container each variable waits on, and `reason:
/// engine_not_running` when Docker itself does not run.
pub(crate) fn left_out(entries: &BTreeMap<String, LeftOut>) -> Value {
    entries
        .iter()
        .map(|(key, entry)| {
            let mut item = json!({
                "key": devlanes_core::env::fold_name(key.clone()),
                "infra": entry.infra,
            });
            if entry.reason == DownReason::EngineNotRunning {
                item["reason"] = json!("engine_not_running");
            }
            item
        })
        .collect()
}

impl Inner {
    /// The environment of a command outside a unit in `lane`, assembled as
    /// Client (the shell), Contributor (the CA variables neither the shell nor
    /// an override sets), Devlanes, Global, Profile and Override. `${{ env.X }}`
    /// reads the lane's stored env, else the shell. A value that reads a
    /// container that is down is left out; one that reads a container whose
    /// state Docker did not give refuses with `infra_unknown`, unless an
    /// override replaces it. Changes nothing but the lane's live infra.
    pub(crate) async fn command_env(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        request: CommandEnvRequest<'_>,
    ) -> Result<CommandEnv, RpcError> {
        let reference = lane.st().reference(&project.name());
        let (config, stored) = {
            let st = lane.st();
            if st.record().is_none() {
                return Err(st
                    .unassigned
                    .clone()
                    .or_else(|| st.last_error.clone())
                    .unwrap_or_else(|| unassigned(&reference, Unassigned::NoRecord)));
            }
            (st.effective_config(Some(&reference))?, st.env().cloned())
        };
        let inputs = self.lane_inputs(project, lane, &config).await?;
        let none = EnvMap::new();
        let env = stored.as_deref().or(request.shell).unwrap_or(&none);
        let (namespace, states) = match self.current_infra(project, lane).await {
            Ok(current) => (current.namespace, current.states),
            // Without the declared names no `infra.<name>` can be told from a
            // typo; that matters only to a value that reads one.
            Err(error) => {
                let input = inputs.input(self, &config, env, &Value::Null);
                if devlanes_resolve::command_reads_infra(&input) {
                    return Err(error);
                }
                (Value::Null, BTreeMap::new())
            }
        };
        let input = inputs.input(self, &config, env, &namespace);
        let mut layers = devlanes_resolve::lane_command_env(&input, &states)
            .map_err(|error| resolve_error(&error, &reference))?;
        // A command must never run with a port the config leaves to a server
        // devlanes does not run.
        let own = crate::external::lane_ports(&reference, [], &namespace);
        if let Some(error) = self.external_conflict(&config, lane, own) {
            return Err(error);
        }
        layers
            .left_out
            .retain(|key, _| !request.overrides.contains_key(key));
        layers
            .unknown
            .retain(|key, _| !request.overrides.contains_key(key));
        if !layers.unknown.is_empty() {
            let keys: Vec<Value> = layers
                .unknown
                .iter()
                .map(|(key, infra)| {
                    json!({"key": devlanes_core::env::fold_name(key.clone()), "infra": infra})
                })
                .collect();
            return Err(RpcError::new(
                "infra_unknown",
                json!({"lane": reference, "keys": keys}),
                1,
            ));
        }
        let shell = request.shell.cloned().unwrap_or_default();
        let runtime_ca = runtime_ca_env(&layers.devlanes, |name| {
            shell.contains_key(name) || request.overrides.contains_key(name)
        });
        let env = assemble([
            (EnvLayer::Client, shell),
            (EnvLayer::Contributor, runtime_ca),
            (EnvLayer::Devlanes, layers.devlanes),
            (EnvLayer::Global, layers.global),
            (EnvLayer::Profile, layers.profile),
            (EnvLayer::Override, request.overrides.clone()),
        ]);
        Ok(CommandEnv {
            env,
            left_out: layers.left_out,
        })
    }
}
