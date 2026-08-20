//! End-to-end behaviour of `sarnaut-pack build`.

mod common;

use std::fs;

use prost::Message;
use sarnaut_pack::compile::{self, PlayerSpawn};
use sarnaut_pack::manifest::Manifest;
use sarnaut_pack::proto;
use sarnaut_pack::table;

#[test]
fn two_builds_of_one_source_tree_are_byte_identical() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));

    let first = workspace.path().join("first");
    let second = workspace.path().join("second");
    let left = compile::build(&common::options(source.clone()), &first).expect("first build");
    let right = compile::build(&common::options(source), &second).expect("second build");

    assert_eq!(
        left.pack_id(),
        right.pack_id(),
        "pack_id is not reproducible"
    );
    for relative in [
        "manifest.json",
        "tables/zone.sptbl",
        "tables/placements.sptbl",
        "tables/spawn-tables.sptbl",
        "tables/abilities.sptbl",
        "tables/factions.sptbl",
        "tables/mobs.sptbl",
    ] {
        assert_eq!(
            fs::read(first.join(relative)).expect("read first"),
            fs::read(second.join(relative)).expect("read second"),
            "{relative} differs between builds"
        );
    }
}

#[test]
fn manifest_records_every_table_and_its_digest() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    let report = compile::build(&common::options(source), &out).expect("build");

    let document: Manifest = serde_json::from_str(
        &fs::read_to_string(out.join("manifest.json")).expect("read manifest"),
    )
    .expect("parse manifest");

    assert_eq!(document.schema_version, 1);
    assert_eq!(document.zone, common::ZONE);
    assert_eq!(document.pack_id, report.pack_id());
    assert!(!document.keep_extra);
    assert_eq!(
        document
            .tables
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "abilities",
            "factions",
            "locale",
            "mob-kinds",
            "mobs",
            "placements",
            "quests",
            "routes",
            "spawn-tables",
            "zone"
        ],
        "manifest tables are not sorted by name"
    );
    for entry in &document.tables {
        let bytes = fs::read(out.join(&entry.file)).expect("read table");
        assert_eq!(bytes.len() as u64, entry.bytes);
        assert_eq!(blake3::hash(&bytes).to_hex().to_string(), entry.blake3);
    }
}

#[test]
fn manifest_json_is_canonical() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");

    let text = fs::read_to_string(out.join("manifest.json")).expect("read manifest");
    assert!(text.ends_with("}\n"), "manifest has no trailing newline");
    assert!(!text.contains('\r'), "manifest uses CRLF line endings");
    assert!(
        text.contains("\n  \"ruleset\""),
        "manifest is not indented with two spaces"
    );
}

#[test]
fn extra_passthrough_is_stripped_by_default() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");

    for row in decode_spawn_tables(&out) {
        assert!(
            row.extra.is_empty(),
            "spawn table {} carries an extra passthrough in a default build",
            row.id
        );
    }
}

#[test]
fn keep_extra_records_the_flag_and_the_values() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    let mut options = common::options(source);
    options.keep_extra = true;
    compile::build(&options, &out).expect("build");

    let document: Manifest = serde_json::from_str(
        &fs::read_to_string(out.join("manifest.json")).expect("read manifest"),
    )
    .expect("parse manifest");
    assert!(document.keep_extra, "manifest does not record --keep-extra");

    let rows = decode_spawn_tables(&out);
    let table = rows.first().expect("one spawn table row");
    assert_eq!(
        table.extra.get("commonsLimit").map(String::as_str),
        Some("\"3\"")
    );
    assert_eq!(
        table.extra.get("leashData").map(String::as_str),
        Some(r#"{"globalLeash":"false"}"#)
    );
}

#[test]
fn keeping_extra_changes_pack_id() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));

    let out = workspace.path().join("plain");
    let kept_out = workspace.path().join("kept");
    let plain = compile::build(&common::options(source.clone()), &out).expect("plain build");
    let mut options = common::options(source);
    options.keep_extra = true;
    let kept = compile::build(&options, &kept_out).expect("keep-extra build");

    assert_ne!(
        plain.pack_id(),
        kept.pack_id(),
        "a pack that carries extra rows must not share a digest with one that does not"
    );
}

