//! Builds the card into one HTML document, `OUT_DIR/lane.html`: compiles
//! `ui/styles/app.scss` with grass, joins the vendored htm/preact module and the
//! modules in `ui/js/` into one inline module script, hashes both for the
//! document's Content-Security-Policy and renders `ui/app.html` with askama.

use std::{collections::BTreeSet, env, fs, path::Path};

use askama::Template;
use base64::Engine;
use sha2::{Digest, Sha256};

/// The card's modules in dependency order: a module uses only names that the
/// vendored file and the modules before it declare.
const MODULES: &[&str] = &[
    "bridge.js",
    "util.js",
    "ui.js",
    "store.js",
    "lane.js",
    "board.js",
    "failure.js",
    "clean.js",
    "pair.js",
    "app.js",
];

/// The vendored htm/preact standalone module (see `ui/vendor/README.md`).
const VENDOR: &str = "ui/vendor/standalone.module.js";

#[derive(Template)]
#[template(path = "app.html")]
struct Shell<'a> {
    csp: &'a str,
    style: &'a str,
    script: &'a str,
}

fn main() {
    println!("cargo:rerun-if-changed=ui");
    let out = Path::new(&env::var_os("OUT_DIR").expect("set by cargo")).to_path_buf();

    let source = Path::new("ui/styles/app.scss");
    let options = grass::Options::default().style(grass::OutputStyle::Compressed);
    let css = grass::from_path(source, &options)
        .unwrap_or_else(|error| panic!("cannot compile {}: {error}", source.display()));
    let css = css.trim_end();

    let script = script();
    // The script sits inside <script>; this text would end it early.
    assert!(
        !script.to_ascii_lowercase().contains("</script"),
        "the card script must not contain </script"
    );
    let csp = format!(
        "default-src 'none'; script-src 'sha256-{}'; style-src 'sha256-{}'; base-uri 'none'; form-action 'none'",
        hash(&script),
        hash(css)
    );
    let html = Shell {
        csp: &csp,
        style: css,
        script: &script,
    }
    .render()
    .unwrap_or_else(|error| panic!("cannot render ui/app.html: {error}"));
    write(&out.join("lane.html"), &html);
}

/// The one inline module: the vendored file wrapped in a function whose exports
/// become top-level constants, then each module with its `import` lines,
/// `export` keywords, comments and needless whitespace removed. An inline module cannot
/// import another inline module, so they share one scope instead; a name two of
/// them declare fails the build here rather than in the browser.
fn script() -> String {
    let vendor = read(Path::new(VENDOR));
    let (body, exports) = split_exports(&vendor);
    let mut names = BTreeSet::from(["VERSION".to_owned()]);
    for (_, name) in &exports {
        declare(&mut names, name, VENDOR);
    }
    let local: Vec<&str> = exports.iter().map(|(_, name)| name.as_str()).collect();
    let returned: Vec<String> = exports
        .iter()
        .map(|(inner, name)| format!("{name}:{inner}"))
        .collect();
    let mut out = format!("const VERSION={:?};\n", devlanes_version::VERSION);
    out.push_str(&format!(
        "const{{{}}}=(()=>{{{body};return{{{}}}}})();\n",
        local.join(","),
        returned.join(",")
    ));
    for module in MODULES {
        let path = Path::new("ui/js").join(module);
        out.push_str(&squeeze(&module_body(&read(&path), module, &mut names)));
        out.push('\n');
    }
    out
}

