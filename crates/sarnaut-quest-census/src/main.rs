//! `sarnaut-quest-census`: report how many extracted quests the current server
//! objective rules can serve.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::{Deserialize, Serialize};

const SUPPORTED_OBJECTIVE_KINDS: [&str; 2] = ["quest-count-kill", "quest-count-item"];

#[derive(Parser)]
#[command(
    name = "sarnaut-quest-census",
    version,
    about = "Measure servable quests against the server's objective rules"
)]
struct Cli {
    /// Directory containing extracted quest YAML documents.
    #[arg(long, value_name = "DIR")]
    quests: PathBuf,
    /// Committed census baseline. The command fails if the servable or total
    /// quest count falls below it.
    #[arg(long, value_name = "FILE")]
    baseline: Option<PathBuf>,
    /// Write the machine-readable census to this JSON file.
    #[arg(long, value_name = "FILE")]
    json: Option<PathBuf>,
    /// Optional ADR 0036 per-tier count JSON. Its three fields occupy the same
    /// output object now even when no script census exists yet.
    #[arg(long, value_name = "FILE")]
    script_node_counts: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct QuestDocument {
    id: Option<String>,
    #[serde(default)]
    objectives: Vec<Objective>,
}

#[derive(Debug, Deserialize)]
struct Objective {
    kind: Option<String>,
    #[serde(default)]
    limit: u64,
    #[serde(default)]
    targets: Vec<serde_yaml::Value>,
}

#[derive(Debug, Serialize)]
struct Census {
    schema_version: u32,
    source: String,
    quests: Vec<QuestResult>,
    summary: Summary,
    script_nodes: ScriptNodeCounts,
}

#[derive(Debug, Serialize)]
struct QuestResult {
    id: String,
    source: String,
    status: &'static str,
    reason_code: &'static str,
    reason: String,
    objective_kinds: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct Summary {
    total_quests: usize,
    servable: usize,
    unservable: usize,
    objectives_extracted: usize,
    objective_kinds: BTreeMap<String, usize>,
    reasons: BTreeMap<String, usize>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ScriptNodeCounts {
    implemented: Option<u64>,
    inert_and_counted: Option<u64>,
    refused: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Baseline {
    minimum_servable: usize,
    measured: BaselineMeasurement,
}

#[derive(Debug, Deserialize)]
struct BaselineMeasurement {
    total_quests: usize,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let script_nodes = read_script_node_counts(cli.script_node_counts.as_deref())?;
    let census = census(&cli.quests, script_nodes)?;

    print_table(&census);
    if let Some(path) = cli.json.as_deref() {
        write_json(path, &census)?;
    }
    if let Some(path) = cli.baseline.as_deref() {
        enforce_baseline(path, &census.summary)?;
    }
    Ok(())
}

fn census(root: &Path, script_nodes: ScriptNodeCounts) -> Result<Census> {
    let mut paths = fs::read_dir(root)
        .with_context(|| format!("read quest directory {}", root.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("list quest directory {}", root.display()))?;
    paths.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "yaml")
    });
    paths.sort();
    if paths.is_empty() {
        bail!(
            "quest directory {} contains no .yaml documents",
            root.display()
        );
    }

    let mut quests = Vec::with_capacity(paths.len());
    for path in paths {
        quests.push(read_quest(&path));
    }
    quests.sort_by(|left, right| left.id.cmp(&right.id).then(left.source.cmp(&right.source)));

    let mut objective_kinds = BTreeMap::new();
    let mut reasons = BTreeMap::new();
    for quest in &quests {
        for kind in &quest.objective_kinds {
            *objective_kinds.entry(kind.clone()).or_insert(0) += 1;
        }
        let reason_group = if quest.status == "servable" {
            "servable"
        } else {
            quest.reason_code
        };
        *reasons.entry(reason_group.to_owned()).or_insert(0) += 1;
    }
    let servable = quests
        .iter()
        .filter(|quest| quest.status == "servable")
        .count();
    let summary = Summary {
        total_quests: quests.len(),
        servable,
        unservable: quests.len() - servable,
        objectives_extracted: objective_kinds.values().sum(),
        objective_kinds,
        reasons,
    };

    Ok(Census {
        schema_version: 1,
        source: root.display().to_string(),
        quests,
        summary,
        script_nodes,
    })
}

fn read_quest(path: &Path) -> QuestResult {
    let source = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<non-UTF-8 filename>")
        .to_owned();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            return unservable(
                source.clone(),
                source,
                "unreadable quest document",
                format!("unreadable quest document: {error}"),
                Vec::new(),
            );
        }
    };
    let document: QuestDocument = match serde_yaml::from_str(&text) {
        Ok(document) => document,
        Err(error) => {
            return unservable(
                source.clone(),
                source,
                "malformed quest document",
                format!("malformed quest document: {error}"),
                Vec::new(),
            );
        }
    };
    let id = match document.id {
        Some(id) if !id.is_empty() => id,
        _ => {
            return unservable(
                source.clone(),
                source,
                "malformed quest document",
                "malformed quest document: missing id".to_owned(),
                document.objectives.iter().map(objective_kind).collect(),
            );
        }
    };
    let kinds: Vec<_> = document.objectives.iter().map(objective_kind).collect();

