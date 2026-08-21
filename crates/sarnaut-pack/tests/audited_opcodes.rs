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
    for table_name in ["quest-scripts", "script-triggers", "map-locators"] {
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
    let locator_record = match destination.fields[0]
        .value
        .as_ref()
        .and_then(|value| value.value.as_ref())
    {
        Some(proto::script_value::Value::Node(node)) => node,
        other => panic!("DestinationLocator.locator is not a node: {other:?}"),
    };
    let map_reference = match locator_record.fields[0]
        .value
        .as_ref()
        .and_then(|value| value.value.as_ref())
    {
        Some(proto::script_value::Value::Reference(reference)) => reference,
        other => panic!("MapPointer.map is not a reference: {other:?}"),
    };
    assert_eq!(map_reference.id, "quay");
    assert_eq!(map_reference.row_type, "map");
    assert!(!map_reference.id.starts_with("ext."));
    let locators: Vec<proto::MapLocator> = decode(&first, "map-locators");
    assert_eq!(locators.len(), 2);
    assert_eq!(locators[0].map_id, "quay");
    assert_eq!(locators[0].script_id, "Arrival");
    assert_eq!(locators[1].script_id, "Firewall");
    assert_eq!(
        table::validate(&fs::read(first.join("tables/map-locators.sptbl")).expect("locator table"))
            .expect("validate locator table")
            .row_type_id,
        17
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

#[test]
fn destination_locator_missing_from_the_map_index_fails_the_build() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    write_rows(&source);
    let placements = source.join("classic/zones/harbour-watch/spawns/placements/quay.yaml");
    let text = fs::read_to_string(&placements)
        .expect("placement fixture")
        .replace("script_id: Firewall", "script_id: SomewhereElse");
    fs::write(&placements, text).expect("write missing locator fixture");

    let error = compile::build(&common::options(source), &workspace.path().join("pack"))
        .expect_err("absent DestinationLocator target must fail");
    assert!(
        error
            .to_string()
            .contains("references absent map locator quay/Firewall"),
        "{error:#}"
    );
}

#[test]
fn duplicate_map_and_script_id_fails_even_when_positions_differ() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    write_rows(&source);
    let placements = source.join("classic/zones/harbour-watch/spawns/placements/quay.yaml");
    let mut text = fs::read_to_string(&placements).expect("placement fixture");
    text.push_str("  - script_id: Arrival\n    position: { x: 99.0, y: 98.0, z: 97.0 }\n");
    fs::write(&placements, text).expect("write duplicate locator fixture");

    let error = compile::build(&common::options(source), &workspace.path().join("pack"))
        .expect_err("duplicate locator identity must fail");
    assert!(
        error
            .to_string()
            .contains("map locator id quay/Arrival is declared twice"),
        "{error:#}"
    );
}

#[test]
fn locator_row_key_components_reject_source_paths() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    write_rows(&source);
    let placements = source.join("classic/zones/harbour-watch/spawns/placements/quay.yaml");
    let text = fs::read_to_string(&placements)
        .expect("placement fixture")
        .replace("script_id: Arrival", "script_id: Maps\\Arrival");
    fs::write(&placements, text).expect("write invalid locator fixture");

    let error = compile::build(&common::options(source), &workspace.path().join("pack"))
        .expect_err("source-shaped locator component must fail");
    assert!(error.to_string().contains("a path separator"), "{error:#}");
}

#[test]
fn locator_map_resource_must_match_the_placement_map() {
    let workspace = tempfile::tempdir().expect("temp dir");
    let source = common::write_source(&workspace.path().join("src"));
    write_rows(&source);
    let placements = source.join("classic/zones/harbour-watch/spawns/placements/quay.yaml");
    let text = fs::read_to_string(&placements)
        .expect("placement fixture")
        .replace("map_resource: quay", "map_resource: tide-steps");
    fs::write(&placements, text).expect("write map mismatch fixture");

    let error = compile::build(&common::options(source), &workspace.path().join("pack"))
        .expect_err("map identity mismatch must fail");
    assert!(
        error
            .to_string()
            .contains("map_resource \"tide-steps\" differs from map \"quay\""),
        "{error:#}"
    );
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
    let placements = source.join("classic/zones/harbour-watch/spawns/placements/quay.yaml");
    let mut placement_text = fs::read_to_string(&placements).expect("placement fixture");
    placement_text.push_str(
        "map_resource: quay\nlocators:\n  - script_id: Arrival\n    position: { x: 11.0, y: 12.0, z: 13.0 }\n  - script_id: Firewall\n    position: { x: 21.0, y: 22.0, z: 23.0 }\n",
    );
    fs::write(&placements, placement_text).expect("locator placement fixture");
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
                    id: quay
                    row_type: map
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
