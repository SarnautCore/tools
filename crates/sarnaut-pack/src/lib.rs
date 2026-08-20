//! Compiles authored SarnautCore YAML into the runtime pack format of ADR 0029.
//!
//! A pack is a directory of `.sptbl` tables plus a `manifest.json` whose
//! `pack_id` is a pure function of the table bytes. The shard reads packs and
//! never parses YAML (ADR 0006).

pub mod compile;
pub mod manifest;
pub mod proto;
pub mod source;
pub mod table;
pub mod verify;
