//! Reading the authored YAML: the placement and spawn-table documents the shard
//! used to walk itself before ADR 0029 moved compilation into this crate.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_yaml::Value;

/// Where the authored documents sit under the source root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// The private `data` repo: `<src>/<ruleset>/zones/<zone>/spawns/{placements,tables}`.
    Ruleset,
    /// A flat directory of hand-authored documents, as in `data-schemas/demo`.
    Flat,
}

/// Every authored document the compiler consumes, already split by kind.
#[derive(Debug, Default)]
pub struct SourceTree {
    pub placement_documents: Vec<PlacementDocument>,
    pub tables: Vec<SpawnTableDocument>,
    pub mobs: Vec<MobDocument>,
    pub mob_kinds: Vec<MobKindDocument>,
    pub abilities: Vec<AbilityDocument>,
    pub factions: Vec<FactionDocument>,
}

/// The localization keys a document carries. Only `name` reaches a pack row;
/// the rest are read by the client from the locale tables (ADR 0007).
#[derive(Debug, Default, Deserialize)]
pub struct LocRef {
    #[serde(default)]
    pub name: Option<String>,
}

/// One creature record: the combat inputs of `mechanics/combat.md` section 4.
#[derive(Debug, Deserialize)]
pub struct MobDocument {
    pub id: String,
    pub zone: String,
    #[serde(default)]
    pub loc_ref: LocRef,
    #[serde(default)]
    pub mob_kind: Option<ObjectRef>,
    #[serde(default)]
    pub faction: Option<ObjectRef>,
    #[serde(default)]
    pub loot_table: Option<ObjectRef>,
    #[serde(default)]
    pub level_min: Option<u32>,
    #[serde(default)]
    pub level_max: Option<u32>,
    #[serde(default)]
    pub walk_speed: Option<f32>,
    #[serde(default)]
    pub aggro_radius_m: Option<f32>,
    #[serde(default)]
    pub leash_radius_m: Option<f32>,
    #[serde(default)]
    pub abilities: Vec<ObjectRef>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
}

