//! The plugin-prefix check: which plugins a pom and its parents in the worktree declare.
//!
//! Maven resolves `prefix:goal` against the plugins of the project (build
//! plugins and plugin management, inherited from parents), then against the
//! plugin groups `org.apache.maven.plugins`, `org.codehaus.mojo` and those in
//! `settings.xml`. Only the worktree is visible here, so a miss is a note.

use crate::{normalize, shown};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Prefixes of common plugins in the default plugin groups, which Maven finds
/// without a declaration (`mvn dependency:tree`, `mvn versions:set`).
const DEFAULT_GROUP_PREFIXES: &[&str] = &[
    "antrun",
    "archetype",
    "assembly",
    "build-helper",
    "clean",
    "compiler",
    "dependency",
    "deploy",
    "enforcer",
    "exec",
    "failsafe",
    "help",
    "install",
    "jar",
    "javadoc",
    "release",
    "resources",
    "shade",
    "site",
    "source",
    "surefire",
    "versions",
    "war",
    "wrapper",
];

/// Notes for `prefix:goal` tokens in `goals` whose plugin `pom` and its
/// parents inside `worktree` do not declare. Reads the poms only when a
/// token needs checking; a pom that does not parse yields a note, since
/// Maven reports its own parse error with more detail.
pub(crate) fn plugin_notes(pom: &Path, worktree: &Path, goals: &[&str]) -> Vec<String> {
    let prefixes: BTreeSet<&str> = goals.iter().filter_map(|goal| prefix(goal)).collect();
    if prefixes.is_empty() {
        return Vec::new();
    }
    let declared = match declared_plugins(pom, worktree) {
        Ok(declared) => declared,
        Err(note) => return vec![note],
    };
    prefixes
        .into_iter()
        .filter(|prefix| {
            !declared.contains(&format!("{prefix}-maven-plugin"))
                && !declared.contains(&format!("maven-{prefix}-plugin"))
        })
        .map(|prefix| {
            format!(
                "no {prefix}-maven-plugin or maven-{prefix}-plugin is declared in {} or its parents in the worktree, so Maven may fail with \"No plugin found for prefix '{prefix}'\"; parents outside the worktree and pluginGroups in settings.xml were not checked",
                shown(worktree, pom)
            )
        })
        .collect()
}

/// The plugin prefix of `goal` when it needs checking: `prefix:goal` with one
/// colon (`group:artifact[:version]:goal` names the plugin itself), not a
/// Maven 4 `before:`/`after:` phase hook, and not a common default-group plugin.
fn prefix(goal: &str) -> Option<&str> {
    let (prefix, rest) = goal.split_once(':')?;
    if rest.contains(':') || prefix.is_empty() {
        return None;
    }
    if matches!(prefix, "before" | "after") || DEFAULT_GROUP_PREFIXES.contains(&prefix) {
        return None;
    }
    Some(prefix)
}

/// Artifact ids of every `<plugin>` in `pom` and its parents reachable through
/// `<parent><relativePath>` (default `../pom.xml`) inside `worktree`. Plugins
/// in profiles and reporting count too: a looser check only means fewer notes.
fn declared_plugins(pom: &Path, worktree: &Path) -> Result<BTreeSet<String>, String> {
    let mut declared = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut next = Some((pom.to_path_buf(), None::<String>));
    while let Some((path, expected)) = next.take() {
        if !visited.insert(path.clone()) {
            break;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", shown(worktree, &path)))?;
        let options = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let doc = roxmltree::Document::parse_with_options(&text, options).map_err(|error| {
            format!(
                "cannot parse {} ({error}), so plugin prefixes were not checked",
                shown(worktree, &path)
            )
        })?;
        let project = doc.root_element();
        // A relativePath that leads to another project means Maven takes the
        // parent from a repository instead.
        if expected.is_some_and(|expected| text_of(project, "artifactId") != Some(expected)) {
            break;
        }
        declared.extend(
            project
                .descendants()
                .filter(|node| node.tag_name().name() == "plugin")
                .filter_map(|plugin| text_of(plugin, "artifactId")),
        );
        next = child(project, "parent").and_then(|parent| {
            let path = parent_pom(&path, parent, worktree)?;
            Some((path, text_of(parent, "artifactId")))
        });
    }
    Ok(declared)
}

/// The parent pom `parent` points to, when it is a file inside `worktree`. An
/// empty `<relativePath/>` turns the local lookup off.
fn parent_pom(pom: &Path, parent: roxmltree::Node<'_, '_>, worktree: &Path) -> Option<PathBuf> {
    let relative = match child(parent, "relativePath") {
        Some(node) => node.text().unwrap_or_default().trim().to_owned(),
        None => "../pom.xml".to_owned(),
    };
    if relative.is_empty() {
        return None;
    }
    let mut path = normalize(&pom.parent()?.join(relative));
    if path.is_dir() {
        path.push(crate::MANIFEST);
    }
    (path.starts_with(worktree) && path.is_file()).then_some(path)
}

/// The first child element of `node` with local name `name`, ignoring the POM namespace.
fn child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == name)
}

/// The trimmed text of `node`'s child element `name`.
fn text_of(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    child(node, name)
        .and_then(|child| child.text())
        .map(|text| text.trim().to_owned())
}
