//! Cross-reference integrity over the whole authored chain:
//! quest → mob → spawn table → placement → item → loot table → locale.
//!
//! ADR 0006 makes the compiler the place a broken reference is caught, because
//! a pack is built once and read at every shard boot. Every failure here names
//! **both** ends — the document that made the reference and the id it could not
//! resolve — since "dangling reference" without the referrer is a search, not a
//! diagnosis.
//!
//! Three outcomes are deliberately not failures:
//!
//! * **External.** A pack covers exactly one zone (ADR 0029). A zone-scoped id
//!   naming a different zone is out of this pack, not missing from it: the M2
//!   zone has a quest whose finisher stands in the next zone along.
//! * **Unmodelled.** `item.interactive-objects.*` ids are `ChestResource` and
//!   `SteleResource` records — doors, chests, steles. No row type describes
//!   them (`mechanics/quests.md` §7.1 defers the impact system that drives
//!   them), so a reference into that namespace has nothing to resolve against.
//!   Fifty-two of the M2 zone's spawn tables place one.
//! * **Locale gap.** A localization key no locale document supplies means a
//!   loc pack has not been extracted yet, which is a coverage fact rather than
//!   a broken edge. `--require-locale` promotes gaps to failures for a build
//!   that must be fully translated.

use std::collections::{BTreeMap, BTreeSet};

use crate::source::{LocRef, LocaleDocument, ObjectRef, Provenance, SourceTree};

/// Ids under this prefix name interactive world objects, which this compiler
/// emits no rows for.
const INTERACTIVE_PREFIX: &str = "item.interactive-objects.";

/// Id namespaces that are scoped to one zone, and therefore whose second
/// segment is a zone slug.
const ZONE_SCOPED_PREFIXES: [&str; 4] = ["mob", "quest", "spawn", "route"];

/// One reference the pack cannot resolve.
#[derive(Clone, Debug)]
pub struct DanglingRef {
    /// What kind of edge broke, for example `quest -> starter mob`.
    pub class: &'static str,
    /// The document that made the reference.
    pub referencer: String,
    /// The id it names.
    pub target: String,
}

impl DanglingRef {
    pub fn describe(&self) -> String {
        format!(
            "{}: {} references {}, which this pack does not carry",
            self.class, self.referencer, self.target
        )
    }
}

/// A reference into another zone's content.
#[derive(Clone, Debug)]
pub struct ExternalRef {
    pub class: &'static str,
    pub referencer: String,
    pub target: String,
    pub zone: String,
}

/// A reference into a namespace this compiler emits no rows for.
#[derive(Clone, Debug)]
pub struct UnmodelledRef {
    pub class: &'static str,
    pub referencer: String,
    pub target: String,
}

/// A localization key no locale document in this pack supplies.
#[derive(Clone, Debug)]
pub struct LocaleGap {
    pub referencer: String,
    pub field: &'static str,
    pub key: String,
}

/// What the reference pass found.
#[derive(Debug, Default)]
pub struct ReferenceReport {
    pub resolved: usize,
    pub dangling: Vec<DanglingRef>,
    pub external: Vec<ExternalRef>,
    pub unmodelled: Vec<UnmodelledRef>,
    pub locale_gaps: Vec<LocaleGap>,
}

impl ReferenceReport {
    pub fn is_clean(&self) -> bool {
        self.dangling.is_empty()
    }

    /// One message per broken edge, capped so a tree-wide breakage produces a
    /// diagnosis rather than a log dump.
    pub fn failure_message(&self) -> String {
        const SHOWN: usize = 20;
        let mut lines: Vec<String> = self
            .dangling
            .iter()
            .take(SHOWN)
            .map(DanglingRef::describe)
            .collect();
        if self.dangling.len() > SHOWN {
            lines.push(format!("... and {} more", self.dangling.len() - SHOWN));
        }
        format!(
            "{} unresolved reference(s):\n  {}",
            self.dangling.len(),
            lines.join("\n  ")
        )
    }
}

/// Every localization key the pack supplies, and the rule for turning an
/// authored `loc_ref` value into one of them.
#[derive(Debug, Default)]
pub struct LocaleIndex {
    keys: BTreeSet<String>,
}

