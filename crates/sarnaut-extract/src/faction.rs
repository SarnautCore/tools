//! Faction extraction, closed over the relations the seed factions reach.
//!
//! A source `Faction` lists only the factions it treats better than default: a
//! `friends` list and a `neutrals` list. It never lists enemies, so hostility is what
//! is left over. Two derived fields have no direct source field and are called out
//! where they are computed: `default_stance` (from `defaultReputation`) and
//! `attackable` (from `littleOldMan`).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use anyhow::{Context, Result};
use roxmltree::Node;

use crate::model::{FactionDocument, FactionRelation, LocRefs};
use crate::output::OutputWriter;
use crate::reference::canonical_id_from_source_path;
use crate::scan::{loc_key, resolve_href};
use crate::validation::SchemaKind;
use crate::xdb::{
    bool_value, child, children, extra_fields, href, i64_value, parse_document, read_xdb_from,
    resource_id,
};

const FACTION: &str = "gameMechanics.world.creature.Faction";

#[derive(Debug, Default)]
pub(crate) struct FactionSummary {
    pub(crate) emitted: usize,
    pub(crate) unchanged: usize,
}

/// Extract `seeds` and every faction reachable from them through their relations.
pub(crate) fn extract_faction_closure(
    src: &Path,
    seeds: &BTreeSet<String>,
    out: &Path,
    writer: &OutputWriter,
) -> Result<FactionSummary> {
    let documents = faction_closure(src, seeds)?;
    let mut summary = FactionSummary::default();
    for (relative, document) in &documents {
        let file = document
            .id
            .strip_prefix("faction.")
            .with_context(|| format!("faction id prefix for {relative}"))?;
        let output = out.join("factions").join(format!("{file}.yaml"));
        summary.unchanged += usize::from(writer.write(&output, SchemaKind::Faction, document)?);
        summary.emitted += 1;
    }
    Ok(summary)
}

/// Every faction reachable from `seeds`, keyed by its source-relative path.
pub(crate) fn faction_closure(
    src: &Path,
    seeds: &BTreeSet<String>,
) -> Result<BTreeMap<String, FactionDocument>> {
    let mut queue: VecDeque<String> = seeds.iter().cloned().collect();
    let mut seen: BTreeSet<String> = seeds.iter().cloned().collect();
    let mut documents: BTreeMap<String, FactionDocument> = BTreeMap::new();

    while let Some(relative) = queue.pop_front() {
        let path = src.join(&relative);
        if !path.is_file() {
            anyhow::bail!("faction {relative} does not exist below {}", src.display());
        }
        let xdb = read_xdb_from(&path, src, Some(crate::mobkind::CLASSIC_SERVER_ROOT))?;
        let xml = parse_document(&xdb.text, &path)?;
        let root = xml.root_element();
        if root.tag_name().name() != FACTION {
            anyhow::bail!(
                "{relative} is a {}, not a {FACTION}",
                root.tag_name().name()
            );
        }
        let id = canonical_id_from_source_path(&relative)
            .with_context(|| format!("build faction ID from {relative}"))?;

        let mut relations = Vec::new();
        for (list, stance) in [("friends", "friendly"), ("neutrals", "neutral")] {
            for target in related_hrefs(root, list) {
                let Some((_, target_relative)) = resolve_href(src, &path, &target) else {
                    continue;
                };
                let Some(target_id) = canonical_id_from_source_path(&target_relative) else {
                    continue;
                };
                relations.push(FactionRelation {
                    faction: target_id,
                    stance: stance.to_owned(),
                });
                if seen.insert(target_relative.clone()) {
                    queue.push_back(target_relative);
                }
            }
        }
        if let Some(parent) = href(root, "parentFaction")
            && let Some((_, parent_relative)) = resolve_href(src, &path, &parent)
            && seen.insert(parent_relative.clone())
        {
            queue.push_back(parent_relative);
        }
        relations.sort_by(|left, right| left.faction.cmp(&right.faction));
        relations.dedup_by(|left, right| left.faction == right.faction);

        let mut extra = extra_fields(root, &["Header", "name", "friends", "neutrals"]);
        extra.insert(
            "derivation".into(),
            serde_json::Value::String(DERIVATION.into()),
        );
        documents.insert(
            relative.clone(),
            FactionDocument {
                schema_version: 1,
                id,
                source_type: root.tag_name().name().to_owned(),
                resource_id: resource_id(root),
                loc_ref: LocRefs {
                    name: href(root, "name").and_then(|value| loc_key(src, &path, &value)),
                    ..LocRefs::default()
                },
                // No source field marks a faction as playable: `sysTutorialName` and
                // the PvP flag part are both carried by 25 NPC factions as well as by
                // League and Empire, so player membership stays a curated overlay.
                player_faction: None,
                attackable: attackable(root),
                default_stance: default_stance(root).to_owned(),
                relations,
                extra,
                source: xdb.source,
            },
        );
    }
    Ok(documents)
}

