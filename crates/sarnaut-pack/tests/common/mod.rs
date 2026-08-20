//! A tiny authored source tree, written per test so the suite never depends on
//! the private `data` repository.
//!
//! The tree is deliberately **reference-complete**: every id it mentions has a
//! document. `sarnaut-pack` fails a build with a dangling reference, so a
//! fixture that named a mob it never wrote would make every test here a test of
//! the reference checker.
//!
//! Every integration test binary compiles this module in full, so a helper only
//! `build.rs` uses looks dead to `verify.rs` and vice versa. The allow is about
//! how Cargo builds test binaries, not about unused code.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use sarnaut_pack::compile::BuildOptions;
use sarnaut_pack::source::Layout;

pub const ZONE: &str = "harbour-watch";

/// Writes a `data`-shaped tree under `root` and returns the source root.
pub fn write_source(root: &Path) -> PathBuf {
    let classic = root.join("classic");
    let zone_root = classic.join("zones").join(ZONE);
    let spawns = zone_root.join("spawns");
    for directory in [
        &spawns.join("placements"),
        &spawns.join("tables"),
        &spawns.join("mobs"),
        &zone_root.join("quests"),
        &zone_root.join("routes"),
        &classic.join("factions"),
        &classic.join("abilities"),
        &classic.join("mobkinds"),
        &classic.join("mobclasses"),
        &classic.join("mobqualities"),
        &classic.join("locale").join("en"),
    ] {
        fs::create_dir_all(directory).expect("create source directory");
    }

    write(
        &spawns.join("tables").join("sparrows.yaml"),
        r#"schema_version: 1
id: spawn.harbour-watch.table.sparrows
kind: table
zone: zone.harbour-watch
source_type: demo.SpawnTable
scripted: true
entries:
  - group: commons
    object:
      id: mob.harbour-watch.sparrow
      href: /Demo/Creatures/Sparrow.xdb#mob
    chance: 1.0
    spawn_time: time-range
extra:
  commonsLimit: '3'
  leashData:
    globalLeash: 'false'
"#,
    );

    write(
        &spawns.join("placements").join("quay.yaml"),
        r#"schema_version: 1
id: spawn.harbour-watch.placements.quay
kind: placements
zone: zone.harbour-watch
map: quay
source_type: demo.PatchObjects
placements:
  - id: spawn.harbour-watch.placement.quay.1
    object:
      id: spawn.harbour-watch.table.sparrows
      href: /Demo/Maps/Quay/SpawnTables/Sparrows.xdb#table
    position: { x: 1.5, y: 2.25, z: 3.0 }
    orientation: { yaw: 0.75, pitch: 0.0, roll: 0.0 }
    spawn_time: time-once
    route: route.harbour-watch.quay
  - id: spawn.harbour-watch.placement.quay.2
    object:
      id: mob.harbour-watch.gull
      href: /Demo/Creatures/Gull.xdb#mob
    position: { x: -8.0, y: 0.5, z: 3.0 }
    orientation: { yaw: 2.0, pitch: 0.0, roll: 0.0 }
    spawn_time: time-never
"#,
    );

    write_sparrow(root, None);
    write(
        &spawns.join("mobs").join("gull.yaml"),
        r#"schema_version: 1
id: mob.harbour-watch.gull
kind: mob
zone: zone.harbour-watch
source_type: demo.MobWorldResource
loc_ref:
  name: Gull.Name.txt
mob_kind:
  id: mobkind.base.harbour
quality:
  id: mobquality.common
faction:
  id: faction.wild
level_min: 1
level_max: 1
walk_speed: 3.0
"#,
    );

    write(
        &zone_root.join("quests").join("first.yaml"),
        r#"schema_version: 1
id: quest.harbour-watch.first
zone: zone.harbour-watch
source_type: demo.QuestResource
loc_ref:
  name: FirstQuest.Name.txt
level: 1
required_level: 1
quest_type: quest-type-solo
starter:
  id: mob.harbour-watch.gull
finisher:
  id: mob.harbour-watch.gull
objectives:
  - kind: quest-count-kill
    limit: 3
    show_count: true
    targets:
      - id: mob.harbour-watch.sparrow
rewards:
  experience: 8
flags:
  can_cancel: false
"#,
    );

    write(
        &zone_root.join("routes").join("quay.yaml"),
        r#"schema_version: 1
id: route.harbour-watch.quay
zone: zone.harbour-watch
map: quay
source_type: demo.RouteResource
points:
  - index: 0
    position: { x: 0.0, y: 0.0, z: 0.0 }
  - index: 1
    position: { x: 4.0, y: 0.0, z: 0.0 }
links:
  - from: 0
    to: 1
    weight: 1.0
    movement: walk
"#,
    );

    for (name, id, stance) in [
        ("wild.yaml", "faction.wild", "hostile"),
        ("league.yaml", "faction.league", "neutral"),
        ("empire.yaml", "faction.empire", "neutral"),
    ] {
        write(
            &classic.join("factions").join(name),
            &format!(
                r#"schema_version: 1
id: {id}
source_type: demo.FactionResource
attackable: true
default_stance: {stance}
"#
            ),
        );
    }

    write(
        &classic.join("abilities").join("cleave.yaml"),
        r#"schema_version: 1
id: ability.melee.cleave
source_type: demo.SpellResource
loc_ref:
  name: Cleave.Name.txt
target: enemy
range_m: 10.0
triggers_gcd: true
effects:
  - kind: damage
    element: physical
    amount: 18
    attack_power_coeff: 0.5
"#,
    );

    write(
        &classic.join("mobkinds").join("harbour.yaml"),
        r#"schema_version: 1
id: mobkind.base.harbour
kind: mobkind
source_type: demo.MobKindTemplate
mob_class:
  id: mobclass.beast
quality:
  id: mobquality.common
hp_mod: 0.5
dps_mod: 0.25
exp_mod: 1.5
loot_mod: 0.33
speed: 1.25
"#,
    );
    write(
        &classic.join("mobclasses").join("beast.yaml"),
        r#"schema_version: 1
id: mobclass.beast
kind: mobclass
source_type: demo.MobClass
"#,
    );
    write(
        &classic.join("mobqualities").join("common.yaml"),
        r#"schema_version: 1
id: mobquality.common
kind: mobquality
rank: 2
source_type: demo.MobQuality
"#,
    );

    write(
        &classic.join("locale").join("en").join("harbour.yaml"),
        r#"schema_version: 1
id: locale.en.harbour
language: en
source_root: demo-client
source_type: demo.LocPack
entries:
  - key: Sparrow.Name.txt
    text: Harbour Sparrow
  - key: Gull.Name.txt
    text: Quay Gull
  - key: Cleave.Name.txt
    text: Cleave
  - key: FirstQuest.Name.txt
    text: A First Errand
  - key: HarbourWarrior.Name.txt
    text: Harbour Warrior
  - key: HarbourWarrior.Description.txt
    text: Melee starter. Hits things until they stop.
  - key: HarbourTonic.Name.txt
    text: Harbour Tonic
  - key: HarbourShell.Name.txt
    text: Harbour Shell
"#,
    );

    root.to_path_buf()
}

