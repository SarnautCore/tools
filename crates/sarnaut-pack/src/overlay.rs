//! ADR 0021's generated-base-plus-curated-overlay model, with the merge
//! semantics ADR 0029 pins down.
//!
//! The base layer is whatever the extractor wrote. An overlay layer is a
//! directory of **patches**: an overlay document is merged into the document
//! that already carries its id rather than replacing it, so re-running an
//! extractor regenerates the base without touching curation.
//!
//! Three properties are load-bearing and are enforced here rather than left to
//! convention:
//!
//! * **Order comes from `layers.yaml` and nowhere else.** Not filesystem order,
//!   not lexicographic id order, not a precedence hint inside a layer. A layer
//!   the manifest does not list does not exist.
//! * **Every overlay document states why it exists.** A patch with no
//!   `curation_note` is indistinguishable from an accident, so it is a compile
//!   error.
//! * **Two layers editing the same leaf is a conflict, not a race.** Silent
//!   last-one-wins would make the pack depend on manifest order in a way
//!   nothing checks.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_yaml::{Mapping, Value};

/// The layer manifest file, always directly inside the overlays root.
pub const LAYERS_FILE_NAME: &str = "layers.yaml";
/// Layer id recorded for documents that came from the extractor's output.
pub const BASE_LAYER: &str = "base";

/// Reserved keys an authored document may carry that describe the patch rather
/// than the content. None of them reaches a compiled row.
const KEY_CURATION_NOTE: &str = "curation_note";
const KEY_OP: &str = "_op";
const KEY_DELETE: &str = "_delete";
const OP_REPLACE: &str = "replace";
const OP_DELETE: &str = "delete";

