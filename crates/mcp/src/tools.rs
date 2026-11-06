//! One MCP tool per curated devlanes command, generated from the CLI's clap
//! definition, and the way back from a call's arguments to the command line.
//!
//! clap stays the single source of truth: a tool's name is its subcommand
//! path, its description the command's help, and its input schema one
//! property per argument, so a new flag on a curated command reaches its tool
//! without a change here. A [`Spec`] only says which commands get a tool,
//! which of their arguments make no sense through MCP, and what an agent needs
//! to know beyond the help text.

use std::any::TypeId;

use devlanes_cli::{
    Cli,
    clap::{self, ArgAction, CommandFactory as _, Parser as _},
};
use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde_json::{Map, Value, json};

use crate::text;

/// What clap does not say about a curated command's tool.
pub struct Spec {
    /// The subcommand path, such as `["debug", "why"]`; the tool's name joins
    /// it with `_`.
    pub path: &'static [&'static str],
    /// Ids of the command's own arguments the tool leaves out.
    pub skip: &'static [&'static str],
    /// Global options the tool offers besides `lane`, from [`GLOBALS`].
    pub globals: &'static [&'static str],
    /// Whether the long help after its first paragraph goes in the description.
    pub long_help: bool,
    /// What an agent should know beyond the help text.
    pub guidance: &'static str,
    /// Calling it again with the same arguments changes nothing more.
    pub idempotent: bool,
    /// Whether its results can carry an MCP Apps card, so the tool names the
    /// cards resource.
    pub card: bool,
}

impl Spec {
    /// A spec for `path` with nothing else set, for struct update syntax.
    const NONE: Self = Self {
        path: &[],
        skip: &[],
        globals: &[],
        long_help: false,
        guidance: "",
        idempotent: false,
        card: false,
    };
}

/// The global options a tool may offer. Every tool takes `lane`, unless the
/// command has a positional lane of its own; the others only where
/// [`Spec::globals`] names them. Output modes, `--quiet`, `--verbose` and
/// `--budget` belong to the server, and `--project` and `--file` are rare
/// enough for the `devlanes` tool.
pub const GLOBALS: [&str; 4] = ["lane", "timeout", "env", "yes"];

/// Guidance shared by the commands that wait for units.
const BLOCKS: &str = "Returns once the units are ready or one failed, or at the timeout the \
server adds with what is ready so far; then call wait. No sleep or curl polling.";

