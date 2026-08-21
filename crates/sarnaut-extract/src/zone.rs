use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use roxmltree::Node;

use crate::model::{
    ExtractionOptions, LocRefs, MobDocument, PlacementDocument, Provenance, QuestDocument,
    QuestObjective, QuestPrerequisite, QuestRewards, ResourceRef, RewardItem, RouteDocument,
    RouteLink, RoutePoint, SpawnEntry, SpawnPlacement, SpawnTableDocument, ZoneSummary,
};
use crate::objective::derive_objective_id;
use crate::output::OutputWriter;
use crate::reference::{
    canonical_id_from_source_path, canonical_zone_slug, resource_ref, slug, slug_path,
};
use crate::scan::{find_named_directory, loc_key, sorted_xdb_files, zone_instance_files};
use crate::validation::SchemaKind;
use crate::xdb::{
    bool_value, child, children, extra_fields, f64_value, href, i64_value, node_value, orientation,
    parse_document, position, read_xdb, resource_id, text,
};

const QUEST_RESOURCE: &str = "gameMechanics.constructor.schemes.quest.QuestResource";
const MOB_RESOURCE: &str = "gameMechanics.world.mob.MobWorld";
const SPAWN_TABLE: &str = "gameMechanics.map.spawn.SpawnTable";
const PATCH_OBJECTS: &str = "gameMechanics.map.PatchObjects";

pub fn extract_zone(name: &str, options: &ExtractionOptions) -> Result<ZoneSummary> {
    let resolved = resolve_zone(&options.src, name)?;
    let zone_slug = canonical_zone_slug(&resolved.zone);
    let map_slug = slug(&resolved.map);
    let zone_output = options.out.join("zones").join(&zone_slug);
    let writer = OutputWriter::new(options.dry_run, options.schema_dir.as_deref())?;
    let mut summary = ZoneSummary {
        zone: resolved.zone.clone(),
        map: resolved.map.clone(),
        ..ZoneSummary::default()
    };

    let mobs = load_mobs(&options.src, &resolved.zone)?;
    let mut starters = BTreeMap::new();
    for (_, mob) in &mobs {
        for quest in &mob.quests {
            if let Some(quest_id) = &quest.id {
                starters
                    .entry(quest_id.clone())
                    .or_insert_with(|| ResourceRef {
                        id: Some(mob.id.clone()),
                        href: mob.source.path.clone(),
                    });
            }
        }
    }

    for path in sorted_xdb_files(&resolved.quest_dir)? {
        let xdb = read_xdb(&path, &options.src)?;
        let xml = parse_document(&xdb.text, &path)?;
        let root = xml.root_element();
        if root.tag_name().name() != QUEST_RESOURCE {
            continue;
        }
        let id = canonical_id_from_source_path(&xdb.source.path)
            .with_context(|| format!("build quest ID from {}", xdb.source.path))?;
        let document = quest_document(
            root,
            id.clone(),
            zone_slug.clone(),
            starters.get(&id).cloned(),
            &options.src,
            &path,
            xdb.source,
        )?;
        let file_slug = id
            .strip_prefix(&format!("quest.{zone_slug}."))
            .context("quest ID zone prefix")?;
        let output = zone_output.join("quests").join(format!("{file_slug}.yaml"));
        summary.unchanged += usize::from(writer.write(&output, SchemaKind::Quest, &document)?);
        summary.quests += 1;
    }

    for (_, document) in mobs {
        let file_slug = document
            .id
            .strip_prefix(&format!("mob.{zone_slug}."))
            .context("mob ID zone prefix")?;
        let output = zone_output
            .join("spawns/mobs")
            .join(format!("{file_slug}.yaml"));
        summary.unchanged += usize::from(writer.write(&output, SchemaKind::Spawn, &document)?);
        summary.mobs += 1;
    }

    if let Some(spawn_dir) = &resolved.spawn_dir {
        for path in sorted_xdb_files(spawn_dir)? {
            let xdb = read_xdb(&path, &options.src)?;
            let xml = parse_document(&xdb.text, &path)?;
            let root = xml.root_element();
            if root.tag_name().name() != SPAWN_TABLE {
                continue;
            }
            let id = canonical_id_from_source_path(&xdb.source.path)
                .with_context(|| format!("build spawn table ID from {}", xdb.source.path))?;
            let document = spawn_table_document(root, id.clone(), &zone_slug, xdb.source);
            let file_slug = id
                .strip_prefix(&format!("spawn.{zone_slug}.table."))
                .context("spawn table ID zone prefix")?;
            let output = zone_output
                .join("spawns/tables")
                .join(format!("{file_slug}.yaml"));
            summary.unchanged +=
                usize::from(writer.write(&output, SchemaKind::Spawn, &document)?);
            summary.spawn_tables += 1;
        }
    }

    for path in server_object_files(&resolved.map_dir)? {
        let xdb = read_xdb(&path, &options.src)?;
        let xml = parse_document(&xdb.text, &path)?;
        let root = xml.root_element();
        if root.tag_name().name() != PATCH_OBJECTS {
            continue;
        }
        let relative = path.strip_prefix(&resolved.map_dir)?;
        let parts: Vec<_> = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        let borrowed: Vec<_> = parts.iter().map(String::as_str).collect();
        let source_slug = slug_path(&borrowed);
        let origin = tile_origin(&resolved.map_dir, &path)?;
        let (placements, routes) = placements_and_routes(
            root,
            &zone_slug,
            &map_slug,
            &source_slug,
            &xdb.source,
            origin,
        );
        if !placements.is_empty() {
            let document = PlacementDocument {
                schema_version: 1,
                id: format!("spawn.{zone_slug}.placements.{source_slug}"),
                kind: "placements".into(),
                zone: format!("zone.{zone_slug}"),
                map: map_slug.clone(),
                source_type: root.tag_name().name().to_owned(),
                placements,
                source: xdb.source.clone(),
            };
            let output = zone_output
                .join("spawns/placements")
                .join(format!("{source_slug}.yaml"));
            summary.spawn_points += document.placements.len();
            summary.unchanged +=
                usize::from(writer.write(&output, SchemaKind::Spawn, &document)?);
        }
        for route in routes {
            let file_slug = route
                .id
                .strip_prefix(&format!("route.{zone_slug}."))
                .context("route ID zone prefix")?;
            let output = zone_output.join("routes").join(format!("{file_slug}.yaml"));
            summary.unchanged += usize::from(writer.write(&output, SchemaKind::Route, &route)?);
            summary.routes += 1;
        }
    }

    Ok(summary)
}