/// A `MobKind`, `MobClass` or `MobQuality` document. Only `hp_mod` is read:
/// `mechanics/combat.md` section 7.1 defers the rest of the multiplier chain.
#[derive(Debug, Deserialize)]
pub struct MobKindDocument {
    pub id: String,
    #[serde(default)]
    pub hp_mod: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct AbilityDocument {
    pub id: String,
    #[serde(default)]
    pub loc_ref: LocRef,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub range_m: Option<f32>,
    #[serde(default)]
    pub cast_time_ms: Option<u32>,
    #[serde(default)]
    pub cooldown_ms: Option<u32>,
    #[serde(default)]
    pub triggers_gcd: Option<bool>,
    #[serde(default)]
    pub effects: Vec<AbilityEffectDocument>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct AbilityEffectDocument {
    pub kind: String,
    #[serde(default)]
    pub element: Option<String>,
    #[serde(default)]
    pub amount: Option<f32>,
    #[serde(default)]
    pub attack_power_coeff: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct FactionDocument {
    pub id: String,
    #[serde(default)]
    pub loc_ref: LocRef,
    #[serde(default)]
    pub player_faction: bool,
    #[serde(default)]
    pub attackable: bool,
    #[serde(default)]
    pub default_stance: Option<String>,
    #[serde(default)]
    pub relations: Vec<FactionRelationDocument>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct FactionRelationDocument {
    pub faction: String,
    pub stance: String,
}

/// The respawn window an authored placement carries, in milliseconds.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct RespawnWindow {
    #[serde(default)]
    pub min: u32,
    #[serde(default)]
    pub max: u32,
}

#[derive(Debug, Deserialize)]
pub struct PlacementDocument {
    pub id: String,
    pub zone: String,
    #[serde(default)]
    pub placements: Vec<SourcePlacement>,
}

#[derive(Debug, Deserialize)]
pub struct SourcePlacement {
    pub id: String,
    pub object: ObjectRef,
    #[serde(default)]
    pub position: Option<Position>,
    #[serde(default)]
    pub orientation: Option<Orientation>,
    #[serde(default)]
    pub spawn_time: Option<String>,
    #[serde(default)]
    pub script_id: Option<String>,
    #[serde(default)]
    pub route: Option<String>,
    #[serde(default)]
    pub scan_radius: Option<f32>,
    #[serde(default)]
    pub respawn_delay_ms: Option<RespawnWindow>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct SpawnTableDocument {
    pub id: String,
    pub zone: String,
    #[serde(default)]
    pub scripted: bool,
    #[serde(default)]
    pub entries: Vec<SourceTableEntry>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct SourceTableEntry {
    pub object: ObjectRef,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub chance: Option<f32>,
    #[serde(default)]
    pub spawn_time: Option<String>,
}

/// A reference to another authored document.
///
/// The `href` sibling in the authored YAML is a verbatim MY.GAMES resource path
/// and is deliberately not represented here: it must never reach a compiled pack
/// (ADR 0011).
#[derive(Debug, Deserialize)]
pub struct ObjectRef {
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct Position {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct Orientation {
    #[serde(default)]
    pub yaw: f32,
}

/// Loads every document the compiler understands from `root`, then from each
/// directory in `overlays` in order.
///
/// `zone` is required for [`Layout::Ruleset`], which addresses one zone
/// directory; [`Layout::Flat`] reads whatever documents the directory holds.
///
/// An overlay is always a flat directory of extra documents. It adds; it does
/// not patch. Two documents with the same id are rejected wherever they came
/// from, because a silent last-one-wins would make the compiled pack depend on
/// argument order in a way nothing checks.
pub fn load(
    root: &Path,
    overlays: &[PathBuf],
    layout: Layout,
    ruleset: &str,
    zone: Option<&str>,
) -> Result<SourceTree> {
    let mut files = match layout {
        Layout::Ruleset => {
            let zone = zone.ok_or_else(|| {
                anyhow!("a zone slug is required when reading a ruleset source tree")
            })?;
            let spawns = root.join(ruleset).join("zones").join(zone).join("spawns");
            let mut files = yaml_files(&spawns.join("placements"))?;
            files.extend(yaml_files(&spawns.join("tables"))?);
            // Mob records are per-zone in the authored tree. The directory is
            // optional because a zone can be all placements and tables, and a
            // pack with no mob rows is a legal, if unplayable, pack. Global
            // resources — abilities, factions — have no home in the ruleset
            // tree yet and arrive through `--overlay` until the extractor
            // writes them.
            files.extend(optional_yaml_files(&spawns.join("mobs"))?);
            files
        }
        Layout::Flat => yaml_files(root)?,
    };
    for overlay in overlays {
        files.extend(yaml_files(overlay)?);
    }

    let mut tree = SourceTree::default();
    for path in files {
        let text = fs::read_to_string(&path)
            .with_context(|| format!("read authored document {}", path.display()))?;
        let document: Value = serde_yaml::from_str(&text)
            .with_context(|| format!("parse authored document {}", path.display()))?;
        read_document(&mut tree, document, &path)?;
    }

    if tree.placement_documents.is_empty() {
        bail!("{} holds no placement documents", root.display());
    }
    Ok(tree)
}

/// Files a document into the tree.
///
/// Spawn documents declare their own `kind`. Abilities and factions are global
/// resources with no `kind` field, so they are recognised by the first segment
/// of their canonical id, which ADR 0007 makes the document's type tag.
fn read_document(tree: &mut SourceTree, document: Value, path: &Path) -> Result<()> {
    let kind = document.get("kind").and_then(Value::as_str).unwrap_or("");
    let id = document.get("id").and_then(Value::as_str).unwrap_or("");
    let prefix = id.split('.').next().unwrap_or("");
    let what = if kind.is_empty() { prefix } else { kind };
    let context = |label: &str| format!("read {label} from {}", path.display());
    match what {
        "placements" => tree
            .placement_documents
            .push(serde_yaml::from_value(document).with_context(|| context("placements"))?),
        "table" => tree
            .tables
            .push(serde_yaml::from_value(document).with_context(|| context("spawn table"))?),
        "mob" => tree
            .mobs
            .push(serde_yaml::from_value(document).with_context(|| context("mob"))?),
        // The creature taxonomy is three record types in one schema. Only the
        // ones that can carry `hp_mod` are read, and only for that field.
        "mobkind" | "mobquality" => tree
            .mob_kinds
            .push(serde_yaml::from_value(document).with_context(|| context("mob kind"))?),
        "ability" => tree
            .abilities
            .push(serde_yaml::from_value(document).with_context(|| context("ability"))?),
        "faction" => tree
            .factions
            .push(serde_yaml::from_value(document).with_context(|| context("faction"))?),
        // Items, quests, routes, locales and chargen options carry no runtime
        // rows here. A pack that needs them gets a table of its own rather
        // than a guess in this match.
        _ => {}
    }
    Ok(())
}

/// Like [`yaml_files`], but an absent directory yields nothing instead of an
/// error.
fn optional_yaml_files(directory: &Path) -> Result<Vec<PathBuf>> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    yaml_files(directory)
}

/// Lists `*.yaml` files directly inside `directory`, sorted by file name so that
/// two runs on the same tree see the same order.
fn yaml_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(directory)
        .with_context(|| format!("read source directory {}", directory.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("list {}", directory.display()))?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("yaml"))
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Converts an authored YAML value into the JSON text stored in a `keep_extra`
/// pack. Mapping keys are stringified because protobuf maps are keyed by string.
pub fn json_text(value: &Value) -> Result<String> {
    serde_json::to_string(&to_json(value)?).context("encode extra value as JSON")
}

fn to_json(value: &Value) -> Result<serde_json::Value> {
    Ok(match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(inner) => serde_json::Value::Bool(*inner),
        Value::Number(inner) => {
            if let Some(signed) = inner.as_i64() {
                serde_json::Value::from(signed)
            } else if let Some(float) = inner.as_f64() {
                serde_json::Number::from_f64(float)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            } else {
                serde_json::Value::Null
            }
        }
        Value::String(inner) => serde_json::Value::String(inner.clone()),
        Value::Sequence(items) => {
            serde_json::Value::Array(items.iter().map(to_json).collect::<Result<_>>()?)
        }
        Value::Mapping(entries) => {
            let mut object = serde_json::Map::new();
            for (key, item) in entries {
                object.insert(scalar_key(key)?, to_json(item)?);
            }
            serde_json::Value::Object(object)
        }
        Value::Tagged(tagged) => to_json(&tagged.value)?,
    })
}

fn scalar_key(key: &Value) -> Result<String> {
    Ok(match key {
        Value::String(inner) => inner.clone(),
        Value::Bool(inner) => inner.to_string(),
        Value::Number(inner) => inner.to_string(),
        other => bail!("extra mapping key {other:?} is not a scalar"),
    })
}
