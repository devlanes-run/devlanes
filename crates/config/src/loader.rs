use crate::{
    CheckReport, ComposeFiles, ConfigError, Env, Extensions, InfraCommand, InfraCommandOptions,
    InfraConfig, LanesConfig, OpaqueExtensions, ProfileConfig, ProfileOptions, ProjectConfig,
    ProjectExtends, TaskConfig, service::PartialService,
};
use indexmap::IndexMap;
use serde::{Deserialize, de::DeserializeOwned};
use serde_saphyr::Spanned;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

/// Hint for `on:`, which users borrow from CI syntax for task hooks and service lifecycles.
pub(crate) const ON_HINT: &str = "unknown key `on`; lifecycle hooks are tasks with `when`";

/// Original YAML documents kept in memory so later integration checks can point at a line.
/// They are in merge order: extended files, then the entry file, each followed by its local
/// overlay.
#[derive(Clone, Default)]
pub struct ConfigSources {
    sources: Vec<Source>,
    /// Index of the file the project was loaded from; errors no file explains point at it.
    entry: usize,
}

/// One YAML document of a project and the name diagnostics give it.
#[derive(Clone)]
struct Source {
    file: String,
    text: String,
    local: bool,
}

impl std::fmt::Debug for ConfigSources {
    /// Hide source contents, which may contain credentials, from debug output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConfigSources { .. }")
    }
}

impl ConfigSources {
    /// Attach an error whose message starts with a field path to that field's YAML position.
    pub fn locate(&self, error: ConfigError) -> ConfigError {
        if self.sources.is_empty() {
            return error;
        }
        locate_in(error, &self.sources, self.entry)
    }

    /// Source position of the deepest YAML node on a dotted field path such as
    /// `services.api.hosts.0`, preferring the later file when two set that field.
    /// `None` when the config was not parsed from YAML or no part of the path matches.
    pub fn position(&self, path: &str) -> Option<crate::SourceLocation> {
        let (index, offset) = deepest(&self.sources, path)?;
        let source = &self.sources[index];
        Some(ConfigError::at(&source.file, &source.text, offset, "").location)
    }

    /// The files merged into the config, in merge order, as diagnostics name them.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.sources.iter().map(|source| source.file.as_str())
    }
}

/// Parsed before defaults, so an omitted local field never erases a base field.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, bound = "E: Extensions")]
struct PartialProject<E: Extensions> {
    project: Option<Spanned<String>>,
    extends: Option<Spanned<ProjectExtends>>,
    #[serde(default)]
    tags: IndexMap<String, crate::TagConfig>,
    lanes: Option<Spanned<LanesConfig>>,
    infra: Option<Spanned<InfraConfig>>,
    global_env: Option<Spanned<Env>>,
    #[serde(default)]
    templates: IndexMap<String, PartialService<E>>,
    #[serde(default)]
    services: IndexMap<String, PartialService<E>>,
    #[serde(default)]
    tasks: IndexMap<String, TaskConfig>,
    #[serde(default)]
    profiles: IndexMap<String, ProfileConfig>,
}

/// Apply scalar/list replacement without introducing absent defaults.
macro_rules! replace {
    ($base:ident, $overlay:ident, $($field:ident),* $(,)?) => {
        $(if $overlay.$field.is_some() { $base.$field = $overlay.$field; })*
    };
}

/// Merge map entries in place to preserve the original declaration order.
fn merge_map<T>(
    base: &mut IndexMap<String, T>,
    overlay: IndexMap<String, T>,
    merge: impl Fn(&mut T, T),
) {
    for (key, value) in overlay {
        if let Some(base) = base.get_mut(&key) {
            merge(base, value);
        } else {
            base.insert(key, value);
        }
    }
}

/// Combine typed source-bearing root fields without a YAML-tree round trip.
fn merge_spanned<T>(
    base: &mut Option<Spanned<T>>,
    overlay: Option<Spanned<T>>,
    merge: impl FnOnce(&mut T, T),
) {
    if let Some(overlay) = overlay {
        if let Some(base) = base {
            merge(&mut base.value, overlay.value);
        } else {
            *base = Some(overlay);
        }
    }
}

/// Merge nested lane maps; matrix lists replace as a whole.
fn merge_lanes(base: &mut LanesConfig, overlay: LanesConfig) {
    replace!(
        base,
        overlay,
        primary,
        new_worktrees,
        removed_worktrees,
        block_size,
        max,
        hosts_file,
        matrix
    );
    base.vars.extend(overlay.vars);
}

