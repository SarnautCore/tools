//! `sarnaut-pack`: compile authored YAML into a runtime pack, check a source
//! tree, and verify a built pack.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use sarnaut_pack::compile::{self, BuildOptions, CompiledPack, PlayerSpawn};
use sarnaut_pack::manifest;
use sarnaut_pack::refs;
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
    /// Compile a source tree in memory and report what it found. Writes
    /// nothing; intended for the data repository's CI.
    Check(CheckArgs),
    /// Check a built pack's manifest digest against its table bytes.
    Verify(VerifyArgs),
}

/// Everything both `build` and `check` need to read a source tree.
#[derive(Args)]
struct SourceArgs {
    /// Source tree. Defaults to the demo dataset under `--fixture`.
    #[arg(long, value_name = "DIR")]
    src: Option<PathBuf>,
    /// Overlay layer to apply, by the id `layers.yaml` gives it. Repeatable.
    /// Layers always apply in `layers.yaml` order whatever order they are named
    /// in. With none given, every layer whose `apply_by_default` is true
    /// applies.
    #[arg(long = "overlay", value_name = "LAYER")]
    overlays: Vec<String>,
    /// Directory holding `layers.yaml`. Defaults to `<src>/overlays`.
    #[arg(long, value_name = "DIR")]
    overlays_root: Option<PathBuf>,
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
    /// Player spawn as `x,y,z` or `x,y,z,yaw`. Overrides the zone document.
    #[arg(long, value_name = "X,Y,Z[,YAW]")]
    player_spawn: Option<String>,
    /// Let the later layer win when two overlay layers write one leaf. Every
    /// conflict is listed in `build-report.json` either way.
    #[arg(long)]
    allow_overlay_conflicts: bool,
    /// Fail on a localization key no locale document supplies, instead of
    /// recording it as a coverage gap.
    #[arg(long)]
    require_locale: bool,
    /// Repository name recorded in `source.repo`.
    #[arg(long, value_name = "NAME")]
    source_repo: Option<String>,
    /// Commit recorded in `source.commit`. Defaults to the source repo's HEAD.
    #[arg(long, value_name = "SHA")]
    source_commit: Option<String>,
}

#[derive(Args)]
struct BuildArgs {
    #[command(flatten)]
    source: SourceArgs,
    /// Pack directory to write.
    #[arg(long, value_name = "DIR")]
    out: PathBuf,
}

#[derive(Args)]
struct CheckArgs {
    #[command(flatten)]
    source: SourceArgs,
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
        Command::Check(args) => run_check(args),
        Command::Verify(args) => run_verify(args),
    }
}

impl SourceArgs {
    fn into_options(self) -> Result<BuildOptions> {
        let source = match (&self.src, self.fixture) {
            (Some(path), _) => path.clone(),
            (None, true) => PathBuf::from(FIXTURE_SOURCE),
            (None, false) => bail!("--src is required unless --fixture supplies the demo dataset"),
        };
        Ok(BuildOptions {
            source,
            overlays_root: self.overlays_root,
            overlays: self.overlays,
            layout: if self.fixture {
                Layout::Flat
            } else {
                Layout::Ruleset
            },
            ruleset: self.ruleset,
            zone: self.zone,
            keep_extra: self.keep_extra,
            player_spawn: self.player_spawn.as_deref().map(parse_spawn).transpose()?,
            allow_overlay_conflicts: self.allow_overlay_conflicts,
            require_locale: self.require_locale,
            source_repo: self
                .source_repo
                .unwrap_or_else(|| if self.fixture { FIXTURE_REPO } else { "data" }.to_string()),
            source_commit: match (self.source_commit, self.fixture) {
                (Some(commit), _) => Some(commit),
                // A fixture rebuilt in CI must match the copy vendored in
                // `server`, so it must not pick up whatever commit happens to
                // be checked out.
                (None, true) => Some(manifest::UNKNOWN_COMMIT.to_string()),
                (None, false) => None,
            },
        })
    }
}

fn run_build(args: BuildArgs) -> Result<()> {
    let options = args.source.into_options()?;
    let pack = compile::build(&options, &args.out).context("build pack")?;
    print_summary(&pack);
    Ok(())
}

fn run_check(args: CheckArgs) -> Result<()> {
    let options = args.source.into_options()?;
    let pack = compile::compile(&options).context("check source tree")?;
    print_summary(&pack);
    if let Some(scripts) = &pack.report.scripts {
        let refused: usize = scripts.refused.values().sum();
        if refused > 0 {
            bail!(
                "script coverage contains {refused} refused node(s); change their checked-in tier only after the runtime supports them"
            );
        }
    }
    Ok(())
}

/// One report shape for `build` and `check`, so a CI log and a local build read
/// the same way.
///
/// A clean run still prints the external, unmodelled and locale-gap counts.
/// They are the numbers that quietly grow when an extractor loses coverage, and
/// a check that only printed failures would hide that.
fn print_summary(pack: &CompiledPack) {
    println!("pack_id  {}", pack.pack_id());
    println!("zone     {}/{}", pack.manifest.ruleset, pack.zone());
    if !pack.manifest.source.overlays.is_empty() {
        println!("overlays {}", pack.manifest.source.overlays.join(", "));
    }
    for document in &pack.report.skipped_overlay_documents {
        println!(
            "skip     {} (layer {}): {}",
            document.document, document.layer, document.note
        );
    }
    for (name, rows) in pack.tables() {
        println!("table    {name} ({rows} rows)");
    }
    if let Some(scripts) = &pack.report.scripts {
        let implemented: usize = scripts.implemented.values().sum();
        let inert: usize = scripts.inert_and_counted.values().sum();
        let refused: usize = scripts.refused.values().sum();
        println!(
            "scripts  {} nodes: implemented={implemented}, inert-and-counted={inert}, refused={refused}",
            scripts.nodes
        );
        for (tier, opcodes) in [
            ("implemented", &scripts.implemented),
            ("inert-and-counted", &scripts.inert_and_counted),
            ("refused", &scripts.refused),
        ] {
            for (opcode, count) in opcodes {
                println!("opcode   {tier} {opcode} {count}");
            }
        }
    }
    let references = &pack.references;
    println!(
        "refs     {} resolved, 0 unresolved, {} external, {} unmodelled",
        references.resolved,
        references.external.len(),
        references.unmodelled.len()
    );
    if !references.locale_gaps.is_empty() {
        let by_namespace = refs::gaps_by_namespace(references)
            .into_iter()
            .map(|(namespace, count)| format!("{namespace} {count}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "locale   {} key(s) no locale document supplies ({by_namespace}); pass --require-locale to fail on them",
            references.locale_gaps.len()
        );
    }
    println!(
        "notes    {} curated document(s)",
        pack.report.curation_notes.len()
    );
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
