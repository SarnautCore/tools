use std::fs;

use sarnaut_assets::{IngestOptions, Manifest, ingest, lookup, stats, verify};

#[test]
fn second_ingest_is_idempotent_and_content_is_deduplicated() {
    let fixture = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    fs::create_dir_all(fixture.path().join("nested")).unwrap();
    fs::write(fixture.path().join("one.txt"), b"same bytes").unwrap();
    fs::write(fixture.path().join("nested/two.txt"), b"same bytes").unwrap();
    fs::write(fixture.path().join("100%.txt"), b"same bytes").unwrap();
    fs::write(fixture.path().join("three.txt"), b"different").unwrap();

    let options = IngestOptions {
        root: fixture.path().to_path_buf(),
        label: "test-fixture-1.0".into(),
        store: store.path().to_path_buf(),
        show_progress: false,
    };
    let first = ingest(&options).unwrap();
    assert_eq!(first.report.recorded_files, 4);
    assert_eq!(first.report.new_blobs, 2);
    assert_eq!(first.report.existing_blobs, 2);
    assert_eq!(first.report.cache_hits, 0);
    assert!(first.report.errors.is_empty());

    let second = ingest(&options).unwrap();
    assert_eq!(second.report.recorded_files, 4);
    assert_eq!(second.report.new_blobs, 0);
    assert_eq!(second.report.existing_blobs, 0);
    assert_eq!(second.report.cache_hits, 4);
    assert_eq!(second.report.bytes_read, 0);
    assert!(second.report.errors.is_empty());

    let manifest = Manifest::read(&second.manifest_path).unwrap();
    assert_eq!(manifest.files.len(), 4);
    let report = stats(store.path()).unwrap();
    assert_eq!(report.blob_count, 2);
    assert_eq!(report.unique_referenced_blobs, 2);
    assert_eq!(report.dedup_saved_bytes, 20);

    let found = lookup(store.path(), "nested/two.txt".as_ref()).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].label, "test-fixture-1.0");

    let percent_by_source_spelling = lookup(store.path(), "100%.txt".as_ref()).unwrap();
    let percent_by_logical_path = lookup(store.path(), "100%25.txt".as_ref()).unwrap();
    assert_eq!(percent_by_source_spelling, percent_by_logical_path);
    assert_eq!(percent_by_logical_path.len(), 1);

    let verified = verify(store.path(), Some("test-fixture-1.0"), false).unwrap();
    assert_eq!(verified.checked, 2);
    assert!(verified.failures.is_empty());
}
