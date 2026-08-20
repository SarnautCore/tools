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
```

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

## The `extra:` passthrough

The extractor keeps every unmapped XML attribute in an untyped `extra:` map
whose keys are verbatim MY.GAMES type and attribute names. `sarnaut-pack` drops
those maps. `--keep-extra` retains them as JSON-encoded strings and records
`"keep_extra": true` in the manifest; the shard then refuses to load the pack
unless `content.allow_extra` is set. Use it for local mapping work only
([ADR 0011](https://github.com/SarnautCore/docs/blob/main/adr/0011-clean-room-reimplementation-rule.md)).

## Not yet implemented

- **Overlay layering.** ADR 0029 specifies `data/overlays/<layer-id>` merge
  semantics. No layers are authored yet, so `manifest.source.overlays` is always
  empty and there is no `--overlay` flag.
- **The item, quest and route corpora.** This version compiles the placement and
  spawn-table documents the shard resolves at boot. Each further corpus gets a
  table and a row type of its own.
- **An authored player start point.** No extracted document carries one, so the
  `zone` row defaults `player_spawn` to the first live placement in canonical-id
  order. Pass `--player-spawn x,y,z[,yaw]` to pin it.
