//! Checking a pack the way the shard checks it, so a bad pack is caught at the
//! build step rather than at startup.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use crate::manifest::{self, Manifest};
use crate::proto::RowType;
use crate::table;

#[derive(Debug)]
pub struct VerifyReport {
    pub pack_id: String,
    pub ruleset: String,
    pub zone: String,
    pub tables: Vec<(String, u32)>,
}

/// Reads `pack`, checks every rule ADR 0029 puts on a reader, and reports what
/// it found. Every failure names the table and the expectation it broke.
pub fn verify(pack: &Path) -> Result<VerifyReport> {
    let manifest_path = pack.join(manifest::FILE_NAME);
    let text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let document: Manifest = serde_json::from_str(&text)
        .with_context(|| format!("parse {}", manifest_path.display()))?;

    ensure!(
        document.schema_version == manifest::SCHEMA_VERSION,
        "pack schema_version is {}, and this build only reads version {}",
        document.schema_version,
        manifest::SCHEMA_VERSION
    );

    let mut listed = BTreeSet::new();
    let mut digest_input = Vec::with_capacity(document.tables.len());
    let mut summary = Vec::with_capacity(document.tables.len());
    for entry in &document.tables {
        let path = pack.join(&entry.file);
        let bytes = fs::read(&path)
            .with_context(|| format!("read table {} at {}", entry.name, path.display()))?;

        ensure!(
            bytes.len() as u64 == entry.bytes,
            "table {} is {} bytes, but the manifest records {}",
            entry.name,
            bytes.len(),
            entry.bytes
        );
        let digest = blake3::hash(&bytes).to_hex().to_string();
        ensure!(
            digest == entry.blake3,
            "table {} digest mismatch: table bytes hash to {digest}, manifest records {}",
            entry.name,
            entry.blake3
        );

        let view =
            table::validate(&bytes).with_context(|| format!("validate table {}", entry.name))?;
        let row_type = RowType::try_from(view.row_type_id).map_err(|_| {
            anyhow::anyhow!(
                "table {} declares unknown row_type_id {}",
                entry.name,
                view.row_type_id
            )
        })?;
        ensure!(
            row_type.as_str_name() == entry.row_type,
            "table {} holds {} rows but the manifest records {}",
            entry.name,
            row_type.as_str_name(),
            entry.row_type
        );
        ensure!(
            view.row_count == entry.rows,
            "table {} holds {} rows but the manifest records {}",
            entry.name,
            view.row_count,
            entry.rows
        );

        listed.insert(entry.file.clone());
        summary.push((entry.name.clone(), view.row_count));
        digest_input.push((entry.name.clone(), bytes));
    }

    let tables_directory = pack.join("tables");
    if tables_directory.is_dir() {
        for found in fs::read_dir(&tables_directory)
            .with_context(|| format!("list {}", tables_directory.display()))?
        {
            let name = found?.file_name();
            let relative = format!("tables/{}", name.to_string_lossy());
            if !listed.contains(&relative) {
                bail!("pack holds {relative}, which the manifest does not list");
            }
        }
    }

    let recomputed = manifest::pack_id(&digest_input);
    ensure!(
        recomputed == document.pack_id,
        "pack_id mismatch: table bytes hash to {recomputed}, manifest records {}",
        document.pack_id
    );

    Ok(VerifyReport {
        pack_id: document.pack_id,
        ruleset: document.ruleset,
        zone: document.zone,
        tables: summary,
    })
}
