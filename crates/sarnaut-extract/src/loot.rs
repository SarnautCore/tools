//! Loot-table extraction, closed over the tables one zone's mob kinds reach.
//!
//! There are 4,234 loot trees in the source tree and M2 needs the few hundred a
//! single zone can drop, so extraction is closure-driven from `MobKind.lootTable`
//! rather than a directory sweep.
//!
//! The one structural invariant worth being loud about: a container node's `entries`
//! and `chances` are two parallel arrays. A length mismatch means the file disagrees
//! with itself, and silently zipping to the shorter of the two would emit a tree that
//! validates, loads, and quietly never drops the tail. It is a hard error.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use anyhow::{Context, Result, bail};
use roxmltree::Node;

use crate::mobkind::{CLASSIC_SERVER_ROOT, zone_loot_table_hrefs};
use crate::model::{
    ExtractionOptions, ItemIndexDocument, LootNode, LootSummary, LootTableDocument, ResourceRef,
};
use crate::output::OutputWriter;
use crate::reference::{canonical_id_from_source_path, resource_ref, slug};
use crate::scan::{resolve_href, sorted_xdb_files};
use crate::validation::SchemaKind;
use crate::xdb::{
    descendant_hrefs, extra_fields, i64_value, parse_document, read_xdb_from, resource_id,
};

const LOOT_TABLE_RESOURCE: &str = "gameMechanics.constructor.schemes.item.LootTableResource";
const ITEM_RESOURCE: &str = "gameMechanics.constructor.schemes.item.ItemResource";
const LOOT_TABLES_ROOT: &str = "World/LootTables/";

pub fn extract_loot(zone: &str, options: &ExtractionOptions) -> Result<LootSummary> {
    let writer = OutputWriter::new(options.dry_run, options.schema_dir.as_deref())?;
    let mut summary = LootSummary {
        zone: zone.to_owned(),
        ..LootSummary::default()
    };

    let index = build_item_index(&options.src)?;
    summary.item_index = index.len();

    let seeds = zone_loot_table_hrefs(&options.src, zone)?;
    let mut queue: VecDeque<String> = seeds.iter().cloned().collect();
    let mut seen: BTreeSet<String> = seeds;
    let mut documents: BTreeMap<String, LootTableDocument> = BTreeMap::new();
    let mut dangling: BTreeSet<String> = BTreeSet::new();

    while let Some(relative) = queue.pop_front() {
        let path = options.src.join(&relative);
        if !path.is_file() {
            bail!(
                "loot table {relative} does not exist below {}",
                options.src.display()
            );
        }
        let xdb = read_xdb_from(&path, &options.src, Some(CLASSIC_SERVER_ROOT))?;
        let xml = parse_document(&xdb.text, &path)?;
        let root = xml.root_element();
        if root.tag_name().name() != LOOT_TABLE_RESOURCE {
            bail!(
                "{relative} is a {}, not a {LOOT_TABLE_RESOURCE}",
                root.tag_name().name()
            );
        }
        let id = canonical_id_from_source_path(&relative)
            .with_context(|| format!("build loot table ID from {relative}"))?;
        let table = crate::xdb::child(root, "table")
            .with_context(|| format!("{relative} carries no <table>"))?;
        let node =
            loot_node(table, &relative).with_context(|| format!("parse loot tree {relative}"))?;
        summary.nodes += count_nodes(&node);
        collect_item_refs(&node, &index, &mut dangling);

        // A loot node has never been observed to reference another table, but the
        // closure follows one if it appears rather than dropping it on the floor.
        for reference in descendant_hrefs(root) {
            let Some((_, target)) = resolve_href(&options.src, &path, &reference) else {
                continue;
            };
            if target.starts_with(LOOT_TABLES_ROOT) && seen.insert(target.clone()) {
                queue.push_back(target);
            }
        }

        documents.insert(
            id.clone(),
            LootTableDocument {
                schema_version: 1,
                id,
                source_type: root.tag_name().name().to_owned(),
                resource_id: resource_id(root),
                root: node,
                extra: extra_fields(root, &["Header", "table"]),
                source: xdb.source,
            },
        );
    }

    for (id, document) in &documents {
        let file = id.strip_prefix("loot.").context("loot table id prefix")?;
        let output = options.out.join("loot").join(format!("{file}.yaml"));
        summary.unchanged += usize::from(writer.write(&output, SchemaKind::LootTable, document)?);
        summary.tables += 1;
    }

    let index_document = ItemIndexDocument {
        schema_version: 1,
        id: "index.items".into(),
        kind: "item-index".into(),
        count: index.len(),
        entries: index,
    };
    summary.unchanged += usize::from(
        writer.write_unvalidated(&options.out.join("items/index.yaml"), &index_document)?,
    );

    summary.dangling_items = dangling.into_iter().collect();
    Ok(summary)
}

