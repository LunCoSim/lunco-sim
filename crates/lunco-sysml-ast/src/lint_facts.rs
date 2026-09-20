//! Typed facts for the authored SysML lint policy.
//!
//! Parsing and resolution stay in [`crate::SysmlAnalysis`].  This module only
//! projects that immutable snapshot into the neutral hook value consumed by
//! `assets/scripting/policy/lint_sysml.rhai`; it does not decide severity and
//! it does not execute a requirement or verification.

use crate::{
    SysmlAnalysis, SysmlAttribute, SysmlConstraint, SysmlDiagnostic, SysmlElement, SysmlLiteral,
    SysmlReference, SysmlRelationship, SysmlRequirementRecord, SysmlSubject, SysmlType,
    SysmlVerificationRecord,
};
use lunco_hooks::HookValue as H;
use std::collections::BTreeSet;

/// A source-neutral SysML fact table that can be requested by a policy.
///
/// This selector controls transport volume only. It does not encode domain
/// interpretation: the caller still decides what records mean and how they
/// are checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SysmlFactTable {
    Elements,
    References,
    Relationships,
    Constraints,
    Attributes,
    Requirements,
    Verifications,
    Diagnostics,
}

impl SysmlFactTable {
    /// Parse one stable API table name.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "elements" => Self::Elements,
            "references" => Self::References,
            "relationships" => Self::Relationships,
            "constraints" => Self::Constraints,
            "attributes" => Self::Attributes,
            "requirements" => Self::Requirements,
            "verifications" => Self::Verifications,
            "diagnostics" => Self::Diagnostics,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Elements => "elements",
            Self::References => "references",
            Self::Relationships => "relationships",
            Self::Constraints => "constraints",
            Self::Attributes => "attributes",
            Self::Requirements => "requirements",
            Self::Verifications => "verifications",
            Self::Diagnostics => "diagnostics",
        }
    }
}

/// Generic table/name selection for a typed SysML snapshot.
///
/// `tables: None` returns the full fact set. Name selectors accept either an
/// exact qualified identity or a local name and intentionally return every
/// match; callers such as requirement policies own ambiguity handling.
#[derive(Debug, Clone, Default)]
pub struct SysmlFactSelection {
    pub tables: Option<BTreeSet<SysmlFactTable>>,
    /// Attribute names: exact qualified identities or local names.
    pub attribute_names: Option<BTreeSet<String>>,
    /// Exact qualified attribute owners.
    pub attribute_owners: Option<BTreeSet<String>>,
    /// Exact values of typed SysML string literals.
    pub attribute_string_values: Option<BTreeSet<String>>,
    /// Requirement identities: exact qualified identities or local names.
    pub requirement_names: Option<BTreeSet<String>>,
    /// Verification identities: exact qualified identities or local names.
    pub verification_names: Option<BTreeSet<String>>,
}

fn selected_identity(names: &Option<BTreeSet<String>>, qualified_name: &str) -> bool {
    names.as_ref().map_or(true, |names| {
        let local_name = qualified_name.rsplit("::").next().unwrap_or(qualified_name);
        names.contains(qualified_name) || names.contains(local_name)
    })
}