#[test]
fn player_spawn_defaults_to_the_first_live_placement() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");

    let zone = decode_zone(&out);
    let spawn = zone.player_spawn.expect("zone row carries a player spawn");
    assert_eq!((spawn.x, spawn.y, spawn.z), (1.5, 2.25, 3.0));
    assert_eq!(zone.slug, common::ZONE);
    assert_eq!(zone.ruleset, "classic");
}

#[test]
fn explicit_player_spawn_wins() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    let mut options = common::options(source);
    options.player_spawn = Some(PlayerSpawn {
        x: -1.0,
        y: 4.0,
        z: 9.5,
        yaw: 2.5,
    });
    compile::build(&options, &out).expect("build");

    let zone = decode_zone(&out);
    let spawn = zone.player_spawn.expect("zone row carries a player spawn");
    assert_eq!((spawn.x, spawn.y, spawn.z), (-1.0, 4.0, 9.5));
    assert_eq!(zone.player_spawn_heading, 2.5);
}

#[test]
fn placements_keep_their_authored_spawn_time() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");

    let bytes = fs::read(out.join("tables/placements.sptbl")).expect("read placements");
    let rows: Vec<proto::Placement> = table::rows(&bytes)
        .expect("rows")
        .into_iter()
        .map(|row| proto::Placement::decode(row).expect("decode placement"))
        .collect();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].spawn_time, "time-once");
    assert_eq!(rows[1].spawn_time, "time-never");
    assert_eq!(rows[1].object_id, "mob.harbour-watch.gull");
}

#[test]
fn a_placement_referencing_nothing_fails_the_build() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    fs::write(
        source
            .join("classic/zones")
            .join(common::ZONE)
            .join("spawns/placements/broken.yaml"),
        r#"schema_version: 1
id: spawn.harbour-watch.placements.broken
kind: placements
zone: zone.harbour-watch
map: quay
source_type: demo.PatchObjects
placements:
  - id: spawn.harbour-watch.placement.broken.1
    object:
      id: spawn.harbour-watch.table.does-not-exist
    position: { x: 0.0, y: 0.0, z: 0.0 }
"#,
    )
    .expect("write broken placement");

    let error = compile::compile(&common::options(source))
        .expect_err("build should reject a dangling reference");
    let message = format!("{error:#}");
    assert!(
        message.contains("spawn.harbour-watch.table.does-not-exist"),
        "error does not name the missing reference: {message}"
    );
}

fn decode_spawn_tables(pack: &std::path::Path) -> Vec<proto::SpawnTable> {
    let bytes = fs::read(pack.join("tables/spawn-tables.sptbl")).expect("read spawn tables");
    table::rows(&bytes)
        .expect("rows")
        .into_iter()
        .map(|row| proto::SpawnTable::decode(row).expect("decode spawn table"))
        .collect()
}

fn decode_zone(pack: &std::path::Path) -> proto::Zone {
    let bytes = fs::read(pack.join("tables/zone.sptbl")).expect("read zone table");
    let rows = table::rows(&bytes).expect("rows");
    assert_eq!(rows.len(), 1, "a pack holds exactly one zone row");
    proto::Zone::decode(rows[0]).expect("decode zone")
}

