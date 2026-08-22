//! `sarnaut-pack verify` is the same gate the shard applies at startup.

mod common;

use std::fs;

use sarnaut_pack::manifest::Manifest;
use sarnaut_pack::{compile, verify};

#[test]
fn a_freshly_built_pack_verifies() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    let report = compile::build(&common::options(source), &out).expect("build");

    let verified = verify::verify(&out).expect("verify");
    assert_eq!(verified.pack_id, report.pack_id());
    assert_eq!(verified.zone, common::ZONE);
}

#[test]
fn one_flipped_table_byte_is_reported_as_a_digest_mismatch() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");

    let table = out.join("tables/placements.sptbl");
    let mut bytes = fs::read(&table).expect("read table");
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    fs::write(&table, &bytes).expect("corrupt table");

    let error = verify::verify(&out).expect_err("verify should reject a corrupted table");
    let message = format!("{error:#}");
    assert!(
        message.contains("placements") && message.contains("digest mismatch"),
        "error does not name the table and the mismatch: {message}"
    );
}

#[test]
fn an_unsupported_schema_version_is_rejected() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");

    let path = out.join("manifest.json");
    let text = fs::read_to_string(&path).expect("read manifest");
    fs::write(
        &path,
        text.replace("\"schema_version\": 2", "\"schema_version\": 3"),
    )
    .expect("rewrite manifest");

    let error = verify::verify(&out).expect_err("verify should reject a future schema version");
    assert!(
        format!("{error:#}").contains("schema_version"),
        "error does not name schema_version: {error:#}"
    );
}

#[test]
fn a_table_the_manifest_does_not_list_is_rejected() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");

    fs::write(out.join("tables/stowaway.sptbl"), b"SPK1").expect("write stray table");

    let error = verify::verify(&out).expect_err("verify should reject an unlisted table");
    assert!(
        format!("{error:#}").contains("stowaway"),
        "error does not name the stray table: {error:#}"
    );
}

#[test]
fn a_rewritten_pack_id_is_rejected() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    let report = compile::build(&common::options(source), &out).expect("build");

    let path = out.join("manifest.json");
    let text = fs::read_to_string(&path).expect("read manifest");
    fs::write(&path, text.replace(report.pack_id(), &"0".repeat(64))).expect("rewrite manifest");

    let error = verify::verify(&out).expect_err("verify should reject a rewritten pack_id");
    assert!(
        format!("{error:#}").contains("pack_id mismatch"),
        "error does not name the pack_id mismatch: {error:#}"
    );
}

#[test]
fn a_foreign_bag_layout_catalog_is_rejected() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    let out = workspace.path().join("pack");
    compile::build(&common::options(source), &out).expect("build");

    let path = out.join("manifest.json");
    let text = fs::read_to_string(&path).expect("read manifest");
    let document: Manifest = serde_json::from_str(&text).expect("decode manifest");
    fs::write(
        &path,
        text.replace(
            &document.contracts.bag_layout_catalog_blake3,
            &"0".repeat(64),
        ),
    )
    .expect("rewrite manifest");

    let error = verify::verify(&out).expect_err("verify should reject a foreign catalog");
    assert!(
        format!("{error:#}").contains("bag-layout catalog contract mismatch"),
        "error does not name the contract mismatch: {error:#}"
    );
}
