use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

const BASELINE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../census/inst-league1-baseline.json"
);

#[test]
fn census_matches_the_baseline_and_rejects_a_planted_regression() {
    let fixture = TempDir::new().expect("create quest fixture");
    write_day_zero_fixture(fixture.path());
    let report = fixture.path().join("census.json");

    let matching = run_census(fixture.path(), &report);
    assert!(
        matching.status.success(),
        "matching census failed:\n{}",
        String::from_utf8_lossy(&matching.stderr)
    );

    let actual: Value =
        serde_json::from_str(&fs::read_to_string(&report).expect("read generated census report"))
            .expect("parse generated census report");
    let baseline: Value = serde_json::from_str(
        &fs::read_to_string(BASELINE).expect("read committed census baseline"),
    )
    .expect("parse committed census baseline");
    assert_eq!(actual["summary"], baseline["measured"]);

    write_quest(
        &fixture.path().join("servable-01.yaml"),
        "quest.fixture.servable-01",
        "objectives:\n- kind: quest-count-special\n  limit: 1\n",
    );
    let regressed = run_census(fixture.path(), &report);
    assert!(!regressed.status.success(), "planted regression passed");
    assert!(
        String::from_utf8_lossy(&regressed.stderr)
            .contains("servable quest count regressed below baseline: 11 < 12"),
        "unexpected regression diagnostic:\n{}",
        String::from_utf8_lossy(&regressed.stderr)
    );
}

#[test]
fn unsupported_kinds_are_distinct_from_other_unservable_causes() {
    let fixture = TempDir::new().expect("create quest fixture");
    write_quest(
        &fixture.path().join("unsupported.yaml"),
        "quest.fixture.unsupported",
        "objectives:\n- kind: quest-count-special\n  limit: 1\n",
    );
    write_quest(
        &fixture.path().join("invalid.yaml"),
        "quest.fixture.invalid",
        "objectives:\n- kind: quest-count-item\n  limit: 1\n",
    );
    let report = fixture.path().join("census.json");

    let output = Command::new(env!("CARGO_BIN_EXE_sarnaut-quest-census"))
        .args([
            "--quests",
            fixture.path().to_str().expect("UTF-8 fixture path"),
        ])
        .args(["--json", report.to_str().expect("UTF-8 report path")])
        .output()
        .expect("run sarnaut-quest-census");
    assert!(output.status.success());

    let report: Value =
        serde_json::from_str(&fs::read_to_string(report).expect("read generated census report"))
            .expect("parse generated census report");
    assert_eq!(
        report["summary"]["reasons"]["unsupported objective kind"],
        1
    );
    assert_eq!(report["summary"]["reasons"]["invalid objective shape"], 1);
}

#[test]
fn script_tier_counts_fill_the_reserved_report_fields() {
    let fixture = TempDir::new().expect("create quest fixture");
    write_quest(
        &fixture.path().join("servable.yaml"),
        "quest.fixture.servable",
        "",
    );
    let counts = fixture.path().join("script-node-counts.json");
    fs::write(
        &counts,
        r#"{"implemented":17,"inert_and_counted":45,"refused":3}"#,
    )
    .expect("write script counts");
    let report = fixture.path().join("census.json");

    let output = Command::new(env!("CARGO_BIN_EXE_sarnaut-quest-census"))
        .args([
            "--quests",
            fixture.path().to_str().expect("UTF-8 fixture path"),
        ])
        .args([
            "--script-node-counts",
            counts.to_str().expect("UTF-8 counts path"),
        ])
        .args(["--json", report.to_str().expect("UTF-8 report path")])
        .output()
        .expect("run sarnaut-quest-census");
    assert!(
        output.status.success(),
        "census failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: Value =
        serde_json::from_str(&fs::read_to_string(report).expect("read generated census report"))
            .expect("parse generated census report");
    assert_eq!(report["script_nodes"]["implemented"], 17);
    assert_eq!(report["script_nodes"]["inert_and_counted"], 45);
    assert_eq!(report["script_nodes"]["refused"], 3);
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("script nodes: implemented=17, inert-and-counted=45, refused=3")
    );
}

fn run_census(quests: &Path, report: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sarnaut-quest-census"))
        .args(["--quests", quests.to_str().expect("UTF-8 fixture path")])
        .args(["--baseline", BASELINE])
        .args(["--json", report.to_str().expect("UTF-8 report path")])
        .output()
        .expect("run sarnaut-quest-census")
}

fn write_day_zero_fixture(root: &Path) {
    for index in 1..=12 {
        write_quest(
            &root.join(format!("servable-{index:02}.yaml")),
            &format!("quest.fixture.servable-{index:02}"),
            "",
        );
    }

    write_quest(
        &root.join("unservable-01.yaml"),
        "quest.fixture.unservable-01",
        concat!(
            "objectives:\n",
            "- kind: quest-count-item\n  limit: 1\n  targets:\n  - id: item.fixture\n",
            "- kind: quest-count-kill\n  limit: 0\n  targets:\n  - id: mob.fixture\n",
            "- kind: quest-count-special\n  limit: 1\n",
        ),
    );
    write_quest(
        &root.join("unservable-02.yaml"),
        "quest.fixture.unservable-02",
        concat!(
            "objectives:\n",
            "- kind: quest-count-special\n  limit: 1\n",
            "- kind: quest-count-special\n  limit: 1\n",
        ),
    );
    for index in 3..=9 {
        write_quest(
            &root.join(format!("unservable-{index:02}.yaml")),
            &format!("quest.fixture.unservable-{index:02}"),
            "objectives:\n- kind: quest-count-special\n  limit: 1\n",
        );
    }
}

fn write_quest(path: &PathBuf, id: &str, objectives: &str) {
    fs::write(path, format!("schema_version: 1\nid: {id}\n{objectives}"))
        .expect("write quest fixture");
}
