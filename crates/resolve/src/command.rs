//! The environment of a command outside a unit (`devlanes exec`, `env show`).

use std::{collections::BTreeMap, fmt};

use devlanes_config::{Env, Extensions, PreparedContext, ProfileConfig, Scalar, Templated};

use crate::{
    InfraState, ResolveError, ResolveInput,
    layout::{self, env_namespace},
    plan::{reads_unit, shared_globals},
    select::active_profiles,
};

/// The layers devlanes adds to a command outside a unit, lowest first after
/// the client env and contributors. `Debug` prints keys only, since values can
/// carry secrets.
#[derive(Clone, Default)]
pub struct CommandLayers {
    /// `DEVLANES_*`, `DEVLANES_PUBLIC_URL_<SERVICE>` and the CA paths.
    pub devlanes: BTreeMap<String, String>,
    pub global: BTreeMap<String, String>,
    pub profile: BTreeMap<String, String>,
    /// Entries that failed on a declared container that is down: variable to container.
    pub left_out: BTreeMap<String, String>,
    /// Entries that failed on a declared container whose state is unknown.
    pub unknown: BTreeMap<String, String>,
}

impl fmt::Debug for CommandLayers {
    /// Prints only the keys.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommandLayers")
            .field("devlanes", &self.devlanes.keys().collect::<Vec<_>>())
            .field("global", &self.global.keys().collect::<Vec<_>>())
            .field("profile", &self.profile.keys().collect::<Vec<_>>())
            .field("left_out", &self.left_out)
            .field("unknown", &self.unknown)
            .finish()
    }
}

/// The `global_env` and profile `env` entries of the lane's profiles (in the
/// order given, later ones winning) that read neither `self` nor `replica`,
/// with `devlanes`. `infra` maps every declared name to its state; a name it
/// lacks is not declared, so an entry reading it fails before evaluation. An
/// entry that fails while it reads a down container is left out, one that
/// reads a container in an unknown state goes in `unknown`; any other failure
/// is the error, the first in declaration order.
pub fn lane_command_env<E: Extensions>(
    input: &ResolveInput<'_, E>,
    infra: &BTreeMap<String, InfraState>,
) -> Result<CommandLayers, ResolveError> {
    command_layers(input, infra).map_err(|error| error.locate(input.config))
}

/// Whether an entry [`lane_command_env`] evaluates reads `infra`, so its
/// declared names are needed. An entry whose references cannot be read
/// counts, since its error must surface.
pub fn command_reads_infra<E: Extensions>(input: &ResolveInput<'_, E>) -> bool {
    let Ok(profiles) = active_profiles(input.config, &requested(input)) else {
        return true;
    };
    entries(input.config, &profiles).any(|entry| {
        reads_unit(entry.value) != Some(true)
            && infra_names(entry.value).is_none_or(|names| !names.is_empty())
    })
}

/// One `global_env` or profile `env` entry.
struct Entry<'c> {
    /// Its config path.
    path: String,
    key: &'c str,
    value: &'c Templated<Scalar>,
    /// In `global_env`, not a profile's `env`.
    global: bool,
}

/// The profiles `input` asks for, first `profile` then `profiles`.
fn requested<'a, E: Extensions>(input: &'a ResolveInput<'_, E>) -> Vec<&'a str> {
    input
        .profile
        .into_iter()
        .chain(input.profiles.iter().map(String::as_str))
        .collect()
}

/// `global_env`, then each profile's `env`, in declaration order.
fn entries<'c, E: Extensions>(
    config: &'c devlanes_config::ProjectConfig<E>,
    profiles: &'c [String],
) -> impl Iterator<Item = Entry<'c>> {
    let global = config.global_env.iter().map(|(key, value)| Entry {
        path: format!("global_env.{key}"),
        key,
        value,
        global: true,
    });
    let profile = profiles
        .iter()
        .filter_map(|name| match config.profiles.get(name) {
            Some(ProfileConfig::Object(options)) => Some((name, &options.env)),
            _ => None,
        })
        .flat_map(|(name, env): (&String, &Env)| {
            env.iter().map(move |(key, value)| Entry {
                path: format!("profiles.{name}.env.{key}"),
                key,
                value,
                global: false,
            })
        });
    global.chain(profile)
}

