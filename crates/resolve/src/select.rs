use devlanes_config::{Env, Extensions, ProfileConfig, ProjectConfig, ServiceConfig, Templated};
use globset::Glob;
use indexmap::IndexMap;

use crate::ResolveError;

/// Services to start and the profiles that apply to them.
#[derive(Debug, Clone)]
pub struct Selection {
    /// Service names in declaration order.
    pub services: Vec<String>,
    /// Profiles in effect, in the order given: the requested ones, else
    /// `["default"]` when the config defines it, else empty.
    pub profiles: Vec<String>,
    /// First entry of `profiles`, for single-profile callers.
    pub profile: Option<String>,
    /// What the profiles give the units they plan (see [`profile_settings`]).
    pub settings: ProfileSettings,
    /// Entries of `services` kept only because their `autostart` is an expression,
    /// which needs the lane to evaluate. [`retain_autostart`] drops the ones that
    /// evaluate to false.
    pub autostart_pending: Vec<String>,
}

/// Drops the `pending` entries of `services` (see [`Selection::autostart_pending`])
/// whose evaluated `autostart` is false. `autostart` answers per service as
/// [`crate::ResolvedLane::autostart`] does: true when any replica's is, `None` when none
/// planned. A service without an answer stays, so the start reports its failure.
pub fn retain_autostart(
    services: &mut Vec<String>,
    pending: &[String],
    autostart: impl Fn(&str) -> Option<bool>,
) {
    services.retain(|name| !pending.contains(name) || autostart(name) != Some(false));
}

/// One profile as clients list it (TUI picker, `devlanes profiles`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProfileSummary {
    pub name: String,
    /// Selector list, or `include` of the object form.
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    /// `infra: false` written literally. An expression counts as false here; the
    /// lane start evaluates it.
    pub skips_infra: bool,
    /// The profile `devlanes start` uses without `--profile`.
    pub is_default: bool,
}

/// Lists the config's profiles in declaration order. Pure: no lane context.
pub fn profile_summaries<E: Extensions>(config: &ProjectConfig<E>) -> Vec<ProfileSummary> {
    config
        .profiles
        .iter()
        .map(|(name, profile)| {
            let (include, exclude, skips_infra) = match profile {
                ProfileConfig::Selectors(values) => (values.clone(), Vec::new(), false),
                ProfileConfig::Object(options) => (
                    options.include.clone().unwrap_or_default(),
                    options.exclude.clone().unwrap_or_default(),
                    matches!(options.infra, Some(Templated::Literal(false))),
                ),
            };
            ProfileSummary {
                name: name.clone(),
                include,
                exclude,
                skips_infra,
                is_default: name == "default",
            }
        })
        .collect()
}

/// Validates and orders requested profile names: blanks and repeats dropped,
/// `default` allowed even when the config lacks it (it then means "no profile"),
/// and `["default"]` when nothing was requested and the config defines it.
pub(crate) fn active_profiles<E: Extensions, P: AsRef<str>>(
    config: &ProjectConfig<E>,
    requested: &[P],
) -> Result<Vec<String>, ResolveError> {
    let mut names: Vec<String> = Vec::new();
    for name in requested
        .iter()
        .map(|n| n.as_ref().trim())
        .filter(|n| !n.is_empty())
    {
        if !config.profiles.contains_key(name) && name != "default" {
            return Err(ResolveError::UnknownProfile {
                name: name.to_owned(),
                available: config.profiles.keys().cloned().collect(),
            });
        }
        if !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    }
    if names.is_empty() && config.profiles.contains_key("default") {
        names.push("default".to_owned());
    }
    Ok(names)
}

/// What a set of profiles gives the units it plans, apart from which services
/// it selects: compared to decide whether two profile sets plan units alike.
/// A new profile option goes here when it changes how units are planned, and
/// into selection when it picks services.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileSettings {
    /// The profiles' `env`, merged in order (later wins), as written.
    pub env: Env,
    /// The profiles' `run_scripts`, merged in order.
    pub run_scripts: IndexMap<String, String>,
    /// The `infra` values that can turn infra off, in profile order without
    /// repeats; empty when infra stays on (no profile, or one leaving it on).
    /// Infra is off when every entry evaluates false.
    pub infra: Vec<Templated<bool>>,
}

