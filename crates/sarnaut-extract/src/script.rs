//! Quest script extraction (ADR 0036, M3-09): the counter bindings and the
//! `startImpacts` / `triggerAgents` trees of one zone's quests, plus every
//! `TriggerResource` document those trees reach by reference, emitted as typed
//! `ScriptNode` YAML rather than the untyped `extra:` passthrough.
//!
//! The representation mirrors `sarnaut.content.v1.ScriptNode` field for field.
//! Opcodes are stored with their source namespace stripped and every href is
//! resolved to a canonical id at extraction, so no MY.GAMES type name or
//! resource path survives into anything a pack compiles (ADR 0011).
//!
//! Trigger discovery dispatches on the XML root rather than filename markers:
//! `Quest_3_20`'s `TriggerAvatar.xdb` carries no `(TriggerResource)` suffix.
//! Every trigger rooted inside the selected quest tree is seeded, then hrefs
//! are followed transitively; this also pulls in the `IL_QuestSpells` kill
//! triggers that live outside the quest tree.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use roxmltree::Node;
use serde::Serialize;

use crate::model::{ExtractionOptions, Provenance};
use crate::objective::derive_objective_id;
use crate::output::OutputWriter;
use crate::reference::{
    canonical_id_from_source_path, canonical_zone_slug, slug, slug_path, strip_xdb_suffix,
};
use crate::scan::sorted_xdb_files;
use crate::tiers::tier_for;
use crate::validation::SchemaKind;
use crate::xdb::{child, children, href, parse_document, read_xdb, resource_id};
use crate::zone::resolve_zone;

const QUEST_RESOURCE: &str = "gameMechanics.constructor.schemes.quest.QuestResource";
const TRIGGER_RESOURCE: &str = "gameMechanics.constructor.schemes.quest.trigger.TriggerResource";

/// What one `scripts` run produced.
#[derive(Debug, Default)]
pub struct ScriptSummary {
    pub zone: String,
    pub quest_scripts: usize,
    pub triggers: usize,
    pub counters: usize,
    pub nodes: usize,
    pub implemented: usize,
    pub inert: usize,
    pub refused: usize,
    /// Refused-tier opcodes and how often each appears, for the census report.
    pub refused_opcodes: BTreeMap<String, usize>,
    /// Canonical ids of the root-element-discovered trigger set.
    pub trigger_ids: BTreeSet<String>,
    /// References no canonical namespace models yet, minted under `ext.`.
    pub external_resources: BTreeSet<String>,
    pub unchanged: usize,
}

// --- output document shapes -------------------------------------------------
// Serialize-side mirrors of the pack compiler's Deserialize documents; the two
// meet at the JSON Schema, which validates what travels between them.

#[derive(Debug, Serialize)]
struct QuestScriptDocument {
    schema_version: u32,
    id: String,
    zone: String,
    quest: String,
    source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource_id: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    counters: Vec<CounterBinding>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    start_impacts: Vec<ScriptNode>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    trigger_agents: Vec<ScriptNode>,
    #[serde(rename = "_source")]
    source: Provenance,
}

#[derive(Debug, Serialize)]
struct CounterBinding {
    count_id: String,
    objective: u32,
    objective_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    href: Option<String>,
}

#[derive(Debug, Serialize)]
struct TriggerDocument {
    schema_version: u32,
    id: String,
    zone: String,
    source_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource_id: Option<u64>,
    /// The map attaches this trigger outside the quest-script graph, for
    /// example through a SpawnTunerMobImpacts row. It is therefore a census
    /// root even when no quest node names it directly.
    #[serde(skip_serializing_if = "is_false")]
    entrypoint: bool,
    root: ScriptNode,
    #[serde(rename = "_source")]
    source: Provenance,
}

#[derive(Debug, Serialize)]
struct ScriptNode {
    key: String,
    family: String,
    opcode: String,
    tier: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    fields: Vec<ScriptField>,
}

#[derive(Debug, Serialize)]
struct ScriptField {
    name: String,
    value: ScriptValue,
}

#[derive(Debug, Default, Serialize)]
struct ScriptValue {
    #[serde(skip_serializing_if = "Option::is_none")]
    integer: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decimal: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    boolean: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reference: Option<Reference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node: Option<Box<ScriptNode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    list: Option<Vec<ScriptValue>>,
}

#[derive(Debug, Serialize)]
struct Decimal {
    mantissa: i64,
    scale: i32,
}

