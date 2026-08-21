//! Stable quest objective identity shared by quest and script extraction.

use std::collections::BTreeSet;

const DOMAIN: &str = "sarnaut.quest-objective.v1";

pub(crate) fn derive_objective_id<'a>(
    quest_id: &str,
    kind: &str,
    source_counter_id: Option<&str>,
    custom_name: Option<&str>,
    targets: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut hasher = blake3::Hasher::new();
    add_component(&mut hasher, DOMAIN);
    add_component(&mut hasher, quest_id);
    add_component(&mut hasher, kind);

    if let Some(source_id) = source_counter_id.filter(|value| !value.trim().is_empty()) {
        add_component(&mut hasher, "source-id");
        add_component(&mut hasher, &normalize_ref(source_id));
    } else {
        add_component(&mut hasher, "semantic");
        add_component(
            &mut hasher,
            &custom_name.map(normalize_ref).unwrap_or_default(),
        );
        let targets: BTreeSet<String> = targets.into_iter().map(normalize_ref).collect();
        for target in targets {
            add_component(&mut hasher, &target);
        }
    }

    format!("{quest_id}.objective.{}", hasher.finalize().to_hex())
}

fn add_component(hasher: &mut blake3::Hasher, value: &str) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn normalize_ref(value: &str) -> String {
    value
        .trim()
        .replace('\\', "/")
        .split('#')
        .next()
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_counter_href_is_stable_across_spelling_noise() {
        let quest = "quest.inst-league1.quest-1-30";
        let left = derive_objective_id(
            quest,
            "quest-count-special",
            Some("/World/Quests/InstLeague1/Quest_1_30/CountId_1.xdb#xpointer(/Count)"),
            None,
            std::iter::empty(),
        );
        let right = derive_objective_id(
            quest,
            "quest-count-special",
            Some(" world\\quests\\instleague1\\quest_1_30\\countid_1.xdb "),
            None,
            std::iter::empty(),
        );
        assert_eq!(left, right);
        assert!(
            left.strip_prefix(&format!("{quest}.objective."))
                .is_some_and(|digest| digest.len() == 64
                    && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        );
    }

    #[test]
    fn fallback_identity_sorts_and_deduplicates_targets() {
        let left = derive_objective_id(
            "quest.test.first",
            "quest-count-kill",
            None,
            Some("Counter.txt#ignored"),
            ["/Mobs/B.xdb#b", "/Mobs/A.xdb#a", "/Mobs/A.xdb#other"],
        );
        let right = derive_objective_id(
            "quest.test.first",
            "quest-count-kill",
            None,
            Some("counter.txt"),
            ["mobs/a.xdb", "mobs/b.xdb"],
        );
        assert_eq!(left, right);
    }
}