/// Merge a local task per key; `run`, `run_script` and `steps` are alternatives, so setting
/// one clears the others.
fn merge_task(base: &mut TaskConfig, mut overlay: TaskConfig) {
    if overlay.run.is_some() || overlay.steps.is_some() {
        base.run_script = None;
    }
    if overlay.run_script.is_some() || overlay.steps.is_some() {
        base.run = None;
    }
    if overlay.run.is_some() || overlay.run_script.is_some() {
        base.steps = None;
    }
    merge_option(
        &mut base.run_script,
        overlay.run_script.take(),
        merge_script,
    );
    replace!(
        base,
        overlay,
        run,
        cwd,
        when,
        after,
        profiles,
        steps,
        confirm,
        interactive,
        timeout,
        pty
    );
    base.env.extend(overlay.env);
}

/// Merge optional typed objects rather than replacing an entire object.
pub(crate) fn merge_option<T>(
    base: &mut Option<T>,
    overlay: Option<T>,
    merge: impl FnOnce(&mut T, T),
) {
    if let Some(overlay) = overlay {
        if let Some(base) = base {
            merge(base, overlay);
        } else {
            *base = Some(overlay);
        }
    }
}

/// Read command shorthand as the mapping `{ run: ... }` it abbreviates.
fn command_options(command: InfraCommand) -> InfraCommandOptions {
    match command {
        InfraCommand::Object(options) => *options,
        InfraCommand::Command(run) => InfraCommandOptions {
            run: Some(run),
            ..Default::default()
        },
    }
}

/// Merge named infra commands per key; shorthand counts as `run`.
fn merge_command(base: &mut InfraCommand, overlay: InfraCommand) {
    if matches!(
        (&*base, &overlay),
        (InfraCommand::Command(_), InfraCommand::Command(_))
    ) {
        *base = overlay;
        return;
    }
    let mut merged = command_options(std::mem::replace(
        base,
        InfraCommand::Object(Box::default()),
    ));
    let overlay = command_options(overlay);
    replace!(merged, overlay, run, container, ignore_errors, cwd, timeout);
    merged.env.extend(overlay.env);
    *base = InfraCommand::Object(Box::new(merged));
}

/// Read a selector list as the `{ include: [...] }` mapping it abbreviates.
fn profile_options(profile: ProfileConfig) -> ProfileOptions {
    match profile {
        ProfileConfig::Object(options) => options,
        ProfileConfig::Selectors(include) => ProfileOptions {
            include: Some(include),
            ..Default::default()
        },
    }
}

/// Merge profiles per key; a selector list replaces only `include`.
fn merge_profile(base: &mut ProfileConfig, overlay: ProfileConfig) {
    if matches!(
        (&*base, &overlay),
        (ProfileConfig::Selectors(_), ProfileConfig::Selectors(_))
    ) {
        *base = overlay;
        return;
    }
    let mut merged = profile_options(std::mem::replace(
        base,
        ProfileConfig::Selectors(Vec::new()),
    ));
    let overlay = profile_options(overlay);
    replace!(merged, overlay, include, exclude, infra);
    merged.env.extend(overlay.env);
    merged.run_scripts.extend(overlay.run_scripts);
    *base = ProfileConfig::Object(merged);
}

/// Merge infra maps including individual container and external declarations.
fn merge_infra(base: &mut InfraConfig, overlay: InfraConfig) {
    replace!(
        base,
        overlay,
        compose,
        project_name,
        exclude,
        per_lane,
        profiles,
        hidden
    );
    base.env.extend(overlay.env);
    base.hosts.extend(overlay.hosts);
    merge_map(&mut base.setup, overlay.setup, merge_command);
    merge_map(&mut base.teardown, overlay.teardown, merge_command);
    merge_map(&mut base.containers, overlay.containers, |base, overlay| {
        replace!(base, overlay, image, command, volumes, per_lane);
        merge_option(&mut base.ready, overlay.ready, merge_ready);
        base.ports.extend(overlay.ports);
        base.env.extend(overlay.env);
    });
    merge_map(&mut base.external, overlay.external, |base, overlay| {
        replace!(base, overlay, address, hosts, hidden);
    });
}

