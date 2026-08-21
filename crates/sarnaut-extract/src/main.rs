use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use sarnaut_extract::{
    ExtractionOptions, LocaleOptions, discover_schema_dir, extract_items, extract_locale,
    extract_loot, extract_mobkinds, extract_scripts, extract_zone,
};

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
    /// Resolve MobKind prototype chains, plus the classes, qualities and factions
    /// one zone's mobs reach.
    Mobkinds(ZoneArgs),
    /// Extract the loot tables one zone's mob kinds reach, plus the item index.
    Loot(ZoneArgs),
    /// Resolve the loc_ref strings one zone's content points at.
    Locale(LocaleArgs),
    /// Extract quest script trees and the shared triggers they reference.
    Scripts(ZoneArgs),
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

#[derive(Debug, Args)]
struct LocaleArgs {
    /// Zone directory name or its kebab-case form.
    #[arg(long)]
    name: String,
    /// BCP 47 language subtag for the strings in the source trees.
    #[arg(long, default_value = "ru")]
    language: String,
    /// A second data root read only for keys the primary root does not carry.
    #[arg(long)]
    supplemental_src: Option<PathBuf>,
    /// Fail when more than this share of loc_refs stay unresolved, in percent.
    #[arg(long, default_value_t = 5.0)]
    max_unresolved_rate: f64,
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
            println!("locators: {}", summary.locators);
            println!("mobs: {}", summary.mobs);
            println!("routes: {}", summary.routes);
            println!("unchanged files: {}", summary.unchanged);
        }
        Command::Mobkinds(args) => {
            let name = args.name;
            let options = options(args.common)?;
            let summary = extract_mobkinds(&name, &options)?;
            println!("zone: {}", summary.zone);
            println!("mob worlds scanned: {}", summary.mob_worlds);
            println!("mob kinds: {}", summary.mob_kinds);
            println!("prototype mob kinds: {}", summary.prototypes);
            println!("mob classes: {}", summary.mob_classes);
            println!("mob qualities: {}", summary.mob_qualities);
            println!("factions: {}", summary.factions);
            println!("unchanged files: {}", summary.unchanged);
            println!("level curve: not found in source; curated overlay constant");
        }
        Command::Loot(args) => {
            let name = args.name;
            let validate = args.common.validate;
            let options = options(args.common)?;
            let summary = extract_loot(&name, &options)?;
            println!("zone: {}", summary.zone);
            println!("loot tables: {}", summary.tables);
            println!("loot nodes: {}", summary.nodes);
            println!("indexed items: {}", summary.item_index);
            println!("dangling item references: {}", summary.dangling_items.len());
            println!("unchanged files: {}", summary.unchanged);
            if validate && !summary.dangling_items.is_empty() {
                for offender in &summary.dangling_items {
                    eprintln!("dangling item reference: {offender}");
                }
                bail!(
                    "{} loot item references resolve to no item document",
                    summary.dangling_items.len()
                );
            }
        }
        Command::Locale(args) => {
            let name = args.name;
            let validate = args.common.validate;
            let limit = args.max_unresolved_rate;
            let options = LocaleOptions {
                common: options(args.common)?,
                language: args.language,
                supplemental_src: args.supplemental_src,
            };
            let summary = extract_locale(&name, &options)?;
            let rate = summary.unresolved_rate();
            println!("zone: {}", summary.zone);
            println!("language: {}", summary.language);
            println!("loc_refs requested: {}", summary.requested);
            println!("resolved: {}", summary.resolved);
            println!("from primary root: {}", summary.from_primary);
            println!("from supplemental root: {}", summary.from_supplemental);
            println!("root mismatches: {}", summary.mismatched.len());
            println!("unresolved: {}", summary.unresolved.len());
            println!("unresolved rate: {rate:.2}%");
            println!("unchanged files: {}", summary.unchanged);
            for key in summary.mismatched.iter().take(20) {
                eprintln!("root mismatch: {key}");
            }
            for key in summary.unresolved.iter().take(20) {
                eprintln!("unresolved loc_ref: {key}");
            }
            if validate && rate > limit {
                bail!("unresolved loc_ref rate {rate:.2}% exceeds the {limit:.2}% budget");
            }
        }
        Command::Scripts(args) => {
            let name = args.name;
            let options = options(args.common)?;
            let summary = extract_scripts(&name, &options)?;
            println!("zone: {}", summary.zone);
            println!("quest scripts: {}", summary.quest_scripts);
            println!("script triggers: {}", summary.triggers);
            println!("counter bindings: {}", summary.counters);
            println!("script nodes: {}", summary.nodes);
            println!("implemented nodes: {}", summary.implemented);
            println!("inert-and-counted nodes: {}", summary.inert);
            println!("refused nodes: {}", summary.refused);
            println!("external resources: {}", summary.external_resources.len());
            println!("unchanged files: {}", summary.unchanged);
            for trigger_id in summary.trigger_ids {
                println!("script trigger {trigger_id}");
            }
            for (opcode, count) in summary.refused_opcodes {
                println!("refused opcode {opcode}: {count}");
            }
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
