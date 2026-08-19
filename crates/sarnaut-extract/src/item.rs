use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use roxmltree::Node;
use walkdir::WalkDir;

use crate::model::{
    CombatStats, ExtractionOptions, ItemDocument, ItemSummary, LocRefs, StatBonus, VendorPrice,
};
use crate::output::OutputWriter;
use crate::reference::{canonical_id_from_source_path, resource_ref, slug};
use crate::validation::SchemaKind;
use crate::xdb::{
    bool_value, child, children, extra_fields, f64_value, href, i64_value, parse_document,
    read_xdb, resource_id, text,
};

const ITEM_RESOURCE: &str = "gameMechanics.constructor.schemes.item.ItemResource";

pub fn extract_items(options: &ExtractionOptions) -> Result<ItemSummary> {
    let items_root = options.src.join("Items");
    if !items_root.is_dir() {
        bail!(
            "item source directory does not exist: {}",
            items_root.display()
        );
    }
    let writer = OutputWriter::new(options.dry_run, options.schema_dir.as_deref())?;
    let mut files = xdb_files(&items_root)?;
    files.sort_by_cached_key(|path| path.to_string_lossy().to_ascii_lowercase());

    let mut summary = ItemSummary::default();
    for entry in std::fs::read_dir(&items_root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            summary
                .categories
                .insert(slug(&entry.file_name().to_string_lossy()), 0);
        }
    }
    let mut ids = BTreeSet::new();
    for path in files {
        let xdb = read_xdb(&path, &options.src)?;
        let xml = parse_document(&xdb.text, &path)?;
        let root = xml.root_element();
        if root.tag_name().name() != ITEM_RESOURCE {
            summary.skipped_auxiliary += 1;
            continue;
        }

        let relative = xdb.source.path.as_str();
        let id = canonical_id_from_source_path(relative)
            .with_context(|| format!("build item ID from {relative}"))?;
        if !ids.insert(id.clone()) {
            bail!("canonical item ID collision for {id} at {relative}");
        }
        let category_source = path
            .strip_prefix(&items_root)?
            .components()
            .next()
            .context("item has no category")?
            .as_os_str()
            .to_string_lossy();
        let category = slug(&category_source);
        let document = item_document(root, id.clone(), category.clone(), xdb.source);
        let file_slug = id
            .strip_prefix(&format!("item.{category}."))
            .context("item ID category prefix")?;
        let output = options
            .out
            .join("items")
            .join(&category)
            .join(format!("{file_slug}.yaml"));
        summary.unchanged += usize::from(writer.write(&output, SchemaKind::Item, &document)?);
        summary.emitted += 1;
        *summary.categories.entry(category).or_default() += 1;
    }
    Ok(summary)
}

