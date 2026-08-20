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
    let left =
        compile::build(&common::options(source.clone(), first.clone())).expect("first build");
    let right = compile::build(&common::options(source, second.clone())).expect("second build");

    assert_eq!(left.pack_id, right.pack_id, "pack_id is not reproducible");
    for relative in [
        "manifest.json",
        "tables/zone.sptbl",
        "tables/placements.sptbl",
        "tables/spawn-tables.sptbl",
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
    let report = compile::build(&common::options(source, out.clone())).expect("build");

    let document: Manifest = serde_json::from_str(
        &fs::read_to_string(out.join("manifest.json")).expect("read manifest"),
    )
    .expect("parse manifest");

    assert_eq!(document.schema_version, 1);
    assert_eq!(document.zone, common::ZONE);
    assert_eq!(document.pack_id, report.pack_id);
    assert!(!document.keep_extra);
    assert_eq!(
        document
            .tables
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["placements", "spawn-tables", "zone"],
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
    compile::build(&common::options(source, out.clone())).expect("build");

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
    compile::build(&common::options(source, out.clone())).expect("build");

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
    let mut options = common::options(source, out.clone());
    options.keep_extra = true;
    compile::build(&options).expect("build");

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

    let plain = compile::build(&common::options(
        source.clone(),
        workspace.path().join("plain"),
    ))
    .expect("plain build");
    let mut options = common::options(source, workspace.path().join("kept"));
    options.keep_extra = true;
    let kept = compile::build(&options).expect("keep-extra build");

    assert_ne!(
        plain.pack_id, kept.pack_id,
        "a pack that carries extra rows must not share a digest with one that does not"
    );
}

#[test]
fn player_spawn_defaults_to_the_first_live_placement() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    compile::build(&common::options(source, out.clone())).expect("build");

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
    let mut options = common::options(source, out.clone());
    options.player_spawn = Some(PlayerSpawn {
        x: -1.0,
        y: 4.0,
        z: 9.5,
        yaw: 2.5,
    });
    compile::build(&options).expect("build");

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
    compile::build(&common::options(source, out.clone())).expect("build");

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

    let error = compile::build(&common::options(source, workspace.path().join("pack")))
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