/// Merge readiness checks per key. `started` and the check mapping are alternatives, and so
/// are `tcp`, `http`, `log` and `cmd`: setting one clears the others.
pub(crate) fn merge_ready(base: &mut crate::ReadyConfig, overlay: crate::ReadyConfig) {
    match (base, overlay) {
        (crate::ReadyConfig::Check(base), crate::ReadyConfig::Check(overlay)) => {
            let probes = [
                overlay.tcp.is_some(),
                overlay.http.is_some(),
                overlay.log.is_some(),
                overlay.cmd.is_some(),
            ];
            if probes.contains(&true) {
                if !probes[0] {
                    base.tcp = None;
                }
                if !probes[1] {
                    base.http = None;
                }
                if !probes[2] {
                    base.log = None;
                }
                if !probes[3] {
                    base.cmd = None;
                }
            }
            replace!(base, overlay, tcp, log, cmd, timeout);
            merge_option(&mut base.http, overlay.http, |base, overlay| {
                replace!(base, overlay, path, status, host, timeout);
            });
        }
        (base, overlay) => *base = overlay,
    }
}

/// Merge `{ file, name }` per key; a key the overlay leaves out keeps the base value.
pub(crate) fn merge_script(base: &mut crate::RunScript, overlay: crate::RunScript) {
    if !overlay.file.is_empty() {
        base.file = overlay.file;
    }
    if !overlay.name.is_empty() {
        base.name = overlay.name;
    }
}

/// Merge opaque preset mappings per key, recursively; lists and scalars replace.
pub(crate) fn merge_json(base: &mut serde_json::Value, overlay: serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base), serde_json::Value::Object(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(base) => merge_json(base, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

impl<E: Extensions> PartialProject<E> {
    /// Apply local settings before any template is expanded. Mappings merge per key at every
    /// level; lists and scalars replace.
    fn merge(&mut self, overlay: Self) {
        // Each file names its own bases; only the entry file's list describes the result.
        replace!(self, overlay, project, extends);
        merge_spanned(&mut self.lanes, overlay.lanes, merge_lanes);
        merge_spanned(&mut self.infra, overlay.infra, merge_infra);
        merge_spanned(&mut self.global_env, overlay.global_env, |base, overlay| {
            base.extend(overlay)
        });
        merge_map(
            &mut self.templates,
            overlay.templates,
            PartialService::merge,
        );
        merge_map(&mut self.services, overlay.services, PartialService::merge);
        merge_map(&mut self.tasks, overlay.tasks, merge_task);
        merge_map(&mut self.tags, overlay.tags, |base, overlay| {
            replace!(base, overlay, color, description);
        });
        merge_map(&mut self.profiles, overlay.profiles, merge_profile);
    }

    /// Expand all templates, including unused ones so hidden cycles cannot survive checks.
    fn finish(self) -> Result<ProjectConfig<E>, ConfigError> {
        let mut templates = IndexMap::new();
        for (name, value) in &self.templates {
            templates.insert(
                name.clone(),
                resolve(value, &self.templates, &mut vec![name.clone()])
                    .map_err(|e| {
                        ConfigError::plain(format!("templates.{name}.extends: {}", e.message))
                    })?
                    .finish(&format!("templates.{name}"))?,
            );
        }
        let mut services = IndexMap::new();
        for (name, value) in self.services {
            services.insert(
                name.clone(),
                resolve(&value, &self.templates, &mut Vec::new())
                    .map_err(|e| {
                        ConfigError::plain(format!("services.{name}.extends: {}", e.message))
                    })?
                    .finish(&format!("services.{name}"))?,
            );
        }
        Ok(ProjectConfig {
            sources: ConfigSources::default(),
            project: self
                .project
                .ok_or_else(|| ConfigError::plain("project: missing required project name"))?
                .value,
            extends: self.extends.map(|v| v.value),
            lanes: self.lanes.map(|v| v.value).unwrap_or_default(),
            infra: self.infra.map(|v| v.value).unwrap_or_default(),
            global_env: self.global_env.map(|v| v.value).unwrap_or_default(),
            templates,
            services,
            tasks: self.tasks,
            profiles: self.profiles,
            tags: self.tags,
        })
    }
}

/// Follow typed template references with an explicit recursion stack.
fn resolve<E: Extensions>(
    service: &PartialService<E>,
    templates: &IndexMap<String, PartialService<E>>,
    stack: &mut Vec<String>,
) -> Result<PartialService<E>, ConfigError> {
    let Some(parent) = &service.extends else {
        return Ok(service.clone());
    };
    if stack.contains(&parent.value) {
        stack.push(parent.value.clone());
        return Err(ConfigError::plain(format!(
            "template cycle: {}",
            stack.join(" -> ")
        )));
    }
    let template = templates.get(&parent.value).ok_or_else(|| {
        ConfigError::plain(format!(
            "unknown template `{}`{}",
            parent.value,
            did_you_mean(&parent.value, templates.keys().map(String::as_str))
        ))
    })?;
    stack.push(parent.value.clone());
    let mut resolved = resolve(template, templates, stack)?;
    stack.pop();
    resolved.merge(service.clone());
    Ok(resolved)
}

/// Decode with serde-saphyr, preserving its precise syntax/type error position.
pub(crate) fn parse_yaml<T: DeserializeOwned>(file: &str, source: &str) -> Result<T, ConfigError> {
    serde_saphyr::from_str(source).map_err(|error| {
        let offset = error.location().map_or(0, |l| {
            line_offset(source, l.line() as usize, l.column() as usize)
        });
        // Miette owns source rendering and the location, so drop serde-saphyr's own copies.
        let mut message = strip_location(&error.without_snippet().to_string()).to_owned();
        // serde-saphyr names a scalar that does not parse by its Rust type alone.
        if let Some(ty) = message
            .strip_prefix("invalid ")
            .filter(|ty| matches!(*ty, "u16" | "u32" | "u64" | "i64" | "f64"))
        {
            message = format!("invalid value, expected {ty}");
        }
        message = crate::scalar::plain_type_names(&message);
        // Templated settings take an expression before decoding, so one failing here
        // sits in a setting that takes only a plain value, such as `replicas`.
        let value = source.get(offset..).unwrap_or_default();
        if value.trim_start_matches(['\'', '"']).starts_with("${{") {
            message.push_str("; this setting takes a plain value, not a `${{ ... }}` expression");
        }
        if let Some((_, unknown)) = message.split_once("unknown field `")
            && let Some((key, expected)) = unknown.split_once('`')
            && let Some(hint) = unknown_key_hint(key, expected)
        {
            message = hint;
        }
        ConfigError::at(file, source, offset, message)
    })
}

/// Remove a trailing ` at line N, column M`, which the diagnostic already shows.
fn strip_location(message: &str) -> &str {
    message
        .rfind(" at line ")
        .filter(|&start| {
            let tail = &message[start + " at line ".len()..];
            tail.split_once(", column ").is_some_and(|(line, column)| {
                !line.is_empty()
                    && !column.is_empty()
                    && line.bytes().all(|b| b.is_ascii_digit())
                    && column.bytes().all(|b| b.is_ascii_digit())
            })
        })
        .map_or(message, |start| &message[..start])
}

/// The candidate closest to `key`, when one is close enough to be a typo.
pub(crate) fn closest<'a>(
    key: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    candidates
        .into_iter()
        .filter(|candidate| *candidate != key)
        .max_by(|a, b| strsim::jaro_winkler(key, a).total_cmp(&strsim::jaro_winkler(key, b)))
        .filter(|candidate| strsim::jaro_winkler(key, candidate) > 0.75)
}

/// `; did you mean `x`?` for the candidate closest to `key`, or nothing when none is close.
pub(crate) fn did_you_mean<'a>(key: &str, candidates: impl IntoIterator<Item = &'a str>) -> String {
    closest(key, candidates)
        .map(|candidate| format!("; did you mean `{candidate}`?"))
        .unwrap_or_default()
}