fn item_document(
    root: Node<'_, '_>,
    id: String,
    category: String,
    source: crate::model::Provenance,
) -> ItemDocument {
    let functional = child(root, "functionalPart");
    let stats = functional
        .and_then(|node| child(node, "statBonuses"))
        .map(|node| {
            children(node, "Item")
                .filter_map(|entry| {
                    Some(StatBonus {
                        stat: text(entry, "stat")?,
                        value: f64_value(entry, "value")?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let combat = functional
        .map(combat_stats)
        .filter(|value| !value.is_empty());
    let sell = i64_value(root, "sellPrice");
    let buy = i64_value(root, "buyPrice");
    let vendor_price = (sell.is_some() || buy.is_some()).then_some(VendorPrice { sell, buy });
    let mut flags = BTreeMap::new();
    for (source_name, output_name) in [
        ("includeInBoxes", "include_in_boxes"),
        ("customSellPrice", "custom_sell_price"),
        ("customBuyPrice", "custom_buy_price"),
        ("isQuestRelated", "quest_related"),
        ("canDrop", "can_drop"),
        ("notForItemMall", "not_for_item_mall"),
        ("nameChecked", "name_checked"),
    ] {
        if let Some(value) = bool_value(root, source_name) {
            flags.insert(output_name.to_owned(), value);
        }
    }

    ItemDocument {
        schema_version: 1,
        id,
        category,
        source_type: root.tag_name().name().to_owned(),
        resource_id: resource_id(root),
        sys_name: text(root, "sysName"),
        loc_ref: LocRefs {
            name: href(root, "name"),
            description: href(root, "description"),
            bind_description: href(root, "bindDescription"),
            ..LocRefs::default()
        },
        image_ref: href(root, "image"),
        visual_ref: href(root, "visualElement"),
        level: i64_value(root, "level"),
        required_level: i64_value(root, "requiredLevel"),
        quality: href(root, "quality").map(|value| resource_ref(&value)),
        slot: text(root, "slot").map(|value| slug(&value)),
        item_class: href(root, "itemClass").map(|value| resource_ref(&value)),
        item_category: href(root, "category").map(|value| resource_ref(&value)),
        binding: text(root, "binding").map(|value| slug(&value)),
        stack_limit: i64_value(root, "stackLimit"),
        vendor_price,
        stats,
        combat,
        capacity: functional.and_then(|node| i64_value(node, "baseSize")),
        spell_ref: href(root, "spell"),
        flags,
        extra: extra_fields(
            root,
            &[
                "Header",
                "sysName",
                "name",
                "description",
                "bindDescription",
                "image",
                "visualElement",
                "level",
                "requiredLevel",
                "quality",
                "slot",
                "itemClass",
                "category",
                "binding",
                "stackLimit",
                "sellPrice",
                "buyPrice",
                "spell",
                "includeInBoxes",
                "customSellPrice",
                "customBuyPrice",
                "isQuestRelated",
                "canDrop",
                "notForItemMall",
                "nameChecked",
            ],
        ),
        source,
    }
}

fn combat_stats(functional: Node<'_, '_>) -> CombatStats {
    CombatStats {
        armor: f64_value(functional, "armor"),
        min_damage: f64_value(functional, "minDamage"),
        max_damage: f64_value(functional, "maxDamage"),
        weapon_speed: f64_value(functional, "weaponSpeed"),
        spell_power: f64_value(functional, "spellPower"),
        damage_element: text(functional, "damageElement").map(|value| slug(&value)),
        resist_divine: f64_value(functional, "resistDivine"),
        resist_elemental: f64_value(functional, "resistElemental"),
        resist_nature: f64_value(functional, "resistNature"),
    }
}

fn xdb_files(root: &Path) -> Result<Vec<PathBuf>> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| match entry {
            Ok(entry)
                if entry.file_type().is_file()
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("xdb")) =>
            {
                Some(Ok(entry.into_path()))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error.into())),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn extracts_item_fields_and_keeps_unmapped_xml() {
        let source = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let category = source.path().join("Items/Junk");
        fs::create_dir_all(&category).unwrap();
        fs::write(
            category.join("GibberlingFur.xdb"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<gameMechanics.constructor.schemes.item.ItemResource>
  <Header><resourceId>42</resourceId></Header>
  <name href="GibberlingFur.txt"/><description href="GibberlingFur.Description.txt"/>
  <slot>RANGED</slot><level>7</level><requiredLevel>5</requiredLevel>
  <quality href="/Mechanics/ItemQualities/Common.xdb#xpointer(/quality)"/>
  <functionalPart type="ItemBonus"><statBonuses><Item><stat>IS_Luck</stat><value>3</value></Item></statBonuses><minDamage>2</minDamage><maxDamage>4</maxDamage></functionalPart>
  <stackLimit>20</stackLimit><sellPrice>11</sellPrice><buyPrice>22</buyPrice>
  <mystery answer="yes"><nested>kept</nested></mystery>
</gameMechanics.constructor.schemes.item.ItemResource>"#,
        )
        .unwrap();
        fs::write(category.join("Aux.xdb"), "<VisualItem/>").unwrap();

        let summary = extract_items(&ExtractionOptions {
            src: source.path().into(),
            out: output.path().into(),
            dry_run: false,
            schema_dir: None,
        })
        .unwrap();
        assert_eq!(summary.emitted, 1);
        assert_eq!(summary.skipped_auxiliary, 1);
        let yaml =
            fs::read_to_string(output.path().join("items/junk/gibberling-fur.yaml")).unwrap();
        assert!(yaml.contains("id: item.junk.gibberling-fur"));
        assert!(yaml.contains("stat: IS_Luck"));
        assert!(yaml.contains("mystery:"));
        assert!(yaml.contains("@answer"));
        assert!(yaml.contains("nested: kept"));
        assert!(yaml.contains("path: Items/Junk/GibberlingFur.xdb"));

        let second = extract_items(&ExtractionOptions {
            src: source.path().into(),
            out: output.path().into(),
            dry_run: false,
            schema_dir: None,
        })
        .unwrap();
        assert_eq!(second.unchanged, 1);
    }
}
