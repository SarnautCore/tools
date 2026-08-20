//! Fixture helpers shared by the extractor unit tests.
//!
//! Every test builds its own synthetic XDB tree in a temporary directory. Nothing
//! under this crate's tests reads the real reference trees, so the suite runs on a
//! machine that has never seen them.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::model::ExtractionOptions;

pub(crate) fn write(path: &Path, contents: &str) {
    write_bytes(path, contents.as_bytes());
}

pub(crate) fn write_bytes(path: &Path, contents: &[u8]) {
    fs::create_dir_all(path.parent().expect("fixture path has a parent")).expect("create fixture");
    fs::write(path, contents).expect("write fixture");
}

/// A source tree, an output tree, and options wired to both. The temporary
/// directories are returned so the caller keeps them alive for the whole test.
pub(crate) fn fixture_options() -> (TempDir, TempDir, ExtractionOptions) {
    let source = tempfile::tempdir().expect("source tempdir");
    let output = tempfile::tempdir().expect("output tempdir");
    let options = ExtractionOptions {
        src: source.path().to_path_buf(),
        out: output.path().to_path_buf(),
        dry_run: false,
        schema_dir: None,
    };
    (source, output, options)
}
