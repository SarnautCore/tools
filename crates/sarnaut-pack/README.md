# sarnaut-pack

Compiles authored SarnautCore YAML into the runtime pack format of
[ADR 0029](https://github.com/SarnautCore/docs/blob/main/adr/0029-runtime-pack-format.md).
The shard reads packs and never parses YAML ([ADR 0006](https://github.com/SarnautCore/docs/blob/main/adr/0006-yaml-source-compiled-runtime-data.md)).

A pack is a directory, not an archive:

```
<pack>/
  manifest.json
  tables/zone.sptbl
  tables/placements.sptbl
  tables/spawn-tables.sptbl
  tables/abilities.sptbl
  tables/factions.sptbl
  tables/mobs.sptbl
```

Every table is written even when it holds no rows, so a reader can insist on the
full set rather than treating "absent" and "empty" as one case.

`pack_id` is a BLAKE3-256 digest over the table bytes alone. The manifest is not
an input, so a builder-version bump that produces identical tables leaves the id
alone — which is the property the ADR 0027 handshake comparison needs.

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

# Compile the golden fixture from the hand-authored demo dataset.
sarnaut-pack build --fixture --src ..\data-schemas\demo --out ..\server\testdata\packs\demo

# Layer an overlay of extra documents over it.
sarnaut-pack build --fixture --src ..\data-schemas\demo `
  --overlay ..\data-schemas\demo\overlays\m2-combat-extended `
  --out ..\server\testdata\packs\demo-extended

# Check a pack's manifest digest against its table bytes.
sarnaut-pack verify E:\SarnautCore\data\packs\classic\inst-league1
```

Packs built from `data` are private-path artifacts: `data/.gitignore` carries
`packs/`, and no pack but the fixture may be committed to a public repository.

## Determinism

Two builds over one source tree produce byte-identical output, `pack_id`
included. Rows are sorted by canonical id, protobuf fields are emitted in
ascending field number, and no timestamp, path or hostname reaches the table
bytes. `--fixture` additionally pins `source.commit` to zeros, because a fixture
that recorded whichever commit was checked out would stop matching the copy
vendored in `server`.

## What reaches a pack

`zone`, `placements` and `spawn-tables` are what the shard resolves at boot.
`abilities`, `factions` and `mobs` are what it resolves combat against: an
ability's range, damage terms, cast time and cooldown; a faction's attackable
flag and its directed stances; a mob's level range, walk speed, aggro and leash
radii, and the `hp_mod` resolved from the `MobKind` it names. A placement also
carries the respawn window for whatever fills it, because respawn is a property
of the spawn slot rather than of the creature.

Only the named `MobKind`'s own `hp_mod` is read. The prototype chain and the
quality multipliers are deferred by `mechanics/combat.md` section 7.1, and
walking half of a chain would produce a number nothing can explain.

Cross-references are checked at compile time: a placement must name a mob or a
spawn table this pack carries, and a mob must name a faction and abilities it
carries.

## The `extra:` passthrough

The extractor keeps every unmapped XML attribute in an untyped `extra:` map
whose keys are verbatim MY.GAMES type and attribute names. `sarnaut-pack` drops
those maps. `--keep-extra` retains them as JSON-encoded strings and records
`"keep_extra": true` in the manifest; the shard then refuses to load the pack
unless `content.allow_extra` is set. Use it for local mapping work only
([ADR 0011](https://github.com/SarnautCore/docs/blob/main/adr/0011-clean-room-reimplementation-rule.md)).

## Overlays

`--overlay <dir>` layers a flat directory of extra documents over `--src`, and
is repeatable. An overlay **adds** documents; it does not patch them, so a
duplicate id is a build error rather than a silent last-one-wins that would make
the output depend on argument order.

`manifest.source.overlays` records the directory **name** of each layer, never
the path it was read from: a manifest that embedded a local checkout path would
differ between a Windows rebuild and the Linux CI one, and the vendored fixture
pack is compared byte for byte.

This is not yet the full `data/overlays/<layer-id>` merge semantics of ADR 0029,
which lets a layer override a base document. Nothing authored needs that yet.

## Not yet implemented

- **Overlay overrides.** See above: layering adds, it does not replace.
- **The item, quest and route corpora.** This version compiles the documents the
  shard resolves at boot: placements, spawn tables, and the abilities, factions
  and mobs that `mechanics/combat.md` reads. Each further corpus gets a table and
  a row type of its own.
- **An authored player start point.** No extracted document carries one, so the
  `zone` row defaults `player_spawn` to the first live placement in canonical-id
  order. Pass `--player-spawn x,y,z[,yaw]` to pin it.