/// Rewrites the sparrow mob, optionally pointing it at a loot table. The loot
/// link is what makes a loot table reachable, and reachability is what selects
/// it out of a ruleset tree.
fn write_sparrow(root: &Path, loot_table: Option<&str>) {
    let loot = loot_table
        .map(|id| format!("loot_table:\n  id: {id}\n"))
        .unwrap_or_default();
    write(
        &root
            .join("classic")
            .join("zones")
            .join(ZONE)
            .join("spawns")
            .join("mobs")
            .join("sparrow.yaml"),
        &format!(
            r#"schema_version: 1
id: mob.harbour-watch.sparrow
kind: mob
zone: zone.harbour-watch
source_type: demo.MobWorldResource
loc_ref:
  name: Sparrow.Name.txt
mob_kind:
  id: mobkind.base.harbour
quality:
  id: mobquality.common
faction:
  id: faction.wild
level_min: 2
level_max: 2
walk_speed: 2.0
aggro_radius_m: 12.0
leash_radius_m: 40.0
abilities:
  - id: ability.melee.cleave
{loot}"#
        ),
    );
}

/// The two item documents the chargen and loot fixtures reference. Written by
/// both, with identical bytes, so a test may call either or both.
fn write_items(source: &Path) {
    let items = source.join("classic").join("items");
    fs::create_dir_all(&items).expect("create items directory");
    write(
        &items.join("tonic.yaml"),
        r#"schema_version: 1
id: item.consumable.harbour-tonic
category: consumable
source_type: demo.ItemResource
loc_ref:
  name: HarbourTonic.Name.txt
level: 2
stack_limit: 20
vendor_price:
  sell: 4
  buy: 16
"#,
    );
    write(
        &items.join("shell.yaml"),
        r#"schema_version: 1
id: item.junk.harbour-shell
category: junk
source_type: demo.ItemResource
loc_ref:
  name: HarbourShell.Name.txt
stack_limit: 1
"#,
    );
}