#[test]
fn combat_rows_carry_what_the_shard_reads() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_flat_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    compile::build(&common::flat_options(source), &out).expect("build");

    let abilities: Vec<proto::Ability> = decode(&out, "abilities");
    assert_eq!(abilities.len(), 1);
    assert_eq!(abilities[0].id, "ability.melee.cleave");
    assert_eq!(abilities[0].range_m, 10.0);
    assert!(abilities[0].triggers_gcd);
    assert_eq!(abilities[0].effects.len(), 1);
    assert_eq!(abilities[0].effects[0].amount, 18.0);
    assert_eq!(abilities[0].effects[0].attack_power_coeff, 0.5);
    assert_eq!(abilities[0].name_key, "Cleave.Name.txt");

    let factions: Vec<proto::Faction> = decode(&out, "factions");
    assert_eq!(factions.len(), 2);
    let wild = &factions[1];
    assert_eq!(wild.id, "faction.wild");
    assert!(wild.attackable);
    assert_eq!(wild.default_stance, "hostile");
    assert_eq!(wild.relations[0].faction_id, "faction.league");

    let mobs: Vec<proto::Mob> = decode(&out, "mobs");
    assert_eq!(mobs.len(), 1);
    assert_eq!(mobs[0].faction_id, "faction.wild");
    assert_eq!(mobs[0].level_min, 2);
    assert_eq!(mobs[0].level_max, 2);
    assert_eq!(mobs[0].walk_speed, 2.0);
    assert_eq!(mobs[0].aggro_radius_m, 12.0);
    assert_eq!(mobs[0].leash_radius_m, 40.0);
    // Resolved from the MobKind the mob names, not defaulted.
    assert_eq!(mobs[0].hp_mod, 0.5);
    assert_eq!(mobs[0].ability_ids, vec!["ability.melee.cleave"]);

    let placements: Vec<proto::Placement> = decode(&out, "placements");
    assert_eq!(placements[0].respawn_delay_min_ms, 10000);
    assert_eq!(placements[0].respawn_delay_max_ms, 14000);
}

#[test]
fn an_overlay_adds_documents_and_is_recorded_in_the_manifest() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_flat_source(&workspace.path().join("src"));

    // Built before the layer exists, so the two packs differ only by the layer.
    let base = workspace.path().join("base-pack");
    compile::build(&common::flat_options(source.clone()), &base).expect("base build");

    common::write_layer(
        &source,
        "extended",
        &[(
            "ability.brine-bolt.yaml",
            r#"schema_version: 1
id: ability.magic.brine-bolt
curation_note: A second ability, so that adding one is a data change.
source_type: demo.SpellResource
target: enemy
range_m: 25.0
cooldown_ms: 3000
triggers_gcd: true
effects:
  - kind: damage
    element: water
    amount: 30
    attack_power_coeff: 0.25
"#,
        )],
    );

    let out = workspace.path().join("pack");
    compile::build(&common::flat_options(source), &out).expect("overlay build");

    assert_eq!(decode::<proto::Ability>(&base, "abilities").len(), 1);
    let abilities: Vec<proto::Ability> = decode(&out, "abilities");
    assert_eq!(abilities.len(), 2);
    assert_eq!(abilities[0].id, "ability.magic.brine-bolt");
    assert_eq!(abilities[0].range_m, 25.0);
    assert_eq!(abilities[0].cooldown_ms, 3000);

    let document: Manifest = serde_json::from_str(
        &fs::read_to_string(out.join("manifest.json")).expect("read manifest"),
    )
    .expect("parse manifest");
    // The layer id, never the path it was read from: packs are compared byte
    // for byte between a local rebuild and the CI one.
    assert_eq!(document.source.overlays, vec!["extended".to_string()]);
}

#[test]
fn a_mob_naming_an_unknown_faction_fails_the_build() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_flat_source(&workspace.path().join("src"));
    fs::write(
        source.join("mob.crab.yaml"),
        format!(
            r#"schema_version: 1
id: mob.{zone}.crab
kind: mob
zone: zone.{zone}
source_type: demo.MobWorldResource
faction:
  id: faction.does-not-exist
level_min: 2
level_max: 2
"#,
            zone = common::ZONE
        ),
    )
    .expect("rewrite mob document");

    let error = compile::compile(&common::flat_options(source))
        .expect_err("build should reject a dangling faction reference");
    let message = format!("{error:#}");
    assert!(
        message.contains("faction.does-not-exist"),
        "error does not name the missing faction: {message}"
    );
}