impl LocaleIndex {
    pub fn build(locales: &[LocaleDocument]) -> Self {
        let mut keys = BTreeSet::new();
        for locale in locales {
            for entry in &locale.entries {
                keys.insert(entry.key.clone());
            }
        }
        Self { keys }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Resolves an authored `loc_ref` value to the key a locale document
    /// carries.
    ///
    /// Two conventions exist in the trees this compiler reads and neither can
    /// be dropped: the demo dataset writes whole keys (`TideCrab.Name.txt`),
    /// while the extractor writes a file name relative to the document's own
    /// source directory (`ZombieWarrior1_1_Name.txt` beside
    /// `Creatures/ZombieWarrior/Instances/InstLeague1/…`). Rather than guess
    /// from the shape of the string, try the candidates in a fixed order and
    /// take the first the locale supplies.
    ///
    /// When none of them resolves, `found` is false and the caller writes
    /// **nothing** into the row.
    ///
    /// That is an ADR 0011 rule, not a stylistic one. An authored key the pack
    /// cannot resolve is a verbatim MY.GAMES resource path, and some of those
    /// paths carry the source resource's class name in parentheses, so writing
    /// one into a table would put a MY.GAMES type name in a compiled artifact.
    /// A key that resolves has been replaced by a locale document's own key and
    /// is safe; an unresolved one is a source string, and source strings do not
    /// go in packs. The shard falls back to the canonical id, which this
    /// project owns.
    pub fn resolve(&self, authored: &str, source: &Provenance) -> Resolved {
        for candidate in candidates(authored, &source.path) {
            if self.keys.contains(&candidate) {
                return Resolved {
                    key: candidate,
                    found: true,
                };
            }
        }
        Resolved {
            key: authored.trim().to_string(),
            found: false,
        }
    }
}

/// A resolved localization key and whether the pack actually supplies it.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub key: String,
    pub found: bool,
}

fn candidates(authored: &str, source_path: &str) -> Vec<String> {
    let trimmed = authored.trim();
    let mut out = vec![trimmed.to_string()];

    let stem = trimmed.strip_suffix(".txt").unwrap_or(trimmed);
    let absolute = stem.trim_start_matches('/').to_string();
    if !out.contains(&absolute) {
        out.push(absolute);
    }

    if !source_path.is_empty() && !stem.contains('/') {
        let directory = source_path
            .trim_start_matches('/')
            .rsplit_once('/')
            .map(|(head, _)| head)
            .unwrap_or("");
        let qualified = if directory.is_empty() {
            stem.to_string()
        } else {
            format!("{directory}/{stem}")
        };
        if !out.contains(&qualified) {
            out.push(qualified);
        }
    }
    out
}

/// Every id the pack carries, grouped by the document type that owns it.
#[derive(Debug, Default)]
pub struct Universe {
    pub zone_slug: String,
    pub zones: BTreeSet<String>,
    pub mobs: BTreeSet<String>,
    pub spawn_tables: BTreeSet<String>,
    pub routes: BTreeSet<String>,
    pub quests: BTreeSet<String>,
    pub factions: BTreeSet<String>,
    pub mob_kinds: BTreeSet<String>,
    pub abilities: BTreeSet<String>,
    pub items: BTreeSet<String>,
    pub loot_tables: BTreeSet<String>,
    pub level_curves: BTreeSet<String>,
}

impl Universe {
    pub fn of(tree: &SourceTree, zone_slug: &str) -> Self {
        Self {
            zone_slug: zone_slug.to_string(),
            zones: tree
                .zones
                .iter()
                .map(|document| document.id.clone())
                .chain(std::iter::once(format!("zone.{zone_slug}")))
                .collect(),
            mobs: ids(tree.mobs.iter().map(|document| &document.id)),
            spawn_tables: ids(tree.tables.iter().map(|document| &document.id)),
            routes: ids(tree.routes.iter().map(|document| &document.id)),
            quests: ids(tree.quests.iter().map(|document| &document.id)),
            factions: ids(tree.factions.iter().map(|document| &document.id)),
            mob_kinds: ids(tree.mob_kinds.iter().map(|document| &document.id)),
            abilities: ids(tree.abilities.iter().map(|document| &document.id)),
            items: ids(tree.items.iter().map(|document| &document.id)),
            loot_tables: ids(tree.loot_tables.iter().map(|document| &document.id)),
            level_curves: ids(tree.level_curves.iter().map(|document| &document.id)),
        }
    }
}