impl ProfileSettings {
    /// `name` swapped by the profiles' `run_scripts`, once, not chained.
    pub fn remap_script<'a>(&'a self, name: &'a str) -> &'a str {
        self.run_scripts.get(name).map_or(name, String::as_str)
    }

    /// The settings that differ from `other`'s, worded as drift fields are:
    /// `env NAME` and `run_scripts NAME` per entry, then `infra`.
    pub fn differences(&self, other: &Self) -> Vec<String> {
        let mut fields = Vec::new();
        changed_entries("env", &self.env, &other.env, &mut fields);
        changed_entries(
            "run_scripts",
            &self.run_scripts,
            &other.run_scripts,
            &mut fields,
        );
        if self.infra != other.infra {
            fields.push("infra".to_owned());
        }
        fields
    }
}

/// Adds `label KEY` to `fields` for each key whose value differs between
/// `ours` and `theirs`, or that only one of them has; `ours`' order first.
fn changed_entries<V: PartialEq>(
    label: &str,
    ours: &IndexMap<String, V>,
    theirs: &IndexMap<String, V>,
    fields: &mut Vec<String>,
) {
    let keys = ours
        .keys()
        .chain(theirs.keys().filter(|key| !ours.contains_key(*key)));
    for key in keys {
        if ours.get(key) != theirs.get(key) {
            fields.push(format!("{label} {key}"));
        }
    }
}

/// The settings of the profiles `names`, in that order. Names the config does
/// not have are left out, and `default` applies when none remain and the
/// config defines it, as planning does with a lane's saved profiles.
pub fn profile_settings<E: Extensions>(
    config: &ProjectConfig<E>,
    names: &[String],
) -> ProfileSettings {
    let mut configured: Vec<&ProfileConfig> = names
        .iter()
        .filter_map(|name| config.profiles.get(name))
        .collect();
    if configured.is_empty()
        && let Some(default) = config.profiles.get("default")
    {
        configured.push(default);
    }
    let mut settings = ProfileSettings::default();
    let mut infra_on = configured.is_empty();
    for profile in configured {
        let ProfileConfig::Object(options) = profile else {
            infra_on = true;
            continue;
        };
        settings
            .env
            .extend(options.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        settings.run_scripts.extend(
            options
                .run_scripts
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        match &options.infra {
            None | Some(Templated::Literal(true)) => infra_on = true,
            Some(value) if !settings.infra.contains(value) => settings.infra.push(value.clone()),
            Some(_) => {}
        }
    }
    if infra_on {
        settings.infra.clear();
    }
    settings
}

/// How a selector matched a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Match {
    No,
    /// By exact name or `+tag`.
    Explicit,
    /// By a glob pattern.
    Pattern,
}

/// Single-profile form of [`select_profiles`].
pub fn select<E: Extensions>(
    config: &ProjectConfig<E>,
    profile: Option<&str>,
    selectors: &[String],
) -> Result<Selection, ResolveError> {
    select_profiles(config, profile.as_slice(), selectors)
}

/// Picks the services for `devlanes start` with one or more profiles.
///
/// - `selectors` (names, globs, `+tag`) choose services directly; each must match
///   something. They may name `autostart: false` services. The profiles still
///   contribute their options.
/// - Without selectors, the union of each profile's list (or `include` minus
///   `exclude`) applies, in declaration order. Inside a profile, `autostart:
///   false` services are included only by exact name or `+tag`, so `include: ["*"]`
///   keeps on-demand services stopped.
/// - Without profiles, `default` is used when defined; otherwise every
///   `autostart` service starts.
/// - Where `autostart` matters and is an expression, the service is kept and
///   listed in `autostart_pending`; [`retain_autostart`] settles it once the lane
///   is resolved.
///
/// `settings` merges the profiles' `env` and `run_scripts` in the order given
/// (later wins) and lists the `infra` values that can turn infra off;
/// [`crate::resolve_lane`] evaluates templated `infra` values per lane.
pub fn select_profiles<E: Extensions, P: AsRef<str>>(
    config: &ProjectConfig<E>,
    profiles: &[P],
    selectors: &[String],
) -> Result<Selection, ResolveError> {
    let names = active_profiles(config, profiles)?;
    let configured: Vec<&ProfileConfig> = names
        .iter()
        .filter_map(|name| config.profiles.get(name))
        .collect();
    let mut autostart_pending = Vec::new();
    let services = if !selectors.is_empty() {
        let mut chosen = vec![false; config.services.len()];
        for selector in selectors {
            let mut any = false;
            for (index, (name, service)) in config.services.iter().enumerate() {
                if matches(selector, name, service.tags.as_deref())? != Match::No {
                    chosen[index] = true;
                    any = true;
                }
            }
            if !any {
                return Err(ResolveError::NoMatch {
                    selector: selector.clone(),
                    candidates: config.services.keys().cloned().collect(),
                });
            }
        }
        config
            .services
            .keys()
            .zip(chosen)
            .filter(|(_, chosen)| *chosen)
            .map(|(name, _)| name.clone())
            .collect()
    } else if !configured.is_empty() {
        let mut result = Vec::new();
        for (name, service) in &config.services {
            let mut keep = Keep::No;
            for profile in &configured {
                keep = keep.max(profile_selects(profile, name, service)?);
            }
            if keep == Keep::Pending {
                autostart_pending.push(name.clone());
            }
            if keep != Keep::No {
                result.push(name.clone());
            }
        }
        result
    } else {
        let mut result = Vec::new();
        for (name, service) in &config.services {
            let keep = autostart(&service.autostart);
            if keep == Keep::Pending {
                autostart_pending.push(name.clone());
            }
            if keep != Keep::No {
                result.push(name.clone());
            }
        }
        result
    };
    Ok(Selection {
        services,
        profile: names.first().cloned(),
        settings: profile_settings(config, &names),
        profiles: names,
        autostart_pending,
    })
}

/// Whether selection keeps a service, ordered so the strongest reason wins
/// across profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Keep {
    No,
    /// Kept unless its `autostart` expression evaluates to false for the lane.
    Pending,
    Yes,
}