/// One entry of `layers.yaml`.
#[derive(Debug, Deserialize)]
pub struct LayerEntry {
    pub id: String,
    #[serde(default)]
    pub description: String,
    /// Whether a build that names no `--overlay` applies this layer.
    ///
    /// Defaulting to true keeps the common case — a data repository whose
    /// overlays are the content — free of flags. A layer that exists to be
    /// switched on for one pack, as the demo dataset's combat exhibit does,
    /// sets it false so that adding the layer cannot move an existing pack's
    /// digest.
    #[serde(default = "default_true")]
    pub apply_by_default: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct LayerManifest {
    #[serde(default)]
    layers: Vec<LayerEntry>,
}

/// A layer selected for this build, with the directory its documents live in.
#[derive(Debug)]
pub struct Layer {
    pub id: String,
    pub description: String,
    pub directory: PathBuf,
}

/// Reads `layers.yaml` and selects the layers this build applies.
///
/// `selected` empty means "every layer whose `apply_by_default` is true".
/// Naming layers explicitly selects exactly those, and they still apply in
/// manifest order: the caller chooses the subset, never the ordering.
pub fn select_layers(overlays_root: &Path, selected: &[String]) -> Result<Vec<Layer>> {
    let manifest_path = overlays_root.join(LAYERS_FILE_NAME);
    if !manifest_path.is_file() {
        if !selected.is_empty() {
            bail!(
                "--overlay names {} but {} does not exist, so no layer is defined",
                selected.join(", "),
                manifest_path.display()
            );
        }
        return Ok(Vec::new());
    }

    let text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let manifest: LayerManifest = serde_yaml::from_str(&text)
        .with_context(|| format!("parse {}", manifest_path.display()))?;

    let mut seen = BTreeMap::new();
    for entry in &manifest.layers {
        if !is_layer_id(&entry.id) {
            bail!(
                "{}: layer id {:?} is not a lowercase hyphenated slug",
                manifest_path.display(),
                entry.id
            );
        }
        if seen.insert(entry.id.clone(), ()).is_some() {
            bail!(
                "{}: layer id {} is listed twice; the manifest is the sole authority on order and cannot hold a document twice",
                manifest_path.display(),
                entry.id
            );
        }
    }
    for wanted in selected {
        if !seen.contains_key(wanted) {
            bail!(
                "--overlay {wanted} names no layer in {}; it lists {}",
                manifest_path.display(),
                if seen.is_empty() {
                    "none".to_string()
                } else {
                    seen.keys().cloned().collect::<Vec<_>>().join(", ")
                }
            );
        }
    }

    let mut layers = Vec::new();
    for entry in manifest.layers {
        let wanted = if selected.is_empty() {
            entry.apply_by_default
        } else {
            selected.contains(&entry.id)
        };
        if !wanted {
            continue;
        }
        let directory = overlays_root.join(&entry.id);
        if !directory.is_dir() {
            bail!(
                "{} lists layer {} but {} is not a directory",
                manifest_path.display(),
                entry.id,
                directory.display()
            );
        }
        layers.push(Layer {
            id: entry.id,
            description: entry.description,
            directory,
        });
    }
    Ok(layers)
}

fn is_layer_id(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// One overlay document's stated reason for existing.
#[derive(Clone, Debug)]
pub struct CurationNote {
    pub document_id: String,
    pub layer: String,
    pub source: String,
    pub note: String,
}

/// Two layers writing the same leaf of the same document.
#[derive(Clone, Debug)]
pub struct Conflict {
    pub document_id: String,
    pub path: String,
    pub first_layer: String,
    pub second_layer: String,
}

impl Conflict {
    pub fn describe(&self) -> String {
        format!(
            "document {}: layers {} and {} both write {}",
            self.document_id, self.first_layer, self.second_layer, self.path
        )
    }
}

/// A merged document plus the provenance that produced it.
#[derive(Debug)]
pub struct MergedDocument {
    pub id: String,
    pub value: Value,
    /// Where the document was first seen. A document that only an overlay
    /// creates records that overlay.
    pub origin_layer: String,
    pub origin_path: PathBuf,
    /// True when at least one overlay layer touched this document.
    pub curated: bool,
    /// Which layer last wrote each leaf, for conflict reporting. Base-layer
    /// writes are not recorded: a patch over generated data is the point.
    leaf_layers: BTreeMap<String, String>,
}

/// Every document of a build, keyed by canonical id, after merging.
#[derive(Debug, Default)]
pub struct DocumentSet {
    documents: BTreeMap<String, MergedDocument>,
    pub notes: Vec<CurationNote>,
    pub conflicts: Vec<Conflict>,
    /// Ids an overlay removed with a top-level `_op: delete`.
    pub deleted: Vec<String>,
}

impl DocumentSet {
    pub fn iter(&self) -> impl Iterator<Item = &MergedDocument> {
        self.documents.values()
    }

    pub fn get(&self, id: &str) -> Option<&MergedDocument> {
        self.documents.get(id)
    }

    pub fn len(&self) -> usize {
        self.documents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    /// Files one base-layer document. Two base documents with the same id are
    /// rejected: nothing decides which one wins, and a silent last-one-wins
    /// would make the pack depend on directory listing order.
    pub fn insert_base(&mut self, path: &Path, value: Value) -> Result<()> {
        let id = document_id(&value, path)?;
        if let Some(existing) = self.documents.get(&id) {
            bail!(
                "document id {id} is declared twice: {} and {}",
                existing.origin_path.display(),
                path.display()
            );
        }
        if let Some(note) = note_of(&value) {
            self.notes.push(CurationNote {
                document_id: id.clone(),
                layer: BASE_LAYER.to_string(),
                source: path.display().to_string(),
                note,
            });
        }
        self.documents.insert(
            id.clone(),
            MergedDocument {
                id,
                value,
                origin_layer: BASE_LAYER.to_string(),
                origin_path: path.to_path_buf(),
                curated: false,
                leaf_layers: BTreeMap::new(),
            },
        );
        Ok(())
    }

    /// Applies one overlay document as a patch over whatever the merged result
    /// holds for its id so far.
    pub fn apply_overlay(&mut self, layer: &str, path: &Path, patch: Value) -> Result<()> {
        let id = document_id(&patch, path)?;
        let note = note_of(&patch).ok_or_else(|| {
            anyhow!(
                "overlay layer {layer}: {}: document {id} carries no curation_note. ADR 0029 requires every overlay document to state why it exists",
                path.display()
            )
        })?;
        self.notes.push(CurationNote {
            document_id: id.clone(),
            layer: layer.to_string(),
            source: path.display().to_string(),
            note,
        });

        if operation(&patch) == Some(OP_DELETE) {
            if self.documents.remove(&id).is_none() {
                bail!(
                    "overlay layer {layer}: {}: `_op: delete` names document {id}, which no layer supplies",
                    path.display()
                );
            }
            self.deleted.push(id);
            return Ok(());
        }

        let deletions = deletion_paths(&patch, path)?;
        let Some(target) = self.documents.get_mut(&id) else {
            // An overlay that names an id no base layer carries is an
            // addition, which ADR 0021 explicitly allows: curation adds as
            // well as fixes.
            let mut created = MergedDocument {
                id: id.clone(),
                value: Value::Mapping(Mapping::new()),
                origin_layer: layer.to_string(),
                origin_path: path.to_path_buf(),
                curated: true,
                leaf_layers: BTreeMap::new(),
            };
            merge_into(
                &mut created.value,
                &patch,
                "",
                layer,
                &mut created.leaf_layers,
                &mut self.conflicts,
                &id,
            );
            apply_deletions(&mut created.value, &deletions);
            self.documents.insert(id, created);
            return Ok(());
        };

        target.curated = true;
        merge_into(
            &mut target.value,
            &patch,
            "",
            layer,
            &mut target.leaf_layers,
            &mut self.conflicts,
            &id,
        );
        apply_deletions(&mut target.value, &deletions);
        Ok(())
    }
}

fn document_id(value: &Value, path: &Path) -> Result<String> {
    value
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{}: document has no top-level string id", path.display()))
}

fn note_of(value: &Value) -> Option<String> {
    value
        .get(KEY_CURATION_NOTE)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|note| !note.is_empty())
        .map(str::to_string)
}

fn operation(value: &Value) -> Option<&str> {
    value.get(KEY_OP).and_then(Value::as_str)
}

fn deletion_paths(patch: &Value, path: &Path) -> Result<Vec<String>> {
    let Some(node) = patch.get(KEY_DELETE) else {
        return Ok(Vec::new());
    };
    let Some(items) = node.as_sequence() else {
        bail!(
            "{}: `_delete` must be a list of dotted paths",
            path.display()
        );
    };
    items
        .iter()
        .map(|item| {
            item.as_str().map(str::to_string).ok_or_else(|| {
                anyhow!(
                    "{}: `_delete` entry {item:?} is not a dotted path string",
                    path.display()
                )
            })
        })
        .collect()
}

/// Merges `patch` into `base`, recording which layer wrote each leaf.
///
/// Scalars replace. Mappings merge key by key. **Sequences replace wholesale**:
/// there is no stable identity to key list elements on, and a half-merged list
/// is worse than an explicit rewrite.
fn merge_into(
    base: &mut Value,
    patch: &Value,
    path: &str,
    layer: &str,
    leaf_layers: &mut BTreeMap<String, String>,
    conflicts: &mut Vec<Conflict>,
    document_id: &str,
) {
    let replace_wholesale = operation(patch) == Some(OP_REPLACE);
    match patch {
        Value::Mapping(entries) if !replace_wholesale => {
            if !base.is_mapping() {
                *base = Value::Mapping(Mapping::new());
            }
            let Some(target) = base.as_mapping_mut() else {
                return;
            };
            for (key, value) in entries {
                let Some(name) = key.as_str() else {
                    continue;
                };
                if is_reserved(name, path) {
                    continue;
                }
                let child_path = join_path(path, name);
                let slot = target
                    .entry(Value::String(name.to_string()))
                    .or_insert(Value::Null);
                merge_into(
                    slot,
                    value,
                    &child_path,
                    layer,
                    leaf_layers,
                    conflicts,
                    document_id,
                );
            }
        }
        _ => {
            let replacement = strip_reserved(patch);
            // Writing the value that is already there is not a write. Every
            // patch repeats the document's `id` and `schema_version` by
            // necessity, and reporting those as conflicts would bury the one
            // leaf two layers genuinely disagree about.
            let changed = *base != replacement;
            *base = replacement;
            if changed {
                record_leaf(path, layer, leaf_layers, conflicts, document_id);
            }
        }
    }
}

/// `curation_note` and `_delete` are patch metadata and only mean anything at
/// the top level; `_op` is consumed by the merge at every level.
fn is_reserved(name: &str, path: &str) -> bool {
    name == KEY_OP || (path.is_empty() && (name == KEY_CURATION_NOTE || name == KEY_DELETE))
}

fn join_path(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

fn record_leaf(
    path: &str,
    layer: &str,
    leaf_layers: &mut BTreeMap<String, String>,
    conflicts: &mut Vec<Conflict>,
    document_id: &str,
) {
    for (existing, owner) in leaf_layers.iter() {
        if owner == layer {
            continue;
        }
        let overlapping = existing == path
            || existing.starts_with(&format!("{path}."))
            || path.starts_with(&format!("{existing}."));
        if overlapping {
            conflicts.push(Conflict {
                document_id: document_id.to_string(),
                path: path.to_string(),
                first_layer: owner.clone(),
                second_layer: layer.to_string(),
            });
        }
    }
    leaf_layers.insert(path.to_string(), layer.to_string());
}

/// Copies a patch value with every reserved key removed, so that `_op` never
/// reaches a compiled row.
fn strip_reserved(value: &Value) -> Value {
    match value {
        Value::Mapping(entries) => {
            let mut copy = Mapping::new();
            for (key, item) in entries {
                if key.as_str() == Some(KEY_OP) {
                    continue;
                }
                copy.insert(key.clone(), strip_reserved(item));
            }
            Value::Mapping(copy)
        }
        Value::Sequence(items) => Value::Sequence(items.iter().map(strip_reserved).collect()),
        other => other.clone(),
    }
}

/// Applies a document's top-level `_delete` list after the merge, so a patch
/// can remove an inherited key rather than only overwrite it.
fn apply_deletions(value: &mut Value, paths: &[String]) {
    for dotted in paths {
        let mut segments: Vec<&str> = dotted.split('.').collect();
        let Some(last) = segments.pop() else {
            continue;
        };
        if let Some(parent) = walk_to(value, &segments)
            && let Some(mapping) = parent.as_mapping_mut()
        {
            mapping.remove(Value::String(last.to_string()));
        }
    }
}

/// Follows a dotted path to the node holding the final segment, or `None` when
/// the path does not exist in the merged document.
fn walk_to<'a>(value: &'a mut Value, segments: &[&str]) -> Option<&'a mut Value> {
    let mut cursor = value;
    for segment in segments {
        cursor = cursor.get_mut(*segment)?;
    }
    Some(cursor)
}

/// Removes the patch vocabulary from a merged document before it is parsed
/// into a typed row. Notes are aggregated into `build-report.json` instead, so
/// they never reach table bytes and never move `pack_id`.
pub fn strip_patch_keys(value: &mut Value) {
    if let Some(mapping) = value.as_mapping_mut() {
        mapping.remove(Value::String(KEY_CURATION_NOTE.to_string()));
        mapping.remove(Value::String(KEY_DELETE.to_string()));
        mapping.remove(Value::String(KEY_OP.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(text: &str) -> Value {
        serde_yaml::from_str(text).expect("parse test document")
    }

    fn base_set(text: &str) -> DocumentSet {
        let mut set = DocumentSet::default();
        set.insert_base(Path::new("base/doc.yaml"), yaml(text))
            .expect("insert base");
        set
    }

    #[test]
    fn an_overlay_overrides_a_scalar_and_leaves_its_siblings_alone() {
        let mut set = base_set("id: mob.a\nlevel_min: 2\nwalk_speed: 2.0\n");
        set.apply_overlay(
            "curated",
            Path::new("overlays/curated/mob.yaml"),
            yaml("id: mob.a\ncuration_note: faster\nwalk_speed: 4.5\n"),
        )
        .expect("apply overlay");

        let document = set.get("mob.a").expect("document");
        assert_eq!(document.value["walk_speed"].as_f64(), Some(4.5));
        assert_eq!(document.value["level_min"].as_u64(), Some(2));
        assert!(document.curated);
        assert_eq!(set.notes.len(), 1);
        assert_eq!(set.notes[0].note, "faster");
    }

    #[test]
    fn an_overlay_adds_a_document_no_base_layer_carries() {
        let mut set = base_set("id: mob.a\n");
        set.apply_overlay(
            "curated",
            Path::new("overlays/curated/quest.yaml"),
            yaml("id: quest.new\ncuration_note: invented\nlevel: 1\n"),
        )
        .expect("apply overlay");

        let document = set.get("quest.new").expect("document");
        assert_eq!(document.origin_layer, "curated");
        assert_eq!(document.value["level"].as_u64(), Some(1));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn a_mapping_merges_key_by_key_and_a_sequence_replaces_wholesale() {
        let mut set = base_set(
            "id: mob.a\nrewards:\n  experience: 2\n  money: 5\nabilities: [one, two, three]\n",
        );
        set.apply_overlay(
            "curated",
            Path::new("overlays/curated/mob.yaml"),
            yaml("id: mob.a\ncuration_note: rebalance\nrewards:\n  money: 9\nabilities: [only]\n"),
        )
        .expect("apply overlay");

        let document = set.get("mob.a").expect("document");
        assert_eq!(document.value["rewards"]["experience"].as_u64(), Some(2));
        assert_eq!(document.value["rewards"]["money"].as_u64(), Some(9));
        assert_eq!(
            document.value["abilities"].as_sequence().map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn op_replace_discards_the_merged_value_beneath_it() {
        let mut set = base_set("id: mob.a\nrewards:\n  experience: 2\n  money: 5\n");
        set.apply_overlay(
            "curated",
            Path::new("overlays/curated/mob.yaml"),
            yaml("id: mob.a\ncuration_note: reset\nrewards:\n  _op: replace\n  money: 9\n"),
        )
        .expect("apply overlay");

        let document = set.get("mob.a").expect("document");
        assert!(document.value["rewards"].get("experience").is_none());
        assert_eq!(document.value["rewards"]["money"].as_u64(), Some(9));
        assert!(document.value["rewards"].get("_op").is_none());
    }

    #[test]
    fn delete_removes_a_path_after_the_merge() {
        let mut set = base_set("id: mob.a\nrewards:\n  experience: 2\n  money: 5\n");
        set.apply_overlay(
            "curated",
            Path::new("overlays/curated/mob.yaml"),
            yaml("id: mob.a\ncuration_note: drop money\n_delete: [rewards.money]\n"),
        )
        .expect("apply overlay");

        let document = set.get("mob.a").expect("document");
        assert!(document.value["rewards"].get("money").is_none());
        assert_eq!(document.value["rewards"]["experience"].as_u64(), Some(2));
    }

    #[test]
    fn op_delete_removes_the_document() {
        let mut set = base_set("id: mob.a\n");
        set.apply_overlay(
            "curated",
            Path::new("overlays/curated/mob.yaml"),
            yaml("id: mob.a\ncuration_note: retired\n_op: delete\n"),
        )
        .expect("apply overlay");

        assert!(set.get("mob.a").is_none());
        assert_eq!(set.deleted, vec!["mob.a".to_string()]);
    }

    #[test]
    fn an_overlay_document_without_a_curation_note_is_refused() {
        let mut set = base_set("id: mob.a\n");
        let error = set
            .apply_overlay(
                "curated",
                Path::new("overlays/curated/mob.yaml"),
                yaml("id: mob.a\nwalk_speed: 4.0\n"),
            )
            .expect_err("an overlay without a note must fail");
        let message = format!("{error:#}");
        assert!(message.contains("curation_note"), "{message}");
        assert!(message.contains("mob.a"), "{message}");
        assert!(message.contains("curated"), "{message}");
    }

    #[test]
    fn an_empty_curation_note_is_refused() {
        let mut set = base_set("id: mob.a\n");
        let error = set
            .apply_overlay(
                "curated",
                Path::new("overlays/curated/mob.yaml"),
                yaml("id: mob.a\ncuration_note: \"   \"\n"),
            )
            .expect_err("a blank note must fail");
        assert!(format!("{error:#}").contains("curation_note"));
    }

    #[test]
    fn two_base_documents_with_one_id_are_refused() {
        let mut set = base_set("id: mob.a\n");
        let error = set
            .insert_base(Path::new("base/other.yaml"), yaml("id: mob.a\n"))
            .expect_err("a duplicate base id must fail");
        let message = format!("{error:#}");
        assert!(message.contains("declared twice"), "{message}");
        assert!(message.contains("mob.a"), "{message}");
    }

    #[test]
    fn two_layers_writing_one_leaf_is_a_conflict_naming_both() {
        let mut set = base_set("id: mob.a\nwalk_speed: 2.0\n");
        set.apply_overlay(
            "first",
            Path::new("overlays/first/mob.yaml"),
            yaml("id: mob.a\ncuration_note: one\nwalk_speed: 3.0\n"),
        )
        .expect("first layer");
        set.apply_overlay(
            "second",
            Path::new("overlays/second/mob.yaml"),
            yaml("id: mob.a\ncuration_note: two\nwalk_speed: 4.0\n"),
        )
        .expect("second layer");

        assert_eq!(set.conflicts.len(), 1);
        let described = set.conflicts[0].describe();
        assert!(described.contains("first"), "{described}");
        assert!(described.contains("second"), "{described}");
        assert!(described.contains("walk_speed"), "{described}");
        // Later layer wins, which is what `--allow-overlay-conflicts` accepts.
        assert_eq!(
            set.get("mob.a").expect("document").value["walk_speed"].as_f64(),
            Some(4.0)
        );
    }

    #[test]
    fn one_layer_rewriting_its_own_leaf_twice_is_not_a_conflict() {
        let mut set = base_set("id: mob.a\nrewards:\n  money: 1\n");
        set.apply_overlay(
            "only",
            Path::new("overlays/only/a.yaml"),
            yaml("id: mob.a\ncuration_note: one\nrewards:\n  money: 2\n"),
        )
        .expect("first document");
        set.apply_overlay(
            "only",
            Path::new("overlays/only/b.yaml"),
            yaml("id: mob.a\ncuration_note: two\nrewards:\n  money: 3\n"),
        )
        .expect("second document");
        assert!(set.conflicts.is_empty());
    }

    #[test]
    fn a_layer_touching_a_subtree_another_layer_wrote_inside_is_a_conflict() {
        let mut set = base_set("id: mob.a\nrewards:\n  money: 1\n");
        set.apply_overlay(
            "first",
            Path::new("overlays/first/mob.yaml"),
            yaml("id: mob.a\ncuration_note: one\nrewards:\n  money: 2\n"),
        )
        .expect("first layer");
        set.apply_overlay(
            "second",
            Path::new("overlays/second/mob.yaml"),
            yaml("id: mob.a\ncuration_note: two\nrewards: {_op: replace, honor: 4}\n"),
        )
        .expect("second layer");
        assert_eq!(set.conflicts.len(), 1);
        assert_eq!(set.conflicts[0].path, "rewards");
    }
}
