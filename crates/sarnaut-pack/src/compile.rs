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
pub const TABLE_ABILITIES: &str = "abilities";
pub const TABLE_FACTIONS: &str = "factions";
pub const TABLE_MOBS: &str = "mobs";
pub const TABLE_CHARGEN: &str = "chargen";
pub const TABLE_ITEMS: &str = "items";
pub const TABLE_LOOT_TABLES: &str = "loot-tables";

/// Maximum container depth of a loot tree, `mechanics/loot.md` section 3's
/// `MAX_TREE_DEPTH`. The deepest tree in reference data is 2; this is headroom,
/// and rejecting past it at compile time means the shard never has to defend a
/// recursive walk against authored content.
const MAX_LOOT_TREE_DEPTH: u32 = 8;

/// A mob record that names no MobKind, or one whose MobKind carries no
/// `hp_mod`, is written with this multiplier. It matches `mechanics/combat.md`
/// rule 5.1.1, which defaults the argument rather than treating it as missing.
const DEFAULT_HP_MOD: f32 = 1.0;

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
    /// Extra flat directories layered over `source`, in order. They add
    /// documents; they do not patch the ones already there.
    pub overlays: Vec<PathBuf>,
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
        &options.overlays,
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
    let abilities = ability_rows(&tree, options.keep_extra)?;
    let factions = faction_rows(&tree, options.keep_extra)?;
    let known_abilities: BTreeSet<&str> = abilities.iter().map(|row| row.key.as_str()).collect();
    let known_factions: BTreeSet<&str> = factions.iter().map(|row| row.key.as_str()).collect();
    let mobs = mob_rows(&tree, &known_abilities, &known_factions, options.keep_extra)?;

    let chargen = chargen_rows(&tree, options.keep_extra)?;
    let items = item_rows(&tree, options.keep_extra)?;
    let known_items: BTreeSet<&str> = items.iter().map(|row| row.key.as_str()).collect();
    let loot_tables = loot_table_rows(&tree, &known_items, options.keep_extra)?;

    // Every gameplay table below is written even when it has no rows, so a
    // reader can insist on the full set rather than treating "absent" and
    // "empty" as one case. Chargen is the exception, appended after the vec
    // only when the source tree authors one; see the comment there.
    let mut encoded = vec![
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
        (
            TABLE_ABILITIES.to_string(),
            proto::RowType::Ability,
            table::encode(proto::RowType::Ability as i32, &abilities)?,
            abilities.len() as u32,
        ),
        (
            TABLE_FACTIONS.to_string(),
            proto::RowType::Faction,
            table::encode(proto::RowType::Faction as i32, &factions)?,
            factions.len() as u32,
        ),
        (
            TABLE_MOBS.to_string(),
            proto::RowType::Mob,
            table::encode(proto::RowType::Mob as i32, &mobs)?,
            mobs.len() as u32,
        ),
    ];
    // A source tree with no chargen documents produces no chargen table, so a
    // pack built before ADR 0032's document type existed keeps its digest. The
    // item and loot tables follow the same rule for the same reason.
    if !chargen.is_empty() {
        encoded.push((
            TABLE_CHARGEN.to_string(),
            proto::RowType::ChargenOption,
            table::encode(proto::RowType::ChargenOption as i32, &chargen)?,
            chargen.len() as u32,
        ));
    }
    if !items.is_empty() {
        encoded.push((
            TABLE_ITEMS.to_string(),
            proto::RowType::Item,
            table::encode(proto::RowType::Item as i32, &items)?,
            items.len() as u32,
        ));
    }
    if !loot_tables.is_empty() {
        encoded.push((
            TABLE_LOOT_TABLES.to_string(),
            proto::RowType::LootTable,
            table::encode(proto::RowType::LootTable as i32, &loot_tables)?,
            loot_tables.len() as u32,
        ));
    }

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
            // Only the directory name is recorded, never the path it was read
            // from: a manifest that embedded `..\data-schemas\...` would differ
            // between a Windows rebuild and the Linux CI one, and the vendored
            // fixture pack is compared byte for byte.
            overlays: options
                .overlays
                .iter()
                .map(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
                .collect(),
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
            let respawn = placed.respawn_delay_ms.unwrap_or_default();
            if respawn.max < respawn.min {
                bail!(
                    "placement {} declares respawn_delay_ms min {} above max {}",
                    placed.id,
                    respawn.min,
                    respawn.max
                );
            }
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
                respawn_delay_min_ms: respawn.min,
                respawn_delay_max_ms: respawn.max,
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

fn ability_rows(tree: &SourceTree, keep_extra: bool) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::Ability> = BTreeMap::new();
    for document in &tree.abilities {
        let message = proto::Ability {
            id: document.id.clone(),
            target: document.target.clone().unwrap_or_default(),
            range_m: document.range_m.unwrap_or_default(),
            cast_time_ms: document.cast_time_ms.unwrap_or_default(),
            cooldown_ms: document.cooldown_ms.unwrap_or_default(),
            triggers_gcd: document.triggers_gcd.unwrap_or_default(),
            effects: document
                .effects
                .iter()
                .map(|effect| proto::AbilityEffect {
                    kind: effect.kind.clone(),
                    element: effect.element.clone().unwrap_or_default(),
                    amount: effect.amount.unwrap_or_default(),
                    attack_power_coeff: effect.attack_power_coeff.unwrap_or_default(),
                })
                .collect(),
            name_key: document.loc_ref.name.clone().unwrap_or_default(),
            extra: extra_map(&document.extra, keep_extra)?,
        };
        if seen.insert(document.id.clone(), message).is_some() {
            bail!("ability id {} is declared twice", document.id);
        }
    }
    Ok(encode_rows(seen))
}

