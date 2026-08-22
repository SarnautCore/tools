//! `manifest.json`: the human-readable index of a pack, and the definition of
//! `pack_id` that ADR 0027's handshake digest compares.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = "manifest.json";
/// Format version of the pack itself. A reader rejects any other value.
pub const SCHEMA_VERSION: u32 = 2;
pub const BUILDER_NAME: &str = "sarnaut-pack";
/// `source.commit` for a build with no identified source revision.
pub const UNKNOWN_COMMIT: &str = "0000000000000000000000000000000000000000";

#[derive(Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub ruleset: String,
    pub zone: String,
    pub pack_id: String,
    pub builder: Builder,
    pub source: Source,
    pub keep_extra: bool,
    pub contracts: Contracts,
    pub tables: Vec<TableEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Contracts {
    /// BLAKE3-256 of the canonical product bag-layout catalog. Readers recompute
    /// this value from their own catalog and reject a compiler/runtime drift.
    pub bag_layout_catalog_blake3: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Builder {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Source {
    pub repo: String,
    pub commit: String,
    pub overlays: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TableEntry {
    pub name: String,
    pub file: String,
    pub row_type: String,
    pub rows: u32,
    pub bytes: u64,
    pub blake3: String,
}

impl Manifest {
    /// Canonical JSON: UTF-8, LF, two-space indent, trailing newline, keys in
    /// declaration order.
    pub fn to_canonical_json(&self) -> Result<String> {
        let mut text = serde_json::to_string_pretty(self).context("encode manifest as JSON")?;
        text.push('\n');
        Ok(text)
    }
}

/// BLAKE3-256 over the table bytes, in table-name order, with length prefixes so
/// that no pair of tables can be reassociated into the same byte stream.
///
/// The manifest itself is not an input: a builder-version bump that produces
/// identical tables leaves `pack_id` unchanged.
pub fn pack_id(tables: &[(String, Vec<u8>)]) -> String {
    let mut ordered: Vec<&(String, Vec<u8>)> = tables.iter().collect();
    ordered.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));

    let mut hasher = blake3::Hasher::new();
    for (name, bytes) in ordered {
        hasher.update(&(name.len() as u32).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    hasher.finalize().to_hex().to_string()
}
