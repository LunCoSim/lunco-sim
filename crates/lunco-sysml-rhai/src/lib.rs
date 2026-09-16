//! Read-only Rhai reporting for SysML v2 analysis.
//!
//! Rhai owns test/report policy in LunCoSim. This adapter only exposes a
//! serialized semantic snapshot; it never parses source or mutates a
//! document, keeping the language boundary small and deterministic.

use std::sync::Arc;

use lunco_sysml_ast::{SysmlAnalysis, SysmlAttribute, SysmlDiagnostic, SysmlElement, SysmlSubject};
use rhai::{Dynamic, Map};

/// A requirement declaration/usage projected for a Rhai test report.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SysmlRequirement {
    /// Root-qualified requirement name.
    pub qualified_name: String,
    /// Logical source file containing the requirement.
    pub file: String,
    /// Declaration byte-range start.
    pub start: u32,
    /// Declaration byte-range end.
    pub end: u32,
    /// Upstream metamodel kind.
    pub kind: String,
    /// Documentation blocks owned by the requirement.
    pub documentation: Vec<String>,
    /// Requirement subjects.
    pub subjects: Vec<SysmlSubject>,
    /// Authored attributes and their literal values.
    pub attributes: Vec<SysmlAttribute>,
    /// Requirements named by `verify` memberships.
    pub verifies: Vec<String>,
    /// Written satisfaction targets.
    pub satisfies: Vec<String>,
    /// Written realization targets.
    pub realizations: Vec<String>,
}

/// A verification case projected for a Rhai test report.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SysmlVerification {
    /// Root-qualified verification case name.
    pub qualified_name: String,
    /// Logical source file containing the case.
    pub file: String,
    /// Declaration byte-range start.
    pub start: u32,
    /// Declaration byte-range end.
    pub end: u32,
    /// Upstream metamodel kind.
    pub kind: String,
    /// Documentation blocks owned by the case.
    pub documentation: Vec<String>,
    /// Verification subjects.
    pub subjects: Vec<SysmlSubject>,
    /// Requirements named by `verify` memberships.
    pub verifies: Vec<String>,
    /// Written realization targets.
    pub realizations: Vec<String>,
}

/// Compact, deterministic requirement report consumed by authored Rhai tests.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SysmlRequirementReport {
    /// Source generation tested.
    pub source_revision: u64,
    /// Logical files included in the resolved source set.
    pub source_files: Vec<String>,
    /// Typed attributes retained for requirement thresholds and units.
    pub attributes: Vec<SysmlAttribute>,
    /// Requirements found in project files.
    pub requirements: Vec<SysmlRequirement>,
    /// Verification-case definitions/usages in the source set.
    pub verifications: Vec<SysmlVerification>,
    /// Parser/resolution diagnostics that a test may gate on.
    pub diagnostics: Vec<SysmlDiagnostic>,
}

/// Produce a stable JSON report for a SysML analysis snapshot.
pub fn report_json(analysis: &SysmlAnalysis) -> String {
    serde_json::to_string(analysis).expect("SysML analysis projection is serializable")
}

/// Extract requirement declarations/usages from an immutable analysis.
pub fn requirements(analysis: &SysmlAnalysis) -> Vec<SysmlRequirement> {
    analysis
        .requirements()
        .iter()
        .map(requirement_from_record)
        .collect()
}

/// Extract verification cases from an immutable analysis.
pub fn verifications(analysis: &SysmlAnalysis) -> Vec<SysmlVerification> {
    analysis
        .verifications()
        .iter()
        .map(|record| SysmlVerification {
            qualified_name: record.element.qualified_name.clone(),
            file: record.element.file.clone(),
            start: record.element.start,
            end: record.element.end,
            kind: record.element.kind.clone(),
            documentation: record.documentation.clone(),
            subjects: record.subjects.clone(),
            verifies: record.verifies.clone(),
            realizations: record.realizations.clone(),
        })
        .collect()
}

/// Produce the compact requirement/test input report as JSON.
pub fn requirement_report_json(analysis: &SysmlAnalysis) -> String {
    let report = SysmlRequirementReport {
        source_revision: analysis.source_revision(),
        source_files: analysis
            .files()
            .iter()
            .map(|file| file.name.clone())
            .collect(),
        attributes: analysis.attributes().to_vec(),
        requirements: requirements(analysis),
        verifications: verifications(analysis),
        diagnostics: analysis.diagnostics().to_vec(),
    };
    serde_json::to_string(&report).expect("SysML requirement report is serializable")
}