/// The curated commands, in the order of `tools/list`.
pub const CURATED: &[Spec] = &[
    Spec {
        path: &["status"],
        // A stream, and the line under an editor's prompt.
        skip: &["watch", "statusline"],
        card: true,
        ..Spec::NONE
    },
    Spec {
        path: &["start"],
        // The TUI, which an MCP call never shows.
        skip: &["attach", "no_ui"],
        globals: &["timeout", "env"],
        long_help: true,
        guidance: BLOCKS,
        idempotent: true,
        card: true,
    },
    Spec {
        path: &["exec"],
        globals: &["env"],
        long_help: true,
        guidance: "Use it for dev servers, watchers, workers, tunnels and anything else that \
keeps running, never nohup or a background shell. command is the program and its arguments. The \
result says how to read and stop the unit.",
        ..Spec::NONE
    },
    Spec {
        path: &["logs"],
        globals: &["timeout"],
        long_help: true,
        guidance: "follow needs timeout.",
        card: true,
        ..Spec::NONE
    },
    Spec {
        path: &["stop"],
        long_help: true,
        guidance: "Stop your own units with mine when they are done, and leave the lane running \
when you hand work to the user for review.",
        idempotent: true,
        ..Spec::NONE
    },
    Spec {
        path: &["restart"],
        skip: &["no_ui"],
        globals: &["timeout", "env"],
        long_help: true,
        guidance: BLOCKS,
        card: true,
        ..Spec::NONE
    },
    Spec {
        path: &["reload"],
        skip: &["no_ui"],
        globals: &["timeout"],
        long_help: true,
        guidance: BLOCKS,
        idempotent: true,
        card: true,
    },
    Spec {
        path: &["wait"],
        globals: &["timeout"],
        long_help: true,
        guidance: "Call it when start, restart or reload returned at their timeout.",
        ..Spec::NONE
    },
    Spec {
        path: &["with"],
        globals: &["timeout"],
        guidance: "Use it for tests and other commands that exit and need the lane running. The \
command runs once the units are ready, with the lane's environment, and the result has its exit \
code and output.",
        ..Spec::NONE
    },
    Spec {
        path: &["run"],
        globals: &["env", "yes"],
        // Its long help is about terminals and Ctrl-C.
        guidance: "The result has the task's output. The server asks the user before a task with \
confirm: runs.",
        ..Spec::NONE
    },
    Spec {
        path: &["debug", "why"],
        guidance: "Call it first when a unit failed or is not ready.",
        card: true,
        ..Spec::NONE
    },
    Spec {
        path: &["debug", "errors"],
        card: true,
        ..Spec::NONE
    },
    Spec {
        path: &["debug", "top"],
        guidance: "The tool for any question about what uses CPU or memory: units and \
containers per lane, with the system's totals.",
        card: true,
        ..Spec::NONE
    },
    Spec {
        path: &["url", "print"],
        ..Spec::NONE
    },
    Spec {
        path: &["url", "ls"],
        ..Spec::NONE
    },
    Spec {
        path: &["env", "show"],
        ..Spec::NONE
    },
    Spec {
        path: &["lane", "ls"],
        ..Spec::NONE
    },
    Spec {
        path: &["lane", "clean"],
        globals: &["yes"],
        guidance: "The server shows the user what the clean removes and asks first. dry_run \
lists it without asking or changing anything.",
        card: true,
        ..Spec::NONE
    },
    Spec {
        path: &["daemon", "pair"],
        globals: &["yes"],
        guidance: "The server asks the user first. The code is in the text.",
        card: true,
        ..Spec::NONE
    },
    Spec {
        path: &["doctor"],
        ..Spec::NONE
    },
];

/// How an argument goes on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Form {
    /// A positional value.
    Positional,
    /// A flag, such as `--tail`, with its value after `=` when it takes one.
    Flag(String),
    /// The values after `--`, such as `exec`'s command.
    Last,
}

/// The JSON type of an argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Boolean,
    Integer,
    String,
    Array,
}

/// One property of a tool's input and the argument it becomes.
#[derive(Debug, Clone)]
struct Param {
    property: String,
    form: Form,
    kind: Kind,
    required: bool,
}

/// A generated tool and how to turn its arguments into a command line.
pub struct Generated {
    /// The tool name, such as `debug_why`.
    pub name: String,
    /// The tool as `tools/list` shows it, without `_meta`.
    pub tool: Tool,
    /// Whether [`devlanes_cli::Command::is_read_only`] holds for the command.
    pub read_only: bool,
    /// What the tool was generated from.
    pub spec: &'static Spec,
    params: Vec<Param>,
}

/// A call's command line and its `cwd`.
#[derive(Debug, PartialEq, Eq)]
pub struct Line {
    /// The arguments after `devlanes`.
    pub args: Vec<String>,
    pub cwd: Option<String>,
}

/// The curated tools a server offers.
pub struct Catalog {
    tools: Vec<Generated>,
}

impl Catalog {
    /// The tools of every curated command, or only of the read-only ones.
    /// A spec whose command is gone from the CLI is left out with a warning;
    /// a test makes sure none is.
    pub fn new(read_only: bool) -> Self {
        let mut root = Cli::command();
        // Propagates the global options and fills in each argument's num_args.
        root.build();
        let tools = CURATED
            .iter()
            .filter_map(|spec| match generate(&root, spec, is_read_only) {
                Ok(generated) => Some(generated),
                Err(error) => {
                    tracing::warn!(%error, "no MCP tool");
                    None
                }
            })
            .filter(|generated| !read_only || generated.read_only)
            .collect();
        Self { tools }
    }

    /// The tool named `name`.
    pub fn get(&self, name: &str) -> Option<&Generated> {
        self.tools.iter().find(|generated| generated.name == name)
    }

    /// Every tool, in [`CURATED`] order.
    pub fn iter(&self) -> impl Iterator<Item = &Generated> {
        self.tools.iter()
    }
}