#[derive(Debug, Serialize)]
struct Reference {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    row_type: Option<String>,
    /// Verbatim source href, provenance for the private YAML only; the pack
    /// compiler never reads it (ADR 0011).
    #[serde(skip_serializing_if = "Option::is_none")]
    href: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

// --- extraction -------------------------------------------------------------

pub fn extract_scripts(name: &str, options: &ExtractionOptions) -> Result<ScriptSummary> {
    let resolved = resolve_zone(&options.src, name)?;
    let zone_slug = canonical_zone_slug(&resolved.zone);
    let zone_output = options.out.join("zones").join(&zone_slug);
    let writer = OutputWriter::new(options.dry_run, options.schema_dir.as_deref())?;
    let mut summary = ScriptSummary {
        zone: resolved.zone.clone(),
        ..ScriptSummary::default()
    };
    let external_entrypoints =
        map_trigger_entrypoints(&resolved.map_dir, &options.src, &zone_slug)?;

    // Triggers the quest trees referenced, id -> source-relative path. A queue
    // rather than a set walk, because a trigger can attach further triggers.
    let mut wanted_triggers: BTreeMap<String, String> = BTreeMap::new();
    let mut queue: VecDeque<String> = VecDeque::new();

    for path in sorted_xdb_files(&resolved.quest_dir)? {
        let xdb = read_xdb(&path, &options.src)?;
        let xml = parse_document(&xdb.text, &path)?;
        let root = xml.root_element();
        if root.tag_name().name() == TRIGGER_RESOURCE {
            let converter = Converter {
                zone_slug: &zone_slug,
                document_dir: parent_dir(&xdb.source.path),
                summary: &mut summary,
                wanted_triggers: &mut wanted_triggers,
                queue: &mut queue,
            };
            let trigger_id = converter.trigger_id(&xdb.source.path);
            if !wanted_triggers.contains_key(&trigger_id) {
                wanted_triggers.insert(trigger_id.clone(), xdb.source.path);
                queue.push_back(trigger_id);
            }
            continue;
        }
        if root.tag_name().name() != QUEST_RESOURCE {
            continue;
        }
        let quest_id = canonical_id_from_source_path(&xdb.source.path)
            .with_context(|| format!("build quest ID from {}", xdb.source.path))?;
        let script_id = format!(
            "script.{zone_slug}.{}",
            quest_id
                .strip_prefix(&format!("quest.{zone_slug}."))
                .context("quest ID zone prefix")?
        );

        let mut converter = Converter {
            zone_slug: &zone_slug,
            document_dir: parent_dir(&xdb.source.path),
            summary: &mut summary,
            wanted_triggers: &mut wanted_triggers,
            queue: &mut queue,
        };

        let counters = converter.counters(root, &script_id, &quest_id)?;
        let start_impacts = converter.node_list_field(root, "startImpacts", &script_id)?;
        let trigger_agents = converter.node_list_field(root, "triggerAgents", &script_id)?;

        let document = QuestScriptDocument {
            schema_version: 1,
            id: script_id.clone(),
            zone: format!("zone.{zone_slug}"),
            quest: quest_id,
            source_type: QUEST_RESOURCE.to_owned(),
            resource_id: resource_id(root),
            counters,
            start_impacts,
            trigger_agents,
            source: xdb.source,
        };
        summary.counters += document.counters.len();
        let file_slug = script_id
            .strip_prefix(&format!("script.{zone_slug}."))
            .context("script ID zone prefix")?;
        let output = zone_output
            .join("scripts/quests")
            .join(format!("{file_slug}.yaml"));
        summary.unchanged +=
            usize::from(writer.write(&output, SchemaKind::QuestScript, &document)?);
        summary.quest_scripts += 1;
    }

    // The discovered trigger corpus, transitively.
    let mut converted: BTreeSet<String> = BTreeSet::new();
    while let Some(trigger_id) = queue.pop_front() {
        if !converted.insert(trigger_id.clone()) {
            continue;
        }
        let source_path = wanted_triggers
            .get(&trigger_id)
            .cloned()
            .with_context(|| format!("trigger {trigger_id} has no recorded source path"))?;
        let disk_path: PathBuf = options.src.join(source_path.replace('/', "\\"));
        let disk_path = if disk_path.is_file() {
            disk_path
        } else {
            options.src.join(&source_path)
        };
        let xdb = read_xdb(&disk_path, &options.src).with_context(|| {
            format!("read trigger {trigger_id} from {source_path} (referenced by a quest tree)")
        })?;
        let xml = parse_document(&xdb.text, &disk_path)?;
        let root = xml.root_element();
        if root.tag_name().name() != TRIGGER_RESOURCE {
            bail!(
                "trigger {trigger_id} at {source_path} has root element {}, want {TRIGGER_RESOURCE}",
                root.tag_name().name()
            );
        }

        let mut converter = Converter {
            zone_slug: &zone_slug,
            document_dir: parent_dir(&xdb.source.path),
            summary: &mut summary,
            wanted_triggers: &mut wanted_triggers,
            queue: &mut queue,
        };
        let node = converter.typed_node(root, TRIGGER_RESOURCE, &trigger_id)?;

        let document = TriggerDocument {
            schema_version: 1,
            id: trigger_id.clone(),
            zone: format!("zone.{zone_slug}"),
            source_type: TRIGGER_RESOURCE.to_owned(),
            resource_id: resource_id(root),
            entrypoint: external_entrypoints.contains(&trigger_id),
            root: node,
            source: xdb.source,
        };
        let file_slug = trigger_id
            .strip_prefix(&format!("trigger.{zone_slug}."))
            .with_context(|| format!("trigger ID {trigger_id} lacks the zone prefix"))?;
        let output = zone_output
            .join("scripts/triggers")
            .join(format!("{file_slug}.yaml"));
        summary.unchanged +=
            usize::from(writer.write(&output, SchemaKind::ScriptTrigger, &document)?);
        summary.trigger_ids.insert(trigger_id);
        summary.triggers += 1;
    }

    Ok(summary)
}

/// The source-relative directory of a document, forward slashes.
fn parent_dir(source_path: &str) -> String {
    source_path
        .rsplit_once('/')
        .map(|(head, _)| head.to_owned())
        .unwrap_or_default()
}

struct Converter<'a> {
    zone_slug: &'a str,
    document_dir: String,
    summary: &'a mut ScriptSummary,
    wanted_triggers: &'a mut BTreeMap<String, String>,
    queue: &'a mut VecDeque<String>,
}