/// Project only requested tables from one immutable analysis. Source identity
/// metadata is always present so policy results can cite their inputs.
pub fn selected_sysml_facts(analysis: &SysmlAnalysis, selection: &SysmlFactSelection) -> H {
    let mut facts = vec![
        (
            "source_revision_hex",
            H::str(format!("0x{:016x}", analysis.source_revision())),
        ),
        ("stdlib", H::Bool(analysis.includes_stdlib())),
        (
            "source_files",
            H::Array(
                analysis
                    .files()
                    .iter()
                    .map(|file| H::str(file.name.clone()))
                    .collect(),
            ),
        ),
    ];

    let includes = |table| {
        selection
            .tables
            .as_ref()
            .map_or(true, |tables| tables.contains(&table))
    };

    if includes(SysmlFactTable::Elements) {
        facts.push((
            "elements",
            H::Array(analysis.elements().iter().map(element).collect()),
        ));
    }
    if includes(SysmlFactTable::References) {
        facts.push((
            "references",
            H::Array(analysis.references().iter().map(reference).collect()),
        ));
    }
    if includes(SysmlFactTable::Relationships) {
        facts.push((
            "relationships",
            H::Array(analysis.relationships().iter().map(relationship).collect()),
        ));
    }
    if includes(SysmlFactTable::Constraints) {
        facts.push((
            "constraints",
            H::Array(analysis.constraints().iter().map(constraint).collect()),
        ));
    }
    if includes(SysmlFactTable::Attributes) {
        facts.push((
            "attributes",
            H::Array(
                analysis
                    .attributes()
                    .iter()
                    .filter(|record| {
                        selected_identity(&selection.attribute_names, &record.qualified_name)
                            && selection
                                .attribute_owners
                                .as_ref()
                                .map_or(true, |owners| owners.contains(&record.owner))
                            && selection
                                .attribute_string_values
                                .as_ref()
                                .map_or(true, |values| {
                                    record
                                        .value
                                        .as_ref()
                                        .and_then(|value| value.string_value.as_ref())
                                        .is_some_and(|value| values.contains(value))
                                })
                    })
                    .map(attribute)
                    .collect(),
            ),
        ));
    }
    if includes(SysmlFactTable::Requirements) {
        facts.push((
            "requirements",
            H::Array(
                analysis
                    .requirements()
                    .iter()
                    .filter(|record| {
                        selected_identity(
                            &selection.requirement_names,
                            &record.element.qualified_name,
                        )
                    })
                    .map(requirement)
                    .collect(),
            ),
        ));
    }
    if includes(SysmlFactTable::Verifications) {
        facts.push((
            "verifications",
            H::Array(
                analysis
                    .verifications()
                    .iter()
                    .filter(|record| {
                        selected_identity(
                            &selection.verification_names,
                            &record.element.qualified_name,
                        )
                    })
                    .map(verification)
                    .collect(),
            ),
        ));
    }
    if includes(SysmlFactTable::Diagnostics) {
        facts.push((
            "diagnostics",
            H::Array(analysis.diagnostics().iter().map(diagnostic).collect()),
        ));
    }
    H::map(facts)
}

/// Project one resolved SysML snapshot into top-level lint facts.
///
/// The shape is deliberately stable and lossless enough for structural rules:
/// source identity, elements, references, typed attributes, requirements,
/// verification cases, and parser diagnostics are all retained.  Rules should
/// use the qualified names and source-backed spans rather than reparsing text.
pub fn sysml_facts(analysis: &SysmlAnalysis) -> H {
    H::map([
        (
            "source_revision_hex",
            H::str(format!("0x{:016x}", analysis.source_revision())),
        ),
        ("stdlib", H::Bool(analysis.includes_stdlib())),
        (
            "source_files",
            H::Array(
                analysis
                    .files()
                    .iter()
                    .map(|file| H::str(file.name.clone()))
                    .collect(),
            ),
        ),
        (
            "elements",
            H::Array(analysis.elements().iter().map(element).collect()),
        ),
        (
            "references",
            H::Array(analysis.references().iter().map(reference).collect()),
        ),
        (
            "relationships",
            H::Array(analysis.relationships().iter().map(relationship).collect()),
        ),
        (
            "constraints",
            H::Array(analysis.constraints().iter().map(constraint).collect()),
        ),
        (
            "attributes",
            H::Array(analysis.attributes().iter().map(attribute).collect()),
        ),
        (
            "requirements",
            H::Array(analysis.requirements().iter().map(requirement).collect()),
        ),
        (
            "verifications",
            H::Array(analysis.verifications().iter().map(verification).collect()),
        ),
        (
            "diagnostics",
            H::Array(analysis.diagnostics().iter().map(diagnostic).collect()),
        ),
    ])
}

fn element(value: &SysmlElement) -> H {
    H::map([
        ("id", H::Int(i64::from(value.id))),
        ("file", H::str(value.file.clone())),
        ("qualified_name", H::str(value.qualified_name.clone())),
        ("kind", H::str(value.kind.clone())),
        ("start", H::Int(i64::from(value.start))),
        ("end", H::Int(i64::from(value.end))),
    ])
}