/// Whether `devlanes ARGS` parses to a command that only reads.
fn is_read_only(args: &[String]) -> bool {
    Cli::try_parse_from(std::iter::once("devlanes".to_owned()).chain(args.iter().cloned()))
        .ok()
        .and_then(|cli| cli.command)
        .is_some_and(|command| command.is_read_only())
}

/// The tool of `spec` from the built command tree `root`. `read_only` tells
/// from the command's shortest valid line whether it only reads.
fn generate(
    root: &clap::Command,
    spec: &'static Spec,
    read_only: impl Fn(&[String]) -> bool,
) -> Result<Generated, String> {
    let name = spec.path.join("_");
    let mut command = root;
    for part in spec.path {
        command = command
            .find_subcommand(part)
            .ok_or_else(|| format!("devlanes has no command {}", spec.path.join(" ")))?;
    }
    let last = spec.path.last().copied().unwrap_or_default();
    let mut params = Vec::new();
    let mut properties = Map::new();
    let own = command.get_arguments().filter(|arg| {
        !arg.is_global_set()
            && !arg.is_hide_set()
            && !matches!(
                arg.get_action(),
                ArgAction::Help | ArgAction::HelpShort | ArgAction::HelpLong | ArgAction::Version
            )
            && !spec.skip.contains(&arg.get_id().as_str())
    });
    for arg in own {
        let param = param(arg, last);
        properties.insert(param.property.clone(), schema(arg, param.kind, None));
        params.push(param);
    }
    for id in GLOBALS {
        let offered = if id == "lane" {
            // A command's own lane argument says the same.
            !properties.contains_key("lane")
        } else {
            spec.globals.contains(&id)
        };
        if !offered {
            continue;
        }
        let arg = root
            .get_arguments()
            .find(|arg| arg.get_id() == id)
            .ok_or_else(|| format!("devlanes has no global option {id}"))?;
        let param = param(arg, last);
        properties.insert(
            param.property.clone(),
            schema(arg, param.kind, text::global_help(id)),
        );
        params.push(param);
    }
    if let Some(unknown) = spec
        .globals
        .iter()
        .find(|id| **id == "lane" || !GLOBALS.contains(id))
    {
        return Err(format!("{name} names the global option {unknown}"));
    }
    properties.insert(
        "cwd".into(),
        json!({"type": "string", "description": text::CWD}),
    );
    let required: Vec<&str> = params
        .iter()
        .filter(|param| param.required)
        .map(|param| param.property.as_str())
        .collect();
    let mut input = json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": false,
    });
    if !required.is_empty() {
        input["required"] = json!(required);
    }
    let input = match input {
        Value::Object(input) => input,
        _ => JsonObject::new(),
    };
    let shortest = shortest(spec, &params);
    let read_only = read_only(&shortest);
    let title = format!("devlanes {}", spec.path.join(" "));
    let annotations = if read_only {
        ToolAnnotations::with_title(title).read_only(true)
    } else {
        let annotations = ToolAnnotations::with_title(title)
            .read_only(false)
            .destructive(true);
        if spec.idempotent {
            annotations.idempotent(true)
        } else {
            annotations
        }
    }
    .open_world(false);
    let tool =
        Tool::new(name.clone(), description(command, spec), input).with_annotations(annotations);
    Ok(Generated {
        name,
        tool,
        read_only,
        spec,
        params,
    })
}

/// The property and form of `arg`, an argument of the subcommand `last`.
/// A flag's property is its long name with `_` for `-`. A positional's is its
/// id without a `last_` prefix: the CLI gives `status LANE` the id
/// `status_lane` only so it does not collide with the global `--lane`.
fn param(arg: &clap::Arg, last: &str) -> Param {
    let id = arg.get_id().as_str();
    let (property, form) = if arg.is_last_set() {
        (id.to_owned(), Form::Last)
    } else if arg.is_positional() {
        let property = id.strip_prefix(&format!("{last}_")).unwrap_or(id);
        (property.to_owned(), Form::Positional)
    } else if let Some(long) = arg.get_long() {
        (long.replace('-', "_"), Form::Flag(format!("--{long}")))
    } else {
        let short = arg.get_short().map(String::from).unwrap_or_default();
        (id.to_owned(), Form::Flag(format!("-{short}")))
    };
    Param {
        property,
        form,
        kind: kind(arg),
        required: arg.is_required_set(),
    }
}