impl Converter<'_> {
    /// The quest's `counters` list: each `QuestCountId` reference bound to the
    /// objective index its position defines (quests.md rule 7.5).
    fn counters(
        &mut self,
        root: Node<'_, '_>,
        script_id: &str,
        quest_id: &str,
    ) -> Result<Vec<CounterBinding>> {
        let Some(list) = child(root, "counters") else {
            return Ok(Vec::new());
        };
        let mut bindings = Vec::new();
        for (index, item) in children(list, "Item").enumerate() {
            let Some(count_href) = href(item, "id") else {
                // count-item and count-kill objectives carry no QuestCountId;
                // nothing scripts them and nothing needs a binding.
                continue;
            };
            let (count_id, row_type) = self.resolve(&count_href)?;
            if row_type.as_deref() != Some("quest-count-id") {
                bail!(
                    "{script_id}: counter {index} id href {count_href} does not point at a QuestCountId"
                );
            }
            let kind = item
                .attribute("type")
                .map(|value| value.rsplit('.').next().unwrap_or(value))
                .map(slug)
                .unwrap_or_else(|| "unknown".to_owned());
            bindings.push(CounterBinding {
                count_id,
                objective: index as u32,
                objective_id: derive_objective_id(
                    quest_id,
                    &kind,
                    Some(&count_href),
                    None,
                    std::iter::empty(),
                ),
                href: Some(count_href),
            });
        }
        Ok(bindings)
    }

    /// A root-level `<name>` element holding an `Item` list of typed nodes.
    fn node_list_field(
        &mut self,
        root: Node<'_, '_>,
        name: &str,
        row_id: &str,
    ) -> Result<Vec<ScriptNode>> {
        let Some(list) = child(root, name) else {
            return Ok(Vec::new());
        };
        let mut nodes = Vec::new();
        for (index, item) in children(list, "Item").enumerate() {
            let key = format!("{row_id}/{name}[{index}]");
            let type_name = item.attribute("type").with_context(|| {
                format!("{key}: an Item under {name} carries no type attribute")
            })?;
            nodes.push(self.typed_node(item, type_name, &key)?);
        }
        Ok(nodes)
    }

    /// One typed node: family and opcode from the source type name, tier from
    /// the checked-in tier table, fields from attributes and child elements,
    /// sorted bytewise by name as ADR 0036 requires.
    fn typed_node(
        &mut self,
        element: Node<'_, '_>,
        type_name: &str,
        key: &str,
    ) -> Result<ScriptNode> {
        let opcode = type_name.rsplit('.').next().unwrap_or(type_name).to_owned();
        let tier = tier_for(&opcode);
        self.count_tier(tier, &opcode);
        let fields = self.fields_of(element, key)?;
        Ok(ScriptNode {
            key: key.to_owned(),
            family: family_of(type_name).to_owned(),
            fields: canonical_opcode_fields(&opcode, key, fields)?,
            opcode,
            tier: tier.to_owned(),
        })
    }

    /// An untyped record element — `<mob>`, a `<path>` entry — kept as a node
    /// so the shape survives, inert because a record is data, not behaviour.
    fn record_node(&mut self, element: Node<'_, '_>, key: &str) -> Result<ScriptNode> {
        self.count_tier("inert-and-counted", "Struct");
        Ok(ScriptNode {
            key: key.to_owned(),
            family: "basic".to_owned(),
            opcode: "Struct".to_owned(),
            tier: "inert-and-counted".to_owned(),
            fields: self.fields_of(element, key)?,
        })
    }

    fn count_tier(&mut self, tier: &str, opcode: &str) {
        self.summary.nodes += 1;
        match tier {
            "implemented" => self.summary.implemented += 1,
            "inert-and-counted" => self.summary.inert += 1,
            _ => {
                self.summary.refused += 1;
                *self
                    .summary
                    .refused_opcodes
                    .entry(opcode.to_owned())
                    .or_insert(0) += 1;
            }
        }
    }

    /// The fields of a node or record: XML attributes as scalars, child
    /// elements by shape, grouped by name, sorted bytewise.
    fn fields_of(&mut self, element: Node<'_, '_>, key: &str) -> Result<Vec<ScriptField>> {
        let mut fields: Vec<ScriptField> = Vec::new();

        for attribute in element.attributes() {
            if attribute.name() == "type" {
                continue;
            }
            if attribute.name() == "href" {
                // A record with an href *is* a reference-bearing element; the
                // caller handles that shape before reaching here, so an href
                // beside other content is stored as a reference field.
                let (id, row_type) = self.resolve(attribute.value())?;
                fields.push(ScriptField {
                    name: "href".to_owned(),
                    value: ScriptValue {
                        reference: Some(Reference {
                            id,
                            row_type,
                            href: Some(attribute.value().to_owned()),
                        }),
                        ..ScriptValue::default()
                    },
                });
                continue;
            }
            fields.push(ScriptField {
                name: attribute.name().to_owned(),
                value: scalar(attribute.value(), attribute.name()),
            });
        }

        // Child elements grouped by tag name, in first-seen order; two children
        // sharing one name merge into one list-valued field.
        let mut grouped: Vec<(String, Vec<Node<'_, '_>>)> = Vec::new();
        for node in element.children().filter(Node::is_element) {
            let name = node.tag_name().name().to_owned();
            if name == "Header" {
                continue;
            }
            match grouped.iter_mut().find(|(existing, _)| *existing == name) {
                Some((_, entries)) => entries.push(node),
                None => grouped.push((name, vec![node])),
            }
        }
        for (name, entries) in grouped {
            let value = if entries.len() == 1 {
                self.field_value(entries[0], &name, key)?
            } else {
                let mut values = Vec::with_capacity(entries.len());
                for (index, entry) in entries.iter().enumerate() {
                    values.push(self.element_value(*entry, &format!("{key}/{name}[{index}]"))?);
                }
                ScriptValue {
                    list: Some(values),
                    ..ScriptValue::default()
                }
            };
            fields.push(ScriptField { name, value });
        }

        fields.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
        for pair in fields.windows(2) {
            if pair[0].name == pair[1].name {
                bail!("{key}: field {} appears twice", pair[0].name);
            }
        }
        Ok(fields)
    }

    /// The value of one single-element field.
    fn field_value(&mut self, element: Node<'_, '_>, name: &str, key: &str) -> Result<ScriptValue> {
        // An `Item` wrapper list, even with one entry: order is meaning.
        let items: Vec<Node<'_, '_>> = children(element, "Item").collect();
        if !items.is_empty() {
            let mut values = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                values.push(self.element_value(*item, &format!("{key}/{name}[{index}]"))?);
            }
            return Ok(ScriptValue {
                list: Some(values),
                ..ScriptValue::default()
            });
        }
        self.element_value(element, &format!("{key}/{name}"))
    }

    /// One element as a value: a typed node, a reference, a record, or a scalar.
    fn element_value(&mut self, element: Node<'_, '_>, key: &str) -> Result<ScriptValue> {
        if let Some(type_name) = element.attribute("type") {
            return Ok(ScriptValue {
                node: Some(Box::new(self.typed_node(element, type_name, key)?)),
                ..ScriptValue::default()
            });
        }
        let has_children = element.children().any(|node| node.is_element());
        let other_attributes = element
            .attributes()
            .any(|attribute| attribute.name() != "href");
        if let Some(reference) = element.attribute("href") {
            if !has_children && !other_attributes {
                if reference.is_empty() {
                    // `<zone href="" />`: an authored null. Text keeps the shape.
                    return Ok(ScriptValue {
                        text: Some(String::new()),
                        ..ScriptValue::default()
                    });
                }
                let (id, row_type) = self.resolve(reference)?;
                return Ok(ScriptValue {
                    reference: Some(Reference {
                        id,
                        row_type,
                        href: Some(reference.to_owned()),
                    }),
                    ..ScriptValue::default()
                });
            }
        }
        if has_children || element.attributes().count() > 0 {
            return Ok(ScriptValue {
                node: Some(Box::new(self.record_node(element, key)?)),
                ..ScriptValue::default()
            });
        }
        let content = element
            .children()
            .filter(Node::is_text)
            .filter_map(|node| node.text())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("");
        let field_name = element.tag_name().name();
        Ok(scalar(&content, field_name))
    }

    /// Resolves one href to a canonical id and the row-type slug of what it
    /// points at. Relative hrefs resolve against the document's own directory:
    /// `DressTrigger` names `CountId_1.xdb` bare, `RatKiller` names its count
    /// id absolutely, and both must land on the same id shape.
    fn resolve(&mut self, value: &str) -> Result<(String, Option<String>)> {
        let (path_part, xpointer) = match value.split_once('#') {
            Some((path, pointer)) => (path, Some(pointer)),
            None => (value, None),
        };
        let absolute = if let Some(trimmed) = path_part.strip_prefix('/') {
            trimmed.to_owned()
        } else if self.document_dir.is_empty() {
            path_part.to_owned()
        } else {
            normalize(&format!("{}/{}", self.document_dir, path_part))
        };

        let root_element = xpointer
            .and_then(|pointer| pointer.strip_prefix("xpointer(/"))
            .map(|rest| rest.trim_end_matches(')'))
            .map(|element| element.rsplit('.').next().unwrap_or(element).to_owned())
            .or_else(|| marker_of(&absolute));

        let mut row_type = root_element.as_deref().map(row_type_slug);

        let id = match row_type.as_deref() {
            Some("trigger") => {
                let id = self.trigger_id(&absolute);
                // Remember where the trigger lives so the corpus walk can read
                // it, and queue it exactly once.
                if !self.wanted_triggers.contains_key(&id) {
                    self.wanted_triggers.insert(id.clone(), absolute.clone());
                    self.queue.push_back(id.clone());
                }
                id
            }
            Some("quest-count-id") => questcount_id(&absolute)
                .with_context(|| format!("no QuestCountId id rule covers {absolute}"))?,
            Some("map-resource") => {
                row_type = Some("map".to_string());
                map_resource_id(&absolute)?
            }
            _ => canonical_id_from_source_path(&absolute).unwrap_or_else(|| {
                let parts: Vec<&str> = absolute.split('/').collect();
                let minted = format!("ext.{}", slug_path(&parts));
                self.summary.external_resources.insert(minted.clone());
                minted
            }),
        };
        Ok((id, row_type))
    }

    /// Trigger ids are zone-scoped: this pack is the unit of self-containment,
    /// and a shared QuestSpells trigger is re-minted per zone that reaches it.
    fn trigger_id(&self, absolute: &str) -> String {
        trigger_id(self.zone_slug, absolute)
    }
}

