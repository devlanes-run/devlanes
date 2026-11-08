//! What devlanes hides before output leaves the process: secret values, URL
//! passwords, secret assignments and, in archives, the user's home. Every rule
//! deciding what is secret is here; callers pick the reader with a constructor.

use std::path::Path;

use serde_json::Value;

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

/// Name fragments of devlanes diagnostics fields whose values are never shown.
const FIELD_WORDS: [&str; 4] = ["secret", "password", "token", "value"];

/// Whether a variable named `name` likely holds a secret.
fn secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_WORDS.iter().any(|word| upper.contains(word))
}

/// Whether a devlanes diagnostics field named `name` must not be shown.
/// A separate list from variable names: fields are devlanes' own snake_case
/// names, and `env_keys` (variable names only) or `session` must stay
/// visible, while `KEY` and `SESSION` in a variable name mark a secret.
pub fn secret_field(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    FIELD_WORDS.iter().any(|word| lower.contains(word))
}

/// Who reads the redacted output. Every difference between the two is here:
/// - copies of secret values: Screen only when they look like credentials
///   (8+ characters, no whitespace, not only letters), so ordinary words under
///   secret-looking names (`SESSIONNAME=Console`, `SESSION_DRIVER=database`)
///   stay readable; Archive any value of 6+ characters, since a missed secret
///   in a bug report costs more than a hidden word;
/// - `env` objects in JSON: Screen hides their secret-looking values,
///   Archive hides every value;
/// - home: Screen has none; Archive replaces each spelling of the home it was
///   given by `~` (see [`hide_home`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Output {
    Screen,
    Archive,
}

impl Output {
    /// Whether copies of `value`, held by a secret-named variable, are hidden
    /// wherever they appear. Values under 6 characters ("1", "yes") never are:
    /// they would blank out unrelated text.
    fn hides_copies_of(self, value: &str) -> bool {
        match self {
            Self::Screen => {
                value.len() >= 8
                    && !value.chars().any(char::is_whitespace)
                    && !value.chars().all(|c| c.is_ascii_alphabetic())
            }
            Self::Archive => value.len() >= 6,
        }
    }
}

/// Hides secrets in variable values, text and JSON for one kind of output.
#[derive(Debug, Clone)]
pub struct Redactor {
    output: Output,
    /// Longest first, so a value containing another is replaced whole.
    values: Vec<String>,
    /// Spellings of the user's home, such as its long and 8.3 short paths.
    homes: Vec<String>,
}

impl Redactor {
    /// For output read on screen or by an agent (`env show --explain`), with
    /// the secrets of `vars`.
    pub fn screen<K: AsRef<str>, V: AsRef<str>>(vars: impl IntoIterator<Item = (K, V)>) -> Self {
        Self::new(Output::Screen, vars, Vec::new())
    }

    /// For a file attached to a bug report (`debug bundle`), with the secrets
    /// of `vars`; each of `homes`, the paths that name the user's home, is
    /// replaced by `~`.
    pub fn archive<K: AsRef<str>, V: AsRef<str>>(
        vars: impl IntoIterator<Item = (K, V)>,
        homes: impl IntoIterator<Item = impl AsRef<Path>>,
    ) -> Self {
        let homes = homes
            .into_iter()
            .map(|home| home.as_ref().to_string_lossy().into_owned())
            .collect();
        Self::new(Output::Archive, vars, homes)
    }

    /// Learns the values of the secret-named variables in `vars` that `output`
    /// hides copies of.
    fn new<K: AsRef<str>, V: AsRef<str>>(
        output: Output,
        vars: impl IntoIterator<Item = (K, V)>,
        homes: Vec<String>,
    ) -> Self {
        let mut values: Vec<String> = vars
            .into_iter()
            .filter(|(name, value)| {
                secret_name(name.as_ref()) && output.hides_copies_of(value.as_ref())
            })
            .map(|(_, value)| value.as_ref().to_owned())
            .collect();
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        values.dedup();
        Self {
            output,
            values,
            homes,
        }
    }

    /// The value of variable `name` safe to show: [`REDACTED`] for a
    /// secret-looking name, else `value` with known secrets, URL passwords and
    /// the home hidden.
    pub fn value(&self, name: &str, value: &str) -> String {
        if secret_name(name) {
            REDACTED.to_owned()
        } else {
            self.hide_known(value)
        }
    }