fn faction_rows(tree: &SourceTree, keep_extra: bool) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::Faction> = BTreeMap::new();
    for document in &tree.factions {
        let message = proto::Faction {
            id: document.id.clone(),
            player_faction: document.player_faction,
            attackable: document.attackable,
            default_stance: document.default_stance.clone().unwrap_or_default(),
            relations: document
                .relations
                .iter()
                .map(|relation| proto::FactionRelation {
                    faction_id: relation.faction.clone(),
                    stance: relation.stance.clone(),
                })
                .collect(),
            name_key: document.loc_ref.name.clone().unwrap_or_default(),
            extra: extra_map(&document.extra, keep_extra)?,
        };
        if seen.insert(document.id.clone(), message).is_some() {
            bail!("faction id {} is declared twice", document.id);
        }
    }
    Ok(encode_rows(seen))
}

/// Builds the mob rows, resolving each mob's `hp_mod` from the MobKind it
/// names. Only the named record's own value is read: the prototype chain and
/// the quality multipliers are deferred by `mechanics/combat.md` section 7.1,
/// and walking half of a chain would produce a number nothing can explain.
fn mob_rows(
    tree: &SourceTree,
    known_abilities: &BTreeSet<&str>,
    known_factions: &BTreeSet<&str>,
    keep_extra: bool,
) -> Result<Vec<Row>> {
    let mut hp_mods: BTreeMap<&str, f32> = BTreeMap::new();
    for kind in &tree.mob_kinds {
        if let Some(value) = kind.hp_mod {
            hp_mods.insert(kind.id.as_str(), value);
        }
    }

    let mut seen: BTreeMap<String, proto::Mob> = BTreeMap::new();
    for document in &tree.mobs {
        let faction_id = document
            .faction
            .as_ref()
            .and_then(|reference| reference.id.clone())
            .unwrap_or_default();
        // ADR 0006's cross-reference check. A mob whose faction is absent has
        // no hostility to evaluate, so combat.md rule 5.2.5 could not run.
        if !faction_id.is_empty() && !known_factions.contains(faction_id.as_str()) {
            bail!(
                "mob {} names faction {faction_id}, which this pack does not carry",
                document.id
            );
        }
        let mob_kind_id = document
            .mob_kind
            .as_ref()
            .and_then(|reference| reference.id.clone())
            .unwrap_or_default();
        let mut ability_ids = Vec::with_capacity(document.abilities.len());
        for reference in &document.abilities {
            let Some(ability_id) = reference.id.clone() else {
                continue;
            };
            if !known_abilities.contains(ability_id.as_str()) {
                bail!(
                    "mob {} names ability {ability_id}, which this pack does not carry",
                    document.id
                );
            }
            ability_ids.push(ability_id);
        }

        let message = proto::Mob {
            id: document.id.clone(),
            name_key: document.loc_ref.name.clone().unwrap_or_default(),
            faction_id,
            hp_mod: hp_mods
                .get(mob_kind_id.as_str())
                .copied()
                .unwrap_or(DEFAULT_HP_MOD),
            mob_kind_id,
            level_min: document.level_min.unwrap_or_default(),
            level_max: document.level_max.unwrap_or_default(),
            walk_speed: document.walk_speed.unwrap_or_default(),
            aggro_radius_m: document.aggro_radius_m.unwrap_or_default(),
            leash_radius_m: document.leash_radius_m.unwrap_or_default(),
            ability_ids,
            loot_table_id: document
                .loot_table
                .as_ref()
                .and_then(|reference| reference.id.clone())
                .unwrap_or_default(),
            extra: extra_map(&document.extra, keep_extra)?,
        };
        if seen.insert(document.id.clone(), message).is_some() {
            bail!("mob id {} is declared twice", document.id);
        }
    }
    Ok(encode_rows(seen))
}