fn ids<'a>(values: impl Iterator<Item = &'a String>) -> BTreeSet<String> {
    values.cloned().collect()
}

/// Walks every edge of the authored chain.
pub fn check(tree: &SourceTree, universe: &Universe, locales: &LocaleIndex) -> ReferenceReport {
    let mut report = ReferenceReport::default();
    let mut pass = Pass {
        universe,
        report: &mut report,
    };

    for zone in &tree.zones {
        pass.locale(&zone.id, &zone.loc_ref, &zone.source, locales);
    }

    for document in &tree.placement_documents {
        pass.one("placements -> zone", &document.id, &document.zone, |u| {
            &u.zones
        });
        for placed in &document.placements {
            if let Some(target) = placed.object.id() {
                // A placement names a mob directly or names a spawn table that
                // resolves to one.
                pass.either(
                    "placement -> object",
                    &placed.id,
                    target,
                    |u| &u.mobs,
                    |u| &u.spawn_tables,
                );
            }
            if let Some(route) = placed.route.as_deref() {
                pass.one("placement -> route", &placed.id, route, |u| &u.routes);
            }
        }
    }

    for document in &tree.tables {
        pass.one("spawn table -> zone", &document.id, &document.zone, |u| {
            &u.zones
        });
        for entry in &document.entries {
            if let Some(target) = entry.object.id() {
                pass.one("spawn table -> mob", &document.id, target, |u| &u.mobs);
            }
        }
    }

    for document in &tree.mobs {
        pass.one("mob -> zone", &document.id, &document.zone, |u| &u.zones);
        pass.optional("mob -> faction", &document.id, &document.faction, |u| {
            &u.factions
        });
        pass.optional("mob -> mob kind", &document.id, &document.mob_kind, |u| {
            &u.mob_kinds
        });
        pass.optional("mob -> quality", &document.id, &document.quality, |u| {
            &u.mob_kinds
        });
        pass.optional(
            "mob -> loot table",
            &document.id,
            &document.loot_table,
            |u| &u.loot_tables,
        );
        for reference in &document.abilities {
            pass.optional_one("mob -> ability", &document.id, reference, |u| &u.abilities);
        }
        for reference in &document.quests {
            pass.optional_one("mob -> quest", &document.id, reference, |u| &u.quests);
        }
        pass.locale(&document.id, &document.loc_ref, &document.source, locales);
    }

    for document in &tree.mob_kinds {
        pass.optional(
            "mob kind -> prototype",
            &document.id,
            &document.prototype,
            |u| &u.mob_kinds,
        );
        pass.optional(
            "mob kind -> class",
            &document.id,
            &document.mob_class,
            |u| &u.mob_kinds,
        );
        pass.optional(
            "mob kind -> quality",
            &document.id,
            &document.quality,
            |u| &u.mob_kinds,
        );
        pass.locale(&document.id, &document.loc_ref, &document.source, locales);
    }

    for document in &tree.factions {
        for relation in &document.relations {
            pass.one("faction -> faction", &document.id, &relation.faction, |u| {
                &u.factions
            });
        }
        pass.locale(&document.id, &document.loc_ref, &document.source, locales);
    }

    for document in &tree.abilities {
        pass.locale(&document.id, &document.loc_ref, &document.source, locales);
    }

    for document in &tree.routes {
        pass.one("route -> zone", &document.id, &document.zone, |u| &u.zones);
    }

    for document in &tree.quests {
        pass.one("quest -> zone", &document.id, &document.zone, |u| &u.zones);
        pass.optional(
            "quest -> starter mob",
            &document.id,
            &document.starter,
            |u| &u.mobs,
        );
        pass.optional(
            "quest -> finisher mob",
            &document.id,
            &document.finisher,
            |u| &u.mobs,
        );
        for prerequisite in &document.prerequisites {
            pass.optional_one(
                "quest -> prerequisite quest",
                &document.id,
                &prerequisite.quest,
                |u| &u.quests,
            );
        }
        for objective in &document.objectives {
            for target in &objective.targets {
                if let Some(id) = target.id() {
                    pass.either(
                        "quest objective -> target",
                        &document.id,
                        id,
                        |u| &u.items,
                        |u| &u.mobs,
                    );
                }
            }
        }
        for reward in document
            .rewards
            .mandatory_items
            .iter()
            .chain(&document.rewards.alternative_items)
        {
            pass.optional_one("quest -> reward item", &document.id, &reward.item, |u| {
                &u.items
            });
        }
        pass.locale(&document.id, &document.loc_ref, &document.source, locales);
    }

    for document in &tree.items {
        pass.locale(&document.id, &document.loc_ref, &document.source, locales);
    }

    for document in &tree.loot_tables {
        let mut nodes = vec![&document.root];
        while let Some(node) = nodes.pop() {
            if let Some(id) = node.item.as_ref().and_then(ObjectRef::id) {
                pass.one("loot table -> item", &document.id, id, |u| &u.items);
            }
            nodes.extend(node.entries.iter());
        }
    }

    for document in &tree.chargen_options {
        pass.one(
            "chargen -> zone",
            &document.id,
            &document.spawn.zone_id,
            |u| &u.zones,
        );
        pass.one("chargen -> faction", &document.id, &document.faction, |u| {
            &u.factions
        });
        for entry in &document.starting_loadout {
            pass.one(
                "chargen -> loadout item",
                &document.id,
                &entry.item_id,
                |u| &u.items,
            );
        }
        for ability in &document.starting_abilities {
            pass.one("chargen -> ability", &document.id, ability, |u| {
                &u.abilities
            });
        }
        for quest in &document.starting_quests {
            pass.one("chargen -> quest", &document.id, quest, |u| &u.quests);
        }
        pass.locale(&document.id, &document.loc_ref, &document.source, locales);
    }

    report
}

