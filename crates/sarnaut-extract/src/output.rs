use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::validation::{SchemaKind, SchemaValidator};

pub(crate) struct OutputWriter {
    dry_run: bool,
    validator: Option<SchemaValidator>,
}

impl OutputWriter {
    pub(crate) fn new(dry_run: bool, schema_dir: Option<&Path>) -> Result<Self> {
        Ok(Self {
            dry_run,
            validator: schema_dir.map(SchemaValidator::load).transpose()?,
        })
    }

    pub(crate) fn write<T: Serialize>(
        &self,
        path: &Path,
        kind: SchemaKind,
        document: &T,
    ) -> Result<bool> {
        if let Some(validator) = &self.validator {
            let instance = serde_json::to_value(document)?;
            validator
                .validate(kind, &instance)
                .with_context(|| format!("validate {}", path.display()))?;
        }
        self.write_unvalidated(path, document)
    }

    /// Write a document that no public schema covers, such as the item index.
    pub(crate) fn write_unvalidated<T: Serialize>(
        &self,
        path: &Path,
        document: &T,
    ) -> Result<bool> {
        let mut yaml = serde_yaml::to_string(document)?;
        if !yaml.ends_with('\n') {
            yaml.push('\n');
        }
        if self.dry_run {
            return Ok(false);
        }
        if fs::read_to_string(path).is_ok_and(|existing| existing == yaml) {
            return Ok(true);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create output directory {}", parent.display()))?;
        }
        fs::write(path, yaml).with_context(|| format!("write {}", path.display()))?;
        Ok(false)
    }
}
