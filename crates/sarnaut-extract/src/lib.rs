//! Deterministic extraction of Allods XDB resources into authored YAML.

mod encoding;
mod faction;
mod item;
mod locale;
mod loot;
mod mobkind;
mod model;
mod output;
mod reference;
mod scan;
#[cfg(test)]
mod testing;
mod validation;
mod xdb;
mod zone;

use std::path::PathBuf;

pub use item::extract_items;
pub use locale::extract_locale;
pub use loot::extract_loot;
pub use mobkind::extract_mobkinds;
pub use model::{
    ExtractionOptions, ItemSummary, LocaleOptions, LocaleSummary, LootSummary, MobKindSummary,
    ZoneSummary,
};
pub use validation::{SchemaKind, SchemaValidator};
pub use zone::extract_zone;

/// Locate the schema directory beside the SarnautCore data and tools repositories.
pub fn discover_schema_dir(output: &std::path::Path) -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("SARNAUT_SCHEMA_DIR") {
        let path = PathBuf::from(value);
        if path.is_dir() {
            return Some(path);
        }
    }

    let mut candidates = Vec::new();
    if let Ok(current) = std::env::current_dir() {
        candidates.push(current.join("../data-schemas/schemas"));
        candidates.push(current.join("data-schemas/schemas"));
    }
    for ancestor in output.ancestors() {
        if let Some(parent) = ancestor.parent() {
            candidates.push(parent.join("data-schemas/schemas"));
        }
    }
    candidates.into_iter().find(|path| path.is_dir())
}
