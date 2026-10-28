use crate::ConfigError;
use indexmap::IndexMap;
use minijinja::{Environment, Expression, UndefinedBehavior, syntax::SyntaxConfig};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap},
    sync::{Arc, LazyLock, PoisonError, RwLock},
};

/// Runtime-owned values. No process environment is read implicitly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionContext {
    pub project: Value,
    pub lane: Value,
    pub lanes: Value,
    #[serde(rename = "self")]
    pub service: Value,
    pub services: Value,
    pub infra: Value,
    pub env: Value,
    pub projects: Value,
    pub replica: Value,
    /// What `domain()` ends the domains it returns in: `localhost`, or the TLD
    /// under the `resolver` strategy. Not a namespace of its own.
    #[serde(skip, default = "default_suffix")]
    pub domain_suffix: String,
}

/// [`crate::DEFAULT_DOMAIN_SUFFIX`] as an owned value, for serde.
fn default_suffix() -> String {
    crate::DEFAULT_DOMAIN_SUFFIX.to_owned()
}

/// The variable that hands [`ExpressionContext::domain_suffix`] to `domain()`.
/// Its name keeps it out of the way of every namespace.
const SUFFIX_VARIABLE: &str = "__devlanes_domain_suffix";

impl Default for ExpressionContext {
    /// Provide empty namespaces so missing leaf values remain strictly undefined.
    fn default() -> Self {
        Self {
            project: serde_json::json!({}),
            lane: serde_json::json!({}),
            lanes: serde_json::json!({}),
            service: serde_json::json!({}),
            services: serde_json::json!({}),
            infra: serde_json::json!({}),
            env: serde_json::json!({}),
            projects: serde_json::json!({}),
            replica: serde_json::json!({}),
            domain_suffix: default_suffix(),
        }
    }
}

impl ExpressionContext {
    /// Converts the context once, with underscore aliases for dashed keys, so a unit's
    /// expressions can all be evaluated against it without converting it again.
    pub fn prepare(&self) -> PreparedContext {
        let namespaces = [
            ("project", &self.project),
            ("lane", &self.lane),
            ("lanes", &self.lanes),
            ("self", &self.service),
            ("services", &self.services),
            ("infra", &self.infra),
            ("env", &self.env),
            ("projects", &self.projects),
            ("replica", &self.replica),
        ];
        let mut json = Value::Object(
            namespaces
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.clone()))
                .collect(),
        );
        if has_dashed_keys(&json) {
            aliases(&mut json);
        }
        if let Some(map) = json.as_object_mut() {
            map.insert(SUFFIX_VARIABLE.into(), self.domain_suffix.clone().into());
        }
        PreparedContext {
            value: template_value(&json),
            json,
        }
    }
}

/// An [`ExpressionContext`] in the form expressions read, built by
/// [`ExpressionContext::prepare`].
#[derive(Debug, Clone)]
pub struct PreparedContext {
    /// The namespaces with dashed-key aliases; `projects.*` placeholders are added to a copy.
    json: Value,
    /// `json` as MiniJinja values.
    value: minijinja::Value,
}

impl PreparedContext {
    /// The value to evaluate an expression with. `projects` lists the expression's
    /// `projects.*` references: each gets empty parent objects, so a project that is not
    /// running reads as undefined at the leaf and `default(...)` applies.
    fn value_for(&self, projects: &[String]) -> minijinja::Value {
        if projects.is_empty() {
            return self.value.clone();
        }
        let mut value = self.json.clone();
        for reference in projects {
            let components: Vec<_> = reference.split('.').collect();
            let mut node = &mut value;
            for component in components.iter().take(components.len().saturating_sub(1)) {
                if let Some(map) = node.as_object_mut() {
                    node = map
                        .entry(*component)
                        .or_insert_with(|| serde_json::json!({}));
                } else {
                    break;
                }
            }
        }
        template_value(&value)
    }
}

/// Values expressions can be evaluated against: an [`ExpressionContext`], converted on
/// each call, or a [`PreparedContext`], reused as is.
pub trait EvaluationContext {
    /// The context in the form expressions read.
    fn prepared(&self) -> Cow<'_, PreparedContext>;
}

