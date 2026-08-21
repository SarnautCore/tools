//! Reading the authored YAML: which files a build consumes, in what order the
//! overlay layers apply, and what the merged documents deserialise into.
//!
//! Two things here are not obvious from the file list.
//!
//! **The item tree is never walked.** `data/classic/items/` holds 36,980
//! documents across two dozen directories. A zone pack needs the handful of
//! them its loot tables, chargen options and quest rewards actually name, so
//! items are loaded on demand through `items/index.yaml` and the build's cost
//! tracks the zone, not the catalogue.
//!
//! **Reachability selects loot and items, but only for a ruleset tree.** A flat
//! hand-authored dataset is small and every document in it is deliberate, so it
//! is compiled whole. A ruleset tree is a bulk extraction where "everything the
//! extractor wrote" is not a useful answer to "what does this zone need".

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_yaml::Value;
use walkdir::WalkDir;

use crate::overlay::{self, Conflict, CurationNote, DocumentSet, Layer};

/// Where the authored documents sit under the source root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// The private `data` repo: `<src>/<ruleset>/…`, one zone directory per
    /// zone plus the ruleset-global resource directories beside it.
    Ruleset,
    /// A flat directory of hand-authored documents, as in `data-schemas/demo`.
    Flat,
}

/// Every authored document the compiler consumes, already split by kind.
#[derive(Debug, Default)]
pub struct SourceTree {
    pub zones: Vec<ZoneDocument>,
    pub placement_documents: Vec<PlacementDocument>,
    pub tables: Vec<SpawnTableDocument>,
    pub mobs: Vec<MobDocument>,
    pub mob_kinds: Vec<MobKindDocument>,
    pub abilities: Vec<AbilityDocument>,
    pub factions: Vec<FactionDocument>,
    pub chargen_options: Vec<ChargenDocument>,
    pub items: Vec<ItemDocument>,
    pub loot_tables: Vec<LootTableDocument>,
    pub quests: Vec<QuestDocument>,
    pub quest_scripts: Vec<QuestScriptDocument>,
    pub script_triggers: Vec<ScriptTriggerDocument>,
    pub routes: Vec<RouteDocument>,
    pub locales: Vec<LocaleDocument>,
    pub level_curves: Vec<LevelCurveDocument>,
    /// Which overlay layers were applied, in `layers.yaml` order.
    pub layers: Vec<AppliedLayer>,
    /// Why each curated document exists. Aggregated into `build-report.json`
    /// and never written into table bytes (ADR 0029).
    pub notes: Vec<CurationNote>,
    /// Overlay documents whose declared or canonical-id zone is outside this
    /// build's zone selection.
    pub skipped_overlay_documents: Vec<SkippedOverlayDocument>,
    /// Leaves two overlay layers both wrote.
    pub conflicts: Vec<Conflict>,
    /// Ids an overlay removed outright.
    pub deleted: Vec<String>,
    /// Loot tables the source tree carries that no mob in this zone reaches.
    pub unreachable_loot_tables: usize,
    /// How many item documents were opened out of the index.
    pub items_loaded_from_index: usize,
}

/// One overlay layer that contributed to this build.
#[derive(Clone, Debug)]
pub struct AppliedLayer {
    pub id: String,
    pub description: String,
    pub documents: usize,
}

/// One overlay document omitted because it belongs to another zone pack.
#[derive(Clone, Debug)]
pub struct SkippedOverlayDocument {
    pub document_id: String,
    pub layer: String,
    pub source: String,
    pub note: String,
}

/// Where an extracted document came from. Only the path is read, and only to
/// resolve a relative localization key against the directory it was authored
/// in. A hand-authored document carries none, and its keys stand alone.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Provenance {
    #[serde(default)]
    pub path: String,
}

/// The localization keys a document carries (ADR 0007).
///
/// The field set is the union of the `loc_ref` shapes `data-schemas` declares.
/// It is spelled out rather than read as a free map so that a misspelled key is
/// a schema error in the data repository rather than a string that silently
/// reaches nothing.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct LocRef {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub start: Option<String>,
    #[serde(default)]
    pub check: Option<String>,
    #[serde(default)]
    pub finish: Option<String>,
}

impl LocRef {
    /// Every key this document declares, as `(field, authored value)`.
    pub fn entries(&self) -> Vec<(&'static str, &str)> {
        [
            ("name", self.name.as_deref()),
            ("description", self.description.as_deref()),
            ("title", self.title.as_deref()),
            ("goal", self.goal.as_deref()),
            ("start", self.start.as_deref()),
            ("check", self.check.as_deref()),
            ("finish", self.finish.as_deref()),
        ]
        .into_iter()
        .filter_map(|(field, value)| value.map(|value| (field, value)))
        .collect()
    }
}

