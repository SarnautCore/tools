# SarnautCore tools

This repository contains command-line tools used to rebuild SarnautCore assets. Its first crate, `sarnaut-assets`, ingests source trees into a BLAKE3 content-addressed store.

## Build

Rust 1.85 or newer is required.

```powershell
cargo build --release
```

The executable is written to `target\release\sarnaut-assets.exe`.

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

This keeps provenance separate from blob storage. Changing converter code or settings produces a different key, while byte-identical outputs still deduplicate through their output BLAKE3 hash. This repository currently implements source ingestion only.

## Clean-room boundary

SarnautCore is a clean-room recreation kit. The asset store records hashes and provenance for locally supplied source trees; this repository does not distribute original Allods Online data. Project policy and architecture records live in the [SarnautCore documentation](https://github.com/SarnautCore/docs).

## License

Apache-2.0. See [LICENSE](LICENSE).
