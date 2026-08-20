use std::path::Path;

use crate::model::ResourceRef;

pub(crate) fn slug(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut output = String::new();
    let mut separator = false;

    for (index, current) in chars.iter().copied().enumerate() {
        if !current.is_ascii_alphanumeric() {
            separator = !output.is_empty();
            continue;
        }

        let previous = index.checked_sub(1).and_then(|i| chars.get(i)).copied();
        let next = chars.get(index + 1).copied();
        let camel_boundary = current.is_ascii_uppercase()
            && previous.is_some_and(|value| value.is_ascii_lowercase())
            || current.is_ascii_uppercase()
                && previous.is_some_and(|value| value.is_ascii_uppercase())
                && next.is_some_and(|value| value.is_ascii_lowercase());
        if (separator || camel_boundary) && !output.ends_with('-') {
            output.push('-');
        }
        output.push(current.to_ascii_lowercase());
        separator = false;
    }

    output.trim_matches('-').to_owned()
}

pub(crate) fn source_path(path: &Path, root: &Path) -> anyhow::Result<String> {
    let relative = path.strip_prefix(root)?;
    Ok(relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

pub(crate) fn resource_ref(href: &str) -> ResourceRef {
    ResourceRef {
        id: canonical_id_from_href(href),
        href: href.to_owned(),
    }
}

/// Translate source-era zone names at the point where they become public ids.
/// Most source names are already usable fallbacks, but ADR 0007 names Lightwood
/// explicitly and keeps that English id stable across source versions.
pub(crate) fn canonical_zone_slug(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "zoneleague1" | "zone-league1" | "lightwood" => "lightwood".into(),
        _ => slug(value),
    }
}

pub(crate) fn canonical_id_from_href(href: &str) -> Option<String> {
    let path = href.split('#').next()?.trim_start_matches('/');
    canonical_id_from_source_path(path)
}

pub(crate) fn canonical_id_from_source_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let first = *parts.first()?;

    match first {
        "Items" if parts.len() >= 3 => {
            let category = slug(parts[1]);
            let tail = item_slug_path(&parts[2..]);
            Some(format!("item.{category}.{tail}"))
        }
        "World" if parts.get(1) == Some(&"Quests") && parts.len() >= 4 => {
            let zone = canonical_zone_slug(parts[2]);
            let tail = quest_slug_path(&parts[3..]);
            Some(format!("quest.{zone}.{tail}"))
        }
        "Characters" | "Creatures" if parts.get(2) == Some(&"Instances") && parts.len() >= 5 => {
            let family = slug(parts[1]);
            let zone = canonical_zone_slug(parts[3]);
            let tail = slug_path(&parts[4..]);
            Some(format!("mob.{zone}.{family}.{tail}"))
        }
        "Maps" if parts.get(2) == Some(&"SpawnTables") && parts.len() >= 5 => {
            let zone = canonical_zone_slug(parts[3]);
            let tail = slug_path(&parts[4..]);
            Some(format!("spawn.{zone}.table.{tail}"))
        }
        "World" if parts.get(1) == Some(&"LootTables") && parts.len() >= 4 => {
            Some(format!("loot.{}", slug_path(&parts[2..])))
        }
        // Factions live under two roots that never collide on file name; both flatten
        // to a single slug because faction ids are two segments by schema.
        "World" if parts.get(1) == Some(&"Factions") && parts.len() >= 3 => {
            Some(format!("faction.{}", flat_slug(&parts[2..])))
        }
        "Mechanics" if parts.get(1) == Some(&"Faction") && parts.len() >= 3 => {
            Some(format!("faction.{}", flat_slug(&parts[2..])))
        }
        "Mechanics" if parts.get(1) == Some(&"MobClasses") && parts.len() == 3 => {
            Some(format!("mobclass.{}", flat_slug(&parts[2..])))
        }
        "Mechanics" if parts.get(1) == Some(&"MobQualities") && parts.len() == 3 => {
            Some(format!("mobquality.{}", flat_slug(&parts[2..])))
        }
        "Mechanics" if parts.get(1) == Some(&"MobKindTemplates") && parts.len() == 3 => Some(
            format!("mobkind.mob-kind-templates.{}", flat_slug(&parts[2..])),
        ),
        // `Mechanics/Creatures` and `Mechanics/Characters` mix mob kinds with ability
        // kits and roaming data, so only the `(MobKind)` resource marker mints a
        // mobkind id; anything else there stays unmapped rather than mislabelled.
        "Mechanics"
            if matches!(parts.get(1), Some(&"Creatures") | Some(&"Characters"))
                && parts.len() >= 3
                && parts[parts.len() - 1].contains(".(MobKind)") =>
        {
            Some(format!("mobkind.{}", slug_path(&parts[1..])))
        }
        _ => None,
    }
}

/// Mint a canonical id after the caller has already parsed and type-checked a
/// `MobKind` document. Classic character kinds do not consistently carry the
/// `.(MobKind)` marker in their file name, so the context-free mapper above must
/// leave them alone while the MobKind resolver can map them safely.
pub(crate) fn mobkind_id_from_source_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.first() == Some(&"Mechanics")
        && matches!(parts.get(1), Some(&"Creatures") | Some(&"Characters"))
        && parts.len() >= 3
    {
        return Some(format!("mobkind.{}", slug_path(&parts[1..])));
    }
    canonical_id_from_source_path(path)
}

