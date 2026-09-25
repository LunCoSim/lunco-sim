//! Typed facts for the authored SysML lint policy.
//!
//! Parsing and resolution stay in [`crate::SysmlAnalysis`].  This module only
//! projects that immutable snapshot into the neutral hook value consumed by
//! `assets/scripting/policy/lint_sysml.rhai`; it does not decide severity and
//! it does not execute a requirement or verification.

use crate::{
    SysmlAnalysis, SysmlAttribute, SysmlConstraint, SysmlConstraintBinding, SysmlDiagnostic,
    SysmlElement, SysmlElementHandle, SysmlExpression, SysmlExpressionKind,
    SysmlExpressionOperator, SysmlFeatureHandle, SysmlLiteral, SysmlReference, SysmlRelationship,
    SysmlRequirementRecord, SysmlStandardConstant, SysmlStandardFunction, SysmlSubject, SysmlType,
    SysmlTypeRef, SysmlUnsupportedExpression, SysmlVerificationRecord,
};
use lunco_hooks::HookValue as H;
use std::collections::{BTreeSet, HashMap, HashSet};

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

/// A bounded window over each selected fact table.
///
/// Pagination is transport-only: it preserves each table's authored order and
/// does not interpret the returned facts. The source revision in the enclosing
/// snapshot lets a caller reject an inconsistent multi-page read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SysmlFactPage {
    pub offset: usize,
    pub limit: usize,
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
/// `tables: None` returns every fact table unless a bounded page is requested.
/// Name selectors accept an exact qualified identity or local name and
/// intentionally return every match; callers such as requirement policies
/// own ambiguity handling.
#[derive(Debug, Clone, Default)]
pub struct SysmlFactSelection {
    pub tables: Option<BTreeSet<SysmlFactTable>>,
    /// Optional bounded page applied independently to each selected table.
    pub page: Option<SysmlFactPage>,
    /// Attribute names: exact qualified identities or local names.
    pub attribute_names: Option<BTreeSet<String>>,
    /// Snapshot-scoped attribute features selected by semantic feature handle.
    pub attribute_handles: Option<HashSet<SysmlFeatureHandle>>,
    /// Exact qualified attribute owners.
    pub attribute_owners: Option<BTreeSet<String>>,
    /// Snapshot-scoped attribute owners selected by semantic element handle.
    pub attribute_owner_handles: Option<HashSet<SysmlElementHandle>>,
    /// Exact values of typed SysML string literals.
    pub attribute_string_values: Option<BTreeSet<String>>,
    /// Requirement identities: exact qualified identities or local names.
    pub requirement_names: Option<BTreeSet<String>>,
    /// Verification identities: exact qualified identities or local names.
    pub verification_names: Option<BTreeSet<String>>,
    /// Names or standard short names of reference targets to include.
    pub reference_target_names: Option<BTreeSet<String>>,
    /// Snapshot-scoped reference targets selected by semantic element handle.
    pub reference_target_handles: Option<HashSet<SysmlElementHandle>>,
    /// Qualified or local names of the elements owning reference features.
    pub reference_from_owners: Option<BTreeSet<String>>,
    /// Snapshot-scoped reference owners selected by semantic element handle.
    pub reference_from_owner_handles: Option<HashSet<SysmlElementHandle>>,
    /// Local names of reference features to include.
    pub reference_from_features: Option<BTreeSet<String>>,
}

fn selected_identity(names: &Option<BTreeSet<String>>, qualified_name: &str) -> bool {
    names.as_ref().map_or(true, |names| {
        let local_name = qualified_name.rsplit("::").next().unwrap_or(qualified_name);
        names.contains(qualified_name) || names.contains(local_name)
    })
}

fn element_identity_matches(names: &BTreeSet<String>, element: &SysmlElement) -> bool {
    let local_name = element
        .qualified_name
        .rsplit("::")
        .next()
        .unwrap_or(&element.qualified_name);
    names.contains(&element.qualified_name)
        || names.contains(local_name)
        || element
            .short_name
            .as_ref()
            .is_some_and(|short_name| names.contains(short_name))
}