impl EvaluationContext for ExpressionContext {
    /// Converts the context; callers evaluating several values should prepare it once.
    fn prepared(&self) -> Cow<'_, PreparedContext> {
        Cow::Owned(self.prepare())
    }
}

impl EvaluationContext for PreparedContext {
    /// The context itself.
    fn prepared(&self) -> Cow<'_, PreparedContext> {
        Cow::Borrowed(self)
    }
}

/// `json` as MiniJinja values; on Windows `env` also answers names in any case.
fn template_value(json: &Value) -> minijinja::Value {
    let value = minijinja::Value::from(minijinja::value::Serde(json));
    #[cfg(windows)]
    let value = env_lookups::fold(value);
    value
}

/// Construct a strict expression-only template environment.
fn environment() -> Result<Environment<'static>, ConfigError> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    let syntax = SyntaxConfig::builder()
        .variable_delimiters("${{", "}}")
        .block_delimiters("\u{1}block", "endblock\u{1}")
        .comment_delimiters("\u{1}comment", "endcomment\u{1}")
        .build()
        .map_err(|e| ConfigError::plain(e.to_string()))?;
    env.set_syntax(syntax);
    env.add_function(DOMAIN_FUNCTION, domain);
    Ok(env)
}

/// The name of [`domain`] in expressions.
const DOMAIN_FUNCTION: &str = "domain";

/// `domain(name)`: the domain of `name` in the current lane, by the rule
/// `lane.domain` follows by default ([`crate::lane_domain`]). It reads
/// `lane.index` and the suffix only, which are set before `lanes.vars`, so it
/// works inside `lanes.vars` too.
fn domain(state: &minijinja::State<'_, '_>, name: &str) -> Result<String, minijinja::Error> {
    let fail = |message: String| {
        minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("domain('{name}'): {message}"),
        )
    };
    let index = state
        .lookup("lane")
        .and_then(|lane| lane.get_attr("index").ok())
        .and_then(|index| u64::try_from(index).ok())
        .ok_or_else(|| fail("only works where a lane is known".into()))?;
    let suffix = state
        .lookup(SUFFIX_VARIABLE)
        .and_then(|suffix| suffix.as_str().map(str::to_owned))
        .unwrap_or_else(default_suffix);
    crate::checked_lane_domain(name, index, &suffix).map_err(fail)
}

/// A MiniJinja error without its `(in <string>:1)` suffix, which names no real file.
fn describe(error: &minijinja::Error) -> String {
    match error.detail() {
        Some(detail) => format!("{}: {detail}", error.kind()),
        None => error.kind().to_string(),
    }
}

/// Extract expression bodies without confusing braces inside quoted strings.
fn parts(text: &str) -> Result<Vec<(usize, usize, &str)>, ConfigError> {
    let mut parts = Vec::new();
    let mut cursor = 0;
    while let Some(start) = text[cursor..].find("${{").map(|i| i + cursor) {
        let mut quote = None;
        let mut escaped = false;
        let mut depth = 0_usize;
        let mut end = None;
        let body = start + 3;
        let mut iter = text[body..].char_indices().peekable();
        while let Some((offset, c)) = iter.next() {
            if escaped {
                escaped = false;
                continue;
            }
            if let Some(q) = quote {
                if c == '\\' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
                continue;
            }
            match c {
                '\'' | '"' => quote = Some(c),
                '(' | '[' | '{' => depth += 1,
                '}' if depth == 0 && iter.peek().is_some_and(|(_, c)| *c == '}') => {
                    end = Some(body + offset);
                    break;
                }
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        let end = end.ok_or_else(|| ConfigError::plain("unclosed `${{` expression"))?;
        parts.push((start, end + 2, &text[body..end]));
        cursor = end + 2;
    }
    Ok(parts)
}

/// Whether any mapping key in the tree contains a dash and so needs an underscore alias.
fn has_dashed_keys(value: &Value) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| key.contains('-') || has_dashed_keys(value)),
        Value::Array(values) => values.iter().any(has_dashed_keys),
        _ => false,
    }
}

