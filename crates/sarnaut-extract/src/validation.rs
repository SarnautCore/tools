use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

#[derive(Debug, Clone, Copy)]
pub enum SchemaKind {
    Item,
    Quest,
    Spawn,
    Route,
}

impl SchemaKind {
    fn file_name(self) -> &'static str {
        match self {
            Self::Item => "item.schema.json",
            Self::Quest => "quest.schema.json",
            Self::Spawn => "spawn.schema.json",
            Self::Route => "route.schema.json",
        }
    }
}

pub struct SchemaValidator {
    item: jsonschema::Validator,
    quest: jsonschema::Validator,
    spawn: jsonschema::Validator,
    route: jsonschema::Validator,
}

impl SchemaValidator {
    pub fn load(directory: &Path) -> Result<Self> {
        Ok(Self {
            item: load_one(directory, SchemaKind::Item)?,
            quest: load_one(directory, SchemaKind::Quest)?,
            spawn: load_one(directory, SchemaKind::Spawn)?,
            route: load_one(directory, SchemaKind::Route)?,
        })
    }

    pub fn validate(&self, kind: SchemaKind, instance: &Value) -> Result<()> {
        let validator = match kind {
            SchemaKind::Item => &self.item,
            SchemaKind::Quest => &self.quest,
            SchemaKind::Spawn => &self.spawn,
            SchemaKind::Route => &self.route,
        };
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

fn load_one(directory: &Path, kind: SchemaKind) -> Result<jsonschema::Validator> {
    let path = directory.join(kind.file_name());
    let bytes = fs::read(&path).with_context(|| format!("read schema {}", path.display()))?;
    let schema: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse schema {}", path.display()))?;
    jsonschema::validator_for(&schema)
        .map_err(|error| anyhow::anyhow!(error.to_string()))
        .with_context(|| format!("compile schema {}", path.display()))
}