fn reference(value: &SysmlReference) -> H {
    H::map([
        ("file", H::str(value.file.clone())),
        ("start", H::Int(i64::from(value.start))),
        ("end", H::Int(i64::from(value.end))),
        ("name", H::str(value.name.clone())),
        ("from", H::str(value.from.clone())),
        ("target", H::str(value.target.clone())),
    ])
}

fn relationship(value: &SysmlRelationship) -> H {
    H::map([
        ("element", element(&value.element)),
        (
            "properties",
            H::Array(
                value
                    .properties
                    .iter()
                    .map(|property| {
                        H::map([
                            ("name", H::str(property.name.clone())),
                            ("targets", strings(&property.targets)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn constraint(value: &SysmlConstraint) -> H {
    H::map([
        ("element", element(&value.element)),
        (
            "expression",
            H::str(value.expression.clone().unwrap_or_default()),
        ),
    ])
}

fn attribute(value: &SysmlAttribute) -> H {
    H::map([
        ("owner", H::str(value.owner.clone())),
        ("name", H::str(value.name.clone())),
        ("qualified_name", H::str(value.qualified_name.clone())),
        (
            "type_name",
            H::str(value.type_name.clone().unwrap_or_default()),
        ),
        (
            "declared_type",
            value
                .declared_type
                .as_ref()
                .map(type_facts)
                .unwrap_or(H::Unit),
        ),
        (
            "value",
            value.value.as_ref().map(literal).unwrap_or(H::Unit),
        ),
        ("file", H::str(value.file.clone())),
        ("start", H::Int(i64::from(value.start))),
        ("end", H::Int(i64::from(value.end))),
    ])
}

fn literal(value: &SysmlLiteral) -> H {
    let mut facts = vec![
        ("literal", H::str(value.literal.clone())),
        ("kind", H::str(value.kind.clone())),
        ("literal_kind", H::str(value.literal_kind.as_str())),
        ("number", H::str(value.number.clone().unwrap_or_default())),
        (
            "integer_value",
            value.integer_value.map(H::Int).unwrap_or(H::Unit),
        ),
        (
            "boolean_value",
            value.boolean_value.map(H::Bool).unwrap_or(H::Unit),
        ),
        (
            "string_value",
            value
                .string_value
                .as_ref()
                .map(|string| H::str(string.clone()))
                .unwrap_or(H::Unit),
        ),
        (
            "unit",
            value
                .unit
                .as_ref()
                .map(|unit| H::str(unit.clone()))
                .unwrap_or(H::Unit),
        ),
    ];
    if let Some(number) = value.number_value {
        facts.push(("number_value", H::Float(number.as_f64())));
    }
    facts.push((
        "elements",
        value
            .elements
            .as_ref()
            .map(|elements| H::Array(elements.iter().map(literal).collect()))
            .unwrap_or(H::Unit),
    ));
    H::map(facts)
}

fn type_facts(value: &SysmlType) -> H {
    let mut facts = vec![
        ("base", H::str(value.base.clone())),
        ("category", H::str(format!("{:?}", value.category))),
        (
            "primitive",
            value
                .primitive
                .map(|primitive| H::str(format!("{:?}", primitive)))
                .unwrap_or(H::Unit),
        ),
        (
            "dimensions",
            H::Array(
                value
                    .dimensions
                    .iter()
                    .map(|dimension| H::Int(*dimension as i64))
                    .collect(),
            ),
        ),
        (
            "multiplicity",
            H::map([
                ("lower", H::Int(value.multiplicity.lower as i64)),
                (
                    "upper",
                    value
                        .multiplicity
                        .upper
                        .map(|upper| H::Int(upper as i64))
                        .unwrap_or(H::Unit),
                ),
                ("ordered", H::Bool(value.multiplicity.ordered)),
                ("unique", H::Bool(value.multiplicity.unique)),
            ]),
        ),
        (
            "quantity_kind",
            value
                .quantity_kind
                .as_ref()
                .map(|kind| H::str(kind.clone()))
                .unwrap_or(H::Unit),
        ),
        (
            "unit",
            value
                .unit
                .as_ref()
                .map(|unit| H::str(unit.clone()))
                .unwrap_or(H::Unit),
        ),
        (
            "modelica_type",
            H::str(format!("{:?}", value.modelica_type())),
        ),
    ];
    facts.shrink_to_fit();
    H::map(facts)
}

fn subject(value: &SysmlSubject) -> H {
    H::map([
        ("name", H::str(value.name.clone())),
        (
            "type_name",
            H::str(value.type_name.clone().unwrap_or_default()),
        ),
    ])
}

fn subjects(values: &[SysmlSubject]) -> H {
    H::Array(values.iter().map(subject).collect())
}

fn strings(values: &[String]) -> H {
    H::Array(values.iter().cloned().map(H::str).collect())
}

fn requirement(value: &SysmlRequirementRecord) -> H {
    H::map([
        ("element", element(&value.element)),
        (
            "qualified_name",
            H::str(value.element.qualified_name.clone()),
        ),
        ("file", H::str(value.element.file.clone())),
        ("kind", H::str(value.element.kind.clone())),
        ("documentation", strings(&value.documentation)),
        ("subjects", subjects(&value.subjects)),
        (
            "attributes",
            H::Array(value.attributes.iter().map(attribute).collect()),
        ),
        ("verifies", strings(&value.verifies)),
        ("satisfies", strings(&value.satisfies)),
        ("realizations", strings(&value.realizations)),
    ])
}

fn verification(value: &SysmlVerificationRecord) -> H {
    H::map([
        ("element", element(&value.element)),
        (
            "qualified_name",
            H::str(value.element.qualified_name.clone()),
        ),
        ("file", H::str(value.element.file.clone())),
        ("kind", H::str(value.element.kind.clone())),
        ("documentation", strings(&value.documentation)),
        ("subjects", subjects(&value.subjects)),
        ("verifies", strings(&value.verifies)),
        ("realizations", strings(&value.realizations)),
    ])
}

fn diagnostic(value: &SysmlDiagnostic) -> H {
    H::map([
        ("file", H::str(value.file.clone())),
        ("kind", H::str(format!("{:?}", value.kind))),
        ("start", H::Int(i64::from(value.start))),
        ("end", H::Int(i64::from(value.end))),
        ("message", H::str(value.message.clone())),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_keep_qualified_requirement_and_verification_identity() {
        let analysis = SysmlAnalysis::from_files([(
            "example.sysml",
            "package Example { requirement def R { doc /* documented */ } verification def V { subject x : A; verify R; } }",
        )]);
        let H::Map(facts) = sysml_facts(&analysis) else {
            panic!("facts must be a map");
        };
        let requirements = facts
            .iter()
            .find(|(key, _)| key == "requirements")
            .map(|(_, value)| value)
            .expect("requirements");
        let H::Array(requirements) = requirements else {
            panic!("requirements must be an array");
        };
        assert!(requirements.iter().any(|value| {
            value
                .get("qualified_name")
                .and_then(H::as_str)
                .is_some_and(|name| name.ends_with("::R"))
        }));
    }

    #[test]
    fn facts_keep_typed_attribute_shape_for_rhai_policy() {
        let analysis = SysmlAnalysis::from_files_without_stdlib([(
            "typed.sysml",
            "part def A { attribute stations : Real[3] = (1.0, 2.0, 3.0); }",
        )]);
        let H::Map(facts) = sysml_facts(&analysis) else {
            panic!("facts must be a map");
        };
        let H::Array(attributes) = facts
            .iter()
            .find(|(key, _)| key == "attributes")
            .map(|(_, value)| value)
            .expect("attributes")
        else {
            panic!("attributes must be an array");
        };
        let station = attributes.first().expect("station attribute");
        let declared = station.get("declared_type").expect("declared type");
        assert_eq!(declared.get("base").and_then(H::as_str), Some("Real"));
        assert_eq!(
            declared.get("modelica_type").and_then(H::as_str),
            Some("RealArray")
        );
        let value = station.get("value").expect("literal");
        assert_eq!(
            value.get("literal_kind").and_then(H::as_str),
            Some("vector")
        );
        assert!(matches!(value.get("elements"), Some(H::Array(elements)) if elements.len() == 3));
    }
}
