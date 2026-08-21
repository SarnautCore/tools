mod common;

use std::fs;

use prost::Message;
use sarnaut_pack::{compile, proto, table};

#[test]
fn native_actions_and_progression_compile_deterministically() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    write_native_runtime(&source);

    let first = workspace.path().join("first");
    let second = workspace.path().join("second");
    compile::build(&common::options(source.clone()), &first).expect("first build");
    compile::build(&common::options(source), &second).expect("second build");

    for name in ["native-actions", "player-progression"] {
        let left = fs::read(first.join(format!("tables/{name}.sptbl"))).expect("first table");
        let right = fs::read(second.join(format!("tables/{name}.sptbl"))).expect("second table");
        assert_eq!(left, right, "{name} is not deterministic");
    }

    let actions: Vec<proto::NativeAction> = decode(&first, "native-actions");
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].id, "action.demo.auto-attack");
    assert_eq!(actions[0].cooldown.as_ref().unwrap().duration_ms, 0);
    assert_eq!(
        actions[0].target_impacts[0].opcode,
        "ScaledPhysicalWeaponDamage"
    );

    let progressions: Vec<proto::PlayerProgression> = decode(&first, "player-progression");
    assert_eq!(progressions.len(), 1);
    assert_eq!(progressions[0].thresholds.len(), 2);
    assert_eq!(progressions[0].experience_impacts[0].mob_count, 2);
    assert_eq!(
        progressions[0].experience_impacts[0].resolved_experience,
        20
    );
    assert_eq!(progressions[0].respawn_delay_ms, 1000);
}

#[test]
fn progression_refuses_a_threshold_gap() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    write_native_runtime(&source);
    let path = source.join("classic/progression/avatar.yaml");
    let text = fs::read_to_string(&path).expect("read progression");
    fs::write(&path, text.replace("level: 2,", "level: 3,")).expect("write gap");

    let error = compile::compile(&common::options(source)).expect_err("gap must fail");
    let message = format!("{error:#}");
    assert!(
        message.contains("names level 3, want 2"),
        "unexpected error: {message}"
    );
}

fn decode<M: Message + Default>(root: &std::path::Path, name: &str) -> Vec<M> {
    let bytes = fs::read(root.join(format!("tables/{name}.sptbl"))).expect("read table");
    table::rows(&bytes)
        .expect("rows")
        .into_iter()
        .map(|row| M::decode(row).expect("decode row"))
        .collect()
}

fn write_native_runtime(source: &std::path::Path) {
    let actions = source.join("classic/actions");
    let progression = source.join("classic/progression");
    fs::create_dir_all(&actions).expect("action dir");
    fs::create_dir_all(&progression).expect("progression dir");
    fs::write(
        actions.join("auto-attack.yaml"),
        r#"schema_version: 1
id: action.demo.auto-attack
source_type: demo.NativeAction
target_policy: current-target
range_m: { mantissa: 5, scale: 0 }
cast_duration_ms: 0
channel_duration_ms: 0
prepare_duration_ms: 0
requires_los: true
is_aggro: true
triggers_gcd: true
ignores_gcd: true
action_group_id: action-group.demo.auto-attack
cooldown: { duration_ms: 0, scaler: weapon-speed }
resource:
  kind: energy
  cost: { mantissa: 125, scale: 1 }
  scale_by_weapon_speed: true
  source: mainhand
caster_conditions: []
target_impacts:
  - key: action.demo.auto-attack/targetImpacts[0]
    family: impact
    opcode: ScaledPhysicalWeaponDamage
    tier: inert-and-counted
    fields:
      - name: avgDamage
        value: { decimal: { mantissa: 875, scale: 2 } }
      - name: scaler
        value: { text: physical }
_source:
  path: Demo/AutoAttack.xdb
  blake3: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  extractor: sarnaut-extract@0.1.0
  source_root: demo
"#,
    )
    .expect("write action");
    fs::write(
        progression.join("avatar.yaml"),
        r#"schema_version: 1
id: progression.demo.avatar
source_type: demo.PlayerProgression
max_level: 2
respawn_delay_ms: 1000
resurrection_sickness_duration_ms: 5000
thresholds:
  - { level: 1, cumulative_experience: 0 }
  - { level: 2, cumulative_experience: 100 }
experience_impacts:
  - id: impact.demo.mob-pack
    mob_count: 2
    mob_level: 1
    resolved_experience: 20
_source:
  path: Demo/ExperienceTable.xdb
  blake3: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
  extractor: sarnaut-extract@0.1.0
  source_root: demo
"#,
    )
    .expect("write progression");
}
