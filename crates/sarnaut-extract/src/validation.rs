use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// The public JSON Schemas an extracted document can be checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SchemaKind {
    Item,
    Quest,
    Spawn,
    Route,
    MobKind,
    Faction,
    LootTable,
    Locale,
}

impl SchemaKind {
    pub(crate) const ALL: [Self; 8] = [
        Self::Item,
        Self::Quest,
        Self::Spawn,
        Self::Route,
        Self::MobKind,
        Self::Faction,
        Self::LootTable,
        Self::Locale,
    ];

    fn file_name(self) -> &'static str {
        match self {
            Self::Item => "item.schema.json",
            Self::Quest => "quest.schema.json",
            Self::Spawn => "spawn.schema.json",
            Self::Route => "route.schema.json",
            Self::MobKind => "mobkind.schema.json",
            Self::Faction => "faction.schema.json",
            Self::LootTable => "loot-table.schema.json",
            Self::Locale => "locale.schema.json",
        }
    }
}

pub struct SchemaValidator {
    validators: BTreeMap<SchemaKind, jsonschema::Validator>,
}

impl SchemaValidator {
    pub fn load(directory: &Path) -> Result<Self> {
        let mut validators = BTreeMap::new();
        for kind in SchemaKind::ALL {
            validators.insert(kind, load_one(directory, kind)?);
        }
        Ok(Self { validators })
    }

    pub fn validate(&self, kind: SchemaKind, instance: &Value) -> Result<()> {
        let validator = self
            .validators
            .get(&kind)
            .with_context(|| format!("no validator for {}", kind.file_name()))?;
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .take(20)
            .map(|error| error.to_string())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            bail!(
                "{} validation failed:\n{}",
                kind.file_name(),
                errors.join("\n")
            )
        }
    }
}

/// Serve every `$ref` out of the schema directory instead of over the network.
///
/// The schemas carry absolute `$id`s under `https://schemas.sarnautcore.org/`, so a
/// sibling reference such as `common.schema.json#/$defs/base` resolves to an absolute
/// URI that no local registry knows. Mapping the URI's last path segment back onto a
/// file in the same directory is what makes offline validation possible at all.
struct DirectoryRetriever {
    directory: PathBuf,
}

impl jsonschema::Retrieve for DirectoryRetriever {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let name = uri
            .path()
            .as_str()
            .rsplit('/')
            .next()
            .filter(|segment| !segment.is_empty())
            .ok_or_else(|| format!("cannot map {uri} onto a schema file"))?;
        let path = self.directory.join(name);
        let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

fn load_one(directory: &Path, kind: SchemaKind) -> Result<jsonschema::Validator> {
    let path = directory.join(kind.file_name());
    let bytes = fs::read(&path).with_context(|| format!("read schema {}", path.display()))?;
    let schema: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse schema {}", path.display()))?;
    jsonschema::options()
        .with_retriever(DirectoryRetriever {
            directory: directory.to_path_buf(),
        })
        .build(&schema)
        .map_err(|error| anyhow::anyhow!(error.to_string()))
        .with_context(|| format!("compile schema {}", path.display()))
}
