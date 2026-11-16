//! The skill and the docs pages it sends agents to name commands and flags that exist.

use clap::{Parser, error::ErrorKind};
use devlanes_cli::Cli;

/// Docs pages the skill sends agents to, besides the skill's own files.
const PAGES: &[&str] = &[
    "docs/how-to/agents/set-up.md",
    "docs/how-to/agents/sandboxes.md",
    "docs/how-to/ci/tests.md",
];

/// The skill's `SKILL.md`, every file under its `references/`, and [`PAGES`],
/// relative to the repository root.
fn agent_files(root: &str) -> Vec<String> {
    let references = format!("{root}/skills/devlanes/references");
    let mut files: Vec<String> = std::fs::read_dir(&references)
        .unwrap_or_else(|error| panic!("{references}: {error}"))
        .map(|entry| {
            let name = entry.expect("directory entry").file_name();
            format!("skills/devlanes/references/{}", name.to_string_lossy())
        })
        .collect();
    files.sort();
    files.insert(0, "skills/devlanes/SKILL.md".to_owned());
    files.extend(PAGES.iter().map(|page| (*page).to_owned()));
    files
}

/// Every command in `text` that starts with `devlanes `: spans between
/// backticks in prose, and lines of fenced blocks after a `$ ` prompt and
/// without a trailing `# comment`. Prose and blocks are split first, since a
/// fence's backticks would shift which spans count as code.
fn quoted_commands(text: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut prose = String::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if fenced {
            let line = line.trim();
            let line = line.strip_prefix("$ ").unwrap_or(line);
            let line = line.split(" #").next().unwrap_or(line).trim_end();
            if line.starts_with("devlanes ") {
                commands.push(line.to_owned());
            }
        } else {
            prose.push_str(line);
            prose.push('\n');
        }
    }
    commands.extend(
        prose
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|span| span.starts_with("devlanes "))
            .map(str::to_owned),
    );
    commands
}

/// Catches commands in what agents read that a CLI change renamed or removed.
/// Usage forms such as `devlanes with [units...] -- COMMAND` are skipped, and
/// a bare `devlanes exec` naming the command without its arguments passes.
#[test]
fn every_command_agents_read_parses() {
    let root = format!("{}/../..", env!("CARGO_MANIFEST_DIR"));
    let mut failures = Vec::new();
    for file in agent_files(&root) {
        let path = format!("{root}/{file}");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{file}: {error}"));
        let commands = quoted_commands(&text);
        assert!(!commands.is_empty(), "{file}: found no commands");
        for command in commands.iter().filter(|command| !command.contains('[')) {
            match Cli::try_parse_from(command.split_whitespace()) {
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::MissingRequiredArgument
                            | ErrorKind::DisplayHelp
                            | ErrorKind::DisplayVersion
                            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                    ) => {}
                Err(error) => failures.push(format!("{file}: `{command}` does not parse: {error}")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
