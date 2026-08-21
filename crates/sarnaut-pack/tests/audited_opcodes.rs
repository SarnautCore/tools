//! Pack and census proof for the five opcodes promoted by the M3 semantics audit.

mod common;

use std::fs;

use prost::Message;
use sarnaut_pack::compile;
use sarnaut_pack::proto;
use sarnaut_pack::table;

const PROMOTED: [&str; 5] = [
    "DestinationLocator",
    "Guard",
    "PredicateIsAvatar",
    "ScalerAllInputDamage",
    "ScalerAllOutputDamage",
];

#[test]
fn promoted_opcodes_round_trip_and_report_seven_reachable_of_nine_total() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    write_rows(&source);

    let first = workspace.path().join("first");
    let second = workspace.path().join("second");
    compile::build(&common::options(source.clone()), &first).expect("first pack build");
    compile::build(&common::options(source), &second).expect("second pack build");
    for table_name in ["quest-scripts", "script-triggers"] {
        assert_eq!(
            fs::read(first.join(format!("tables/{table_name}.sptbl"))).expect("first table"),
            fs::read(second.join(format!("tables/{table_name}.sptbl"))).expect("second table"),
            "{table_name} is not deterministic"
        );
    }

    let scripts: Vec<proto::QuestScript> = decode(&first, "quest-scripts");
    let encoded = scripts[0].encode_to_vec();
    assert_eq!(
        proto::QuestScript::decode(encoded.as_slice())
            .expect("decode quest script")
            .encode_to_vec(),
        encoded
    );
    let destination = &scripts[0].start_impacts[0];
    assert_eq!(destination.opcode, "DestinationLocator");
    assert_eq!(destination.tier, proto::CoverageTier::Implemented as i32);
    assert_eq!(
        destination
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        ["locator", "yaw"]
    );

    let report: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(first.join("build-report.json")).expect("build report"),
    )
    .expect("parse build report");
    let total: i64 = PROMOTED
        .iter()
        .map(|opcode| {
            report["scripts"]["implemented"][opcode]
                .as_i64()
                .unwrap_or(0)
        })
        .sum();
    let reachable: i64 = PROMOTED
        .iter()
        .map(|opcode| {
            report["scripts"]["reachable_implemented"][opcode]
                .as_i64()
                .unwrap_or(0)
        })
        .sum();
    assert_eq!(total, 9, "all retained promoted nodes");
    assert_eq!(reachable, 7, "quest-reachable promoted nodes");
    assert_eq!(report["scripts"]["reachable_script_triggers"], 3);
    assert_eq!(report["scripts"]["refused"], serde_json::json!({}));
}

fn decode<T: Message + Default>(root: &std::path::Path, name: &str) -> Vec<T> {
    table::rows(&fs::read(root.join(format!("tables/{name}.sptbl"))).expect("read table"))
        .expect("decode table")
        .into_iter()
        .map(|row| T::decode(row).expect("decode row"))
        .collect()
}

fn write_rows(source: &std::path::Path) {
    let root = source.join("classic/zones/harbour-watch/scripts");
    fs::create_dir_all(root.join("quests")).expect("quest scripts dir");
    fs::create_dir_all(root.join("triggers")).expect("trigger scripts dir");
    let trigger_agents = format!(
        "{}\n{}",
        trigger_agent(
            "script.harbour-watch.audited/triggerAgents[0]",
            "trigger.harbour-watch.q3"
        ),
        trigger_agent(
            "script.harbour-watch.audited/triggerAgents[1]",
            "trigger.harbour-watch.gibber"
        )
    );
    fs::write(
        root.join("quests/audited.yaml"),
        format!(
            r#"id: script.harbour-watch.audited
zone: zone.harbour-watch
quest: quest.harbour-watch.first
start_impacts:
{}
{}
trigger_agents:
{}
"#,
            destination("script.harbour-watch.audited/startImpacts[0]", "Arrival"),
            destination("script.harbour-watch.audited/startImpacts[1]", "Firewall"),
            trigger_agents
        ),
    )
    .expect("write quest row");
    write_trigger(
        &root,
        "q3",
        &[output_scaler("-9", 1), input_scaler("-9", 1)],
        false,
    );
    write_trigger(
        &root,
        "gibber",
        &[guard(15, false), input_scaler("100", 0)],
        false,
    );
    write_trigger(&root, "final", &[predicate_avatar()], true);
    write_trigger(
        &root,
        "orphan",
        &[guard(50, false), output_scaler("-5", 1)],
        false,
    );
}

