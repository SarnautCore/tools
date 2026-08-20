//! Turning an authored source tree into a pack directory.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use prost::Message;

use crate::manifest::{self, Manifest};
use crate::proto;
use crate::refs::{self, LocaleIndex, ReferenceReport, Universe};
use crate::report::{self, BuildReport};
use crate::source::{self, Layout, LoadOptions, ObjectRef, Provenance, SourceTree};
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
pub const TABLE_QUESTS: &str = "quests";
pub const TABLE_ROUTES: &str = "routes";
pub const TABLE_LOCALE: &str = "locale";
pub const TABLE_MOB_KINDS: &str = "mob-kinds";
pub const TABLE_LEVEL_CURVE: &str = "level-curve";

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
    /// Directory holding `layers.yaml`. Defaults to `<source>/overlays`.
    pub overlays_root: Option<PathBuf>,
    /// Overlay layer ids to apply. Empty applies every default-on layer.
    pub overlays: Vec<String>,
    pub layout: Layout,
    pub ruleset: String,
    pub zone: Option<String>,
    pub keep_extra: bool,
    pub player_spawn: Option<PlayerSpawn>,
    pub source_repo: String,
    /// `None` asks the compiler to read the source repository's HEAD.
    pub source_commit: Option<String>,
    /// Accept two layers writing the same leaf, later layer winning. Every
    /// conflict is still listed in `build-report.json`.
    pub allow_overlay_conflicts: bool,
    /// Fail on a localization key no locale document supplies, rather than
    /// recording it as a coverage gap.
    pub require_locale: bool,
}

impl BuildOptions {
    fn overlays_root(&self) -> PathBuf {
        self.overlays_root
            .clone()
            .unwrap_or_else(|| self.source.join("overlays"))
    }
}

/// A pack held in memory: everything `build` would write, and the report that
/// explains it. `check` produces one and throws the bytes away.
#[derive(Debug)]
pub struct CompiledPack {
    pub manifest: Manifest,
    pub report: BuildReport,
    pub references: ReferenceReport,
    tables: Vec<EncodedTable>,
}

impl CompiledPack {
    pub fn pack_id(&self) -> &str {
        &self.manifest.pack_id
    }

    pub fn zone(&self) -> &str {
        &self.manifest.zone
    }

    /// Table names and row counts, in manifest order.
    pub fn tables(&self) -> Vec<(String, u32)> {
        self.tables
            .iter()
            .map(|(name, _, _, rows)| (name.clone(), *rows))
            .collect()
    }
}

type EncodedTable = (String, proto::RowType, Vec<u8>, u32);

