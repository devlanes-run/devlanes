//! The agent plugin's `PreToolUse` hook for Claude Code's PowerShell tool, run
//! as `devlanes __agent-hook --shell powershell` from
//! `agent-plugin/hooks/hooks.json`. It denies PowerShell commands that start a
//! dev server in the background inside a devlanes project, with the message
//! `agent-plugin/scripts/block-background.sh` gives for the Bash tool, and uses
//! the same notion of a long-running command.
//!
//! The plugin's command ends in `; exit 0`, so a missing devlanes, or an older
//! one whose usage error exits 2 (which the agent reads as a block), lets every
//! call pass. That also hides this hook's exit status, so a block is a JSON
//! `deny` on stdout with exit 0 instead of the script's exit 2.

use std::{
    ffi::OsString,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

/// First argument that runs the hook instead of the CLI. Hidden: the plugin's
/// hook calls it, people do not.
pub const ARG: &str = "__agent-hook";

/// What the agent reads when a call is blocked: the Bash hook's text.
pub const BLOCK_MESSAGE: &str = "\
Blocked by the devlanes plugin: this starts a long-running process in the background, where nothing supervises it and its logs are lost.
Use instead:
  devlanes start [UNIT...]               services defined in devlanes.yaml
  devlanes exec --name NAME -- COMMAND   any other long-running command
Then check it with `devlanes wait NAME --timeout 180s` and `devlanes logs NAME --tail 100`.";

/// The hook's decision for one tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Block,
}

/// Runs the hook with the arguments after [`ARG`]: reads the tool call from
/// stdin and prints [`deny_output`] for a blocked one. Returns the exit
/// status, always 0. Arguments other than `--shell powershell` come from a
/// newer plugin and allow the call; so does `DEVLANES_BLOCK_BACKGROUND=0`,
/// which turns the Bash hook off too.
pub fn run(args: impl IntoIterator<Item = OsString>) -> u8 {
    let mut input = Vec::new();
    // Drained first, so the agent never writes into a closed pipe.
    if std::io::stdin().read_to_end(&mut input).is_err() {
        return 0;
    }
    let args: Vec<OsString> = args.into_iter().collect();
    if args != ["--shell", "powershell"]
        || std::env::var_os("DEVLANES_BLOCK_BACKGROUND").is_some_and(|value| value == "0")
    {
        return 0;
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let home = std::env::home_dir();
    if decide(&String::from_utf8_lossy(&input), &cwd, home.as_deref()) == Verdict::Block {
        let mut stdout = std::io::stdout().lock();
        // An agent that stopped reading gets no decision either way.
        let _ = writeln!(stdout, "{}", deny_output()).and_then(|()| stdout.flush());
    }
    0
}

/// The `PreToolUse` answer that denies the call with [`BLOCK_MESSAGE`] as the
/// reason the agent sees.
pub fn deny_output() -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": BLOCK_MESSAGE,
        }
    })
    .to_string()
}

/// Decides one PowerShell `PreToolUse` payload. `cwd` stands in when the
/// payload names no directory; `home` resolves `cd ~`. A payload this hook
/// cannot read is allowed.
///
/// A command is blocked when it runs in the background (a trailing `&`,
/// `run_in_background`, `Start-Process` or cmd's `start` without a wait,
/// `Start-Job`, `Start-ThreadJob`), looks like a dev server, and runs in a
/// directory, after any `cd` before it, inside a project with `devlanes.yaml`.
pub fn decide(payload: &str, cwd: &Path, home: Option<&Path>) -> Verdict {
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(payload) else {
        return Verdict::Allow;
    };
    let input = &payload["tool_input"];
    let Some(command) = input["command"].as_str() else {
        return Verdict::Allow;
    };
    let background = input["run_in_background"].as_bool() == Some(true);
    let mut dir = match payload["cwd"].as_str() {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => cwd.to_path_buf(),
    };
    if script_blocks(command, background, &mut dir, home) {
        Verdict::Block
    } else {
        Verdict::Allow
    }
}

/// A piece of a PowerShell script.
#[derive(Debug)]
enum Token {
    /// A word with its quotes and escapes removed.
    Word(String),
    /// The text inside `{ }`: a script block, hashtable or `${name}`.
    Block(String),
    /// `|`, `&&` or `||`: the next command is part of the same chain.
    Join,
    /// `;` or a line break.
    End,
    /// A `&` that ends a chain, which PowerShell 7 then runs as a job.
    Background,
}