/// Name the closest expected key for an unknown-field error. serde-saphyr lists the expected
/// names after "expected" or "expected one of", with or without backticks.
fn unknown_key_hint(key: &str, expected: &str) -> Option<String> {
    if key == "on" {
        return Some(ON_HINT.into());
    }
    let expected = expected.split(" at line").next().unwrap_or(expected);
    let expected = expected
        .trim_start_matches(',')
        .trim_start()
        .trim_start_matches("expected ")
        .trim_start_matches("one of ");
    let names = expected
        .split(", ")
        .map(|name| name.trim().trim_matches('`'))
        .filter(|name| !name.is_empty() && !name.contains(' '));
    // 0.2 named the `run_script` file `source`, too far from `file` for the typo match.
    if key == "source" && names.clone().eq(["file", "name"]) {
        return Some("unknown key `source`; `source` is now `file`".into());
    }
    names
        .max_by(|a, b| strsim::jaro_winkler(key, a).total_cmp(&strsim::jaro_winkler(key, b)))
        .filter(|name| strsim::jaro_winkler(key, name) > 0.75)
        .map(|name| format!("unknown key `{key}`; did you mean `{name}`?"))
}

/// Translate character-based YAML positions into miette byte spans.
fn line_offset(source: &str, line: usize, column: usize) -> usize {
    let start = source
        .split_inclusive('\n')
        .take(line.saturating_sub(1))
        .map(str::len)
        .sum::<usize>();
    start
        + source
            .get(start..)
            .and_then(|s| s.lines().next())
            .map_or(0, |s| {
                s.char_indices()
                    .nth(column.saturating_sub(1))
                    .map_or(s.len(), |(i, _)| i)
            })
}

