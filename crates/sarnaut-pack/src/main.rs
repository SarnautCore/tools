//! `sarnaut-pack`: compile authored YAML into a runtime pack, and check one.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use sarnaut_pack::compile::{self, BuildOptions, PlayerSpawn};
use sarnaut_pack::manifest;
use sarnaut_pack::source::Layout;
use sarnaut_pack::verify;

/// Default source for `--fixture`, relative to the `tools` checkout.
const FIXTURE_SOURCE: &str = "../data-schemas/demo";
const FIXTURE_REPO: &str = "data-schemas";

#[derive(Parser)]
#[command(
    name = "sarnaut-pack",
    version,
    about = "Compile SarnautCore YAML into runtime packs (ADR 0029)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile a source tree into a pack directory.
    Build(BuildArgs),
    /// Check a pack's manifest digest against its table bytes.
    Verify(VerifyArgs),
}

#[derive(Args)]
struct BuildArgs {
    /// Source tree. Defaults to the demo dataset under `--fixture`.
    #[arg(long, value_name = "DIR")]
    src: Option<PathBuf>,
    /// Pack directory to write.
    #[arg(long, value_name = "DIR")]
    out: PathBuf,
    /// Build the golden fixture from `data-schemas/demo`: no private data, and
    /// a pinned `source.commit` so the vendored copy stays byte-stable.
    #[arg(long)]
    fixture: bool,
    #[arg(long, default_value = "classic")]
    ruleset: String,
    /// Zone slug. Required for a `data`-shaped source tree.
    #[arg(long, value_name = "SLUG")]
    zone: Option<String>,
    /// Keep the untyped `extra:` passthrough. Private-path artifact only.
    #[arg(long)]
    keep_extra: bool,
    /// Player spawn as `x,y,z` or `x,y,z,yaw`.
    #[arg(long, value_name = "X,Y,Z[,YAW]")]
    player_spawn: Option<String>,
    /// Repository name recorded in `source.repo`.
    #[arg(long, value_name = "NAME")]
    source_repo: Option<String>,
    /// Commit recorded in `source.commit`. Defaults to the source repo's HEAD.
    #[arg(long, value_name = "SHA")]
    source_commit: Option<String>,
}

#[derive(Args)]
struct VerifyArgs {
    /// Pack directory holding `manifest.json`.
    #[arg(value_name = "PACK")]
    pack: PathBuf,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Build(args) => run_build(args),
        Command::Verify(args) => run_verify(args),
    }
}

fn run_build(args: BuildArgs) -> Result<()> {
    let source = match (&args.src, args.fixture) {
        (Some(path), _) => path.clone(),
        (None, true) => PathBuf::from(FIXTURE_SOURCE),
        (None, false) => bail!("--src is required unless --fixture supplies the demo dataset"),
    };
    let options = BuildOptions {
        source,
        out: args.out,
        layout: if args.fixture {
            Layout::Flat
        } else {
            Layout::Ruleset
        },
        ruleset: args.ruleset,
        zone: args.zone,
        keep_extra: args.keep_extra,
        player_spawn: args.player_spawn.as_deref().map(parse_spawn).transpose()?,
        source_repo: args
            .source_repo
            .unwrap_or_else(|| if args.fixture { FIXTURE_REPO } else { "data" }.to_string()),
        source_commit: match (args.source_commit, args.fixture) {
            (Some(commit), _) => Some(commit),
            // A fixture rebuilt in CI must match the copy vendored in `server`,
            // so it must not pick up whatever commit happens to be checked out.
            (None, true) => Some(manifest::UNKNOWN_COMMIT.to_string()),
            (None, false) => None,
        },
    };

    let report = compile::build(&options).context("build pack")?;
    println!("pack_id {}", report.pack_id);
    println!("zone     {}", report.zone);
    for (name, rows) in &report.tables {
        println!("table    {name} ({rows} rows)");
    }
    Ok(())
}

fn run_verify(args: VerifyArgs) -> Result<()> {
    let report = verify::verify(&args.pack)
        .with_context(|| format!("verify pack {}", args.pack.display()))?;
    println!("pack_id {}", report.pack_id);
    println!("zone     {}/{}", report.ruleset, report.zone);
    for (name, rows) in &report.tables {
        println!("table    {name} ({rows} rows) ok");
    }
    Ok(())
}

fn parse_spawn(value: &str) -> Result<PlayerSpawn> {
    let parts: Vec<&str> = value.split(',').map(str::trim).collect();
    if parts.len() != 3 && parts.len() != 4 {
        bail!("--player-spawn wants `x,y,z` or `x,y,z,yaw`, got {value:?}");
    }
    let mut numbers = Vec::with_capacity(parts.len());
    for part in &parts {
        numbers.push(
            part.parse::<f32>()
                .with_context(|| format!("--player-spawn component {part:?} is not a number"))?,
        );
    }
    Ok(PlayerSpawn {
        x: numbers[0],
        y: numbers[1],
        z: numbers[2],
        yaw: numbers.get(3).copied().unwrap_or(0.0),
    })
}