/// Compiles the character-creation options (ADR 0032).
///
/// Options are global rather than zone-scoped: a document names the zone it
/// spawns into. The cross-reference this pack can actually resolve is that the
/// named zone is a canonical `zone.` id; resolving `starting_loadout` item ids,
/// `starting_quests` and `starting_abilities` waits for the tables that hold
/// them, and is deliberately not faked with a prefix check.
fn chargen_rows(tree: &SourceTree, keep_extra: bool) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::ChargenOption> = BTreeMap::new();
    for document in &tree.chargen_options {
        if !document.spawn.zone_id.starts_with("zone.") {
            bail!(
                "chargen option {} spawns into {}, which is not a canonical zone id",
                document.id,
                document.spawn.zone_id
            );
        }
        let loc = document.loc_ref.as_ref();
        let message = proto::ChargenOption {
            id: document.id.clone(),
            race: document.race.clone(),
            class: document.class.clone(),
            sex: document.sex.clone(),
            faction: document.faction.clone(),
            enabled: document.enabled,
            name_key: loc
                .and_then(|reference| reference.name.clone())
                .unwrap_or_default(),
            description_key: loc
                .and_then(|reference| reference.description.clone())
                .unwrap_or_default(),
            visual_ref: document.visual_ref.clone(),
            spawn_zone_id: document.spawn.zone_id.clone(),
            spawn_position: Some(proto::Vec3 {
                x: document.spawn.position.x,
                y: document.spawn.position.y,
                z: document.spawn.position.z,
            }),
            spawn_heading: document.spawn.heading,
            starting_level: document.starting_level,
            starting_stats: document
                .starting_stats
                .iter()
                .map(|entry| proto::StatEntry {
                    stat: entry.stat.clone(),
                    value: entry.value,
                })
                .collect(),
            starting_loadout: document
                .starting_loadout
                .iter()
                .map(|entry| proto::LoadoutEntry {
                    item_id: entry.item_id.clone(),
                    quantity: entry.quantity,
                    slot: entry.slot.clone(),
                })
                .collect(),
            starting_abilities: document.starting_abilities.clone(),
            starting_quests: document.starting_quests.clone(),
            extra: extra_map(&document.extra, keep_extra)?,
        };
        if seen.insert(document.id.clone(), message).is_some() {
            bail!("chargen option id {} is declared twice", document.id);
        }
    }
    Ok(encode_rows(seen))
}

/// Compiles the item definitions.
///
/// `stack_limit` is copied through rather than defaulted. `mechanics/loot.md`
/// rule 5.7.1 makes the limit per-item content, and a compiler that invented a
/// default here would be deciding a gameplay rule in Rust; the reader treats an
/// absent limit as unstackable, and does so in one documented place.
fn item_rows(tree: &SourceTree, keep_extra: bool) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::Item> = BTreeMap::new();
    for document in &tree.items {
        let price = document.vendor_price.unwrap_or_default();
        let message = proto::Item {
            id: document.id.clone(),
            name_key: document.loc_ref.name.clone().unwrap_or_default(),
            category: document.category.clone().unwrap_or_default(),
            level: document.level.unwrap_or_default(),
            required_level: document.required_level.unwrap_or_default(),
            stack_limit: document.stack_limit.unwrap_or_default(),
            vendor_sell: price.sell,
            vendor_buy: price.buy,
            extra: extra_map(&document.extra, keep_extra)?,
        };
        if document.stack_limit.is_some_and(|limit| limit < 0) {
            bail!(
                "item {} declares a negative stack_limit {}",
                document.id,
                document.stack_limit.unwrap_or_default()
            );
        }
        if seen.insert(document.id.clone(), message).is_some() {
            bail!("item id {} is declared twice", document.id);
        }
    }
    Ok(encode_rows(seen))
}