/// Add underscore aliases recursively. An alias is added only when exactly one dashed key
/// maps to it and no key already has that name, so `A-B` beside `A_B` keeps both literal.
fn aliases(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for value in map.values_mut() {
                aliases(value);
            }
            let mut extra: IndexMap<String, Option<Value>> = IndexMap::new();
            for (key, value) in map.iter().filter(|(key, _)| key.contains('-')) {
                extra
                    .entry(key.replace('-', "_"))
                    .and_modify(|slot| *slot = None)
                    .or_insert_with(|| Some(value.clone()));
            }
            for (key, value) in extra {
                if let Some(value) = value
                    && !map.contains_key(&key)
                {
                    map.insert(key, value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                aliases(value);
            }
        }
        _ => {}
    }
}

/// Evaluate a scalar, preserving a single expression's native JSON type.
/// Mixed text always produces a string. Missing values fail unless filtered with `default`.
/// Write `${{ '${{' }}` for a literal `${{`.
///
/// Evaluating many values against one [`ExpressionContext`] converts it each time; pass
/// a [`PreparedContext`] from [`ExpressionContext::prepare`] instead.
pub fn evaluate<C: EvaluationContext>(text: &str, context: &C) -> Result<Value, ConfigError> {
    if !text.contains("${{") {
        return Ok(Value::String(text.into()));
    }
    let compiled = compiled(text)?;
    let context = context.prepared().value_for(&compiled.projects);
    match &compiled.form {
        Form::Expression(expression) => {
            let value = expression
                .eval(&context)
                .map_err(|e| ConfigError::plain(format!("expression failed: {}", describe(&e))))?;
            if value.is_undefined() {
                return Err(ConfigError::plain(
                    "expression resolved to an undefined value",
                ));
            }
            serde_json::to_value(value).map_err(|e| ConfigError::plain(e.to_string()))
        }
        Form::Template(env) => env
            .get_template(TEMPLATE_NAME)
            .and_then(|template| template.render(context))
            .map(Value::String)
            .map_err(|e| ConfigError::plain(format!("expression failed: {}", describe(&e)))),
        Form::Broken(message) => Err(ConfigError::plain(message.clone())),
    }
}

/// Name of the one template in a [`Form::Template`] environment; the same name
/// `render_str` uses, so auto-escaping stays off.
const TEMPLATE_NAME: &str = "<string>";

/// Texts kept before the cache starts over. A daemon that reloads configs for weeks would
/// otherwise keep every text it ever evaluated.
const CACHE_LIMIT: usize = 8192;

/// Compiled scalars by their text, shared by every config and lane in the process.
static CACHE: LazyLock<RwLock<HashMap<String, Arc<Compiled>>>> = LazyLock::new(Default::default);

/// The environment cached expressions borrow; it lives as long as they do.
static EXPRESSIONS: LazyLock<Result<Environment<'static>, String>> =
    LazyLock::new(|| environment().map_err(|e| e.message));

/// How many times each text was compiled, so tests can check the cache.
#[cfg(test)]
static COMPILES: LazyLock<std::sync::Mutex<HashMap<String, usize>>> =
    LazyLock::new(Default::default);

/// One scalar text containing `${{`, compiled once.
struct Compiled {
    /// What [`references`] returns for the text.
    references: BTreeSet<String>,
    /// The `projects.*` references, which need placeholders in the context.
    projects: Vec<String>,
    form: Form,
}