/// Track source positions independently from typed merge data.
#[derive(Debug, Default)]
struct SourceNode {
    children: IndexMap<String, Spanned<SourceNode>>,
}

impl<'de> Deserialize<'de> for SourceNode {
    /// Capture every mapping value's parser span without interpreting its content.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct NodeVisitor;
        impl<'de> serde::de::Visitor<'de> for NodeVisitor {
            type Value = SourceNode;
            /// Describe all legal YAML values to serde.
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a YAML value")
            }
            /// Capture map descendants for validation diagnostics.
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                let mut children = IndexMap::new();
                while let Some((key, value)) = map.next_entry()? {
                    children.insert(key, value);
                }
                Ok(SourceNode { children })
            }
            /// Preserve list indices in diagnostic paths.
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut children = IndexMap::new();
                while let Some(value) = seq.next_element()? {
                    children.insert(children.len().to_string(), value);
                }
                Ok(SourceNode { children })
            }
            /// Scalars carry their spans in the outer wrapper.
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Self::Value, E> {
                Ok(SourceNode::default())
            }
            /// Ignore scalar contents when capturing metadata.
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
                Ok(SourceNode::default())
            }
            /// Ignore scalar contents when capturing metadata.
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
                Ok(SourceNode::default())
            }
            /// Ignore scalar contents when capturing metadata.
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
                Ok(SourceNode::default())
            }
            /// Ignore scalar contents when capturing metadata.
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(SourceNode::default())
            }
            /// Null values need no descendant positions.
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(SourceNode::default())
            }
        }
        d.deserialize_any(NodeVisitor)
    }
}

/// Find the deepest source node matching a validation path.
fn source_position(source: &str, path: &str) -> Option<(usize, usize)> {
    let node: SourceNode = serde_saphyr::from_str(source).ok()?;
    let mut children = &node.children;
    let mut found = None;
    let mut remaining = path;
    let mut depth = 0;
    while !remaining.is_empty() {
        // Keys may contain dots, so match the longest key that ends at a path separator.
        let Some((key, child)) = children
            .iter()
            .filter(|(key, _)| {
                remaining
                    .strip_prefix(key.as_str())
                    .is_some_and(|tail| tail.is_empty() || tail.starts_with('.'))
            })
            .max_by_key(|(key, _)| key.len())
        else {
            break;
        };
        depth += key.len();
        found = Some((
            depth,
            line_offset(
                source,
                child.referenced.line() as usize,
                child.referenced.column() as usize,
            ),
        ));
        remaining = remaining[key.len()..].trim_start_matches('.');
        children = &child.value.children;
    }
    found
}

/// Attribute semantic failures to the most specific source: the deepest match of the
/// message's field path, the later file when two match equally deep, else the entry file.
fn locate_in(error: ConfigError, sources: &[Source], entry: usize) -> ConfigError {
    let path = error
        .message
        .split_once(':')
        .map_or(error.message.as_str(), |(path, _)| path);
    let (index, offset) = deepest(sources, path).unwrap_or((entry, 0));
    let source = &sources[index];
    ConfigError::at(&source.file, &source.text, offset, error.message)
}

/// The source whose YAML reaches deepest along `path`, with the byte offset of that node.
/// A later source wins a tie because it overrides the earlier one.
fn deepest(sources: &[Source], path: &str) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize, usize)> = None;
    for (index, source) in sources.iter().enumerate() {
        if let Some((depth, offset)) = source_position(&source.text, path)
            && best.is_none_or(|(known, _, _)| depth >= known)
        {
            best = Some((depth, index, offset));
        }
    }
    best.map(|(_, index, offset)| (index, offset))
}

/// [`locate_in`] for a single document, such as the user config.
pub(crate) fn locate(error: ConfigError, file: &str, source: &str) -> ConfigError {
    let source = Source {
        file: file.into(),
        text: source.into(),
        local: false,
    };
    locate_in(error, std::slice::from_ref(&source), 0)
}

/// Parse and validate an in-memory project plus an optional local overlay.
pub fn parse_project(source: &str, local: Option<&str>) -> Result<ProjectConfig, ConfigError> {
    parse_project_with::<OpaqueExtensions>(source, local)
}

