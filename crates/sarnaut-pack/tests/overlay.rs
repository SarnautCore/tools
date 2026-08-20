//! End-to-end overlay behaviour: ADR 0021's curated layers over a generated
//! base, with the merge semantics ADR 0029 pins down.

mod common;

use std::fs;

use prost::Message;
use sarnaut_pack::compile;
use sarnaut_pack::proto;
use sarnaut_pack::table;

fn decode<M: Message + Default>(pack: &std::path::Path, name: &str) -> Vec<M> {
    let bytes = fs::read(pack.join(format!("tables/{name}.sptbl"))).expect("read table");
    table::rows(&bytes)
        .expect("rows")
        .into_iter()
        .map(|row| M::decode(row).expect("decode row"))
        .collect()
}

fn mob<'a>(mobs: &'a [proto::Mob], id: &str) -> &'a proto::Mob {
    mobs.iter()
        .find(|mob| mob.id == id)
        .unwrap_or_else(|| panic!("no mob row {id}"))
}

#[test]
fn an_overlay_patches_a_base_document_rather_than_replacing_it() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_layer(
        &source,
        "curated",
        &[(
            "sparrow.yaml",
            r#"schema_version: 1
id: mob.harbour-watch.sparrow
curation_note: The extracted walk speed is too slow to demo a chase.
walk_speed: 4.5
"#,
        )],
    );

    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");

    let mobs: Vec<proto::Mob> = decode(&out, "mobs");
    let sparrow = mob(&mobs, "mob.harbour-watch.sparrow");
    assert_eq!(sparrow.walk_speed, 4.5, "the patched field did not apply");
    // Everything the patch did not mention survives.
    assert_eq!(sparrow.level_min, 2);
    assert_eq!(sparrow.faction_id, "faction.wild");
    assert_eq!(sparrow.aggro_radius_m, 12.0);
    assert_eq!(sparrow.ability_ids, vec!["ability.melee.cleave"]);
}

#[test]
fn an_overlay_adds_a_document_the_base_tree_does_not_carry() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_layer(
        &source,
        "curated",
        &[(
            "curated-quest.yaml",
            r#"schema_version: 1
id: quest.harbour-watch.curated
curation_note: The zone has no kill quest to use, so one is invented.
zone: zone.harbour-watch
quest_type: quest-type-solo
level: 1
required_level: 1
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
        )],
    );

    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");

    let quests: Vec<proto::Quest> = decode(&out, "quests");
    let curated = quests
        .iter()
        .find(|quest| quest.id == "quest.harbour-watch.curated")
        .expect("the overlay quest is missing");
    assert_eq!(curated.objectives.len(), 1);
    assert_eq!(
        curated.objectives[0].kind,
        proto::QuestObjectiveKind::CountKill as i32
    );
    assert_eq!(curated.objectives[0].limit, 3);
    assert_eq!(
        curated.objectives[0].target_ids,
        vec!["mob.harbour-watch.sparrow"]
    );
    assert_eq!(curated.rewards.as_ref().expect("rewards").experience, 8);
    assert!(!curated.can_cancel);
    // The base quest is untouched by an overlay that adds beside it.
    assert_eq!(quests.len(), 2);
}

#[test]
fn layers_apply_in_manifest_order_whatever_order_they_are_named_in() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    for (id, speed) in [("first", "3.0"), ("second", "9.0")] {
        common::write_layer_documents(
            &source,
            id,
            &[(
                "sparrow.yaml",
                &format!(
                    r#"schema_version: 1
id: mob.harbour-watch.sparrow
curation_note: layer {id}
walk_speed: {speed}
"#
                ),
            )],
        );
    }
    common::write_layers_manifest(&source, &[("first", true), ("second", true)]);

    let out = workspace.path().join("pack");
    let mut options = common::options(source);
    // Named in reverse. The manifest still decides, so `second` still wins.
    options.overlays = vec!["second".to_string(), "first".to_string()];
    options.allow_overlay_conflicts = true;
    let pack = compile::build(&options, &out).expect("build");

    assert_eq!(
        pack.manifest.source.overlays,
        vec!["first".to_string(), "second".to_string()]
    );
    let mobs: Vec<proto::Mob> = decode(&out, "mobs");
    assert_eq!(mob(&mobs, "mob.harbour-watch.sparrow").walk_speed, 9.0);
}

