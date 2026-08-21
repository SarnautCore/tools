//! `build-report.json`: everything about a build that must not affect its
//! digest.
//!
//! ADR 0029 keeps curation notes out of table bytes so that documenting a patch
//! cannot move `pack_id`. They still have to go somewhere a human can read, and
//! so do the answers to "which layers applied", "what did the compiler decline
//! to resolve", and "how much of the item catalogue did this touch". The report
//! sits beside `manifest.json`, is not an input to the digest, and is a
//! private-path artifact like the pack itself.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scripts: Option<ScriptCensusReport>,
    pub references: ReferenceSummary,
}

/// ADR 0036's per-tier node counts over every script row in the pack. CI diffs
/// these against the interpreter's checked-in tier table, which is what makes
/// widening coverage a reviewable diff rather than a quiet behaviour change.
#[derive(Debug, Serialize)]
pub struct ScriptCensusReport {
    pub quest_scripts: usize,
    pub script_triggers: usize,
    pub nodes: usize,
    /// Nodes per opcode, by tier.
    pub implemented: std::collections::BTreeMap<String, usize>,
    pub inert_and_counted: std::collections::BTreeMap<String, usize>,
    pub refused: std::collections::BTreeMap<String, usize>,
    /// Trigger rows reached from quest roots by following canonical trigger
    /// references. Root-element discovery also retains orphan rows, so this is
    /// deliberately separate from `script_triggers`.
    pub reachable_script_triggers: usize,
    pub reachable_nodes: usize,
    pub reachable_implemented: BTreeMap<String, usize>,
    pub reachable_inert_and_counted: BTreeMap<String, usize>,
    pub reachable_refused: BTreeMap<String, usize>,
}

impl ScriptCensusReport {
    fn of(tree: &SourceTree) -> Option<Self> {
        if tree.quest_scripts.is_empty() && tree.script_triggers.is_empty() {
            return None;
        }
        let mut census = Self {
            quest_scripts: tree.quest_scripts.len(),
            script_triggers: tree.script_triggers.len(),
            nodes: 0,
            implemented: Default::default(),
            inert_and_counted: Default::default(),
            refused: Default::default(),
            reachable_script_triggers: 0,
            reachable_nodes: 0,
            reachable_implemented: Default::default(),
            reachable_inert_and_counted: Default::default(),
            reachable_refused: Default::default(),
        };
        for document in &tree.quest_scripts {
            for node in document
                .start_impacts
                .iter()
                .chain(&document.trigger_agents)
            {
                census.count(node);
                census.count_reachable(node);
            }
        }
        for document in &tree.script_triggers {
            census.count(&document.root);
        }
        let triggers: BTreeMap<_, _> = tree
            .script_triggers
            .iter()
            .map(|document| (document.id.as_str(), document))
            .collect();
        let mut queue = VecDeque::new();
        queue.extend(
            tree.script_triggers
                .iter()
                .filter(|document| document.entrypoint)
                .map(|document| document.id.clone()),
        );
        for document in &tree.quest_scripts {
            for node in document
                .start_impacts
                .iter()
                .chain(&document.trigger_agents)
            {
                enqueue_trigger_refs(node, &mut queue);
            }
        }
        let mut reached = BTreeSet::new();
        while let Some(id) = queue.pop_front() {
            if !reached.insert(id.clone()) {
                continue;
            }
            let Some(document) = triggers.get(id.as_str()) else {
                continue;
            };
            census.count_reachable(&document.root);
            enqueue_trigger_refs(&document.root, &mut queue);
        }
        census.reachable_script_triggers = reached
            .iter()
            .filter(|id| triggers.contains_key(id.as_str()))
            .count();
        Some(census)
    }

    fn count(&mut self, node: &crate::source::ScriptNodeDocument) {
        self.nodes += 1;
        let bucket = match node.tier.as_str() {
            "implemented" => &mut self.implemented,
            "inert-and-counted" => &mut self.inert_and_counted,
            _ => &mut self.refused,
        };
        *bucket.entry(node.opcode.clone()).or_insert(0) += 1;
        for field in &node.fields {
            self.count_value(&field.value);
        }
    }

    fn count_value(&mut self, value: &crate::source::ScriptValueDocument) {
        if let Some(node) = &value.node {
            self.count(node);
        }
        if let Some(list) = &value.list {
            for entry in list {
                self.count_value(entry);
            }
        }
    }

    fn count_reachable(&mut self, node: &crate::source::ScriptNodeDocument) {
        self.reachable_nodes += 1;
        let bucket = match node.tier.as_str() {
            "implemented" => &mut self.reachable_implemented,
            "inert-and-counted" => &mut self.reachable_inert_and_counted,
            _ => &mut self.reachable_refused,
        };
        *bucket.entry(node.opcode.clone()).or_insert(0) += 1;
        for field in &node.fields {
            self.count_reachable_value(&field.value);
        }
    }

    fn count_reachable_value(&mut self, value: &crate::source::ScriptValueDocument) {
        if let Some(node) = &value.node {
            self.count_reachable(node);
        }
        if let Some(list) = &value.list {
            for entry in list {
                self.count_reachable_value(entry);
            }
        }
    }
}

fn enqueue_trigger_refs(node: &crate::source::ScriptNodeDocument, queue: &mut VecDeque<String>) {
    let mut references = Vec::new();
    crate::source::collect_script_refs(node, &mut references);
    queue.extend(
        references
            .into_iter()
            .filter(|reference| reference.row_type.as_deref() == Some("trigger"))
            .map(|reference| reference.id.clone()),
    );
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
            scripts: ScriptCensusReport::of(tree),
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
                            "{}: {} references {}, which this pack models no row for",
                            entry.class, entry.referencer, entry.target
                        )
                    })
                    .collect(),
                locale_gaps: references
                    .locale_gaps
                    .iter()
                    .map(|gap| format!("{} ({}) wants {}", gap.referencer, gap.field, gap.key))
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
