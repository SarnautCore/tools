//! Localization extraction: turn every `loc_ref` into the string it points at.
//!
//! A `loc_ref` is an href at a `.txt` sibling of the resource that carries it. The
//! reference tree `servers-clean/1.1.02.0` ships about 705 of those files while the
//! wider `servers/1.1` tree ships tens of thousands, so most strings only exist in
//! the supplemental root. The two are different builds of the same game, which is why
//! every entry records the root that supplied it and a key present in both with
//! different text is reported rather than silently resolved from whichever root won.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::encoding::decode;
use crate::mobkind::{CLASSIC_SERVER_ROOT, zone_source_files};
use crate::model::{LocaleDocument, LocaleEntry, LocaleOptions, LocaleSummary, Provenance};
use crate::output::OutputWriter;
use crate::reference::slug;
use crate::scan::{find_named_directory, resolve_href, sort_paths, sorted_xdb_files, strip_txt};
use crate::validation::SchemaKind;
use crate::xdb::{descendant_hrefs, parse_document, read_xdb_from};

/// The `servers/1.1` tree, read only to fill gaps the reference tree leaves.
pub(crate) const SUPPLEMENTAL_SERVER_ROOT: &str = "classic-server-1-1";

pub fn extract_locale(zone: &str, options: &LocaleOptions) -> Result<LocaleSummary> {
    let common = &options.common;
    let writer = OutputWriter::new(common.dry_run, common.schema_dir.as_deref())?;
    let zone_slug = slug(zone);
    let mut summary = LocaleSummary {
        zone: zone_slug.clone(),
        language: options.language.clone(),
        ..LocaleSummary::default()
    };

    let mut requests: BTreeSet<String> = BTreeSet::new();
    for path in referrer_files(&common.src, zone)? {
        let xdb = read_xdb_from(&path, &common.src, Some(CLASSIC_SERVER_ROOT))?;
        let xml = parse_document(&xdb.text, &path)?;
        for href in descendant_hrefs(xml.root_element()) {
            if !is_loc_ref(&href) {
                continue;
            }
            let Some((_, relative)) = resolve_href(&common.src, &path, &href) else {
                continue;
            };
            requests.insert(relative);
        }
    }
    summary.requested = requests.len();

    let mut entries = Vec::new();
    for relative in &requests {
        let key = strip_txt(relative).to_owned();
        let primary = read_string(&common.src.join(relative), &mut summary.encodings)?;
        let supplemental = match &options.supplemental_src {
            Some(root) => read_string(&root.join(relative), &mut summary.encodings)?,
            None => None,
        };
        match (primary, supplemental) {
            (Some(text), other) => {
                if other.is_some_and(|value| value != text) {
                    summary.mismatched.push(key.clone());
                }
                summary.from_primary += 1;
                entries.push(LocaleEntry {
                    key,
                    text,
                    source_root: Some(CLASSIC_SERVER_ROOT.to_owned()),
                });
            }
            (None, Some(text)) => {
                summary.from_supplemental += 1;
                entries.push(LocaleEntry {
                    key,
                    text,
                    source_root: Some(SUPPLEMENTAL_SERVER_ROOT.to_owned()),
                });
            }
            (None, None) => summary.unresolved.push(key),
        }
    }
    summary.resolved = entries.len();
    if entries.is_empty() {
        bail!("no loc_ref resolved for zone {zone}; nothing to write");
    }

    // The document-level root is whichever supplied more entries; the minority keep
    // an explicit per-entry override so no string's origin is ambiguous.
    let document_root = if summary.from_primary >= summary.from_supplemental {
        CLASSIC_SERVER_ROOT
    } else {
        SUPPLEMENTAL_SERVER_ROOT
    };
    for entry in &mut entries {
        if entry.source_root.as_deref() == Some(document_root) {
            entry.source_root = None;
        }
    }

    let document = LocaleDocument {
        schema_version: 1,
        id: format!("locale.{}.{zone_slug}", options.language),
        language: options.language.clone(),
        source_root: document_root.to_owned(),
        source_type: "locTxt".into(),
        entries,
        extra: BTreeMap::new(),
        source: Provenance {
            path: format!("{zone_slug} loc_ref closure"),
            blake3: blake3::hash(
                requests
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n")
                    .as_bytes(),
            )
            .to_hex()
            .to_string(),
            extractor: format!("sarnaut-extract@{}", env!("CARGO_PKG_VERSION")),
            source_root: Some(document_root.to_owned()),
            prototype_chain: None,
        },
    };
    let output = common
        .out
        .join("locale")
        .join(&options.language)
        .join(format!("{zone_slug}.yaml"));
    summary.unchanged += usize::from(writer.write(&output, SchemaKind::Locale, &document)?);
    Ok(summary)
}

/// Every source document whose `loc_ref`s this zone's content depends on.
fn referrer_files(src: &Path, zone: &str) -> Result<Vec<PathBuf>> {
    let mut files = zone_source_files(src, zone)?;
    let quest_root = src.join("World/Quests");
    let quest_dir = find_named_directory(&quest_root, zone)
        .with_context(|| format!("find quest zone {zone} in {}", quest_root.display()))?;
    files.extend(sorted_xdb_files(&quest_dir)?);
    sort_paths(&mut files);
    files.dedup();
    Ok(files)
}