/// The JSON type `arg` takes.
fn kind(arg: &clap::Arg) -> Kind {
    match arg.get_action() {
        ArgAction::SetTrue | ArgAction::SetFalse => Kind::Boolean,
        ArgAction::Count => Kind::Integer,
        ArgAction::Append => Kind::Array,
        _ if arg
            .get_num_args()
            .is_some_and(|range| range.max_values() > 1) =>
        {
            Kind::Array
        }
        _ => {
            let parsed = arg.get_value_parser().type_id();
            let integers = [
                TypeId::of::<usize>(),
                TypeId::of::<u64>(),
                TypeId::of::<u32>(),
                TypeId::of::<u16>(),
                TypeId::of::<u8>(),
                TypeId::of::<i64>(),
                TypeId::of::<i32>(),
            ];
            if integers.iter().any(|integer| parsed == *integer) {
                Kind::Integer
            } else {
                Kind::String
            }
        }
    }
}

/// The JSON Schema of `arg` as `kind`: an enum of its possible values, its
/// default, and its help (or `help`) as the description.
fn schema(arg: &clap::Arg, kind: Kind, help: Option<&str>) -> Value {
    let values: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| value.get_name().to_owned())
        .collect();
    let scalar = |ty: &str| {
        let mut scalar = json!({"type": ty});
        if !values.is_empty() {
            scalar["enum"] = json!(values);
        }
        scalar
    };
    let mut schema = match kind {
        Kind::Boolean => json!({"type": "boolean"}),
        Kind::Integer => json!({"type": "integer"}),
        Kind::String => scalar("string"),
        Kind::Array => json!({"type": "array", "items": scalar("string")}),
    };
    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    if let Some(first) = defaults.first().filter(|_| kind != Kind::Boolean) {
        schema["default"] = match kind {
            Kind::Integer => first
                .parse::<i64>()
                .map_or_else(|_| json!(first), |n| json!(n)),
            Kind::Array => json!(defaults),
            _ => json!(first),
        };
    }
    let help = help.map(str::to_owned).or_else(|| {
        arg.get_long_help()
            .or(arg.get_help())
            .map(|help| help.to_string().trim().to_owned())
    });
    if let Some(help) = help.filter(|help| !help.is_empty()) {
        schema["description"] = json!(help);
    }
    schema
}

