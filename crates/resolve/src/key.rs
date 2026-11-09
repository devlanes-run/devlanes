//! A digest of everything a plan reads, so a caller can tell whether a fresh
//! plan of the same lane could differ from an earlier one without planning.

use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicU64, Ordering},
};

use devlanes_config::{Extensions, ProjectConfig};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ResolveInput;

/// The digest of a [`ResolveInput`]: two inputs with the same key plan the same
/// launches, except for what contributors read from their own files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InputsKey([u8; 32]);

/// What a config's expressions read of the namespaces that only expressions
/// reach (`lanes.main` and `projects`), so a key leaves out the rest of them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct References {
    /// Some expression reads `lanes.main`.
    lanes_main: bool,
    /// The projects expressions read by name; `None` when one is read by a
    /// name only known at evaluation, so every project counts.
    projects: Option<BTreeSet<String>>,
}

impl References {
    /// The references of every templated text of `config`. A text whose
    /// references cannot be read counts as reading both namespaces whole.
    pub fn of<E: Extensions>(config: &ProjectConfig<E>) -> Self {
        let mut references = Self {
            lanes_main: false,
            projects: Some(BTreeSet::new()),
        };
        match serde_json::to_value(config) {
            Ok(value) => references.collect(&value),
            Err(_) => references.read_everything(),
        }
        references
    }

    /// Adds the references of every string under `value`.
    fn collect(&mut self, value: &Value) {
        match value {
            Value::String(text) => match devlanes_config::references(text) {
                Ok(found) => found.iter().for_each(|path| self.add(path)),
                Err(_) => self.read_everything(),
            },
            Value::Array(items) => items.iter().for_each(|item| self.collect(item)),
            Value::Object(map) => map.values().for_each(|item| self.collect(item)),
            _ => {}
        }
    }

    /// Adds one dotted reference, such as `projects.acme.services.web.port`.
    fn add(&mut self, path: &str) {
        let mut parts = path.split('.');
        match (parts.next(), parts.next()) {
            (Some("lanes"), None | Some("main")) => self.lanes_main = true,
            (Some("projects"), Some(name)) => {
                if let Some(projects) = &mut self.projects {
                    projects.insert(name.to_owned());
                }
            }
            (Some("projects"), None) => self.projects = None,
            _ => {}
        }
    }

    /// Counts both namespaces as read whole.
    fn read_everything(&mut self) {
        self.lanes_main = true;
        self.projects = None;
    }
}

impl<E: Extensions> ResolveInput<'_, E> {
    /// The key of this input, with `lanes_main` and `projects` keyed only as far
    /// as `references` (the references of [`Self::config`]) reads them. `only`
    /// limits what is planned, not how, so it is left out. Contributors and
    /// script runners count by id: their own files are the one input no key
    /// covers.
    pub fn key(&self, references: &References) -> InputsKey {
        // Destructured without `..`, so a new input does not compile until it
        // is keyed here.
        let ResolveInput {
            config,
            facts,
            block,
            offsets,
            client_env,
            dotenv,
            infra,
            lanes_main,
            projects,
            profile,
            profiles,
            overrides,
            contributors,
            script_runners,
            ca_env,
            public_https_port,
            only: _,
        } = self;
        let mut digest = Digester(Sha256::new());
        digest.part("config", config);
        digest.part("facts", facts);
        digest.part("block", block);
        digest.part("offsets", &offsets.iter().collect::<Vec<_>>());
        digest.part("client_env", client_env);
        digest.part("dotenv", dotenv);
        digest.part("infra", infra);
        digest.part("lanes_main", &lanes_main.filter(|_| references.lanes_main));
        let projects: Value = match &references.projects {
            None => (*projects).clone(),
            Some(names) => names
                .iter()
                .map(|name| {
                    (
                        name.clone(),
                        projects.get(name).cloned().unwrap_or_default(),
                    )
                })
                .collect::<serde_json::Map<String, Value>>()
                .into(),
        };
        digest.part("projects", &projects);
        digest.part("profile", profile);
        digest.part("profiles", profiles);
        digest.part("overrides", overrides);
        digest.part(
            "contributors",
            &contributors.iter().map(|c| c.id()).collect::<Vec<_>>(),
        );
        digest.part(
            "script_runners",
            &script_runners.iter().map(|r| r.id()).collect::<Vec<_>>(),
        );
        digest.part("ca_env", ca_env);
        digest.part("public_https_port", public_https_port);
        InputsKey(digest.0.finalize().into())
    }
}

/// Feeds serialized values to a SHA-256.
struct Digester(Sha256);

impl Digester {
    /// Adds `value` under `name`. A value that does not serialize makes the
    /// key equal to no other, so it can only cause a plan, never skip one.
    fn part(&mut self, name: &str, value: &impl Serialize) {
        static UNIQUE: AtomicU64 = AtomicU64::new(0);
        self.0.update(name.as_bytes());
        self.0.update([0]);
        if serde_json::to_writer(&mut *self, value).is_err() {
            self.0
                .update(UNIQUE.fetch_add(1, Ordering::Relaxed).to_le_bytes());
            self.0.update(b"unserializable");
        }
        self.0.update([0]);
    }
}

impl std::io::Write for Digester {
    /// Hashes `bytes`; never fails.
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    /// Nothing is buffered.
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