/// Parse one `<table>` or `<Item>` node into the authored loot tree.
fn loot_node(node: Node<'_, '_>, relative: &str) -> Result<LootNode> {
    let node_type = node
        .attribute("type")
        .with_context(|| format!("{} has no type attribute", node.tag_name().name()))?;
    let tail = node_type.rsplit('.').next().unwrap_or(node_type);
    match tail {
        "LootTableAnd" | "LootTableOr" => {
            let entries: Vec<Node<'_, '_>> = crate::xdb::child(node, "entries")
                .map(|parent| crate::xdb::children(parent, "Item").collect())
                .unwrap_or_default();
            let chances: Vec<f64> = crate::xdb::child(node, "chances")
                .map(|parent| {
                    crate::xdb::children(parent, "Item")
                        .map(|item| {
                            item.text()
                                .map(str::trim)
                                .and_then(|value| value.parse::<f64>().ok())
                                .with_context(|| format!("non-numeric chance in {relative}"))
                        })
                        .collect::<Result<Vec<f64>>>()
                })
                .transpose()?
                .unwrap_or_default();
            if entries.len() != chances.len() {
                bail!(
                    "{relative}: {tail} has {} entries but {} chances; entries and chances are positionally paired",
                    entries.len(),
                    chances.len()
                );
            }
            if entries.is_empty() {
                bail!("{relative}: {tail} has no entries");
            }
            Ok(LootNode::Container {
                node: slug(tail.trim_start_matches("LootTable")),
                entries: entries
                    .into_iter()
                    .map(|entry| loot_node(entry, relative))
                    .collect::<Result<_>>()?,
                chances,
            })
        }
        "LootTableSingleItem" => {
            let item = crate::xdb::href(node, "item")
                .with_context(|| format!("{relative}: single-item node has no item href"))?;
            Ok(LootNode::SingleItem {
                node: "single-item".into(),
                item: resource_ref(&item),
                min_number: counts(node, relative, "minNumber")?,
                max_number: counts(node, relative, "maxNumber")?,
            })
        }
        "LootTableMoney" => Ok(LootNode::Money {
            node: "money".into(),
            min_number: counts(node, relative, "minNumber")?,
            max_number: counts(node, relative, "maxNumber")?,
        }),
        other => bail!("{relative}: unknown loot node type {other}"),
    }
}

fn counts(node: Node<'_, '_>, relative: &str, field: &str) -> Result<i64> {
    i64_value(node, field).with_context(|| format!("{relative}: loot node has no <{field}>"))
}

fn count_nodes(node: &LootNode) -> usize {
    match node {
        LootNode::Container { entries, .. } => 1 + entries.iter().map(count_nodes).sum::<usize>(),
        _ => 1,
    }
}

fn collect_item_refs(
    node: &LootNode,
    index: &BTreeMap<String, String>,
    dangling: &mut BTreeSet<String>,
) {
    match node {
        LootNode::Container { entries, .. } => {
            for entry in entries {
                collect_item_refs(entry, index, dangling);
            }
        }
        LootNode::SingleItem { item, .. } => {
            let ResourceRef { id, href } = item;
            match id {
                Some(value) if index.contains_key(value) => {}
                _ => {
                    dangling.insert(href.clone());
                }
            }
        }
        LootNode::Money { .. } => {}
    }
}

/// Map every canonical item id to the ruleset-relative YAML that defines it, so the
/// pack build resolves an item reference by lookup instead of walking 36,980 files.
fn build_item_index(src: &Path) -> Result<BTreeMap<String, String>> {
    let items_root = src.join("Items");
    if !items_root.is_dir() {
        bail!(
            "item source directory does not exist: {}",
            items_root.display()
        );
    }
    let mut index = BTreeMap::new();
    for path in sorted_xdb_files(&items_root)? {
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        if root_element_name(&bytes).as_deref() != Some(ITEM_RESOURCE) {
            continue;
        }
        let relative = crate::reference::source_path(&path, src)?;
        let Some(id) = canonical_id_from_source_path(&relative) else {
            continue;
        };
        let category = path
            .strip_prefix(&items_root)?
            .components()
            .next()
            .context("item has no category")?
            .as_os_str()
            .to_string_lossy()
            .into_owned();
        let category = slug(&category);
        let file = id
            .strip_prefix(&format!("item.{category}."))
            .context("item ID category prefix")?;
        if let Some(previous) = index.insert(id.clone(), format!("items/{category}/{file}.yaml")) {
            bail!("canonical item ID collision for {id}: {previous} and {relative}");
        }
    }
    Ok(index)
}

