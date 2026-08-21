use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ExtractionOptions {
    pub src: PathBuf,
    pub out: PathBuf,
    pub dry_run: bool,
    pub schema_dir: Option<PathBuf>,
}

/// Locale extraction reads a second source root, because the reference tree ships
/// only a fraction of the display strings its own resources point at.
#[derive(Debug, Clone)]
pub struct LocaleOptions {
    pub common: ExtractionOptions,
    pub language: String,
    pub supplemental_src: Option<PathBuf>,
}

#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct ItemSummary {
    pub emitted: usize,
    pub skipped_auxiliary: usize,
    pub unchanged: usize,
    pub categories: BTreeMap<String, usize>,
}

#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct ZoneSummary {
    pub zone: String,
    pub map: String,
    pub quests: usize,
    pub spawn_tables: usize,
    pub spawn_points: usize,
    pub locators: usize,
    pub mobs: usize,
    pub routes: usize,
    pub unchanged: usize,
}

#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct MobKindSummary {
    pub zone: String,
    pub mob_worlds: usize,
    pub mob_kinds: usize,
    pub prototypes: usize,
    pub mob_classes: usize,
    pub mob_qualities: usize,
    pub factions: usize,
    pub unchanged: usize,
}

#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct LootSummary {
    pub zone: String,
    pub tables: usize,
    pub nodes: usize,
    pub item_index: usize,
    pub dangling_items: Vec<String>,
    pub unchanged: usize,
}

#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct LocaleSummary {
    pub zone: String,
    pub language: String,
    pub requested: usize,
    pub resolved: usize,
    pub unresolved: Vec<String>,
    pub from_primary: usize,
    pub from_supplemental: usize,
    pub mismatched: Vec<String>,
    /// How many payloads each detected encoding accounted for.
    pub encodings: BTreeMap<String, usize>,
    pub unchanged: usize,
}