/// Compiles `options.source` without writing anything.
pub fn compile(options: &BuildOptions) -> Result<CompiledPack> {
    let overlays_root = options.overlays_root();
    let load = LoadOptions {
        root: &options.source,
        overlays_root,
        selected_layers: &options.overlays,
        layout: options.layout,
        ruleset: &options.ruleset,
        zone: options.zone.as_deref(),
    };
    let tree = source::load(&load)?;

    if !tree.conflicts.is_empty() && !options.allow_overlay_conflicts {
        let described: Vec<String> = tree
            .conflicts
            .iter()
            .map(|conflict| conflict.describe())
            .collect();
        bail!(
            "{} overlay conflict(s); pass --allow-overlay-conflicts to let the later layer win:\n  {}",
            described.len(),
            described.join("\n  ")
        );
    }

    let zone_id = zone_id(&tree, options)?;
    let zone_slug = zone_id
        .strip_prefix("zone.")
        .ok_or_else(|| anyhow::anyhow!("zone id {zone_id} does not start with `zone.`"))?
        .to_string();

    let locales = LocaleIndex::build(&tree.locales);
    let universe = Universe::of(&tree, &zone_slug);
    let mut references = refs::check(&tree, &universe, &locales);
    if options.require_locale {
        refs::require_locale(&mut references);
    }
    if !references.is_clean() {
        bail!("{}", references.failure_message());
    }

    let level_curve = level_curve_row(&tree, options)?;
    let level_curve_id = tree
        .level_curves
        .iter()
        .find(|curve| curve.ruleset == options.ruleset)
        .map(|curve| curve.id.clone())
        .unwrap_or_default();

    let spawn_tables = spawn_table_rows(&tree, options.keep_extra)?;
    let placements = placement_rows(&tree, options.keep_extra)?;
    let zone_row = zone_row(
        &zone_id,
        &zone_slug,
        &level_curve_id,
        options,
        &tree,
        &locales,
    )?;
    let abilities = ability_rows(&tree, &locales, options.keep_extra)?;
    let factions = faction_rows(&tree, &locales, options.keep_extra)?;
    let mob_kinds = mob_kind_rows(&tree, &locales, options.keep_extra)?;
    let mobs = mob_rows(&tree, &locales, options.keep_extra)?;
    let chargen = chargen_rows(&tree, &locales, options.keep_extra)?;
    let items = item_rows(&tree, &locales, options.keep_extra)?;
    let loot_tables = loot_table_rows(&tree, options.keep_extra)?;
    let quests = quest_rows(&tree, &locales, options.keep_extra)?;
    let routes = route_rows(&tree, options.keep_extra)?;
    let locale_rows = locale_rows(&tree, options.keep_extra)?;

    // Every gameplay table below is written even when it has no rows, so a
    // reader can insist on the full set rather than treating "absent" and
    // "empty" as one case.
    let mut encoded = vec![
        encode(TABLE_ZONE, proto::RowType::Zone, &zone_row)?,
        encode(TABLE_PLACEMENTS, proto::RowType::Placement, &placements)?,
        encode(
            TABLE_SPAWN_TABLES,
            proto::RowType::SpawnTable,
            &spawn_tables,
        )?,
        encode(TABLE_ABILITIES, proto::RowType::Ability, &abilities)?,
        encode(TABLE_FACTIONS, proto::RowType::Faction, &factions)?,
        encode(TABLE_MOBS, proto::RowType::Mob, &mobs)?,
    ];
    // A source tree with no documents of a kind produces no table of that
    // kind, so a pack built before a row type existed keeps its digest.
    for (name, row_type, rows) in [
        (TABLE_CHARGEN, proto::RowType::ChargenOption, &chargen),
        (TABLE_ITEMS, proto::RowType::Item, &items),
        (TABLE_LOOT_TABLES, proto::RowType::LootTable, &loot_tables),
        (TABLE_QUESTS, proto::RowType::Quest, &quests),
        (TABLE_ROUTES, proto::RowType::Route, &routes),
        (TABLE_LOCALE, proto::RowType::Locale, &locale_rows),
        (TABLE_MOB_KINDS, proto::RowType::MobKind, &mob_kinds),
        (TABLE_LEVEL_CURVE, proto::RowType::LevelCurve, &level_curve),
    ] {
        if !rows.is_empty() {
            encoded.push(encode(name, row_type, rows)?);
        }
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
            // Layer ids, never the paths they were read from: a manifest that
            // embedded `..\data-schemas\...` would differ between a Windows
            // rebuild and the Linux CI one, and packs are compared byte for
            // byte.
            overlays: tree.layers.iter().map(|layer| layer.id.clone()).collect(),
        },
        keep_extra: options.keep_extra,
        tables: entries,
    };

    let report = BuildReport::new(
        &pack_id,
        &options.ruleset,
        &zone_slug,
        &tree,
        &references,
        locales.len(),
    );
    Ok(CompiledPack {
        manifest: document,
        report,
        references,
        tables: encoded,
    })
}

/// Compiles `options.source` into a pack directory at `out`.
pub fn build(options: &BuildOptions, out: &Path) -> Result<CompiledPack> {
    let pack = compile(options)?;
    write_pack(out, &pack)?;
    Ok(pack)
}

