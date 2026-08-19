use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use sarnaut_assets::{DEFAULT_STORE, IngestOptions, ingest, lookup, stats, verify};

#[derive(Debug, Parser)]
#[command(name = "sarnaut-assets", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Ingest a source tree into the content-addressed store.
    Ingest(IngestArgs),
    /// Report store size and deduplication statistics.
    Stats(StoreArgs),
    /// Re-hash stored blobs and report corruption.
    Verify(VerifyArgs),
    /// Find manifest entries by BLAKE3 hash or logical path.
    Lookup(LookupArgs),
}

#[derive(Debug, Args)]
struct StoreArgs {
    /// Store directory. SARNAUT_STORE overrides the default.
    #[arg(long, env = "SARNAUT_STORE", default_value = DEFAULT_STORE)]
    store: PathBuf,
}

#[derive(Debug, Args)]
struct IngestArgs {
    /// Source directory to read.
    #[arg(long)]
    root: PathBuf,
    /// Stable source identifier, optionally separated with slashes.
    #[arg(long)]
    label: String,
    /// Store directory. SARNAUT_STORE overrides the default.
    #[arg(long, env = "SARNAUT_STORE", default_value = DEFAULT_STORE)]
    store: PathBuf,
}

#[derive(Debug, Args)]
struct VerifyArgs {
    /// Store directory. SARNAUT_STORE overrides the default.
    #[arg(long, env = "SARNAUT_STORE", default_value = DEFAULT_STORE)]
    store: PathBuf,
    /// Verify only blobs referenced by this manifest label.
    #[arg(long)]
    label: Option<String>,
}

#[derive(Debug, Args)]
struct LookupArgs {
    /// Store directory. SARNAUT_STORE overrides the default.
    #[arg(long, env = "SARNAUT_STORE", default_value = DEFAULT_STORE)]
    store: PathBuf,
    /// A 64-character BLAKE3 hash or a forward-slash logical path.
    query: OsString,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Command::Ingest(args) => {
            let started = Instant::now();
            let summary = ingest(&IngestOptions {
                root: args.root,
                label: args.label,
                store: args.store,
                show_progress: true,
            })?;
            println!("manifest: {}", summary.manifest_path.display());
            println!("duration: {:.3}s", started.elapsed().as_secs_f64());
            println!("discovered files: {}", summary.report.discovered_files);
            println!("recorded files: {}", summary.report.recorded_files);
            println!("cache hits: {}", summary.report.cache_hits);
            println!("new blobs: {}", summary.report.new_blobs);
            println!("existing blobs: {}", summary.report.existing_blobs);
            println!("bytes read: {}", summary.report.bytes_read);
            println!("errors: {}", summary.report.errors.len());
            for error in &summary.report.errors {
                eprintln!(
                    "skip: {} [{}]: {}",
                    error.path, error.operation, error.message
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Stats(args) => {
            let report = stats(&args.store)?;
            println!("blobs: {}", report.blob_count);
            println!("blob bytes: {}", report.blob_bytes);
            for manifest in report.manifests {
                println!(
                    "manifest {}: {} files, {} bytes, {} errors",
                    manifest.label,
                    manifest.file_count,
                    manifest.logical_bytes,
                    manifest.error_count
                );
            }
            println!("manifest references: {}", report.referenced_files);
            println!("logical bytes: {}", report.referenced_bytes);
            println!(
                "unique referenced blobs: {}",
                report.unique_referenced_blobs
            );
            println!(
                "unique referenced bytes: {}",
                report.unique_referenced_bytes
            );
            println!("dedup savings: {} bytes", report.dedup_saved_bytes);
            println!("dedup ratio: {:.4}x", report.dedup_ratio);
            Ok(ExitCode::SUCCESS)
        }
        Command::Verify(args) => {
            let result = verify(&args.store, args.label.as_deref(), true)?;
            println!("checked blobs: {}", result.checked);
            println!("checked bytes: {}", result.checked_bytes);
            println!("corrupt or unreadable: {}", result.failures.len());
            for failure in &result.failures {
                eprintln!(
                    "bad blob {} at {}: {}",
                    failure.blake3,
                    failure.path.display(),
                    failure.error
                );
            }
            if result.failures.is_empty() {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::from(2))
            }
        }
        Command::Lookup(args) => {
            let matches = lookup(&args.store, &args.query)?;
            for found in &matches {
                println!(
                    "{}\t{}\t{}\t{}",
                    found.label, found.path, found.blake3, found.size
                );
            }
            if matches.is_empty() {
                Ok(ExitCode::from(1))
            } else {
                Ok(ExitCode::SUCCESS)
            }
        }
    }
}
