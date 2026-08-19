use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestHeader {
    pub label: String,
    pub root: String,
    pub created: String,
    pub tool_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub size: u64,
    pub blake3: String,
    pub mtime: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestError {
    pub path: String,
    pub operation: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestRunReport {
    pub started: String,
    pub finished: String,
    pub discovered_files: u64,
    pub recorded_files: u64,
    pub cache_hits: u64,
    pub new_blobs: u64,
    pub existing_blobs: u64,
    pub bytes_read: u64,
    pub errors: Vec<ManifestError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum ManifestRecord {
    Header(ManifestHeader),
    File(ManifestEntry),
    RunReport(ManifestRunReport),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub header: ManifestHeader,
    pub files: Vec<ManifestEntry>,
    pub run_report: ManifestRunReport,
}

impl Manifest {
    pub fn read(path: &Path) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("failed to open manifest {}", path.display()))?;
        let mut header = None;
        let mut files = Vec::new();
        let mut run_report = None;

        for (index, line) in BufReader::new(file).lines().enumerate() {
            let line = line.with_context(|| {
                format!("failed to read line {} from {}", index + 1, path.display())
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let record: ManifestRecord = serde_json::from_str(&line).with_context(|| {
                format!("invalid JSON on line {} of {}", index + 1, path.display())
            })?;
            match record {
                ManifestRecord::Header(value) => {
                    if header.replace(value).is_some() {
                        bail!("manifest {} has more than one header", path.display());
                    }
                }
                ManifestRecord::File(value) => files.push(value),
                ManifestRecord::RunReport(value) => {
                    if run_report.replace(value).is_some() {
                        bail!("manifest {} has more than one run report", path.display());
                    }
                }
            }
        }

        Ok(Self {
            header: header.with_context(|| format!("manifest {} has no header", path.display()))?,
            files,
            run_report: run_report
                .with_context(|| format!("manifest {} has no run report", path.display()))?,
        })
    }

    pub fn write_atomic(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .with_context(|| format!("manifest path {} has no parent", path.display()))?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        let mut temporary = NamedTempFile::new_in(parent).with_context(|| {
            format!("failed to create a temporary file in {}", parent.display())
        })?;

        {
            let mut writer = BufWriter::new(temporary.as_file_mut());
            write_record(&mut writer, &ManifestRecord::Header(self.header.clone()))?;
            for entry in &self.files {
                write_record(&mut writer, &ManifestRecord::File(entry.clone()))?;
            }
            write_record(
                &mut writer,
                &ManifestRecord::RunReport(self.run_report.clone()),
            )?;
            writer.flush().context("failed to flush manifest")?;
        }
        temporary
            .as_file()
            .sync_all()
            .context("failed to sync manifest")?;
        temporary
            .persist(path)
            .map_err(|error| error.error)
            .with_context(|| format!("failed to replace manifest {}", path.display()))?;
        Ok(())
    }
}

fn write_record(writer: &mut impl Write, record: &ManifestRecord) -> Result<()> {
    serde_json::to_writer(&mut *writer, record).context("failed to serialize manifest record")?;
    writer.write_all(b"\n").context("failed to write manifest")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("test.jsonl");
        let manifest = Manifest {
            header: ManifestHeader {
                label: "test/source".into(),
                root: "C:/source".into(),
                created: "2026-08-20T10:00:00Z".into(),
                tool_version: "0.1.0".into(),
            },
            files: vec![ManifestEntry {
                path: "data/hello.txt".into(),
                size: 5,
                blake3: "a".repeat(64),
                mtime: 1_756_000_000_000_000_000,
            }],
            run_report: ManifestRunReport {
                started: "2026-08-20T10:00:00Z".into(),
                finished: "2026-08-20T10:00:01Z".into(),
                discovered_files: 1,
                recorded_files: 1,
                cache_hits: 0,
                new_blobs: 1,
                existing_blobs: 0,
                bytes_read: 5,
                errors: Vec::new(),
            },
        };

        manifest.write_atomic(&path).unwrap();
        assert_eq!(Manifest::read(&path).unwrap(), manifest);
    }
}
