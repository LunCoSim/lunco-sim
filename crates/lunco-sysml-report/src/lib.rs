//! Stable JSON projections for the pure SysML semantic snapshot.
//!
//! The parser (`lunco-sysml-ast`), runtime document (`lunco-sysml`), API
//! validator, and Rhai adapter have different dependency closures. This
//! crate owns only the lossless JSON shape shared at those boundaries; it has
//! no Bevy, filesystem, Twin, or scripting dependency.

use lunco_sysml_ast::{SysmlAnalysis, SysmlAttribute};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// Return attributes keyed by their local (short) name.
pub fn attributes(analysis: &SysmlAnalysis) -> Value {
    let mut output = Map::new();
    for attribute in analysis.attributes() {
        output.insert(attribute.name.clone(), attribute_record(attribute));
    }
    Value::Object(output)
}

/// Return the lossless attribute map keyed by qualified SysML name.
pub fn attributes_qualified(analysis: &SysmlAnalysis) -> Value {
    let mut output = Map::new();
    for attribute in analysis.attributes() {
        output.insert(
            attribute.qualified_name.clone(),
            attribute_record(attribute),
        );
    }
    Value::Object(output)
}

/// Return local-name collisions and their qualified alternatives.
pub fn attribute_collisions(analysis: &SysmlAnalysis) -> Value {
    let mut names = BTreeMap::<String, Vec<String>>::new();
    for attribute in analysis.attributes() {
        names
            .entry(attribute.name.clone())
            .or_default()
            .push(attribute.qualified_name.clone());
    }
    Value::Array(
        names
            .into_iter()
            .filter_map(|(name, qualified_names)| {
                (qualified_names.len() > 1).then_some(json!({
                    "name": name,
                    "qualified_names": qualified_names,
                }))
            })
            .collect(),
    )
}

/// Return one source-backed, typed attribute record.
pub fn attribute_record(attribute: &SysmlAttribute) -> Value {
    let value = attribute.value.as_ref().map(|literal| {
        let number = literal.number_value.map(|value| value.as_f64());
        json!({
            "literal": literal.literal,
            "kind": literal.kind,
            "number": number,
            "number_value": number,
        })
    });
    json!({
        "owner": attribute.owner,
        "name": attribute.name,
        "qualified_name": attribute.qualified_name,
        "type_name": attribute.type_name,
        "value": value,
        "file": attribute.file,
        "start": attribute.start,
        "end": attribute.end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_projection_reports_collisions_without_losing_values() {
        let analysis = SysmlAnalysis::from_files([(
            "example.sysml",
            "package Example { part def A { attribute mass : Real = 1.0; } part def B { attribute mass : Real = 2.0; } }",
        )]);
        assert!(attributes_qualified(&analysis)["Example::A::mass"].is_object());
        assert!(attributes_qualified(&analysis)["Example::B::mass"].is_object());
        assert_eq!(attribute_collisions(&analysis).as_array().unwrap().len(), 1);
    }
}