/// Whether `script` starts a dev server in the background inside a project.
/// `detached` holds for all of it, as in a job's script block. A `cd` moves
/// `dir` for the commands after it.
fn script_blocks(script: &str, detached: bool, dir: &mut PathBuf, home: Option<&Path>) -> bool {
    commands(lex(script))
        .into_iter()
        .any(|(tokens, background)| command_blocks(&tokens, detached || background, dir, home))
}

/// Splits `script` into tokens. Comments (`#`, `<# #>`) are dropped, and so is
/// the call operator `&` at the start of a command (`& 'C:\tools\vite.cmd'`).
fn lex(script: &str) -> Vec<Token> {
    let chars: Vec<char> = script.chars().collect();
    let mut tokens = Vec::new();
    let mut word = String::new();
    // Set once a word has started, so `''` is an empty word.
    let mut in_word = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\'' => {
                in_word = true;
                i = single_quoted(&chars, i + 1, &mut word);
                continue;
            }
            '"' => {
                in_word = true;
                i = double_quoted(&chars, i + 1, &mut word);
                continue;
            }
            // A backtick before a line break continues the line.
            '`' if matches!(next, Some('\n' | '\r')) => {
                i += if chars.get(i + 2) == Some(&'\n') {
                    3
                } else {
                    2
                };
                continue;
            }
            '`' => {
                in_word = true;
                word.extend(next);
                i += 2;
                continue;
            }
            '@' if !in_word && matches!(next, Some('\'' | '"')) => {
                in_word = true;
                match here_string(&chars, i) {
                    Some((body, end)) => {
                        word.push_str(&body);
                        i = end;
                        continue;
                    }
                    None => word.push(c),
                }
            }
            '#' if !in_word => {
                while chars.get(i).is_some_and(|&c| c != '\n') {
                    i += 1;
                }
                continue;
            }
            '<' if !in_word && next == Some('#') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '#' && chars.get(i + 1) == Some(&'>')) {
                    i += 1;
                }
                i += 2;
                continue;
            }
            '{' => {
                end_word(&mut word, &mut in_word, &mut tokens);
                let end = block_end(&chars, i + 1);
                tokens.push(Token::Block(chars[i + 1..end].iter().collect()));
                i = end + 1;
                continue;
            }
            ' ' | '\t' | '\r' | ',' | '(' | ')' | '}' => {
                end_word(&mut word, &mut in_word, &mut tokens)
            }
            '@' if next == Some('(') => end_word(&mut word, &mut in_word, &mut tokens),
            ';' | '\n' => {
                end_word(&mut word, &mut in_word, &mut tokens);
                tokens.push(Token::End);
            }
            '|' => {
                end_word(&mut word, &mut in_word, &mut tokens);
                if next == Some('|') {
                    i += 1;
                }
                tokens.push(Token::Join);
            }
            '&' if next == Some('&') => {
                end_word(&mut word, &mut in_word, &mut tokens);
                i += 1;
                tokens.push(Token::Join);
            }
            // A redirection such as `2>&1`.
            '&' if in_word && word.ends_with('>') => word.push(c),
            // The call operator, also in `$p = & npm run dev`.
            '&' if !in_word
                && match tokens.last() {
                    None | Some(Token::End | Token::Join) => true,
                    Some(Token::Word(word)) => word == "=",
                    _ => false,
                } => {}
            '&' => {
                end_word(&mut word, &mut in_word, &mut tokens);
                tokens.push(Token::Background);
            }
            _ => {
                in_word = true;
                word.push(c);
            }
        }
        i += 1;
    }
    end_word(&mut word, &mut in_word, &mut tokens);
    tokens
}

/// Ends the current word, if one started.
fn end_word(word: &mut String, in_word: &mut bool, tokens: &mut Vec<Token>) {
    if *in_word {
        tokens.push(Token::Word(std::mem::take(word)));
    }
    *in_word = false;
}

/// Reads a `'...'` string from just after its quote into `word`, where `''`
/// is one quote, and returns the index after the closing quote.
fn single_quoted(chars: &[char], mut i: usize, word: &mut String) -> usize {
    while i < chars.len() {
        if chars[i] == '\'' {
            if chars.get(i + 1) != Some(&'\'') {
                return i + 1;
            }
            i += 1;
        }
        word.push(chars[i]);
        i += 1;
    }
    i
}