#[test]
fn a_source_tree_with_no_chargen_document_carries_no_chargen_table() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    let report = compile::build(&common::options(source), &out).expect("build");

    assert!(
        !report.tables().iter().any(|(name, _)| name == "chargen"),
        "a tree with no chargen documents produced a chargen table"
    );
    assert!(!out.join("tables/chargen.sptbl").exists());
}

#[test]
fn chargen_documents_compile_into_the_chargen_table() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_chargen(&source);
    let out = workspace.path().join("pack");
    let report = compile::build(&common::options(source), &out).expect("build");

    assert_eq!(
        report
            .tables()
            .iter()
            .find(|(name, _)| name == "chargen")
            .map(|(_, rows)| *rows),
        Some(2),
        "both chargen documents should compile, enabled or not"
    );

    let bytes = fs::read(out.join("tables/chargen.sptbl")).expect("read chargen table");
    let rows: Vec<proto::ChargenOption> = table::rows(&bytes)
        .expect("rows")
        .into_iter()
        .map(|row| proto::ChargenOption::decode(row).expect("decode chargen option"))
        .collect();

    // Rows are keyed by canonical id, so the disabled `chargen.empire.*`
    // document sorts ahead of the enabled League one.
    assert_eq!(rows[0].id, "chargen.empire.warrior");
    assert!(
        !rows[0].enabled,
        "a disabled option is carried, not dropped"
    );

    let league = &rows[1];
    assert_eq!(league.id, "chargen.league.warrior");
    assert!(league.enabled);
    assert_eq!(league.race, "race.human");
    assert_eq!(league.class, "class.warrior");
    assert_eq!(league.starting_level, 3);
    assert_eq!(league.spawn_zone_id, "zone.harbour-watch");
    let spawn = league.spawn_position.expect("option carries a spawn");
    assert_eq!((spawn.x, spawn.y, spawn.z), (12.0, 4.5, 0.5));
    assert_eq!(league.spawn_heading, 1.5);
    assert_eq!(league.starting_stats.len(), 1);
    assert_eq!(league.starting_stats[0].stat, "strength");
    assert_eq!(league.starting_loadout.len(), 1);
    assert_eq!(
        league.starting_loadout[0].item_id,
        "item.consumable.harbour-tonic"
    );
    assert_eq!(league.starting_loadout[0].quantity, 3);
    assert_eq!(league.starting_loadout[0].slot, "bag");
    assert_eq!(league.starting_abilities, vec!["ability.melee.cleave"]);
    assert_eq!(league.starting_quests, vec!["quest.harbour-watch.first"]);
    assert_eq!(league.name_key, "HarbourWarrior.Name.txt");
}

#[test]
fn a_chargen_option_spawning_outside_a_canonical_zone_fails_the_build() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_chargen(&source);
    fs::write(
        source.join("classic/chargen/broken.yaml"),
        r#"schema_version: 1
id: chargen.league.broken
source_type: demo.ChargenOption
race: race.human
class: class.warrior
sex: female
faction: faction.league
enabled: true
visual_ref: Demo/Visuals/Broken.gd
spawn:
  zone_id: harbour-watch
  position: { x: 0.0, y: 0.0, z: 0.0 }
  heading: 0.0
starting_level: 1
"#,
    )
    .expect("write broken chargen document");

    let error = compile::compile(&common::options(source))
        .expect_err("build should reject a non-canonical zone reference");
    let message = format!("{error:#}");
    assert!(
        message.contains("chargen.league.broken"),
        "error does not name the option: {message}"
    );
}

fn decode<M: Message + Default>(pack: &std::path::Path, name: &str) -> Vec<M> {
    let bytes = fs::read(pack.join(format!("tables/{name}.sptbl"))).expect("read table");
    table::rows(&bytes)
        .expect("rows")
        .into_iter()
        .map(|row| M::decode(row).expect("decode row"))
        .collect()
}

