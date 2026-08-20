//! A tiny authored source tree, written per test so the suite never depends on
//! the private `data` repository.

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
