//! A tiny authored source tree, written per test so the suite never depends on
//! the private `data` repository.
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
    let spawns = root.join("classic").join("zones").join(ZONE).join("spawns");
    let placements = spawns.join("placements");
    let tables = spawns.join("tables");
    fs::create_dir_all(&placements).expect("create placements directory");
    fs::create_dir_all(&tables).expect("create tables directory");

    fs::write(
        tables.join("sparrows.yaml"),
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
    )
    .expect("write table document");

    fs::write(
        placements.join("quay.yaml"),
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
  - id: spawn.harbour-watch.placement.quay.2
    object:
      id: mob.harbour-watch.gull
      href: /Demo/Creatures/Gull.xdb#mob
    position: { x: -8.0, y: 0.5, z: 3.0 }
    orientation: { yaw: 2.0, pitch: 0.0, roll: 0.0 }
    spawn_time: time-never
"#,
    )
    .expect("write placements document");

    root.to_path_buf()
}

/// Build options for the tree [`write_source`] writes.
pub fn options(source: PathBuf, out: PathBuf) -> BuildOptions {
    BuildOptions {
        source,
        overlays: Vec::new(),
        out,
        layout: Layout::Ruleset,
        ruleset: "classic".to_string(),
        zone: Some(ZONE.to_string()),
        keep_extra: false,
        player_spawn: None,
        source_repo: "data".to_string(),
        source_commit: Some("a".repeat(40)),
    }
}

/// Writes a flat, `data-schemas/demo`-shaped tree under `root`: one zone's
/// placements plus the global resources combat reads.
pub fn write_flat_source(root: &Path) -> PathBuf {
    fs::create_dir_all(root).expect("create flat source directory");

    fs::write(
        root.join("faction.wild.yaml"),
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
    )
    .expect("write faction document");

    fs::write(
        root.join("ability.cleave.yaml"),
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
    )
    .expect("write ability document");

    fs::write(
        root.join("mobkind.crab.yaml"),
        r#"schema_version: 1
id: mobkind.base.crab
kind: mobkind
source_type: demo.MobKindTemplate
hp_mod: 0.5
"#,
    )
    .expect("write mob kind document");

    fs::write(
        root.join("mob.crab.yaml"),
        format!(
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
    )
    .expect("write mob document");

    fs::write(
        root.join("spawn.placements.yaml"),
        format!(
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
    )
    .expect("write placements document");

    root.to_path_buf()
}

/// Build options for the tree [`write_flat_source`] writes.
pub fn flat_options(source: PathBuf, out: PathBuf) -> BuildOptions {
    BuildOptions {
        source,
        overlays: Vec::new(),
        out,
        layout: Layout::Flat,
        ruleset: "classic".to_string(),
        zone: Some(ZONE.to_string()),
        keep_extra: false,
        player_spawn: None,
        source_repo: "data-schemas".to_string(),
        source_commit: Some("b".repeat(40)),
    }
}