const DERIVATION: &str = "friends -> friendly, neutrals -> neutral, everything else -> default_stance; default_stance from the <defaultReputation> sign, attackable from <littleOldMan>; player_faction has no source field and is a curated overlay";

fn related_hrefs(root: Node<'_, '_>, list: &str) -> Vec<String> {
    child(root, list)
        .map(|node| {
            children(node, "Item")
                .filter_map(|item| item.attribute("href"))
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// `defaultReputation` is the standing a character starts at with this faction, and
/// its sign is the only source signal for how the faction treats an unlisted party.
/// A faction that declares none is hostile by default — that is what makes `Wild`
/// hostile to players while `Neutral`, which declares `0`, is not.
fn default_stance(root: Node<'_, '_>) -> &'static str {
    match i64_value(root, "defaultReputation") {
        Some(value) if value > 0 => "friendly",
        Some(_) => "neutral",
        None => "hostile",
    }
}

/// `littleOldMan` marks the invulnerable-bystander factions the source uses for NPCs
/// that must never be a valid hostile target. Absent means an ordinary faction whose
/// members can be attacked.
fn attackable(root: Node<'_, '_>) -> bool {
    !bool_value(root, "littleOldMan").unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::testing::{fixture_options, write};

    #[test]
    fn closes_over_relations_and_derives_stance_from_the_source_fields() {
        let (source, output, options) = fixture_options();
        write(
            &source.path().join("World/Factions/Wild.xdb"),
            r#"<gameMechanics.world.creature.Faction><Header><resourceId>1</resourceId></Header>
<name href="WILD_NAME.txt"/>
<friends><Item href="Neutral.xdb#x"/></friends>
</gameMechanics.world.creature.Faction>"#,
        );
        write(
            &source.path().join("World/Factions/Neutral.xdb"),
            r#"<gameMechanics.world.creature.Faction><Header><resourceId>2</resourceId></Header>
<name href="NEUTRAL_NAME.txt"/><littleOldMan>true</littleOldMan><defaultReputation>0</defaultReputation>
<neutrals><Item href="ZoneLeague1/CityOrder.(Faction).xdb#x"/></neutrals>
</gameMechanics.world.creature.Faction>"#,
        );
        write(
            &source
                .path()
                .join("World/Factions/ZoneLeague1/CityOrder.(Faction).xdb"),
            r#"<gameMechanics.world.creature.Faction><Header><resourceId>3</resourceId></Header><sysTutorialName>CityOrder</sysTutorialName></gameMechanics.world.creature.Faction>"#,
        );

        let writer = OutputWriter::new(false, None).unwrap();
        let seeds = BTreeSet::from(["World/Factions/Wild.xdb".to_owned()]);
        let summary =
            extract_faction_closure(source.path(), &seeds, &options.out, &writer).unwrap();
        assert_eq!(summary.emitted, 3);

        let wild = fs::read_to_string(output.path().join("factions/wild.yaml")).unwrap();
        assert!(wild.contains("default_stance: hostile"), "{wild}");
        assert!(wild.contains("attackable: true"), "{wild}");
        assert!(wild.contains("faction: faction.neutral"), "{wild}");
        assert!(wild.contains("stance: friendly"), "{wild}");

        let neutral = fs::read_to_string(output.path().join("factions/neutral.yaml")).unwrap();
        assert!(neutral.contains("default_stance: neutral"), "{neutral}");
        assert!(neutral.contains("attackable: false"), "{neutral}");
        assert!(
            neutral.contains("faction: faction.zone-league1-city-order"),
            "{neutral}"
        );
    }
}