/// The tool's description: the command's about line, its long help after the
/// first paragraph when the spec asks for it, unwrapped, and the guidance.
fn description(command: &clap::Command, spec: &Spec) -> String {
    let about = command
        .get_about()
        .map(ToString::to_string)
        .unwrap_or_default();
    // The server asks the user instead, and the tool has `yes` for a consent
    // given in the chat.
    let about = about.trim_end_matches(" (needs --yes)");
    let mut text = format!("{about}.");
    if spec.long_help
        && let Some(long) = command.get_long_about()
    {
        // The first paragraph repeats the about line.
        for paragraph in long.to_string().split("\n\n").skip(1) {
            text.push(' ');
            text.push_str(&paragraph.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }
    if !spec.guidance.is_empty() {
        text.push(' ');
        text.push_str(spec.guidance);
    }
    text
}

/// The shortest valid command line of a tool: its path, with a placeholder
/// for each required argument.
fn shortest(spec: &Spec, params: &[Param]) -> Vec<String> {
    let mut args: Vec<String> = spec.path.iter().map(|part| (*part).to_owned()).collect();
    let mut last = Vec::new();
    for param in params.iter().filter(|param| param.required) {
        match &param.form {
            Form::Positional => args.push("x".into()),
            Form::Flag(flag) => args.push(format!("{flag}=x")),
            Form::Last => last.extend(["--".to_owned(), "x".to_owned()]),
        }
    }
    args.extend(last);
    args
}

impl Generated {
    /// The command line of a call with `arguments`: the subcommand path, each
    /// argument in the order clap declares it, the global options, and the
    /// values after `--` last. Values go after `=` so one that starts with
    /// `-` stays a value; a positional value that starts with `-` is refused,
    /// since the command line would read it as a flag. Fails with the text
    /// the agent gets.
    pub fn line(&self, arguments: &JsonObject) -> Result<Line, String> {
        let mut cwd = None;
        for (key, value) in arguments {
            if key == "cwd" {
                cwd = match value {
                    Value::String(dir) => Some(dir.clone()),
                    Value::Null => None,
                    _ => return Err("cwd must be a string".into()),
                };
            } else if !self.params.iter().any(|param| param.property == *key) {
                let known: Vec<&str> = self
                    .params
                    .iter()
                    .map(|param| param.property.as_str())
                    .chain(["cwd"])
                    .collect();
                return Err(format!(
                    "{} takes no argument {key}; it takes {}",
                    self.name,
                    known.join(", ")
                ));
            }
        }
        let mut args: Vec<String> = self
            .spec
            .path
            .iter()
            .map(|part| (*part).to_owned())
            .collect();
        let mut trailing = Vec::new();
        for param in &self.params {
            let values = match arguments.get(&param.property) {
                None | Some(Value::Null) => Vec::new(),
                Some(value) => param.values(value).map_err(|expected| {
                    format!("{} of {} must be {expected}", param.property, self.name)
                })?,
            };
            if values.is_empty() {
                if param.required {
                    return Err(format!("{} needs {}", self.name, param.property));
                }
                continue;
            }
            match &param.form {
                Form::Flag(flag) if param.kind == Kind::Boolean => args.push(flag.clone()),
                Form::Flag(flag) => {
                    args.extend(values.iter().map(|value| format!("{flag}={value}")));
                }
                Form::Positional => {
                    if let Some(value) = values.iter().find(|value| value.starts_with('-')) {
                        return Err(format!(
                            "{} of {}: {value} starts with -, which the command line would read \
                             as a flag",
                            param.property, self.name
                        ));
                    }
                    args.extend(values);
                }
                Form::Last => trailing = values,
            }
        }
        if !trailing.is_empty() {
            args.push("--".into());
            args.extend(trailing);
        }
        Ok(Line { args, cwd })
    }

    /// Whether the tool has the property `name`.
    pub fn takes(&self, name: &str) -> bool {
        self.params.iter().any(|param| param.property == name)
    }
}

impl Param {
    /// The command-line values of `value`, or what the property expects: no
    /// value for `false`, one for `true` and for scalars, and one per item for
    /// arrays, which also take a single string.
    fn values(&self, value: &Value) -> Result<Vec<String>, &'static str> {
        let scalar = |value: &Value| match value {
            Value::String(text) => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        };
        match self.kind {
            Kind::Boolean => match value {
                Value::Bool(true) => Ok(vec!["true".into()]),
                Value::Bool(false) => Ok(Vec::new()),
                _ => Err("true or false"),
            },
            Kind::Integer => match value {
                Value::Number(number) if number.is_i64() || number.is_u64() => {
                    Ok(vec![number.to_string()])
                }
                Value::String(text) => Ok(vec![text.clone()]),
                _ => Err("an integer"),
            },
            Kind::String => scalar(value).map(|value| vec![value]).ok_or("a string"),
            Kind::Array => match value {
                Value::Array(items) => items
                    .iter()
                    .map(scalar)
                    .collect::<Option<Vec<_>>>()
                    .ok_or("an array of strings"),
                Value::String(text) => Ok(vec![text.clone()]),
                _ => Err("an array of strings"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Call, Rejected, Tool as Scope};

    /// `value` as a JSON object.
    fn object(value: Value) -> JsonObject {
        match value {
            Value::Object(object) => object,
            _ => JsonObject::new(),
        }
    }

    /// The command line of `tool` called with `arguments`, joined by spaces.
    fn line(catalog: &Catalog, tool: &str, arguments: Value) -> String {
        let generated = catalog
            .get(tool)
            .unwrap_or_else(|| panic!("no tool {tool}"));
        match generated.line(&object(arguments)) {
            Ok(line) => line.args.join(" "),
            Err(error) => panic!("{tool}: {error}"),
        }
    }

    /// Every curated command still exists and has its tool, every argument of
    /// it is in the tool's schema unless its spec skips it, and every skip
    /// names a real argument. A new flag on one of these commands therefore
    /// reaches its tool, or fails here until it is skipped on purpose.
    #[test]
    fn tools_cover_every_argument() {
        let mut root = Cli::command();
        root.build();
        let catalog = Catalog::new(false);
        for spec in CURATED {
            let name = spec.path.join("_");
            let tool = catalog
                .get(&name)
                .unwrap_or_else(|| panic!("no tool {name}"));
            let mut command = &root;
            for part in spec.path {
                command = command.find_subcommand(part).expect("subcommand");
            }
            let own: Vec<&clap::Arg> = command
                .get_arguments()
                .filter(|arg| {
                    !arg.is_global_set()
                        && !arg.is_hide_set()
                        && !matches!(arg.get_action(), ArgAction::Help | ArgAction::Version)
                })
                .collect();
            for skipped in spec.skip {
                assert!(
                    own.iter().any(|arg| arg.get_id() == *skipped),
                    "{name} skips {skipped}, which it does not have"
                );
            }
            let properties = tool.tool.input_schema["properties"]
                .as_object()
                .expect("properties");
            for arg in own {
                let id = arg.get_id().as_str();
                if spec.skip.contains(&id) {
                    continue;
                }
                let property = param(arg, spec.path.last().copied().unwrap_or_default()).property;
                assert!(
                    properties.contains_key(&property),
                    "{name} has no property for {id}"
                );
            }
            for global in spec.globals {
                assert!(properties.contains_key(*global), "{name} has no {global}");
            }
            assert!(
                properties.contains_key("cwd") && properties.contains_key("lane"),
                "{name}"
            );
        }
    }

    /// A flag added to a command, with a type, possible values and a
    /// default, shows up in the tool and back on the command line.
    #[test]
    fn new_flags_need_no_mcp_code() {
        static SPEC: Spec = Spec {
            path: &["probe"],
            ..Spec::NONE
        };
        let mut root = clap::Command::new("devlanes")
            .arg(clap::Arg::new("lane").long("lane").global(true))
            .subcommand(
                clap::Command::new("probe")
                    .about("Probe a unit")
                    .arg(clap::Arg::new("probe_unit").value_name("UNIT"))
                    .arg(
                        clap::Arg::new("mode")
                            .long("mode")
                            .value_parser(["fast", "slow"])
                            .default_value("fast")
                            .help("How to probe"),
                    )
                    .arg(
                        clap::Arg::new("tries")
                            .long("tries")
                            .value_parser(clap::value_parser!(u32)),
                    )
                    .arg(
                        clap::Arg::new("dry-run")
                            .long("dry-run")
                            .action(ArgAction::SetTrue),
                    ),
            );
        root.build();
        let generated = generate(&root, &SPEC, |_| true).expect("tool");
        let properties = &generated.tool.input_schema["properties"];
        assert_eq!(
            properties["mode"],
            json!({"type": "string", "enum": ["fast", "slow"], "default": "fast", "description": "How to probe"})
        );
        assert_eq!(properties["tries"], json!({"type": "integer"}));
        assert_eq!(properties["dry_run"], json!({"type": "boolean"}));
        assert_eq!(properties["unit"], json!({"type": "string"}));
        assert_eq!(generated.tool.description.as_deref(), Some("Probe a unit."));
        let line = generated
            .line(&object(
                json!({"unit": "api", "mode": "slow", "tries": 3, "dry_run": true, "lane": "2"}),
            ))
            .expect("line");
        assert_eq!(
            line.args,
            [
                "probe",
                "api",
                "--mode=slow",
                "--tries=3",
                "--dry-run",
                "--lane=2"
            ]
        );
    }

    /// Each curated tool turns its arguments into the command line the CLI
    /// parses: arrays, enums, booleans, integers, positional units, the
    /// global options and the command after `--`.
    #[test]
    fn arguments_become_command_lines() {
        let catalog = Catalog::new(false);
        let cases = [
            ("status", json!({}), "status"),
            (
                "status",
                json!({"lane": "acme@2", "mine": true}),
                "status acme@2 --mine",
            ),
            (
                "start",
                json!({"units": ["api", "web"], "profile": ["e2e", "ci"], "all_at_once": true, "timeout": "5m", "env": ["A=1"], "lane": "2"}),
                "start api web --profile=e2e --profile=ci --all-at-once --lane=2 --timeout=5m --env=A=1",
            ),
            ("start", json!({"units": "api"}), "start api"),
            (
                "exec",
                json!({"name": "web", "command": ["pnpm", "dev", "--port", "3000"], "no_pty": true}),
                "exec --name=web --no-pty -- pnpm dev --port 3000",
            ),
            (
                "logs",
                json!({"units": ["api"], "tail": 100, "grep": "-v|error", "context": 5, "follow": false}),
                "logs api --tail=100 --grep=-v|error --context=5",
            ),
            (
                "logs",
                json!({"all": true, "follow": true, "timeout": "30s"}),
                "logs --all --follow --timeout=30s",
            ),
            ("stop", json!({"mine": true}), "stop --mine"),
            (
                "stop",
                json!({"units": ["acme@1:api"], "logs_dir": "out"}),
                "stop acme@1:api --logs-dir=out",
            ),
            ("restart", json!({"units": ["api"]}), "restart api"),
            ("reload", json!({}), "reload"),
            (
                "wait",
                json!({"target": "api", "timeout": "10s"}),
                "wait api --timeout=10s",
            ),
            (
                "with",
                json!({"command": ["pnpm", "test"], "keep": true, "profile": ["e2e"]}),
                "with --profile=e2e --keep -- pnpm test",
            ),
            (
                "run",
                json!({"task": "deploy", "detach": true, "yes": true}),
                "run deploy --detach --yes",
            ),
            ("run", json!({"list": true}), "run --list"),
            ("debug_why", json!({"unit": "api"}), "debug why api"),
            (
                "debug_errors",
                json!({"since": "5m"}),
                "debug errors --since=5m",
            ),
            (
                "debug_top",
                json!({"lane": "acme@1"}),
                "debug top --lane=acme@1",
            ),
            ("url_print", json!({"unit": "web"}), "url print web"),
            ("url_ls", json!({}), "url ls"),
            (
                "env_show",
                json!({"unit": "api", "explain": true}),
                "env show api --explain",
            ),
            ("lane_ls", json!({}), "lane ls"),
            (
                "lane_clean",
                json!({"lane": "4", "dry_run": true}),
                "lane clean 4 --dry-run",
            ),
            (
                "lane_clean",
                json!({"lane": "4", "yes": true}),
                "lane clean 4 --yes",
            ),
            ("daemon_pair", json!({}), "daemon pair"),
            ("doctor", json!({}), "doctor"),
        ];
        for (tool, arguments, expected) in &cases {
            let args = line(&catalog, tool, arguments.clone());
            assert_eq!(args, *expected, "{tool} {arguments}");
            let args: Vec<String> = args.split(' ').map(str::to_owned).collect();
            if let Err(Rejected::Error(text) | Rejected::Help(text)) =
                Call::parse(&args, Scope::Any)
            {
                panic!("{tool} {arguments}: {text}");
            }
        }
        let tested: Vec<&str> = cases.iter().map(|(tool, ..)| *tool).collect();
        for generated in catalog.iter() {
            assert!(
                tested.contains(&generated.name.as_str()),
                "{}",
                generated.name
            );
        }
    }

    /// Wrong arguments are refused with what the tool takes, before anything
    /// runs.
    #[test]
    fn wrong_arguments_are_refused() {
        let catalog = Catalog::new(false);
        let refused = |tool: &str, arguments: Value| {
            catalog
                .get(tool)
                .expect("tool")
                .line(&object(arguments))
                .expect_err("refused")
        };
        assert!(refused("logs", json!({"unit": "api"})).contains("it takes units"));
        assert!(refused("exec", json!({"command": ["x"]})).contains("exec needs name"));
        assert!(refused("exec", json!({"name": "x"})).contains("exec needs command"));
        assert!(refused("logs", json!({"tail": "x", "mine": "yes"})).contains("true or false"));
        assert!(refused("logs", json!({"tail": true})).contains("an integer"));
        assert!(refused("stop", json!({"units": ["--all"]})).contains("read as a flag"));
        assert!(refused("status", json!({"cwd": 1})).contains("cwd"));
        let line = catalog
            .get("status")
            .expect("status")
            .line(&object(json!({"cwd": "sub", "lane": null})))
            .expect("line");
        assert_eq!(
            line,
            Line {
                args: vec!["status".into()],
                cwd: Some("sub".into())
            }
        );
    }

    /// Read-only tools come from `Command::is_read_only`, and `--read-only`
    /// keeps only those.
    #[test]
    fn annotations_follow_the_classification() {
        let catalog = Catalog::new(false);
        let read_only: Vec<&str> = catalog
            .iter()
            .filter(|tool| tool.read_only)
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(
            read_only,
            [
                "status",
                "logs",
                "wait",
                "debug_why",
                "debug_errors",
                "debug_top",
                "url_print",
                "url_ls",
                "env_show",
                "lane_ls",
                "doctor"
            ]
        );
        for tool in catalog.iter() {
            let annotations = tool.tool.annotations.as_ref().expect("annotations");
            assert_eq!(annotations.read_only_hint, Some(tool.read_only));
            assert_eq!(
                annotations.destructive_hint,
                (!tool.read_only).then_some(true),
                "{}",
                tool.name
            );
        }
        let names: Vec<String> = Catalog::new(true)
            .iter()
            .map(|tool| tool.name.clone())
            .collect();
        assert_eq!(names, read_only);
    }

    /// The command line of every `TOOL {json}` call between backticks in
    /// `text`. Each must name a tool of `catalog` and convert.
    fn quoted_calls(catalog: &Catalog, text: &str) -> Vec<Vec<String>> {
        text.split('`')
            .skip(1)
            .step_by(2)
            .filter_map(|quoted| quoted.split_once(" {"))
            .map(|(tool, arguments)| {
                let generated = catalog
                    .get(tool)
                    .unwrap_or_else(|| panic!("`{tool}` is not a tool here"));
                let arguments: Value = serde_json::from_str(&format!("{{{arguments}"))
                    .unwrap_or_else(|error| panic!("`{tool}`: {error}"));
                match generated.line(&object(arguments)) {
                    Ok(line) => line.args,
                    Err(error) => panic!("{tool}: {error}"),
                }
            })
            .collect()
    }

    /// Every `["...", ...]` args array in `text`.
    fn quoted_args(text: &str) -> Vec<Vec<String>> {
        text.split('[')
            .skip(1)
            .filter_map(|rest| rest.split_once(']'))
            .filter_map(|(array, _)| serde_json::from_str(&format!("[{array}]")).ok())
            .collect()
    }

    /// The calls agents are told to make become valid devlanes command lines,
    /// the read-only instructions name only tools a `--read-only` server has,
    /// and the args arrays of the `devlanes` tool's descriptions parse too.
    #[test]
    fn quoted_calls_parse() {
        let catalog = Catalog::new(false);
        let mut lines = quoted_calls(&catalog, text::INSTRUCTIONS);
        lines.extend(quoted_calls(
            &Catalog::new(true),
            text::READ_ONLY_INSTRUCTIONS,
        ));
        for tool in catalog.iter() {
            lines.extend(quoted_calls(
                &catalog,
                tool.tool.description.as_deref().unwrap_or_default(),
            ));
        }
        assert!(lines.len() >= 7, "{lines:?}");
        let before = lines.len();
        for text in [text::DEVLANES, text::DEVLANES_READ_ONLY] {
            lines.extend(quoted_args(text));
        }
        assert!(lines.len() >= before + 4, "{lines:?}");
        for args in lines {
            if let Err(Rejected::Error(text) | Rejected::Help(text)) =
                Call::parse(&args, Scope::Any)
            {
                panic!("devlanes {}: {text}", args.join(" "));
            }
        }
    }
}