fn encode(name: &str, row_type: proto::RowType, rows: &[Row]) -> Result<EncodedTable> {
    Ok((
        name.to_string(),
        row_type,
        table::encode(row_type as i32, rows)?,
        rows.len() as u32,
    ))
}

fn write_pack(out: &Path, pack: &CompiledPack) -> Result<()> {
    let tables = out.join("tables");
    if tables.exists() {
        fs::remove_dir_all(&tables)
            .with_context(|| format!("clear {} before writing", tables.display()))?;
    }
    fs::create_dir_all(&tables)
        .with_context(|| format!("create pack directory {}", tables.display()))?;
    for (name, _, bytes, _) in &pack.tables {
        let path = tables.join(format!("{name}.sptbl"));
        fs::write(&path, bytes).with_context(|| format!("write table {}", path.display()))?;
    }
    let path = out.join(manifest::FILE_NAME);
    fs::write(&path, pack.manifest.to_canonical_json()?)
        .with_context(|| format!("write {}", path.display()))?;
    let path = out.join(report::FILE_NAME);
    fs::write(&path, pack.report.to_canonical_json()?)
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
        insert_unique(&mut seen, &document.id, message, "spawn table")?;
    }
    Ok(encode_rows(seen))
}

fn placement_rows(tree: &SourceTree, keep_extra: bool) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::Placement> = BTreeMap::new();
    for document in &tree.placement_documents {
        for placed in &document.placements {
            let object_id = placed
                .object
                .id
                .clone()
                .ok_or_else(|| anyhow::anyhow!("placement {} has no object id", placed.id))?;
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
            insert_unique(&mut seen, &placed.id, message, "placement")?;
        }
    }
    if seen.is_empty() {
        bail!("source tree contains no placements");
    }
    Ok(encode_rows(seen))
}

fn ability_rows(tree: &SourceTree, locales: &LocaleIndex, keep_extra: bool) -> Result<Vec<Row>> {
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
            name_key: key(locales, document.loc_ref.name.as_deref(), &document.source),
            description_key: key(
                locales,
                document.loc_ref.description.as_deref(),
                &document.source,
            ),
            extra: extra_map(&document.extra, keep_extra)?,
        };
        insert_unique(&mut seen, &document.id, message, "ability")?;
    }
    Ok(encode_rows(seen))
}

fn faction_rows(tree: &SourceTree, locales: &LocaleIndex, keep_extra: bool) -> Result<Vec<Row>> {
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
            name_key: key(locales, document.loc_ref.name.as_deref(), &document.source),
            extra: extra_map(&document.extra, keep_extra)?,
        };
        insert_unique(&mut seen, &document.id, message, "faction")?;
    }
    Ok(encode_rows(seen))
}

/// Compiles the creature taxonomy — MobKind, MobClass and MobQuality — into one
/// table.
///
/// Each record carries its own declared multipliers and its links. Nothing is
/// resolved across the prototype chain here: `mechanics/combat.md` section 7.1
/// defers the order the chain composes in, and a compiler that picked one would
/// be settling a rule in Rust that the spec deliberately leaves open.
fn mob_kind_rows(tree: &SourceTree, locales: &LocaleIndex, keep_extra: bool) -> Result<Vec<Row>> {
    use source::TaxonomyKind;

    let mut seen: BTreeMap<String, proto::MobKind> = BTreeMap::new();
    for document in &tree.mob_kinds {
        let taxonomy = match document.taxonomy {
            Some(TaxonomyKind::Class) => proto::MobTaxonomyKind::Class,
            Some(TaxonomyKind::Quality) => proto::MobTaxonomyKind::Quality,
            Some(TaxonomyKind::Kind) => proto::MobTaxonomyKind::Kind,
            None => proto::MobTaxonomyKind::Unspecified,
        };
        let message = proto::MobKind {
            id: document.id.clone(),
            taxonomy: taxonomy as i32,
            prototype_id: reference_id(&document.prototype),
            mob_class_id: reference_id(&document.mob_class),
            quality_id: reference_id(&document.quality),
            hp_mod: document.hp_mod.unwrap_or_default(),
            dps_mod: document.dps_mod.unwrap_or_default(),
            exp_mod: document.exp_mod.unwrap_or_default(),
            mana_mod: document.mana_mod.unwrap_or_default(),
            loot_mod: document.loot_mod.unwrap_or_default(),
            speed: document.speed.unwrap_or_default(),
            rank: document.rank.unwrap_or_default(),
            name_key: key(locales, document.loc_ref.name.as_deref(), &document.source),
            extra: extra_map(&document.extra, keep_extra)?,
        };
        insert_unique(&mut seen, &document.id, message, "mob kind")?;
    }
    Ok(encode_rows(seen))
}