/// Parse a project with the binary's registered integration schemas. A file-level
/// `extends` needs the files on disk, so it fails here; use [`load_project_with`].
pub fn parse_project_with<E: Extensions>(
    source: &str,
    local: Option<&str>,
) -> Result<ProjectConfig<E>, ConfigError> {
    let entry = parse_document::<E>("devlanes.yaml", source, false)?;
    if entry.partial.extends.is_some() {
        return Err(locate(
            ConfigError::plain("extends: needs the project files on disk; load the file instead"),
            &entry.source.file,
            &entry.source.text,
        ));
    }
    let mut documents = vec![entry];
    if let Some(local) = local {
        documents.push(parse_document("devlanes.local.yaml", local, true)?);
    }
    merge_documents(documents, 0).map(|(config, _)| config)
}

/// A parsed document waiting to be merged, with the source it came from.
struct Document<E: Extensions> {
    source: Source,
    partial: PartialProject<E>,
}

/// Parse one document, keeping its text for later diagnostics.
fn parse_document<E: Extensions>(
    file: &str,
    text: &str,
    local: bool,
) -> Result<Document<E>, ConfigError> {
    Ok(Document {
        partial: parse_yaml(file, text)?,
        source: Source {
            file: file.into(),
            text: text.into(),
            local,
        },
    })
}

/// The last component of a diagnostic file name, for messages that name another file.
fn short_name(file: &str) -> &str {
    file.rsplit(['/', '\\']).next().unwrap_or(file)
}

/// Merge documents in order into one validated project. `entry` is the document the
/// project was loaded from: it names the project, local overlays must not, and the
/// names in extended files are ignored.
fn merge_documents<E: Extensions>(
    documents: Vec<Document<E>>,
    entry: usize,
) -> Result<(ProjectConfig<E>, CheckReport), ConfigError> {
    let sources: Vec<Source> = documents.iter().map(|doc| doc.source.clone()).collect();
    let entry_source = &sources[entry];
    if documents[entry].partial.project.is_none() {
        // A project name set only in a local file would differ between checkouts.
        return Err(ConfigError::at(
            &entry_source.file,
            &entry_source.text,
            0,
            format!("project: {} must set the project name", entry_source.file),
        ));
    }
    let mut base_services: Vec<String> = Vec::new();
    let mut local_services: Vec<String> = Vec::new();
    let mut merged: Option<PartialProject<E>> = None;
    let mut overlaid = entry;
    for (index, document) in documents.into_iter().enumerate() {
        let mut partial = document.partial;
        let own = std::slice::from_ref(&sources[index]);
        if document.source.local {
            // One machine renaming the project would give its lanes other domains,
            // container names and `projects.<name>` references than everyone else's.
            if partial.project.is_some() {
                return Err(locate_in(
                    ConfigError::plain(format!(
                        "project: set the project name in {}, not in {}",
                        short_name(&entry_source.file),
                        short_name(&document.source.file),
                    )),
                    own,
                    0,
                ));
            }
            if partial.extends.is_some() {
                return Err(locate_in(
                    ConfigError::plain(format!(
                        "extends: set it in {}, not in {}",
                        short_name(&sources[overlaid].file),
                        short_name(&document.source.file),
                    )),
                    own,
                    0,
                ));
            }
            local_services.extend(partial.services.keys().cloned());
        } else {
            if index != entry {
                partial.project = None;
            }
            base_services.extend(partial.services.keys().cloned());
            overlaid = index;
        }
        match &mut merged {
            Some(merged) => merged.merge(partial),
            None => merged = Some(partial),
        }
    }
    let Some(merged) = merged else {
        return Err(ConfigError::plain("project: no project file to load"));
    };
    let mut config = merged
        .finish()
        .map_err(|error| locate_in(error, &sources, entry))?;
    config.sources = ConfigSources { sources, entry };
    for name in local_services
        .iter()
        .filter(|name| !base_services.contains(name))
    {
        let service = &config.services[name];
        if service.run.is_none() && service.run_script.is_none() {
            let suggestion = base_services
                .iter()
                .max_by(|a, b| {
                    strsim::jaro_winkler(name, a).total_cmp(&strsim::jaro_winkler(name, b))
                })
                .filter(|candidate| strsim::jaro_winkler(name, candidate) > 0.75)
                .map(|candidate| format!("; did you mean `{candidate}`?"))
                .unwrap_or_default();
            return Err(config.sources.locate(ConfigError::plain(format!(
                "services.{name}: service is defined only in the local file and needs run or run_script{suggestion}"
            ))));
        }
    }
    let report = crate::validate(&config).map_err(|error| config.sources.locate(error))?;
    Ok((config, report))
}

