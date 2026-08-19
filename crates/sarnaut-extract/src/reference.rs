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
            let zone = slug(parts[2]);
            let tail = quest_slug_path(&parts[3..]);
            Some(format!("quest.{zone}.{tail}"))
        }
        "Characters" | "Creatures" if parts.get(2) == Some(&"Instances") && parts.len() >= 5 => {
            let family = slug(parts[1]);
            let zone = slug(parts[3]);
            let tail = slug_path(&parts[4..]);
            Some(format!("mob.{zone}.{family}.{tail}"))
        }
        "Maps" if parts.get(2) == Some(&"SpawnTables") && parts.len() >= 5 => {
            let zone = slug(parts[3]);
            let tail = slug_path(&parts[4..]);
            Some(format!("spawn.{zone}.table.{tail}"))
        }
        _ => None,
    }
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
}