/// Project only requested tables from one immutable analysis. Source identity
/// metadata is always present so policy results can cite their inputs.
pub fn selected_sysml_facts(analysis: &SysmlAnalysis, selection: &SysmlFactSelection) -> H {
    let elements_by_handle = analysis
        .elements()
        .iter()
        .map(|element| (element.handle, element))
        .collect::<HashMap<_, _>>();
    let mut facts = vec![
        ("source_revision", H::UInt(analysis.source_revision())),
        ("source_fingerprint", H::UInt(analysis.source_fingerprint())),
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
    let mut page_tables = Vec::new();

    if includes(SysmlFactTable::Elements) {
        let (total, elements) = page_records(analysis.elements().iter().collect(), selection.page);
        facts.push((
            "elements",
            H::Array(elements.iter().map(|record| element(record)).collect()),
        ));
        page_tables.push((
            "elements",
            table_page(total, elements.len(), selection.page),
        ));
    }
    if includes(SysmlFactTable::References) {
        let selected = analysis
            .references()
            .iter()
            .filter(|reference| {
                let target = elements_by_handle.get(&reference.target).copied();
                let from_owner = reference
                    .from_owner
                    .and_then(|handle| elements_by_handle.get(&handle).copied());
                let from_feature = elements_by_handle.get(&reference.from).map(|element| {
                    element
                        .qualified_name
                        .rsplit("::")
                        .next()
                        .unwrap_or_default()
                });
                selection
                    .reference_target_names
                    .as_ref()
                    .map_or(true, |names| {
                        target.is_some_and(|element| element_identity_matches(names, element))
                    })
                    && selection
                        .reference_target_handles
                        .as_ref()
                        .map_or(true, |handles| handles.contains(&reference.target))
                    && selection
                        .reference_from_owners
                        .as_ref()
                        .map_or(true, |names| {
                            from_owner
                                .is_some_and(|element| element_identity_matches(names, element))
                        })
                    && selection
                        .reference_from_owner_handles
                        .as_ref()
                        .map_or(true, |handles| {
                            reference
                                .from_owner
                                .is_some_and(|owner| handles.contains(&owner))
                        })
                    && selection
                        .reference_from_features
                        .as_ref()
                        .map_or(true, |names| {
                            from_feature.is_some_and(|name| names.contains(name))
                        })
            })
            .collect();
        let (total, references) = page_records(selected, selection.page);
        facts.push((
            "references",
            H::Array(
                references
                    .iter()
                    .map(|record| reference(record, &elements_by_handle))
                    .collect(),
            ),
        ));
        page_tables.push((
            "references",
            table_page(total, references.len(), selection.page),
        ));
    }
    if includes(SysmlFactTable::Relationships) {
        let (total, relationships) =
            page_records(analysis.relationships().iter().collect(), selection.page);
        facts.push((
            "relationships",
            H::Array(
                relationships
                    .iter()
                    .map(|record| relationship(record))
                    .collect(),
            ),
        ));
        page_tables.push((
            "relationships",
            table_page(total, relationships.len(), selection.page),
        ));
    }
    if includes(SysmlFactTable::Constraints) {
        let (total, constraints) =
            page_records(analysis.constraints().iter().collect(), selection.page);
        facts.push((
            "constraints",
            H::Array(
                constraints
                    .iter()
                    .map(|record| constraint(record))
                    .collect(),
            ),
        ));
        page_tables.push((
            "constraints",
            table_page(total, constraints.len(), selection.page),
        ));
    }
    if includes(SysmlFactTable::Attributes) {
        let selected: Vec<_> = analysis
            .attributes()
            .iter()
            .filter(|record| {
                selected_identity(&selection.attribute_names, &record.qualified_name)
                    && selection
                        .attribute_handles
                        .as_ref()
                        .map_or(true, |handles| handles.contains(&record.handle))
                    && selection
                        .attribute_owners
                        .as_ref()
                        .map_or(true, |owners| owners.contains(&record.owner))
                    && selection
                        .attribute_owner_handles
                        .as_ref()
                        .map_or(true, |handles| {
                            record
                                .owner_handle
                                .is_some_and(|owner| handles.contains(&owner))
                        })
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
            .collect();
        let (total, attributes) = page_records(selected, selection.page);
        facts.push((
            "attributes",
            H::Array(attributes.iter().map(|record| attribute(record)).collect()),
        ));
        page_tables.push((
            "attributes",
            table_page(total, attributes.len(), selection.page),
        ));
    }
    if includes(SysmlFactTable::Requirements) {
        let selected: Vec<_> = analysis
            .requirements()
            .iter()
            .filter(|record| {
                selected_identity(&selection.requirement_names, &record.element.qualified_name)
            })
            .collect();
        let (total, requirements) = page_records(selected, selection.page);
        facts.push((
            "requirements",
            H::Array(
                requirements
                    .iter()
                    .map(|record| requirement(record))
                    .collect(),
            ),
        ));
        page_tables.push((
            "requirements",
            table_page(total, requirements.len(), selection.page),
        ));
    }
    if includes(SysmlFactTable::Verifications) {
        let selected: Vec<_> = analysis
            .verifications()
            .iter()
            .filter(|record| {
                selected_identity(
                    &selection.verification_names,
                    &record.element.qualified_name,
                )
            })
            .collect();
        let (total, verifications) = page_records(selected, selection.page);
        facts.push((
            "verifications",
            H::Array(
                verifications
                    .iter()
                    .map(|record| verification(record))
                    .collect(),
            ),
        ));
        page_tables.push((
            "verifications",
            table_page(total, verifications.len(), selection.page),
        ));
    }
    if includes(SysmlFactTable::Diagnostics) {
        let (total, diagnostics) =
            page_records(analysis.diagnostics().iter().collect(), selection.page);
        facts.push((
            "diagnostics",
            H::Array(
                diagnostics
                    .iter()
                    .map(|record| diagnostic(record))
                    .collect(),
            ),
        ));
        page_tables.push((
            "diagnostics",
            table_page(total, diagnostics.len(), selection.page),
        ));
    }
    let offset = selection.page.map_or(0, |page| page.offset);
    let limit = selection
        .page
        .map_or(H::Unit, |page| H::Int(page.limit as i64));
    facts.push((
        "page",
        H::map([
            ("offset", H::Int(offset as i64)),
            ("limit", limit),
            ("tables", H::map(page_tables)),
        ]),
    ));
    H::map(facts)
}

fn page_records<'a, T>(records: Vec<&'a T>, page: Option<SysmlFactPage>) -> (usize, Vec<&'a T>) {
    let total = records.len();
    let selected = match page {
        Some(page) => records
            .into_iter()
            .skip(page.offset)
            .take(page.limit)
            .collect(),
        None => records,
    };
    (total, selected)
}

fn table_page(total: usize, returned: usize, page: Option<SysmlFactPage>) -> H {
    let offset = page.map_or(0, |page| page.offset);
    H::map([
        ("offset", H::Int(offset as i64)),
        (
            "limit",
            page.map_or(H::Unit, |page| H::Int(page.limit as i64)),
        ),
        ("total", H::Int(total as i64)),
        ("returned", H::Int(returned as i64)),
        ("has_more", H::Bool(offset.saturating_add(returned) < total)),
    ])
}

/// Project one resolved SysML snapshot into top-level lint facts.
///
/// The shape is deliberately stable and lossless enough for structural rules:
/// source identity, elements, references, typed attributes, requirements,
/// verification cases, and parser diagnostics are all retained.  Rules should
/// use the qualified names and source-backed spans rather than reparsing text.
pub fn sysml_facts(analysis: &SysmlAnalysis) -> H {
    let elements_by_handle = analysis
        .elements()
        .iter()
        .map(|element| (element.handle, element))
        .collect::<HashMap<_, _>>();
    H::map([
        ("source_revision", H::UInt(analysis.source_revision())),
        ("source_fingerprint", H::UInt(analysis.source_fingerprint())),
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
            H::Array(
                analysis
                    .references()
                    .iter()
                    .map(|reference_value| reference(reference_value, &elements_by_handle))
                    .collect(),
            ),
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
        ("handle", element_handle(value.handle)),
        (
            "owner_handle",
            value.owner_handle.map(element_handle).unwrap_or(H::Unit),
        ),
        (
            "feature_handle",
            value.feature_handle.map(feature_handle).unwrap_or(H::Unit),
        ),
        ("id", H::Int(i64::from(value.id))),
        ("file", H::str(value.file.clone())),
        ("qualified_name", H::str(value.qualified_name.clone())),
        (
            "short_name",
            value
                .short_name
                .as_ref()
                .map(|name| H::str(name.clone()))
                .unwrap_or(H::Unit),
        ),
        ("kind", H::str(value.kind.clone())),
        ("start", H::Int(i64::from(value.start))),
        ("end", H::Int(i64::from(value.end))),
    ])
}

fn reference(value: &SysmlReference, elements: &HashMap<SysmlElementHandle, &SysmlElement>) -> H {
    let from_feature = elements.get(&value.from).copied();
    let from_owner = value
        .from_owner
        .and_then(|handle| elements.get(&handle).copied());
    let target = elements.get(&value.target).copied();
    H::map([
        ("file", H::str(value.file.clone())),
        ("start", H::Int(i64::from(value.start))),
        ("end", H::Int(i64::from(value.end))),
        ("name", H::str(value.name.clone())),
        ("from", element_handle(value.from)),
        (
            "from_owner",
            value.from_owner.map(element_handle).unwrap_or(H::Unit),
        ),
        (
            "from_feature_name",
            from_feature
                .and_then(|element| element.qualified_name.rsplit("::").next())
                .map(|name| H::str(name.to_owned()))
                .unwrap_or(H::Unit),
        ),
        (
            "from_owner_name",
            from_owner
                .map(|element| H::str(element.qualified_name.clone()))
                .unwrap_or(H::Unit),
        ),
        ("target", element_handle(value.target)),
        (
            "target_name",
            target
                .map(|element| H::str(element.qualified_name.clone()))
                .unwrap_or(H::Unit),
        ),
        (
            "target_short_name",
            target
                .and_then(|element| element.short_name.as_ref())
                .map(|name| H::str(name.clone()))
                .unwrap_or(H::Unit),
        ),
        (
            "target_feature",
            value.target_feature.map(feature_handle).unwrap_or(H::Unit),
        ),
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
                            (
                                "targets",
                                H::Array(
                                    property
                                        .targets
                                        .iter()
                                        .copied()
                                        .map(element_handle)
                                        .collect(),
                                ),
                            ),
                            (
                                "feature_targets",
                                H::Array(
                                    property
                                        .feature_targets
                                        .iter()
                                        .copied()
                                        .map(feature_handle)
                                        .collect(),
                                ),
                            ),
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
            "bindings",
            H::Array(value.bindings.iter().map(constraint_binding).collect()),
        ),
        (
            "expressions",
            H::Array(value.expressions.iter().map(expression).collect()),
        ),
    ])
}

fn constraint_binding(value: &SysmlConstraintBinding) -> H {
    H::map([
        ("formal_parameter", feature_handle(value.formal_parameter)),
        ("value", expression(&value.value)),
    ])
}

fn element_handle(value: SysmlElementHandle) -> H {
    H::map([
        ("source_revision", H::UInt(value.source_revision)),
        ("source_fingerprint", H::UInt(value.source_fingerprint)),
        ("element_id", H::UInt(u64::from(value.element_id))),
    ])
}

fn feature_handle(value: SysmlFeatureHandle) -> H {
    H::map([("element", element_handle(value.element))])
}

fn expression(value: &SysmlExpression) -> H {
    H::map([
        (
            "source",
            H::map([
                ("file", H::str(value.source.file.clone())),
                ("start", H::Int(i64::from(value.source.start))),
                ("end", H::Int(i64::from(value.source.end))),
                ("revision", H::UInt(value.source.revision)),
            ]),
        ),
        ("kind_code", H::Int(expression_kind_code(value.kind()))),
        (
            "feature",
            value.feature().map(feature_handle).unwrap_or(H::Unit),
        ),
        (
            "standard_constant_code",
            value
                .standard_constant()
                .map(standard_constant_code)
                .map(H::Int)
                .unwrap_or(H::Unit),
        ),
        (
            "function_element",
            value
                .function()
                .map(|function| element_handle(function.element))
                .unwrap_or(H::Unit),
        ),
        (
            "standard_function_code",
            value
                .function()
                .and_then(|function| function.standard_function)
                .map(standard_function_code)
                .map(H::Int)
                .unwrap_or(H::Unit),
        ),
        (
            "argument_parameters",
            H::Array(
                value
                    .argument_parameters()
                    .iter()
                    .map(|parameter| parameter.map(element_handle).unwrap_or(H::Unit))
                    .collect(),
            ),
        ),
        (
            "operator_code",
            value
                .operator()
                .map(expression_operator_code)
                .map(H::Int)
                .unwrap_or(H::Unit),
        ),
        (
            "integer_value",
            value.integer_value().map(H::Int).unwrap_or(H::Unit),
        ),
        (
            "real_value",
            value
                .real_value()
                .map(|number| H::Float(number.as_f64()))
                .unwrap_or(H::Unit),
        ),
        (
            "boolean_value",
            value.boolean_value().map(H::Bool).unwrap_or(H::Unit),
        ),
        (
            "string_value",
            value.string_value().map(H::str).unwrap_or(H::Unit),
        ),
        (
            "unsupported_code",
            value
                .unsupported()
                .map(unsupported_expression_code)
                .map(H::Int)
                .unwrap_or(H::Unit),
        ),
        (
            "children",
            H::Array(value.children().into_iter().map(expression).collect()),
        ),
    ])
}

// Numeric tags keep the language-neutral hook contract typed. Rhai's native
// SysmlExpression adapter exposes the Rust enums directly.
fn expression_kind_code(kind: SysmlExpressionKind) -> i64 {
    match kind {
        SysmlExpressionKind::FeatureReference => 1,
        SysmlExpressionKind::StandardConstant => 12,
        SysmlExpressionKind::Invocation => 13,
        SysmlExpressionKind::Index => 14,
        SysmlExpressionKind::Collection => 15,
        SysmlExpressionKind::FeatureChain => 16,
        SysmlExpressionKind::IntegerLiteral => 2,
        SysmlExpressionKind::RealLiteral => 3,
        SysmlExpressionKind::BooleanLiteral => 4,
        SysmlExpressionKind::StringLiteral => 5,
        SysmlExpressionKind::NullLiteral => 6,
        SysmlExpressionKind::Unary => 7,
        SysmlExpressionKind::Binary => 8,
        SysmlExpressionKind::Conditional => 9,
        SysmlExpressionKind::Group => 10,
        SysmlExpressionKind::Unsupported => 11,
    }
}

fn standard_constant_code(constant: SysmlStandardConstant) -> i64 {
    match constant {
        SysmlStandardConstant::Pi => 1,
    }
}

fn standard_function_code(function: SysmlStandardFunction) -> i64 {
    match function {
        SysmlStandardFunction::Abs => 1,
        SysmlStandardFunction::Min => 2,
        SysmlStandardFunction::Max => 3,
        SysmlStandardFunction::Sqrt => 4,
        SysmlStandardFunction::Floor => 5,
        SysmlStandardFunction::Round => 6,
        SysmlStandardFunction::Sum => 7,
        SysmlStandardFunction::Product => 8,
        SysmlStandardFunction::IsZero => 9,
        SysmlStandardFunction::IsUnit => 10,
        SysmlStandardFunction::Sin => 11,
        SysmlStandardFunction::Cos => 12,
        SysmlStandardFunction::Tan => 13,
        SysmlStandardFunction::Cot => 14,
        SysmlStandardFunction::ArcSin => 15,
        SysmlStandardFunction::ArcCos => 16,
        SysmlStandardFunction::ArcTan => 17,
        SysmlStandardFunction::Deg => 18,
        SysmlStandardFunction::Rad => 19,
        SysmlStandardFunction::Size => 20,
        SysmlStandardFunction::IsEmpty => 21,
        SysmlStandardFunction::NotEmpty => 22,
        SysmlStandardFunction::AllTrue => 23,
        SysmlStandardFunction::AnyTrue => 24,
        SysmlStandardFunction::ToStringBoolean => 25,
        SysmlStandardFunction::ToStringInteger => 26,
        SysmlStandardFunction::ToStringReal => 27,
        SysmlStandardFunction::ToStringString => 28,
    }
}

fn expression_operator_code(operator: SysmlExpressionOperator) -> i64 {
    match operator {
        SysmlExpressionOperator::Positive => 1,
        SysmlExpressionOperator::Negative => 2,
        SysmlExpressionOperator::Not => 3,
        SysmlExpressionOperator::Add => 4,
        SysmlExpressionOperator::Subtract => 5,
        SysmlExpressionOperator::Multiply => 6,
        SysmlExpressionOperator::Divide => 7,
        SysmlExpressionOperator::Power => 8,
        SysmlExpressionOperator::Equal => 9,
        SysmlExpressionOperator::NotEqual => 10,
        SysmlExpressionOperator::Less => 11,
        SysmlExpressionOperator::LessEqual => 12,
        SysmlExpressionOperator::Greater => 13,
        SysmlExpressionOperator::GreaterEqual => 14,
        SysmlExpressionOperator::And => 15,
        SysmlExpressionOperator::Or => 16,
        SysmlExpressionOperator::Implies => 17,
        SysmlExpressionOperator::Equivalent => 18,
    }
}

fn unsupported_expression_code(reason: SysmlUnsupportedExpression) -> i64 {
    match reason {
        SysmlUnsupportedExpression::UnresolvedReference => 1,
        SysmlUnsupportedExpression::NonFeatureReference => 2,
        SysmlUnsupportedExpression::Operator => 3,
        SysmlUnsupportedExpression::Call => 4,
        SysmlUnsupportedExpression::NonFunctionCall => 11,
        SysmlUnsupportedExpression::Collection => 5,
        SysmlUnsupportedExpression::Index => 6,
        SysmlUnsupportedExpression::Metadata => 7,
        SysmlUnsupportedExpression::Arrow => 8,
        SysmlUnsupportedExpression::OtherSyntax => 9,
        SysmlUnsupportedExpression::InvalidLiteral => 10,
    }
}

fn attribute(value: &SysmlAttribute) -> H {
    H::map([
        ("handle", feature_handle(value.handle)),
        (
            "owner_handle",
            value.owner_handle.map(element_handle).unwrap_or(H::Unit),
        ),
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
            "value_category",
            H::str(format!("{:?}", value.value_category)),
        ),
        (
            "resolved_type",
            value
                .resolved_type
                .as_ref()
                .map(type_ref_facts)
                .unwrap_or(H::Unit),
        ),
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
                .map(type_ref_facts)
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

fn type_ref_facts(value: &SysmlTypeRef) -> H {
    H::map([("qualified_name", H::str(value.qualified_name.clone()))])
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
        (
            "constraints",
            H::Array(
                value
                    .constraints
                    .iter()
                    .map(|constraint| {
                        let mut fields = vec![
                            ("kind", H::str(constraint.kind.as_str())),
                            (
                                "qualified_name",
                                H::str(constraint.usage.qualified_name.clone()),
                            ),
                            ("usage", element(&constraint.usage)),
                        ];
                        if let Some(definition) = &constraint.definition {
                            fields.push((
                                "definition_name",
                                H::str(definition.qualified_name.clone()),
                            ));
                            fields.push(("definition", element(definition)));
                        }
                        H::map(fields)
                    })
                    .collect(),
            ),
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
        (
            "verified_requirements",
            H::Array(
                value
                    .verified_requirements
                    .iter()
                    .copied()
                    .map(element_handle)
                    .collect(),
            ),
        ),
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
