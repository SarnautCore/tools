use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use roxmltree::Node;
use serde_json::{Map, Value};

use crate::model::{Orientation, Position, Provenance};
use crate::reference::source_path;

pub(crate) struct XdbFile {
    pub text: String,
    pub source: Provenance,
}

pub(crate) fn read_xdb(path: &Path, root: &Path) -> Result<XdbFile> {
    read_xdb_from(path, root, None)
}

/// Read one XDB, recording which canonical source root (ADR 0009) supplied it.
pub(crate) fn read_xdb_from(
    path: &Path,
    root: &Path,
    source_root: Option<&str>,
) -> Result<XdbFile> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let text =
        String::from_utf8(bytes).with_context(|| format!("decode {} as UTF-8", path.display()))?;
    Ok(XdbFile {
        text,
        source: Provenance {
            path: source_path(path, root)?,
            blake3: hash,
            extractor: format!("sarnaut-extract@{}", env!("CARGO_PKG_VERSION")),
            source_root: source_root.map(ToOwned::to_owned),
            prototype_chain: None,
        },
    })
}

/// The `href` on `Header/Prototype`, the XDB inheritance edge.
pub(crate) fn prototype_href(root: Node<'_, '_>) -> Option<String> {
    child(root, "Header").and_then(|header| href(header, "Prototype"))
}

/// Every `href` attribute below `node`, in document order.
pub(crate) fn descendant_hrefs(node: Node<'_, '_>) -> Vec<String> {
    node.descendants()
        .filter(Node::is_element)
        .filter_map(|element| element.attribute("href"))
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub(crate) fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == name)
}

pub(crate) fn children<'a, 'input>(
    node: Node<'a, 'input>,
    name: &'a str,
) -> impl Iterator<Item = Node<'a, 'input>> + 'a {
    node.children()
        .filter(move |child| child.is_element() && child.tag_name().name() == name)
}

pub(crate) fn text(node: Node<'_, '_>, name: &str) -> Option<String> {
    child(node, name)
        .and_then(|element| element.text())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn href(node: Node<'_, '_>, name: &str) -> Option<String> {
    child(node, name)
        .and_then(|element| element.attribute("href"))
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn i64_value(node: Node<'_, '_>, name: &str) -> Option<i64> {
    text(node, name).and_then(|value| value.parse().ok())
}

pub(crate) fn u64_value(node: Node<'_, '_>, name: &str) -> Option<u64> {
    text(node, name).and_then(|value| value.parse().ok())
}

pub(crate) fn f64_value(node: Node<'_, '_>, name: &str) -> Option<f64> {
    text(node, name).and_then(|value| value.parse().ok())
}

pub(crate) fn bool_value(node: Node<'_, '_>, name: &str) -> Option<bool> {
    text(node, name).and_then(|value| value.parse().ok())
}

pub(crate) fn resource_id(root: Node<'_, '_>) -> Option<u64> {
    child(root, "Header").and_then(|header| u64_value(header, "resourceId"))
}

pub(crate) fn position(node: Node<'_, '_>) -> Option<Position> {
    Some(Position {
        x: node.attribute("x")?.parse().ok()?,
        y: node.attribute("y")?.parse().ok()?,
        z: node.attribute("z")?.parse().ok()?,
    })
}

pub(crate) fn orientation(node: Node<'_, '_>) -> Option<Orientation> {
    Some(Orientation {
        yaw: f64_value(node, "yaw")?,
        pitch: f64_value(node, "pitch")?,
        roll: f64_value(node, "roll")?,
    })
}

pub(crate) fn extra_fields(root: Node<'_, '_>, excluded: &[&str]) -> BTreeMap<String, Value> {
    let excluded: BTreeSet<&str> = excluded.iter().copied().collect();
    let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for node in root.children().filter(Node::is_element) {
        if excluded.contains(node.tag_name().name()) {
            continue;
        }
        grouped
            .entry(node.tag_name().name().to_owned())
            .or_default()
            .push(node_value(node));
    }
    grouped
        .into_iter()
        .map(|(key, mut values)| {
            let value = if values.len() == 1 {
                values.pop().expect("one value")
            } else {
                Value::Array(values)
            };
            (key, value)
        })
        .collect()
}

pub(crate) fn node_value(node: Node<'_, '_>) -> Value {
    let elements: Vec<_> = node.children().filter(Node::is_element).collect();
    let attributes: Vec<_> = node.attributes().collect();
    let content = node
        .children()
        .filter(Node::is_text)
        .filter_map(|child| child.text())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("");

    if elements.is_empty() && attributes.is_empty() {
        return if content.is_empty() {
            Value::Null
        } else {
            Value::String(content)
        };
    }

    let mut object = Map::new();
    for attribute in attributes {
        object.insert(
            format!("@{}", attribute.name()),
            Value::String(attribute.value().to_owned()),
        );
    }
    if !content.is_empty() {
        object.insert("_text".into(), Value::String(content));
    }

    let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for element in elements {
        grouped
            .entry(element.tag_name().name().to_owned())
            .or_default()
            .push(node_value(element));
    }
    for (key, mut values) in grouped {
        object.insert(
            key,
            if values.len() == 1 {
                values.pop().expect("one value")
            } else {
                Value::Array(values)
            },
        );
    }
    Value::Object(object)
}

pub(crate) fn parse_document<'a>(text: &'a str, path: &Path) -> Result<roxmltree::Document<'a>> {
    roxmltree::Document::parse(text).with_context(|| format!("parse XML {}", path.display()))
}