/// Builds the mob rows, copying the multipliers of the MobKind each mob names.
///
/// Only the named record's own values are read. The prototype chain and the
/// product across kind, class and quality are deferred by `mechanics/combat.md`
/// section 7.1, and the `mob-kinds` table carries every record so a shard can
/// resolve the chain itself and name the record a number came from.
fn mob_rows(tree: &SourceTree, locales: &LocaleIndex, keep_extra: bool) -> Result<Vec<Row>> {
    let kinds: BTreeMap<&str, &source::MobKindDocument> = tree
        .mob_kinds
        .iter()
        .map(|kind| (kind.id.as_str(), kind))
        .collect();

    let mut seen: BTreeMap<String, proto::Mob> = BTreeMap::new();
    for document in &tree.mobs {
        let mob_kind_id = reference_id(&document.mob_kind);
        let quality_id = reference_id(&document.quality);
        let kind = kinds.get(mob_kind_id.as_str()).copied();
        let quality = kinds.get(quality_id.as_str()).copied();

        let message = proto::Mob {
            id: document.id.clone(),
            name_key: key(locales, document.loc_ref.name.as_deref(), &document.source),
            title_key: key(locales, document.loc_ref.title.as_deref(), &document.source),
            faction_id: reference_id(&document.faction),
            hp_mod: kind.and_then(|kind| kind.hp_mod).unwrap_or(DEFAULT_HP_MOD),
            dps_mod: kind.and_then(|kind| kind.dps_mod).unwrap_or_default(),
            exp_mod: kind.and_then(|kind| kind.exp_mod).unwrap_or_default(),
            mana_mod: kind.and_then(|kind| kind.mana_mod).unwrap_or_default(),
            loot_mod: kind.and_then(|kind| kind.loot_mod).unwrap_or_default(),
            speed_mod: kind.and_then(|kind| kind.speed).unwrap_or_default(),
            mob_class_id: kind
                .map(|kind| reference_id(&kind.mob_class))
                .unwrap_or_default(),
            quality_rank: quality.and_then(|quality| quality.rank).unwrap_or_default(),
            quality_id,
            mob_kind_id,
            level_min: document.level_min.unwrap_or_default(),
            level_max: document.level_max.unwrap_or_default(),
            walk_speed: document.walk_speed.unwrap_or_default(),
            aggro_radius_m: document.aggro_radius_m.unwrap_or_default(),
            leash_radius_m: document.leash_radius_m.unwrap_or_default(),
            ability_ids: document
                .abilities
                .iter()
                .filter_map(|reference| reference.id.clone())
                .collect(),
            loot_table_id: reference_id(&document.loot_table),
            extra: extra_map(&document.extra, keep_extra)?,
        };
        insert_unique(&mut seen, &document.id, message, "mob")?;
    }
    Ok(encode_rows(seen))
}