#[test]
fn a_source_tree_with_no_loot_documents_carries_no_item_or_loot_table() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    let report = compile::build(&common::options(source), &out).expect("build");

    for name in ["items", "loot-tables"] {
        assert!(
            !report.tables().iter().any(|(table, _)| table == name),
            "a tree with no loot documents produced a {name} table"
        );
        assert!(!out.join(format!("tables/{name}.sptbl")).exists());
    }
}

#[test]
fn loot_documents_compile_into_the_item_and_loot_tables() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_loot(&source);
    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");

    let items: Vec<proto::Item> = decode(&out, "items");
    assert_eq!(
        items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["item.consumable.harbour-tonic", "item.junk.harbour-shell"],
    );
    assert_eq!(items[0].stack_limit, 20);
    assert_eq!(items[0].vendor_sell, 4);
    // Rule 5.7.6: a limit of one is a legal authored value, not a missing one.
    assert_eq!(items[1].stack_limit, 1);

    let tables: Vec<proto::LootTable> = decode(&out, "loot-tables");
    assert_eq!(tables.len(), 1);
    let root = tables[0].root.as_ref().expect("root node");
    assert_eq!(root.kind, proto::LootNodeKind::And as i32);
    assert_eq!(root.entries.len(), 2);

    // The eight-digit chance survives the round trip bit for bit. It is not
    // representable in binary32, so a `float` field would fail here.
    assert_eq!(root.chances, vec![1.0_f64, 0.006_187_51_f64]);

    let money = &root.entries[0];
    assert_eq!(money.kind, proto::LootNodeKind::Money as i32);
    assert_eq!((money.min_number, money.max_number), (2, 4));
    assert!(money.item_id.is_empty(), "money carries no item reference");

    let inner = &root.entries[1];
    assert_eq!(inner.kind, proto::LootNodeKind::Or as i32);
    assert_eq!(inner.chances, vec![0.3_f64, 0.7_f64]);
    assert_eq!(inner.entries[0].item_id, "item.junk.harbour-shell");
    assert_eq!(inner.entries[1].max_number, 45);
}

#[test]
fn a_loot_tree_whose_chances_do_not_pair_with_its_entries_is_refused() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_loot(&source);
    common::rewrite_loot_table(
        &source,
        r#"schema_version: 1
id: loot.fixture.nested
source_type: demo.LootTableResource
root:
  node: and
  chances: [1.0]
  entries:
    - node: money
      min_number: 1
      max_number: 1
    - node: money
      min_number: 2
      max_number: 2
"#,
    );

    let error = compile::compile(&common::options(source))
        .expect_err("build should reject an unpaired chances array");
    let message = format!("{error:#}");
    assert!(
        message.contains("2 entries but 1 chances"),
        "error does not name the pairing: {message}"
    );
}

#[test]
fn a_loot_grant_of_an_item_the_pack_does_not_carry_is_refused() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_loot(&source);
    common::rewrite_loot_table(
        &source,
        r#"schema_version: 1
id: loot.fixture.nested
source_type: demo.LootTableResource
root:
  node: and
  chances: [1.0]
  entries:
    - node: single-item
      item:
        id: item.junk.does-not-exist
      min_number: 1
      max_number: 1
"#,
    );

    let error = compile::compile(&common::options(source))
        .expect_err("build should reject a dangling item reference");
    let message = format!("{error:#}");
    assert!(
        message.contains("item.junk.does-not-exist"),
        "error does not name the missing item: {message}"
    );
}

#[test]
fn a_loot_leaf_whose_counts_are_inverted_is_refused() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_loot(&source);
    common::rewrite_loot_table(
        &source,
        r#"schema_version: 1
id: loot.fixture.nested
source_type: demo.LootTableResource
root:
  node: and
  chances: [1.0]
  entries:
    - node: money
      min_number: 9
      max_number: 2
"#,
    );

    let error = compile::compile(&common::options(source))
        .expect_err("build should reject inverted counts");
    let message = format!("{error:#}");
    assert!(
        message.contains("max_number 2 below min_number 9"),
        "error does not name the counts: {message}"
    );
}
