//! Cross-reference integrity across the authored chain
//! quest → mob → spawn table → placement → item → loot table → locale.
//!
//! One test per broken edge, each asserting that the message names **both** the
//! document that made the reference and the id it could not resolve. A message
//! that names only one of them turns a diagnosis into a search.

mod common;

use std::fs;
use std::path::Path;

use sarnaut_pack::compile;

/// Compiles the fixture tree after `mutate` has broken something, and returns
/// the failure message.
fn refuse(mutate: impl FnOnce(&Path)) -> String {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_chargen(&source);
    common::write_loot(&source);
    mutate(&source);

    let error = compile::compile(&common::options(source))
        .expect_err("the compiler should refuse a dangling reference");
    format!("{error:#}")
}

/// Compiles the fixture tree after `mutate`, expecting it to succeed.
fn accept(mutate: impl FnOnce(&Path)) -> compile::CompiledPack {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    common::write_chargen(&source);
    common::write_loot(&source);
    mutate(&source);
    compile::compile(&common::options(source)).expect("compile")
}

fn assert_names_both(message: &str, referencer: &str, target: &str) {
    assert!(
        message.contains(referencer),
        "message does not name the referencing document {referencer}: {message}"
    );
    assert!(
        message.contains(target),
        "message does not name the missing target {target}: {message}"
    );
}

/// Rewrites one line of a document, which is how each test breaks exactly one
/// edge without disturbing the rest of the tree.
fn substitute(source: &Path, relative: &str, from: &str, to: &str) {
    let path = source.join(relative);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert!(
        text.contains(from),
        "{} does not contain {from:?}",
        path.display()
    );
    fs::write(&path, text.replace(from, to)).expect("rewrite document");
}

const SPARROW: &str = "classic/zones/harbour-watch/spawns/mobs/sparrow.yaml";
const QUAY: &str = "classic/zones/harbour-watch/spawns/placements/quay.yaml";
const TABLE: &str = "classic/zones/harbour-watch/spawns/tables/sparrows.yaml";
const QUEST: &str = "classic/zones/harbour-watch/quests/first.yaml";
const KIND: &str = "classic/mobkinds/harbour.yaml";
const WILD: &str = "classic/factions/wild.yaml";
const LOOT: &str = "classic/loot/nested.yaml";
const CHARGEN: &str = "classic/chargen/league-warrior.yaml";

#[test]
fn a_placement_naming_no_mob_or_table_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            QUAY,
            "id: mob.harbour-watch.gull",
            "id: mob.harbour-watch.ghost",
        );
    });
    assert_names_both(
        &message,
        "spawn.harbour-watch.placement.quay.2",
        "mob.harbour-watch.ghost",
    );
}

#[test]
fn a_placement_naming_no_route_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            QUAY,
            "route: route.harbour-watch.quay",
            "route: route.harbour-watch.nowhere",
        );
    });
    assert_names_both(
        &message,
        "spawn.harbour-watch.placement.quay.1",
        "route.harbour-watch.nowhere",
    );
}

#[test]
fn a_spawn_table_naming_no_mob_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            TABLE,
            "id: mob.harbour-watch.sparrow",
            "id: mob.harbour-watch.wren",
        );
    });
    assert_names_both(
        &message,
        "spawn.harbour-watch.table.sparrows",
        "mob.harbour-watch.wren",
    );
}

#[test]
fn a_mob_naming_no_mob_kind_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            SPARROW,
            "id: mobkind.base.harbour",
            "id: mobkind.base.absent",
        );
    });
    assert_names_both(&message, "mob.harbour-watch.sparrow", "mobkind.base.absent");
}

#[test]
fn a_mob_naming_no_quality_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            SPARROW,
            "id: mobquality.common",
            "id: mobquality.legendary",
        );
    });
    assert_names_both(
        &message,
        "mob.harbour-watch.sparrow",
        "mobquality.legendary",
    );
}

#[test]
fn a_mob_naming_no_loot_table_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            SPARROW,
            "id: loot.fixture.nested",
            "id: loot.fixture.absent",
        );
    });
    assert_names_both(&message, "mob.harbour-watch.sparrow", "loot.fixture.absent");
}

