//! `build-report.json`: everything about a build that must not affect its
//! digest.
//!
//! ADR 0029 keeps curation notes out of table bytes so that documenting a patch
//! cannot move `pack_id`. They still have to go somewhere a human can read, and
//! so do the answers to "which layers applied", "what did the compiler decline
//! to resolve", and "how much of the item catalogue did this touch". The report
//! sits beside `manifest.json`, is not an input to the digest, and is a
//! private-path artifact like the pack itself.

use anyhow::{Context, Result};
use serde::Serialize;

use crate::refs::ReferenceReport;
use crate::source::SourceTree;

pub const FILE_NAME: &str = "build-report.json";

#[derive(Debug, Serialize)]
pub struct BuildReport {
    pub pack_id: String,
    pub ruleset: String,
    pub zone: String,
    pub layers: Vec<LayerReport>,
    pub curation_notes: Vec<NoteReport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped_overlay_documents: Vec<SkippedOverlayReport>,
    pub overlay_conflicts: Vec<String>,
    pub deleted_documents: Vec<String>,
    pub selection: SelectionReport,
    pub references: ReferenceSummary,
}

#[derive(Debug, Serialize)]
pub struct LayerReport {
    pub id: String,
    pub description: String,
    pub documents: usize,
}

#[derive(Debug, Serialize)]
pub struct NoteReport {
    pub document: String,
    pub layer: String,
    pub source: String,
    pub note: String,
}

#[derive(Debug, Serialize)]
pub struct SkippedOverlayReport {
    pub document: String,
    pub layer: String,
    pub source: String,
    pub note: String,
}

/// What the compiler chose to compile out of what the tree offered.
#[derive(Debug, Serialize)]
pub struct SelectionReport {
    pub items_opened_from_index: usize,
    pub loot_tables_compiled: usize,
    pub loot_tables_unreachable: usize,
    pub locale_keys: usize,
}

#[derive(Debug, Serialize)]
pub struct ReferenceSummary {
    pub resolved: usize,
    pub dangling: Vec<String>,
    pub external: Vec<String>,
    pub unmodelled: Vec<String>,
    pub locale_gaps: Vec<String>,
    pub locale_gaps_by_namespace: std::collections::BTreeMap<String, usize>,
}

impl BuildReport {
    pub fn new(
        pack_id: &str,
        ruleset: &str,
        zone: &str,
        tree: &SourceTree,
        references: &ReferenceReport,
        locale_keys: usize,
    ) -> Self {
        Self {
            pack_id: pack_id.to_string(),
            ruleset: ruleset.to_string(),
            zone: zone.to_string(),
            layers: tree
                .layers
                .iter()
                .map(|layer| LayerReport {
                    id: layer.id.clone(),
                    description: layer.description.clone(),
                    documents: layer.documents,
                })
                .collect(),
            curation_notes: tree
                .notes
                .iter()
                .map(|note| NoteReport {
                    document: note.document_id.clone(),
                    layer: note.layer.clone(),
                    source: note.source.clone(),
                    note: note.note.clone(),
                })
                .collect(),
            skipped_overlay_documents: tree
                .skipped_overlay_documents
                .iter()
                .map(|document| SkippedOverlayReport {
                    document: document.document_id.clone(),
                    layer: document.layer.clone(),
                    source: document.source.clone(),
                    note: document.note.clone(),
                })
                .collect(),
            overlay_conflicts: tree
                .conflicts
                .iter()
                .map(|conflict| conflict.describe())
                .collect(),
            deleted_documents: tree.deleted.clone(),
            selection: SelectionReport {
                items_opened_from_index: tree.items_loaded_from_index,
                loot_tables_compiled: tree.loot_tables.len(),
                loot_tables_unreachable: tree.unreachable_loot_tables,
                locale_keys,
            },
            references: ReferenceSummary {
                resolved: references.resolved,
                dangling: references
                    .dangling
                    .iter()
                    .map(|entry| entry.describe())
                    .collect(),
                external: references
                    .external
                    .iter()
                    .map(|entry| {
                        format!(
                            "{}: {} references {} in zone {}",
                            entry.class, entry.referencer, entry.target, entry.zone
                        )
                    })
                    .collect(),
                unmodelled: references
                    .unmodelled
                    .iter()
                    .map(|entry| {
                        format!(
                            "{}: {} references {}, an interactive object this pack models no row for",
                            entry.class, entry.referencer, entry.target
                        )
                    })
                    .collect(),
                locale_gaps: references
                    .locale_gaps
                    .iter()
                    .map(|gap| {
                        format!("{} ({}) wants {}", gap.referencer, gap.field, gap.key)
                    })
                    .collect(),
                locale_gaps_by_namespace: crate::refs::gaps_by_namespace(references),
            },
        }
    }

    /// Canonical JSON, matching `manifest.json`'s conventions so the two files
    /// beside each other read the same way.
    pub fn to_canonical_json(&self) -> Result<String> {
        let mut text = serde_json::to_string_pretty(self).context("encode build report as JSON")?;
        text.push('\n');
        Ok(text)
    }
}
