//! Read-only Rhai reporting for SysML v2 analysis.
//!
//! Rhai owns test/report policy in LunCoSim. This adapter only exposes a
//! serialized semantic snapshot; it never parses source or mutates a
//! document, keeping the language boundary small and deterministic.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::sync::Arc;

use lunco_sysml_ast::{SysmlAnalysis, SysmlAttribute, SysmlDiagnostic, SysmlElement, SysmlSubject};

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
        requirements: requirements(analysis),
        verifications: verifications(analysis),
        diagnostics: analysis.diagnostics().to_vec(),
    };
    serde_json::to_string(&report).expect("SysML requirement report is serializable")
}

/// Register the read-only `sysml_report_json()` function in a Rhai engine.
///
/// A snapshot is captured by `Arc`, so script execution does not borrow a
/// Bevy world or a live document. Callers can create a fresh registration when
/// a `DocumentChanged` event publishes a newer generation.
pub fn register_sysml_report(engine: &mut rhai::Engine, analysis: Arc<SysmlAnalysis>) {
    let report = Arc::clone(&analysis);
    engine.register_fn("sysml_report_json", move || report_json(&analysis));
    engine.register_fn("sysml_requirement_report_json", move || {
        requirement_report_json(&report)
    });
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
        let json: String = engine.eval("sysml_report_json()").unwrap();
        assert!(json.contains("example.sysml"));
        let requirements: String = engine.eval("sysml_requirement_report_json()").unwrap();
        assert!(requirements.contains("MassRequirement"));
    }
}