/// Reads a `"..."` string from just after its quote into `word`, where
/// `` ` `` escapes and `""` is one quote, and returns the index after the
/// closing quote.
fn double_quoted(chars: &[char], mut i: usize, word: &mut String) -> usize {
    while i < chars.len() {
        match chars[i] {
            '`' if i + 1 < chars.len() => {
                word.push(chars[i + 1]);
                i += 2;
            }
            '"' if chars.get(i + 1) == Some(&'"') => {
                word.push('"');
                i += 2;
            }
            '"' => return i + 1,
            c => {
                word.push(c);
                i += 1;
            }
        }
    }
    i
}

/// The body of a here-string whose `@` is at `at`, and the index after it.
/// `@'` or `@"` ends its line, and the string closes at a line that starts
/// with `'@` or `"@`; unclosed, it runs to the end. `None` when `@'` is not a
/// here-string. Read as one word, so a `'` or `&` in a commit message neither
/// quotes nor splits.
fn here_string(chars: &[char], at: usize) -> Option<(String, usize)> {
    let quote = chars[at + 1];
    let mut start = at + 2;
    while matches!(chars.get(start), Some(' ' | '\t' | '\r')) {
        start += 1;
    }
    if chars.get(start) != Some(&'\n') {
        return None;
    }
    start += 1;
    let mut i = start;
    while i < chars.len() {
        if (i == start || chars[i - 1] == '\n')
            && chars[i] == quote
            && chars.get(i + 1) == Some(&'@')
        {
            return Some((chars[start..i].iter().collect(), i + 2));
        }
        i += 1;
    }
    Some((chars[start..].iter().collect(), chars.len()))
}