#[test]
fn a_mob_naming_no_ability_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            SPARROW,
            "- id: ability.melee.cleave",
            "- id: ability.melee.absent",
        );
    });
    assert_names_both(
        &message,
        "mob.harbour-watch.sparrow",
        "ability.melee.absent",
    );
}

#[test]
fn a_mob_offering_no_quest_is_refused() {
    let message = refuse(|source| {
        let path = source.join(SPARROW);
        let text = fs::read_to_string(&path).expect("read mob");
        fs::write(
            &path,
            format!("{text}quests:\n  - id: quest.harbour-watch.absent\n"),
        )
        .expect("rewrite mob");
    });
    assert_names_both(
        &message,
        "mob.harbour-watch.sparrow",
        "quest.harbour-watch.absent",
    );
}

#[test]
fn a_mob_kind_naming_no_prototype_is_refused() {
    let message = refuse(|source| {
        let path = source.join(KIND);
        let text = fs::read_to_string(&path).expect("read mob kind");
        fs::write(
            &path,
            format!("{text}prototype:\n  id: mobkind.base.absent\n"),
        )
        .expect("rewrite mob kind");
    });
    assert_names_both(&message, "mobkind.base.harbour", "mobkind.base.absent");
}

#[test]
fn a_mob_kind_naming_no_class_is_refused() {
    let message = refuse(|source| {
        substitute(source, KIND, "id: mobclass.beast", "id: mobclass.absent");
    });
    assert_names_both(&message, "mobkind.base.harbour", "mobclass.absent");
}

#[test]
fn a_faction_naming_no_faction_is_refused() {
    let message = refuse(|source| {
        let path = source.join(WILD);
        let text = fs::read_to_string(&path).expect("read faction");
        fs::write(
            &path,
            format!("{text}relations:\n  - faction: faction.absent\n    stance: hostile\n"),
        )
        .expect("rewrite faction");
    });
    assert_names_both(&message, "faction.wild", "faction.absent");
}

#[test]
fn a_quest_naming_no_starter_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            QUEST,
            "starter:\n  id: mob.harbour-watch.gull",
            "starter:\n  id: mob.harbour-watch.absent",
        );
    });
    assert_names_both(
        &message,
        "quest.harbour-watch.first",
        "mob.harbour-watch.absent",
    );
}

#[test]
fn a_quest_naming_no_finisher_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            QUEST,
            "finisher:\n  id: mob.harbour-watch.gull",
            "finisher:\n  id: mob.harbour-watch.absent",
        );
    });
    assert_names_both(
        &message,
        "quest.harbour-watch.first",
        "mob.harbour-watch.absent",
    );
}

#[test]
fn a_quest_naming_no_prerequisite_is_refused() {
    let message = refuse(|source| {
        let path = source.join(QUEST);
        let text = fs::read_to_string(&path).expect("read quest");
        fs::write(
            &path,
            format!(
                "{text}prerequisites:\n  - quest:\n      id: quest.harbour-watch.absent\n    status: Finished\n"
            ),
        )
        .expect("rewrite quest");
    });
    assert_names_both(
        &message,
        "quest.harbour-watch.first",
        "quest.harbour-watch.absent",
    );
}

#[test]
fn a_quest_objective_naming_no_target_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            QUEST,
            "- id: mob.harbour-watch.sparrow",
            "- id: mob.harbour-watch.absent",
        );
    });
    assert_names_both(
        &message,
        "quest.harbour-watch.first",
        "mob.harbour-watch.absent",
    );
}

#[test]
fn a_quest_reward_naming_no_item_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            QUEST,
            "rewards:\n  experience: 8",
            "rewards:\n  experience: 8\n  mandatory_items:\n    - item:\n        id: item.junk.absent\n      count: 1",
        );
    });
    assert_names_both(&message, "quest.harbour-watch.first", "item.junk.absent");
}

#[test]
fn a_loot_grant_naming_no_item_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            LOOT,
            "id: item.junk.harbour-shell",
            "id: item.junk.absent",
        );
    });
    assert_names_both(&message, "loot.fixture.nested", "item.junk.absent");
}

#[test]
fn a_chargen_option_naming_no_loadout_item_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            CHARGEN,
            "item_id: item.consumable.harbour-tonic",
            "item_id: item.consumable.absent",
        );
    });
    assert_names_both(&message, "chargen.league.warrior", "item.consumable.absent");
}