/// How a [`Compiled`] text is evaluated.
enum Form {
    /// The whole text is one expression, whose value keeps its JSON type.
    Expression(Expression<'static, 'static>),
    /// Text around expressions, rendered to a string. Each holds its own environment
    /// because a MiniJinja template borrows the environment it is stored in.
    Template(Environment<'static>),
    /// Text whose parts compile but whose template does not; evaluation reports this
    /// message, while [`references`] still answers.
    Broken(String),
}

/// The compiled form of `text`, from the cache or compiled now. Compile errors are not
/// cached; such a text fails every time it is evaluated anyway.
fn compiled(text: &str) -> Result<Arc<Compiled>, ConfigError> {
    if let Some(found) = CACHE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(text)
    {
        return Ok(found.clone());
    }
    let mut cache = CACHE.write().unwrap_or_else(PoisonError::into_inner);
    // Another thread may have compiled it while this one waited for the write lock.
    if let Some(found) = cache.get(text) {
        return Ok(found.clone());
    }
    let compiled = Arc::new(compile(text)?);
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(text.to_owned(), compiled.clone());
    Ok(compiled)
}

/// Compiles `text`: each expression for its references, then the evaluation form.
/// Errors keep the order and wording evaluation had before the cache: an unclosed
/// `${{` first, then the first invalid expression.
fn compile(text: &str) -> Result<Compiled, ConfigError> {
    #[cfg(test)]
    {
        *COMPILES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(text.to_owned())
            .or_default() += 1;
    }
    let env = EXPRESSIONS
        .as_ref()
        .map_err(|message| ConfigError::plain(message.clone()))?;
    let chunks = parts(text)?;
    let whole = chunks.len() == 1 && chunks[0].0 == 0 && chunks[0].1 == text.len();
    let invalid =
        |e: minijinja::Error| ConfigError::plain(format!("invalid expression: {}", describe(&e)));
    let mut references = BTreeSet::new();
    let mut single = None;
    for (_, _, body) in chunks {
        let expression = env
            .compile_expression_owned(body.to_owned())
            .map_err(invalid)?;
        references.extend(expression.undeclared_variables(true));
        let (normalized, original_keys) = normalize_static_subscripts(body);
        if normalized != body {
            let expression = env.compile_expression_owned(normalized).map_err(invalid)?;
            for reference in expression.undeclared_variables(true) {
                let original = reference
                    .split('.')
                    .map(|component| {
                        original_keys
                            .iter()
                            .find(|(alias, _)| alias == component)
                            .map_or(component, |(_, key)| key.as_str())
                    })
                    .collect::<Vec<_>>()
                    .join(".");
                references.insert(reference);
                references.insert(original);
            }
        }
        if whole {
            single = Some(expression);
        }
    }
    // A call of `domain()` reads like a variable to the analysis. What it
    // depends on is the lane index, which also keeps a text that calls it from
    // being evaluated as a constant at load time, when no lane is known.
    if references.remove(DOMAIN_FUNCTION) {
        references.insert("lane.index".into());
    }
    let form = match single {
        Some(expression) => Form::Expression(expression),
        None => {
            let mut env = environment()?;
            match env.add_template_owned(TEMPLATE_NAME, text.to_owned()) {
                Ok(()) => Form::Template(env),
                Err(e) => Form::Broken(format!("expression failed: {}", describe(&e))),
            }
        }
    };
    Ok(Compiled {
        projects: references
            .iter()
            .filter(|r| r.starts_with("projects."))
            .cloned()
            .collect(),
        references,
        form,
    })
}

/// `${{ env.X }}` on Windows, where variable names ignore case: the `env`
/// namespace holds names folded by `devlanes_core::env::fold_name`, and a
/// lookup folds the name it asks for the same way, so `env.Path` finds `PATH`.
#[cfg(windows)]
mod env_lookups {
    use std::sync::Arc;

    use minijinja::value::{Enumerator, Object, Value};

    /// `context` with an `env` namespace that answers names in any case.
    pub(super) fn fold(context: Value) -> Value {
        Value::from_object(Context(context))
    }

    /// The evaluation context; only `env` is wrapped.
    #[derive(Debug)]
    struct Context(Value);

    impl Object for Context {
        /// The namespace `key`, with `env` wrapped in [`Env`].
        fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
            let value = defined(self.0.get_item(key).ok()?)?;
            if key.as_str() == Some("env") {
                Some(Value::from_object(Env(value)))
            } else {
                Some(value)
            }
        }

        /// The namespace names.
        fn enumerate(self: &Arc<Self>) -> Enumerator {
            keys(&self.0)
        }
    }

    /// The `env` namespace.
    #[derive(Debug)]
    struct Env(Value);

