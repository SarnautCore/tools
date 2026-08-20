//! Turning an authored source tree into a pack directory.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use prost::Message;

use crate::manifest::{self, Manifest};
use crate::proto;
use crate::source::{self, Layout, SourceTree};
use crate::table::{self, Row};

pub const TABLE_ZONE: &str = "zone";
pub const TABLE_PLACEMENTS: &str = "placements";
pub const TABLE_SPAWN_TABLES: &str = "spawn-tables";

/// A spawn schedule of `time-never` marks an authored object as inert. The
/// shard still sees the row; the resolver skips it.
pub const SPAWN_TIME_NEVER: &str = "time-never";

/// Where an entering player is placed, when the caller pins it explicitly.
#[derive(Clone, Copy, Debug)]
pub struct PlayerSpawn {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
}

#[derive(Debug)]
pub struct BuildOptions {
    pub source: PathBuf,
    pub out: PathBuf,
    pub layout: Layout,
    pub ruleset: String,
    pub zone: Option<String>,
    pub keep_extra: bool,
    pub player_spawn: Option<PlayerSpawn>,
    pub source_repo: String,
    /// `None` asks the compiler to read the source repository's HEAD.
    pub source_commit: Option<String>,
}

#[derive(Debug)]
pub struct BuildReport {
    pub pack_id: String,
    pub zone: String,
    pub tables: Vec<(String, u32)>,
}

/// Compiles `options.source` into a pack directory at `options.out`.
pub fn build(options: &BuildOptions) -> Result<BuildReport> {
    let tree = source::load(
        &options.source,
        options.layout,
        &options.ruleset,
        options.zone.as_deref(),
    )?;
    let zone_id = zone_id(&tree, options)?;
    let zone_slug = zone_id
        .strip_prefix("zone.")
        .ok_or_else(|| anyhow::anyhow!("zone id {zone_id} does not start with `zone.`"))?
        .to_string();

    let spawn_tables = spawn_table_rows(&tree, options.keep_extra)?;
    let known_tables: BTreeSet<&str> = spawn_tables.iter().map(|row| row.key.as_str()).collect();
    let placements = placement_rows(&tree, &known_tables, options.keep_extra)?;
    let zone_row = zone_row(&zone_id, &zone_slug, options, &tree)?;

    let encoded = vec![
        (
            TABLE_ZONE.to_string(),
            proto::RowType::Zone,
            table::encode(proto::RowType::Zone as i32, &zone_row)?,
            zone_row.len() as u32,
        ),
        (
            TABLE_PLACEMENTS.to_string(),
            proto::RowType::Placement,
            table::encode(proto::RowType::Placement as i32, &placements)?,
            placements.len() as u32,
        ),
        (
            TABLE_SPAWN_TABLES.to_string(),
            proto::RowType::SpawnTable,
            table::encode(proto::RowType::SpawnTable as i32, &spawn_tables)?,
            spawn_tables.len() as u32,
        ),
    ];

    let digest_input: Vec<(String, Vec<u8>)> = encoded
        .iter()
        .map(|(name, _, bytes, _)| (name.clone(), bytes.clone()))
        .collect();
    let pack_id = manifest::pack_id(&digest_input);

    let mut entries: Vec<manifest::TableEntry> = encoded
        .iter()
        .map(|(name, row_type, bytes, rows)| manifest::TableEntry {
            name: name.clone(),
            file: format!("tables/{name}.sptbl"),
            row_type: row_type.as_str_name().to_string(),
            rows: *rows,
            bytes: bytes.len() as u64,
            blake3: blake3::hash(bytes).to_hex().to_string(),
        })
        .collect();
    entries.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));

    let commit = match &options.source_commit {
        Some(commit) => commit.clone(),
        None => {
            head_commit(&options.source).unwrap_or_else(|| manifest::UNKNOWN_COMMIT.to_string())
        }
    };
    let document = Manifest {
        schema_version: manifest::SCHEMA_VERSION,
        ruleset: options.ruleset.clone(),
        zone: zone_slug.clone(),
        pack_id: pack_id.clone(),
        builder: manifest::Builder {
            name: manifest::BUILDER_NAME.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        source: manifest::Source {
            repo: options.source_repo.clone(),
            commit,
            // Overlay layering (ADR 0029) has no authored layers yet; the field
            // is written empty rather than omitted so readers see a stable shape.
            overlays: Vec::new(),
        },
        keep_extra: options.keep_extra,
        tables: entries,
    };

    write_pack(&options.out, &document, &encoded)?;
    Ok(BuildReport {
        pack_id,
        zone: zone_slug,
        tables: encoded
            .into_iter()
            .map(|(name, _, _, rows)| (name, rows))
            .collect(),
    })
}

