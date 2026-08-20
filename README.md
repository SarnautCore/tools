# SarnautCore tools

This repository contains command-line tools used to rebuild SarnautCore assets and game data. `sarnaut-assets` ingests source trees into a BLAKE3 content-addressed store. `sarnaut-extract` converts supported XDB resources to schema-validated YAML. `sarnaut-pack` compiles that YAML into the runtime packs the shard loads; see [crates/sarnaut-pack/README.md](crates/sarnaut-pack/README.md).

## Build

Rust 1.85 or newer is required. `sarnaut-pack` also needs `protoc` on `PATH`, or
the `PROTOC` environment variable pointing at it.

```powershell
cargo build --release
```

The executables are written to `target\release`, including
`sarnaut-assets.exe`, `sarnaut-extract.exe`, `sarnaut-pack.exe`, and
`sarnaut-quest-census.exe`.

## Commands

The default store is `E:\SarnautCore\assets\store`. Set `SARNAUT_STORE` or pass `--store` to use another directory. A command-line `--store` value takes precedence over the environment variable.

```powershell
# Read a source tree and write its manifest and any new blobs.
sarnaut-assets ingest `
  --root E:\allods\servers-clean\1.1.02.0\game\data `
  --label classic-1.1-server

# Summarize blobs, manifests, and deduplication.
sarnaut-assets stats

# Re-hash every blob, or only blobs referenced by one manifest.
sarnaut-assets verify
sarnaut-assets verify --label classic-1.1-server

# Find every reference to a hash or logical path.
sarnaut-assets lookup 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
sarnaut-assets lookup textures/interface/example.dds
```

`ingest` never opens source files for writing. It walks the source tree, reads files in parallel, and copies new content into the store. A repeated run uses the previous entry when path, size, and nanosecond-resolution modification time match and its blob still exists. Read, metadata, and walk errors are printed as skipped files and saved in the run report. They do not discard other successful entries.

On Windows, paths are canonicalized before I/O so the standard library uses extended-length paths. Valid UTF-16 names are preserved. The rare unpaired UTF-16 code unit is written as `%uXXXX` in a logical path, and a literal percent sign is escaped as `%25`.

## Store layout

```text
store/
  blobs/
    5a/
      5a2c...full-64-character-blake3...
  manifests/
    classic-1.1-server.jsonl
    era/source-id.jsonl
  tmp/
```

Each blob path is `blobs/<first-two-hex>/<full-blake3>`. Ingestion writes a temporary file, flushes it, then installs it without replacing an existing target. Identical source files therefore share one blob, including files referenced by different manifests.

A slash in a label creates directories below `manifests`. Labels accept ASCII letters, digits, `.`, `_`, `-`, and `/`; `.` and `..` path segments are rejected.

## Source manifest schema

Manifests use JSON Lines. Each line has a `record` discriminator. Paths are relative to the source root and use forward slashes.

```json
{"record":"header","label":"classic-1.1-server","root":"E:\\allods\\servers-clean\\1.1.02.0\\game\\data","created":"2026-08-20T10:00:00.000Z","tool_version":"0.1.0"}
{"record":"file","path":"textures/interface/example.dds","size":32768,"blake3":"5a2c...64 hex characters...","mtime":1755684000000000000}
{"record":"run_report","started":"2026-08-20T10:00:00.000Z","finished":"2026-08-20T10:01:12.000Z","discovered_files":127000,"recorded_files":126999,"cache_hits":0,"new_blobs":120000,"existing_blobs":6999,"bytes_read":1181116006,"errors":[{"path":"locked.bin","operation":"ingest","message":"failed to open ..."}]}
```

`mtime` is signed nanoseconds from the Unix epoch. A new run replaces the manifest as one atomic file update. Deleted source paths disappear from the new manifest; skipped paths do not claim stale content.

`stats` defines deduplication ratio as total logical bytes divided by bytes in unique referenced hashes. Deduplication savings are total logical bytes minus unique referenced bytes.

## XDB extraction

`sarnaut-extract` reads XDB files without changing the source tree. Output ordering,
IDs, and YAML field order are deterministic. `--validate` checks every document
against `data-schemas/schemas` before writing it.

```powershell
sarnaut-extract items `
  --src E:\allods\servers-clean\1.1.02.0\game\data `
  --out E:\SarnautCore\data\classic `
  --validate