/// The name of the document element, read without building a full XML tree.
///
/// The index visits every item file in the tree, and parsing 36,980 documents to
/// learn one tag name each is the difference between a snappy run and a slow one.
fn root_element_name(bytes: &[u8]) -> Option<String> {
    let mut rest = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    loop {
        let start = rest.iter().position(|byte| *byte == b'<')?;
        rest = &rest[start..];
        if let Some(after) = rest.strip_prefix(b"<?") {
            let end = find(after, b"?>")?;
            rest = &after[end + 2..];
        } else if let Some(after) = rest.strip_prefix(b"<!--") {
            let end = find(after, b"-->")?;
            rest = &after[end + 3..];
        } else if let Some(after) = rest.strip_prefix(b"<!") {
            let end = after.iter().position(|byte| *byte == b'>')?;
            rest = &after[end + 1..];
        } else {
            let name: Vec<u8> = rest[1..]
                .iter()
                .copied()
                .take_while(|byte| !byte.is_ascii_whitespace() && !b"/>".contains(byte))
                .collect();
            return String::from_utf8(name).ok().filter(|it| !it.is_empty());
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::testing::{fixture_options, write};

    fn zone_with_loot(root: &Path, tables: &str) {
        write(
            &root.join("Mechanics/Creatures/Zombie/ZombieKind.(MobKind).xdb"),
            &format!(
                r#"<gameMechanics.world.mob.MobKind><Header><resourceId>1</resourceId></Header><lootTable>{tables}</lootTable></gameMechanics.world.mob.MobKind>"#
            ),
        );
        write(
            &root.join("Creatures/Zombie/Instances/TestZone/Zombie1.(MobWorld).xdb"),
            r#"<gameMechanics.world.mob.MobWorld><Header><resourceId>2</resourceId></Header><kind href="/Mechanics/Creatures/Zombie/ZombieKind.(MobKind).xdb#x"/></gameMechanics.world.mob.MobWorld>"#,
        );
        write(
            &root.join("Items/Mechanics/ZombieDenture.xdb"),
            r#"<gameMechanics.constructor.schemes.item.ItemResource><Header><resourceId>3</resourceId></Header></gameMechanics.constructor.schemes.item.ItemResource>"#,
        );
        write(&root.join("Items/Mechanics/Visual.xdb"), "<VisualItem/>");
    }

    #[test]
    fn extracts_a_flat_and_table_and_round_trips_the_literal_chance() {
        let (source, output, options) = fixture_options();
        zone_with_loot(
            source.path(),
            r#"<Item href="/World/LootTables/Zombie/Zombie(01).xdb#x"/>"#,
        );
        write(
            &source.path().join("World/LootTables/Zombie/Zombie(01).xdb"),
            r#"<gameMechanics.constructor.schemes.item.LootTableResource><Header><resourceId>9</resourceId></Header>
<table type="gameMechanics.constructor.schemes.item.LootTableAnd">
  <entries>
    <Item type="gameMechanics.constructor.schemes.item.LootTableMoney"><minNumber>2</minNumber><maxNumber>6</maxNumber></Item>
    <Item type="gameMechanics.constructor.schemes.item.LootTableSingleItem"><item href="/Items/Mechanics/ZombieDenture.xdb#x"/><minNumber>1</minNumber><maxNumber>1</maxNumber></Item>
  </entries>
  <chances><Item>1</Item><Item>0.00618751</Item></chances>
</table></gameMechanics.constructor.schemes.item.LootTableResource>"#,
        );

        let summary = extract_loot("TestZone", &options).unwrap();
        assert_eq!(summary.tables, 1);
        assert_eq!(summary.nodes, 3);
        assert_eq!(summary.item_index, 1);
        assert!(summary.dangling_items.is_empty());

        let yaml = fs::read_to_string(output.path().join("loot/zombie.zombie-01.yaml")).unwrap();
        assert!(yaml.contains("node: and"), "{yaml}");
        assert!(yaml.contains("node: money"), "{yaml}");
        assert!(yaml.contains("node: single-item"), "{yaml}");
        assert!(yaml.contains("id: item.mechanics.zombie-denture"), "{yaml}");
        assert!(yaml.contains("- 0.00618751"), "{yaml}");

        let index = fs::read_to_string(output.path().join("items/index.yaml")).unwrap();
        assert!(
            index.contains("item.mechanics.zombie-denture: items/mechanics/zombie-denture.yaml"),
            "{index}"
        );
        assert!(!index.contains("visual"), "{index}");

        assert_eq!(extract_loot("TestZone", &options).unwrap().unchanged, 2);
    }

    #[test]
    fn extracts_a_nested_and_inside_an_or() {
        let (source, output, options) = fixture_options();
        zone_with_loot(
            source.path(),
            r#"<Item href="/World/LootTables/Zombie/Zombie(02).xdb#x"/>"#,
        );
        write(
            &source.path().join("World/LootTables/Zombie/Zombie(02).xdb"),
            r#"<gameMechanics.constructor.schemes.item.LootTableResource><Header><resourceId>10</resourceId></Header>
<table type="gameMechanics.constructor.schemes.item.LootTableOr">
  <entries>
    <Item type="gameMechanics.constructor.schemes.item.LootTableAnd">
      <entries><Item type="gameMechanics.constructor.schemes.item.LootTableMoney"><minNumber>1</minNumber><maxNumber>2</maxNumber></Item></entries>
      <chances><Item>0.5</Item></chances>
    </Item>
    <Item type="gameMechanics.constructor.schemes.item.LootTableSingleItem"><item href="/Items/Mechanics/ZombieDenture.xdb#x"/><minNumber>1</minNumber><maxNumber>3</maxNumber></Item>
  </entries>
  <chances><Item>0.25</Item><Item>0.75</Item></chances>
</table></gameMechanics.constructor.schemes.item.LootTableResource>"#,
        );

        let summary = extract_loot("TestZone", &options).unwrap();
        assert_eq!(summary.nodes, 4);
        let yaml = fs::read_to_string(output.path().join("loot/zombie.zombie-02.yaml")).unwrap();
        assert!(yaml.contains("node: or"), "{yaml}");
        assert!(yaml.contains("node: and"), "{yaml}");
        assert!(yaml.contains("max_number: 3"), "{yaml}");
    }

    #[test]
    fn an_entries_chances_length_mismatch_is_an_error() {
        let (source, _output, options) = fixture_options();
        zone_with_loot(
            source.path(),
            r#"<Item href="/World/LootTables/Zombie/Zombie(03).xdb#x"/>"#,
        );
        write(
            &source.path().join("World/LootTables/Zombie/Zombie(03).xdb"),
            r#"<gameMechanics.constructor.schemes.item.LootTableResource><Header><resourceId>11</resourceId></Header>
<table type="gameMechanics.constructor.schemes.item.LootTableAnd">
  <entries>
    <Item type="gameMechanics.constructor.schemes.item.LootTableMoney"><minNumber>1</minNumber><maxNumber>2</maxNumber></Item>
    <Item type="gameMechanics.constructor.schemes.item.LootTableSingleItem"><item href="/Items/Mechanics/ZombieDenture.xdb#x"/><minNumber>1</minNumber><maxNumber>1</maxNumber></Item>
  </entries>
  <chances><Item>1</Item></chances>
</table></gameMechanics.constructor.schemes.item.LootTableResource>"#,
        );

        let rendered = format!("{:#}", extract_loot("TestZone", &options).unwrap_err());
        assert!(rendered.contains("2 entries but 1 chances"), "{rendered}");
    }

    #[test]
    fn reports_a_loot_reference_to_an_item_that_is_not_in_the_tree() {
        let (source, _output, options) = fixture_options();
        zone_with_loot(
            source.path(),
            r#"<Item href="/World/LootTables/Zombie/Zombie(04).xdb#x"/>"#,
        );
        write(
            &source.path().join("World/LootTables/Zombie/Zombie(04).xdb"),
            r#"<gameMechanics.constructor.schemes.item.LootTableResource><Header><resourceId>12</resourceId></Header>
<table type="gameMechanics.constructor.schemes.item.LootTableAnd">
  <entries><Item type="gameMechanics.constructor.schemes.item.LootTableSingleItem"><item href="/Items/Mechanics/Ghost.xdb#x"/><minNumber>1</minNumber><maxNumber>1</maxNumber></Item></entries>
  <chances><Item>1</Item></chances>
</table></gameMechanics.constructor.schemes.item.LootTableResource>"#,
        );

        let summary = extract_loot("TestZone", &options).unwrap();
        assert_eq!(summary.dangling_items, vec!["/Items/Mechanics/Ghost.xdb#x"]);
    }

    #[test]
    fn reads_the_document_element_without_parsing_the_document() {
        assert_eq!(
            root_element_name(b"<?xml version=\"1.0\"?>\n<!-- note -->\n<Root a=\"1\"/>")
                .as_deref(),
            Some("Root")
        );
        assert_eq!(root_element_name(b"<Bare>").as_deref(), Some("Bare"));
        assert_eq!(root_element_name(b"   "), None);
    }
}