/// Slug a path tail into one hyphenated segment, for ids the schemas cap at two dots.
fn flat_slug(parts: &[&str]) -> String {
    let segments: Vec<String> = parts
        .iter()
        .map(|part| slug(strip_xdb_suffix(part)))
        .filter(|part| !part.is_empty())
        .collect();
    bounded_slug(&segments.join("-"), 120)
}

pub(crate) fn slug_path(parts: &[&str]) -> String {
    let segments: Vec<String> = parts
        .iter()
        .map(|part| slug(strip_xdb_suffix(part)))
        .filter(|part| !part.is_empty())
        .collect();
    bounded_slug(&segments.join("."), 120)
}

fn item_slug_path(parts: &[&str]) -> String {
    let segments: Vec<String> = parts
        .iter()
        .flat_map(|part| {
            part.strip_suffix(".xdb")
                .unwrap_or(part)
                .split('_')
                .map(slug)
        })
        .filter(|part| !part.is_empty())
        .collect();
    bounded_slug(&segments.join("."), 120)
}

fn quest_slug_path(parts: &[&str]) -> String {
    let mut segments: Vec<String> = parts
        .iter()
        .map(|part| slug(strip_xdb_suffix(part)))
        .filter(|part| !part.is_empty())
        .collect();
    if segments.len() >= 2 && segments.last() == segments.get(segments.len() - 2) {
        segments.pop();
    }
    bounded_slug(&segments.join("."), 120)
}

pub(crate) fn strip_xdb_suffix(value: &str) -> &str {
    let without_xdb = value.strip_suffix(".xdb").unwrap_or(value);
    without_xdb
        .rfind(".(")
        .map_or(without_xdb, |index| &without_xdb[..index])
}

pub(crate) fn bounded_slug(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let hash = blake3::hash(value.as_bytes()).to_hex();
    let prefix = value[..limit - 13].trim_end_matches(['-', '.']);
    format!("{prefix}-{}", &hash[..12])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_stable_english_path_ids() {
        assert_eq!(slug("GibberlingFur"), "gibberling-fur");
        assert_eq!(slug("Quest_1_10"), "quest-1-10");
        assert_eq!(
            canonical_id_from_source_path("World/Quests/InstLeague1/Quest_1_10/Quest_1_10.xdb"),
            Some("quest.inst-league1.quest-1-10".into())
        );
        assert_eq!(
            canonical_id_from_href(
                "/Items/QuestItems/Test/Test.(ItemResource).xdb#xpointer(/ItemResource)"
            ),
            Some("item.quest-items.test.test-item-resource".into())
        );
    }

    #[test]
    fn mints_ids_for_the_creature_taxonomy_loot_and_factions() {
        assert_eq!(
            canonical_id_from_source_path(
                "Mechanics/Creatures/ZombieWarrior/ZombieWarriorStartInstKind.(MobKind).xdb"
            ),
            Some("mobkind.creatures.zombie-warrior.zombie-warrior-start-inst-kind".into())
        );
        assert_eq!(
            canonical_id_from_source_path("Mechanics/MobKindTemplates/AE1Player.xdb"),
            Some("mobkind.mob-kind-templates.ae1player".into())
        );
        assert_eq!(
            canonical_id_from_source_path("Mechanics/MobClasses/UDZombieFighter.xdb"),
            Some("mobclass.ud-zombie-fighter".into())
        );
        assert_eq!(
            canonical_id_from_source_path("Mechanics/MobQualities/Common.xdb"),
            Some("mobquality.common".into())
        );
        assert_eq!(
            canonical_id_from_source_path("World/Factions/Wild.xdb"),
            Some("faction.wild".into())
        );
        assert_eq!(
            canonical_id_from_source_path("World/Factions/ZoneLeague1/CityOrder.(Faction).xdb"),
            Some("faction.zone-league1-city-order".into())
        );
        assert_eq!(
            canonical_id_from_source_path("World/LootTables/ZombieWarrior/ZombieWarrior(01).xdb"),
            Some("loot.zombie-warrior.zombie-warrior-01".into())
        );
    }

    #[test]
    fn leaves_neighbouring_mechanics_resources_unmapped() {
        // An ability kit sits in the same directory as the mob kind it belongs to.
        assert_eq!(
            canonical_id_from_source_path(
                "Mechanics/Creatures/ZombieWarrior/ZombieWarriorStartInstKit.(MobAbilityKit).xdb"
            ),
            None
        );
        assert_eq!(
            canonical_id_from_source_path("Mechanics/ItemQualities/Common.xdb"),
            None
        );
    }

    #[test]
    fn mints_markerless_character_kind_ids_only_in_typed_context() {
        let path = "Mechanics/Characters/Kania_male/ZoneLeague1/Aidenus.xdb";
        assert_eq!(canonical_id_from_source_path(path), None);
        assert_eq!(
            mobkind_id_from_source_path(path),
            Some("mobkind.characters.kania-male.zone-league1.aidenus".into())
        );
    }

    #[test]
    fn uses_the_canonical_lightwood_zone_id_for_source_zone_league1() {
        assert_eq!(canonical_zone_slug("ZoneLeague1"), "lightwood");
        assert_eq!(
            canonical_id_from_source_path("World/Quests/ZoneLeague1/Quest_01_01/Quest_01_01.xdb"),
            Some("quest.lightwood.quest-01-01".into())
        );
        assert_eq!(
            canonical_id_from_source_path(
                "Characters/Kania_male/Instances/ZoneLeague1/Forester.(MobWorld).xdb"
            ),
            Some("mob.lightwood.kania-male.forester".into())
        );
    }
}
