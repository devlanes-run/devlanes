//! In-memory environment precedence and provenance, independent of config loading.

use std::collections::BTreeMap;

/// Layers are ordered from the lowest to highest precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EnvLayer {
    Client,
    Contributor,
    Devlanes,
    Global,
    Service,
    Profile,
    Override,
}

/// Effective environment with provenance. No serialization or debug implementation
/// is provided because both values and overridden values may contain secrets.
#[derive(Default)]
pub struct Environment {
    pub values: BTreeMap<String, String>,
    pub sources: BTreeMap<String, EnvLayer>,
    /// For each key of `values` whose name the winning layer spelled
    /// differently (Windows only: `Path` for `PATH`), that spelling. Empty
    /// on Unix, where names are not folded.
    pub names: BTreeMap<String, String>,
}

impl Environment {
    /// The name a process gets for `key`, a key of `values`.
    pub fn name<'a>(&'a self, key: &'a str) -> &'a str {
        self.names.get(key).map_or(key, String::as_str)
    }

    /// `values` keyed by the names a process gets.
    pub fn spelled(&self) -> BTreeMap<String, String> {
        self.values
            .iter()
            .map(|(key, value)| (self.name(key).to_owned(), value.clone()))
            .collect()
    }
}

/// Assembles layers by precedence regardless of caller order. Within a layer,
/// later entries win. Dotenv values belong to expression evaluation, not this API.
/// Names go through [`fold_name`] for merging, so on Windows `Path` from one
/// layer and `PATH` from another are one variable; the winning entry's
/// spelling is kept in [`Environment::names`].
pub fn assemble(
    layers: impl IntoIterator<Item = (EnvLayer, BTreeMap<String, String>)>,
) -> Environment {
    let mut layers: Vec<_> = layers.into_iter().collect();
    layers.sort_by_key(|(layer, _)| *layer);
    let mut result = Environment::default();
    for (layer, values) in layers {
        for (name, value) in values {
            let key = fold_name(name.clone());
            if key == name {
                result.names.remove(&key);
            } else {
                result.names.insert(key.clone(), name);
            }
            result.sources.insert(key.clone(), layer);
            result.values.insert(key, value);
        }
    }
    result
}

/// An environment variable name as devlanes keeps it: unchanged on Unix,
/// where names are case-sensitive. The browser (wasm32) build keeps them as
/// Unix does.
#[cfg(not(windows))]
pub fn fold_name(name: String) -> String {
    name
}

/// An environment variable name as devlanes compares it: uppercased on
/// Windows, where names ignore case, so `Path` and `PATH` are one key
/// wherever environments are merged or looked up. Processes get the spelling
/// from [`Environment::name`]. The folding is [`crate::paths::fold_case`]'s,
/// which is how Windows compares names.
#[cfg(windows)]
pub fn fold_name(name: String) -> String {
    crate::paths::fold_case(&name)
}

/// For each name of `env` that folds to another name, folded name to
/// spelling; with two spellings of one name, the one iterated last. Empty on
/// Unix.
pub fn spellings(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut names = BTreeMap::new();
    for name in env.keys() {
        let key = fold_name(name.clone());
        if &key == name {
            names.remove(&key);
        } else {
            names.insert(key, name.clone());
        }
    }
    names
}

/// `env`, keyed by folded names, with each name spelled as `spellings` has it.
pub fn respell(
    env: BTreeMap<String, String>,
    spellings: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    env.into_iter()
        .map(|(key, value)| match spellings.get(&key) {
            Some(name) => (name.clone(), value),
            None => (key, value),
        })
        .collect()
}

/// The value of `name` in `env`, whose names keep their spelling: exact.
#[cfg(not(windows))]
pub fn lookup<'a>(env: &'a BTreeMap<String, String>, name: &str) -> Option<&'a String> {
    env.get(name)
}