/// What loading a project from disk read: the result, and every file it read or
/// looked for.
pub struct ProjectLoad<E: Extensions = OpaqueExtensions> {
    /// The merged, validated config with its warnings, or the first error.
    pub result: Result<(ProjectConfig<E>, CheckReport), ConfigError>,
    /// Every file read or looked for, in merge order. Local overlays that do not exist
    /// are listed too, so a caller watching these files notices one being created.
    pub files: Vec<PathBuf>,
}

/// Where a lane's missing local overlays come from: a local file under `lane_dir` that
/// does not exist is read from the same place under `primary_dir`, the primary
/// worktree's config directory, as lanes share one developer's overrides.
#[derive(Debug, Clone, Copy)]
pub struct LocalFallback<'a> {
    pub lane_dir: &'a Path,
    pub primary_dir: &'a Path,
}

/// The local overlay of a project file: `devlanes.yaml` has `devlanes.local.yaml`,
/// `devlanes.prod.yaml` has `devlanes.prod.local.yaml`.
pub fn local_overlay_path(file: &Path) -> PathBuf {
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let local = match name.rsplit_once('.') {
        Some((stem, extension)) => format!("{stem}.local.{extension}"),
        None => format!("{name}.local.yaml"),
    };
    file.with_file_name(local)
}

/// Whether `file` is a local overlay, which only applies over the project file it
/// belongs to and cannot be loaded as a project itself.
pub fn is_local_overlay(file: &Path) -> bool {
    file.file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|name| name.ends_with(".local.yaml") || name.ends_with(".local.yml"))
}

/// Load a project file, the files its `extends` names (recursively, first) and each
/// one's local overlay, without checking that Compose files exist. Blocking: reads files.
pub fn load_project_files<E: Extensions>(
    entry: &Path,
    fallback: Option<LocalFallback<'_>>,
) -> ProjectLoad<E> {
    let mut files = Vec::new();
    let mut documents = Vec::new();
    let result = if is_local_overlay(entry) {
        Err(ConfigError::at(
            &entry.display().to_string(),
            "",
            0,
            "project: this is a local overlay; load the project file it belongs to",
        ))
    } else {
        collect(
            entry,
            fallback,
            &mut Vec::new(),
            &mut documents,
            &mut files,
            None,
        )
        .and_then(|index| merge_documents(documents, index))
    };
    ProjectLoad { result, files }
}

/// Parse `file`, the files it extends and each one's local overlay, appending them to
/// `documents` in merge order, and return `file`'s index there. `stack` holds the files
/// whose `extends` led here, to refuse cycles; `referrer` is the document and field that
/// named `file`, where a failure to read it is reported.
fn collect<E: Extensions>(
    file: &Path,
    fallback: Option<LocalFallback<'_>>,
    stack: &mut Vec<(PathBuf, String)>,
    documents: &mut Vec<Document<E>>,
    files: &mut Vec<PathBuf>,
    referrer: Option<(&Source, &str)>,
) -> Result<usize, ConfigError> {
    files.push(file.to_owned());
    let name = file.display().to_string();
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) => {
            return Err(match referrer {
                Some((source, field)) => locate_in(
                    ConfigError::plain(format!("{field}: cannot read {name}: {error}")),
                    std::slice::from_ref(source),
                    0,
                ),
                None => ConfigError::io(file, error),
            });
        }
    };
    let document = parse_document::<E>(&name, &text, false)?;
    let key = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_owned());
    if let Some(start) = stack.iter().position(|(known, _)| *known == key) {
        let chain: Vec<&str> = stack[start..]
            .iter()
            .map(|(_, shown)| short_name(shown))
            .chain([short_name(&name)])
            .collect();
        let message = format!("extends: files extend each other: {}", chain.join(" -> "));
        return Err(match referrer {
            Some((source, _)) => {
                locate_in(ConfigError::plain(message), std::slice::from_ref(source), 0)
            }
            None => ConfigError::plain(message),
        });
    }
    if let Some(extends) = &document.partial.extends {
        stack.push((key, name.clone()));
        let dir = file.parent().unwrap_or(Path::new("."));
        let many = matches!(extends.value, ProjectExtends::Files(_));
        for (position, base) in extends.value.files().iter().enumerate() {
            let field = if many {
                format!("extends.{position}")
            } else {
                "extends".to_owned()
            };
            collect(
                &dir.join(base),
                fallback,
                stack,
                documents,
                files,
                Some((&document.source, &field)),
            )?;
        }
        stack.pop();
    }
    let index = documents.len();
    documents.push(document);
    if let Some(local) = read_local(file, fallback, files)? {
        documents.push(local);
    }
    Ok(index)
}