/// Trigger rows referenced by map machinery rather than by a quest node are
/// still execution roots. This catches SpawnTunerMobImpacts without teaching
/// the runtime pack about the original SpawnTuner format.
fn map_trigger_entrypoints(
    map_dir: &std::path::Path,
    source_root: &std::path::Path,
    zone_slug: &str,
) -> Result<BTreeSet<String>> {
    let mut entrypoints = BTreeSet::new();
    for path in sorted_xdb_files(map_dir)? {
        let xdb = read_xdb(&path, source_root)?;
        let xml = parse_document(&xdb.text, &path)?;
        let document_dir = parent_dir(&xdb.source.path);
        for element in xml.descendants().filter(Node::is_element) {
            for attribute in element
                .attributes()
                .filter(|attribute| attribute.name() == "href")
            {
                let href = attribute.value();
                let (path_part, marker) = match href.split_once('#') {
                    Some((path, pointer)) => (path, Some(pointer)),
                    None => (href, None),
                };
                let is_trigger = marker.is_some_and(|pointer| pointer.contains("TriggerResource"))
                    || marker_of(path_part).as_deref() == Some("TriggerResource");
                if !is_trigger || path_part.is_empty() {
                    continue;
                }
                let absolute = if let Some(path) = path_part.strip_prefix('/') {
                    path.to_owned()
                } else {
                    normalize(&format!("{document_dir}/{path_part}"))
                };
                entrypoints.insert(trigger_id(zone_slug, &absolute));
            }
        }
    }
    Ok(entrypoints)
}

fn trigger_id(zone_slug: &str, absolute: &str) -> String {
    let parts: Vec<&str> = absolute
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let file = parts
        .last()
        .map(|name| slug(strip_xdb_suffix(name)))
        .unwrap_or_default();
    match parts.as_slice() {
        ["World", "Quests", zone, quest, ..] => {
            format!(
                "trigger.{}.{}.{file}",
                canonical_zone_slug(zone),
                slug(quest)
            )
        }
        ["Characters" | "Creatures", family, "Instances", zone, ..] => {
            format!(
                "trigger.{}.{}.{file}",
                canonical_zone_slug(zone),
                slug(family)
            )
        }
        ["Mechanics", "Spells", "QuestSpells", directory, ..] => {
            format!("trigger.{zone_slug}.{}.{file}", slug(directory))
        }
        _ => format!(
            "trigger.{}.{}",
            zone_slug,
            slug_path(&parts).replace('.', "-")
        ),
    }
}