/// The value of `name` in `env`, whose names keep their spelling: any
/// spelling matches on Windows (`Path` for `PATH`).
#[cfg(windows)]
pub fn lookup<'a>(env: &'a BTreeMap<String, String>, name: &str) -> Option<&'a String> {
    env.get(name).or_else(|| {
        let key = fold_name(name.to_owned());
        env.iter()
            .find(|(candidate, _)| fold_name((*candidate).clone()) == key)
            .map(|(_, value)| value)
    })
}

/// Puts `layer` over `env`.
#[cfg(not(windows))]
pub fn overlay(
    env: &mut BTreeMap<String, String>,
    layer: impl IntoIterator<Item = (String, String)>,
) {
    env.extend(layer);
}

/// Puts `layer` over `env`. Each of its names first removes every variable of
/// `env` Windows reads as the same name, so the result has the layer's
/// spelling and value once. `layer` itself has one spelling per name.
#[cfg(windows)]
pub fn overlay(
    env: &mut BTreeMap<String, String>,
    layer: impl IntoIterator<Item = (String, String)>,
) {
    let layer: Vec<_> = layer.into_iter().collect();
    let folded: std::collections::BTreeSet<String> = layer
        .iter()
        .map(|(name, _)| fold_name(name.clone()))
        .collect();
    env.retain(|name, _| !folded.contains(&fold_name(name.clone())));
    env.extend(layer);
}

/// `env` unchanged: Unix names are case-sensitive.
#[cfg(not(windows))]
pub fn fold_names(env: BTreeMap<String, String>) -> BTreeMap<String, String> {
    env
}

/// `env` with every name through [`fold_name`]; when two names fold to one,
/// the value of the one sorting last wins.
#[cfg(windows)]
pub fn fold_names(env: BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.into_iter()
        .map(|(name, value)| (fold_name(name), value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Caller iteration order cannot invert the documented environment precedence.
    fn precedence_is_independent_of_input_order() {
        let env = assemble([
            (
                EnvLayer::Override,
                BTreeMap::from([("PORT".into(), "3".into())]),
            ),
            (
                EnvLayer::Client,
                BTreeMap::from([("PORT".into(), "1".into())]),
            ),
            (
                EnvLayer::Service,
                BTreeMap::from([("PORT".into(), "2".into())]),
            ),
        ]);
        assert_eq!(env.values["PORT"], "3");
        assert_eq!(env.sources["PORT"], EnvLayer::Override);
    }

    #[test]
    /// Names keep their case on Unix and fold to uppercase on Windows, so a
    /// later layer's `PATH` replaces an earlier layer's `Path` there.
    fn names_fold_only_on_windows() {
        let env = assemble([
            (
                EnvLayer::Client,
                BTreeMap::from([("Path".into(), "client".into())]),
            ),
            (
                EnvLayer::Service,
                BTreeMap::from([("PATH".into(), "service".into())]),
            ),
        ]);
        let mixed = BTreeMap::from([("Path".to_owned(), "a".to_owned())]);
        let mut overlaid = BTreeMap::from([
            ("Path".to_owned(), "a".to_owned()),
            ("X".to_owned(), "1".to_owned()),
        ]);
        overlay(&mut overlaid, [("PATH".to_owned(), "b".to_owned())]);
        #[cfg(unix)]
        {
            assert_eq!(env.values.len(), 2);
            assert_eq!(fold_name("Path".into()), "Path");
            assert_eq!(fold_names(mixed.clone()), mixed);
            assert_eq!(overlaid.len(), 3);
        }
        #[cfg(windows)]
        {
            assert_eq!(
                env.values,
                BTreeMap::from([("PATH".to_owned(), "service".to_owned())])
            );
            assert_eq!(env.sources["PATH"], EnvLayer::Service);
            assert_eq!(fold_name("ProgramFiles(x86)".into()), "PROGRAMFILES(X86)");
            assert_eq!(fold_name("straße".into()), "STRAßE");
            assert_eq!(
                fold_names(mixed),
                BTreeMap::from([("PATH".to_owned(), "a".to_owned())])
            );
            assert_eq!(
                overlaid,
                BTreeMap::from([
                    ("PATH".to_owned(), "b".to_owned()),
                    ("X".to_owned(), "1".to_owned()),
                ])
            );
        }
    }
}