/// Whether one profile's `include` minus `exclude` keeps `service`.
fn profile_selects<E: Extensions>(
    profile: &ProfileConfig,
    name: &str,
    service: &ServiceConfig<E>,
) -> Result<Keep, ResolveError> {
    let (include, exclude): (&[String], &[String]) = match profile {
        ProfileConfig::Selectors(values) => (values, &[]),
        ProfileConfig::Object(options) => (
            options.include.as_deref().unwrap_or_default(),
            options.exclude.as_deref().unwrap_or_default(),
        ),
    };
    let tags = service.tags.as_deref();
    let mut included = Match::No;
    for selector in include {
        match matches(selector, name, tags)? {
            Match::Explicit => included = Match::Explicit,
            Match::Pattern if included == Match::No => included = Match::Pattern,
            _ => {}
        }
    }
    for selector in exclude {
        if matches(selector, name, tags)? != Match::No {
            return Ok(Keep::No);
        }
    }
    Ok(match included {
        Match::Explicit => Keep::Yes,
        Match::Pattern => autostart(&service.autostart),
        Match::No => Keep::No,
    })
}

/// `autostart`, defaulting to true. An expression needs the lane, so the service
/// stays pending until [`retain_autostart`].
fn autostart(value: &Option<Templated<bool>>) -> Keep {
    match value {
        None | Some(Templated::Literal(true)) => Keep::Yes,
        Some(Templated::Literal(false)) => Keep::No,
        Some(Templated::Expression(_)) => Keep::Pending,
    }
}

/// Matches one selector against a service name and its tags.
fn matches(selector: &str, name: &str, tags: Option<&[String]>) -> Result<Match, ResolveError> {
    if let Some(tag) = selector.strip_prefix('+') {
        let hit = tags.is_some_and(|tags| tags.iter().any(|t| t == tag));
        return Ok(if hit { Match::Explicit } else { Match::No });
    }
    if selector == name {
        return Ok(Match::Explicit);
    }
    if !selector.contains(['*', '?', '[', '{']) {
        return Ok(Match::No);
    }
    let glob = Glob::new(selector).map_err(|e| ResolveError::InvalidSelector {
        selector: selector.to_owned(),
        message: e.to_string(),
    })?;
    Ok(if glob.compile_matcher().is_match(name) {
        Match::Pattern
    } else {
        Match::No
    })
}