pub(crate) struct ResolvedZone {
    pub(crate) zone: String,
    pub(crate) map: String,
    pub(crate) quest_dir: PathBuf,
    pub(crate) map_dir: PathBuf,
    pub(crate) spawn_dir: Option<PathBuf>,
}

pub(crate) fn resolve_zone(src: &Path, requested: &str) -> Result<ResolvedZone> {
    let quest_root = src.join("World/Quests");
    let map_root = src.join("Maps");
    let quest_dir = find_named_directory(&quest_root, requested)
        .with_context(|| format!("find quest zone {requested} in {}", quest_root.display()))?;
    let zone = quest_dir
        .file_name()
        .context("quest zone directory name")?
        .to_string_lossy()
        .into_owned();

    let direct_map = find_named_directory(&map_root, requested);
    let map_dir = if let Some(path) = direct_map {
        path
    } else {
        let mut candidates = Vec::new();
        for entry in fs::read_dir(&map_root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let zones = entry.path().join("Zones");
            if find_named_directory(&zones, &zone).is_some() {
                candidates.push(entry.path());
            }
        }
        candidates.sort_by_cached_key(|path| path.to_string_lossy().to_ascii_lowercase());
        match candidates.as_slice() {
            [only] => only.clone(),
            [] => bail!("no map contains zone {zone}"),
            many => bail!(
                "zone {zone} occurs in multiple maps: {}",
                many.iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    };
    let map = map_dir
        .file_name()
        .context("map directory name")?
        .to_string_lossy()
        .into_owned();
    let spawn_dir = find_named_directory(&map_dir.join("SpawnTables"), &zone);
    Ok(ResolvedZone {
        zone,
        map,
        quest_dir,
        map_dir,
        spawn_dir,
    })
}

fn load_mobs(src: &Path, zone: &str) -> Result<Vec<(PathBuf, MobDocument)>> {
    let files = zone_instance_files(src, zone)?;
    let mut documents = Vec::new();
    for path in files {
        let xdb = read_xdb(&path, src)?;
        let xml = parse_document(&xdb.text, &path)?;
        let root = xml.root_element();
        if root.tag_name().name() != MOB_RESOURCE {
            continue;
        }
        let id = canonical_id_from_source_path(&xdb.source.path)
            .with_context(|| format!("build mob ID from {}", xdb.source.path))?;
        let document = mob_document(root, id, &canonical_zone_slug(zone), src, &path, xdb.source);
        documents.push((path, document));
    }
    Ok(documents)
}

fn mob_document(
    root: Node<'_, '_>,
    id: String,
    zone: &str,
    src: &Path,
    path: &Path,
    source: Provenance,
) -> MobDocument {
    let quests = child(root, "interactions")
        .and_then(|node| child(node, "availableQuests"))
        .map(|node| {
            children(node, "Item")
                .filter_map(|item| item.attribute("href"))
                .filter(|value| !value.is_empty())
                .map(resource_ref)
                .collect()
        })
        .unwrap_or_default();
    let abilities = child(root, "personalAbilities")
        .map(|node| {
            children(node, "Item")
                .filter_map(|item| item.attribute("href"))
                .filter(|value| !value.is_empty())
                .map(resource_ref)
                .collect()
        })
        .unwrap_or_default();
    MobDocument {
        schema_version: 1,
        id,
        kind: "mob".into(),
        zone: format!("zone.{zone}"),
        source_type: root.tag_name().name().to_owned(),
        resource_id: resource_id(root),
        loc_ref: LocRefs {
            name: localized_href(root, "name", src, path),
            title: localized_href(root, "title", src, path),
            ..LocRefs::default()
        },
        visual_ref: href(root, "visMob"),
        mob_kind: href(root, "kind").map(|value| resource_ref(&value)),
        faction: href(root, "faction").map(|value| resource_ref(&value)),
        quality: href(root, "quality").map(|value| resource_ref(&value)),
        level_min: i64_value(root, "levelMin"),
        level_max: i64_value(root, "levelMax"),
        walk_speed: f64_value(root, "walkSpeed"),
        quests,
        abilities,
        extra: extra_fields(
            root,
            &[
                "Header",
                "name",
                "title",
                "visMob",
                "kind",
                "faction",
                "quality",
                "levelMin",
                "levelMax",
                "walkSpeed",
                "interactions",
                "personalAbilities",
            ],
        ),
        source,
    }
}

fn quest_document(
    root: Node<'_, '_>,
    id: String,
    zone: String,
    starter: Option<ResourceRef>,
    src: &Path,
    path: &Path,
    source: Provenance,
) -> Result<QuestDocument> {
    let finisher = child(root, "finisher")
        .and_then(|node| href(node, "mobWorld"))
        .map(|value| resource_ref(&value));
    let prerequisites = child(root, "startConditions")
        .map(|conditions| {
            conditions
                .descendants()
                .filter(Node::is_element)
                .filter_map(|node| {
                    let quest = href(node, "quest")?;
                    Some(QuestPrerequisite {
                        quest: resource_ref(&quest),
                        status: text(node, "status"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let objectives: Vec<_> = child(root, "counters")
        .map(|node| {
            children(node, "Item")
                .map(|item| quest_objective(item, &id, src, path))
                .collect()
        })
        .unwrap_or_default();
    let mut objective_ids = BTreeMap::new();
    for objective in &objectives {
        if let Some(previous_kind) =
            objective_ids.insert(objective.objective_id.clone(), objective.kind.clone())
        {
            bail!(
                "quest {id} has colliding objective id {} for {previous_kind} and {}",
                objective.objective_id,
                objective.kind
            );
        }
    }
    let rewards = child(root, "reward").map(quest_rewards).unwrap_or_default();
    let mut flags = BTreeMap::new();
    for (source_name, output_name) in [
        ("canCancel", "can_cancel"),
        ("pvp", "pvp"),
        ("secretSequence", "secret_sequence"),
        ("tutorial", "tutorial"),
        ("autoSetGoalLocation", "auto_set_goal_location"),
        ("autoSetReturnLocation", "auto_set_return_location"),
    ] {
        if let Some(value) = bool_value(root, source_name) {
            flags.insert(output_name.to_owned(), value);
        }
    }

    Ok(QuestDocument {
        schema_version: 1,
        id,
        zone: format!("zone.{zone}"),
        source_type: root.tag_name().name().to_owned(),
        resource_id: resource_id(root),
        internal_name: text(root, "internalName"),
        loc_ref: LocRefs {
            name: localized_href(root, "name", src, path),
            goal: localized_href(root, "goal", src, path),
            start: localized_href(root, "startText", src, path),
            check: localized_href(root, "checkText", src, path),
            finish: localized_href(root, "finishText", src, path),
            ..LocRefs::default()
        },
        level: i64_value(root, "level"),
        required_level: i64_value(root, "requiredLevel"),
        quest_type: text(root, "type").map(|value| slug(&value)),
        plotline: text(root, "plotline"),
        starter,
        finisher,
        prerequisites,
        objectives,
        rewards,
        flags,
        extra: extra_fields(
            root,
            &[
                "Header",
                "internalName",
                "name",
                "goal",
                "startText",
                "checkText",
                "finishText",
                "level",
                "requiredLevel",
                "type",
                "plotline",
                "finisher",
                "startConditions",
                "counters",
                "reward",
                "canCancel",
                "pvp",
                "secretSequence",
                "tutorial",
                "autoSetGoalLocation",
                "autoSetReturnLocation",
            ],
        ),
        source,
    })
}

fn quest_objective(node: Node<'_, '_>, quest_id: &str, src: &Path, path: &Path) -> QuestObjective {
    let mut targets = Vec::new();
    for collection in ["items", "objects", "mobs"] {
        if let Some(parent) = child(node, collection) {
            targets.extend(
                children(parent, "Item")
                    .filter_map(|item| item.attribute("href"))
                    .filter(|value| !value.is_empty())
                    .map(resource_ref),
            );
        }
    }
    let kind = node
        .attribute("type")
        .map(type_tail)
        .map(slug)
        .unwrap_or_else(|| "unknown".into());
    let objective_id = derive_objective_id(
        quest_id,
        &kind,
        href(node, "id").as_deref(),
        href(node, "customName").as_deref(),
        targets
            .iter()
            .map(|target| target.id.as_deref().unwrap_or(&target.href)),
    );
    QuestObjective {
        objective_id,
        kind,
        loc_ref: localized_href(node, "customName", src, path),
        limit: i64_value(node, "limit"),
        internal: bool_value(node, "isInternal"),
        show_count: bool_value(node, "showCounterValue"),
        targets,
        extra: extra_fields(
            node,
            &[
                "customName",
                "limit",
                "isInternal",
                "showCounterValue",
                "items",
                "objects",
                "mobs",
            ],
        ),
    }
}

fn localized_href(node: Node<'_, '_>, name: &str, src: &Path, path: &Path) -> Option<String> {
    href(node, name).and_then(|value| loc_key(src, path, &value))
}

fn quest_rewards(node: Node<'_, '_>) -> QuestRewards {
    let reputations = child(node, "reputations")
        .map(|parent| children(parent, "Item").map(node_value).collect())
        .unwrap_or_default();
    QuestRewards {
        experience: i64_value(node, "experience"),
        money: i64_value(node, "money"),
        honor: i64_value(node, "honor"),
        mandatory_items: reward_items(node, "mandatoryItems"),
        alternative_items: reward_items(node, "alternativeItems"),
        reputations,
    }
}

fn reward_items(node: Node<'_, '_>, collection: &str) -> Vec<RewardItem> {
    child(node, collection)
        .map(|parent| {
            children(parent, "Item")
                .filter_map(|entry| {
                    let href = href(entry, "item")?;
                    Some(RewardItem {
                        item: resource_ref(&href),
                        count: i64_value(entry, "number").unwrap_or(1),
                        hidden: bool_value(entry, "hidden"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn spawn_table_document(
    root: Node<'_, '_>,
    id: String,
    zone: &str,
    source: Provenance,
) -> SpawnTableDocument {
    let mut entries = Vec::new();
    for group in root.children().filter(Node::is_element) {
        for entry in children(group, "Item") {
            let Some(object_href) = href(entry, "object") else {
                continue;
            };
            let spawn_time = child(entry, "spawnTime")
                .and_then(|value| value.attribute("type"))
                .map(type_tail)
                .map(slug);
            entries.push(SpawnEntry {
                group: slug(group.tag_name().name()),
                object: resource_ref(&object_href),
                chance: f64_value(entry, "chance"),
                spawn_time,
                extra: extra_fields(entry, &["object", "chance", "spawnTime"]),
            });
        }
    }
    SpawnTableDocument {
        schema_version: 1,
        id,
        kind: "table".into(),
        zone: format!("zone.{zone}"),
        source_type: root.tag_name().name().to_owned(),
        resource_id: resource_id(root),
        scripted: bool_value(root, "isScriptSpawn"),
        entries,
        extra: extra_fields(root, &["Header", "isScriptSpawn"]),
        source,
    }
}

fn placements_and_routes(
    root: Node<'_, '_>,
    zone: &str,
    map: &str,
    source_slug: &str,
    source: &Provenance,
    origin: TileOrigin,
) -> (Vec<SpawnPlacement>, Vec<RouteDocument>) {
    let mut placements = Vec::new();
    let mut routes = Vec::new();
    let Some(objects) = child(root, "objects") else {
        return (placements, routes);
    };
    for object in children(objects, "Item") {
        let object_type = object.attribute("type").unwrap_or_default();
        if object_type.ends_with("SpawnLocus") {
            let Some(table_href) = href(object, "spawnTable") else {
                continue;
            };
            let Some(places) = child(object, "places") else {
                continue;
            };
            for place in children(places, "Item") {
                push_placement(
                    &mut placements,
                    &mut routes,
                    PlacementContext {
                        zone,
                        map,
                        source_slug,
                        source,
                        script_id: text(object, "scriptID"),
                        object: resource_ref(&table_href),
                        scan_radius: f64_value(object, "scanRadius"),
                        spawn_time: None,
                        origin,
                    },
                    place,
                );
            }
        } else if object_type.ends_with("MobSingleSpawn") {
            let Some(object_href) = href(object, "object") else {
                continue;
            };
            let Some(place) = child(object, "place") else {
                continue;
            };
            let spawn_time = child(object, "spawnTime")
                .and_then(|value| value.attribute("type"))
                .map(type_tail)
                .map(slug);
            push_placement(
                &mut placements,
                &mut routes,
                PlacementContext {
                    zone,
                    map,
                    source_slug,
                    source,
                    script_id: text(object, "scriptID"),
                    object: resource_ref(&object_href),
                    scan_radius: f64_value(object, "scanRadius"),
                    spawn_time,
                    origin,
                },
                place,
            );
        }
    }
    (placements, routes)
}

struct PlacementContext<'a> {
    zone: &'a str,
    map: &'a str,
    source_slug: &'a str,
    source: &'a Provenance,
    script_id: Option<String>,
    object: ResourceRef,
    scan_radius: Option<f64>,
    spawn_time: Option<String>,
    origin: TileOrigin,
}

fn push_placement(
    placements: &mut Vec<SpawnPlacement>,
    routes: &mut Vec<RouteDocument>,
    context: PlacementContext<'_>,
    place: Node<'_, '_>,
) {
    let index = placements.len() + 1;
    let route = child(place, "points").and_then(|points| {
        let route_points: Vec<_> = children(points, "Item")
            .enumerate()
            .filter_map(|(index, item)| {
                Some(RoutePoint {
                    index,
                    position: context
                        .origin
                        .apply(child(item, "coords").and_then(position)?),
                    script_ref: href(item, "script"),
                })
            })
            .collect();
        if route_points.is_empty() {
            return None;
        }
        let route_id = format!("route.{}.{}.{}", context.zone, context.source_slug, index);
        let links = child(place, "links")
            .map(|parent| {
                children(parent, "Item")
                    .map(|item| {
                        let transfer = child(item, "transferenceType");
                        RouteLink {
                            from: i64_value(item, "first").unwrap_or(0).max(0) as usize,
                            to: i64_value(item, "second").unwrap_or(0).max(0) as usize,
                            weight: f64_value(item, "weight"),
                            movement: transfer
                                .and_then(|node| node.attribute("type"))
                                .map(type_tail)
                                .map(slug),
                            flying: transfer.and_then(|node| bool_value(node, "isFly")),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        routes.push(RouteDocument {
            schema_version: 1,
            id: route_id.clone(),
            zone: format!("zone.{}", context.zone),
            map: context.map.to_owned(),
            source_type: "gameMechanics.map.spawn.patrol.SpawnPlacePatrol".into(),
            points: route_points,
            links,
            source: context.source.clone(),
        });
        Some(route_id)
    });
    let center = child(place, "center")
        .and_then(position)
        .or_else(|| {
            child(place, "points")
                .and_then(|points| child(points, "Item"))
                .and_then(|point| child(point, "coords"))
                .and_then(position)
        })
        .map(|position| context.origin.apply(position));
    let place_orientation = orientation(place);
    placements.push(SpawnPlacement {
        id: format!(
            "spawn.{}.placement.{}.{}",
            context.zone, context.source_slug, index
        ),
        script_id: context.script_id,
        object: context.object,
        position: center,
        orientation: place_orientation,
        route,
        tuner_ref: href(place, "tuner"),
        spawn_time: context.spawn_time,
        scan_radius: context.scan_radius,
    });
}

fn type_tail(value: &str) -> &str {
    value.rsplit('.').next().unwrap_or(value)
}

fn server_object_files(map_root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = sorted_xdb_files(map_root)?;
    files.retain(|path| {
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with("_ServerObjects.xdb"))
    });
    Ok(files)
}

const TILE_PITCH: f64 = 256.0;
const POSITION_PRECISION: f64 = 1_000_000_000.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct TileOrigin {
    x: f64,
    y: f64,
}

impl TileOrigin {
    fn apply(self, local: crate::model::Position) -> crate::model::Position {
        crate::model::Position {
            x: rounded_position(self.x + local.x),
            y: rounded_position(self.y + local.y),
            z: local.z,
        }
    }
}

fn rounded_position(value: f64) -> f64 {
    (value * POSITION_PRECISION).round() / POSITION_PRECISION
}

/// Parses ADR 0038's `SX_SY/R_C_ServerObjects.xdb` naming contract. The first
/// sector and tile components contribute X; the second components contribute Y.
fn tile_origin(map_root: &Path, server_objects: &Path) -> Result<TileOrigin> {
    let relative = server_objects.strip_prefix(map_root).with_context(|| {
        format!(
            "{} is outside map root {}",
            server_objects.display(),
            map_root.display()
        )
    })?;
    let sector_name = relative
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .context("server-object path has no UTF-8 sector directory")?;
    let file_name = relative
        .file_name()
        .and_then(|name| name.to_str())
        .context("server-object path has no UTF-8 filename")?;
    let tile_name = file_name
        .strip_suffix("_ServerObjects.xdb")
        .with_context(|| format!("server-object filename {file_name:?} has no expected suffix"))?;
    let (sector_x, sector_y) = coordinate_pair(sector_name, "sector directory")?;
    let (tile_x, tile_y) = coordinate_pair(tile_name, "tile filename")?;
    Ok(TileOrigin {
        x: f64::from(sector_x + tile_x) * TILE_PITCH,
        y: f64::from(sector_y + tile_y) * TILE_PITCH,
    })
}

fn coordinate_pair(value: &str, label: &str) -> Result<(u32, u32)> {
    let (first, second) = value
        .split_once('_')
        .with_context(|| format!("{label} {value:?} is not FIRST_SECOND"))?;
    if second.contains('_') {
        bail!("{label} {value:?} has more than two components");
    }
    let first = first
        .parse()
        .with_context(|| format!("{label} {value:?} has a non-numeric first component"))?;
    let second = second
        .parse()
        .with_context(|| format!("{label} {value:?} has a non-numeric second component"))?;
    Ok((first, second))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::testing::write;

    #[test]
    fn extracts_quest_mob_spawn_and_patrol_route() {
        let source = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        write(
            &source.path().join("World/Quests/TestZone/First/First.xdb"),
            r#"<gameMechanics.constructor.schemes.quest.QuestResource>
<Header><resourceId>1</resourceId></Header><level>1</level><name href="Name.txt"/><goal href="Goal.txt"/><internalName>First</internalName><type>QUEST_TYPE_SOLO</type>
<startConditions><Item type="PredicateQuestStatus"><quest href="/World/Quests/TestZone/Before/Before.xdb#x"/><status>Finished</status></Item></startConditions>
<finisher type="QuestFinisherMob"><mobWorld href="/Characters/Human/Instances/TestZone/Guide.(MobWorld).xdb#x"/></finisher>
<counters><Item type="QuestCountKill"><customName href="Counter.txt"/><limit>2</limit><objects><Item href="/Creatures/Rat/Instances/TestZone/Rat.(MobWorld).xdb#x"/></objects><unknown>kept</unknown></Item></counters>
<reward><experience>5</experience><money>2</money><mandatoryItems><Item><item href="/Items/Junk/Fur.xdb#x"/><number>1</number></Item></mandatoryItems></reward>
</gameMechanics.constructor.schemes.quest.QuestResource>"#,
        );
        write(
            &source.path().join("Maps/TestMap/Zones/TestZone/Zone.xdb"),
            "<ZoneResource/>",
        );
        write(
            &source
                .path()
                .join("Characters/Human/Instances/TestZone/Guide.(MobWorld).xdb"),
            r#"<gameMechanics.world.mob.MobWorld><Header><resourceId>2</resourceId></Header><name href="Guide.txt"/><levelMin>1</levelMin><levelMax>1</levelMax><interactions><availableQuests><Item href="/World/Quests/TestZone/First/First.xdb#x"/></availableQuests></interactions></gameMechanics.world.mob.MobWorld>"#,
        );
        write(
            &source
                .path()
                .join("Maps/TestMap/SpawnTables/TestZone/Rats.(MobSpawnTable).xdb"),
            r#"<gameMechanics.map.spawn.SpawnTable><Header><resourceId>3</resourceId></Header><isScriptSpawn>false</isScriptSpawn><commons><Item><object href="/Creatures/Rat/Instances/TestZone/Rat.(MobWorld).xdb#x"/><chance>1</chance><spawnTime type="TimeOnce"/></Item></commons></gameMechanics.map.spawn.SpawnTable>"#,
        );
        write(
            &source
                .path()
                .join("Maps/TestMap/000_000/0_0_ServerObjects.xdb"),
            r#"<gameMechanics.map.PatchObjects><objects><Item type="gameMechanics.map.spawn.SpawnLocus"><scanRadius>10</scanRadius><spawnTable href="/Maps/TestMap/SpawnTables/TestZone/Rats.(MobSpawnTable).xdb#x"/><places><Item type="gameMechanics.map.spawn.patrol.SpawnPlacePatrol"><points><Item><coords x="1" y="2" z="3"/></Item><Item><coords x="4" y="5" z="6"/></Item></points><links><Item><first>0</first><second>1</second><weight>1</weight><transferenceType type="Walk"><isFly>false</isFly></transferenceType></Item></links></Item></places></Item></objects></gameMechanics.map.PatchObjects>"#,
        );

        let summary = extract_zone(
            "TestZone",
            &ExtractionOptions {
                src: source.path().into(),
                out: output.path().into(),
                dry_run: false,
                schema_dir: None,
            },
        )
        .unwrap();
        assert_eq!(summary.quests, 1);
        assert_eq!(summary.mobs, 1);
        assert_eq!(summary.spawn_tables, 1);
        assert_eq!(summary.spawn_points, 1);
        assert_eq!(summary.routes, 1);
        let quest =
            fs::read_to_string(output.path().join("zones/test-zone/quests/first.yaml")).unwrap();
        assert!(quest.contains("starter:"));
        assert!(quest.contains("kind: quest-count-kill"));
        assert!(quest.contains("experience: 5"));
        assert!(quest.contains("unknown: kept"));
        assert!(
            quest.contains("name: World/Quests/TestZone/First/Name"),
            "{quest}"
        );
        assert!(
            quest.contains("loc_ref: World/Quests/TestZone/First/Counter"),
            "{quest}"
        );
        assert!(
            quest.contains("objective_id: quest.test-zone.first.objective."),
            "{quest}"
        );

        let mob = fs::read_to_string(
            output
                .path()
                .join("zones/test-zone/spawns/mobs/human.guide.yaml"),
        )
        .unwrap();
        assert!(
            mob.contains("name: Characters/Human/Instances/TestZone/Guide"),
            "{mob}"
        );
    }

    #[test]
    fn instleague1_patch_local_placement_round_trips_to_global_frame() {
        let map_root = Path::new("Maps/Inst_LeagueStart");
        let source = map_root.join("000_020/1_2_ServerObjects.xdb");
        let origin = tile_origin(map_root, &source).unwrap();
        let local = crate::model::Position {
            x: 61.1128,
            y: 241.054,
            z: 56.4219,
        };

        // Derive the expected value from ADR 0038's names, so swapping the
        // filename axes or omitting either origin makes this regression fail.
        let sector_x = 0.0;
        let sector_y = 20.0;
        let tile_x = 1.0;
        let tile_y = 2.0;
        let expected_x = (sector_x + tile_x) * TILE_PITCH + local.x;
        let expected_y = (sector_y + tile_y) * TILE_PITCH + local.y;
        let global = origin.apply(local);

        assert_eq!(global.x, expected_x);
        assert_eq!(global.y, expected_y);
        assert_eq!(global.z, local.z);
        assert!(global.y > TILE_PITCH, "placement remained tile-local");
    }

    #[test]
    fn adjacent_tile_wrap_and_sector_axis_match_adr_0038() {
        let map_root = Path::new("Maps/Inst_LeagueStart");
        let tile_1_2 =
            tile_origin(map_root, &map_root.join("000_020/1_2_ServerObjects.xdb")).unwrap();
        let tile_1_3 =
            tile_origin(map_root, &map_root.join("000_020/1_3_ServerObjects.xdb")).unwrap();
        let sector_20_20 =
            tile_origin(map_root, &map_root.join("020_020/1_2_ServerObjects.xdb")).unwrap();

        assert_eq!(tile_1_3.x, tile_1_2.x);
        assert_eq!(tile_1_3.y - tile_1_2.y, TILE_PITCH);
        assert_eq!(sector_20_20.x - tile_1_2.x, 20.0 * TILE_PITCH);
        assert_eq!(sector_20_20.y, tile_1_2.y);

        let before_wrap = tile_1_2.apply(crate::model::Position {
            x: 61.1128,
            y: 255.472,
            z: 0.0,
        });
        let after_wrap = tile_1_3.apply(crate::model::Position {
            x: 61.1128,
            y: 0.819767,
            z: 0.0,
        });
        assert!(after_wrap.y > before_wrap.y);
        assert!(after_wrap.y - before_wrap.y < 2.0);
    }

    #[test]
    fn objective_ids_survive_reorder_rerun_and_unrelated_fields() {
        fn ids(xml: &str) -> Vec<String> {
            let document = roxmltree::Document::parse(xml).unwrap();
            let root = document.root_element();
            let mut ids: Vec<_> = children(child(root, "counters").unwrap(), "Item")
                .map(|node| {
                    quest_objective(
                        node,
                        "quest.test-zone.stable",
                        Path::new("source"),
                        Path::new("source/Quest.xdb"),
                    )
                    .objective_id
                })
                .collect();
            ids.sort();
            ids
        }

        let first = r#"<QuestResource><counters>
            <Item type="QuestCountSpecial"><id href="CountId_1.xdb#old"/><limit>1</limit></Item>
            <Item type="QuestCountKill"><objects><Item href="/Creatures/Rat.xdb#old"/></objects></Item>
        </counters></QuestResource>"#;
        let reordered_with_new_field = r#"<QuestResource><counters>
            <Item type="QuestCountKill"><objects><Item href="/Creatures/Rat.xdb#new"/></objects><futureSchemaField>ignored</futureSchemaField></Item>
            <Item type="QuestCountSpecial"><id href="CountId_1.xdb#new"/><limit>99</limit><futureSchemaField>ignored</futureSchemaField></Item>
        </counters></QuestResource>"#;

        assert_eq!(ids(first), ids(first), "two reruns must be stable");
        assert_eq!(ids(first), ids(reordered_with_new_field));
    }

    #[test]
    fn colliding_objective_ids_fail_extraction() {
        let xml = roxmltree::Document::parse(
            r#"<gameMechanics.constructor.schemes.quest.QuestResource><counters>
                <Item type="QuestCountKill"><objects><Item href="/Creatures/Rat.xdb"/></objects></Item>
                <Item type="QuestCountKill"><objects><Item href="/Creatures/Rat.xdb"/></objects></Item>
            </counters><reward/></gameMechanics.constructor.schemes.quest.QuestResource>"#,
        )
        .unwrap();
        let error = quest_document(
            xml.root_element(),
            "quest.test-zone.collision".into(),
            "test-zone".into(),
            None,
            Path::new("source"),
            Path::new("source/Quest.xdb"),
            Provenance {
                path: "Quest.xdb".into(),
                blake3: "test".into(),
                extractor: "test".into(),
                source_root: None,
                prototype_chain: None,
            },
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("colliding objective id"),
            "{error:#}"
        );
    }
}
