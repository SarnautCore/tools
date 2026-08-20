//! MobKind / MobClass / MobQuality extraction with prototype-chain resolution.
//!
//! A `MobKind` XDB serialises only the fields that differ from its `Header/Prototype`,
//! so the multipliers a mob actually runs with are the *merge* of a chain, not the
//! contents of one file. This module walks that chain, records it in
//! `_source.prototype_chain`, and pulls in the classes, qualities and factions the
//! resolved kinds reference.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use roxmltree::Node;
use serde_json::Value;

use crate::faction::{extract_faction_closure, faction_closure};
use crate::model::{
    ExtractionOptions, LocRefs, MobClassDocument, MobKindDocument, MobKindSummary,
    MobQualityDocument, Provenance, ResourceRef,
};
use crate::output::OutputWriter;
use crate::reference::{canonical_id_from_source_path, resource_ref};
use crate::scan::{loc_key, resolve_href, sort_paths, zone_instance_files};
use crate::validation::SchemaKind;
use crate::xdb::{
    child, children, extra_fields, f64_value, href, parse_document, prototype_href, read_xdb_from,
    resource_id, text,
};

pub(crate) const CLASSIC_SERVER_ROOT: &str = "classic-server-1-1-02-0";

const MOB_WORLD: &str = "gameMechanics.world.mob.MobWorld";
const MOB_KIND: &str = "gameMechanics.world.mob.MobKind";
const MOB_CLASS: &str = "gameMechanics.world.mob.MobClass";
const MOB_QUALITY: &str = "gameMechanics.world.mob.MobQuality";

/// The multipliers on a `MobKind` scale a per-level base HP and DPS curve that does
/// not exist anywhere under the source data root. Every emitted kind carries this
/// note so the gap is visible at the point of use, not just in the spec.
pub(crate) const LEVEL_CURVE_GAP: &str = "the per-level base HP/DPS curve these modifiers multiply is absent from the source data tree; it is a curated SarnautCore overlay constant (docs/specs/mechanics/combat.md 7.1)";

/// Fields lifted onto the document; everything else on a `MobKind` lands in `extra`.
const KIND_MAPPED: &[&str] = &[
    "Header",
    "name",
    "mobClass",
    "quality",
    "hpMod",
    "dpsMod",
    "expMod",
    "manaMod",
    "lootMod",
    "speed",
    "lootTable",
];

pub fn extract_mobkinds(zone: &str, options: &ExtractionOptions) -> Result<MobKindSummary> {
    let writer = OutputWriter::new(options.dry_run, options.schema_dir.as_deref())?;
    let scope = scan_zone(&options.src, zone)?;
    let mut summary = MobKindSummary {
        zone: zone.to_owned(),
        mob_worlds: scope.mob_worlds,
        ..MobKindSummary::default()
    };

    // Resolving a kind memoises every prototype it walks through, so draining the
    // resolver afterwards yields the referenced kinds and their ancestors together.
    let mut resolver = KindResolver::new(&options.src);
    for relative in &scope.kinds {
        resolver.resolve(relative)?;
    }
    let documents: BTreeMap<String, MobKindDocument> = resolver
        .resolved
        .values()
        .map(|resolved| (resolved.id.clone(), resolved.document.clone()))
        .collect();

    let mut classes: BTreeSet<String> = scope.classes.clone();
    let mut qualities: BTreeSet<String> = scope.qualities.clone();
    for (relative, resolved) in &resolver.resolved {
        let referrer = options.src.join(relative);
        for (reference, sink) in [
            (&resolved.document.mob_class, &mut classes),
            (&resolved.document.quality, &mut qualities),
        ] {
            if let Some(reference) = reference
                && let Some((_, target)) = resolve_href(&options.src, &referrer, &reference.href)
            {
                sink.insert(target);
            }
        }
    }

    for (id, document) in &documents {
        let file = id.strip_prefix("mobkind.").context("mob kind id prefix")?;
        let output = options.out.join("mobkinds").join(format!("{file}.yaml"));
        summary.unchanged += usize::from(writer.write(&output, SchemaKind::MobKind, document)?);
    }
    summary.mob_kinds = scope.kinds.len();
    summary.prototypes = documents.len() - summary.mob_kinds;

    for relative in &classes {
        let document = mob_class_document(&options.src, relative)?;
        let file = document
            .id
            .strip_prefix("mobclass.")
            .context("mob class id prefix")?;
        let output = options.out.join("mobclasses").join(format!("{file}.yaml"));
        summary.unchanged += usize::from(writer.write(&output, SchemaKind::MobKind, &document)?);
        summary.mob_classes += 1;
    }

    for relative in &qualities {
        let document = mob_quality_document(&options.src, relative)?;
        let file = document
            .id
            .strip_prefix("mobquality.")
            .context("mob quality id prefix")?;
        let output = options
            .out
            .join("mobqualities")
            .join(format!("{file}.yaml"));
        summary.unchanged += usize::from(writer.write(&output, SchemaKind::MobKind, &document)?);
        summary.mob_qualities += 1;
    }

    let factions = extract_faction_closure(&options.src, &scope.factions, &options.out, &writer)?;
    summary.factions = factions.emitted;
    summary.unchanged += factions.unchanged;
    Ok(summary)
}