/// Produce a native Rhai map for a complete immutable SysML snapshot.
///
/// This is the preferred in-process path: every field is constructed directly
/// as a Rhai value, so callers do not serialize to JSON and immediately parse
/// the same data back into maps.  Source generations are exposed as canonical
/// decimal text plus hexadecimal text: Rhai's signed integer is not a lossless
/// representation of the full u64 identity, so the bridge never narrows it.
pub fn report_dynamic(analysis: &SysmlAnalysis) -> Dynamic {
    let mut report = Map::new();
    report.insert(
        "source_revision_hex".into(),
        Dynamic::from(format!("0x{:016x}", analysis.source_revision())),
    );
    report.insert(
        "source_revision".into(),
        Dynamic::from(analysis.source_revision().to_string()),
    );
    report.insert(
        "stdlib".into(),
        Dynamic::from_bool(analysis.includes_stdlib()),
    );
    report.insert(
        "files".into(),
        Dynamic::from_array(
            analysis
                .files()
                .iter()
                .map(|file| {
                    let mut value = Map::new();
                    value.insert("name".into(), Dynamic::from(file.name.clone()));
                    value.insert("text".into(), Dynamic::from(file.text.clone()));
                    Dynamic::from_map(value)
                })
                .collect(),
        ),
    );
    report.insert(
        "elements".into(),
        Dynamic::from_array(analysis.elements().iter().map(element_dynamic).collect()),
    );
    report.insert(
        "references".into(),
        Dynamic::from_array(
            analysis
                .references()
                .iter()
                .map(reference_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "attributes".into(),
        Dynamic::from_array(
            analysis
                .attributes()
                .iter()
                .map(attribute_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "requirements".into(),
        Dynamic::from_array(
            analysis
                .requirements()
                .iter()
                .map(requirement_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "verifications".into(),
        Dynamic::from_array(
            analysis
                .verifications()
                .iter()
                .map(verification_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "diagnostics".into(),
        Dynamic::from_array(
            analysis
                .diagnostics()
                .iter()
                .map(diagnostic_dynamic)
                .collect(),
        ),
    );
    Dynamic::from_map(report)
}

/// Produce the compact native Rhai requirement/verification projection.
pub fn requirement_report_dynamic(analysis: &SysmlAnalysis) -> Dynamic {
    let mut report = Map::new();
    report.insert(
        "source_revision_hex".into(),
        Dynamic::from(format!("0x{:016x}", analysis.source_revision())),
    );
    report.insert(
        "source_revision".into(),
        Dynamic::from(analysis.source_revision().to_string()),
    );
    report.insert(
        "stdlib".into(),
        Dynamic::from_bool(analysis.includes_stdlib()),
    );
    report.insert(
        "source_files".into(),
        Dynamic::from_array(
            analysis
                .files()
                .iter()
                .map(|file| Dynamic::from(file.name.clone()))
                .collect(),
        ),
    );
    report.insert("attributes".into(), attributes_short_dynamic(analysis));
    report.insert(
        "attributes_qualified".into(),
        attributes_qualified_dynamic(analysis),
    );
    report.insert(
        "attribute_collisions".into(),
        attribute_collisions_dynamic(analysis),
    );
    report.insert(
        "requirements".into(),
        Dynamic::from_array(
            analysis
                .requirements()
                .iter()
                .map(requirement_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "verifications".into(),
        Dynamic::from_array(
            analysis
                .verifications()
                .iter()
                .map(verification_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "diagnostics".into(),
        Dynamic::from_array(
            analysis
                .diagnostics()
                .iter()
                .map(diagnostic_dynamic)
                .collect(),
        ),
    );
    Dynamic::from_map(report)
}

/// Register the read-only `sysml_report_json()` function in a Rhai engine.
///
/// A snapshot is captured by `Arc`, so script execution does not borrow a
/// Bevy world or a live document. Callers can create a fresh registration when
/// a `DocumentChanged` event publishes a newer generation.
pub fn register_sysml_report(engine: &mut rhai::Engine, analysis: Arc<SysmlAnalysis>) {
    let json_report = Arc::clone(&analysis);
    let dynamic_report = Arc::clone(&analysis);
    let compact_report = Arc::clone(&analysis);
    let json_compact_report = Arc::clone(&analysis);
    engine.register_fn("sysml_report", move || report_dynamic(&dynamic_report));
    engine.register_fn("sysml_requirement_report", move || {
        requirement_report_dynamic(&compact_report)
    });
    engine.register_fn("sysml_report_json", move || report_json(&json_report));
    engine.register_fn("sysml_requirement_report_json", move || {
        requirement_report_json(&json_compact_report)
    });
}

fn string_array(values: &[String]) -> Dynamic {
    Dynamic::from_array(values.iter().cloned().map(Dynamic::from).collect())
}

fn element_dynamic(element: &SysmlElement) -> Dynamic {
    let mut value = Map::new();
    value.insert("id".into(), Dynamic::from_int(element.id as i64));
    value.insert("file".into(), Dynamic::from(element.file.clone()));
    value.insert(
        "qualified_name".into(),
        Dynamic::from(element.qualified_name.clone()),
    );
    value.insert("kind".into(), Dynamic::from(element.kind.clone()));
    value.insert("start".into(), Dynamic::from_int(element.start as i64));
    value.insert("end".into(), Dynamic::from_int(element.end as i64));
    Dynamic::from_map(value)
}

fn reference_dynamic(reference: &lunco_sysml_ast::SysmlReference) -> Dynamic {
    let mut value = Map::new();
    value.insert("file".into(), Dynamic::from(reference.file.clone()));
    value.insert("start".into(), Dynamic::from_int(reference.start as i64));
    value.insert("end".into(), Dynamic::from_int(reference.end as i64));
    value.insert("name".into(), Dynamic::from(reference.name.clone()));
    value.insert("target".into(), Dynamic::from(reference.target.clone()));
    Dynamic::from_map(value)
}

fn subject_array(values: &[SysmlSubject]) -> Dynamic {
    Dynamic::from_array(
        values
            .iter()
            .map(|subject| {
                let mut value = Map::new();
                value.insert("name".into(), Dynamic::from(subject.name.clone()));
                if let Some(type_name) = &subject.type_name {
                    value.insert("type_name".into(), Dynamic::from(type_name.clone()));
                }
                Dynamic::from_map(value)
            })
            .collect(),
    )
}

fn optional_string(value: &Option<String>) -> Option<Dynamic> {
    value.as_ref().map(|value| Dynamic::from(value.clone()))
}

fn literal_dynamic(literal: &lunco_sysml_ast::SysmlLiteral) -> Dynamic {
    let mut value = Map::new();
    value.insert("literal".into(), Dynamic::from(literal.literal.clone()));
    value.insert("kind".into(), Dynamic::from(literal.kind.clone()));
    if let Some(number) = &literal.number {
        value.insert("number".into(), Dynamic::from(number.clone()));
    }
    if let Some(number) = literal.number_value {
        value.insert("number_value".into(), Dynamic::from_float(number.as_f64()));
    }
    Dynamic::from_map(value)
}

fn attribute_dynamic(attribute: &SysmlAttribute) -> Dynamic {
    let mut value = Map::new();
    value.insert("owner".into(), Dynamic::from(attribute.owner.clone()));
    value.insert("name".into(), Dynamic::from(attribute.name.clone()));
    value.insert(
        "qualified_name".into(),
        Dynamic::from(attribute.qualified_name.clone()),
    );
    if let Some(type_name) = optional_string(&attribute.type_name) {
        value.insert("type_name".into(), type_name);
    }
    if let Some(literal) = &attribute.value {
        value.insert("value".into(), literal_dynamic(literal));
    }
    value.insert("file".into(), Dynamic::from(attribute.file.clone()));
    value.insert("start".into(), Dynamic::from_int(attribute.start as i64));
    value.insert("end".into(), Dynamic::from_int(attribute.end as i64));
    Dynamic::from_map(value)
}

fn attributes_qualified_dynamic(analysis: &SysmlAnalysis) -> Dynamic {
    let mut output = Map::new();
    for attribute in analysis.attributes() {
        output.insert(
            attribute.qualified_name.clone().into(),
            attribute_dynamic(attribute),
        );
    }
    Dynamic::from_map(output)
}

fn attributes_short_dynamic(analysis: &SysmlAnalysis) -> Dynamic {
    let mut output = Map::new();
    for attribute in analysis.attributes() {
        output.insert(attribute.name.clone().into(), attribute_dynamic(attribute));
    }
    Dynamic::from_map(output)
}

fn attribute_collisions_dynamic(analysis: &SysmlAnalysis) -> Dynamic {
    let mut names = std::collections::BTreeMap::<String, Vec<String>>::new();
    for attribute in analysis.attributes() {
        names
            .entry(attribute.name.clone())
            .or_default()
            .push(attribute.qualified_name.clone());
    }
    Dynamic::from_array(
        names
            .into_iter()
            .filter_map(|(name, qualified_names)| {
                if qualified_names.len() < 2 {
                    return None;
                }
                let mut value = Map::new();
                value.insert("name".into(), Dynamic::from(name));
                value.insert("qualified_names".into(), string_array(&qualified_names));
                Some(Dynamic::from_map(value))
            })
            .collect(),
    )
}

fn requirement_dynamic(record: &lunco_sysml_ast::SysmlRequirementRecord) -> Dynamic {
    let mut value = Map::new();
    value.insert("element".into(), element_dynamic(&record.element));
    value.insert("documentation".into(), string_array(&record.documentation));
    value.insert("subjects".into(), subject_array(&record.subjects));
    value.insert(
        "attributes".into(),
        Dynamic::from_array(record.attributes.iter().map(attribute_dynamic).collect()),
    );
    value.insert("verifies".into(), string_array(&record.verifies));
    value.insert("satisfies".into(), string_array(&record.satisfies));
    value.insert("realizations".into(), string_array(&record.realizations));
    Dynamic::from_map(value)
}

fn verification_dynamic(record: &lunco_sysml_ast::SysmlVerificationRecord) -> Dynamic {
    let mut value = Map::new();
    value.insert("element".into(), element_dynamic(&record.element));
    value.insert("documentation".into(), string_array(&record.documentation));
    value.insert("subjects".into(), subject_array(&record.subjects));
    value.insert("verifies".into(), string_array(&record.verifies));
    value.insert("realizations".into(), string_array(&record.realizations));
    Dynamic::from_map(value)
}

fn diagnostic_dynamic(diagnostic: &SysmlDiagnostic) -> Dynamic {
    let mut value = Map::new();
    value.insert("file".into(), Dynamic::from(diagnostic.file.clone()));
    value.insert(
        "kind".into(),
        Dynamic::from(format!("{:?}", diagnostic.kind)),
    );
    value.insert("start".into(), Dynamic::from_int(diagnostic.start as i64));
    value.insert("end".into(), Dynamic::from_int(diagnostic.end as i64));
    value.insert("message".into(), Dynamic::from(diagnostic.message.clone()));
    Dynamic::from_map(value)
}

fn requirement_from_record(record: &lunco_sysml_ast::SysmlRequirementRecord) -> SysmlRequirement {
    let element: &SysmlElement = &record.element;
    SysmlRequirement {
        qualified_name: element.qualified_name.clone(),
        file: element.file.clone(),
        start: element.start,
        end: element.end,
        kind: element.kind.clone(),
        documentation: record.documentation.clone(),
        subjects: record.subjects.clone(),
        attributes: record.attributes.clone(),
        verifies: record.verifies.clone(),
        satisfies: record.satisfies.clone(),
        realizations: record.realizations.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_is_consumable_by_rhai() {
        let analysis = Arc::new(SysmlAnalysis::from_files([(
            "example.sysml",
            "requirement def MassRequirement {}",
        )]));
        let mut engine = rhai::Engine::new();
        register_sysml_report(&mut engine, analysis);
        let native: Dynamic = engine.eval("sysml_requirement_report()").unwrap();
        let native = native.cast::<Map>();
        assert!(native.contains_key("requirements"));
        let json: String = engine.eval("sysml_report_json()").unwrap();
        assert!(json.contains("example.sysml"));
        let requirements: String = engine.eval("sysml_requirement_report_json()").unwrap();
        assert!(requirements.contains("MassRequirement"));
    }

    #[test]
    fn native_report_exposes_numeric_literal_without_string_parsing() {
        let analysis = Arc::new(SysmlAnalysis::from_files_without_stdlib([(
            "numeric.sysml",
            "part def A { attribute mass : Real = 2.5; }",
        )]));
        let mut engine = rhai::Engine::new();
        register_sysml_report(&mut engine, analysis);
        let value: f64 = engine
            .eval("sysml_report().attributes[0].value.number_value")
            .expect("native numeric projection");
        assert_eq!(value, 2.5);
    }

    #[test]
    fn source_revision_is_lossless_text_in_native_reports() {
        let analysis = SysmlAnalysis::build(
            [("revision.sysml", "requirement def R {}")],
            false,
            u64::MAX,
        );
        let report = report_dynamic(&analysis);
        let report = report.cast::<Map>();
        assert_eq!(
            report["source_revision"]
                .clone()
                .into_immutable_string()
                .unwrap(),
            u64::MAX.to_string()
        );
        assert_eq!(
            report["source_revision_hex"]
                .clone()
                .into_immutable_string()
                .unwrap(),
            "0xffffffffffffffff"
        );
    }
}