/// The local overlay of `file`, or of the same file in the primary worktree when the lane
/// has none. Recording each path tried in `files`.
fn read_local<E: Extensions>(
    file: &Path,
    fallback: Option<LocalFallback<'_>>,
    files: &mut Vec<PathBuf>,
) -> Result<Option<Document<E>>, ConfigError> {
    let mut candidates = vec![local_overlay_path(file)];
    if let Some(fallback) = fallback
        && let Ok(relative) = candidates[0].strip_prefix(fallback.lane_dir)
    {
        let primary = fallback.primary_dir.join(relative);
        if primary != candidates[0] {
            candidates.push(primary);
        }
    }
    for path in candidates {
        files.push(path.clone());
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                return parse_document(&path.display().to_string(), &text, true).map(Some);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(ConfigError::io(&path, error)),
        }
    }
    Ok(None)
}

/// Load a project file (or a directory containing `devlanes.yaml`), what it extends and
/// their local overlays.
pub fn load_project(path: impl AsRef<Path>) -> Result<ProjectConfig, ConfigError> {
    load_project_with::<OpaqueExtensions>(path)
}

/// Load with custom integration parsing; no external commands are executed.
pub fn load_project_with<E: Extensions>(
    path: impl AsRef<Path>,
) -> Result<ProjectConfig<E>, ConfigError> {
    load_named(path.as_ref(), None).map(|(config, _)| config)
}

/// Like [`load_project_with`] for a lane's worktree: a local overlay the lane does not
/// have is read from `primary_dir` (the primary worktree's directory of the project
/// file) instead, as lanes do.
pub fn load_lane_project_with<E: Extensions>(
    path: impl AsRef<Path>,
    primary_dir: &Path,
) -> Result<ProjectConfig<E>, ConfigError> {
    load_named(path.as_ref(), Some(primary_dir)).map(|(config, _)| config)
}

/// Load a project file and check its Compose files, returning the validation report too.
fn load_named<E: Extensions>(
    path: &Path,
    primary_dir: Option<&Path>,
) -> Result<(ProjectConfig<E>, CheckReport), ConfigError> {
    let file = if path.is_dir() {
        path.join("devlanes.yaml")
    } else {
        path.to_owned()
    };
    let dir = file.parent().unwrap_or(Path::new("."));
    let fallback = primary_dir.map(|primary_dir| LocalFallback {
        lane_dir: dir,
        primary_dir,
    });
    let (config, report) = load_project_files::<E>(&file, fallback).result?;
    check_compose_files(&config, dir)?;
    Ok((config, report))
}

/// Fails, located at the entry, when an `infra.compose` file does not exist
/// relative to `dir`, the directory of `devlanes.yaml`. Blocking: reads the
/// filesystem.
pub fn check_compose_files<E: Extensions>(
    config: &ProjectConfig<E>,
    dir: &Path,
) -> Result<(), ConfigError> {
    let compose: Vec<&String> = match &config.infra.compose {
        Some(ComposeFiles::File(name)) => vec![name],
        Some(ComposeFiles::Files(names)) => names.iter().collect(),
        None => Vec::new(),
    };
    for (index, name) in compose.into_iter().enumerate() {
        let path = dir.join(name);
        if !path.is_file() {
            let field = match &config.infra.compose {
                Some(ComposeFiles::Files(_)) => format!("infra.compose.{index}"),
                _ => "infra.compose".to_owned(),
            };
            return Err(config.sources.locate(ConfigError::plain(format!(
                "{field}: Compose file not found: {}",
                path.display()
            ))));
        }
    }
    Ok(())
}

/// Load and return warnings for `config check`; validation failures are errors.
pub fn check(path: impl AsRef<Path>) -> Result<CheckReport, ConfigError> {
    load_named::<OpaqueExtensions>(path.as_ref(), None).map(|(_, report)| report)
}

/// Determine all transitively referenced templates for unused-template warnings.
pub(crate) fn used_templates<E: Extensions>(config: &ProjectConfig<E>) -> HashSet<String> {
    let mut used = HashSet::new();
    for service in config.services.values() {
        let mut next = service.extends.as_ref();
        while let Some(name) = next {
            if !used.insert(name.clone()) {
                break;
            }
            next = config
                .templates
                .get(name)
                .and_then(|template| template.extends.as_ref());
        }
    }
    used
}
