# sarnaut-pack

Compiles authored SarnautCore YAML into the runtime pack format of
[ADR 0029](https://github.com/SarnautCore/docs/blob/main/adr/0029-runtime-pack-format.md).
The shard reads packs and never parses YAML ([ADR 0006](https://github.com/SarnautCore/docs/blob/main/adr/0006-yaml-source-compiled-runtime-data.md)).

A pack is a directory, not an archive:

```
<pack>/
  manifest.json
  build-report.json
  tables/zone.sptbl
  tables/placements.sptbl
  tables/spawn-tables.sptbl
  tables/abilities.sptbl
  tables/factions.sptbl
  tables/mobs.sptbl
  tables/chargen.sptbl        # when the tree authors any
  tables/items.sptbl
  tables/loot-tables.sptbl
  tables/quests.sptbl
  tables/routes.sptbl
  tables/locale.sptbl
  tables/mob-kinds.sptbl
  tables/level-curve.sptbl
```

The first six tables are written even when they hold no rows, so a reader can
insist on the full set rather than treating "absent" and "empty" as one case.
The rest appear only when the source tree carries documents of that kind, so a
pack built before a row type existed keeps its digest.

`pack_id` is a BLAKE3-256 digest over the table bytes alone. The manifest is not
an input, so a builder-version bump that produces identical tables leaves the id
alone — which is the property the ADR 0027 handshake comparison needs.
`build-report.json` is not an input either, which is what lets a curation note
be reworded without moving the digest.

## Building the crate

`prost-build` shells out to `protoc`. Install it, or point `PROTOC` at the
binary:

```powershell
$env:PROTOC = "C:\tools\protoc\bin\protoc.exe"
cargo build -p sarnaut-pack
```

## Commands

```powershell
# Compile one zone out of the private data repository.
sarnaut-pack build `
  --src E:\SarnautCore\data --ruleset classic --zone inst-league1 `
  --out E:\SarnautCore\data\packs\classic\inst-league1

# Compile the same tree in memory and report what it found. Writes nothing.
sarnaut-pack check --src E:\SarnautCore\data --ruleset classic --zone inst-league1

# Compile the golden fixture from the hand-authored demo dataset.
sarnaut-pack build --fixture --src ..\data-schemas\demo --out ..\server\testdata\packs\demo

# The same fixture with one overlay layer applied.
sarnaut-pack build --fixture --src ..\data-schemas\demo `
  --overlay m2-combat-extended `
  --out ..\server\testdata\packs\demo-extended

# Check a built pack's manifest digest against its table bytes.
sarnaut-pack verify E:\SarnautCore\data\packs\classic\inst-league1
```

Packs built from `data` are private-path artifacts: `data/.gitignore` carries
`packs/`, and no pack but the fixture may be committed to a public repository.

## Determinism

Two builds over one source tree produce byte-identical output, `pack_id`
included. Rows are sorted by canonical id, locale entries are sorted by key,
protobuf fields are emitted in ascending field number, and no timestamp, path or
hostname reaches the table bytes. `--fixture` additionally pins `source.commit`
to zeros, because a fixture that recorded whichever commit was checked out would
stop matching the copy vendored in `server`.

## What reaches a pack

`zone`, `placements`, `spawn-tables` and `routes` are what the shard resolves at
boot. `abilities`, `factions`, `mobs` and `mob-kinds` are what it resolves combat
against. `quests`, `items` and `loot-tables` are what it resolves the M2 slice's
kill-and-loot loop against. `locale` is every string of every language the tree
supplies (ADR 0007), and `level-curve` is the curated per-level base HP/DPS/XP
that `mechanics/combat.md` section 7.1 established is absent from the source tree
and owned by this project.

`mob-kinds` holds the whole creature taxonomy — `MobKind`, `MobClass` and
`MobQuality` — in one table, each row carrying its **own** declared multipliers
and its links to the records above it. Nothing is composed: combat.md section 7.1
defers the order the chain resolves in, so the compiler ships the inputs rather
than baking an answer into Rust. A `mobs` row copies the multipliers of the
`MobKind` it names for the same reason it always copied `hp_mod`: so a shard log
can say which record a number came from.

### The item tree is never walked

`data/classic/items/` holds 36,980 documents. A zone pack needs the few its loot
tables, chargen options and quest rewards actually name, so items are loaded on
demand through `items/index.yaml` and a build's cost tracks the zone rather than
the catalogue. Loot tables are selected the same way, by reachability from the
zone's mobs — plus any a curated overlay authored, which ship whether or not
anything reaches them yet.

That selection applies to a **ruleset** tree only. A flat hand-authored dataset
such as `data-schemas/demo` is small and every document in it is deliberate, so
it is compiled whole.

## Cross-reference integrity

Every edge of the chain quest → mob → spawn table → placement → item → loot table
→ locale is resolved at compile time, and a failure names **both** the document
that made the reference and the id it could not resolve. Three outcomes are
deliberately not failures, and all three are counted in `build-report.json`:

| Outcome | Meaning |
|---|---|
| **External** | A zone-scoped id naming a different zone. A pack covers exactly one zone, so this is out of the pack rather than missing from it. The M2 zone has one: a quest whose finisher stands in the next zone along. |
| **Unmodelled** | An `item.interactive-objects.*` id. Those are chests, doors and steles; no row type describes them, because `mechanics/quests.md` section 7.1 defers the impact system that drives them. Fifty-two of the M2 zone's spawn tables place one. |
| **Locale gap** | A `loc_ref` key no locale document supplies. That is a loc pack nobody has extracted yet, not a broken edge. `--require-locale` promotes gaps to failures. |

A key that does not resolve is written into the row as **empty**, never carried
through. An unresolved key is a verbatim MY.GAMES resource path, and some of
those paths carry the source resource's class name in parentheses, so carrying
one through would put a MY.GAMES type name in a compiled artifact (ADR 0011).
The shard falls back to the canonical id, which this project owns.

## The `extra:` passthrough

The extractor keeps every unmapped XML attribute in an untyped `extra:` map
whose keys are verbatim MY.GAMES type and attribute names. `sarnaut-pack` drops
those maps. `--keep-extra` retains them as JSON-encoded strings and records
`"keep_extra": true` in the manifest; the shard then refuses to load the pack
unless `content.allow_extra` is set. Use it for local mapping work only
([ADR 0011](https://github.com/SarnautCore/docs/blob/main/adr/0011-clean-room-reimplementation-rule.md)).

## Overlays

ADR 0021's generated base plus curated overlay, with the merge semantics ADR 0029
pins down.

- **Layers** live at `<src>/overlays/<layer-id>/**` and are listed in
  `<src>/overlays/layers.yaml`. That file is the sole authority on which layers
  exist and in what order they apply — never filesystem order, never
  lexicographic id order, never a hint inside a layer.
- `--overlay <id>` selects a subset, repeatable. Selected layers keep
  `layers.yaml` order whatever order they are named in. With none given, every
  layer whose `apply_by_default` is true applies; a layer that exists to be
  switched on for one pack sets it false so that adding the layer beside an
  existing pack cannot move that pack's digest.
- **Merge, per document id.** An overlay document is a **patch over the merged
  result so far**, not a replacement. Scalars replace. Mappings merge key by key.
  Sequences replace wholesale — there is no stable identity to key list elements
  on, and a half-merged list is worse than an explicit rewrite. An overlay may
  also create a document no base layer carries.
- **Deletion.** `_op: replace` on a mapping discards the merged value beneath it;
  a top-level `_delete: [dotted.path, …]` runs after the merge; a top-level
  `_op: delete` removes the id from the pack.
- **`curation_note` is required** and non-empty on every overlay document. A
  patch with no stated reason is indistinguishable from an accident, so a missing
  note is a compile error naming the document and the layer. Notes are stripped
  from row bytes and aggregated into `build-report.json`.
- **Conflicts.** Two layers writing the same leaf of the same document fails the
  build, naming both layers and the path. `--allow-overlay-conflicts` lets the
  later layer win and lists every conflict in the report. Writing the value that
  is already there is not a write: every patch repeats its own `id`.

`manifest.source.overlays` records the layer **id** of each applied layer, never
the path it was read from: a manifest that embedded a local checkout path would
differ between a Windows rebuild and the Linux CI one, and packs are compared
byte for byte.

## `sarnaut-pack check`

`check` takes the same arguments as `build` minus `--out`. It compiles the tree
in memory, writes nothing, and exits non-zero on any dangling reference, missing
`curation_note`, duplicate id or overlay conflict. It is what the private `data`
repository's CI runs.

A clean run still prints the external, unmodelled and locale-gap counts. Those
are the numbers that quietly grow when an extractor loses coverage, and a check
that printed only failures would hide that.

## Not yet implemented

- **Composing the multiplier chain.** `mob-kinds` carries every record's own
  numbers and its prototype and class links; nothing multiplies them together.
  `mechanics/combat.md` section 7.1 owns that question.
- **Interactive objects.** Chests, doors and steles have no row type, so
  references into `item.interactive-objects.*` are counted rather than resolved.
- **Modern-ruleset trees.** `--ruleset modern` works as far as directory layout
  goes; nothing has been authored under it.
