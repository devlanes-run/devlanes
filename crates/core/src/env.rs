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
}

/// Assembles layers by precedence regardless of caller order. Within a layer,
/// later entries win. Dotenv values belong to expression evaluation, not this API.
/// Names go through [`fold_name`], so on Windows `Path` from one layer and
/// `PATH` from another are one variable.
pub fn assemble(
    layers: impl IntoIterator<Item = (EnvLayer, BTreeMap<String, String>)>,
) -> Environment {
    let mut layers: Vec<_> = layers.into_iter().collect();
    layers.sort_by_key(|(layer, _)| *layer);
    let mut result = Environment::default();
    for (layer, values) in layers {
        for (key, value) in values {
            let key = fold_name(key);
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

/// An environment variable name as devlanes keeps it: uppercased on Windows,
/// where names ignore case, so `Path` and `PATH` are one key wherever
/// environments are captured, merged or looked up. The folding is
/// [`crate::paths::fold_case`]'s, which is how Windows compares names.
#[cfg(windows)]
pub fn fold_name(name: String) -> String {
    crate::paths::fold_case(&name)
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

/// What replaces a hidden secret value.
pub const REDACTED: &str = "[redacted]";

/// Name fragments of variables that likely hold secrets.
const SECRET_WORDS: [&str; 10] = [
    "TOKEN",
    "SECRET",
    "PASS",
    "KEY",
    "CREDENTIAL",
    "AUTH",
    "PRIVATE",
    "SESSION",
    "COOKIE",
    "DSN",
];

/// Whether a variable named `name` likely holds a secret.
pub fn secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_WORDS.iter().any(|word| upper.contains(word))
}

/// Values of the secret-named variables in `vars`, longest first so a value
/// containing another is replaced whole. Values under 6 characters ("1", "yes")
/// are left out: they would blank out unrelated text.
pub fn secret_values<K: AsRef<str>, V: AsRef<str>>(
    vars: impl IntoIterator<Item = (K, V)>,
) -> Vec<String> {
    let mut values: Vec<String> = vars
        .into_iter()
        .filter(|(name, value)| secret_name(name.as_ref()) && value.as_ref().len() >= 6)
        .map(|(_, value)| value.as_ref().to_owned())
        .collect();
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values.dedup();
    values
}

/// `text` with every one of `values` replaced by [`REDACTED`].
pub fn hide_values(text: &str, values: &[String]) -> String {
    let mut out = text.to_owned();
    for value in values {
        out = out.replace(value.as_str(), REDACTED);
    }
    out
}

/// `text` with the password of each `scheme://user:password@host` replaced by
/// [`REDACTED`].
pub fn hide_url_passwords(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find("://") {
        let (before, after) = rest.split_at(index + 3);
        out.push_str(before);
        let end = after
            .find(|c: char| c.is_whitespace() || "/\"'".contains(c))
            .unwrap_or(after.len());
        let authority = &after[..end];
        match (authority.rfind('@'), authority.find(':')) {
            (Some(at), Some(colon)) if colon < at => {
                out.push_str(&authority[..=colon]);
                out.push_str(REDACTED);
                out.push_str(&authority[at..]);
            }
            _ => out.push_str(authority),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// The value of variable `name` safe to show: [`REDACTED`] for a secret-looking
/// name, else `value` with `secrets` (from [`secret_values`] of the same
/// environment) and URL passwords hidden.
pub fn redact_value(name: &str, value: &str, secrets: &[String]) -> String {
    if secret_name(name) {
        REDACTED.to_owned()
    } else {
        hide_url_passwords(&hide_values(value, secrets))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Secret names, copies of their values under other names and URL passwords
    /// are hidden; short secret values are not hunted down elsewhere.
    fn redacts_secret_looking_values() {
        let vars = [
            ("ACME_API_TOKEN", "tok-4711-very-secret"),
            ("ALIAS", "Bearer tok-4711-very-secret"),
            ("SHORT_KEY", "abc"),
            ("NOTE", "abc"),
            ("DATABASE_URL", "postgres://u:hunter22@127.0.0.1:5432/app"),
            ("SITE", "https://web.acme.localhost"),
            ("PORT", "3000"),
        ];
        let secrets = secret_values(vars);
        let shown: BTreeMap<&str, String> = vars
            .iter()
            .map(|(name, value)| (*name, redact_value(name, value, &secrets)))
            .collect();
        assert_eq!(shown["ACME_API_TOKEN"], REDACTED);
        assert_eq!(shown["ALIAS"], format!("Bearer {REDACTED}"));
        assert_eq!(shown["SHORT_KEY"], REDACTED);
        assert_eq!(shown["NOTE"], "abc");
        assert_eq!(
            shown["DATABASE_URL"],
            format!("postgres://u:{REDACTED}@127.0.0.1:5432/app")
        );
        assert_eq!(shown["SITE"], "https://web.acme.localhost");
        assert_eq!(shown["PORT"], "3000");
    }

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
        #[cfg(unix)]
        {
            assert_eq!(env.values.len(), 2);
            assert_eq!(fold_name("Path".into()), "Path");
            assert_eq!(fold_names(mixed.clone()), mixed);
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
        }
    }
}