/// The MobKind, MobClass, MobQuality and Faction hrefs one zone's mobs reach.
pub(crate) struct ZoneScope {
    pub(crate) mob_worlds: usize,
    pub(crate) kinds: BTreeSet<String>,
    pub(crate) classes: BTreeSet<String>,
    pub(crate) qualities: BTreeSet<String>,
    pub(crate) factions: BTreeSet<String>,
}

pub(crate) fn scan_zone(src: &Path, zone: &str) -> Result<ZoneScope> {
    let files = zone_instance_files(src, zone)?;
    if files.is_empty() {
        bail!(
            "no mob instances found for zone {zone} below {}",
            src.display()
        );
    }
    let mut scope = ZoneScope {
        mob_worlds: 0,
        kinds: BTreeSet::new(),
        classes: BTreeSet::new(),
        qualities: BTreeSet::new(),
        factions: BTreeSet::new(),
    };
    for path in files {
        let xdb = read_xdb_from(&path, src, Some(CLASSIC_SERVER_ROOT))?;
        let xml = parse_document(&xdb.text, &path)?;
        let root = xml.root_element();
        if root.tag_name().name() != MOB_WORLD {
            continue;
        }
        scope.mob_worlds += 1;
        for (field, sink) in [
            ("kind", &mut scope.kinds),
            ("faction", &mut scope.factions),
            ("quality", &mut scope.qualities),
        ] {
            if let Some(value) = href(root, field)
                && let Some((_, relative)) = resolve_href(src, &path, &value)
            {
                sink.insert(relative);
            }
        }
    }
    Ok(scope)
}

/// Resolves prototype chains, memoising each file so a shared template is read once.
struct KindResolver<'a> {
    src: &'a Path,
    resolved: BTreeMap<String, ResolvedKind>,
    in_progress: Vec<String>,
}

#[derive(Clone)]
struct ResolvedKind {
    id: String,
    document: MobKindDocument,
    chain: Vec<String>,
    loot_tables: Vec<ResourceRef>,
}

impl<'a> KindResolver<'a> {
    fn new(src: &'a Path) -> Self {
        Self {
            src,
            resolved: BTreeMap::new(),
            in_progress: Vec::new(),
        }
    }