/// `code` without the whitespace JavaScript does not need, so the card stays
/// in its size budget. Strings and regular expressions stay as written. In
/// template literals only a line break between two tags goes, which htm drops
/// anyway. The modules end every statement with `;`, so no line break is
/// needed for automatic semicolon insertion.
fn squeeze(code: &str) -> String {
    let chars: Vec<char> = code.chars().collect();
    let mut out = String::with_capacity(code.len());
    // The brace depth at each open `${`, whose `}` returns to the template text.
    let mut open: Vec<usize> = Vec::new();
    let mut depth = 0;
    let mut template = false;
    let mut space = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if template {
            match c {
                '`' => template = false,
                '\\' => {
                    out.push(c);
                    i += 1;
                    out.extend(next);
                }
                '$' if next == Some('{') => {
                    out.push_str("${");
                    open.push(depth);
                    template = false;
                    i += 2;
                    continue;
                }
                '\n' if out.ends_with('>') && next == Some('<') => {
                    i += 1;
                    continue;
                }
                _ => {}
            }
            if c != '\\' {
                out.push(c);
            }
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            space = true;
            i += 1;
            continue;
        }
        let last = out.chars().last();
        if space && last.is_some_and(|last| joins(last, c)) {
            out.push(' ');
        }
        space = false;
        match c {
            '\'' | '"' => i = copy_until(&chars, i, &mut out, c),
            '/' if last.is_none_or(|last| "(,=:[!&|?{};>+-*%~^<".contains(last)) => {
                i = copy_regex(&chars, i, &mut out);
            }
            '`' => {
                template = true;
                out.push(c);
                i += 1;
            }
            '}' if open.last() == Some(&depth) => {
                open.pop();
                template = true;
                out.push(c);
                i += 1;
            }
            _ => {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Whether two tokens that whitespace separated need a space to stay apart:
/// two names or keywords, or `+ +` and `- -`.
fn joins(last: char, next: char) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    (word(last) && word(next)) || (last == next && matches!(last, '+' | '-'))
}

/// Copies the string literal at `start`, quotes and escapes included, and
/// returns the index after it.
fn copy_until(chars: &[char], start: usize, out: &mut String, quote: char) -> usize {
    let mut i = start + 1;
    out.push(quote);
    while let Some(&c) = chars.get(i) {
        out.push(c);
        i += 1;
        if c == '\\' {
            out.extend(chars.get(i));
            i += 1;
        } else if c == quote {
            break;
        }
    }
    i
}

/// Copies the regular expression literal at `start` up to its closing `/`
/// (its flags follow as code) and returns the index after it.
fn copy_regex(chars: &[char], start: usize, out: &mut String) -> usize {
    let mut i = start + 1;
    let mut class = false;
    out.push('/');
    while let Some(&c) = chars.get(i) {
        out.push(c);
        i += 1;
        match c {
            '\\' => {
                out.extend(chars.get(i));
                i += 1;
            }
            '[' => class = true,
            ']' => class = false,
            '/' if !class => break,
            _ => {}
        }
    }
    i
}

/// Splits the vendored module into its code and its final `export{a as h,...}`
/// statement, as `(inner name, exported name)` pairs.
fn split_exports(source: &str) -> (&str, Vec<(String, String)>) {
    let start = source
        .rfind("export{")
        .unwrap_or_else(|| panic!("{VENDOR} has no export statement"));
    let rest = &source[start + "export{".len()..];
    let end = rest
        .find('}')
        .unwrap_or_else(|| panic!("{VENDOR}: unterminated export statement"));
    assert!(
        rest[end + 1..].trim().trim_start_matches(';').is_empty(),
        "{VENDOR}: code after the export statement"
    );
    let exports = rest[..end]
        .split(',')
        .map(|pair| match pair.split_once(" as ") {
            Some((inner, name)) => (inner.trim().to_owned(), name.trim().to_owned()),
            None => (pair.trim().to_owned(), pair.trim().to_owned()),
        })
        .collect();
    (source[..start].trim_end(), exports)
}

/// One module's code for the shared scope. Comments are dropped only where they
/// start a line, so `//` inside strings and URLs stays.
fn module_body(source: &str, module: &str, names: &mut BTreeSet<String>) -> String {
    let mut out = String::new();
    let mut in_comment = false;
    let mut in_import = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if in_comment {
            in_comment = !trimmed.contains("*/");
            continue;
        }
        if in_import {
            in_import = !trimmed.ends_with(';');
            continue;
        }
        if trimmed.starts_with("/*") {
            in_comment = !trimmed.contains("*/");
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if line.starts_with("import ") {
            in_import = !trimmed.ends_with(';');
            continue;
        }
        assert!(
            !trimmed.starts_with("export default"),
            "{module}: default exports have no name to share"
        );
        let code = line.strip_prefix("export ").unwrap_or(line);
        if !code.starts_with(char::is_whitespace)
            && let Some(name) = declared(code)
        {
            declare(names, name, module);
        }
        out.push_str(&strip_casts(code.trim()));
        out.push('\n');
    }
    out
}

/// `code` without its inline JSDoc type casts (`/** @type {Error} */ (error)`),
/// which only editors read.
fn strip_casts(code: &str) -> String {
    let mut out = code.to_owned();
    while let Some(start) = out.find("/** @type {") {
        let Some(len) = out[start..].find("*/") else {
            break;
        };
        out.replace_range(start..start + len + 2, "");
    }
    out
}

/// The name a top-level `function`, `class`, `const` or `let` line declares.
fn declared(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("async ").unwrap_or(line);
    let rest = ["function* ", "function ", "class ", "const ", "let "]
        .iter()
        .find_map(|keyword| rest.strip_prefix(keyword))?;
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// Records a top-level name, failing the build when another module took it.
fn declare(names: &mut BTreeSet<String>, name: &str, module: &str) {
    assert!(
        names.insert(name.to_owned()),
        "{module} declares {name}, which an earlier module already declares"
    );
}

/// Base64 SHA-256, the form a Content-Security-Policy hash source takes.
fn hash(text: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(Sha256::digest(text.as_bytes()))
}

/// Reads a source file, failing the build on error.
fn read(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// Writes a generated file, failing the build on error.
fn write(path: &Path, contents: &str) {
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}