/// Compiles the loot trees, enforcing every structural rule
/// `mechanics/loot.md` section 4 states as "enforced at load".
///
/// Doing it here rather than only in the shard is the point of a compiled pack:
/// a tree whose `chances` array is one short is a content mistake, and it
/// should stop a build rather than surface as a mob that never drops its rare.
fn loot_table_rows(
    tree: &SourceTree,
    known_items: &BTreeSet<&str>,
    keep_extra: bool,
) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::LootTable> = BTreeMap::new();
    for document in &tree.loot_tables {
        let root = loot_node(&document.root, &document.id, "/root", known_items, 1)?;
        let message = proto::LootTable {
            id: document.id.clone(),
            root: Some(root),
            extra: extra_map(&document.extra, keep_extra)?,
        };
        if seen.insert(document.id.clone(), message).is_some() {
            bail!("loot table id {} is declared twice", document.id);
        }
    }
    Ok(encode_rows(seen))
}

/// Converts one authored node, recursively. `depth` is the container level this
/// node would occupy, counting containers only, as `MAX_TREE_DEPTH` does.
fn loot_node(
    document: &source::LootNodeDocument,
    table_id: &str,
    pointer: &str,
    known_items: &BTreeSet<&str>,
    depth: u32,
) -> Result<proto::LootNode> {
    let counts = |kind: proto::LootNodeKind| -> Result<(i32, i32)> {
        let low = document.min_number.unwrap_or_default();
        let high = document.max_number.unwrap_or_default();
        if high < low {
            bail!(
                "loot table {table_id}: {pointer}: {} leaf declares max_number {high} below min_number {low}",
                kind.as_str_name()
            );
        }
        Ok((low, high))
    };

    match document.node.as_str() {
        kind @ ("and" | "or") => {
            if depth > MAX_LOOT_TREE_DEPTH {
                bail!(
                    "loot table {table_id}: {pointer}: container depth {depth} exceeds MAX_TREE_DEPTH of {MAX_LOOT_TREE_DEPTH}"
                );
            }
            if document.entries.len() != document.chances.len() {
                bail!(
                    "loot table {table_id}: {pointer}: {kind} node has {} entries but {} chances; they are positionally paired",
                    document.entries.len(),
                    document.chances.len()
                );
            }
            if document.entries.is_empty() {
                bail!("loot table {table_id}: {pointer}: {kind} node has no entries");
            }
            let mut entries = Vec::with_capacity(document.entries.len());
            for (index, child) in document.entries.iter().enumerate() {
                entries.push(loot_node(
                    child,
                    table_id,
                    &format!("{pointer}/entries/{index}"),
                    known_items,
                    depth + 1,
                )?);
            }
            Ok(proto::LootNode {
                kind: if kind == "and" {
                    proto::LootNodeKind::And as i32
                } else {
                    proto::LootNodeKind::Or as i32
                },
                entries,
                chances: document.chances.clone(),
                ..Default::default()
            })
        }
        "single-item" => {
            let (min_number, max_number) = counts(proto::LootNodeKind::SingleItem)?;
            let item_id = document
                .item
                .as_ref()
                .and_then(|reference| reference.id.clone())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "loot table {table_id}: {pointer}: single-item leaf names no item id"
                    )
                })?;
            // ADR 0006's cross-reference check. A grant the pack cannot resolve
            // to a stack limit has no way to reach a bag slot.
            if !known_items.contains(item_id.as_str()) {
                bail!(
                    "loot table {table_id}: {pointer}: grants {item_id}, which this pack does not carry as an item"
                );
            }
            Ok(proto::LootNode {
                kind: proto::LootNodeKind::SingleItem as i32,
                item_id,
                min_number,
                max_number,
                ..Default::default()
            })
        }
        "money" => {
            let (min_number, max_number) = counts(proto::LootNodeKind::Money)?;
            if document.item.is_some() {
                bail!(
                    "loot table {table_id}: {pointer}: money leaf carries an item reference; money credits the purse and occupies no bag slot"
                );
            }
            Ok(proto::LootNode {
                kind: proto::LootNodeKind::Money as i32,
                min_number,
                max_number,
                ..Default::default()
            })
        }
        other => bail!("loot table {table_id}: {pointer}: unknown node type {other:?}"),
    }
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