    for (index, objective) in document.objectives.iter().enumerate() {
        let kind = objective_kind(objective);
        if !SUPPORTED_OBJECTIVE_KINDS.contains(&kind.as_str()) {
            return unservable(
                id,
                source,
                "unsupported objective kind",
                format!("unsupported objective kind: {kind} (objective {index})"),
                kinds,
            );
        }
        if objective.limit > 0 && objective.targets.is_empty() {
            return unservable(
                id,
                source,
                "invalid objective shape",
                format!(
                    "invalid objective shape: {kind} objective {index} asks for {} of nothing",
                    objective.limit
                ),
                kinds,
            );
        }
    }

    QuestResult {
        id,
        source,
        status: "servable",
        reason_code: "servable",
        reason: "servable".to_owned(),
        objective_kinds: kinds,
    }
}

fn objective_kind(objective: &Objective) -> String {
    objective
        .kind
        .clone()
        .unwrap_or_else(|| "unspecified".to_owned())
}

fn unservable(
    id: String,
    source: String,
    reason_code: &'static str,
    reason: String,
    objective_kinds: Vec<String>,
) -> QuestResult {
    QuestResult {
        id,
        source,
        status: "unservable",
        reason_code,
        reason,
        objective_kinds,
    }
}

fn print_table(census: &Census) {
    let id_width = census
        .quests
        .iter()
        .map(|quest| quest.id.len())
        .max()
        .unwrap_or(5)
        .max(5);
    println!("{:<id_width$}  {:<10}  REASON", "QUEST", "STATUS");
    println!("{:-<id_width$}  {:-<10}  {:-<6}", "", "", "");
    for quest in &census.quests {
        println!(
            "{:<id_width$}  {:<10}  {}",
            quest.id, quest.status, quest.reason
        );
    }
    println!();
    println!(
        "{} of {} servable, {} unservable",
        census.summary.servable, census.summary.total_quests, census.summary.unservable
    );
    let kinds = census
        .summary
        .objective_kinds
        .iter()
        .map(|(kind, count)| format!("{kind}={count}"))
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "{} extracted objectives ({kinds})",
        census.summary.objectives_extracted
    );
    println!(
        "script nodes: implemented={}, inert-and-counted={}, refused={}",
        display_count(census.script_nodes.implemented),
        display_count(census.script_nodes.inert_and_counted),
        display_count(census.script_nodes.refused)
    );
}

fn display_count(count: Option<u64>) -> String {
    count.map_or_else(|| "not measured".to_owned(), |count| count.to_string())
}

fn write_json(path: &Path, census: &Census) -> Result<()> {
    let mut text = serde_json::to_string_pretty(census).context("encode census as JSON")?;
    text.push('\n');
    fs::write(path, text).with_context(|| format!("write census JSON to {}", path.display()))
}

fn read_script_node_counts(path: Option<&Path>) -> Result<ScriptNodeCounts> {
    let Some(path) = path else {
        return Ok(ScriptNodeCounts::default());
    };
    let text = fs::read_to_string(path)
        .with_context(|| format!("read script node counts from {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("parse script node counts from {}", path.display()))
}

fn enforce_baseline(path: &Path, summary: &Summary) -> Result<()> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read census baseline {}", path.display()))?;
    let baseline: Baseline = serde_json::from_str(&text)
        .with_context(|| format!("parse census baseline {}", path.display()))?;
    if summary.servable < baseline.minimum_servable {
        bail!(
            "servable quest count regressed below baseline: {} < {}",
            summary.servable,
            baseline.minimum_servable
        );
    }
    if summary.total_quests < baseline.measured.total_quests {
        bail!(
            "total quest count regressed below baseline: {} < {}",
            summary.total_quests,
            baseline.measured.total_quests
        );
    }
    Ok(())
}