sarnaut-extract zone `
  --name InstLeague1 `
  --src E:\allods\servers-clean\1.1.02.0\game\data `
  --out E:\SarnautCore\data\classic `
  --validate
```

Add `--dry-run` to parse, map, hash, and validate without writing YAML.

### Gameplay subcommands

`mobkinds`, `loot`, and `locale` are scoped to one zone and pull in only what that
zone's mobs reach, rather than sweeping a whole directory.

```powershell
# MobKind prototype chains, plus the classes, qualities and faction closure.
sarnaut-extract mobkinds --name InstLeague1 `
  --src E:\allods\servers-clean\1.1.02.0\game\data `
  --out E:\SarnautCore\data\classic --validate

# Loot tables reachable from those mob kinds, plus classic/items/index.yaml.
sarnaut-extract loot --name InstLeague1 `
  --src E:\allods\servers-clean\1.1.02.0\game\data `
  --out E:\SarnautCore\data\classic --validate

# loc_ref strings, gap-filled from a second source root.
sarnaut-extract locale --name InstLeague1 --language ru `
  --src E:\allods\servers-clean\1.1.02.0\game\data `
  --supplemental-src E:\allods\servers\1.1\game\data `
  --out E:\SarnautCore\data\classic --validate
```

Three things about these are worth knowing before reading their output:

- A `MobKind` file lists only what differs from its `Header/Prototype`, so the
  emitted multipliers are the merged chain and `_source.prototype_chain` records
  which documents produced them. The base HP/DPS curve those multipliers scale is not
  in the source tree; every mob kind carries an `extra.level_curve_gap` note pointing
  at `docs/specs/mechanics/combat.md` §7.1, which owns the curated replacement.
- Under `--validate`, `loot` exits non-zero listing any item reference that resolves
  to no item document, and `locale` exits non-zero when the unresolved `loc_ref` rate
  exceeds `--max-unresolved-rate` (5% by default).
- The reference tree ships about 705 `.txt` payloads; `servers/1.1` ships tens of
  thousands. Each locale entry records the root it came from, and a key both roots
  carry with different text is reported as a mismatch instead of being resolved
  silently.

## Quest census

`sarnaut-quest-census` is the weekly M3 delivery metric. It applies the same
objective rule as `server/internal/quests/catalog.go`: count-item and count-kill
are servable, while any other objective kind makes the quest unservable. A
positive item or kill limit with no target is reported separately as an invalid
objective shape.

Run the day-zero InstLeague1 measurement from this repository:

```powershell
cargo run --release -p sarnaut-quest-census -- `
  --quests ..\data\classic\zones\inst-league1\quests `
  --baseline census\inst-league1-baseline.json `
  --json quest-census.json
```

The command prints one table row per quest and writes the same rows, summary,
reason counts, and objective-kind counts as JSON. With `--baseline`, it exits
non-zero if either the servable count or the total quest count falls below the
committed measurement.

The JSON always contains this ADR 0036 extension point:

```json
"script_nodes": {
  "implemented": null,
  "inert_and_counted": null,
  "refused": null
}
```

Pass `--script-node-counts counts.json` to fill those three fields without
changing the report shape. The input file is the object shown above with integer
values in place of `null`.

## Future converted outputs

Converters will write their results into the same blob namespace. Their cache identity is the tuple below:

```text
(source_blake3, converter.id, converter.version, settings_blake3)
```

`settings_blake3` will hash a canonical serialization of all output-affecting settings. A derivation manifest can then map that key to one or more output records:

```json
{
  "record": "output",
  "key": {
    "source_blake3": "...",
    "converter": {"id": "ao-godot-converter", "version": "1.0.0"},
    "settings_blake3": "..."
  },
  "path": "models/example.glb",
  "size": 48192,
  "blake3": "..."
}
```

This keeps provenance separate from blob storage. Changing converter code or settings produces a different key, while byte-identical outputs still deduplicate through their output BLAKE3 hash.

## Clean-room boundary

SarnautCore is a clean-room recreation kit. The asset store records hashes and provenance for locally supplied source trees; this repository does not distribute original Allods Online data. Project policy and architecture records live in the [SarnautCore documentation](https://github.com/SarnautCore/docs).

## License

Apache-2.0. See [LICENSE](LICENSE).