/// Normalizes the five audited M3 script opcodes at the source boundary. The
/// runtime consequently receives one field shape even when the reflection
/// defaults were omitted from the XDB. These checks also stop a malformed
/// source row before it can be blessed as `implemented`.
fn canonical_opcode_fields(
    opcode: &str,
    key: &str,
    mut fields: Vec<ScriptField>,
) -> Result<Vec<ScriptField>> {
    match opcode {
        "DestinationLocator" => {
            let locator = required_field(&fields, "locator", key, opcode)?;
            let Some(locator) = locator.value.node.as_deref() else {
                bail!("{key}: {opcode}.locator is not a MapPointer record");
            };
            if locator.family != "basic" || locator.opcode != "Struct" {
                bail!("{key}: {opcode}.locator is not a MapPointer record");
            }
            let map = required_field(&locator.fields, "map", &locator.key, "MapPointer")?;
            let Some(map) = map.value.reference.as_ref() else {
                bail!("{key}: {opcode}.locator.map is not a content reference");
            };
            if map.row_type.as_deref() != Some("map") || map.id.is_empty() {
                bail!("{key}: {opcode}.locator.map is not a canonical product map reference");
            }
            let script_id =
                required_field(&locator.fields, "scriptID", &locator.key, "MapPointer")?;
            if script_id.value.text.as_deref().is_none_or(str::is_empty) {
                bail!("{key}: {opcode}.locator.scriptID is empty or not text");
            }
            push_default(&mut fields, "yaw", integer_value(0));
            require_integer(&fields, "yaw", key, opcode)?;
        }
        "Guard" => {
            push_default(&mut fields, "noticeTarget", boolean_value(false));
            push_default(
                &mut fields,
                "scanRadius",
                decimal_value(Decimal {
                    mantissa: 425,
                    scale: 1,
                }),
            );
            require_boolean(&fields, "noticeTarget", key, opcode)?;
            let radius = required_field(&fields, "scanRadius", key, opcode)?;
            if let Some(integer) = radius.value.integer {
                let radius = fields
                    .iter_mut()
                    .find(|field| field.name == "scanRadius")
                    .expect("required field exists");
                radius.value = decimal_value(Decimal {
                    mantissa: integer,
                    scale: 0,
                });
            } else if radius.value.decimal.is_none() {
                bail!("{key}: {opcode}.scanRadius is not decimal");
            }
        }
        "PredicateIsAvatar" => {
            // `toLog` is reflection/debug metadata. The predicate is
            // parameter-free and the server must never interpret this bit.
            fields.retain(|field| field.name != "toLog");
            if !fields.is_empty() {
                bail!("{key}: {opcode} is parameter-free");
            }
        }
        "ScalerAllInputDamage" => {
            push_default(&mut fields, "attackerConditions", list_value(Vec::new()));
            push_default(&mut fields, "onlyFromCaster", boolean_value(false));
            push_default(&mut fields, "stackCount", integer_value(1));
            require_list(&fields, "attackerConditions", key, opcode)?;
            require_boolean(&fields, "onlyFromCaster", key, opcode)?;
            require_scaler(&fields, key, opcode)?;
            require_integer(&fields, "stackCount", key, opcode)?;
        }
        "ScalerAllOutputDamage" => {
            push_default(&mut fields, "stackCount", integer_value(1));
            require_scaler(&fields, key, opcode)?;
            require_integer(&fields, "stackCount", key, opcode)?;
            if let Some(group) = fields.iter().find(|field| field.name == "group")
                && group.value.reference.is_none()
                && group.value.text.is_none()
            {
                bail!("{key}: {opcode}.group is not a content reference or group id");
            }
        }
        _ => return Ok(fields),
    }
    fields.sort_by(|left, right| left.name.as_bytes().cmp(right.name.as_bytes()));
    Ok(fields)
}

/// Maps the one classic MapResource path shape to its source-free product id.
/// An unfamiliar path fails closed instead of leaking an `ext.*` identity into
/// authored script rows and, eventually, a runtime pack.
fn map_resource_id(absolute: &str) -> Result<String> {
    let parts: Vec<&str> = absolute
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() != 3
        || !parts[0].eq_ignore_ascii_case("Maps")
        || !parts[2].eq_ignore_ascii_case("MapResource.xdb")
    {
        bail!("map resource reference {absolute:?} is not Maps/<map>/MapResource.xdb");
    }
    let id = slug(parts[1]);
    if id.is_empty() {
        bail!("map resource reference {absolute:?} has no canonical product map id");
    }
    Ok(id)
}

fn required_field<'a>(
    fields: &'a [ScriptField],
    name: &str,
    key: &str,
    opcode: &str,
) -> Result<&'a ScriptField> {
    fields
        .iter()
        .find(|field| field.name == name)
        .with_context(|| format!("{key}: {opcode} requires field {name}"))
}

fn push_default(fields: &mut Vec<ScriptField>, name: &str, value: ScriptValue) {
    if fields.iter().all(|field| field.name != name) {
        fields.push(ScriptField {
            name: name.to_owned(),
            value,
        });
    }
}

fn require_boolean(fields: &[ScriptField], name: &str, key: &str, opcode: &str) -> Result<()> {
    if required_field(fields, name, key, opcode)?
        .value
        .boolean
        .is_none()
    {
        bail!("{key}: {opcode}.{name} is not boolean");
    }
    Ok(())
}

fn require_integer(fields: &[ScriptField], name: &str, key: &str, opcode: &str) -> Result<()> {
    if required_field(fields, name, key, opcode)?
        .value
        .integer
        .is_none()
    {
        bail!("{key}: {opcode}.{name} is not integer");
    }
    Ok(())
}