fn is_loc_ref(href: &str) -> bool {
    href.split('#')
        .next()
        .is_some_and(|path| path.to_ascii_lowercase().ends_with(".txt"))
}

fn read_string(path: &Path, encodings: &mut BTreeMap<String, usize>) -> Result<Option<String>> {
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let (text, encoding) = decode(&bytes);
    *encodings.entry(encoding.label().to_owned()).or_default() += 1;
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::testing::{fixture_options, write, write_bytes};

    fn utf16le(text: &str) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    fn zone(root: &Path) {
        write(
            &root.join("Creatures/Zombie/Instances/TestZone/Zombie1.(MobWorld).xdb"),
            r#"<gameMechanics.world.mob.MobWorld><Header><resourceId>1</resourceId></Header>
<name href="Zombie1_Name.txt"/>
<kind href="/Mechanics/Creatures/Zombie/ZombieKind.(MobKind).xdb#x"/>
</gameMechanics.world.mob.MobWorld>"#,
        );
        write(
            &root.join("Mechanics/Creatures/Zombie/ZombieKind.(MobKind).xdb"),
            r#"<gameMechanics.world.mob.MobKind><Header><resourceId>2</resourceId></Header></gameMechanics.world.mob.MobKind>"#,
        );
        write(
            &root.join("World/Quests/TestZone/Quest_1_10/Quest_1_10.xdb"),
            r#"<gameMechanics.constructor.schemes.quest.QuestResource><Header><resourceId>3</resourceId></Header>
<name href="Name.txt"/><goal href="GoalText.txt"/><startText href="StartText.txt"/>
<checkText href="CheckText.txt"/><finishText href="FinishText.txt"/>
<missing href="Absent.txt"/>
</gameMechanics.constructor.schemes.quest.QuestResource>"#,
        );
    }

    #[test]
    fn resolves_from_both_roots_and_reports_what_it_could_not_find() {
        let (source, output, common) = fixture_options();
        let supplemental = tempfile::tempdir().unwrap();
        zone(source.path());

        // The reference tree ships only the quest name; the rest is gap-filled.
        write_bytes(
            &source
                .path()
                .join("World/Quests/TestZone/Quest_1_10/Name.txt"),
            &utf16le("Костёр"),
        );
        for (file, text) in [
            ("Name.txt", "Костёр (старый билд)"),
            ("GoalText.txt", "Цель"),
            ("StartText.txt", "Начало"),
            ("CheckText.txt", "Проверка"),
            ("FinishText.txt", "Конец"),
        ] {
            write_bytes(
                &supplemental
                    .path()
                    .join("World/Quests/TestZone/Quest_1_10")
                    .join(file),
                &utf16le(text),
            );
        }
        write_bytes(
            &supplemental
                .path()
                .join("Creatures/Zombie/Instances/TestZone/Zombie1_Name.txt"),
            &utf16le("Зомби"),
        );

        let options = LocaleOptions {
            common,
            language: "ru".into(),
            supplemental_src: Some(supplemental.path().to_path_buf()),
        };
        let summary = extract_locale("TestZone", &options).unwrap();
        assert_eq!(summary.requested, 7);
        assert_eq!(summary.resolved, 6);
        assert_eq!(summary.from_primary, 1);
        assert_eq!(summary.from_supplemental, 5);
        assert_eq!(summary.encodings.get("utf-16le"), Some(&7));
        assert_eq!(
            summary.unresolved,
            vec!["World/Quests/TestZone/Quest_1_10/Absent"]
        );
        assert_eq!(
            summary.mismatched,
            vec!["World/Quests/TestZone/Quest_1_10/Name"]
        );
        assert!((summary.unresolved_rate() - 100.0 / 7.0).abs() < 1e-9);

        let yaml = fs::read_to_string(output.path().join("locale/ru/test-zone.yaml")).unwrap();
        assert!(yaml.contains("id: locale.ru.test-zone"), "{yaml}");
        assert!(yaml.contains("source_root: classic-server-1-1\n"), "{yaml}");
        assert!(yaml.contains("Зомби"), "{yaml}");
        // The single reference-tree string keeps an explicit override.
        assert!(
            yaml.contains("source_root: classic-server-1-1-02-0"),
            "{yaml}"
        );
        assert!(yaml.contains("text: Костёр"), "{yaml}");

        assert_eq!(extract_locale("TestZone", &options).unwrap().unchanged, 1);
    }

    #[test]
    fn a_missing_loc_ref_target_is_counted_not_fatal() {
        let (source, _output, common) = fixture_options();
        zone(source.path());
        write_bytes(
            &source
                .path()
                .join("Creatures/Zombie/Instances/TestZone/Zombie1_Name.txt"),
            &utf16le("Зомби"),
        );

        let options = LocaleOptions {
            common,
            language: "ru".into(),
            supplemental_src: None,
        };
        let summary = extract_locale("TestZone", &options).unwrap();
        assert_eq!(summary.resolved, 1);
        assert_eq!(summary.unresolved.len(), 6);
        assert!(summary.unresolved_rate() > 85.0);
    }
}