#[test]
fn two_layers_writing_one_leaf_fail_the_build_naming_both() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    for (id, speed) in [("first", "3.0"), ("second", "9.0")] {
        common::write_layer(
            &source,
            id,
            &[(
                "sparrow.yaml",
                &format!(
                    r#"schema_version: 1
id: mob.harbour-watch.sparrow
curation_note: layer {id}
walk_speed: {speed}
"#
                ),
            )],
        );
    }

    let error = compile::compile(&common::options(source))
        .expect_err("an overlay conflict should fail the build");
    let message = format!("{error:#}");
    for expected in ["first", "second", "walk_speed", "mob.harbour-watch.sparrow"] {
        assert!(
            message.contains(expected),
            "message lacks {expected}: {message}"
        );
    }
    assert!(message.contains("--allow-overlay-conflicts"), "{message}");
}

#[test]
fn a_layer_that_is_not_applied_by_default_needs_naming() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_layer_documents(
        &source,
        "optional",
        &[(
            "sparrow.yaml",
            r#"schema_version: 1
id: mob.harbour-watch.sparrow
curation_note: Only applied when asked for.
walk_speed: 7.0
"#,
        )],
    );
    common::write_layers_manifest(&source, &[("optional", false)]);

    let default_out = workspace.path().join("default");
    let default_pack =
        compile::build(&common::options(source.clone()), &default_out).expect("default build");
    assert!(default_pack.manifest.source.overlays.is_empty());
    let mobs: Vec<proto::Mob> = decode(&default_out, "mobs");
    assert_eq!(mob(&mobs, "mob.harbour-watch.sparrow").walk_speed, 2.0);

    let named_out = workspace.path().join("named");
    let mut options = common::options(source);
    options.overlays = vec!["optional".to_string()];
    let named_pack = compile::build(&options, &named_out).expect("named build");
    assert_eq!(named_pack.manifest.source.overlays, vec!["optional"]);
    let mobs: Vec<proto::Mob> = decode(&named_out, "mobs");
    assert_eq!(mob(&mobs, "mob.harbour-watch.sparrow").walk_speed, 7.0);
    assert_ne!(
        default_pack.pack_id(),
        named_pack.pack_id(),
        "applying a layer must change the digest"
    );
}

#[test]
fn an_overlay_without_a_curation_note_fails_naming_the_document_and_the_layer() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_layer(
        &source,
        "curated",
        &[(
            "sparrow.yaml",
            r#"schema_version: 1
id: mob.harbour-watch.sparrow
walk_speed: 4.5
"#,
        )],
    );

    let error = compile::compile(&common::options(source))
        .expect_err("an overlay without a note should fail the build");
    let message = format!("{error:#}");
    for expected in ["curation_note", "mob.harbour-watch.sparrow", "curated"] {
        assert!(
            message.contains(expected),
            "message lacks {expected}: {message}"
        );
    }
}

#[test]
fn a_curation_note_never_reaches_the_pack_digest() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_layer(
        &source,
        "curated",
        &[(
            "sparrow.yaml",
            r#"schema_version: 1
id: mob.harbour-watch.sparrow
curation_note: One reason.
walk_speed: 4.5
"#,
        )],
    );
    let first =
        compile::compile(&common::options(source.clone())).expect("build with the first note");

    common::write_layer_documents(
        &source,
        "curated",
        &[(
            "sparrow.yaml",
            r#"schema_version: 1
id: mob.harbour-watch.sparrow
curation_note: A completely different and much longer reason, reworded.
walk_speed: 4.5
"#,
        )],
    );
    let second = compile::compile(&common::options(source)).expect("build with the second note");

    assert_eq!(
        first.pack_id(),
        second.pack_id(),
        "rewording a note moved the content digest"
    );
    let note_of = |pack: &compile::CompiledPack| {
        pack.report
            .curation_notes
            .iter()
            .map(|note| note.note.clone())
            .collect::<Vec<_>>()
    };
    assert_ne!(
        note_of(&first),
        note_of(&second),
        "the build report should have recorded both notes"
    );
}

#[test]
fn an_overlay_can_delete_a_document_outright() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_layer(
        &source,
        "curated",
        &[(
            "retire-quest.yaml",
            r#"schema_version: 1
id: quest.harbour-watch.first
curation_note: Retired until the impact system lands.
_op: delete
"#,
        )],
    );

    let out = workspace.path().join("pack");
    let pack = compile::build(&common::options(source), &out).expect("build");
    assert!(
        !pack.tables().iter().any(|(name, _)| name == "quests"),
        "the only quest was deleted, so no quest table should exist"
    );
    assert_eq!(
        pack.report.deleted_documents,
        vec!["quest.harbour-watch.first"]
    );
}