struct Pass<'a> {
    universe: &'a Universe,
    report: &'a mut ReferenceReport,
}

impl Pass<'_> {
    fn one(
        &mut self,
        class: &'static str,
        referencer: &str,
        target: &str,
        pick: fn(&Universe) -> &BTreeSet<String>,
    ) {
        self.resolve(class, referencer, target, &[pick]);
    }

    fn either(
        &mut self,
        class: &'static str,
        referencer: &str,
        target: &str,
        first: fn(&Universe) -> &BTreeSet<String>,
        second: fn(&Universe) -> &BTreeSet<String>,
    ) {
        self.resolve(class, referencer, target, &[first, second]);
    }

    fn optional(
        &mut self,
        class: &'static str,
        referencer: &str,
        reference: &Option<ObjectRef>,
        pick: fn(&Universe) -> &BTreeSet<String>,
    ) {
        if let Some(target) = reference.as_ref().and_then(ObjectRef::id) {
            self.resolve(class, referencer, target, &[pick]);
        }
    }

    fn optional_one(
        &mut self,
        class: &'static str,
        referencer: &str,
        reference: &ObjectRef,
        pick: fn(&Universe) -> &BTreeSet<String>,
    ) {
        if let Some(target) = reference.id() {
            self.resolve(class, referencer, target, &[pick]);
        }
    }

    fn resolve(
        &mut self,
        class: &'static str,
        referencer: &str,
        target: &str,
        picks: &[fn(&Universe) -> &BTreeSet<String>],
    ) {
        if target.is_empty() {
            return;
        }
        if target.starts_with(INTERACTIVE_PREFIX) {
            self.report.unmodelled.push(UnmodelledRef {
                class,
                referencer: referencer.to_string(),
                target: target.to_string(),
            });
            return;
        }
        if picks
            .iter()
            .any(|pick| pick(self.universe).contains(target))
        {
            self.report.resolved += 1;
            return;
        }
        if let Some(zone) = foreign_zone(target, &self.universe.zone_slug) {
            self.report.external.push(ExternalRef {
                class,
                referencer: referencer.to_string(),
                target: target.to_string(),
                zone,
            });
            return;
        }
        self.report.dangling.push(DanglingRef {
            class,
            referencer: referencer.to_string(),
            target: target.to_string(),
        });
    }

    fn locale(
        &mut self,
        referencer: &str,
        loc_ref: &LocRef,
        source: &Provenance,
        locales: &LocaleIndex,
    ) {
        for (field, authored) in loc_ref.entries() {
            let resolved = locales.resolve(authored, source);
            if resolved.found {
                self.report.resolved += 1;
            } else {
                self.report.locale_gaps.push(LocaleGap {
                    referencer: referencer.to_string(),
                    field,
                    key: resolved.key,
                });
            }
        }
    }
}