    /// `text` with secret values, URL passwords and secret assignments hidden,
    /// and the home replaced; lines that are JSON documents go through
    /// [`Redactor::json`].
    pub fn text(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        // Runs of other lines are redacted together, so a value spanning lines
        // is still found.
        let mut plain = String::new();
        for line in text.split_inclusive('\n') {
            let body = line.trim_end_matches(['\r', '\n']);
            match serde_json::from_str::<Value>(body) {
                Ok(mut value) if value.is_object() || value.is_array() => {
                    out.push_str(&self.plain(&std::mem::take(&mut plain)));
                    self.json(&mut value);
                    out.push_str(&value.to_string());
                    out.push_str(&line[body.len()..]);
                }
                _ => plain.push_str(line),
            }
        }
        out.push_str(&self.plain(&plain));
        out
    }

    /// Hides strings under secret-looking keys, treats `env` objects by the
    /// output's rule, and runs every other string through [`Redactor::text`].
    /// Strings are redacted before serialization, so escaping cannot hide a
    /// match.
    pub fn json(&self, value: &mut Value) {
        self.walk(value, false);
    }

    /// [`Redactor::json`] below one node; `hide_all` inside an archive's `env`.
    fn walk(&self, value: &mut Value, hide_all: bool) {
        match value {
            Value::Object(map) => {
                for (key, entry) in map.iter_mut() {
                    if entry.is_string() && (hide_all || secret_name(key)) {
                        *entry = Value::from(REDACTED);
                    } else {
                        let env = self.output == Output::Archive && key == "env";
                        self.walk(entry, hide_all || env);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.walk(item, hide_all);
                }
            }
            Value::String(text) if hide_all => *text = REDACTED.to_owned(),
            Value::String(text) => *text = self.text(text),
            _ => {}
        }
    }

    /// Text without JSON documents, redacted.
    fn plain(&self, text: &str) -> String {
        hide_assignments(&self.hide_known(text))
    }

    /// `text` with known secret values, URL passwords and the home hidden.
    fn hide_known(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for value in &self.values {
            out = out.replace(value.as_str(), REDACTED);
        }
        let mut out = hide_url_passwords(&out);
        for home in &self.homes {
            out = hide_home(&out, home);
        }
        out
    }
}

/// `text` with the password of each `scheme://user:password@host` replaced by
/// [`REDACTED`].
fn hide_url_passwords(text: &str) -> String {
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

/// `NAME=value` with the value hidden up to whitespace or a quote, when NAME is
/// an upper-case variable name that passes [`secret_name`] or a lower-case
/// field name that passes [`secret_field`]. So `env_keys=A,B` stays while
/// `ACME_KEY=...` and `token=...` are hidden.
fn hide_assignments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('=') {
        let (before, after) = rest.split_at(index);
        let name_start = before
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .map_or(0, |i| i + 1);
        let name = &before[name_start..];
        out.push_str(before);
        out.push('=');
        let after = &after[1..];
        let made_of = |case: fn(&char) -> bool| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| case(&c) || c.is_ascii_digit() || c == '_')
        };
        let secret = (made_of(char::is_ascii_uppercase) && secret_name(name))
            || (made_of(char::is_ascii_lowercase) && secret_field(name));
        if secret {
            let end = after
                .find(|c: char| c.is_whitespace() || "\"'".contains(c))
                .unwrap_or(after.len());
            if end > 0 {
                out.push_str(REDACTED);
            }
            rest = &after[end..];
        } else {
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// `text` with `home` replaced by `~` wherever it names the home itself, not
/// the start of a longer name: the end of the text, a separator or a character
/// that cannot continue a file name must follow. On Unix the match is exact.
/// On Windows it ignores case, treats `\` and `/` alike, and matches the path
/// after the drive, taking the drive right before it into the match when
/// present (`C:`, the verbatim `\\?\C:` or Git Bash's `/c`). So
/// `C:\Users\someone`, `c:/users/someone`, `/c/Users/someone`,
/// `\\?\C:\Users\someone` and `HOMEPATH`'s `\Users\someone` all become `~`.
fn hide_home(text: &str, home: &str) -> String {
    let (drive, path) = split_drive(home);
    let path: Vec<char> = path.chars().map(fold).collect();
    // A home of `/` or `C:\` would replace every separator.
    if path.len() < 2 {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut index = 0;
    while index + path.len() <= chars.len() {
        let end = index + path.len();
        let found = chars[index..end]
            .iter()
            .zip(&path)
            .all(|(c, want)| fold(*c) == *want)
            && chars.get(end).is_none_or(|c| !continues_name(*c));
        if found {
            let start = index - drive_before(&chars[copied..index], drive);
            out.extend(&chars[copied..start]);
            out.push('~');
            copied = end;
            index = end;
        } else {
            index += 1;
        }
    }
    out.extend(&chars[copied..]);
    out
}

/// Whether `c` can continue a file name component, so a home followed by it is
/// the start of a longer name (`/home/al` in `/home/alice`, `/home/jos` in
/// `/home/josé`).
fn continues_name(c: char) -> bool {
    c.is_alphanumeric() || ".-_".contains(c)
}

/// How many characters at the end of `before` spell `drive`: `C:`, the
/// verbatim `\\?\C:` or Git Bash's mount `/c`.
fn drive_before(before: &[char], drive: Option<char>) -> usize {
    let Some(drive) = drive else { return 0 };
    let is_drive = |letter: &char| fold(*letter) == drive;
    match before {
        [.., '\\', '\\', '?', '\\', letter, ':'] if is_drive(letter) => 6,
        [.., letter, ':'] | [.., '/', letter] if is_drive(letter) => 2,
        _ => 0,
    }
}

/// `home` split into its drive letter, folded, and the path after it. Unix
/// paths have no drive.
#[cfg(not(windows))]
fn split_drive(home: &str) -> (Option<char>, &str) {
    (None, home)
}

/// `home` split into its drive letter, folded, and the path after it
/// (`C:\Users\someone` gives `C` and `\Users\someone`).
#[cfg(windows)]
fn split_drive(home: &str) -> (Option<char>, &str) {
    match home.as_bytes() {
        [letter, b':', ..] if letter.is_ascii_alphabetic() => {
            (Some(fold(char::from(*letter))), &home[2..])
        }
        _ => (None, home),
    }
}

/// One character as the OS compares paths: unchanged on Unix.
#[cfg(not(windows))]
fn fold(c: char) -> char {
    c
}

/// One character as Windows compares paths: case folded, and `/` as `\`.
#[cfg(windows)]
fn fold(c: char) -> char {
    if c == '/' {
        return '\\';
    }
    crate::paths::fold_case(c.encode_utf8(&mut [0; 4]))
        .chars()
        .next()
        .unwrap_or(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(not(windows))]
    const HOME: &str = "/Users/someone";
    #[cfg(windows)]
    const HOME: &str = r"C:\Users\someone";
    /// The home's spellings the archive is given: on Windows also its 8.3
    /// short path.
    #[cfg(not(windows))]
    const HOMES: [&str; 1] = [HOME];
    #[cfg(windows)]
    const HOMES: [&str; 2] = [HOME, r"C:\Users\SOMEON~1"];
    const SEP: &str = std::path::MAIN_SEPARATOR_STR;

    /// One call on a redactor.
    enum Call {
        Value(&'static str, &'static str),
        Text(String),
        Json(Value),
    }

    impl Call {
        /// What `redactor` returns for this call.
        fn run(&self, redactor: &Redactor) -> Value {
            match self {
                Self::Value(name, value) => redactor.value(name, value).into(),
                Self::Text(text) => redactor.text(text).into(),
                Self::Json(value) => {
                    let mut value = value.clone();
                    redactor.json(&mut value);
                    value
                }
            }
        }
    }

    #[test]
    /// Every method runs under both outputs, so tuning one output's rule shows
    /// its effect on the other here. Screen keeps word-like values of secret
    /// names readable and has no home; the archive hides any 6+ character
    /// secret, every `env` value and the home in each spelling.
    fn outputs_hide_what_their_readers_must_not_see() {
        let vars = [
            ("ACME_API_TOKEN", "tok-4711-very-secret"),
            ("ACME_SECRET", "correcthorsebattery"),
            ("ACME_PASSPHRASE", "staple gun river"),
            ("DB_PASS", "s3cr3t"),
            ("SESSIONNAME", "Console"),
            ("CI_PASS", "1"),
        ];
        let screen = Redactor::screen(vars);
        let archive = Redactor::archive(vars, HOMES);
        let text = |text: &str| Call::Text(text.to_owned());
        let log = format!("DEBUG spawning program=fixture-server cwd={HOME}{SEP}src{SEP}acme");
        let rows: Vec<(Call, Value, Value)> = vec![
            (
                Call::Value("ACME_API_TOKEN", "tok-4711-very-secret"),
                json!(REDACTED),
                json!(REDACTED),
            ),
            (
                Call::Value("ALIAS", "Bearer tok-4711-very-secret"),
                json!("Bearer [redacted]"),
                json!("Bearer [redacted]"),
            ),
            (
                Call::Value("TERMINAL", "Console"),
                json!("Console"),
                json!(REDACTED),
            ),
            (
                Call::Value("WORD", "correcthorsebattery"),
                json!("correcthorsebattery"),
                json!(REDACTED),
            ),
            (
                Call::Value("PHRASE", "staple gun river"),
                json!("staple gun river"),
                json!(REDACTED),
            ),
            (
                Call::Value("DATABASE_URL", "postgres://u:hunter22@h/app"),
                json!("postgres://u:[redacted]@h/app"),
                json!("postgres://u:[redacted]@h/app"),
            ),
            (Call::Value("RETRIES", "1"), json!("1"), json!("1")),
            // Variable and field names disagree on `KEY`.
            (
                text("env_keys=ACME_KEY,PORT ACME_KEY=k-1 token=abc PORT=3000"),
                json!("env_keys=ACME_KEY,PORT ACME_KEY=[redacted] token=[redacted] PORT=3000"),
                json!("env_keys=ACME_KEY,PORT ACME_KEY=[redacted] token=[redacted] PORT=3000"),
            ),
            (
                Call::Text(log.clone()),
                json!(log),
                json!(format!(
                    "DEBUG spawning program=fixture-server cwd=~{SEP}src{SEP}acme"
                )),
            ),
            (
                text("{\"env\":{\"PORT\":\"3000\"},\"note\":\"tok-4711-very-secret\"}\nexit 1\n"),
                json!("{\"env\":{\"PORT\":\"3000\"},\"note\":\"[redacted]\"}\nexit 1\n"),
                json!("{\"env\":{\"PORT\":\"[redacted]\"},\"note\":\"[redacted]\"}\nexit 1\n"),
            ),
            (
                Call::Json(json!({
                    "env": {"PORT": "3000", "ACME_API_TOKEN": "tok-4711-very-secret"},
                    "api_key": "k-123456",
                    "path": format!("{HOME}{SEP}x"),
                    "lane": "acme@0",
                })),
                json!({
                    "env": {"PORT": "3000", "ACME_API_TOKEN": REDACTED},
                    "api_key": REDACTED,
                    "path": format!("{HOME}{SEP}x"),
                    "lane": "acme@0",
                }),
                json!({
                    "env": {"PORT": REDACTED, "ACME_API_TOKEN": REDACTED},
                    "api_key": REDACTED,
                    "path": format!("~{SEP}x"),
                    "lane": "acme@0",
                }),
            ),
        ];
        // Unchanged on screen, `archived` in the archive.
        let home_row = |input: &str, archived: &str| (text(input), json!(input), json!(archived));
        // A longer name that starts with the home is someone else's.
        let longer_names = format!("{HOME}ice{SEP}x {HOME}é{SEP}x");
        let longer = home_row(&longer_names, &longer_names);
        #[cfg(not(windows))]
        let spellings = [longer];
        #[cfg(windows)]
        let spellings = [
            longer,
            home_row(
                r"log at c:\users\someone\.devlanes\logs\daemon.log",
                r"log at ~\.devlanes\logs\daemon.log",
            ),
            home_row(
                "DEVLANES_LANE_DATA=C:/Users/someone/.devlanes/data/acme/0",
                "DEVLANES_LANE_DATA=~/.devlanes/data/acme/0",
            ),
            home_row(r"HOMEPATH=\Users\someone", "HOMEPATH=~"),
            home_row("PWD=/c/Users/someone/src", "PWD=~/src"),
            home_row(r"opened \\?\C:\Users\someone\x", r"opened ~\x"),
            home_row(
                r"TEMP=C:\Users\SOMEON~1\AppData\Local\Temp",
                r"TEMP=~\AppData\Local\Temp",
            ),
        ];
        for (row, (call, on_screen, in_archive)) in rows.iter().chain(&spellings).enumerate() {
            assert_eq!(&call.run(&screen), on_screen, "row {row} on screen");
            assert_eq!(&call.run(&archive), in_archive, "row {row} in archive");
        }
    }
}