/// The names of the `infra.<name>` references in `value`; `None` when its
/// references cannot be read.
fn infra_names(value: &Templated<Scalar>) -> Option<Vec<String>> {
    let Templated::Expression(text) = value else {
        return Some(Vec::new());
    };
    let references = devlanes_config::references(text).ok()?;
    Some(
        references
            .iter()
            .filter_map(|reference| {
                let mut parts = reference.split('.');
                (parts.next() == Some("infra"))
                    .then(|| parts.next())
                    .flatten()
                    .map(str::to_owned)
            })
            .collect(),
    )
}

/// What became of one entry.
enum Outcome {
    Value(String),
    /// Failed on this down container.
    LeftOut(String),
    /// Failed on this container whose state is unknown.
    Unknown(String),
}

/// [`lane_command_env`] before errors get their source location.
fn command_layers<E: Extensions>(
    input: &ResolveInput<'_, E>,
    infra: &BTreeMap<String, InfraState>,
) -> Result<CommandLayers, ResolveError> {
    let config = input.config;
    let profiles = active_profiles(config, &requested(input))?;
    let env = env_namespace(input.client_env, input.dotenv);
    let (layout, ns) = layout::build(input, &env)?;
    let lane = ns.lane_context(&env).prepare();
    let mut layers = CommandLayers {
        devlanes: layout.lane_env,
        ..CommandLayers::default()
    };
    let shared = shared_globals(&config.global_env, &lane);
    let mut shared = shared.into_iter();
    for entry in entries(config, &profiles) {
        // `global_env` comes first, in the order `shared` holds it.
        let evaluated = if entry.global {
            shared.next().flatten()
        } else {
            None
        };
        if reads_unit(entry.value) == Some(true) {
            continue;
        }
        let outcome = outcome(&entry.path, entry.value, evaluated, &lane, infra)?;
        let key = entry.key.to_owned();
        // A profile's entry replaces whatever a lower layer had or left out, and a
        // lower layer's value must not show through where it is left out.
        layers.left_out.remove(&key);
        layers.unknown.remove(&key);
        if !matches!(outcome, Outcome::Value(_)) {
            layers.global.remove(&key);
            layers.profile.remove(&key);
        }
        match outcome {
            Outcome::Value(value) if entry.global => {
                layers.global.insert(key, value);
            }
            Outcome::Value(value) => {
                layers.profile.insert(key, value);
            }
            Outcome::LeftOut(container) => {
                layers.left_out.insert(key, container);
            }
            Outcome::Unknown(container) => {
                layers.unknown.insert(key, container);
            }
        }
    }
    Ok(layers)
}

/// Checks one entry's `infra.<name>` references against `infra`, then
/// evaluates it (`evaluated` holds a `global_env` value already evaluated)
/// and classifies a failure by the state of what it reads.
fn outcome(
    path: &str,
    value: &Templated<Scalar>,
    evaluated: Option<Result<String, ResolveError>>,
    lane: &PreparedContext,
    infra: &BTreeMap<String, InfraState>,
) -> Result<Outcome, ResolveError> {
    // Each reference with the declared name it reads; a misspelled one fails
    // even next to a down one.
    let mut read = Vec::new();
    for name in infra_names(value).unwrap_or_default() {
        let Some((declared, state)) = infra
            .iter()
            .find(|(declared, _)| devlanes_config::infra_alias(declared, &name))
        else {
            return Err(ResolveError::invalid(
                path,
                devlanes_config::unknown_infra_name(&name, infra.keys().map(String::as_str)),
            ));
        };
        read.push((declared, *state));
    }
    let error = match evaluated.unwrap_or_else(|| {
        value
            .evaluate(lane)
            .map(|value| value.to_string())
            .map_err(|e| ResolveError::expression(path, e))
    }) {
        Ok(value) => return Ok(Outcome::Value(value)),
        Err(error) => error,
    };
    let state = |wanted: fn(&InfraState) -> bool| {
        read.iter()
            .find(|(_, state)| wanted(state))
            .map(|(name, _)| (*name).clone())
    };
    if let Some(name) = state(|state| matches!(state, InfraState::Down(_))) {
        Ok(Outcome::LeftOut(name))
    } else if let Some(name) = state(|state| *state == InfraState::Unknown) {
        Ok(Outcome::Unknown(name))
    } else {
        Err(error)
    }
}