/// Adds the ruleset-global chargen documents of ADR 0032 to a tree
/// [`write_source`] already wrote: one playable option and one that is not.
pub fn write_chargen(source: &Path) {
    let chargen = source.join("classic").join("chargen");
    fs::create_dir_all(&chargen).expect("create chargen directory");
    write_items(source);

    write(
        &chargen.join("league-warrior.yaml"),
        r#"schema_version: 1
id: chargen.league.warrior
source_type: demo.ChargenOption
race: race.human
class: class.warrior
sex: female
faction: faction.league
enabled: true
loc_ref:
  name: HarbourWarrior.Name.txt
  description: HarbourWarrior.Description.txt
visual_ref: Demo/Visuals/HarbourWarrior.gd
spawn:
  zone_id: zone.harbour-watch
  position: { x: 12.0, y: 4.5, z: 0.5 }
  heading: 1.5
starting_level: 3
starting_stats:
  - stat: strength
    value: 12
starting_loadout:
  - item_id: item.consumable.harbour-tonic
    quantity: 3
    slot: bag
starting_abilities:
  - ability.melee.cleave
starting_quests:
  - quest.harbour-watch.first
"#,
    );

    write(
        &chargen.join("empire-warrior.yaml"),
        r#"schema_version: 1
id: chargen.empire.warrior
source_type: demo.ChargenOption
race: race.orc
class: class.warrior
sex: male
faction: faction.empire
enabled: false
visual_ref: Demo/Visuals/EmpireWarrior.gd
spawn:
  zone_id: zone.harbour-watch
  position: { x: -2.0, y: 0.0, z: 0.0 }
  heading: 0.0
starting_level: 1
"#,
    );
}

/// Adds the loot documents of `mechanics/loot.md` to a tree
/// [`write_source`] already wrote: two items, one nested tree that grants them,
/// and the mob link that makes the tree reachable.
pub fn write_loot(source: &Path) {
    let loot = source.join("classic").join("loot");
    fs::create_dir_all(&loot).expect("create loot directory");
    write_items(source);
    write_sparrow(source, Some(LOOT_TABLE_ID));

    write(
        &loot.join("nested.yaml"),
        r#"schema_version: 1
id: loot.fixture.nested
source_type: demo.LootTableResource
root:
  node: and
  chances: [1.0, 0.00618751]
  entries:
    - node: money
      min_number: 2
      max_number: 4
    - node: or
      chances: [0.3, 0.7]
      entries:
        - node: single-item
          item:
            id: item.junk.harbour-shell
          min_number: 1
          max_number: 3
        - node: single-item
          item:
            id: item.consumable.harbour-tonic
          min_number: 45
          max_number: 45
"#,
    );
}

/// The id [`write_loot`] writes and the sparrow points at. A negative fixture
/// that rewrites the loot document must keep this id, or the mob's link dangles
/// and the reference checker reports that instead of the structural fault under
/// test.
pub const LOOT_TABLE_ID: &str = "loot.fixture.nested";

/// Overwrites the loot document, keeping its id so the mob link still resolves.
pub fn rewrite_loot_table(source: &Path, body: &str) {
    write(
        &source.join("classic").join("loot").join("nested.yaml"),
        body,
    );
}

/// Writes an overlay layer under `<root>/overlays/<id>` and registers it in
/// `layers.yaml`, which is the only thing that makes a layer exist.
pub fn write_layer(root: &Path, id: &str, documents: &[(&str, &str)]) {
    write_layer_documents(root, id, documents);
    let manifest = root.join("overlays").join("layers.yaml");
    let mut text = fs::read_to_string(&manifest).unwrap_or_else(|_| "layers:\n".to_string());
    text.push_str(&format!("  - id: {id}\n    description: test layer\n"));
    fs::write(&manifest, text).expect("write layers.yaml");
}

/// Writes a layer's documents without registering it, so a test can control
/// `layers.yaml` itself.
pub fn write_layer_documents(root: &Path, id: &str, documents: &[(&str, &str)]) {
    let directory = root.join("overlays").join(id);
    fs::create_dir_all(&directory).expect("create overlay layer directory");
    for (name, body) in documents {
        write(&directory.join(name), body);
    }
}