    fn resolve(&mut self, relative: &str) -> Result<ResolvedKind> {
        if let Some(existing) = self.resolved.get(relative) {
            return Ok(existing.clone());
        }
        if self.in_progress.iter().any(|entry| entry == relative) {
            bail!(
                "prototype cycle: {} -> {relative}",
                self.in_progress.join(" -> ")
            );
        }
        self.in_progress.push(relative.to_owned());
        let result = self.resolve_uncached(relative);
        self.in_progress.pop();
        let resolved = result?;
        self.resolved.insert(relative.to_owned(), resolved.clone());
        Ok(resolved)
    }

    fn resolve_uncached(&mut self, relative: &str) -> Result<ResolvedKind> {
        let path = self.src.join(relative);
        if !path.is_file() {
            bail!(
                "mob kind {relative} does not exist below {}",
                self.src.display()
            );
        }
        let xdb = read_xdb_from(&path, self.src, Some(CLASSIC_SERVER_ROOT))?;
        let xml = parse_document(&xdb.text, &path)?;
        let root = xml.root_element();
        if root.tag_name().name() != MOB_KIND {
            bail!(
                "{relative} is a {}, not a {MOB_KIND}",
                root.tag_name().name()
            );
        }
        let id = canonical_id_from_source_path(relative)
            .with_context(|| format!("build mob kind ID from {relative}"))?;

        let prototype = prototype_href(root);
        let parent = match &prototype {
            Some(value) => {
                let (_, parent_relative) =
                    resolve_href(self.src, &path, value).with_context(|| {
                        format!("resolve prototype {value} referenced by {relative}")
                    })?;
                Some(
                    self.resolve(&parent_relative)
                        .with_context(|| format!("resolve prototype of {relative}"))?,
                )
            }
            None => None,
        };

        let own = own_kind_fields(root, self.src, &path);
        let mut chain = parent
            .as_ref()
            .map_or_else(Vec::new, |value| value.chain.clone());
        chain.push(id.clone());

        let loot_tables = if own.loot_tables.is_empty() {
            parent
                .as_ref()
                .map_or_else(Vec::new, |value| value.loot_tables.clone())
        } else {
            own.loot_tables.clone()
        };

        let inherited = parent.as_ref().map(|value| &value.document);
        let mut extra: BTreeMap<String, Value> = inherited
            .map(|value| value.extra.clone())
            .unwrap_or_default();
        extra.extend(own.extra);
        extra.insert(
            "level_curve_gap".into(),
            Value::String(LEVEL_CURVE_GAP.into()),
        );
        if !loot_tables.is_empty() {
            extra.insert("loot_tables".into(), serde_json::to_value(&loot_tables)?);
        }

        let document = MobKindDocument {
            schema_version: 1,
            id: id.clone(),
            kind: "mobkind".into(),
            source_type: root.tag_name().name().to_owned(),
            resource_id: resource_id(root),
            loc_ref: LocRefs {
                name: own
                    .name
                    .or_else(|| inherited.and_then(|v| v.loc_ref.name.clone())),
                ..LocRefs::default()
            },
            prototype: prototype.as_deref().map(resource_ref),
            mob_class: own
                .mob_class
                .or_else(|| inherited.and_then(|v| v.mob_class.clone())),
            quality: own
                .quality
                .or_else(|| inherited.and_then(|v| v.quality.clone())),
            hp_mod: own.hp_mod.or_else(|| inherited.and_then(|v| v.hp_mod)),
            dps_mod: own.dps_mod.or_else(|| inherited.and_then(|v| v.dps_mod)),
            exp_mod: own.exp_mod.or_else(|| inherited.and_then(|v| v.exp_mod)),
            mana_mod: own.mana_mod.or_else(|| inherited.and_then(|v| v.mana_mod)),
            loot_mod: own.loot_mod.or_else(|| inherited.and_then(|v| v.loot_mod)),
            speed: own.speed.or_else(|| inherited.and_then(|v| v.speed)),
            extra,
            source: Provenance {
                prototype_chain: Some(chain.clone()),
                ..xdb.source
            },
        };
        Ok(ResolvedKind {
            id,
            document,
            chain,
            loot_tables,
        })
    }
}

