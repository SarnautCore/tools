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

/// Loads every document the compiler understands from `root`.
///
/// `zone` is required for [`Layout::Ruleset`], which addresses one zone
/// directory; [`Layout::Flat`] reads whatever documents the directory holds.
pub fn load(root: &Path, layout: Layout, ruleset: &str, zone: Option<&str>) -> Result<SourceTree> {
    let files = match layout {
        Layout::Ruleset => {
            let zone = zone.ok_or_else(|| {
                anyhow!("a zone slug is required when reading a ruleset source tree")
            })?;
            let spawns = root.join(ruleset).join("zones").join(zone).join("spawns");
            let mut files = yaml_files(&spawns.join("placements"))?;
            files.extend(yaml_files(&spawns.join("tables"))?);
            files
        }
        Layout::Flat => yaml_files(root)?,
    };

    let mut tree = SourceTree::default();
    for path in files {
        let text = fs::read_to_string(&path)
            .with_context(|| format!("read authored document {}", path.display()))?;
        let document: Value = serde_yaml::from_str(&text)
            .with_context(|| format!("parse authored document {}", path.display()))?;
        let kind = document.get("kind").and_then(Value::as_str).unwrap_or("");
        match kind {
            "placements" => tree.placement_documents.push(
                serde_yaml::from_value(document)
                    .with_context(|| format!("read placements from {}", path.display()))?,
            ),
            "table" => tree.tables.push(
                serde_yaml::from_value(document)
                    .with_context(|| format!("read spawn table from {}", path.display()))?,
            ),
            // `mob`, items, quests and routes carry no runtime rows yet. A pack
            // that needs them gets a table of its own rather than a guess here.
            _ => continue,
        }
    }

    if tree.placement_documents.is_empty() {
        bail!("{} holds no placement documents", root.display());
    }
    Ok(tree)
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