#[test]
fn a_delete_path_removes_an_inherited_key() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_layer(
        &source,
        "curated",
        &[(
            "sparrow.yaml",
            r#"schema_version: 1
id: mob.harbour-watch.sparrow
curation_note: The extracted leash radius is wrong and has no replacement yet.
_delete: [leash_radius_m]
"#,
        )],
    );

    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");
    let mobs: Vec<proto::Mob> = decode(&out, "mobs");
    let sparrow = mob(&mobs, "mob.harbour-watch.sparrow");
    assert_eq!(sparrow.leash_radius_m, 0.0);
    assert_eq!(sparrow.aggro_radius_m, 12.0, "the sibling key survived");
}

#[test]
fn an_overlay_patches_an_item_that_only_the_index_reaches() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_loot(&source);
    common::write_layer(
        &source,
        "curated",
        &[(
            "tonic.yaml",
            r#"schema_version: 1
id: item.consumable.harbour-tonic
curation_note: A stack of twenty is too many for the M2 bag walkthrough.
stack_limit: 5
"#,
        )],
    );

    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");
    let items: Vec<proto::Item> = decode(&out, "items");
    let tonic = items
        .iter()
        .find(|item| item.id == "item.consumable.harbour-tonic")
        .expect("the tonic is missing");
    assert_eq!(tonic.stack_limit, 5, "the patch did not apply");
    // The base document's other fields survived the patch, which is the whole
    // difference between a patch and a replacement.
    assert_eq!(tonic.vendor_sell, 4);
    assert_eq!(tonic.category, "consumable");
}

#[test]
fn an_unknown_layer_id_fails_and_lists_what_the_manifest_holds() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_layers_manifest(&source, &[("curated", true)]);
    common::write_layer_documents(&source, "curated", &[]);

    let mut options = common::options(source);
    options.overlays = vec!["typo".to_string()];
    let error = compile::compile(&options).expect_err("an unknown layer should fail");
    let message = format!("{error:#}");
    assert!(message.contains("typo"), "{message}");
    assert!(message.contains("curated"), "{message}");
}

#[test]
fn a_layer_id_listed_twice_is_refused() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_layer_documents(&source, "curated", &[]);
    common::write_layers_manifest(&source, &[("curated", true), ("curated", true)]);

    let error =
        compile::compile(&common::options(source)).expect_err("a duplicate layer id should fail");
    assert!(format!("{error:#}").contains("listed twice"));
}

#[test]
fn two_overlay_builds_are_byte_identical() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_loot(&source);
    common::write_layer(
        &source,
        "curated",
        &[(
            "sparrow.yaml",
            r#"schema_version: 1
id: mob.harbour-watch.sparrow
curation_note: Demo pacing.
walk_speed: 4.5
"#,
        )],
    );

    let first = workspace.path().join("first");
    let second = workspace.path().join("second");
    let left = compile::build(&common::options(source.clone()), &first).expect("first build");
    let right = compile::build(&common::options(source), &second).expect("second build");

    assert_eq!(left.pack_id(), right.pack_id());
    for (name, _) in left.tables() {
        let relative = format!("tables/{name}.sptbl");
        assert_eq!(
            fs::read(first.join(&relative)).expect("read first"),
            fs::read(second.join(&relative)).expect("read second"),
            "{relative} differs between builds"
        );
    }
    assert_eq!(
        fs::read(first.join("manifest.json")).expect("read first manifest"),
        fs::read(second.join("manifest.json")).expect("read second manifest"),
    );
}

#[test]
fn a_curated_loot_table_ships_even_before_anything_reaches_it() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_chargen(&source);
    common::write_layer(
        &source,
        "curated",
        &[(
            "curated-loot.yaml",
            r#"schema_version: 1
id: loot.curated.pocket-change
curation_note: A demoable drop, authored ahead of the mob that will use it.
root:
  node: and
  chances: [1.0]
  entries:
    - node: money
      min_number: 2
      max_number: 6
"#,
        )],
    );

    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");
    let tables: Vec<proto::LootTable> = decode(&out, "loot-tables");
    assert_eq!(
        tables
            .iter()
            .map(|table| table.id.as_str())
            .collect::<Vec<_>>(),
        vec!["loot.curated.pocket-change"]
    );
}