type EncodedTable = (String, proto::RowType, Vec<u8>, u32);

fn write_pack(out: &Path, document: &Manifest, encoded: &[EncodedTable]) -> Result<()> {
    let tables = out.join("tables");
    if tables.exists() {
        fs::remove_dir_all(&tables)
            .with_context(|| format!("clear {} before writing", tables.display()))?;
    }
    fs::create_dir_all(&tables)
        .with_context(|| format!("create pack directory {}", tables.display()))?;
    for (name, _, bytes, _) in encoded {
        let path = tables.join(format!("{name}.sptbl"));
        fs::write(&path, bytes).with_context(|| format!("write table {}", path.display()))?;
    }
    let path = out.join(manifest::FILE_NAME);
    fs::write(&path, document.to_canonical_json()?)
        .with_context(|| format!("write {}", path.display()))
}

fn zone_id(tree: &SourceTree, options: &BuildOptions) -> Result<String> {
    let mut zones: BTreeSet<&str> = BTreeSet::new();
    for document in &tree.placement_documents {
        zones.insert(document.zone.as_str());
    }
    for document in &tree.tables {
        zones.insert(document.zone.as_str());
    }
    if zones.len() > 1 {
        bail!(
            "source tree mixes zones {:?}; a pack covers exactly one zone",
            zones
        );
    }
    match (zones.iter().next(), options.zone.as_deref()) {
        (Some(found), Some(wanted)) if *found != format!("zone.{wanted}") => {
            bail!("source documents declare {found} but the requested zone is zone.{wanted}")
        }
        (Some(found), _) => Ok((*found).to_string()),
        (None, Some(wanted)) => Ok(format!("zone.{wanted}")),
        (None, None) => bail!("source tree declares no zone and none was requested"),
    }
}