struct OwnKindFields {
    name: Option<String>,
    mob_class: Option<ResourceRef>,
    quality: Option<ResourceRef>,
    hp_mod: Option<f64>,
    dps_mod: Option<f64>,
    exp_mod: Option<f64>,
    mana_mod: Option<f64>,
    loot_mod: Option<f64>,
    speed: Option<f64>,
    loot_tables: Vec<ResourceRef>,
    extra: BTreeMap<String, Value>,
}

fn own_kind_fields(root: Node<'_, '_>, src: &Path, referrer: &Path) -> OwnKindFields {
    let loot_tables = child(root, "lootTable")
        .map(|node| {
            children(node, "Item")
                .filter_map(|item| item.attribute("href"))
                .filter(|value| !value.is_empty())
                .map(resource_ref)
                .collect()
        })
        .unwrap_or_default();
    OwnKindFields {
        name: href(root, "name").and_then(|value| loc_key(src, referrer, &value)),
        mob_class: href(root, "mobClass").map(|value| resource_ref(&value)),
        quality: href(root, "quality").map(|value| resource_ref(&value)),
        hp_mod: f64_value(root, "hpMod"),
        dps_mod: f64_value(root, "dpsMod"),
        exp_mod: f64_value(root, "expMod"),
        mana_mod: f64_value(root, "manaMod"),
        loot_mod: f64_value(root, "lootMod"),
        speed: f64_value(root, "speed"),
        loot_tables,
        extra: extra_fields(root, KIND_MAPPED),
    }
}

/// Every source file behind one zone's extracted creature documents: the mob
/// instances, the mob kinds they resolve through, the classes and qualities those
/// name, and the faction closure. This is the set whose `loc_ref`s the locale
/// extractor has to satisfy.
pub(crate) fn zone_source_files(src: &Path, zone: &str) -> Result<Vec<PathBuf>> {
    let scope = scan_zone(src, zone)?;
    let mut relatives: BTreeSet<String> = scope.classes.union(&scope.qualities).cloned().collect();
    let mut resolver = KindResolver::new(src);
    for relative in &scope.kinds {
        resolver.resolve(relative)?;
    }
    for (relative, resolved) in &resolver.resolved {
        relatives.insert(relative.clone());
        for reference in [&resolved.document.mob_class, &resolved.document.quality]
            .into_iter()
            .flatten()
        {
            if let Some((_, target)) = resolve_href(src, &src.join(relative), &reference.href) {
                relatives.insert(target);
            }
        }
    }
    relatives.extend(faction_closure(src, &scope.factions)?.into_keys());

    let mut files = zone_instance_files(src, zone)?;
    files.extend(relatives.iter().map(|relative| src.join(relative)));
    sort_paths(&mut files);
    files.dedup();
    Ok(files)
}

/// The loot tables one zone's mob kinds reach, as source-relative paths.
pub(crate) fn zone_loot_table_hrefs(src: &Path, zone: &str) -> Result<BTreeSet<String>> {
    let scope = scan_zone(src, zone)?;
    let mut resolver = KindResolver::new(src);
    let mut tables = BTreeSet::new();
    for relative in &scope.kinds {
        let resolved = resolver.resolve(relative)?;
        let referrer = src.join(relative);
        for reference in &resolved.loot_tables {
            if let Some((_, table)) = resolve_href(src, &referrer, &reference.href) {
                tables.insert(table);
            }
        }
    }
    Ok(tables)
}

fn read_taxonomy<'a>(
    src: &Path,
    relative: &str,
    expected: &str,
    buffer: &'a mut String,
) -> Result<(roxmltree::Document<'a>, Provenance)> {
    let path = src.join(relative);
    let xdb = read_xdb_from(&path, src, Some(CLASSIC_SERVER_ROOT))?;
    *buffer = xdb.text;
    let xml = parse_document(buffer, &path)?;
    if xml.root_element().tag_name().name() != expected {
        bail!(
            "{relative} is a {}, not a {expected}",
            xml.root_element().tag_name().name()
        );
    }
    Ok((xml, xdb.source))
}