#[test]
fn a_chargen_option_naming_no_starting_quest_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            CHARGEN,
            "- quest.harbour-watch.first",
            "- quest.harbour-watch.absent",
        );
    });
    assert_names_both(
        &message,
        "chargen.league.warrior",
        "quest.harbour-watch.absent",
    );
}

#[test]
fn a_chargen_option_naming_no_starting_ability_is_refused() {
    let message = refuse(|source| {
        substitute(
            source,
            CHARGEN,
            "- ability.melee.cleave",
            "- ability.melee.absent",
        );
    });
    assert_names_both(&message, "chargen.league.warrior", "ability.melee.absent");
}

#[test]
fn a_locale_gap_is_reported_but_does_not_fail_a_build() {
    let pack = accept(|source| {
        substitute(
            source,
            SPARROW,
            "name: Sparrow.Name.txt",
            "name: Absent.Name.txt",
        );
    });
    assert!(
        pack.references.is_clean(),
        "a missing translation is coverage, not a broken edge"
    );
    assert!(
        pack.references.locale_gaps.iter().any(
            |gap| gap.referencer == "mob.harbour-watch.sparrow" && gap.key == "Absent.Name.txt"
        ),
        "the gap was not recorded: {:?}",
        pack.references.locale_gaps
    );
}

#[test]
fn an_unresolved_locale_key_is_not_written_into_the_row() {
    // ADR 0011. An authored key the pack cannot resolve is a verbatim MY.GAMES
    // resource path, and some of those paths carry the source resource's class
    // name in parentheses. Carrying one through would put a MY.GAMES type name
    // in a compiled artifact.
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    substitute(
        &source,
        SPARROW,
        "name: Sparrow.Name.txt",
        "name: Sparrow.(MobWorldResource).txt",
    );
    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");

    let bytes = fs::read(out.join("tables/mobs.sptbl")).expect("read mobs table");
    assert!(
        !String::from_utf8_lossy(&bytes).contains("MobWorldResource"),
        "an unresolved key carried a source type name into the pack"
    );
}

#[test]
fn require_locale_turns_a_gap_into_a_failure_naming_both_ends() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    substitute(
        &source,
        SPARROW,
        "name: Sparrow.Name.txt",
        "name: Absent.Name.txt",
    );

    let mut options = common::options(source);
    options.require_locale = true;
    let error = compile::compile(&options).expect_err("--require-locale should refuse a gap");
    assert_names_both(
        &format!("{error:#}"),
        "mob.harbour-watch.sparrow",
        "Absent.Name.txt",
    );
}

#[test]
fn a_reference_into_another_zone_is_external_and_not_a_failure() {
    let pack = accept(|source| {
        substitute(
            source,
            QUEST,
            "finisher:\n  id: mob.harbour-watch.gull",
            "finisher:\n  id: mob.tide-steps.quartermaster",
        );
    });
    assert!(pack.references.is_clean());
    let external = pack
        .references
        .external
        .iter()
        .find(|entry| entry.target == "mob.tide-steps.quartermaster")
        .expect("the external reference was not recorded");
    assert_eq!(external.zone, "tide-steps");
    assert_eq!(external.referencer, "quest.harbour-watch.first");
}

#[test]
fn a_reference_to_an_interactive_object_is_unmodelled_and_not_a_failure() {
    // The M2 zone places fifty-two of these through spawn tables. No row type
    // describes a chest or a stele, so the reference has nothing to resolve
    // against and is counted rather than failed.
    let pack = accept(|source| {
        substitute(
            source,
            TABLE,
            "id: mob.harbour-watch.sparrow",
            "id: item.interactive-objects.harbour-watch.crate.crate-chest-resource",
        );
    });
    assert!(pack.references.is_clean());
    let unmodelled = pack
        .references
        .unmodelled
        .iter()
        .find(|entry| entry.referencer == "spawn.harbour-watch.table.sparrows")
        .expect("the unmodelled reference was not recorded");
    assert!(unmodelled.target.starts_with("item.interactive-objects."));
}

#[test]
fn a_clean_tree_resolves_every_edge() {
    let pack = accept(|_| {});
    assert!(pack.references.is_clean());
    assert!(
        pack.references.resolved > 20,
        "the fixture should exercise more than a handful of edges, got {}",
        pack.references.resolved
    );
}