/// Writes `layers.yaml` from scratch: the order here is the order layers apply.
pub fn write_layers_manifest(root: &Path, layers: &[(&str, bool)]) {
    let overlays = root.join("overlays");
    fs::create_dir_all(&overlays).expect("create overlays root");
    let mut text = String::from("layers:\n");
    for (id, apply_by_default) in layers {
        text.push_str(&format!(
            "  - id: {id}\n    description: test layer\n    apply_by_default: {apply_by_default}\n"
        ));
    }
    fs::write(overlays.join("layers.yaml"), text).expect("write layers.yaml");
}

fn write(path: &Path, body: &str) {
    fs::write(path, body).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

pub fn options(source: PathBuf) -> BuildOptions {
    BuildOptions {
        source,
        overlays_root: None,
        overlays: Vec::new(),
        layout: Layout::Ruleset,
        ruleset: "classic".to_string(),
        zone: Some(ZONE.to_string()),
        keep_extra: false,
        player_spawn: None,
        allow_overlay_conflicts: false,
        require_locale: false,
        source_repo: "data".to_string(),
        source_commit: Some("a".repeat(40)),
    }
}

/// Writes a flat, `data-schemas/demo`-shaped tree under `root`: one zone's
/// placements plus the global resources combat reads.
pub fn write_flat_source(root: &Path) -> PathBuf {
    fs::create_dir_all(root).expect("create flat source directory");

    write(
        &root.join("faction.wild.yaml"),
        r#"schema_version: 1
id: faction.wild
source_type: demo.FactionResource
loc_ref:
  name: FactionWild.Name.txt
player_faction: false
attackable: true
default_stance: hostile
relations:
  - faction: faction.league
    stance: hostile
"#,
    );

    write(
        &root.join("faction.league.yaml"),
        r#"schema_version: 1
id: faction.league
source_type: demo.FactionResource
loc_ref:
  name: FactionLeague.Name.txt
player_faction: true
attackable: false
default_stance: neutral
"#,
    );

    write(
        &root.join("locale.en.yaml"),
        r#"schema_version: 1
id: locale.en.demo
language: en
source_root: demo-client
source_type: demo.LocPack
entries:
  - key: Cleave.Name.txt
    text: Cleave
  - key: Crab.Name.txt
    text: Quay Crab
  - key: FactionWild.Name.txt
    text: Wild
  - key: FactionLeague.Name.txt
    text: League
"#,
    );

    write(
        &root.join("ability.cleave.yaml"),
        r#"schema_version: 1
id: ability.melee.cleave
source_type: demo.SpellResource
loc_ref:
  name: Cleave.Name.txt
target: enemy
range_m: 10.0
cast_time_ms: 0
cooldown_ms: 0
triggers_gcd: true
effects:
  - kind: damage
    element: physical
    amount: 18
    attack_power_coeff: 0.5
"#,
    );

    write(
        &root.join("mobkind.crab.yaml"),
        r#"schema_version: 1
id: mobkind.base.crab
kind: mobkind
source_type: demo.MobKindTemplate
hp_mod: 0.5
"#,
    );

    write(
        &root.join("mob.crab.yaml"),
        &format!(
            r#"schema_version: 1
id: mob.{ZONE}.crab
kind: mob
zone: zone.{ZONE}
source_type: demo.MobWorldResource
loc_ref:
  name: Crab.Name.txt
mob_kind:
  id: mobkind.base.crab
faction:
  id: faction.wild
level_min: 2
level_max: 2
walk_speed: 2.0
aggro_radius_m: 12.0
leash_radius_m: 40.0
abilities:
  - id: ability.melee.cleave
"#
        ),
    );

    write(
        &root.join("spawn.placements.yaml"),
        &format!(
            r#"schema_version: 1
id: spawn.{ZONE}.placements.quay
kind: placements
zone: zone.{ZONE}
map: quay
source_type: demo.MapPlacements
placements:
  - id: placement.{ZONE}.crab-1
    object:
      id: mob.{ZONE}.crab
    position: {{ x: 4.0, y: 0.0, z: 0.0 }}
    spawn_time: once
    respawn_delay_ms:
      min: 10000
      max: 14000
"#
        ),
    );

    root.to_path_buf()
}

/// Build options for the tree [`write_flat_source`] writes.
pub fn flat_options(source: PathBuf) -> BuildOptions {
    BuildOptions {
        source,
        overlays_root: None,
        overlays: Vec::new(),
        layout: Layout::Flat,
        ruleset: "classic".to_string(),
        zone: Some(ZONE.to_string()),
        keep_extra: false,
        player_spawn: None,
        allow_overlay_conflicts: false,
        require_locale: false,
        source_repo: "data-schemas".to_string(),
        source_commit: Some("b".repeat(40)),
    }
}