/// The zone slug a zone-scoped id belongs to, when it is not this pack's.
fn foreign_zone(id: &str, zone_slug: &str) -> Option<String> {
    let mut segments = id.split('.');
    let prefix = segments.next()?;
    if !ZONE_SCOPED_PREFIXES.contains(&prefix) {
        return None;
    }
    let slug = segments.next()?;
    // A one-segment tail is a namespace, not a zone-qualified id.
    segments.next()?;
    (slug != zone_slug).then(|| slug.to_string())
}

/// Turns locale gaps into failures, for a build that must be fully translated.
pub fn require_locale(report: &mut ReferenceReport) {
    let gaps = std::mem::take(&mut report.locale_gaps);
    for gap in gaps {
        report.dangling.push(DanglingRef {
            class: "locale key",
            referencer: format!("{} ({})", gap.referencer, gap.field),
            target: gap.key,
        });
    }
}

/// Groups locale gaps by the id namespace that produced them, so a report can
/// say "every gap is in the item records" in one line instead of five hundred.
pub fn gaps_by_namespace(report: &ReferenceReport) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for gap in &report.locale_gaps {
        let namespace = gap
            .referencer
            .split('.')
            .next()
            .unwrap_or("unknown")
            .to_string();
        *counts.entry(namespace).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_key_resolves_before_a_qualified_one() {
        let locales = LocaleIndex {
            keys: ["TideCrab.Name.txt".to_string()].into_iter().collect(),
        };
        let source = Provenance {
            path: "Demo/Creatures/TideCrab.xdb".to_string(),
        };
        let resolved = locales.resolve("TideCrab.Name.txt", &source);
        assert!(resolved.found);
        assert_eq!(resolved.key, "TideCrab.Name.txt");
    }

    #[test]
    fn a_relative_key_resolves_against_its_source_directory() {
        let locales = LocaleIndex {
            keys: ["Creatures/ZombieWarrior/Instances/A_Name".to_string()]
                .into_iter()
                .collect(),
        };
        let source = Provenance {
            path: "Creatures/ZombieWarrior/Instances/A.(MobWorld).xdb".to_string(),
        };
        let resolved = locales.resolve("A_Name.txt", &source);
        assert!(resolved.found);
        assert_eq!(resolved.key, "Creatures/ZombieWarrior/Instances/A_Name");
    }

    #[test]
    fn an_absolute_key_loses_its_leading_slash_and_extension() {
        let locales = LocaleIndex {
            keys: ["World/Quests/InstLeague1/Quest_1_10/Name".to_string()]
                .into_iter()
                .collect(),
        };
        let source = Provenance {
            path: "World/Quests/InstLeague1/Quest_1_10/Quest_1_10.xdb".to_string(),
        };
        let resolved = locales.resolve("/World/Quests/InstLeague1/Quest_1_10/Name.txt", &source);
        assert!(resolved.found);
    }

    #[test]
    fn an_unresolved_key_reports_not_found_and_the_caller_drops_it() {
        let locales = LocaleIndex::default();
        let source = Provenance {
            path: "Items/Mechanics/ZombieDenture.xdb".to_string(),
        };
        let resolved = locales.resolve("ZombieDenture.txt", &source);
        assert!(!resolved.found);
        assert_eq!(resolved.key, "ZombieDenture.txt");
    }

    #[test]
    fn a_foreign_zone_id_is_external_and_a_local_one_is_not() {
        assert_eq!(
            foreign_zone(
                "mob.archipelago-league1.elf-female.orc-quard",
                "inst-league1"
            ),
            Some("archipelago-league1".to_string())
        );
        assert_eq!(
            foreign_zone("mob.inst-league1.rat.rat1-1", "inst-league1"),
            None
        );
        assert_eq!(
            foreign_zone("item.mechanics.broken-armor", "inst-league1"),
            None
        );
        assert_eq!(foreign_zone("faction.wild", "inst-league1"), None);
    }
}