/// The index of the `}` that closes a block whose text starts at `i`, or the
/// end of the script. Braces inside quotes do not count.
fn block_end(chars: &[char], mut i: usize) -> usize {
    let mut depth = 0;
    let mut quoted = String::new();
    while i < chars.len() {
        match chars[i] {
            '\'' => {
                i = single_quoted(chars, i + 1, &mut quoted);
                continue;
            }
            '"' => {
                i = double_quoted(chars, i + 1, &mut quoted);
                continue;
            }
            '`' => i += 1,
            '{' => depth += 1,
            '}' if depth == 0 => return i,
            '}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    chars.len()
}

/// Groups tokens into simple commands, each with whether it runs in the
/// background. Commands chained with `|`, `&&` or `||` share the `&` that ends
/// their chain, as in the Bash hook.
fn commands(tokens: Vec<Token>) -> Vec<(Vec<Token>, bool)> {
    let mut commands = Vec::new();
    let mut chain = Vec::new();
    let mut current = Vec::new();
    for token in tokens {
        let background = match token {
            Token::Join => {
                chain.push(std::mem::take(&mut current));
                continue;
            }
            Token::End => false,
            Token::Background => true,
            word => {
                current.push(word);
                continue;
            }
        };
        chain.push(std::mem::take(&mut current));
        commands.extend(chain.drain(..).map(|command| (command, background)));
    }
    chain.push(current);
    commands.extend(chain.drain(..).map(|command| (command, false)));
    commands.retain(|(command, _)| !command.is_empty());
    commands
}

/// Whether one simple command starts a dev server in the background inside a
/// project. Script blocks of other commands (`if`, `ForEach-Object`) are
/// judged like the script around them.
fn command_blocks(
    tokens: &[Token],
    detached: bool,
    dir: &mut PathBuf,
    home: Option<&Path>,
) -> bool {
    let (mut words, mut blocks) = (Vec::new(), Vec::new());
    for token in tokens {
        match token {
            Token::Word(word) => words.push(word.as_str()),
            Token::Block(block) => blocks.push(block.as_str()),
            _ => {}
        }
    }
    // `$job = Start-Job { ... }`, `$p=Start-Process ...`
    if let Some(&first) = words.first().filter(|word| word.starts_with('$')) {
        if words.get(1) == Some(&"=") {
            words.drain(..2);
        } else if let Some((_, value)) = first.split_once('=') {
            if value.is_empty() {
                words.remove(0);
            } else {
                words[0] = value;
            }
        }
    }
    let name = words
        .first()
        .map(|first| program(first))
        .unwrap_or_default();
    let args = words.get(1..).unwrap_or_default();
    match name.as_str() {
        "cd" | "chdir" | "sl" | "set-location" | "pushd" | "push-location" => {
            change_dir(args, dir, home);
            false
        }
        "start-process" | "saps" | "start" => start_process(args, dir, home),
        "start-job" | "sajb" | "start-threadjob" => {
            let dir = working_directory(args, dir, home);
            blocks
                .iter()
                .any(|block| script_blocks(block, true, &mut dir.clone(), home))
        }
        "cmd" => cmd(args, detached, dir),
        _ => {
            (detached && serves(&words, dir))
                || blocks
                    .iter()
                    .any(|block| script_blocks(block, detached, &mut dir.clone(), home))
        }
    }
}

/// `Start-Process` returns at once unless `-Wait` (`-Wait:$false` does not
/// wait). Its program and `-ArgumentList` run in `-WorkingDirectory`, else
/// `dir`. Opening a URL starts a browser, not a server.
fn start_process(args: &[&str], dir: &Path, home: Option<&Path>) -> bool {
    let mut target = Vec::new();
    let mut rest = args.iter();
    while let Some(&arg) = rest.next() {
        let Some(name) = parameter(arg) else {
            target.push(arg);
            continue;
        };
        let (name, inline) = match name.split_once(':') {
            Some((name, value)) => (name.to_owned(), Some(value.to_owned())),
            None => (name, None),
        };
        if name == "wait" {
            if !inline.is_some_and(|value| matches!(value.as_str(), "$false" | "0")) {
                return false;
            }
            continue;
        }
        // `-WorkingDirectory` is read below; these and the common parameters
        // take values that are not part of the started command. `-FilePath`,
        // `-ArgumentList`, `-NoNewWindow` and `-PassThru` fall through.
        let takes_value = matches!(
            name.as_str(),
            "workingdirectory"
                | "windowstyle"
                | "verb"
                | "credential"
                | "redirectstandardinput"
                | "redirectstandardoutput"
                | "redirectstandarderror"
        ) || COMMON_VALUE_PARAMETERS
            .split(' ')
            .any(|common| common == name);
        if takes_value && inline.is_none() {
            rest.next();
        }
    }
    start_target(&target, &working_directory(args, dir, home))
}

/// PowerShell's common parameters that take a value, with their aliases.
const COMMON_VALUE_PARAMETERS: &str = "erroraction ea warningaction wa informationaction infa \
                                       progressaction proga errorvariable ev warningvariable wv \
                                       informationvariable iv outvariable ov outbuffer ob \
                                       pipelinevariable pv";

/// The command a `Start-Process` or cmd `start` starts, judged as detached.
fn start_target(target: &[&str], dir: &Path) -> bool {
    !target.first().is_some_and(|first| first.contains("://")) && serves(target, dir)
}

/// `-WorkingDirectory DIR` or `-WorkingDirectory:DIR` from a `Start-Job` or
/// `Start-Process` command, resolved against `dir`, else `dir`.
fn working_directory(args: &[&str], dir: &Path, home: Option<&Path>) -> PathBuf {
    args.iter()
        .enumerate()
        .find_map(|(at, arg)| {
            let name = parameter(arg)?;
            if name == "workingdirectory" {
                return args.get(at + 1).copied();
            }
            name.strip_prefix("workingdirectory:")?;
            arg.split_once(':').map(|(_, value)| value)
        })
        .map_or_else(|| dir.to_path_buf(), |value| moved(dir, value, home))
}

/// `cmd /c COMMAND` (or `/k`). cmd's `start` returns at once unless `/wait`,
/// and runs in `/d DIR`, else `dir`; any other command runs in the foreground
/// of the PowerShell command around it.
fn cmd(args: &[&str], detached: bool, dir: &Path) -> bool {
    let Some(at) = args
        .iter()
        .position(|arg| arg.eq_ignore_ascii_case("/c") || arg.eq_ignore_ascii_case("/k"))
    else {
        return false;
    };
    // cmd reads its command as one line, so `cmd /c "start pnpm dev"` is the
    // same as unquoted.
    let words: Vec<&str> = args[at + 1..]
        .iter()
        .flat_map(|arg| arg.split_whitespace())
        .collect();
    let Some((first, rest)) = words.split_first() else {
        return false;
    };
    if !first.eq_ignore_ascii_case("start") {
        return detached && serves(&words, dir);
    }
    let mut dir = dir.to_path_buf();
    let mut target = Vec::new();
    let mut rest = rest.iter();
    while let Some(&arg) = rest.next() {
        if !target.is_empty() || !arg.starts_with('/') {
            target.push(arg);
        } else if arg.eq_ignore_ascii_case("/wait") {
            return false;
        } else if arg.eq_ignore_ascii_case("/d")
            && let Some(value) = rest.next()
        {
            dir = moved(&dir, value, None);
        }
    }
    start_target(&target, &dir)
}

/// The lowercase name of a `-Name` or `-Name:value` parameter word.
fn parameter(word: &str) -> Option<String> {
    let name = word.strip_prefix('-')?;
    (!name.is_empty()).then(|| name.to_ascii_lowercase())
}

/// `Set-Location DIR` and its aliases, also with `-Path` or `-LiteralPath`.
/// `cd -` goes back to a directory this hook does not know, so it stays put.
fn change_dir(args: &[&str], dir: &mut PathBuf, home: Option<&Path>) {
    if let Some(target) = args.iter().find(|arg| !arg.starts_with('-')) {
        *dir = moved(dir, target, home);
    }
}

/// `target` resolved against `dir`, with `~` as `home`, and `.` and `..`
/// resolved without touching the filesystem. A directory that does not exist
/// still counts, as in the Bash hook.
fn moved(dir: &Path, target: &str, home: Option<&Path>) -> PathBuf {
    let path = match (target.strip_prefix('~'), home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            home.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => dir.join(target),
    };
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    resolved.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    resolved.pop();
                }
            }
            other => resolved.push(other),
        }
    }
    resolved
}