fn write_trigger(root: &std::path::Path, id: &str, children: &[String], entrypoint: bool) {
    let values = children
        .iter()
        .enumerate()
        .map(|(index, child)| {
            child.replace(
                "$KEY",
                &format!("trigger.harbour-watch.{id}/effects[{index}]"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(
        root.join(format!("triggers/{id}.yaml")),
        format!(
            "id: trigger.harbour-watch.{id}\nzone: zone.harbour-watch\n{}root:\n  key: trigger.harbour-watch.{id}\n  family: trigger\n  opcode: TriggerResource\n  tier: implemented\n  fields:\n    - name: effects\n      value:\n        list:\n{values}\n",
            if entrypoint { "entrypoint: true\n" } else { "" }
        ),
    )
    .expect("write trigger row");
}

fn destination(key: &str, script_id: &str) -> String {
    format!(
        r#"  - key: {key}
    family: impact
    opcode: DestinationLocator
    tier: implemented
    fields:
      - name: locator
        value:
          node:
            key: {key}/locator
            family: basic
            opcode: Struct
            tier: inert-and-counted
            fields:
              - name: map
                value:
                  reference:
                    id: ext.maps.harbour-watch.map-resource
                    row_type: map-resource
              - name: scriptID
                value: {{text: {script_id}}}
      - name: yaw
        value: {{integer: 0}}"#
    )
}

fn trigger_agent(key: &str, target: &str) -> String {
    format!(
        "  - key: {key}\n    family: trigger\n    opcode: TriggerAgentSelf\n    tier: implemented\n    fields:\n      - name: trigger\n        value:\n          reference:\n            id: {target}\n            row_type: trigger"
    )
}

fn guard(radius: i64, notice: bool) -> String {
    format!(
        "          - node:\n              key: $KEY\n              family: effect\n              opcode: Guard\n              tier: implemented\n              fields:\n                - name: noticeTarget\n                  value: {{boolean: {notice}}}\n                - name: scanRadius\n                  value:\n                    decimal: {{mantissa: {radius}, scale: 0}}"
    )
}

fn predicate_avatar() -> String {
    "          - node:\n              key: $KEY\n              family: predicate\n              opcode: PredicateIsAvatar\n              tier: implemented".to_owned()
}

fn input_scaler(mantissa: &str, scale: i32) -> String {
    scaler("ScalerAllInputDamage", mantissa, scale, true)
}

fn output_scaler(mantissa: &str, scale: i32) -> String {
    scaler("ScalerAllOutputDamage", mantissa, scale, false)
}

fn scaler(opcode: &str, mantissa: &str, scale: i32, input: bool) -> String {
    let input_fields = if input {
        "                - name: attackerConditions\n                  value: {list: []}\n                - name: onlyFromCaster\n                  value: {boolean: false}\n"
    } else {
        ""
    };
    format!(
        "          - node:\n              key: $KEY\n              family: effect\n              opcode: {opcode}\n              tier: implemented\n              fields:\n{input_fields}                - name: scaler\n                  value:\n                    node:\n                      key: $KEY/scaler\n                      family: scaler\n                      opcode: LinearEffectScaler\n                      tier: implemented\n                      fields:\n                        - name: coeff\n                          value:\n                            decimal: {{mantissa: {mantissa}, scale: {scale}}}\n                - name: stackCount\n                  value: {{integer: 1}}"
    )
}
