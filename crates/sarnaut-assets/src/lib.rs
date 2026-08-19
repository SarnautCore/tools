//! Content-addressed storage for source game assets.

mod manifest;
mod store;

pub use manifest::{Manifest, ManifestEntry, ManifestError, ManifestHeader, ManifestRunReport};
pub use store::{
    DEFAULT_STORE, IngestOptions, IngestSummary, LookupMatch, ManifestStats, Stats, VerifyFailure,
    VerifyResult, ingest, lookup, stats, verify,
};
