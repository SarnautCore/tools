use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use tempfile::NamedTempFile;
use walkdir::WalkDir;

use crate::manifest::{Manifest, ManifestEntry, ManifestError, ManifestHeader, ManifestRunReport};

pub const DEFAULT_STORE: &str = r"E:\SarnautCore\assets\store";
const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct IngestOptions {
    pub root: PathBuf,
    pub label: String,
    pub store: PathBuf,
    pub show_progress: bool,
}

#[derive(Debug, Clone)]
pub struct IngestSummary {
    pub manifest_path: PathBuf,
    pub report: ManifestRunReport,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManifestStats {
    pub label: String,
    pub file_count: u64,
    pub logical_bytes: u64,
    pub error_count: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    pub blob_count: u64,
    pub blob_bytes: u64,
    pub manifests: Vec<ManifestStats>,
    pub referenced_files: u64,
    pub referenced_bytes: u64,
    pub unique_referenced_blobs: u64,
    pub unique_referenced_bytes: u64,
    pub dedup_saved_bytes: u64,
    pub dedup_ratio: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyFailure {
    pub blake3: String,
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyResult {
    pub checked: u64,
    pub checked_bytes: u64,
    pub failures: Vec<VerifyFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupMatch {
    pub label: String,
    pub path: String,
    pub blake3: String,
    pub size: u64,
}

#[derive(Debug, Clone)]
struct Candidate {
    source_path: PathBuf,
    logical_path: String,
    size: u64,
    mtime: i64,
}

enum ProcessResult {
    Cached(ManifestEntry),
    Stored {
        entry: ManifestEntry,
        new_blob: bool,
        bytes_read: u64,
    },
    Error(ManifestError),
}

pub fn ingest(options: &IngestOptions) -> Result<IngestSummary> {
    validate_label(&options.label)?;
    let started = now();
    let source_root = canonical_directory(&options.root, false)?;
    let store_root = canonical_directory(&options.store, true)?;
    initialize_store(&store_root)?;
    let manifest_path = manifest_path(&store_root, &options.label);

    let cache: HashMap<String, ManifestEntry> = if manifest_path.exists() {
        Manifest::read(&manifest_path)?
            .files
            .into_iter()
            .map(|entry| (entry.path.clone(), entry))
            .collect()
    } else {
        HashMap::new()
    };

    let (candidates, mut errors) = collect_candidates(&source_root);
    let discovered_files = candidates.len() as u64;
    let progress = progress_bar(discovered_files, options.show_progress, "ingesting");
    let cache_hits = Arc::new(AtomicU64::new(0));

    let results: Vec<ProcessResult> = candidates
        .par_iter()
        .map(|candidate| {
            let result = if let Some(entry) = cache.get(&candidate.logical_path) {
                let blob = blob_path(&store_root, &entry.blake3);
                if entry.size == candidate.size && entry.mtime == candidate.mtime && blob.is_file()
                {
                    cache_hits.fetch_add(1, Ordering::Relaxed);
                    ProcessResult::Cached(entry.clone())
                } else {
                    ingest_candidate(candidate, &store_root)
                }
            } else {
                ingest_candidate(candidate, &store_root)
            };
            progress.inc(1);
            result
        })
        .collect();
    progress.finish_and_clear();

    let mut files = Vec::with_capacity(results.len());
    let mut new_blobs = 0_u64;
    let mut existing_blobs = 0_u64;
    let mut bytes_read = 0_u64;
    for result in results {
        match result {
            ProcessResult::Cached(entry) => files.push(entry),
            ProcessResult::Stored {
                entry,
                new_blob,
                bytes_read: read,
            } => {
                files.push(entry);
                bytes_read = bytes_read.saturating_add(read);
                if new_blob {
                    new_blobs += 1;
                } else {
                    existing_blobs += 1;
                }
            }
            ProcessResult::Error(error) => errors.push(error),
        }
    }
    files.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    errors.sort_unstable_by(|left, right| left.path.cmp(&right.path));

    let report = ManifestRunReport {
        started,
        finished: now(),
        discovered_files,
        recorded_files: files.len() as u64,
        cache_hits: cache_hits.load(Ordering::Relaxed),
        new_blobs,
        existing_blobs,
        bytes_read,
        errors,
    };
    let manifest = Manifest {
        header: ManifestHeader {
            label: options.label.clone(),
            root: display_path(&source_root),
            created: report.started.clone(),
            tool_version: TOOL_VERSION.into(),
        },
        files,
        run_report: report.clone(),
    };
    manifest.write_atomic(&manifest_path)?;

    Ok(IngestSummary {
        manifest_path,
        report,
    })
}

pub fn stats(store: &Path) -> Result<Stats> {
    let store_root = canonical_directory(store, false)?;
    let mut blob_count = 0_u64;
    let mut blob_bytes = 0_u64;
    let blobs_root = store_root.join("blobs");
    if blobs_root.exists() {
        for item in WalkDir::new(&blobs_root).follow_links(false) {
            let item = item.with_context(|| format!("failed to walk {}", blobs_root.display()))?;
            if item.file_type().is_file() {
                blob_count += 1;
                blob_bytes = blob_bytes.saturating_add(
                    item.metadata()
                        .with_context(|| format!("failed to stat {}", item.path().display()))?
                        .len(),
                );
            }
        }
    }

    let mut manifests = Vec::new();
    let mut referenced_files = 0_u64;
    let mut referenced_bytes = 0_u64;
    let mut unique = HashMap::<String, u64>::new();
    for path in manifest_files(&store_root)? {
        let manifest = Manifest::read(&path)?;
        let logical_bytes = manifest
            .files
            .iter()
            .fold(0_u64, |sum, entry| sum.saturating_add(entry.size));
        referenced_files += manifest.files.len() as u64;
        referenced_bytes = referenced_bytes.saturating_add(logical_bytes);
        for entry in &manifest.files {
            unique.entry(entry.blake3.clone()).or_insert(entry.size);
        }
        manifests.push(ManifestStats {
            label: manifest.header.label,
            file_count: manifest.files.len() as u64,
            logical_bytes,
            error_count: manifest.run_report.errors.len() as u64,
        });
    }
    manifests.sort_unstable_by(|left, right| left.label.cmp(&right.label));
    let unique_referenced_bytes = unique
        .values()
        .fold(0_u64, |sum, size| sum.saturating_add(*size));
    let dedup_saved_bytes = referenced_bytes.saturating_sub(unique_referenced_bytes);
    let dedup_ratio = if unique_referenced_bytes == 0 {
        1.0
    } else {
        referenced_bytes as f64 / unique_referenced_bytes as f64
    };

    Ok(Stats {
        blob_count,
        blob_bytes,
        manifests,
        referenced_files,
        referenced_bytes,
        unique_referenced_blobs: unique.len() as u64,
        unique_referenced_bytes,
        dedup_saved_bytes,
        dedup_ratio,
    })
}

pub fn verify(store: &Path, label: Option<&str>, show_progress: bool) -> Result<VerifyResult> {
    let store_root = canonical_directory(store, false)?;
    let hashes: Vec<String> = if let Some(label) = label {
        validate_label(label)?;
        let manifest = Manifest::read(&manifest_path(&store_root, label))?;
        manifest
            .files
            .into_iter()
            .map(|entry| entry.blake3)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    } else {
        blob_hashes(&store_root)?
    };
    let progress = progress_bar(hashes.len() as u64, show_progress, "verifying");
    let results: Vec<(u64, Option<VerifyFailure>)> = hashes
        .par_iter()
        .map(|expected| {
            let path = blob_path(&store_root, expected);
            let result = match hash_file(&path) {
                Ok((actual, bytes)) if actual == *expected => (bytes, None),
                Ok((actual, bytes)) => (
                    bytes,
                    Some(VerifyFailure {
                        blake3: expected.clone(),
                        path,
                        error: format!("hash mismatch: got {actual}"),
                    }),
                ),
                Err(error) => (
                    0,
                    Some(VerifyFailure {
                        blake3: expected.clone(),
                        path,
                        error: format!("{error:#}"),
                    }),
                ),
            };
            progress.inc(1);
            result
        })
        .collect();
    progress.finish_and_clear();

    let checked_bytes = results
        .iter()
        .fold(0_u64, |sum, (bytes, _)| sum.saturating_add(*bytes));
    let failures = results
        .into_iter()
        .filter_map(|(_, failure)| failure)
        .collect();
    Ok(VerifyResult {
        checked: hashes.len() as u64,
        checked_bytes,
        failures,
    })
}

pub fn lookup(store: &Path, query: &OsStr) -> Result<Vec<LookupMatch>> {
    let store_root = canonical_directory(store, false)?;
    let query_text = query.to_string_lossy();
    let hash_query = is_blake3(&query_text).then(|| query_text.to_ascii_lowercase());
    let mut path_queries = vec![normalize_lookup_path(query)];
    let raw_path_query = query_text.replace('\\', "/");
    if !path_queries.contains(&raw_path_query) {
        path_queries.push(raw_path_query);
    }
    let mut matches = Vec::new();

    for path in manifest_files(&store_root)? {
        let manifest = Manifest::read(&path)?;
        for entry in manifest.files {
            let matched = match &hash_query {
                Some(hash) => entry.blake3 == *hash,
                None => path_queries.contains(&entry.path),
            };
            if matched {
                matches.push(LookupMatch {
                    label: manifest.header.label.clone(),
                    path: entry.path,
                    blake3: entry.blake3,
                    size: entry.size,
                });
            }
        }
    }
    matches.sort_unstable_by(|left, right| {
        left.label
            .cmp(&right.label)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(matches)
}

fn initialize_store(store_root: &Path) -> Result<()> {
    for directory in ["blobs", "manifests", "tmp"] {
        fs::create_dir_all(store_root.join(directory)).with_context(|| {
            format!(
                "failed to create store directory {}",
                store_root.join(directory).display()
            )
        })?;
    }
    Ok(())
}

fn collect_candidates(root: &Path) -> (Vec<Candidate>, Vec<ManifestError>) {
    let mut candidates = Vec::new();
    let mut errors = Vec::new();
    for item in WalkDir::new(root).follow_links(false) {
        let item = match item {
            Ok(item) => item,
            Err(error) => {
                errors.push(ManifestError {
                    path: error
                        .path()
                        .map(display_path)
                        .unwrap_or_else(|| display_path(root)),
                    operation: "walk".into(),
                    message: error.to_string(),
                });
                continue;
            }
        };
        if !item.file_type().is_file() {
            continue;
        }
        let path = item.into_path();
        let logical_path = match path.strip_prefix(root) {
            Ok(relative) => logical_path(relative),
            Err(error) => {
                errors.push(ManifestError {
                    path: display_path(&path),
                    operation: "relative_path".into(),
                    message: error.to_string(),
                });
                continue;
            }
        };
        match fs::metadata(&path) {
            Ok(metadata) => match metadata.modified().and_then(system_time_nanos) {
                Ok(mtime) => candidates.push(Candidate {
                    source_path: path,
                    logical_path,
                    size: metadata.len(),
                    mtime,
                }),
                Err(error) => errors.push(ManifestError {
                    path: logical_path,
                    operation: "mtime".into(),
                    message: error.to_string(),
                }),
            },
            Err(error) => errors.push(ManifestError {
                path: logical_path,
                operation: "metadata".into(),
                message: error.to_string(),
            }),
        }
    }
    (candidates, errors)
}

fn ingest_candidate(candidate: &Candidate, store_root: &Path) -> ProcessResult {
    match ingest_candidate_inner(candidate, store_root) {
        Ok(result) => result,
        Err(error) => ProcessResult::Error(ManifestError {
            path: candidate.logical_path.clone(),
            operation: "ingest".into(),
            message: format!("{error:#}"),
        }),
    }
}

fn ingest_candidate_inner(candidate: &Candidate, store_root: &Path) -> Result<ProcessResult> {
    let source = File::open(&candidate.source_path).with_context(|| {
        format!(
            "failed to open {} read-only",
            candidate.source_path.display()
        )
    })?;
    let mut reader = BufReader::with_capacity(1024 * 1024, source);
    let mut temporary = NamedTempFile::new_in(store_root.join("tmp"))
        .context("failed to create a temporary blob")?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut bytes_read = 0_u64;
    loop {
        let count = reader
            .read(&mut buffer)
            .with_context(|| format!("failed to read {}", candidate.source_path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        temporary
            .write_all(&buffer[..count])
            .context("failed to write temporary blob")?;
        bytes_read = bytes_read.saturating_add(count as u64);
    }

    let final_metadata = fs::metadata(&candidate.source_path)
        .with_context(|| format!("failed to re-stat {}", candidate.source_path.display()))?;
    let final_mtime = final_metadata.modified().and_then(system_time_nanos)?;
    if bytes_read != candidate.size
        || final_metadata.len() != candidate.size
        || final_mtime != candidate.mtime
    {
        bail!("source changed while it was being read");
    }

    temporary
        .as_file_mut()
        .flush()
        .context("failed to flush temporary blob")?;
    temporary
        .as_file()
        .sync_all()
        .context("failed to sync temporary blob")?;
    let hash = hasher.finalize().to_hex().to_string();
    let target = blob_path(store_root, &hash);
    let parent = target.parent().context("blob path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let new_blob = if target.exists() {
        false
    } else {
        match temporary.persist_noclobber(&target) {
            Ok(_) => true,
            Err(_error) if target.exists() => false,
            Err(error) => {
                return Err(error.error)
                    .with_context(|| format!("failed to persist blob {}", target.display()));
            }
        }
    };

    Ok(ProcessResult::Stored {
        entry: ManifestEntry {
            path: candidate.logical_path.clone(),
            size: candidate.size,
            blake3: hash,
            mtime: candidate.mtime,
        },
        new_blob,
        bytes_read,
    })
}

fn manifest_files(store_root: &Path) -> Result<Vec<PathBuf>> {
    let manifests_root = store_root.join("manifests");
    if !manifests_root.exists() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for item in WalkDir::new(&manifests_root).follow_links(false) {
        let item = item.with_context(|| format!("failed to walk {}", manifests_root.display()))?;
        if item.file_type().is_file()
            && item
                .path()
                .extension()
                .is_some_and(|value| value == "jsonl")
        {
            paths.push(item.into_path());
        }
    }
    paths.sort_unstable();
    Ok(paths)
}

fn blob_hashes(store_root: &Path) -> Result<Vec<String>> {
    let blobs_root = store_root.join("blobs");
    if !blobs_root.exists() {
        return Ok(Vec::new());
    }
    let mut hashes = Vec::new();
    for item in WalkDir::new(&blobs_root).follow_links(false) {
        let item = item.with_context(|| format!("failed to walk {}", blobs_root.display()))?;
        if item.file_type().is_file() {
            let name = item.file_name().to_string_lossy();
            if is_blake3(&name) {
                hashes.push(name.to_ascii_lowercase());
            }
        }
    }
    hashes.sort_unstable();
    hashes.dedup();
    Ok(hashes)
}

fn hash_file(path: &Path) -> Result<(String, u64)> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut bytes = 0_u64;
    loop {
        let count = reader
            .read(&mut buffer)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        bytes = bytes.saturating_add(count as u64);
    }
    Ok((hasher.finalize().to_hex().to_string(), bytes))
}

fn canonical_directory(path: &Path, create: bool) -> Result<PathBuf> {
    if create {
        fs::create_dir_all(path)
            .with_context(|| format!("failed to create directory {}", path.display()))?;
    }
    let metadata = fs::metadata(path)
        .with_context(|| format!("directory {} is not accessible", path.display()))?;
    if !metadata.is_dir() {
        bail!("{} is not a directory", path.display());
    }
    fs::canonicalize(path)
        .with_context(|| format!("failed to resolve directory {}", path.display()))
}

fn manifest_path(store_root: &Path, label: &str) -> PathBuf {
    let mut path = store_root.join("manifests");
    for part in label.split('/') {
        path.push(part);
    }
    path.as_mut_os_string().push(".jsonl");
    path
}

fn blob_path(store_root: &Path, hash: &str) -> PathBuf {
    store_root.join("blobs").join(&hash[..2]).join(hash)
}

fn validate_label(label: &str) -> Result<()> {
    if label.is_empty() {
        bail!("label cannot be empty");
    }
    for part in label.split('/') {
        if part.is_empty()
            || part == "."
            || part == ".."
            || !part
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
        {
            bail!(
                "invalid label {label:?}; use slash-separated ASCII letters, digits, '.', '_', or '-'"
            );
        }
    }
    Ok(())
}

fn is_blake3(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn system_time_nanos(time: SystemTime) -> std::io::Result<i64> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_nanos()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "mtime is out of range")
        }),
        Err(error) => {
            let nanos = i64::try_from(error.duration().as_nanos()).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "mtime is out of range")
            })?;
            Ok(-nanos)
        }
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn progress_bar(length: u64, visible: bool, message: &'static str) -> ProgressBar {
    if !visible {
        return ProgressBar::hidden();
    }
    let progress = ProgressBar::new(length);
    progress.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} {msg} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} {per_sec} {eta}",
        )
        .expect("the progress template is valid")
        .progress_chars("=>-"),
    );
    progress.set_message(message);
    progress
}

fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\UNC\")
        .map(|value| format!(r"\\{value}"))
        .or_else(|| text.strip_prefix(r"\\?\").map(ToOwned::to_owned))
        .unwrap_or_else(|| text.into_owned())
}

fn logical_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(component_text(value)),
            Component::CurDir => None,
            _ => Some(component.as_os_str().to_string_lossy().into_owned()),
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(windows)]
fn component_text(value: &OsStr) -> String {
    use std::char::decode_utf16;
    use std::os::windows::ffi::OsStrExt;

    let mut result = String::new();
    for decoded in decode_utf16(value.encode_wide()) {
        match decoded {
            Ok('%') => result.push_str("%25"),
            Ok(character) => result.push(character),
            Err(error) => result.push_str(&format!("%u{:04X}", error.unpaired_surrogate())),
        }
    }
    result
}

#[cfg(not(windows))]
fn component_text(value: &OsStr) -> String {
    value.to_string_lossy().into_owned()
}

fn normalize_lookup_path(value: &OsStr) -> String {
    let path = Path::new(value);
    logical_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_file_contents() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hello.txt");
        fs::write(&path, b"hello world").unwrap();

        let (actual, bytes) = hash_file(&path).unwrap();
        assert_eq!(actual, blake3::hash(b"hello world").to_hex().as_str());
        assert_eq!(bytes, 11);
    }

    #[test]
    fn rejects_traversal_in_labels() {
        assert!(validate_label("../outside").is_err());
        assert!(validate_label("good/source-1.0").is_ok());
    }

    #[test]
    fn manifest_filename_preserves_dots_in_label() {
        assert_eq!(
            manifest_path(Path::new("store"), "classic-1.1-server"),
            Path::new("store")
                .join("manifests")
                .join("classic-1.1-server.jsonl")
        );
    }
}