fn mob_class_document(src: &Path, relative: &str) -> Result<MobClassDocument> {
    let mut buffer = String::new();
    let (xml, source) = read_taxonomy(src, relative, MOB_CLASS, &mut buffer)?;
    let root = xml.root_element();
    let stat_mods: Vec<Value> = child(root, "statMods")
        .map(|node| {
            children(node, "Item")
                .filter_map(|item| {
                    Some(serde_json::json!({
                        "stat": text(item, "stat")?,
                        "mod": f64_value(item, "mod")?,
                    }))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut extra = extra_fields(root, &["Header", "name", "statMods"]);
    if !stat_mods.is_empty() {
        extra.insert("stat_mods".into(), Value::Array(stat_mods));
    }
    Ok(MobClassDocument {
        schema_version: 1,
        id: canonical_id_from_source_path(relative)
            .with_context(|| format!("build mob class ID from {relative}"))?,
        kind: "mobclass".into(),
        source_type: root.tag_name().name().to_owned(),
        resource_id: resource_id(root),
        loc_ref: LocRefs {
            name: href(root, "name").and_then(|value| loc_key(src, &src.join(relative), &value)),
            ..LocRefs::default()
        },
        extra,
        source,
    })
}

/// Rank order for the `MobQuality` enum, ascending by how much the quality is meant
/// to scale a mob. The source carries no ordering of its own — `QualityPrototype.xdb`
/// lists the qualities in authoring order and the resource ids are unordered — so the
/// ladder below is a curated SarnautCore decision. An unrecognised quality is an
/// error rather than a guessed rank.
fn quality_rank(quality: &str) -> Option<u8> {
    match quality {
        "CRITTER" => Some(0),
        "COMMON" => Some(1),
        "FLAVOR_ELITE" => Some(2),
        "MINI_BOSS" => Some(3),
        "ELITE" => Some(4),
        "RAID_ELITE" => Some(5),
        "BOSS" => Some(6),
        "RAID_BOSS" => Some(7),
        _ => None,
    }
}

fn mob_quality_document(src: &Path, relative: &str) -> Result<MobQualityDocument> {
    let mut buffer = String::new();
    let (xml, source) = read_taxonomy(src, relative, MOB_QUALITY, &mut buffer)?;
    let root = xml.root_element();
    let quality =
        text(root, "quality").with_context(|| format!("{relative} carries no <quality>"))?;
    let rank = quality_rank(&quality)
        .with_context(|| format!("{relative} has unranked quality {quality}"))?;

    // `Header/Prototype` on a quality points at `QualityPrototype.xdb`, which carries
    // only an `all` registry list and no inheritable field, so it is recorded rather
    // than merged.
    let mut extra = extra_fields(root, &["Header", "name"]);
    if let Some(prototype) = prototype_href(root) {
        extra.insert(
            "prototype".into(),
            serde_json::to_value(resource_ref(&prototype))?,
        );
    }
    Ok(MobQualityDocument {
        schema_version: 1,
        id: canonical_id_from_source_path(relative)
            .with_context(|| format!("build mob quality ID from {relative}"))?,
        kind: "mobquality".into(),
        rank,
        source_type: root.tag_name().name().to_owned(),
        resource_id: resource_id(root),
        loc_ref: LocRefs {
            name: href(root, "name").and_then(|value| loc_key(src, &src.join(relative), &value)),
            ..LocRefs::default()
        },
        extra,
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::testing::{fixture_options, write};

    fn template(path: &Path) {
        write(
            &path.join("Mechanics/MobKindTemplates/AE1Player.xdb"),
            r#"<gameMechanics.world.mob.MobKind>
<Header><resourceId>92750855</resourceId><isPrototype>true</isPrototype></Header>
<hpMod>0.27</hpMod><dpsMod>0.175</dpsMod><speed>1</speed><expMod>0.25</expMod><manaMod>1</manaMod><lootMod>0.33</lootMod>
</gameMechanics.world.mob.MobKind>"#,
        );
    }

    fn zone_mob(path: &Path, kind: &str) {
        write(
            &path.join("Creatures/Zombie/Instances/TestZone/Zombie1.(MobWorld).xdb"),
            &format!(
                r#"<gameMechanics.world.mob.MobWorld><Header><resourceId>1</resourceId></Header>
<name href="Zombie1_Name.txt"/>
<kind href="{kind}#xpointer(/gameMechanics.world.mob.MobKind)"/>
<faction href="/World/Factions/Wild.xdb#xpointer(/gameMechanics.world.creature.Faction)"/>
<quality href="/Mechanics/MobQualities/Common.xdb#xpointer(/gameMechanics.world.mob.MobQuality)"/>
</gameMechanics.world.mob.MobWorld>"#
            ),
        );
        write(
            &path.join("World/Factions/Wild.xdb"),
            r#"<gameMechanics.world.creature.Faction><Header><resourceId>7</resourceId></Header><name href="WILD_NAME.txt"/></gameMechanics.world.creature.Faction>"#,
        );
        write(
            &path.join("Mechanics/MobQualities/Common.xdb"),
            r#"<gameMechanics.world.mob.MobQuality><Header><resourceId>8</resourceId><Prototype href="QualityPrototype.xdb#x"/></Header><quality>COMMON</quality></gameMechanics.world.mob.MobQuality>"#,
        );
        write(
            &path.join("Mechanics/MobClasses/UDZombieFighter.xdb"),
            r#"<gameMechanics.world.mob.MobClass><Header><resourceId>9</resourceId></Header><expMod>0.925</expMod><statMods><Item><stat>IS_Strength</stat><mod>1.3</mod></Item></statMods><armorMod>0.75</armorMod></gameMechanics.world.mob.MobClass>"#,
        );
    }

    #[test]
    fn a_child_overrides_one_inherited_modifier_and_keeps_the_rest() {
        let (source, output, options) = fixture_options();
        template(source.path());
        write(
            &source
                .path()
                .join("Mechanics/Creatures/Zombie/ZombieKind.(MobKind).xdb"),
            r#"<gameMechanics.world.mob.MobKind>
<Header><resourceId>53144</resourceId><Prototype href="/Mechanics/MobKindTemplates/AE1Player.xdb#xpointer(/gameMechanics.world.mob.MobKind)"/></Header>
<expMod>1</expMod><race>UNDEAD</race>
<mobClass href="/Mechanics/MobClasses/UDZombieFighter.xdb#xpointer(/gameMechanics.world.mob.MobClass)"/>
<lootTable><Item href="/World/LootTables/Zombie/Zombie(01).xdb#x"/></lootTable>
</gameMechanics.world.mob.MobKind>"#,
        );
        zone_mob(
            source.path(),
            "/Mechanics/Creatures/Zombie/ZombieKind.(MobKind).xdb",
        );

        let summary = extract_mobkinds("TestZone", &options).unwrap();
        assert_eq!(summary.mob_worlds, 1);
        assert_eq!(summary.mob_kinds, 1);
        assert_eq!(summary.prototypes, 1);
        assert_eq!(summary.mob_classes, 1);
        assert_eq!(summary.mob_qualities, 1);
        assert_eq!(summary.factions, 1);

        let yaml = fs::read_to_string(
            output
                .path()
                .join("mobkinds/creatures.zombie.zombie-kind.yaml"),
        )
        .unwrap();
        assert!(yaml.contains("hp_mod: 0.27"), "{yaml}");
        assert!(yaml.contains("dps_mod: 0.175"), "{yaml}");
        assert!(yaml.contains("loot_mod: 0.33"), "{yaml}");
        assert!(yaml.contains("mana_mod: 1.0"), "{yaml}");
        // The child sets expMod itself, so the prototype's 0.25 does not survive.
        assert!(yaml.contains("exp_mod: 1.0"), "{yaml}");
        assert!(
            yaml.contains("mobkind.mob-kind-templates.ae1player"),
            "{yaml}"
        );
        assert!(yaml.contains("level_curve_gap"), "{yaml}");
        assert!(yaml.contains("race: UNDEAD"), "{yaml}");

        let quality = fs::read_to_string(output.path().join("mobqualities/common.yaml")).unwrap();
        assert!(quality.contains("rank: 1"), "{quality}");
        let class =
            fs::read_to_string(output.path().join("mobclasses/ud-zombie-fighter.yaml")).unwrap();
        assert!(class.contains("stat: IS_Strength"), "{class}");

        let second = extract_mobkinds("TestZone", &options).unwrap();
        assert_eq!(second.unchanged, 5);
    }

    #[test]
    fn a_two_level_chain_merges_grandparent_fields() {
        let (source, output, options) = fixture_options();
        template(source.path());
        write(
            &source
                .path()
                .join("Mechanics/Creatures/Zombie/Middle.(MobKind).xdb"),
            r#"<gameMechanics.world.mob.MobKind>
<Header><resourceId>2</resourceId><Prototype href="/Mechanics/MobKindTemplates/AE1Player.xdb#x"/></Header>
<dpsMod>2</dpsMod>
</gameMechanics.world.mob.MobKind>"#,
        );
        write(
            &source
                .path()
                .join("Mechanics/Creatures/Zombie/Leaf.(MobKind).xdb"),
            r#"<gameMechanics.world.mob.MobKind>
<Header><resourceId>3</resourceId><Prototype href="Middle.(MobKind).xdb#x"/></Header>
<hpMod>9</hpMod>
</gameMechanics.world.mob.MobKind>"#,
        );
        zone_mob(
            source.path(),
            "/Mechanics/Creatures/Zombie/Leaf.(MobKind).xdb",
        );

        extract_mobkinds("TestZone", &options).unwrap();
        let yaml =
            fs::read_to_string(output.path().join("mobkinds/creatures.zombie.leaf.yaml")).unwrap();
        assert!(yaml.contains("hp_mod: 9.0"), "{yaml}");
        assert!(yaml.contains("dps_mod: 2.0"), "{yaml}");
        assert!(yaml.contains("loot_mod: 0.33"), "{yaml}");
        assert!(
            yaml.contains(
                "  prototype_chain:\n  - mobkind.mob-kind-templates.ae1player\n  - mobkind.creatures.zombie.middle\n  - mobkind.creatures.zombie.leaf"
            ),
            "{yaml}"
        );
        assert!(
            output
                .path()
                .join("mobkinds/creatures.zombie.middle.yaml")
                .is_file()
        );
    }

    #[test]
    fn a_missing_prototype_is_an_error() {
        let (source, _output, options) = fixture_options();
        write(
            &source
                .path()
                .join("Mechanics/Creatures/Zombie/Orphan.(MobKind).xdb"),
            r#"<gameMechanics.world.mob.MobKind>
<Header><resourceId>4</resourceId><Prototype href="/Mechanics/MobKindTemplates/Gone.xdb#x"/></Header>
<hpMod>1</hpMod>
</gameMechanics.world.mob.MobKind>"#,
        );
        zone_mob(
            source.path(),
            "/Mechanics/Creatures/Zombie/Orphan.(MobKind).xdb",
        );

        let error = extract_mobkinds("TestZone", &options).unwrap_err();
        let rendered = format!("{error:#}");
        assert!(
            rendered.contains("Mechanics/MobKindTemplates/Gone.xdb"),
            "{rendered}"
        );
        assert!(rendered.contains("does not exist"), "{rendered}");
    }
}