/// Compiles the quest definitions (`mechanics/quests.md` section 4).
///
/// `quest-count-special` objectives are carried rather than dropped. Ten of the
/// M2 zone's twelve objectives are that kind, and rule 5.5.6 makes refusing one
/// the shard's job at content-load: a compiler that silently discarded them
/// would hand the shard a quest that looks completable and is not.
fn quest_rows(tree: &SourceTree, locales: &LocaleIndex, keep_extra: bool) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::Quest> = BTreeMap::new();
    for document in &tree.quests {
        let mut objectives = Vec::with_capacity(document.objectives.len());
        for objective in &document.objectives {
            let kind = match objective.kind.as_str() {
                "quest-count-kill" => proto::QuestObjectiveKind::CountKill,
                "quest-count-item" => proto::QuestObjectiveKind::CountItem,
                "quest-count-special" => proto::QuestObjectiveKind::CountSpecial,
                other => bail!(
                    "quest {}: objective kind {other:?} is not one mechanics/quests.md rule 5.5 defines",
                    document.id
                ),
            };
            let limit = objective.limit.unwrap_or_default();
            if limit < 0 {
                bail!(
                    "quest {}: objective declares a negative limit {limit}",
                    document.id
                );
            }
            objectives.push(proto::QuestObjective {
                kind: kind as i32,
                limit,
                target_ids: objective
                    .targets
                    .iter()
                    .filter_map(|target| target.id.clone())
                    .collect(),
                internal: objective.internal,
                show_count: objective.show_count,
                counter_key: key(locales, objective.loc_ref.as_deref(), &document.source),
                remove_on_abandon: flag(&objective.extra, "removeOnAbandon"),
            });
        }

        let message = proto::Quest {
            id: document.id.clone(),
            zone_id: document.zone.clone(),
            level: document.level.unwrap_or_default(),
            required_level: document.required_level.unwrap_or_default(),
            quest_type: document.quest_type.clone().unwrap_or_default(),
            starter_id: reference_id(&document.starter),
            finisher_id: reference_id(&document.finisher),
            can_cancel: document
                .flags
                .get("can_cancel")
                .copied()
                .unwrap_or_default(),
            prerequisites: document
                .prerequisites
                .iter()
                .map(|prerequisite| proto::QuestPrerequisite {
                    quest_id: prerequisite.quest.id.clone().unwrap_or_default(),
                    required_status: prerequisite.status.clone().unwrap_or_default(),
                })
                .collect(),
            objectives,
            rewards: Some(proto::QuestRewards {
                experience: document.rewards.experience,
                money: document.rewards.money,
                honor: document.rewards.honor,
                mandatory_items: reward_items(&document.rewards.mandatory_items),
                alternative_items: reward_items(&document.rewards.alternative_items),
            }),
            name_key: key(locales, document.loc_ref.name.as_deref(), &document.source),
            goal_key: key(locales, document.loc_ref.goal.as_deref(), &document.source),
            start_key: key(locales, document.loc_ref.start.as_deref(), &document.source),
            check_key: key(locales, document.loc_ref.check.as_deref(), &document.source),
            finish_key: key(
                locales,
                document.loc_ref.finish.as_deref(),
                &document.source,
            ),
            extra: extra_map(&document.extra, keep_extra)?,
        };
        insert_unique(&mut seen, &document.id, message, "quest")?;
    }
    Ok(encode_rows(seen))
}

fn reward_items(items: &[source::QuestRewardItemDocument]) -> Vec<proto::QuestRewardItem> {
    items
        .iter()
        .map(|reward| proto::QuestRewardItem {
            item_id: reward.item.id.clone().unwrap_or_default(),
            count: reward.count,
            hidden: reward.hidden,
        })
        .collect()
}

/// Reads a boolean out of the untyped passthrough.
///
/// The key is a MY.GAMES attribute name and stays in this file rather than
/// reaching a pack: mapping a value into a typed field is the extractor's whole
/// job, and the field name it maps to is ours (ADR 0011).
fn flag(extra: &BTreeMap<String, serde_yaml::Value>, key: &str) -> bool {
    extra
        .get(key)
        .map(|value| match value {
            serde_yaml::Value::Bool(inner) => *inner,
            serde_yaml::Value::String(inner) => inner.eq_ignore_ascii_case("true"),
            _ => false,
        })
        .unwrap_or(false)
}