    impl Object for Env {
        /// The variable `key` names, looked up by its folded name.
        fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
            let name = devlanes_core::env::fold_name(key.as_str()?.to_owned());
            defined(self.0.get_item(&Value::from(name)).ok()?)
        }

        /// The variable names.
        fn enumerate(self: &Arc<Self>) -> Enumerator {
            keys(&self.0)
        }
    }

    /// `value` unless it is undefined, which a missing key gives.
    fn defined(value: Value) -> Option<Value> {
        (!value.is_undefined()).then_some(value)
    }

    /// The keys of the map `value`.
    fn keys(value: &Value) -> Enumerator {
        Enumerator::Values(value.try_iter().map(Iterator::collect).unwrap_or_default())
    }
}

/// Offsets added to `lane.port` in literal sums such as `lane.port + 5`, `lane['port'] + 5`
/// or `5 + lane.port`, which must stay inside the lane's port block.
pub(crate) fn lane_port_offsets(text: &str) -> Result<Vec<u64>, ConfigError> {
    let mut offsets = Vec::new();
    for (_, _, body) in parts(text)? {
        let (normalized, _) = normalize_static_subscripts(body);
        let compact: String = normalized.chars().filter(|c| !c.is_whitespace()).collect();
        let digits = |s: &str| -> Option<u64> {
            let number: String = s.chars().take_while(char::is_ascii_digit).collect();
            number.parse().ok()
        };
        for (index, _) in compact.match_indices("lane.port") {
            let after = &compact[index + "lane.port".len()..];
            let identifier = |c: char| c.is_alphanumeric() || c == '_' || c == '.';
            // `lane.ports`, `lane.port_name` and `other.lane.port` are other variables.
            if after.starts_with(identifier) || compact[..index].ends_with(identifier) {
                continue;
            }
            if let Some(offset) = after.strip_prefix('+').and_then(digits) {
                offsets.push(offset);
            }
            let before = &compact[..index];
            if let Some(head) = before.strip_suffix('+') {
                let start = head
                    .rfind(|c: char| !c.is_ascii_digit())
                    .map_or(0, |i| i + 1);
                if let Ok(offset) = head[start..].parse() {
                    offsets.push(offset);
                }
            }
        }
    }
    Ok(offsets)
}

/// Return static dotted references using MiniJinja's own expression analysis.
/// The text is compiled once and shared with [`evaluate`].
pub fn references(text: &str) -> Result<BTreeSet<String>, ConfigError> {
    if !text.contains("${{") {
        return Ok(BTreeSet::new());
    }
    Ok(compiled(text)?.references.clone())
}

/// Evaluate every templated scalar in a typed service, task, host, or integration value.
/// Call separately per service/replica after allocating ports and assembling its context.
pub fn evaluate_typed<T: Serialize + serde::de::DeserializeOwned, C: EvaluationContext>(
    value: &T,
    context: &C,
) -> Result<T, ConfigError> {
    let mut value = serde_json::to_value(value)
        .map_err(|_| ConfigError::plain("cannot serialize typed configuration"))?;
    evaluate_tree(&mut value, &context.prepared(), "")?;
    serde_path_to_error::deserialize(value).map_err(|error| {
        ConfigError::plain(format!(
            "{}: expression result has the wrong type: {}",
            error.path(),
            crate::scalar::plain_type_names(&error.inner().to_string())
        ))
    })
}