fn require_list(fields: &[ScriptField], name: &str, key: &str, opcode: &str) -> Result<()> {
    if required_field(fields, name, key, opcode)?
        .value
        .list
        .is_none()
    {
        bail!("{key}: {opcode}.{name} is not a list");
    }
    Ok(())
}

fn require_scaler(fields: &[ScriptField], key: &str, opcode: &str) -> Result<()> {
    let field = required_field(fields, "scaler", key, opcode)?;
    let Some(scaler) = field.value.node.as_deref() else {
        bail!("{key}: {opcode}.scaler is not a scaler node");
    };
    if scaler.family != "scaler" {
        bail!(
            "{key}: {opcode}.scaler has family {}, want scaler",
            scaler.family
        );
    }
    Ok(())
}

fn integer_value(value: i64) -> ScriptValue {
    ScriptValue {
        integer: Some(value),
        ..ScriptValue::default()
    }
}

fn boolean_value(value: bool) -> ScriptValue {
    ScriptValue {
        boolean: Some(value),
        ..ScriptValue::default()
    }
}

fn decimal_value(value: Decimal) -> ScriptValue {
    ScriptValue {
        decimal: Some(value),
        ..ScriptValue::default()
    }
}

fn list_value(value: Vec<ScriptValue>) -> ScriptValue {
    ScriptValue {
        list: Some(value),
        ..ScriptValue::default()
    }
}

/// `questcount.<zone>.<quest>.<count>` from
/// `World/Quests/<Zone>/<Quest>/<CountId>.xdb`.
fn questcount_id(absolute: &str) -> Option<String> {
    let parts: Vec<&str> = absolute
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    match parts.as_slice() {
        ["World", "Quests", zone, quest, file] => Some(format!(
            "questcount.{}.{}.{}",
            canonical_zone_slug(zone),
            slug(quest),
            slug(strip_xdb_suffix(file))
        )),
        _ => None,
    }
}

/// Collapses `a/b/../c` the way the source tree's own relative hrefs use it.
fn normalize(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

/// The `(Marker)` in a file name, for hrefs that carry no xpointer.
fn marker_of(absolute: &str) -> Option<String> {
    let file = absolute.rsplit('/').next()?;
    let start = file.rfind(".(")? + 2;
    let end = file[start..].find(')')? + start;
    Some(file[start..end].to_owned())
}

/// The row-type slug a source root element maps onto. Namespaced types the
/// tables model get the table's slug; everything else gets a stable kebab
/// name so the reference checker can report it without resolving it.
fn row_type_slug(root_element: &str) -> String {
    match root_element {
        "TriggerResource" => "trigger".to_owned(),
        // `MobSpawnTable` is the file-name marker; the root element is
        // `SpawnTable` either way.
        "SpawnTable" | "MobSpawnTable" => "spawn-table".to_owned(),
        "MobWorld" => "mob".to_owned(),
        "QuestCountId" => "quest-count-id".to_owned(),
        "QuestResource" => "quest".to_owned(),
        "ItemResource" => "item".to_owned(),
        other => slug(other),
    }
}

/// Family from the namespaced type name. Behaviour dispatches on opcode; the
/// family is the census axis, so the mapping only has to be deterministic and
/// honest about where a type came from.
fn family_of(type_name: &str) -> &'static str {
    if type_name.contains(".elements.impacts.") {
        "impact"
    } else if type_name.contains(".elements.predicates.") {
        "predicate"
    } else if type_name.contains(".elements.effects.") {
        "effect"
    } else if type_name.contains(".elements.addresseeFinders.") {
        "addresseeFinder"
    } else if type_name.contains(".elements.calcers.") {
        "calcer"
    } else if type_name.contains(".scalers.") {
        "scaler"
    } else if type_name.contains(".basicElements.") {
        "basic"
    } else if type_name.contains(".elements.trigger.")
        || type_name.contains(".schemes.quest.trigger.")
    {
        "trigger"
    } else {
        // Buff attach/detach and the other constructor schemes act as impacts
        // wherever the tutorial reaches them.
        "impact"
    }
}

/// Types a text value by its lexical shape: the reflection schema is not
/// available, and the evaluator's handlers accept integer-or-duration where
/// the distinction matters. `delay` is stored as milliseconds because that is
/// its unit in every `ImpactsDeferred` use.
fn scalar(content: &str, field_name: &str) -> ScriptValue {
    if content == "true" || content == "false" {
        return ScriptValue {
            boolean: Some(content == "true"),
            ..ScriptValue::default()
        };
    }
    if let Ok(number) = content.parse::<i64>() {
        if field_name == "delay" && number >= 0 {
            return ScriptValue {
                duration_ms: Some(number as u64),
                ..ScriptValue::default()
            };
        }
        return ScriptValue {
            integer: Some(number),
            ..ScriptValue::default()
        };
    }
    if let Some(decimal) = parse_decimal(content) {
        return ScriptValue {
            decimal: Some(decimal),
            ..ScriptValue::default()
        };
    }
    ScriptValue {
        text: Some(content.to_owned()),
        ..ScriptValue::default()
    }
}