/// One zone: the document every zone-scoped `zone:` field points at.
#[derive(Debug, Deserialize)]
pub struct ZoneDocument {
    pub id: String,
    #[serde(default)]
    pub loc_ref: LocRef,
    #[serde(default)]
    pub maps: Vec<String>,
    #[serde(default)]
    pub player_spawn: Option<ZonePlayerSpawn>,
    #[serde(default)]
    pub bounds: Option<Bounds>,
    #[serde(default)]
    pub level_range: Option<LevelRange>,
    #[serde(default)]
    pub instance: Option<ZoneInstance>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source", default)]
    pub source: Provenance,
}

#[derive(Debug, Deserialize)]
pub struct ZonePlayerSpawn {
    #[serde(default)]
    pub map: Option<String>,
    pub position: Position,
    #[serde(default)]
    pub heading: f32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Bounds {
    pub min: Position,
    pub max: Position,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct LevelRange {
    pub min: u32,
    pub max: u32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct ZoneInstance {
    #[serde(default)]
    pub instanced: bool,
    #[serde(default)]
    pub max_players: u32,
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
    pub quality: Option<ObjectRef>,
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
    /// Quests this NPC offers. The quest document names its own starter and
    /// finisher too; both directions are authored, and the compiler checks
    /// both rather than trusting one.
    #[serde(default)]
    pub quests: Vec<ObjectRef>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source", default)]
    pub source: Provenance,
}

/// Which of the three creature-taxonomy record types a document is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaxonomyKind {
    Kind,
    Class,
    Quality,
}

/// A `MobKind`, `MobClass` or `MobQuality` document.
///
/// Every multiplier is read as the record's own declared value. The prototype
/// link is carried, not walked: `mechanics/combat.md` section 7.1 defers the
/// resolution order of the chain, and half a walk produces a number nothing can
/// explain.
#[derive(Debug, Deserialize)]
pub struct MobKindDocument {
    pub id: String,
    #[serde(skip)]
    pub taxonomy: Option<TaxonomyKind>,
    #[serde(default)]
    pub loc_ref: LocRef,
    #[serde(default)]
    pub prototype: Option<ObjectRef>,
    #[serde(default)]
    pub mob_class: Option<ObjectRef>,
    #[serde(default)]
    pub quality: Option<ObjectRef>,
    #[serde(default)]
    pub hp_mod: Option<f32>,
    #[serde(default)]
    pub dps_mod: Option<f32>,
    #[serde(default)]
    pub exp_mod: Option<f32>,
    #[serde(default)]
    pub mana_mod: Option<f32>,
    #[serde(default)]
    pub loot_mod: Option<f32>,
    #[serde(default)]
    pub speed: Option<f32>,
    #[serde(default)]
    pub rank: Option<u32>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source", default)]
    pub source: Provenance,
}

/// The curated per-level base HP/DPS/XP table (`mechanics/combat.md` §7.1).
#[derive(Debug, Deserialize)]
pub struct LevelCurveDocument {
    pub id: String,
    pub ruleset: String,
    #[serde(default)]
    pub points: Vec<LevelCurvePointDocument>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct LevelCurvePointDocument {
    pub level: u32,
    pub base_hp: i32,
    #[serde(default)]
    pub base_dps: f32,
    #[serde(default)]
    pub base_experience: i64,
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
    #[serde(rename = "_source", default)]
    pub source: Provenance,
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
    #[serde(rename = "_source", default)]
    pub source: Provenance,
}

#[derive(Debug, Deserialize)]
pub struct FactionRelationDocument {
    pub faction: String,
    pub stance: String,
}

/// One item definition. Only the fields the shard needs to hold an instance in
/// a bag are read; the combat, stat and visual blocks stay in the authored
/// document until something consumes them.
#[derive(Debug, Deserialize)]
pub struct ItemDocument {
    pub id: String,
    #[serde(default)]
    pub loc_ref: LocRef,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub level: Option<u32>,
    #[serde(default)]
    pub required_level: Option<i32>,
    /// Maximum units in one stack. `mechanics/loot.md` rule 5.7.1 makes this
    /// per-item content rather than a constant, so an absent value stays absent
    /// here and the reader decides what it means.
    #[serde(default)]
    pub stack_limit: Option<i32>,
    #[serde(default)]
    pub vendor_price: Option<VendorPriceDocument>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source", default)]
    pub source: Provenance,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct VendorPriceDocument {
    #[serde(default)]
    pub sell: i64,
    #[serde(default)]
    pub buy: i64,
}

#[derive(Debug, Deserialize)]
pub struct LootTableDocument {
    pub id: String,
    pub root: LootNodeDocument,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
}

/// One node of a loot tree, read as the union of every node shape rather than
/// as a tagged enum.
///
/// Discriminating on `node` here would make a misspelled discriminator a serde
/// error naming the whole tree; reading the union and dispatching in the
/// compiler makes it an error naming the one node, which is the difference
/// between a usable message and a wall of YAML.
#[derive(Debug, Deserialize)]
pub struct LootNodeDocument {
    pub node: String,
    #[serde(default)]
    pub entries: Vec<LootNodeDocument>,
    #[serde(default)]
    pub chances: Vec<f64>,
    #[serde(default)]
    pub item: Option<ObjectRef>,
    #[serde(default)]
    pub min_number: Option<i32>,
    #[serde(default)]
    pub max_number: Option<i32>,
}

/// One quest definition (`mechanics/quests.md` section 4).
#[derive(Debug, Deserialize)]
pub struct QuestDocument {
    pub id: String,
    pub zone: String,
    #[serde(default)]
    pub loc_ref: LocRef,
    #[serde(default)]
    pub level: Option<u32>,
    #[serde(default)]
    pub required_level: Option<u32>,
    #[serde(default)]
    pub quest_type: Option<String>,
    #[serde(default)]
    pub starter: Option<ObjectRef>,
    #[serde(default)]
    pub finisher: Option<ObjectRef>,
    #[serde(default)]
    pub prerequisites: Vec<QuestPrerequisiteDocument>,
    #[serde(default)]
    pub objectives: Vec<QuestObjectiveDocument>,
    #[serde(default)]
    pub rewards: QuestRewardsDocument,
    #[serde(default)]
    pub flags: BTreeMap<String, bool>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source", default)]
    pub source: Provenance,
}

#[derive(Debug, Deserialize)]
pub struct QuestPrerequisiteDocument {
    pub quest: ObjectRef,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QuestObjectiveDocument {
    pub objective_id: String,
    pub kind: String,
    #[serde(default)]
    pub loc_ref: Option<String>,
    #[serde(default)]
    pub limit: Option<i32>,
    #[serde(default)]
    pub internal: bool,
    #[serde(default)]
    pub show_count: bool,
    #[serde(default)]
    pub targets: Vec<ObjectRef>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Deserialize)]
pub struct QuestRewardsDocument {
    #[serde(default)]
    pub experience: i64,
    #[serde(default)]
    pub money: i64,
    #[serde(default)]
    pub honor: i64,
    #[serde(default)]
    pub mandatory_items: Vec<QuestRewardItemDocument>,
    #[serde(default)]
    pub alternative_items: Vec<QuestRewardItemDocument>,
}

#[derive(Debug, Deserialize)]
pub struct QuestRewardItemDocument {
    pub item: ObjectRef,
    #[serde(default)]
    pub count: i32,
    #[serde(default)]
    pub hidden: bool,
}

/// One quest's script surface (ADR 0036): the counter bindings and the
/// startImpacts / triggerAgents trees quest activation evaluates.
#[derive(Debug, Deserialize)]
pub struct QuestScriptDocument {
    pub id: String,
    pub zone: String,
    /// The quest this script surface belongs to.
    pub quest: String,
    #[serde(default)]
    pub counters: Vec<QuestCounterDocument>,
    #[serde(default)]
    pub start_impacts: Vec<ScriptNodeDocument>,
    #[serde(default)]
    pub trigger_agents: Vec<ScriptNodeDocument>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source", default)]
    pub source: Provenance,
}

/// One QuestCountId and the objective it advances.
#[derive(Debug, Deserialize)]
pub struct QuestCounterDocument {
    /// Canonical id, for example `questcount.inst-league1.quest-1-30.count-id-1`.
    pub count_id: String,
    /// Position in the owning quest's `objectives` list.
    pub objective: u32,
    /// Stable semantic identity. The positional index is only compatibility.
    pub objective_id: String,
}

/// One TriggerResource document: a trigger tree shared by reference from
/// ImpactAttachTrigger and TriggerAgent* nodes.
#[derive(Debug, Deserialize)]
pub struct ScriptTriggerDocument {
    pub id: String,
    pub zone: String,
    /// True when map machinery outside the quest-script graph attaches this
    /// trigger. Used only to seed the reachability census.
    #[serde(default)]
    pub entrypoint: bool,
    pub root: ScriptNodeDocument,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source", default)]
    pub source: Provenance,
}

/// One node of a recursive script tree, mirroring `sarnaut.content.v1.ScriptNode`
/// field for field (ADR 0036: "Authored YAML has the same fields").
#[derive(Debug, Deserialize)]
pub struct ScriptNodeDocument {
    pub key: String,
    pub family: String,
    pub opcode: String,
    /// `implemented`, `inert-and-counted` or `refused`.
    pub tier: String,
    #[serde(default)]
    pub fields: Vec<ScriptFieldDocument>,
}

#[derive(Debug, Deserialize)]
pub struct ScriptFieldDocument {
    pub name: String,
    pub value: ScriptValueDocument,
}

/// One value of a script field. Exactly one member must be set, mirroring the
/// `ScriptValue` oneof; the compiler rejects zero or several.
#[derive(Debug, Default, Deserialize)]
pub struct ScriptValueDocument {
    #[serde(default)]
    pub integer: Option<i64>,
    #[serde(default)]
    pub decimal: Option<ScriptDecimalDocument>,
    #[serde(default)]
    pub boolean: Option<bool>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub reference: Option<ScriptRefDocument>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub node: Option<Box<ScriptNodeDocument>>,
    #[serde(default)]
    pub list: Option<Vec<ScriptValueDocument>>,
}

/// An exact signed mantissa and base-10 scale: `mantissa * 10^-scale`.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct ScriptDecimalDocument {
    pub mantissa: i64,
    #[serde(default)]
    pub scale: i32,
}

/// A resolved content reference. The `href` sibling is provenance for the
/// private YAML only and never reaches a compiled pack (ADR 0011).
#[derive(Debug, Deserialize)]
pub struct ScriptRefDocument {
    pub id: String,
    #[serde(default)]
    pub row_type: Option<String>,
    #[serde(default)]
    pub href: Option<String>,
}

/// Collects every content reference a script tree makes, in document order.
/// Both the reference checker and the item-reachability pass walk trees this
/// way, so the two never disagree about what a tree names.
pub fn collect_script_refs<'a>(
    node: &'a ScriptNodeDocument,
    into: &mut Vec<&'a ScriptRefDocument>,
) {
    for field in &node.fields {
        collect_value_refs(&field.value, into);
    }
}

fn collect_value_refs<'a>(value: &'a ScriptValueDocument, into: &mut Vec<&'a ScriptRefDocument>) {
    if let Some(reference) = &value.reference {
        into.push(reference);
    }
    if let Some(node) = &value.node {
        collect_script_refs(node, into);
    }
    if let Some(list) = &value.list {
        for entry in list {
            collect_value_refs(entry, into);
        }
    }
}

/// One patrol route a placement can follow.
#[derive(Debug, Deserialize)]
pub struct RouteDocument {
    pub id: String,
    pub zone: String,
    #[serde(default)]
    pub map: Option<String>,
    #[serde(default)]
    pub points: Vec<RoutePointDocument>,
    #[serde(default)]
    pub links: Vec<RouteLinkDocument>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct RoutePointDocument {
    pub index: u32,
    pub position: Position,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RouteLinkDocument {
    pub from: u32,
    pub to: u32,
    #[serde(default)]
    pub weight: f32,
    #[serde(default)]
    pub movement: Option<String>,
    #[serde(default)]
    pub flying: bool,
}

/// Every string of one language (ADR 0007).
#[derive(Debug, Deserialize)]
pub struct LocaleDocument {
    pub id: String,
    pub language: String,
    #[serde(default)]
    pub entries: Vec<LocaleEntryDocument>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct LocaleEntryDocument {
    pub key: String,
    #[serde(default)]
    pub text: String,
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
    pub map: String,
    #[serde(default)]
    pub map_resource: String,
    #[serde(default)]
    pub placements: Vec<SourcePlacement>,
    #[serde(default)]
    pub locators: Vec<SourceMapLocator>,
}

#[derive(Debug, Deserialize)]
pub struct SourceMapLocator {
    pub script_id: String,
    pub position: Position,
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

/// One character-creation option (ADR 0032).
///
/// A chargen document carries no `kind`, so it is recognised by its canonical
/// `chargen.` id prefix. It names the zone it spawns into rather than belonging
/// to one.
#[derive(Debug, Deserialize)]
pub struct ChargenDocument {
    pub id: String,
    pub race: String,
    pub class: String,
    pub sex: String,
    pub faction: String,
    pub enabled: bool,
    #[serde(default)]
    pub loc_ref: LocRef,
    pub visual_ref: String,
    pub spawn: ChargenSpawn,
    pub starting_level: u32,
    #[serde(default)]
    pub starting_stats: Vec<SourceStat>,
    #[serde(default)]
    pub starting_loadout: Vec<SourceLoadoutEntry>,
    #[serde(default)]
    pub starting_abilities: Vec<String>,
    #[serde(default)]
    pub starting_quests: Vec<String>,
    #[serde(default)]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source", default)]
    pub source: Provenance,
}

#[derive(Debug, Deserialize)]
pub struct ChargenSpawn {
    pub zone_id: String,
    pub position: Position,
    pub heading: f32,
}

#[derive(Debug, Deserialize)]
pub struct SourceStat {
    pub stat: String,
    pub value: f32,
}

#[derive(Debug, Deserialize)]
pub struct SourceLoadoutEntry {
    pub item_id: String,
    pub quantity: u32,
    pub slot: String,
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

impl ObjectRef {
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }
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

/// What a build reads from where.
#[derive(Debug)]
pub struct LoadOptions<'a> {
    pub root: &'a Path,
    /// Directory holding `layers.yaml` and one directory per layer. Defaults to
    /// `<root>/overlays`.
    pub overlays_root: PathBuf,
    /// Layer ids to apply. Empty means every default-on layer.
    pub selected_layers: &'a [String],
    pub layout: Layout,
    pub ruleset: &'a str,
    pub zone: Option<&'a str>,
}

impl<'a> LoadOptions<'a> {
    pub fn new(root: &'a Path, layout: Layout, ruleset: &'a str, zone: Option<&'a str>) -> Self {
        Self {
            root,
            overlays_root: root.join("overlays"),
            selected_layers: &[],
            layout,
            ruleset,
            zone,
        }
    }
}

/// The index that makes the item catalogue addressable without walking it.
const ITEM_INDEX_FILE_NAME: &str = "index.yaml";

/// The `id -> relative path` map that makes the item catalogue addressable
/// without walking it.
#[derive(Debug, Default, Deserialize)]
struct ItemIndexDocument {
    #[serde(default)]
    entries: BTreeMap<String, String>,
}

/// Loads every document the compiler understands, merges the overlay layers
/// over the base, and demand-loads the items the result reaches.
pub fn load(options: &LoadOptions<'_>) -> Result<SourceTree> {
    let layers = overlay::select_layers(&options.overlays_root, options.selected_layers)?;

    let mut documents = DocumentSet::default();
    for path in base_files(options)? {
        let value = read_yaml(&path)?;
        documents.insert_base(&path, value)?;
    }

    // Overlay documents are read once and applied in `layers.yaml` order.
    // Item patches are held back: an item's base document is only opened once
    // the reachable set is known, and a patch applied before its base would
    // create a document out of the patch alone.
    let mut applied = Vec::new();
    let mut item_patches: Vec<(String, PathBuf, Value)> = Vec::new();
    let mut skipped_overlay_documents = Vec::new();
    for layer in &layers {
        let files = layer_files(layer)?;
        let mut count = 0;
        for path in files {
            let value = read_yaml(&path)?;
            let (id, _) = overlay::document_metadata(&layer.id, &path, &value)?;
            if let Some(note) = out_of_scope_note(options, &value, &id) {
                skipped_overlay_documents.push(SkippedOverlayDocument {
                    document_id: id,
                    layer: layer.id.clone(),
                    source: path.display().to_string(),
                    note,
                });
                count += 1;
                continue;
            }
            if options.layout == Layout::Ruleset && id.starts_with("item.") {
                item_patches.push((layer.id.clone(), path, value));
            } else {
                documents.apply_overlay(&layer.id, &path, value)?;
            }
            count += 1;
        }
        applied.push(AppliedLayer {
            id: layer.id.clone(),
            description: layer.description.clone(),
            documents: count,
        });
    }

    let mut tree = SourceTree {
        layers: applied,
        skipped_overlay_documents,
        ..SourceTree::default()
    };
    let mut loot_documents: BTreeMap<String, LootTableDocument> = BTreeMap::new();
    let mut item_values: BTreeMap<String, Value> = BTreeMap::new();
    for document in documents.iter() {
        let mut value = clone_without_patch_keys(&document.value);
        let what = document_kind(&value, &document.id);
        // A flat dataset is compiled whole; a ruleset tree holds them back for
        // the reachability pass below.
        match (what.as_str(), options.layout) {
            ("item", Layout::Ruleset) => {
                item_values.insert(document.id.clone(), value);
            }
            ("loot", Layout::Ruleset) => {
                let parsed: LootTableDocument = from_value(&mut value, "loot table", document)?;
                loot_documents.insert(document.id.clone(), parsed);
            }
            _ => file_document(&mut tree, value, &what, document)?,
        }
    }
    tree.notes = documents.notes;
    tree.conflicts = documents.conflicts;
    tree.deleted = documents.deleted;

    if options.layout == Layout::Ruleset {
        // A ruleset tree holds one document per zone in `zones/`. A pack covers
        // exactly one zone (ADR 0029), so the others are not this build's.
        if let Some(zone) = options.zone {
            let wanted = format!("zone.{zone}");
            tree.zones.retain(|document| document.id == wanted);
        }
        select_reachable(
            options,
            &mut tree,
            loot_documents,
            item_values,
            &item_patches,
        )?;
    }

    if tree.placement_documents.is_empty() {
        bail!("{} holds no placement documents", options.root.display());
    }
    Ok(tree)
}

/// Explains why an overlay document is outside a ruleset build's selected
/// zone. A full document's `zone` is authoritative. A partial patch may omit
/// that field, so canonical ids supply the scope for zone-owned kinds.
fn out_of_scope_note(
    options: &LoadOptions<'_>,
    value: &Value,
    document_id: &str,
) -> Option<String> {
    if options.layout != Layout::Ruleset {
        return None;
    }
    let selected = options.zone?;
    let target = explicit_zone(value).or_else(|| zone_from_document_id(document_id))?;
    (target != selected).then(|| {
        format!("target zone {target} is outside the selected zone {selected}; document skipped")
    })
}

fn explicit_zone(value: &Value) -> Option<&str> {
    value
        .get("zone")
        .and_then(Value::as_str)
        .and_then(|zone| zone.strip_prefix("zone."))
}

fn zone_from_document_id(document_id: &str) -> Option<&str> {
    let mut parts = document_id.split('.');
    match parts.next()? {
        "zone" | "spawn" | "mob" | "quest" | "route" | "script" | "trigger" => parts.next(),
        _ => None,
    }
}

/// Chooses the loot tables this zone's mobs reach, then opens exactly the item
/// documents those tables — and the zone's chargen options and quests — name.
fn select_reachable(
    options: &LoadOptions<'_>,
    tree: &mut SourceTree,
    loot_documents: BTreeMap<String, LootTableDocument>,
    item_values: BTreeMap<String, Value>,
    item_patches: &[(String, PathBuf, Value)],
) -> Result<()> {
    let mut wanted_loot: BTreeSet<String> = BTreeSet::new();
    for mob in &tree.mobs {
        if let Some(id) = mob.loot_table.as_ref().and_then(ObjectRef::id) {
            wanted_loot.insert(id.to_string());
        }
    }
    // A loot table an overlay authored is curated on purpose, so it ships even
    // when nothing references it yet.
    for note in &tree.notes {
        if note.layer != overlay::BASE_LAYER && note.document_id.starts_with("loot.") {
            wanted_loot.insert(note.document_id.clone());
        }
    }

    let total_loot = loot_documents.len();
    let reachable: BTreeMap<String, LootTableDocument> = loot_documents
        .into_iter()
        .filter(|(id, _)| wanted_loot.contains(id))
        .collect();
    tree.unreachable_loot_tables = total_loot - reachable.len();

    let mut wanted_items: BTreeSet<String> = BTreeSet::new();
    for table in reachable.values() {
        collect_loot_items(&table.root, &mut wanted_items);
    }
    for option in &tree.chargen_options {
        for entry in &option.starting_loadout {
            wanted_items.insert(entry.item_id.clone());
        }
    }
    for quest in &tree.quests {
        for reward in quest
            .rewards
            .mandatory_items
            .iter()
            .chain(&quest.rewards.alternative_items)
        {
            if let Some(id) = reward.item.id() {
                wanted_items.insert(id.to_string());
            }
        }
        for objective in &quest.objectives {
            for target in &objective.targets {
                if let Some(id) = target.id().filter(|id| id.starts_with("item.")) {
                    wanted_items.insert(id.to_string());
                }
            }
        }
    }
    // A script tree can hand an item to the player (ImpactGiveItem) or take
    // one away; the item document must ship with the pack for the row to mean
    // anything, so script references reach into the catalogue exactly as quest
    // rewards do.
    let mut script_references = Vec::new();
    for document in &tree.quest_scripts {
        for node in document
            .start_impacts
            .iter()
            .chain(&document.trigger_agents)
        {
            collect_script_refs(node, &mut script_references);
        }
    }
    for document in &tree.script_triggers {
        collect_script_refs(&document.root, &mut script_references);
    }
    for reference in script_references {
        if reference.row_type.as_deref() == Some("item") {
            wanted_items.insert(reference.id.clone());
        }
    }
    // An item document that only an overlay supplies is content in its own
    // right, not a patch, so it ships whether or not anything reaches it.
    for (_, _, patch) in item_patches {
        if let Some(id) = patch.get("id").and_then(Value::as_str) {
            wanted_items.insert(id.to_string());
        }
    }

    let index = read_item_index(options)?;
    let mut item_set = DocumentSet::default();
    let mut opened = 0usize;
    for id in &wanted_items {
        if let Some(value) = item_values.get(id) {
            item_set.insert_base(Path::new("<merged>"), value.clone())?;
            continue;
        }
        let Some(relative) = index.get(id) else {
            // Not an error here: the reference checker owns dangling
            // references, and it names the document that asked for this id.
            continue;
        };
        let path = options.root.join(options.ruleset).join(relative);
        let value = read_yaml(&path)?;
        item_set.insert_base(&path, value)?;
        opened += 1;
    }
    for (layer, path, patch) in item_patches {
        item_set.apply_overlay(layer, path, patch.clone())?;
    }
    tree.notes.extend(item_set.notes.iter().cloned());
    tree.conflicts.extend(item_set.conflicts.iter().cloned());

    tree.items_loaded_from_index = opened;
    for document in item_set.iter() {
        let mut value = clone_without_patch_keys(&document.value);
        tree.items.push(from_value(&mut value, "item", document)?);
    }
    tree.loot_tables = reachable.into_values().collect();
    Ok(())
}

fn collect_loot_items(node: &LootNodeDocument, into: &mut BTreeSet<String>) {
    if let Some(id) = node.item.as_ref().and_then(ObjectRef::id) {
        into.insert(id.to_string());
    }
    for child in &node.entries {
        collect_loot_items(child, into);
    }
}

fn read_item_index(options: &LoadOptions<'_>) -> Result<BTreeMap<String, String>> {
    let path = options
        .root
        .join(options.ruleset)
        .join("items")
        .join(ITEM_INDEX_FILE_NAME);
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let text =
        fs::read_to_string(&path).with_context(|| format!("read item index {}", path.display()))?;
    let document: ItemIndexDocument = serde_yaml::from_str(&text)
        .with_context(|| format!("parse item index {}", path.display()))?;
    Ok(document.entries)
}

/// Turns a merged document into a typed one, naming the layer and file the
/// merged form came from when it does not fit.
fn from_value<T: serde::de::DeserializeOwned>(
    value: &mut Value,
    label: &str,
    document: &overlay::MergedDocument,
) -> Result<T> {
    let taken = std::mem::replace(value, Value::Null);
    serde_yaml::from_value(taken).with_context(|| {
        format!(
            "read {label} {} (layer {}, {})",
            document.id,
            document.origin_layer,
            document.origin_path.display()
        )
    })
}

/// Files a merged document into the tree by kind.
///
/// Spawn documents declare their own `kind`. Global resources have no `kind`
/// field, so they are recognised by the first segment of their canonical id,
/// which ADR 0007 makes the document's type tag.
fn document_kind(value: &Value, id: &str) -> String {
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("");
    if kind.is_empty() {
        id.split('.').next().unwrap_or("")
    } else {
        kind
    }
    .to_string()
}

fn file_document(
    tree: &mut SourceTree,
    mut value: Value,
    what: &str,
    document: &overlay::MergedDocument,
) -> Result<()> {
    match what {
        "placements" => {
            tree.placement_documents
                .push(from_value(&mut value, "placements", document)?)
        }
        "table" => tree
            .tables
            .push(from_value(&mut value, "spawn table", document)?),
        "mob" => tree.mobs.push(from_value(&mut value, "mob", document)?),
        "mobkind" | "mobclass" | "mobquality" => {
            let mut parsed: MobKindDocument = from_value(&mut value, "mob kind", document)?;
            parsed.taxonomy = Some(match what {
                "mobclass" => TaxonomyKind::Class,
                "mobquality" => TaxonomyKind::Quality,
                _ => TaxonomyKind::Kind,
            });
            tree.mob_kinds.push(parsed);
        }
        "ability" => tree
            .abilities
            .push(from_value(&mut value, "ability", document)?),
        "faction" => tree
            .factions
            .push(from_value(&mut value, "faction", document)?),
        "chargen" => tree
            .chargen_options
            .push(from_value(&mut value, "chargen option", document)?),
        "quest" => tree.quests.push(from_value(&mut value, "quest", document)?),
        "script" => tree
            .quest_scripts
            .push(from_value(&mut value, "quest script", document)?),
        "trigger" => tree
            .script_triggers
            .push(from_value(&mut value, "script trigger", document)?),
        "route" => tree.routes.push(from_value(&mut value, "route", document)?),
        "locale" => tree
            .locales
            .push(from_value(&mut value, "locale", document)?),
        "levelcurve" => tree
            .level_curves
            .push(from_value(&mut value, "level curve", document)?),
        "zone" => tree.zones.push(from_value(&mut value, "zone", document)?),
        "item" => tree.items.push(from_value(&mut value, "item", document)?),
        "loot" => tree
            .loot_tables
            .push(from_value(&mut value, "loot table", document)?),
        // An id whose first segment names no document type this compiler
        // knows. The item index is the only such file the ruleset tree holds,
        // and it is read separately.
        _ => {}
    }
    Ok(())
}

fn clone_without_patch_keys(value: &Value) -> Value {
    let mut copy = value.clone();
    overlay::strip_patch_keys(&mut copy);
    copy
}

fn read_yaml(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read authored document {}", path.display()))?;
    serde_yaml::from_str(&text)
        .with_context(|| format!("parse authored document {}", path.display()))
}

/// The base-layer files a build reads, in a stable order.
fn base_files(options: &LoadOptions<'_>) -> Result<Vec<PathBuf>> {
    match options.layout {
        Layout::Flat => yaml_files(options.root),
        Layout::Ruleset => {
            let zone = options.zone.ok_or_else(|| {
                anyhow!("a zone slug is required when reading a ruleset source tree")
            })?;
            let ruleset = options.root.join(options.ruleset);
            let zone_root = ruleset.join("zones").join(zone);
            let spawns = zone_root.join("spawns");

            let mut files = yaml_files(&spawns.join("placements"))?;
            files.extend(yaml_files(&spawns.join("tables"))?);
            // Mob records are per-zone in the authored tree. The directory is
            // optional because a zone can be all placements and tables, and a
            // pack with no mob rows is a legal, if unplayable, pack.
            files.extend(optional_yaml_files(&spawns.join("mobs"))?);
            files.extend(optional_yaml_files(&zone_root.join("quests"))?);
            // Script rows (ADR 0036): one quest-script document per quest that
            // carries counters or impact trees, plus the trigger documents they
            // reference. Both are zone-scoped by directory.
            files.extend(optional_yaml_files(
                &zone_root.join("scripts").join("quests"),
            )?);
            files.extend(optional_yaml_files(
                &zone_root.join("scripts").join("triggers"),
            )?);
            files.extend(optional_yaml_files(&zone_root.join("routes"))?);
            // The zone's own document sits beside its directory when the
            // extractor has written one.
            files.extend(optional_yaml_files(&ruleset.join("zones"))?);

            // Ruleset-global resources. None of them belongs to a zone: an
            // item is not owned by the zone whose mob happens to drop it.
            for global in [
                "chargen",
                "loot",
                "factions",
                "mobkinds",
                "mobclasses",
                "mobqualities",
                "abilities",
                "levelcurves",
            ] {
                files.extend(optional_yaml_files(&ruleset.join(global))?);
            }
            // Locale packs are nested one directory per language.
            files.extend(optional_yaml_tree(&ruleset.join("locale"))?);
            // Items directly under `items/` only. The catalogue proper lives in
            // subdirectories and is reached through `index.yaml`, which is a
            // build input rather than a content document and is read once, by
            // `read_item_index`.
            let mut item_files = optional_yaml_files(&ruleset.join("items"))?;
            item_files.retain(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_none_or(|name| name != ITEM_INDEX_FILE_NAME)
            });
            files.extend(item_files);
            Ok(files)
        }
    }
}

/// Every `*.yaml` under a layer directory, recursively: a layer mirrors
/// whatever shape the tree it patches has.
fn layer_files(layer: &Layer) -> Result<Vec<PathBuf>> {
    let mut files = yaml_tree(&layer.directory)?;
    files.retain(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_none_or(|name| name != overlay::LAYERS_FILE_NAME)
    });
    Ok(files)
}

fn optional_yaml_files(directory: &Path) -> Result<Vec<PathBuf>> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    yaml_files(directory)
}

fn optional_yaml_tree(directory: &Path) -> Result<Vec<PathBuf>> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    yaml_tree(directory)
}

/// Lists `*.yaml` files directly inside `directory`, sorted by path so that two
/// runs on the same tree see the same order.
fn yaml_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(directory)
        .with_context(|| format!("read source directory {}", directory.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("list {}", directory.display()))?;
        let path = entry.path();
        if path.is_file() && is_yaml(&path) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Like [`yaml_files`], but recursive.
fn yaml_tree(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in WalkDir::new(directory).sort_by_file_name() {
        let entry = entry.with_context(|| format!("walk {}", directory.display()))?;
        if entry.file_type().is_file() && is_yaml(entry.path()) {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
}

fn is_yaml(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("yaml"))
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