/// Check fully constant expressions at load time; runtime-dependent values remain templated.
pub(crate) fn validate_constants<T: Serialize + serde::de::DeserializeOwned>(
    value: &T,
) -> Result<(), ConfigError> {
    /// Evaluate only expressions whose reference set is empty.
    fn visit(value: &mut Value) -> Result<(), ConfigError> {
        match value {
            Value::String(text) if text.contains("${{") && references(text)?.is_empty() => {
                *value = evaluate(text, &ExpressionContext::default())?
            }
            Value::Object(map) => {
                for value in map.values_mut() {
                    visit(value)?;
                }
            }
            Value::Array(values) => {
                for value in values {
                    visit(value)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut encoded = serde_json::to_value(value)
        .map_err(|_| ConfigError::plain("cannot serialize typed configuration"))?;
    visit(&mut encoded)?;
    let _: T = serde_path_to_error::deserialize(encoded).map_err(|error| {
        ConfigError::plain(format!(
            "{}: constant expression has the wrong type: {}",
            error.path(),
            crate::scalar::plain_type_names(&error.inner().to_string())
        ))
    })?;
    Ok(())
}

/// Traverse evaluated data without including resolved secrets in error messages.
fn evaluate_tree(
    value: &mut Value,
    context: &PreparedContext,
    path: &str,
) -> Result<(), ConfigError> {
    match value {
        Value::String(text) if text.contains("${{") => {
            *value = evaluate(text, context)
                .map_err(|e| ConfigError::plain(format!("{path}: {}", e.message)))?;
        }
        Value::Object(map) => {
            for (key, value) in map {
                evaluate_tree(value, context, &format!("{path}.{key}"))?;
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter_mut().enumerate() {
                evaluate_tree(value, context, &format!("{path}.{index}"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Evaluate each override against the caller's outer environment, never sibling overrides.
pub fn evaluate_env<C: EvaluationContext>(
    values: &crate::Env,
    context: &C,
) -> Result<indexmap::IndexMap<String, crate::Scalar>, ConfigError> {
    let context = context.prepared();
    let context = context.as_ref();
    values
        .iter()
        .map(|(name, value)| value.evaluate(context).map(|value| (name.clone(), value)))
        .collect()
}

/// Resolve lane vars or the current matrix row using lane.index supplied by the caller.
pub fn resolve_lane_vars(
    config: &crate::LanesConfig,
    context: &ExpressionContext,
) -> Result<indexmap::IndexMap<String, crate::Scalar>, ConfigError> {
    let values = if let Some(matrix) = &config.matrix {
        let index = context
            .lane
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|i| usize::try_from(i).ok())
            .ok_or_else(|| {
                ConfigError::plain("lane.index must be a nonnegative integer for matrix resolution")
            })?;
        matrix
            .get(index)
            .ok_or_else(|| ConfigError::plain("lane.index exceeds the configured matrix"))?
    } else {
        &config.vars
    };
    evaluate_lane_variables(values, context)
}

/// Resolve configured keys before consumers, preserving declaration order in the result.
fn evaluate_lane_variables(
    values: &crate::Env,
    context: &ExpressionContext,
) -> Result<indexmap::IndexMap<String, crate::Scalar>, ConfigError> {
    let mut pending: indexmap::IndexMap<String, BTreeSet<String>> = indexmap::IndexMap::new();
    for (name, value) in values {
        let refs = match value {
            crate::Templated::Expression(text) => references(text)?,
            _ => BTreeSet::new(),
        };
        let dependencies = refs
            .iter()
            .filter_map(|r| r.strip_prefix("lane."))
            .filter_map(|r| {
                values
                    .keys()
                    .find(|name| name.as_str() == r || name.replace('-', "_") == r)
                    .cloned()
            })
            .collect();
        pending.insert(name.clone(), dependencies);
    }
    let graph = pending
        .iter()
        .map(|(name, refs)| (name.clone(), refs.iter().cloned().collect()))
        .collect();
    check_cycles(&graph)?;
    let mut context = context.clone();
    let mut done = indexmap::IndexMap::new();
    while !pending.is_empty() {
        let next = pending
            .iter()
            .find(|(_, deps)| deps.iter().all(|name| done.contains_key(name)))
            .map(|(name, _)| name.clone())
            .ok_or_else(|| ConfigError::plain("unresolvable variable dependency cycle"))?;
        let value = values[&next].evaluate(&context)?;
        let map = context
            .lane
            .as_object_mut()
            .ok_or_else(|| ConfigError::plain("lane context must be an object"))?;
        map.insert(
            next.clone(),
            serde_json::to_value(&value)
                .map_err(|_| ConfigError::plain("cannot serialize resolved scalar"))?,
        );
        pending.shift_remove(&next);
        done.insert(next, value);
    }
    Ok(values
        .keys()
        .filter_map(|name| done.shift_remove(name).map(|value| (name.clone(), value)))
        .collect())
}

/// Normalize literal bracket lookups for MiniJinja's attribute-only dependency analysis.
fn normalize_static_subscripts(body: &str) -> (String, Vec<(String, String)>) {
    let mut output = String::new();
    let mut original_keys = Vec::new();
    let mut chars = body.char_indices().peekable();
    let mut quote = None;
    let mut escaped = false;
    while let Some((index, c)) = chars.next() {
        if escaped {
            output.push(c);
            escaped = false;
            continue;
        }
        if let Some(q) = quote {
            output.push(c);
            if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        if c == '['
            && output
                .trim_end()
                .chars()
                .last()
                .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | ']' | ')'))
        {
            let rest = body[index + 1..].trim_start();
            if let Some(q @ ('\'' | '"')) = rest.chars().next()
                && let Some(end) = rest[1..].find(q)
            {
                let key = &rest[1..end + 1];
                let after = rest[end + 2..].trim_start();
                if after.starts_with(']')
                    && !key.is_empty()
                    && key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                    && !key.starts_with(|c: char| c.is_ascii_digit())
                {
                    let consumed = body.len() - after.len() + 1;
                    let alias = key.replace('-', "_");
                    original_keys.push((alias.clone(), key.to_owned()));
                    output.push('.');
                    output.push_str(&alias);
                    while chars.peek().is_some_and(|(i, _)| *i < consumed) {
                        chars.next();
                    }
                    continue;
                }
            }
        }
        if matches!(c, '\'' | '"') {
            quote = Some(c);
        }
        output.push(c);
    }
    (output, original_keys)
}

/// Resolve a dependency graph with a readable cycle chain.
pub(crate) fn check_cycles(
    graph: &indexmap::IndexMap<String, Vec<String>>,
) -> Result<(), ConfigError> {
    /// Walk only configured nodes; external runtime references do not form local edges.
    fn visit(
        node: &str,
        graph: &indexmap::IndexMap<String, Vec<String>>,
        active: &mut Vec<String>,
        done: &mut BTreeSet<String>,
    ) -> Result<(), ConfigError> {
        if done.contains(node) {
            return Ok(());
        }
        if let Some(start) = active.iter().position(|n| n == node) {
            let mut cycle = active[start..].to_vec();
            cycle.push(node.into());
            return Err(ConfigError::plain(format!(
                "expression/reference cycle: {}",
                cycle.join(" -> ")
            )));
        }
        active.push(node.into());
        if let Some(edges) = graph.get(node) {
            for edge in edges {
                if graph.contains_key(edge) {
                    visit(edge, graph, active, done)?;
                }
            }
        }
        active.pop();
        done.insert(node.into());
        Ok(())
    }
    let mut done = BTreeSet::new();
    for node in graph.keys() {
        visit(node, graph, &mut Vec::new(), &mut done)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A context whose `env` namespace holds `PATH`.
    fn context() -> ExpressionContext {
        ExpressionContext {
            env: serde_json::json!({"PATH": "C:/bin", "A-B": "dashed"}),
            lane: serde_json::json!({"index": 2}),
            ..ExpressionContext::default()
        }
    }

    #[test]
    /// Evaluating and analysing one text many times, against raw and prepared
    /// contexts, compiles it once.
    fn a_text_compiles_once() {
        let text = "${{ lane.index + 40 }}";
        let mixed = "port-${{ lane.index }}-${{ env['A-B'] }}";
        let raw = context();
        let prepared = raw.prepare();
        for _ in 0..50 {
            assert_eq!(evaluate(text, &raw).expect("raw"), Value::from(42));
            assert_eq!(
                evaluate(text, &prepared).expect("prepared"),
                Value::from(42)
            );
            assert_eq!(
                evaluate(mixed, &prepared).expect("mixed"),
                Value::String("port-2-dashed".into())
            );
            assert!(references(text).expect("references").contains("lane.index"));
            assert!(references(mixed).expect("references").contains("env.A-B"));
        }
        let compiles = COMPILES.lock().expect("compiles");
        assert_eq!(compiles.get(text), Some(&1));
        assert_eq!(compiles.get(mixed), Some(&1));
    }

    #[test]
    /// `domain()` follows the lane and the suffix, works in mixed text, counts
    /// as a `lane.index` reference, and refuses a name that isn't hostname
    /// labels or a context without a lane.
    fn domain_names_a_site_per_lane() {
        let lane = |index: u64, suffix: &str| ExpressionContext {
            lane: serde_json::json!({"index": index}),
            domain_suffix: suffix.into(),
            ..ExpressionContext::default()
        };
        let eval = |text: &str, context: &ExpressionContext| {
            evaluate(text, context).map_err(|e| e.message)
        };
        let text = "${{ domain('pages') }}";
        assert_eq!(
            eval(text, &lane(0, "localhost")),
            Ok("pages.localhost".into())
        );
        assert_eq!(
            eval(text, &lane(3, "localhost")),
            Ok("pages3.localhost".into())
        );
        assert_eq!(eval(text, &lane(3, "test")), Ok("pages3.test".into()));
        assert_eq!(
            eval("*.${{ domain('Docs.Pages') }}", &lane(2, "localhost")),
            Ok("*.docs.pages2.localhost".into())
        );
        assert_eq!(
            references(text).expect("references"),
            BTreeSet::from(["lane.index".to_owned()])
        );
        for bad in [
            "${{ domain('a_b') }}",
            "${{ domain('*.x') }}",
            "${{ domain('') }}",
        ] {
            let error = eval(bad, &lane(1, "localhost")).expect_err(bad);
            assert!(error.contains("is not a hostname label"), "{bad}: {error}");
            assert!(error.contains("domain('"), "{bad}: {error}");
        }
        let error = eval(text, &ExpressionContext::default()).expect_err("no lane");
        assert!(error.contains("where a lane is known"), "{error}");
    }

    #[test]
    /// A prepared context evaluates like the context it came from, dashed-key
    /// aliases and `projects` placeholders included.
    fn prepared_context_matches_raw() {
        let raw = ExpressionContext {
            services: serde_json::json!({"crm-api": {"port": 7}}),
            projects: serde_json::json!({}),
            ..context()
        };
        let prepared = raw.prepare();
        for text in [
            "${{ services.crm_api.port }}",
            "${{ services['crm-api'].port }}",
            "x=${{ env.A_B }}",
            "${{ projects.other.services.api.url | default('none') }}",
            "${{ env.MISSING }}",
        ] {
            let raw = evaluate(text, &raw).map_err(|e| e.to_string());
            let prepared = evaluate(text, &prepared).map_err(|e| e.to_string());
            assert_eq!(raw, prepared, "{text}");
        }
    }

    #[test]
    /// `env` names match exactly on Unix and ignoring case on Windows, where
    /// `Path` is `PATH`; other namespaces and filters behave the same.
    fn env_lookups_follow_the_platform() {
        let context = context();
        let eval = |text: &str| evaluate(text, &context).map_err(|e| e.to_string());
        assert_eq!(eval("${{ env.PATH }}"), Ok(Value::String("C:/bin".into())));
        assert_eq!(eval("${{ lane.index }}"), Ok(Value::from(2)));
        assert_eq!(
            eval("${{ env.MISSING | default('none') }}"),
            Ok(Value::String("none".into()))
        );
        assert!(eval("${{ env.MISSING }}").is_err());
        assert_eq!(eval("${{ env.A_B }}"), Ok(Value::String("dashed".into())));
        #[cfg(unix)]
        assert!(eval("${{ env.Path }}").is_err());
        #[cfg(windows)]
        {
            assert_eq!(eval("${{ env.Path }}"), Ok(Value::String("C:/bin".into())));
            assert_eq!(
                eval("x-${{ env['path'] }}"),
                Ok(Value::String("x-C:/bin".into()))
            );
            assert_eq!(eval("${{ 'Path' in env }}"), Ok(Value::Bool(true)));
        }
    }
}