fn route_rows(tree: &SourceTree, keep_extra: bool) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::Route> = BTreeMap::new();
    for document in &tree.routes {
        let indices: BTreeSet<u32> = document.points.iter().map(|point| point.index).collect();
        if indices.len() != document.points.len() {
            bail!("route {}: two waypoints share one index", document.id);
        }
        for link in &document.links {
            for endpoint in [link.from, link.to] {
                if !indices.contains(&endpoint) {
                    bail!(
                        "route {}: link {} -> {} names waypoint {endpoint}, which the route does not declare",
                        document.id,
                        link.from,
                        link.to
                    );
                }
            }
        }
        let message = proto::Route {
            id: document.id.clone(),
            zone_id: document.zone.clone(),
            map: document.map.clone().unwrap_or_default(),
            points: document
                .points
                .iter()
                .map(|point| proto::RoutePoint {
                    index: point.index,
                    position: Some(proto::Vec3 {
                        x: point.position.x,
                        y: point.position.y,
                        z: point.position.z,
                    }),
                })
                .collect(),
            links: document
                .links
                .iter()
                .map(|link| proto::RouteLink {
                    from: link.from,
                    to: link.to,
                    weight: link.weight,
                    movement: link.movement.clone().unwrap_or_default(),
                    flying: link.flying,
                })
                .collect(),
            extra: extra_map(&document.extra, keep_extra)?,
        };
        insert_unique(&mut seen, &document.id, message, "route")?;
    }
    Ok(encode_rows(seen))
}

/// Compiles the locale documents, one row per language pack.
///
/// Entries are sorted by key inside the row. The authored order is whatever the
/// loc pack happened to have, and sorting is what makes two builds of the same
/// content produce the same bytes.
fn locale_rows(tree: &SourceTree, keep_extra: bool) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::Locale> = BTreeMap::new();
    for document in &tree.locales {
        let mut entries: BTreeMap<&str, &str> = BTreeMap::new();
        for entry in &document.entries {
            if let Some(existing) = entries.insert(&entry.key, &entry.text)
                && existing != entry.text
            {
                bail!(
                    "locale {}: key {} is declared twice with different text",
                    document.id,
                    entry.key
                );
            }
        }
        let message = proto::Locale {
            id: document.id.clone(),
            language: document.language.clone(),
            entries: entries
                .into_iter()
                .map(|(key, text)| proto::LocaleEntry {
                    key: key.to_string(),
                    text: text.to_string(),
                })
                .collect(),
            extra: extra_map(&document.extra, keep_extra)?,
        };
        insert_unique(&mut seen, &document.id, message, "locale")?;
    }
    Ok(encode_rows(seen))
}

