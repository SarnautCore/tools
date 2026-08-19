use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use sarnaut_extract::{ExtractionOptions, discover_schema_dir, extract_items, extract_zone};

#[derive(Debug, Parser)]
#[command(name = "sarnaut-extract", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Extract all ItemResource XDBs below Items.
    Items(CommonArgs),
    /// Extract quests, mobs, spawn tables, placements, and routes for one zone.
    Zone(ZoneArgs),
}

#[derive(Debug, Args)]
struct CommonArgs {
    /// Root of the extracted game data tree, containing Items, Maps, and World.
    #[arg(long)]
    src: PathBuf,
    /// Ruleset output directory, such as data/classic.
    #[arg(long)]
    out: PathBuf,
    /// Parse and report without writing YAML files.
    #[arg(long)]
    dry_run: bool,
    /// Validate every generated document against the public JSON Schemas.
    #[arg(long)]
    validate: bool,
    /// Schema directory. Otherwise discovered beside the data or tools repository.
    #[arg(long, env = "SARNAUT_SCHEMA_DIR")]
    schema_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ZoneArgs {
    /// Zone directory name or its kebab-case form.
    #[arg(long)]
    name: String,
    #[command(flatten)]
    common: CommonArgs,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Items(args) => {
            let options = options(args)?;
            let summary = extract_items(&options)?;
            println!("items: {}", summary.emitted);
            println!("auxiliary XDBs skipped: {}", summary.skipped_auxiliary);
            println!("unchanged files: {}", summary.unchanged);
            for (category, count) in summary.categories {
                println!("item category {category}: {count}");
            }
        }
        Command::Zone(args) => {
            let name = args.name;
            let options = options(args.common)?;
            let summary = extract_zone(&name, &options)?;
            println!("zone: {}", summary.zone);
            println!("map: {}", summary.map);
            println!("quests: {}", summary.quests);
            println!("spawn tables: {}", summary.spawn_tables);
            println!("spawn points: {}", summary.spawn_points);
            println!("mobs: {}", summary.mobs);
            println!("routes: {}", summary.routes);
            println!("unchanged files: {}", summary.unchanged);
        }
    }
    Ok(())
}

fn options(args: CommonArgs) -> Result<ExtractionOptions> {
    let schema_dir = if args.validate {
        args.schema_dir
            .or_else(|| discover_schema_dir(&args.out))
            .context(
                "--validate needs --schema-dir or a discoverable data-schemas/schemas directory",
            )?
            .into()
    } else {
        None
    };
    Ok(ExtractionOptions {
        src: args.src,
        out: args.out,
        dry_run: args.dry_run,
        schema_dir,
    })
}