/// `-372.122009` -> mantissa -372122009, scale 6. Exact or nothing: a value
/// that does not fit i64 stays text rather than becoming an approximation.
fn parse_decimal(content: &str) -> Option<Decimal> {
    let (sign, digits) = match content.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1i64, content),
    };
    let (whole, fraction) = digits.split_once('.')?;
    if whole.is_empty()
        || fraction.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let scale = i32::try_from(fraction.len()).ok()?;
    let mantissa: i64 = format!("{whole}{fraction}").parse().ok()?;
    Some(Decimal {
        mantissa: sign * mantissa,
        scale,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::testing::{fixture_options, write};

    #[test]
    fn scalars_type_by_lexical_shape() {
        assert_eq!(scalar("true", "runningMode").boolean, Some(true));
        assert_eq!(scalar("100", "value").integer, Some(100));
        assert_eq!(scalar("1500", "delay").duration_ms, Some(1500));
        let decimal = scalar("-372.122009", "x").decimal.expect("decimal");
        assert_eq!((decimal.mantissa, decimal.scale), (-372122009, 6));
        assert_eq!(scalar("MAINHAND", "slot").text.as_deref(), Some("MAINHAND"));
    }

    #[test]
    fn questcount_ids_follow_the_quest_tree() {
        assert_eq!(
            questcount_id("World/Quests/InstLeague1/Quest_1_30/CountId_1.xdb").as_deref(),
            Some("questcount.inst-league1.quest-1-30.count-id-1")
        );
    }

    #[test]
    fn relative_paths_normalize() {
        assert_eq!(
            normalize("World/Quests/Zone/Quest/CountId_1.xdb"),
            "World/Quests/Zone/Quest/CountId_1.xdb"
        );
        assert_eq!(normalize("a/b/../c.xdb"), "a/c.xdb");
    }

    #[test]
    fn markers_type_hrefs_without_xpointers() {
        assert_eq!(
            marker_of("World/Quests/Z/Q/DressTrigger.(TriggerResource).xdb").as_deref(),
            Some("TriggerResource")
        );
        assert_eq!(marker_of("World/Quests/Z/Q/TriggerAvatar.xdb"), None);
    }

    #[test]
    fn extracts_recursive_quest_and_shared_trigger_rows() {
        let (source, output, options) = fixture_options();
        write(
            &source.path().join("Maps/TestMap/Zones/TestZone/Zone.xdb"),
            "<ZoneResource/>",
        );
        write(
            &source
                .path()
                .join("World/Quests/TestZone/Quest_1/Quest_1.xdb"),
            r#"<gameMechanics.constructor.schemes.quest.QuestResource>
  <Header><resourceId>101</resourceId></Header>
  <counters>
    <Item type="gameMechanics.elements.quest.QuestCountSpecial">
      <id href="CountId_1.xdb#xpointer(/gameMechanics.elements.quest.QuestCountId)" />
    </Item>
  </counters>
  <startImpacts>
    <Item type="gameMechanics.elements.impacts.ImpactsDeferred">
      <delay>1500</delay>
      <impacts>
        <Item type="gameMechanics.elements.impacts.ImpactIfTarget">
          <predicate type="gameMechanics.elements.predicates.PredicateHasItem">
            <item href="/Items/QuestItems/Test/Key.(ItemResource).xdb#xpointer(/gameMechanics.constructor.schemes.item.ItemResource)" />
          </predicate>
          <impactsIf>
            <Item type="gameMechanics.elements.impacts.FutureStateChangingOpcode">
              <value>2</value>
            </Item>
          </impactsIf>
        </Item>
      </impacts>
    </Item>
  </startImpacts>
  <triggerAgents>
    <Item type="gameMechanics.elements.trigger.TriggerAgentSelf">
      <trigger href="DressTrigger.(TriggerResource).xdb#xpointer(/gameMechanics.constructor.schemes.quest.trigger.TriggerResource)" />
    </Item>
  </triggerAgents>
</gameMechanics.constructor.schemes.quest.QuestResource>"#,
        );
        write(
            &source
                .path()
                .join("World/Quests/TestZone/Quest_1/DressTrigger.(TriggerResource).xdb"),
            r#"<gameMechanics.constructor.schemes.quest.trigger.TriggerResource>
  <Header><resourceId>102</resourceId></Header>
  <effects>
    <Item type="gameMechanics.elements.effects.EquipTrigger">
      <slot>MAINHAND</slot>
      <effects>
        <Item type="gameMechanics.elements.effects.Switch">
          <impactsOff>
            <Item type="gameMechanics.elements.impacts.ImpactStopTalk" />
          </impactsOff>
          <impactsOn>
            <Item type="gameMechanics.elements.impacts.ImpactIncreaseQuestCount">
              <id href="CountId_1.xdb#xpointer(/gameMechanics.elements.quest.QuestCountId)" />
            </Item>
          </impactsOn>
        </Item>
      </effects>
    </Item>
  </effects>
</gameMechanics.constructor.schemes.quest.trigger.TriggerResource>"#,
        );
        write(
            &source
                .path()
                .join("World/Quests/TestZone/Quest_1/TriggerAvatar.xdb"),
            r#"<gameMechanics.constructor.schemes.quest.trigger.TriggerResource>
  <effects>
    <Item type="gameMechanics.elements.effects.Switch">
      <impactsOn />
      <impactsOff />
    </Item>
  </effects>
</gameMechanics.constructor.schemes.quest.trigger.TriggerResource>"#,
        );
        write(
            &source
                .path()
                .join("World/Quests/TestZone/Quest_1/TriggerTarget.xdb"),
            r#"<gameMechanics.constructor.schemes.quest.trigger.TriggerResource>
  <effects>
    <Item type="gameMechanics.elements.effects.EffectTrigger" />
  </effects>
</gameMechanics.constructor.schemes.quest.trigger.TriggerResource>"#,
        );

        let summary = extract_scripts("TestZone", &options).expect("extract scripts");
        assert_eq!(summary.quest_scripts, 1);
        assert_eq!(summary.triggers, 3);
        assert_eq!(summary.counters, 1);
        assert_eq!(
            summary.trigger_ids,
            BTreeSet::from([
                "trigger.test-zone.quest-1.dress-trigger".to_owned(),
                "trigger.test-zone.quest-1.trigger-avatar".to_owned(),
                "trigger.test-zone.quest-1.trigger-target".to_owned(),
            ]),
            "the root-element-discovered trigger set drifted"
        );
        assert_eq!(
            summary.refused_opcodes.get("FutureStateChangingOpcode"),
            Some(&1)
        );

        let quest = fs::read_to_string(
            output
                .path()
                .join("zones/test-zone/scripts/quests/quest-1.yaml"),
        )
        .expect("read quest script");
        assert!(quest.contains("opcode: ImpactsDeferred"), "{quest}");
        assert!(quest.contains("duration_ms: 1500"), "{quest}");
        assert!(quest.contains("opcode: ImpactIfTarget"), "{quest}");
        assert!(quest.contains("opcode: PredicateHasItem"), "{quest}");
        assert!(
            quest.contains("opcode: FutureStateChangingOpcode"),
            "{quest}"
        );
        assert!(quest.contains("tier: refused"), "{quest}");
        assert!(
            quest.contains("objective_id: quest.test-zone.quest-1.objective."),
            "the stable objective identity is absent: {quest}"
        );
        assert!(
            quest.contains("id: trigger.test-zone.quest-1.dress-trigger"),
            "{quest}"
        );

        for name in ["trigger-avatar", "trigger-target"] {
            let path = output.path().join(format!(
                "zones/test-zone/scripts/triggers/quest-1.{name}.yaml"
            ));
            assert!(
                path.is_file(),
                "unsuffixed TriggerResource {name} was not discovered by root element"
            );
        }

        let dress = fs::read_to_string(
            output
                .path()
                .join("zones/test-zone/scripts/triggers/quest-1.dress-trigger.yaml"),
        )
        .expect("read DressTrigger");
        assert!(dress.contains("opcode: EquipTrigger"), "{dress}");
        assert!(dress.contains("opcode: Switch"), "{dress}");
        assert!(
            dress.contains("opcode: ImpactIncreaseQuestCount"),
            "{dress}"
        );
        assert!(
            dress.contains("id: questcount.test-zone.quest-1.count-id-1"),
            "relative QuestCountId did not resolve against DressTrigger: {dress}"
        );
        assert!(
            !dress.contains("gameMechanics.elements.impacts.ImpactIncreaseQuestCount"),
            "a namespaced opcode leaked into the authored node: {dress}"
        );
    }

    #[test]
    fn audited_opcodes_extract_with_canonical_defaults() {
        let (source, output, options) = fixture_options();
        write(
            &source.path().join("Maps/TestMap/Zones/TestZone/Zone.xdb"),
            "<ZoneResource/>",
        );
        write(
            &source.path().join("Maps/TestMap/SpawnTuners/Tuner.xdb"),
            r#"<SpawnTunerMobImpacts><trigger href="/World/Quests/TestZone/Quest_1/Audit.(TriggerResource).xdb#xpointer(/gameMechanics.constructor.schemes.quest.trigger.TriggerResource)" /></SpawnTunerMobImpacts>"#,
        );
        write(
            &source
                .path()
                .join("World/Quests/TestZone/Quest_1/Quest_1.xdb"),
            r#"<gameMechanics.constructor.schemes.quest.QuestResource>
  <startImpacts>
    <Item type="gameMechanics.elements.impacts.ImpactTurnMob">
      <destination type="gameMechanics.map.destination.DestinationLocator">
        <locator><scriptID>Arrival</scriptID><map href="/Maps/TestMap/MapResource.xdb#xpointer(/mapLoader.MapResource)" /></locator>
      </destination>
    </Item>
    <Item type="gameMechanics.elements.impacts.ImpactIfTarget">
      <predicate type="gameMechanics.elements.predicates.PredicateIsAvatar"><toLog>true</toLog></predicate>
    </Item>
  </startImpacts>
  <triggerAgents><Item type="gameMechanics.elements.trigger.TriggerAgentSelf"><trigger href="Audit.(TriggerResource).xdb#xpointer(/gameMechanics.constructor.schemes.quest.trigger.TriggerResource)" /></Item></triggerAgents>
</gameMechanics.constructor.schemes.quest.QuestResource>"#,
        );
        write(
            &source
                .path()
                .join("World/Quests/TestZone/Quest_1/Audit.(TriggerResource).xdb"),
            r#"<gameMechanics.constructor.schemes.quest.trigger.TriggerResource>
  <effects>
    <Item type="gameMechanics.elements.effects.Guard" />
    <Item type="gameMechanics.elements.effects.ScalerAllInputDamage"><scaler type="gameMechanics.elements.scalers.LinearEffectScaler"><coeff>-0.9</coeff></scaler></Item>
    <Item type="gameMechanics.elements.effects.ScalerAllOutputDamage"><scaler type="gameMechanics.elements.scalers.LinearEffectScaler"><coeff>100</coeff></scaler></Item>
  </effects>
</gameMechanics.constructor.schemes.quest.trigger.TriggerResource>"#,
        );

        let summary = extract_scripts("TestZone", &options).expect("extract audited opcodes");
        assert_eq!(summary.refused, 0, "promoted opcodes stayed refused");

        let quest = fs::read_to_string(
            output
                .path()
                .join("zones/test-zone/scripts/quests/quest-1.yaml"),
        )
        .expect("read quest script");
        assert!(quest.contains("opcode: DestinationLocator\n"), "{quest}");
        assert!(quest.contains("- name: yaw\n"), "{quest}");
        assert!(quest.contains("id: test-map\n"), "{quest}");
        assert!(quest.contains("row_type: map\n"), "{quest}");
        assert!(!quest.contains("ext.maps"), "{quest}");
        assert!(!quest.contains("row_type: map-resource"), "{quest}");
        assert!(quest.contains("opcode: PredicateIsAvatar\n"), "{quest}");
        assert!(
            !quest.contains("name: toLog"),
            "PredicateIsAvatar retained source-only toLog: {quest}"
        );

        let trigger = fs::read_to_string(
            output
                .path()
                .join("zones/test-zone/scripts/triggers/quest-1.audit.yaml"),
        )
        .expect("read audit trigger");
        assert!(trigger.contains("entrypoint: true"), "{trigger}");
        for expected in [
            "opcode: Guard",
            "name: noticeTarget",
            "mantissa: 425",
            "opcode: ScalerAllInputDamage",
            "name: attackerConditions",
            "opcode: ScalerAllOutputDamage",
            "name: stackCount",
        ] {
            assert!(trigger.contains(expected), "missing {expected}: {trigger}");
        }
        assert_eq!(trigger.matches("tier: implemented").count(), 6, "{trigger}");
    }
}