/// Compiles the curated level curve (`mechanics/combat.md` section 7.1).
///
/// At most one curve per ruleset: two would leave the shard choosing which base
/// HP a mob scales, and there is no field anywhere that says which.
fn level_curve_row(tree: &SourceTree, options: &BuildOptions) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::LevelCurve> = BTreeMap::new();
    let matching: Vec<&source::LevelCurveDocument> = tree
        .level_curves
        .iter()
        .filter(|curve| curve.ruleset == options.ruleset)
        .collect();
    if matching.len() > 1 {
        bail!(
            "ruleset {} declares {} level curves ({}); it may declare at most one",
            options.ruleset,
            matching.len(),
            matching
                .iter()
                .map(|curve| curve.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    for document in matching {
        if document.points.is_empty() {
            bail!("level curve {} declares no points", document.id);
        }
        let first = document.points[0].level;
        for (expected, point) in (first..).zip(document.points.iter()) {
            if point.level != expected {
                bail!(
                    "level curve {}: points must be one per level and ascending, but level {} appears where {expected} was expected",
                    document.id,
                    point.level
                );
            }
            if point.base_hp <= 0 {
                bail!(
                    "level curve {}: level {} declares base_hp {}, which is not a health total",
                    document.id,
                    point.level,
                    point.base_hp
                );
            }
        }
        let message = proto::LevelCurve {
            id: document.id.clone(),
            ruleset: document.ruleset.clone(),
            points: document
                .points
                .iter()
                .map(|point| proto::LevelCurvePoint {
                    level: point.level,
                    base_hp: point.base_hp,
                    base_dps: point.base_dps,
                    base_experience: point.base_experience,
                })
                .collect(),
            extra: BTreeMap::new(),
        };
        insert_unique(&mut seen, &document.id, message, "level curve")?;
    }
    Ok(encode_rows(seen))
}

/// Compiles the character-creation options (ADR 0032).
fn chargen_rows(tree: &SourceTree, locales: &LocaleIndex, keep_extra: bool) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::ChargenOption> = BTreeMap::new();
    for document in &tree.chargen_options {
        if !document.spawn.zone_id.starts_with("zone.") {
            bail!(
                "chargen option {} spawns into {}, which is not a canonical zone id",
                document.id,
                document.spawn.zone_id
            );
        }
        let message = proto::ChargenOption {
            id: document.id.clone(),
            race: document.race.clone(),
            class: document.class.clone(),
            sex: document.sex.clone(),
            faction: document.faction.clone(),
            enabled: document.enabled,
            name_key: key(locales, document.loc_ref.name.as_deref(), &document.source),
            description_key: key(
                locales,
                document.loc_ref.description.as_deref(),
                &document.source,
            ),
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
        insert_unique(&mut seen, &document.id, message, "chargen option")?;
    }
    Ok(encode_rows(seen))
}

/// Compiles the item definitions.
///
/// `stack_limit` is copied through rather than defaulted. `mechanics/loot.md`
/// rule 5.7.1 makes the limit per-item content, and a compiler that invented a
/// default here would be deciding a gameplay rule in Rust; the reader treats an
/// absent limit as unstackable, and does so in one documented place.
fn item_rows(tree: &SourceTree, locales: &LocaleIndex, keep_extra: bool) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::Item> = BTreeMap::new();
    for document in &tree.items {
        if document.stack_limit.is_some_and(|limit| limit < 0) {
            bail!(
                "item {} declares a negative stack_limit {}",
                document.id,
                document.stack_limit.unwrap_or_default()
            );
        }
        let price = document.vendor_price.unwrap_or_default();
        let message = proto::Item {
            id: document.id.clone(),
            name_key: key(locales, document.loc_ref.name.as_deref(), &document.source),
            description_key: key(
                locales,
                document.loc_ref.description.as_deref(),
                &document.source,
            ),
            category: document.category.clone().unwrap_or_default(),
            level: document.level.unwrap_or_default(),
            required_level: document.required_level.unwrap_or_default(),
            stack_limit: document.stack_limit.unwrap_or_default(),
            vendor_sell: price.sell,
            vendor_buy: price.buy,
            extra: extra_map(&document.extra, keep_extra)?,
        };
        insert_unique(&mut seen, &document.id, message, "item")?;
    }
    Ok(encode_rows(seen))
}

/// Compiles the loot trees, enforcing every structural rule
/// `mechanics/loot.md` section 4 states as "enforced at load".
///
/// Doing it here rather than only in the shard is the point of a compiled pack:
/// a tree whose `chances` array is one short is a content mistake, and it
/// should stop a build rather than surface as a mob that never drops its rare.
fn loot_table_rows(tree: &SourceTree, keep_extra: bool) -> Result<Vec<Row>> {
    let mut seen: BTreeMap<String, proto::LootTable> = BTreeMap::new();
    for document in &tree.loot_tables {
        let root = loot_node(&document.root, &document.id, "/root", 1)?;
        let message = proto::LootTable {
            id: document.id.clone(),
            root: Some(root),
            extra: extra_map(&document.extra, keep_extra)?,
        };
        insert_unique(&mut seen, &document.id, message, "loot table")?;
    }
    Ok(encode_rows(seen))
}

/// Converts one authored node, recursively. `depth` is the container level this
/// node would occupy, counting containers only, as `MAX_TREE_DEPTH` does.
fn loot_node(
    document: &source::LootNodeDocument,
    table_id: &str,
    pointer: &str,
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

/// Builds the single zone row.
///
/// A zone document is authoritative when the tree carries one. `--player-spawn`
/// overrides it, because a build that wants a player somewhere specific should
/// not have to edit content to say so.
fn zone_row(
    zone_id: &str,
    zone_slug: &str,
    level_curve_id: &str,
    options: &BuildOptions,
    tree: &SourceTree,
    locales: &LocaleIndex,
) -> Result<Vec<Row>> {
    let document = tree.zones.iter().find(|zone| zone.id == zone_id);
    let spawn = match (
        options.player_spawn,
        document.and_then(|zone| zone.player_spawn.as_ref()),
    ) {
        (Some(spawn), _) => spawn,
        (None, Some(authored)) => PlayerSpawn {
            x: authored.position.x,
            y: authored.position.y,
            z: authored.position.z,
            yaw: authored.heading,
        },
        (None, None) => default_player_spawn(tree)?,
    };
    let bounds = document.and_then(|zone| zone.bounds);
    let levels = document.and_then(|zone| zone.level_range);
    let instance = document.and_then(|zone| zone.instance);
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
        name_key: document
            .map(|zone| key(locales, zone.loc_ref.name.as_deref(), &zone.source))
            .unwrap_or_default(),
        description_key: document
            .map(|zone| key(locales, zone.loc_ref.description.as_deref(), &zone.source))
            .unwrap_or_default(),
        maps: document.map(|zone| zone.maps.clone()).unwrap_or_default(),
        bounds_min: bounds.map(|bounds| proto::Vec3 {
            x: bounds.min.x,
            y: bounds.min.y,
            z: bounds.min.z,
        }),
        bounds_max: bounds.map(|bounds| proto::Vec3 {
            x: bounds.max.x,
            y: bounds.max.y,
            z: bounds.max.z,
        }),
        level_min: levels.map(|range| range.min).unwrap_or_default(),
        level_max: levels.map(|range| range.max).unwrap_or_default(),
        instanced: instance
            .map(|instance| instance.instanced)
            .unwrap_or_default(),
        max_players: instance
            .map(|instance| instance.max_players)
            .unwrap_or_default(),
        spawn_map: document
            .and_then(|zone| zone.player_spawn.as_ref())
            .and_then(|spawn| spawn.map.clone())
            .unwrap_or_default(),
        level_curve_id: level_curve_id.to_string(),
        extra: BTreeMap::new(),
    };
    Ok(encode_rows(BTreeMap::from([(
        zone_id.to_string(),
        message,
    )])))
}

/// With no zone document and no `--player-spawn`, the player spawn falls back
/// to the first live placement in canonical-id order. It is deterministic and
/// inside the playable space, which is what the shard needs.
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
            "no live placement carries a position, so the player spawn cannot be derived; author a zone document or pass --player-spawn"
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

fn insert_unique<M>(
    seen: &mut BTreeMap<String, M>,
    id: &str,
    message: M,
    label: &str,
) -> Result<()> {
    if seen.insert(id.to_string(), message).is_some() {
        bail!("{label} id {id} is declared twice");
    }
    Ok(())
}

fn reference_id(reference: &Option<ObjectRef>) -> String {
    reference
        .as_ref()
        .and_then(ObjectRef::id)
        .unwrap_or_default()
        .to_string()
}

/// Resolves an authored localization key to the one a locale document supplies.
/// Resolves an authored localization key to the one a locale document supplies.
///
/// A key the pack cannot resolve is written as empty rather than carried
/// through: see `LocaleIndex::resolve` for why that is an ADR 0011 rule. The
/// gap is recorded in `build-report.json`, which never reaches a public repo.
fn key(locales: &LocaleIndex, authored: Option<&str>, source: &Provenance) -> String {
    let Some(authored) = authored else {
        return String::new();
    };
    let resolved = locales.resolve(authored, source);
    if resolved.found {
        resolved.key
    } else {
        String::new()
    }
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