/// A command name without its directory, case or executable extension:
/// `C:\tools\Vite.CMD` is `vite`.
fn program(word: &str) -> String {
    let name = word
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(word)
        .to_ascii_lowercase();
    for extension in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(stem) = name.strip_suffix(extension) {
            return stem.to_owned();
        }
    }
    name
}

/// Whether `words`, started in the background in `dir`, is a dev server
/// inside a project. A backgrounded devlanes command, such as `devlanes logs
/// -f`, passes. Quoted arguments split on blanks, as in the Bash hook, so
/// `-ArgumentList 'run dev'` reads `dev`; the program stays one word, since
/// its path may hold a space (`& "C:\Program Files\nodejs\npm.cmd" run dev &`).
fn serves(words: &[&str], dir: &Path) -> bool {
    let Some((&first, rest)) = words.split_first() else {
        return false;
    };
    let mut split = vec![first];
    split.extend(rest.iter().flat_map(|word| word.split_whitespace()));
    program(first) != "devlanes" && long_running(&split) && in_project(dir)
}

/// Words that name a long-running script or command: `pnpm dev`, `npm start`,
/// `docker compose up`, or a server tool run without `build` or `test`. The
/// lists are the Bash hook's `long_running` cases.
fn long_running(words: &[&str]) -> bool {
    let listed = |list: &str, word: &str| list.split(' ').any(|item| item == word);
    let script = words.iter().any(|word| {
        listed(LONG_RUNNING_WORDS, word)
            || ["server.js", "server.mjs", "server.ts", "server.py"]
                .iter()
                .any(|suffix| word.ends_with(suffix))
    });
    script
        || words.first().is_some_and(|first| {
            listed(SERVER_TOOLS, &program(first))
                && !words
                    .iter()
                    .any(|word| listed("build test lint check", word))
        })
}

/// Arguments that name a dev script or a serving subcommand.
const LONG_RUNNING_WORDS: &str =
    "dev start serve server preview watch --watch storybook runserver up http.server";

/// Programs that serve until stopped unless given a one-shot subcommand.
const SERVER_TOOLS: &str = "vite next nuxt astro wrangler nodemon webpack-dev-server uvicorn \
                            gunicorn flask hugo http-server live-server";

/// devlanes exec and devlanes start only make sense inside a devlanes project.
fn in_project(dir: &Path) -> bool {
    dir.ancestors()
        .filter(|dir| !dir.as_os_str().is_empty())
        .any(|dir| dir.join("devlanes.yaml").is_file())
}