impl LocaleSummary {
    /// Share of distinct `loc_ref` targets no source root could supply, in percent.
    pub fn unresolved_rate(&self) -> f64 {
        if self.requested == 0 {
            return 0.0;
        }
        (self.unresolved.len() as f64) * 100.0 / (self.requested as f64)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub path: String,
    pub blake3: String,
    pub extractor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_root: Option<String>,
    /// Prototype documents merged into this one, in application order, ending with
    /// this document's own id. Only prototype-resolving extractors set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prototype_chain: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub href: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocRefs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl LocRefs {
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.description.is_none()
            && self.bind_description.is_none()
            && self.goal.is_none()
            && self.start.is_none()
            && self.check.is_none()
            && self.finish.is_none()
            && self.title.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemDocument {
    pub schema_version: u8,
    pub id: String,
    pub category: String,
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sys_name: Option<String>,
    #[serde(skip_serializing_if = "LocRefs::is_empty")]
    pub loc_ref: LocRefs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visual_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_level: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<ResourceRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_class: Option<ResourceRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_category: Option<ResourceRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack_limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor_price: Option<VendorPrice>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<StatBonus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub combat: Option<CombatStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spell_ref: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub flags: BTreeMap<String, bool>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source")]
    pub source: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VendorPrice {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sell: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buy: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatBonus {
    pub stat: String,
    pub value: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CombatStats {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub armor: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_damage: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_damage: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weapon_speed: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spell_power: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damage_element: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resist_divine: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resist_elemental: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resist_nature: Option<f64>,
}

impl CombatStats {
    pub fn is_empty(&self) -> bool {
        self.armor.is_none()
            && self.min_damage.is_none()
            && self.max_damage.is_none()
            && self.weapon_speed.is_none()
            && self.spell_power.is_none()
            && self.damage_element.is_none()
            && self.resist_divine.is_none()
            && self.resist_elemental.is_none()
            && self.resist_nature.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestDocument {
    pub schema_version: u8,
    pub id: String,
    pub zone: String,
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_name: Option<String>,
    #[serde(skip_serializing_if = "LocRefs::is_empty")]
    pub loc_ref: LocRefs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_level: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quest_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plotline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starter: Option<ResourceRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finisher: Option<ResourceRef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prerequisites: Vec<QuestPrerequisite>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub objectives: Vec<QuestObjective>,
    pub rewards: QuestRewards,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub flags: BTreeMap<String, bool>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source")]
    pub source: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestPrerequisite {
    pub quest: ResourceRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestObjective {
    pub objective_id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loc_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_count: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<ResourceRef>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuestRewards {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experience: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub money: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub honor: Option<i64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mandatory_items: Vec<RewardItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alternative_items: Vec<RewardItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reputations: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewardItem {
    pub item: ResourceRef,
    pub count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnTableDocument {
    pub schema_version: u8,
    pub id: String,
    pub kind: String,
    pub zone: String,
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scripted: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<SpawnEntry>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source")]
    pub source: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnEntry {
    pub group: String,
    pub object: ResourceRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spawn_time: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobDocument {
    pub schema_version: u8,
    pub id: String,
    pub kind: String,
    pub zone: String,
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<u64>,
    #[serde(skip_serializing_if = "LocRefs::is_empty")]
    pub loc_ref: LocRefs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visual_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mob_kind: Option<ResourceRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faction: Option<ResourceRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<ResourceRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level_min: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level_max: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub walk_speed: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub quests: Vec<ResourceRef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub abilities: Vec<ResourceRef>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source")]
    pub source: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacementDocument {
    pub schema_version: u8,
    pub id: String,
    pub kind: String,
    pub zone: String,
    pub map: String,
    pub map_resource: String,
    pub source_type: String,
    pub placements: Vec<SpawnPlacement>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub locators: Vec<MapLocator>,
    #[serde(rename = "_source")]
    pub source: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapLocator {
    pub script_id: String,
    pub position: Position,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnPlacement {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_id: Option<String>,
    pub object: ResourceRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<Orientation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tuner_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spawn_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan_radius: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Orientation {
    pub yaw: f64,
    pub pitch: f64,
    pub roll: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteDocument {
    pub schema_version: u8,
    pub id: String,
    pub zone: String,
    pub map: String,
    pub source_type: String,
    pub points: Vec<RoutePoint>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<RouteLink>,
    #[serde(rename = "_source")]
    pub source: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutePoint {
    pub index: usize,
    pub position: Position,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteLink {
    pub from: usize,
    pub to: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub movement: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flying: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobKindDocument {
    pub schema_version: u8,
    pub id: String,
    pub kind: String,
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<u64>,
    #[serde(skip_serializing_if = "LocRefs::is_empty")]
    pub loc_ref: LocRefs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prototype: Option<ResourceRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mob_class: Option<ResourceRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<ResourceRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hp_mod: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dps_mod: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp_mod: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mana_mod: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loot_mod: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source")]
    pub source: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobClassDocument {
    pub schema_version: u8,
    pub id: String,
    pub kind: String,
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<u64>,
    #[serde(skip_serializing_if = "LocRefs::is_empty")]
    pub loc_ref: LocRefs,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source")]
    pub source: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobQualityDocument {
    pub schema_version: u8,
    pub id: String,
    pub kind: String,
    pub rank: u8,
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<u64>,
    #[serde(skip_serializing_if = "LocRefs::is_empty")]
    pub loc_ref: LocRefs,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source")]
    pub source: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactionDocument {
    pub schema_version: u8,
    pub id: String,
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<u64>,
    #[serde(skip_serializing_if = "LocRefs::is_empty")]
    pub loc_ref: LocRefs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player_faction: Option<bool>,
    pub attackable: bool,
    pub default_stance: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<FactionRelation>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source")]
    pub source: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactionRelation {
    pub faction: String,
    pub stance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LootTableDocument {
    pub schema_version: u8,
    pub id: String,
    pub source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<u64>,
    pub root: LootNode,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source")]
    pub source: Provenance,
}

/// One loot node. `entries` and `chances` are positionally paired; the extractor
/// rejects a length mismatch outright rather than truncating to the shorter array,
/// because a truncated tree still validates and would drop drops silently.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LootNode {
    Container {
        node: String,
        entries: Vec<LootNode>,
        chances: Vec<f64>,
    },
    SingleItem {
        node: String,
        item: ResourceRef,
        min_number: i64,
        max_number: i64,
    },
    Money {
        node: String,
        min_number: i64,
        max_number: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemIndexDocument {
    pub schema_version: u8,
    pub id: String,
    pub kind: String,
    pub count: usize,
    /// Canonical item id to the ruleset-relative YAML that defines it.
    pub entries: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocaleDocument {
    pub schema_version: u8,
    pub id: String,
    pub language: String,
    pub source_root: String,
    pub source_type: String,
    pub entries: Vec<LocaleEntry>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
    #[serde(rename = "_source")]
    pub source: Provenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocaleEntry {
    pub key: String,
    pub text: String,
    /// Set only when this key came from a root other than the document's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_root: Option<String>,
}