fn spawn_table_rows(tree: &SourceTree, keep_extra: bool) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::SpawnTable> = BTreeMap::new();
    for document in &tree.tables {
        let message = proto::SpawnTable {
            id: document.id.clone(),
            entries: document
                .entries
                .iter()
                .map(|entry| {
                    Ok(proto::SpawnTableEntry {
                        object_id: entry.object.id.clone().ok_or_else(|| {
                            anyhow::anyhow!(
                                "spawn table {} has an entry with no object id",
                                document.id
                            )
                        })?,
                        group: entry.group.clone().unwrap_or_default(),
                        chance: entry.chance.unwrap_or_default(),
                        spawn_time: entry.spawn_time.clone().unwrap_or_default(),
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            scripted: document.scripted,
            extra: extra_map(&document.extra, keep_extra)?,
        };
        if seen.insert(document.id.clone(), message).is_some() {
            bail!("spawn table id {} is declared twice", document.id);
        }
    }
    Ok(encode_rows(seen))
}

fn placement_rows(
    tree: &SourceTree,
    known_tables: &BTreeSet<&str>,
    keep_extra: bool,
) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::Placement> = BTreeMap::new();
    for document in &tree.placement_documents {
        for placed in &document.placements {
            let object_id = placed
                .object
                .id
                .clone()
                .ok_or_else(|| anyhow::anyhow!("placement {} has no object id", placed.id))?;
            // ADR 0006's cross-reference check: a placement either names a mob
            // directly or names a spawn table that this pack contains.
            if !object_id.starts_with("mob.") && !known_tables.contains(object_id.as_str()) {
                bail!(
                    "placement {} references {object_id}, which is neither a mob nor a spawn table in this pack",
                    placed.id
                );
            }
            let position = placed.position.unwrap_or_default();
            let message = proto::Placement {
                id: placed.id.clone(),
                object_id,
                position: Some(proto::Vec3 {
                    x: position.x,
                    y: position.y,
                    z: position.z,
                }),
                heading: placed.orientation.unwrap_or_default().yaw,
                spawn_time: placed.spawn_time.clone().unwrap_or_default(),
                script_id: placed.script_id.clone().unwrap_or_default(),
                route_id: placed.route.clone().unwrap_or_default(),
                scan_radius: placed.scan_radius.unwrap_or_default(),
                extra: extra_map(&placed.extra, keep_extra)?,
            };
            if seen.insert(placed.id.clone(), message).is_some() {
                bail!("placement id {} is declared twice", placed.id);
            }
        }
    }
    if seen.is_empty() {
        bail!("source tree contains no placements");
    }
    Ok(encode_rows(seen))
}

fn zone_row(
    zone_id: &str,
    zone_slug: &str,
    options: &BuildOptions,
    tree: &SourceTree,
) -> Result<Vec<Row>> {
    let spawn = match options.player_spawn {
        Some(spawn) => spawn,
        None => default_player_spawn(tree)?,
    };
    let message = proto::Zone {
        id: zone_id.to_string(),
        ruleset: options.ruleset.clone(),
        slug: zone_slug.to_string(),
        player_spawn: Some(proto::Vec3 {
            x: spawn.x,
            y: spawn.y,
            z: spawn.z,
        }),
        player_spawn_heading: spawn.yaw,
        extra: BTreeMap::new(),
    };
    Ok(encode_rows(BTreeMap::from([(
        zone_id.to_string(),
        message,
    )])))
}

/// Until the extractor maps an authored start point, the player spawn defaults
/// to the first live placement in canonical-id order. It is deterministic and
/// inside the playable space, which is what the shard needs; a real start point
/// arrives with `--player-spawn` or a future `zone` document.
fn default_player_spawn(tree: &SourceTree) -> Result<PlayerSpawn> {
    let mut live: Vec<&source::SourcePlacement> = tree
        .placement_documents
        .iter()
        .flat_map(|document| document.placements.iter())
        .filter(|placed| {
            placed.position.is_some()
                && !placed
                    .spawn_time
                    .as_deref()
                    .unwrap_or_default()
                    .eq_ignore_ascii_case(SPAWN_TIME_NEVER)
        })
        .collect();
    live.sort_by(|left, right| left.id.as_bytes().cmp(right.id.as_bytes()));

    let first = live.first().ok_or_else(|| {
        anyhow::anyhow!(
            "no live placement carries a position, so the player spawn cannot be derived; pass --player-spawn"
        )
    })?;
    let position = first.position.unwrap_or_default();
    Ok(PlayerSpawn {
        x: position.x,
        y: position.y,
        z: position.z,
        yaw: 0.0,
    })
}

fn encode_rows<M: Message>(rows: BTreeMap<String, M>) -> Vec<Row> {
    rows.into_iter()
        .map(|(key, message)| Row {
            key,
            bytes: message.encode_to_vec(),
        })
        .collect()
}

/// Strips the untyped `extra:` passthrough unless the caller asked to keep it.
/// Its keys are verbatim MY.GAMES type and attribute names (ADR 0011).
fn extra_map(
    extra: &BTreeMap<String, serde_yaml::Value>,
    keep_extra: bool,
) -> Result<BTreeMap<String, String>> {
    if !keep_extra {
        return Ok(BTreeMap::new());
    }
    extra
        .iter()
        .map(|(key, value)| Ok((key.clone(), source::json_text(value)?)))
        .collect()
}

/// Reads the checked-out commit of the repository that contains `start`, so the
/// manifest records which revision of the source produced the pack.
fn head_commit(start: &Path) -> Option<String> {
    let mut directory = fs::canonicalize(start).ok()?;
    loop {
        let candidate = directory.join(".git");
        if candidate.is_dir() {
            return resolve_head(&candidate);
        }
        if candidate.is_file() {
            let pointer = fs::read_to_string(&candidate).ok()?;
            let path = pointer.strip_prefix("gitdir:")?.trim();
            return resolve_head(&directory.join(path));
        }
        if !directory.pop() {
            return None;
        }
    }
}

fn resolve_head(git_dir: &Path) -> Option<String> {
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    let commit = match head.strip_prefix("ref:") {
        Some(reference) => {
            let reference = reference.trim();
            match fs::read_to_string(git_dir.join(reference)) {
                Ok(hash) => hash.trim().to_string(),
                Err(_) => packed_ref(git_dir, reference)?,
            }
        }
        None => head.to_string(),
    };
    (commit.len() == 40 && commit.chars().all(|c| c.is_ascii_hexdigit())).then_some(commit)
}

fn packed_ref(git_dir: &Path, reference: &str) -> Option<String> {
    let packed = fs::read_to_string(git_dir.join("packed-refs")).ok()?;
    packed.lines().find_map(|line| {
        let (hash, name) = line.split_once(' ')?;
        (name == reference).then(|| hash.to_string())
    })
}
