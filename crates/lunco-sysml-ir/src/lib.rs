//! A typed execution boundary for SysML/KerML constraints.
//!
//! The upstream SysML projection owns parsing, name resolution, metamodel
//! identity, and source provenance. This crate owns the next semantic step:
//! compiling the resolved projection into a stable, typed intermediate
//! representation that can be consumed by Rhai policy, a Modelica/Rumoca
//! adapter, verification reports, or a future USD/runtime provider.
//!
//! This is intentionally a compiler boundary, not a second SysML parser. An
//! unsupported construct is retained as a source-linked diagnostic and cannot
//! accidentally become a passing verification result.

use lunco_hash::Fnv1a;
use lunco_sysml_ast::{
    SysmlAnalysis, SysmlAttribute, SysmlConstraint, SysmlElementHandle, SysmlExpression,
    SysmlExpressionKind, SysmlExpressionOperator, SysmlFeature, SysmlFeatureHandle,
    SysmlMultiplicity, SysmlPrimitiveType, SysmlSourceRef, SysmlType, SysmlTypeCategory,
    SysmlUnsupportedExpression,
};
use serde::{Deserialize, Serialize};

const MAX_EXPRESSION_DEPTH: usize = 256;

/// Cardinality and collection semantics carried by an IR value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrMultiplicity {
    pub lower: usize,
    pub upper: Option<usize>,
    pub ordered: bool,
    pub unique: bool,
}

impl IrMultiplicity {
    pub const fn one() -> Self {
        Self {
            lower: 1,
            upper: Some(1),
            ordered: false,
            unique: true,
        }
    }

    pub fn from_sysml(value: SysmlMultiplicity) -> Self {
        Self {
            lower: value.lower,
            upper: value.upper,
            ordered: value.ordered,
            unique: value.unique,
        }
    }

    pub fn is_collection(self) -> bool {
        self.upper != Some(1) || self.lower != 1
    }
}

/// The value domain known by the neutral constraint IR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IrValueType {
    Boolean,
    Integer,
    Real,
    String,
    Quantity { quantity_kind: Option<String> },
    Enumeration { type_name: Option<String> },
    Structured { type_name: Option<String> },
    Unknown,
}

/// Direction of a KerML feature member declared by a constraint definition.
///
/// Direction is semantic metadata, not a naming convention. Keeping it in the
/// neutral IR lets a binding/provider layer distinguish inputs, outputs, and
/// bidirectional parameters without reparsing the SysML declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IrFeatureDirection {
    In,
    Out,
    InOut,
    None,
}

impl From<lunco_sysml_ast::SysmlFeatureDirection> for IrFeatureDirection {
    fn from(value: lunco_sysml_ast::SysmlFeatureDirection) -> Self {
        match value {
            lunco_sysml_ast::SysmlFeatureDirection::In => Self::In,
            lunco_sysml_ast::SysmlFeatureDirection::Out => Self::Out,
            lunco_sysml_ast::SysmlFeatureDirection::InOut => Self::InOut,
            lunco_sysml_ast::SysmlFeatureDirection::None => Self::None,
        }
    }
}

impl IrValueType {
    fn is_numeric(&self) -> bool {
        matches!(self, Self::Integer | Self::Real | Self::Quantity { .. })
    }

    fn is_boolean(&self) -> bool {
        matches!(self, Self::Boolean)
    }

    fn is_comparable(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Integer, Self::Real) | (Self::Real, Self::Integer) => true,
            (
                Self::Quantity {
                    quantity_kind: left,
                },
                Self::Quantity {
                    quantity_kind: right,
                },
            ) => left == right || left.is_none() || right.is_none(),
            (left, right) => {
                left == right || matches!(left, Self::Unknown) || matches!(right, Self::Unknown)
            }
        }
    }
}

/// A fully typed IR value shape, including SysML multiplicity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrType {
    pub value: IrValueType,
    pub multiplicity: IrMultiplicity,
    pub unit: Option<String>,
}

impl IrType {
    pub fn scalar(value: IrValueType) -> Self {
        Self {
            value,
            multiplicity: IrMultiplicity::one(),
            unit: None,
        }
    }

    fn is_boolean_scalar(&self) -> bool {
        !self.multiplicity.is_collection() && self.value.is_boolean()
    }

    fn is_numeric_shape(&self) -> bool {
        self.value.is_numeric()
    }
}

/// A source-linked literal in the neutral IR.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IrLiteral {
    Integer(i64),
    Real(f64),
    Boolean(bool),
    String(String),
    Null,
}

/// Operators whose arity and type rules are owned by the IR compiler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IrOperator {
    Positive,
    Negative,
    Not,
    Add,
    Subtract,
    Multiply,
    Divide,
    Power,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
    Implies,
    Equivalent,
}

impl From<SysmlExpressionOperator> for IrOperator {
    fn from(value: SysmlExpressionOperator) -> Self {
        match value {
            SysmlExpressionOperator::Positive => Self::Positive,
            SysmlExpressionOperator::Negative => Self::Negative,
            SysmlExpressionOperator::Not => Self::Not,
            SysmlExpressionOperator::Add => Self::Add,
            SysmlExpressionOperator::Subtract => Self::Subtract,
            SysmlExpressionOperator::Multiply => Self::Multiply,
            SysmlExpressionOperator::Divide => Self::Divide,
            SysmlExpressionOperator::Power => Self::Power,
            SysmlExpressionOperator::Equal => Self::Equal,
            SysmlExpressionOperator::NotEqual => Self::NotEqual,
            SysmlExpressionOperator::Less => Self::Less,
            SysmlExpressionOperator::LessEqual => Self::LessEqual,
            SysmlExpressionOperator::Greater => Self::Greater,
            SysmlExpressionOperator::GreaterEqual => Self::GreaterEqual,
            SysmlExpressionOperator::And => Self::And,
            SysmlExpressionOperator::Or => Self::Or,
            SysmlExpressionOperator::Implies => Self::Implies,
            SysmlExpressionOperator::Equivalent => Self::Equivalent,
        }
    }
}

/// A compiled expression node. Every node preserves its source location and
/// its statically inferred result type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IrExpression {
    pub source: SysmlSourceRef,
    pub result_type: IrType,
    pub kind: IrExpressionKind,
}

/// Typed expression topology after compilation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IrExpressionKind {
    FeatureReference {
        feature: SysmlFeatureHandle,
        qualified_name: String,
    },
    Literal(IrLiteral),
    Unary {
        operator: IrOperator,
        operand: Box<IrExpression>,
    },
    Binary {
        operator: IrOperator,
        left: Box<IrExpression>,
        right: Box<IrExpression>,
    },
    Conditional {
        condition: Box<IrExpression>,
        when_true: Box<IrExpression>,
        when_false: Box<IrExpression>,
    },
    Group(Box<IrExpression>),
}

/// A typed constraint parameter/member in the neutral IR.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IrParameter {
    pub feature: SysmlFeatureHandle,
    pub owner: Option<SysmlElementHandle>,
    pub direction: IrFeatureDirection,
    pub name: String,
    pub qualified_name: String,
    pub ty: IrType,
    pub source: SysmlSourceRef,
}

/// One neutral, source-pinned constraint definition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConstraintIr {
    pub element: SysmlElementHandle,
    pub qualified_name: String,
    pub source: SysmlSourceRef,
    pub parameters: Vec<IrParameter>,
    pub expressions: Vec<IrExpression>,
    pub dependencies: Vec<SysmlFeatureHandle>,
    pub fingerprint: u64,
}

/// Severity of a compiler/evaluator diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Warning,
    Error,
}

/// A source-linked diagnostic. Diagnostics are part of the contract and are
/// never reduced to a boolean success flag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrDiagnostic {
    pub severity: DiagnosticSeverity,
    pub code: String,
    pub source: Option<SysmlSourceRef>,
    pub message: String,
}

/// Compiler result for one constraint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompiledConstraint {
    pub constraint: Option<ConstraintIr>,
    pub diagnostics: Vec<IrDiagnostic>,
}

impl CompiledConstraint {
    pub fn is_valid(&self) -> bool {
        self.constraint.is_some()
            && !self
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
    }
}

/// Compile a named constraint from the already-resolved SysML projection.
pub fn compile_constraint_by_name(analysis: &SysmlAnalysis, name: &str) -> CompiledConstraint {
    match analysis
        .constraints()
        .iter()
        .find(|constraint| constraint.element.qualified_name == name)
    {
        Some(constraint) => compile_constraint(analysis, constraint),
        None => CompiledConstraint {
            constraint: None,
            diagnostics: vec![IrDiagnostic {
                severity: DiagnosticSeverity::Error,
                code: "SYSML-IR-001".to_owned(),
                source: None,
                message: format!("constraint `{name}` was not found in the semantic snapshot"),
            }],
        },
    }
}

/// Compile one source-backed constraint into the neutral IR.
pub fn compile_constraint(
    analysis: &SysmlAnalysis,
    constraint: &SysmlConstraint,
) -> CompiledConstraint {
    let mut diagnostics = Vec::new();
    let attributes = analysis.attributes();
    let mut expressions = Vec::with_capacity(constraint.expressions.len());
    let mut dependencies = Vec::new();
    let parameters: Vec<IrParameter> = constraint
        .parameters
        .iter()
        .map(|parameter| IrParameter {
            feature: parameter.handle,
            owner: parameter.owner_handle,
            direction: parameter.direction.into(),
            name: parameter.name.clone(),
            qualified_name: parameter.qualified_name.clone(),
            ty: parameter
                .declared_type
                .as_ref()
                .map(ir_type_from_sysml)
                .unwrap_or_else(|| IrType::scalar(IrValueType::Unknown)),
            source: SysmlSourceRef {
                file: parameter.file.clone(),
                start: parameter.start,
                end: parameter.end,
                revision: analysis.source_revision(),
            },
        })
        .collect();

    for expression in &constraint.expressions {
        if let Some(compiled) = compile_expression(
            expression,
            attributes,
            &constraint.parameters,
            &mut diagnostics,
            &mut dependencies,
            0,
        ) {
            expressions.push(compiled);
        }
    }

    if constraint.expressions.is_empty() {
        diagnostics.push(IrDiagnostic {
            severity: DiagnosticSeverity::Warning,
            code: "SYSML-IR-002".to_owned(),
            source: Some(source_of(&constraint.element, analysis.source_revision())),
            message: "constraint has no executable expression statements".to_owned(),
        });
    }

    let mut unique_dependencies = Vec::new();
    for dependency in dependencies {
        if !unique_dependencies.contains(&dependency) {
            unique_dependencies.push(dependency);
        }
    }

    let fingerprint = fingerprint_constraint(&constraint.element, &parameters, &expressions);
    let constraint_ir = ConstraintIr {
        element: constraint.element.handle,
        qualified_name: constraint.element.qualified_name.clone(),
        source: source_of(&constraint.element, analysis.source_revision()),
        parameters,
        expressions,
        dependencies: unique_dependencies,
        fingerprint,
    };

    CompiledConstraint {
        constraint: Some(constraint_ir),
        diagnostics,
    }
}

fn compile_expression(
    expression: &SysmlExpression,
    attributes: &[SysmlAttribute],
    parameters: &[SysmlFeature],
    diagnostics: &mut Vec<IrDiagnostic>,
    dependencies: &mut Vec<SysmlFeatureHandle>,
    depth: usize,
) -> Option<IrExpression> {
    if depth > MAX_EXPRESSION_DEPTH {
        diagnostics.push(error(
            "SYSML-IR-003",
            &expression.source,
            "expression nesting exceeds the compiler safety limit",
        ));
        return None;
    }

    let source = expression.source.clone();
    let kind = match expression.kind {
        SysmlExpressionKind::FeatureReference => {
            let feature = match expression.feature {
                Some(feature) => feature,
                None => {
                    diagnostics.push(error(
                        "SYSML-IR-004",
                        &source,
                        "feature reference has no resolved semantic feature",
                    ));
                    return None;
                }
            };
            let feature_name = attributes
                .iter()
                .find(|attribute| attribute.handle == feature)
                .map(|attribute| attribute.qualified_name.clone())
                .or_else(|| {
                    parameters
                        .iter()
                        .find(|parameter| parameter.handle == feature)
                        .map(|parameter| parameter.qualified_name.clone())
                });
            let feature_name = match feature_name {
                Some(feature_name) => feature_name,
                None => {
                    diagnostics.push(error(
                        "SYSML-IR-005",
                        &source,
                        "feature reference does not resolve to a projected attribute",
                    ));
                    return None;
                }
            };
            dependencies.push(feature);
            IrExpressionKind::FeatureReference {
                feature,
                qualified_name: feature_name,
            }
        }
        SysmlExpressionKind::IntegerLiteral => {
            IrExpressionKind::Literal(match expression.integer_value {
                Some(value) => IrLiteral::Integer(value),
                None => {
                    diagnostics.push(error(
                        "SYSML-IR-006",
                        &source,
                        "integer literal has no value",
                    ));
                    return None;
                }
            })
        }
        SysmlExpressionKind::RealLiteral => {
            IrExpressionKind::Literal(match expression.real_value {
                Some(value) => IrLiteral::Real(value.as_f64()),
                None => {
                    diagnostics.push(error("SYSML-IR-007", &source, "real literal has no value"));
                    return None;
                }
            })
        }
        SysmlExpressionKind::BooleanLiteral => {
            IrExpressionKind::Literal(match expression.boolean_value {
                Some(value) => IrLiteral::Boolean(value),
                None => {
                    diagnostics.push(error(
                        "SYSML-IR-008",
                        &source,
                        "boolean literal has no value",
                    ));
                    return None;
                }
            })
        }
        SysmlExpressionKind::StringLiteral => {
            IrExpressionKind::Literal(match &expression.string_value {
                Some(value) => IrLiteral::String(value.clone()),
                None => {
                    diagnostics.push(error(
                        "SYSML-IR-009",
                        &source,
                        "string literal has no value",
                    ));
                    return None;
                }
            })
        }
        SysmlExpressionKind::NullLiteral => IrExpressionKind::Literal(IrLiteral::Null),
        SysmlExpressionKind::Unary => {
            let operator = match expression.operator {
                Some(operator) => IrOperator::from(operator),
                None => {
                    diagnostics.push(error(
                        "SYSML-IR-010",
                        &source,
                        "unary expression has no operator",
                    ));
                    return None;
                }
            };
            let operand = match one_child(expression, diagnostics) {
                Some(child) => compile_expression(
                    child,
                    attributes,
                    parameters,
                    diagnostics,
                    dependencies,
                    depth + 1,
                )?,
                None => return None,
            };
            validate_unary(operator, &operand.result_type, &source, diagnostics);
            IrExpressionKind::Unary {
                operator,
                operand: Box::new(operand),
            }
        }
        SysmlExpressionKind::Binary => {
            let operator = match expression.operator {
                Some(operator) => IrOperator::from(operator),
                None => {
                    diagnostics.push(error(
                        "SYSML-IR-011",
                        &source,
                        "binary expression has no operator",
                    ));
                    return None;
                }
            };
            let (left, right) = match two_children(expression, diagnostics) {
                Some(children) => (
                    compile_expression(
                        children.0,
                        attributes,
                        parameters,
                        diagnostics,
                        dependencies,
                        depth + 1,
                    )?,
                    compile_expression(
                        children.1,
                        attributes,
                        parameters,
                        diagnostics,
                        dependencies,
                        depth + 1,
                    )?,
                ),
                None => return None,
            };
            validate_binary(
                operator,
                &left.result_type,
                &right.result_type,
                &source,
                diagnostics,
            );
            IrExpressionKind::Binary {
                operator,
                left: Box::new(left),
                right: Box::new(right),
            }
        }
        SysmlExpressionKind::Conditional => {
            let (condition, when_true, when_false) = match three_children(expression, diagnostics) {
                Some(children) => (
                    compile_expression(
                        children.0,
                        attributes,
                        parameters,
                        diagnostics,
                        dependencies,
                        depth + 1,
                    )?,
                    compile_expression(
                        children.1,
                        attributes,
                        parameters,
                        diagnostics,
                        dependencies,
                        depth + 1,
                    )?,
                    compile_expression(
                        children.2,
                        attributes,
                        parameters,
                        diagnostics,
                        dependencies,
                        depth + 1,
                    )?,
                ),
                None => return None,
            };
            if !condition.result_type.is_boolean_scalar() {
                diagnostics.push(error(
                    "SYSML-IR-012",
                    &condition.source,
                    "conditional guard must be a scalar Boolean",
                ));
            }
            if !condition_types_compatible(&when_true.result_type, &when_false.result_type) {
                diagnostics.push(error(
                    "SYSML-IR-013",
                    &source,
                    "conditional branches have incompatible value types",
                ));
            }
            IrExpressionKind::Conditional {
                condition: Box::new(condition),
                when_true: Box::new(when_true),
                when_false: Box::new(when_false),
            }
        }
        SysmlExpressionKind::Group => {
            let child = match one_child(expression, diagnostics) {
                Some(child) => compile_expression(
                    child,
                    attributes,
                    parameters,
                    diagnostics,
                    dependencies,
                    depth + 1,
                )?,
                None => return None,
            };
            IrExpressionKind::Group(Box::new(child))
        }
        SysmlExpressionKind::Unsupported => {
            let reason = expression
                .unsupported
                .map(unsupported_name)
                .unwrap_or("unsupported syntax");
            diagnostics.push(error(
                "SYSML-IR-014",
                &source,
                &format!("expression cannot be compiled: {reason}"),
            ));
            return None;
        }
    };

    let result_type = infer_result_type(&kind, attributes, parameters, expression, diagnostics);
    Some(IrExpression {
        source,
        result_type,
        kind,
    })
}

fn infer_result_type(
    kind: &IrExpressionKind,
    attributes: &[SysmlAttribute],
    parameters: &[SysmlFeature],
    source_expression: &SysmlExpression,
    diagnostics: &mut Vec<IrDiagnostic>,
) -> IrType {
    match kind {
        IrExpressionKind::FeatureReference { feature, .. } => attributes
            .iter()
            .find(|attribute| attribute.handle == *feature)
            .and_then(|attribute| attribute.declared_type.as_ref())
            .or_else(|| {
                parameters
                    .iter()
                    .find(|parameter| parameter.handle == *feature)
                    .and_then(|parameter| parameter.declared_type.as_ref())
            })
            .map(ir_type_from_sysml)
            .unwrap_or_else(|| IrType::scalar(IrValueType::Unknown)),
        IrExpressionKind::Literal(value) => IrType::scalar(match value {
            IrLiteral::Integer(_) => IrValueType::Integer,
            IrLiteral::Real(_) => IrValueType::Real,
            IrLiteral::Boolean(_) => IrValueType::Boolean,
            IrLiteral::String(_) => IrValueType::String,
            IrLiteral::Null => IrValueType::Unknown,
        }),
        IrExpressionKind::Unary { operator, operand } => match operator {
            IrOperator::Not => IrType::scalar(IrValueType::Boolean),
            _ => operand.result_type.clone(),
        },
        IrExpressionKind::Binary {
            operator,
            left,
            right,
        } => match operator {
            IrOperator::Equal
            | IrOperator::NotEqual
            | IrOperator::Less
            | IrOperator::LessEqual
            | IrOperator::Greater
            | IrOperator::GreaterEqual
            | IrOperator::And
            | IrOperator::Or
            | IrOperator::Implies
            | IrOperator::Equivalent => IrType::scalar(IrValueType::Boolean),
            IrOperator::Add
            | IrOperator::Subtract
            | IrOperator::Multiply
            | IrOperator::Divide
            | IrOperator::Power => numeric_result_type(&left.result_type, &right.result_type),
            _ => IrType::scalar(IrValueType::Unknown),
        },
        IrExpressionKind::Conditional {
            when_true,
            when_false,
            ..
        } => {
            if condition_types_compatible(&when_true.result_type, &when_false.result_type) {
                when_true.result_type.clone()
            } else {
                IrType::scalar(IrValueType::Unknown)
            }
        }
        IrExpressionKind::Group(child) => child.result_type.clone(),
    }
    .tap_unknown_diagnostic(source_expression, diagnostics)
}

trait IrTypeDiagnosticExt {
    fn tap_unknown_diagnostic(
        self,
        source: &SysmlExpression,
        diagnostics: &mut Vec<IrDiagnostic>,
    ) -> Self;
}

impl IrTypeDiagnosticExt for IrType {
    fn tap_unknown_diagnostic(
        self,
        source: &SysmlExpression,
        diagnostics: &mut Vec<IrDiagnostic>,
    ) -> Self {
        if matches!(self.value, IrValueType::Unknown) {
            diagnostics.push(IrDiagnostic {
                severity: DiagnosticSeverity::Warning,
                code: "SYSML-IR-015".to_owned(),
                source: Some(source.source.clone()),
                message: "expression result type is unresolved; downstream execution must not assume a scalar type".to_owned(),
            });
        }
        self
    }
}

fn ir_type_from_sysml(value: &SysmlType) -> IrType {
    let value_type = match value.value_category {
        SysmlTypeCategory::Primitive => match value.primitive {
            Some(SysmlPrimitiveType::Boolean) => IrValueType::Boolean,
            Some(SysmlPrimitiveType::Integer | SysmlPrimitiveType::Rational) => {
                IrValueType::Integer
            }
            Some(SysmlPrimitiveType::Real | SysmlPrimitiveType::Complex) => IrValueType::Real,
            Some(SysmlPrimitiveType::String) => IrValueType::String,
            None => IrValueType::Unknown,
        },
        SysmlTypeCategory::Quantity => IrValueType::Quantity {
            quantity_kind: value
                .quantity_kind
                .as_ref()
                .map(|kind| kind.qualified_name.clone()),
        },
        SysmlTypeCategory::Enumeration => IrValueType::Enumeration {
            type_name: value
                .resolved_type
                .as_ref()
                .map(|value| value.qualified_name.clone()),
        },
        SysmlTypeCategory::Structured
        | SysmlTypeCategory::Part
        | SysmlTypeCategory::Item
        | SysmlTypeCategory::Port
        | SysmlTypeCategory::Reference => IrValueType::Structured {
            type_name: value
                .resolved_type
                .as_ref()
                .map(|value| value.qualified_name.clone()),
        },
        // `category` is Collection for `Real[3]`, `Length[3]`, and other
        // multiplicity-bearing features. `value_category` retains the
        // element type and is the only correct source for the IR value kind.
        SysmlTypeCategory::Collection => match value.value_category {
            SysmlTypeCategory::Primitive => match value.primitive {
                Some(SysmlPrimitiveType::Boolean) => IrValueType::Boolean,
                Some(SysmlPrimitiveType::Integer | SysmlPrimitiveType::Rational) => {
                    IrValueType::Integer
                }
                Some(SysmlPrimitiveType::Real | SysmlPrimitiveType::Complex) => IrValueType::Real,
                Some(SysmlPrimitiveType::String) => IrValueType::String,
                None => IrValueType::Unknown,
            },
            SysmlTypeCategory::Quantity => IrValueType::Quantity {
                quantity_kind: value
                    .quantity_kind
                    .as_ref()
                    .map(|kind| kind.qualified_name.clone()),
            },
            SysmlTypeCategory::Enumeration => IrValueType::Enumeration {
                type_name: value
                    .resolved_type
                    .as_ref()
                    .map(|value| value.qualified_name.clone()),
            },
            SysmlTypeCategory::Structured
            | SysmlTypeCategory::Part
            | SysmlTypeCategory::Item
            | SysmlTypeCategory::Port
            | SysmlTypeCategory::Reference => IrValueType::Structured {
                type_name: value
                    .resolved_type
                    .as_ref()
                    .map(|value| value.qualified_name.clone()),
            },
            SysmlTypeCategory::Collection | SysmlTypeCategory::Unknown => IrValueType::Unknown,
        },
        SysmlTypeCategory::Unknown => IrValueType::Unknown,
    };
    IrType {
        value: value_type,
        multiplicity: IrMultiplicity::from_sysml(value.multiplicity),
        unit: value.unit.clone(),
    }
}

fn numeric_result_type(left: &IrType, right: &IrType) -> IrType {
    let value = match (&left.value, &right.value) {
        (IrValueType::Quantity { quantity_kind }, _) => IrValueType::Quantity {
            quantity_kind: quantity_kind.clone(),
        },
        (_, IrValueType::Quantity { quantity_kind }) => IrValueType::Quantity {
            quantity_kind: quantity_kind.clone(),
        },
        (IrValueType::Real, _) | (_, IrValueType::Real) => IrValueType::Real,
        _ => IrValueType::Integer,
    };
    IrType {
        value,
        multiplicity: left.multiplicity,
        unit: left.unit.clone().or_else(|| right.unit.clone()),
    }
}

fn condition_types_compatible(left: &IrType, right: &IrType) -> bool {
    left.multiplicity == right.multiplicity && left.value.is_comparable(&right.value)
}

fn validate_unary(
    operator: IrOperator,
    operand: &IrType,
    source: &SysmlSourceRef,
    diagnostics: &mut Vec<IrDiagnostic>,
) {
    let valid = match operator {
        IrOperator::Positive | IrOperator::Negative => operand.is_numeric_shape(),
        IrOperator::Not => operand.is_boolean_scalar(),
        _ => false,
    };
    if !valid {
        diagnostics.push(error(
            "SYSML-IR-016",
            source,
            "operator is not defined for the operand's resolved type and multiplicity",
        ));
    }
}

fn validate_binary(
    operator: IrOperator,
    left: &IrType,
    right: &IrType,
    source: &SysmlSourceRef,
    diagnostics: &mut Vec<IrDiagnostic>,
) {
    let same_shape = left.multiplicity == right.multiplicity;
    let valid = match operator {
        IrOperator::Add
        | IrOperator::Subtract
        | IrOperator::Multiply
        | IrOperator::Divide
        | IrOperator::Power => same_shape && left.value.is_numeric() && right.value.is_numeric(),
        IrOperator::Equal | IrOperator::NotEqual => {
            same_shape && left.value.is_comparable(&right.value)
        }
        IrOperator::Less
        | IrOperator::LessEqual
        | IrOperator::Greater
        | IrOperator::GreaterEqual => {
            same_shape && left.value.is_numeric() && right.value.is_numeric()
        }
        IrOperator::And | IrOperator::Or | IrOperator::Implies | IrOperator::Equivalent => {
            left.is_boolean_scalar() && right.is_boolean_scalar()
        }
        _ => false,
    };
    if !valid {
        diagnostics.push(error(
            "SYSML-IR-017",
            source,
            "operator is not defined for the operand types, units, or multiplicities",
        ));
    }
}

fn one_child<'a>(
    expression: &'a SysmlExpression,
    diagnostics: &mut Vec<IrDiagnostic>,
) -> Option<&'a SysmlExpression> {
    if expression.children.len() != 1 {
        diagnostics.push(error(
            "SYSML-IR-018",
            &expression.source,
            "unary/group expression must contain exactly one child",
        ));
        return None;
    }
    expression.children.first()
}

fn two_children<'a>(
    expression: &'a SysmlExpression,
    diagnostics: &mut Vec<IrDiagnostic>,
) -> Option<(&'a SysmlExpression, &'a SysmlExpression)> {
    if expression.children.len() != 2 {
        diagnostics.push(error(
            "SYSML-IR-019",
            &expression.source,
            "binary expression must contain exactly two children",
        ));
        return None;
    }
    Some((&expression.children[0], &expression.children[1]))
}

fn three_children<'a>(
    expression: &'a SysmlExpression,
    diagnostics: &mut Vec<IrDiagnostic>,
) -> Option<(
    &'a SysmlExpression,
    &'a SysmlExpression,
    &'a SysmlExpression,
)> {
    if expression.children.len() != 3 {
        diagnostics.push(error(
            "SYSML-IR-020",
            &expression.source,
            "conditional expression must contain guard and two branches",
        ));
        return None;
    }
    Some((
        &expression.children[0],
        &expression.children[1],
        &expression.children[2],
    ))
}

fn unsupported_name(value: SysmlUnsupportedExpression) -> &'static str {
    match value {
        SysmlUnsupportedExpression::UnresolvedReference => "unresolved reference",
        SysmlUnsupportedExpression::NonFeatureReference => "non-feature reference",
        SysmlUnsupportedExpression::Operator => "unsupported operator",
        SysmlUnsupportedExpression::Call => "call expression",
        SysmlUnsupportedExpression::Collection => "collection expression",
        SysmlUnsupportedExpression::Index => "index expression",
        SysmlUnsupportedExpression::Metadata => "metadata expression",
        SysmlUnsupportedExpression::Arrow => "arrow expression",
        SysmlUnsupportedExpression::OtherSyntax => "unsupported syntax",
        SysmlUnsupportedExpression::InvalidLiteral => "invalid literal",
    }
}

fn source_of(element: &lunco_sysml_ast::SysmlElement, revision: u64) -> SysmlSourceRef {
    SysmlSourceRef {
        file: element.file.clone(),
        start: element.start,
        end: element.end,
        revision,
    }
}

fn error(code: &str, source: &SysmlSourceRef, message: &str) -> IrDiagnostic {
    IrDiagnostic {
        severity: DiagnosticSeverity::Error,
        code: code.to_owned(),
        source: Some(source.clone()),
        message: message.to_owned(),
    }
}

fn fingerprint_constraint(
    element: &lunco_sysml_ast::SysmlElement,
    parameters: &[IrParameter],
    expressions: &[IrExpression],
) -> u64 {
    let mut hash = Fnv1a::new();
    hash.write_bytes(b"lunco.sysml.constraint-ir.v1");
    hash.write_bytes(element.qualified_name.as_bytes());
    hash.write_u64(element.handle.source_revision);
    hash.write_u64(element.handle.source_fingerprint);
    hash.write_u64(element.handle.element_id as u64);
    for parameter in parameters {
        hash.write_bytes(parameter.qualified_name.as_bytes());
        hash.write_u64(parameter.feature.element.element_id as u64);
        hash.write_u64(parameter.ty.multiplicity.lower as u64);
        hash.write_u64(parameter.ty.multiplicity.upper.unwrap_or(u64::MAX as usize) as u64);
    }
    for expression in expressions {
        fingerprint_expression(&mut hash, expression);
    }
    hash.finish()
}

fn fingerprint_expression(hash: &mut Fnv1a, expression: &IrExpression) {
    hash.write_bytes(expression.source.file.as_bytes());
    hash.write_u64(expression.source.start as u64);
    hash.write_u64(expression.source.end as u64);
    match &expression.kind {
        IrExpressionKind::FeatureReference { feature, .. } => {
            hash.write_bytes(b"feature");
            hash.write_u64(feature.element.element_id as u64);
        }
        IrExpressionKind::Literal(value) => {
            match value {
                IrLiteral::Integer(value) => hash.write_u64(*value as u64),
                IrLiteral::Real(value) => hash.write_u64(value.to_bits()),
                IrLiteral::Boolean(value) => hash.write_u64(u64::from(*value)),
                IrLiteral::String(value) => hash.write_bytes(value.as_bytes()),
                IrLiteral::Null => hash.write_bytes(b"null"),
            };
        }
        IrExpressionKind::Unary { operator, operand } => {
            hash.write_bytes(b"unary");
            hash.write_u64(*operator as u64);
            fingerprint_expression(hash, operand);
        }
        IrExpressionKind::Binary {
            operator,
            left,
            right,
        } => {
            hash.write_bytes(b"binary");
            hash.write_u64(*operator as u64);
            fingerprint_expression(hash, left);
            fingerprint_expression(hash, right);
        }
        IrExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            hash.write_bytes(b"conditional");
            fingerprint_expression(hash, condition);
            fingerprint_expression(hash, when_true);
            fingerprint_expression(hash, when_false);
        }
        IrExpressionKind::Group(child) => {
            hash.write_bytes(b"group");
            fingerprint_expression(hash, child);
        }
    }
}

/// Runtime value supplied by a binding provider or test fixture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IrValue {
    Integer(i64),
    Real(f64),
    Boolean(bool),
    String(String),
    Quantity { value: f64, unit: String },
    Collection(Vec<IrValue>),
    Null,
}

/// The authoritative provider class for a bound SysML feature value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BindingProvider {
    SourceLiteral,
    Usd,
    Modelica,
    Telemetry,
    Derived,
    External,
}

/// A source-pinned binding contract between a SysML feature and a runtime
/// value provider. A provider must publish one of the explicit observation
/// states below; absence is never interpreted as a numeric zero or Boolean
/// false.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingContract {
    pub feature: SysmlFeatureHandle,
    pub qualified_name: String,
    pub provider: BindingProvider,
    pub required: bool,
    pub unit: Option<String>,
    pub frame: Option<String>,
    pub time_basis: Option<String>,
}

/// Explicit provider state. Missing data is not a false engineering result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObservationState {
    Value,
    Unavailable,
    Invalid,
    Stale,
    ProviderError,
}

/// One observation with its provider state and optional typed value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeatureObservation {
    pub feature: SysmlFeatureHandle,
    pub provider: BindingProvider,
    pub state: ObservationState,
    pub value: Option<IrValue>,
    pub detail: Option<String>,
}

/// Read-only evaluation input. Providers outside this crate map USD,
/// telemetry, Modelica outputs, or derived values into this contract.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EvaluationContext {
    pub observations: Vec<FeatureObservation>,
}

impl EvaluationContext {
    pub fn observation(&self, feature: SysmlFeatureHandle) -> Option<&FeatureObservation> {
        self.observations
            .iter()
            .find(|observation| observation.feature == feature)
    }
}

/// Explicit verification outcome with four states suitable for reports and UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerificationVerdict {
    Pass,
    Fail,
    Inconclusive,
    Error,
}

/// One expression result and the aggregate constraint verdict.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationReport {
    pub verdict: VerificationVerdict,
    pub expression_results: Vec<Option<bool>>,
    pub diagnostics: Vec<IrDiagnostic>,
}

/// Numerical comparison policy used by the evaluator. Exact equality is not
/// a safe default for measured or solver-produced real values.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct EvaluationOptions {
    pub absolute_tolerance: f64,
    pub relative_tolerance: f64,
}

impl Default for EvaluationOptions {
    fn default() -> Self {
        Self {
            absolute_tolerance: 1.0e-9,
            relative_tolerance: 1.0e-9,
        }
    }
}

/// Evaluate a compiled constraint against explicit provider observations.
/// Unavailable or stale data yields `Inconclusive`; invalid/provider errors
/// yield `Error`; only a typed false expression yields `Fail`.
pub fn evaluate_constraint(
    compiled: &CompiledConstraint,
    context: &EvaluationContext,
    options: EvaluationOptions,
) -> EvaluationReport {
    let Some(constraint) = &compiled.constraint else {
        return EvaluationReport {
            verdict: VerificationVerdict::Error,
            expression_results: Vec::new(),
            diagnostics: compiled.diagnostics.clone(),
        };
    };
    if !compiled.is_valid() {
        return EvaluationReport {
            verdict: VerificationVerdict::Error,
            expression_results: vec![None; constraint.expressions.len()],
            diagnostics: compiled.diagnostics.clone(),
        };
    }
    if constraint.expressions.is_empty() {
        return EvaluationReport {
            verdict: VerificationVerdict::Inconclusive,
            expression_results: Vec::new(),
            diagnostics: vec![IrDiagnostic {
                severity: DiagnosticSeverity::Warning,
                code: "SYSML-IR-021".to_owned(),
                source: Some(constraint.source.clone()),
                message: "constraint has no executable body".to_owned(),
            }],
        };
    }

    let mut results = Vec::with_capacity(constraint.expressions.len());
    let mut diagnostics = compiled.diagnostics.clone();
    let mut has_inconclusive = false;
    let mut has_error = false;
    let mut has_failure = false;

    for expression in &constraint.expressions {
        match evaluate_expression(expression, context, options) {
            Ok(EvaluationValue::Boolean(value)) => {
                has_failure |= !value;
                results.push(Some(value));
            }
            Ok(_) => {
                has_error = true;
                results.push(None);
                diagnostics.push(error(
                    "SYSML-IR-022",
                    &expression.source,
                    "constraint body expression did not evaluate to Boolean",
                ));
            }
            Err(EvaluationFailure::Inconclusive(detail)) => {
                has_inconclusive = true;
                results.push(None);
                diagnostics.push(IrDiagnostic {
                    severity: DiagnosticSeverity::Warning,
                    code: "SYSML-IR-023".to_owned(),
                    source: Some(expression.source.clone()),
                    message: detail,
                });
            }
            Err(EvaluationFailure::Error(detail)) => {
                has_error = true;
                results.push(None);
                diagnostics.push(error("SYSML-IR-024", &expression.source, &detail));
            }
        }
    }

    let verdict = if has_error {
        VerificationVerdict::Error
    } else if has_failure {
        VerificationVerdict::Fail
    } else if has_inconclusive {
        VerificationVerdict::Inconclusive
    } else {
        VerificationVerdict::Pass
    };
    EvaluationReport {
        verdict,
        expression_results: results,
        diagnostics,
    }
}

#[derive(Clone, Debug, PartialEq)]
enum EvaluationValue {
    Integer(i64),
    Real(f64),
    Boolean(bool),
    String(String),
    Quantity { value: f64, unit: String },
    Collection(Vec<EvaluationValue>),
    Null,
}

#[derive(Clone, Debug, PartialEq)]
enum EvaluationFailure {
    Inconclusive(String),
    Error(String),
}

fn evaluate_expression(
    expression: &IrExpression,
    context: &EvaluationContext,
    options: EvaluationOptions,
) -> Result<EvaluationValue, EvaluationFailure> {
    match &expression.kind {
        IrExpressionKind::FeatureReference { feature, .. } => {
            let Some(observation) = context.observation(*feature) else {
                return Err(EvaluationFailure::Inconclusive(
                    "no provider observation exists for a referenced feature".to_owned(),
                ));
            };
            match observation.state {
                ObservationState::Value => observation
                    .value
                    .as_ref()
                    .map(runtime_value)
                    .ok_or_else(|| {
                        EvaluationFailure::Error(
                            "provider marked observation as Value without a value".to_owned(),
                        )
                    }),
                ObservationState::Unavailable | ObservationState::Stale => {
                    Err(EvaluationFailure::Inconclusive(
                        observation
                            .detail
                            .clone()
                            .unwrap_or_else(|| "provider value is unavailable or stale".to_owned()),
                    ))
                }
                ObservationState::Invalid | ObservationState::ProviderError => {
                    Err(EvaluationFailure::Error(
                        observation
                            .detail
                            .clone()
                            .unwrap_or_else(|| "provider returned an invalid value".to_owned()),
                    ))
                }
            }
        }
        IrExpressionKind::Literal(value) => Ok(match value {
            IrLiteral::Integer(value) => EvaluationValue::Integer(*value),
            IrLiteral::Real(value) => EvaluationValue::Real(*value),
            IrLiteral::Boolean(value) => EvaluationValue::Boolean(*value),
            IrLiteral::String(value) => EvaluationValue::String(value.clone()),
            IrLiteral::Null => EvaluationValue::Null,
        }),
        IrExpressionKind::Unary { operator, operand } => {
            let value = evaluate_expression(operand, context, options)?;
            evaluate_unary(*operator, value)
        }
        IrExpressionKind::Binary {
            operator,
            left,
            right,
        } => {
            let left = evaluate_expression(left, context, options)?;
            let right = evaluate_expression(right, context, options)?;
            evaluate_binary(*operator, left, right, options)
        }
        IrExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => match evaluate_expression(condition, context, options)? {
            EvaluationValue::Boolean(true) => evaluate_expression(when_true, context, options),
            EvaluationValue::Boolean(false) => evaluate_expression(when_false, context, options),
            _ => Err(EvaluationFailure::Error(
                "conditional guard did not evaluate to Boolean".to_owned(),
            )),
        },
        IrExpressionKind::Group(child) => evaluate_expression(child, context, options),
    }
}

fn runtime_value(value: &IrValue) -> EvaluationValue {
    match value {
        IrValue::Integer(value) => EvaluationValue::Integer(*value),
        IrValue::Real(value) => EvaluationValue::Real(*value),
        IrValue::Boolean(value) => EvaluationValue::Boolean(*value),
        IrValue::String(value) => EvaluationValue::String(value.clone()),
        IrValue::Quantity { value, unit } => EvaluationValue::Quantity {
            value: *value,
            unit: unit.clone(),
        },
        IrValue::Collection(values) => {
            EvaluationValue::Collection(values.iter().map(runtime_value).collect())
        }
        IrValue::Null => EvaluationValue::Null,
    }
}

fn evaluate_unary(
    operator: IrOperator,
    value: EvaluationValue,
) -> Result<EvaluationValue, EvaluationFailure> {
    match (operator, value) {
        (IrOperator::Positive, value) => numeric_map(value, |number| number),
        (IrOperator::Negative, value) => numeric_map(value, |number| -number),
        (IrOperator::Not, EvaluationValue::Boolean(value)) => Ok(EvaluationValue::Boolean(!value)),
        _ => Err(EvaluationFailure::Error(
            "unary operator received an incompatible runtime value".to_owned(),
        )),
    }
}

fn evaluate_binary(
    operator: IrOperator,
    left: EvaluationValue,
    right: EvaluationValue,
    options: EvaluationOptions,
) -> Result<EvaluationValue, EvaluationFailure> {
    match operator {
        IrOperator::And => bool_binary(left, right, |left, right| left && right),
        IrOperator::Or => bool_binary(left, right, |left, right| left || right),
        IrOperator::Implies => bool_binary(left, right, |left, right| !left || right),
        IrOperator::Equivalent => bool_binary(left, right, |left, right| left == right),
        IrOperator::Equal => compare_binary(left, right, options, |ordering| {
            ordering == std::cmp::Ordering::Equal
        }),
        IrOperator::NotEqual => compare_binary(left, right, options, |ordering| {
            ordering != std::cmp::Ordering::Equal
        }),
        IrOperator::Less => compare_binary(left, right, options, |ordering| {
            ordering == std::cmp::Ordering::Less
        }),
        IrOperator::LessEqual => compare_binary(left, right, options, |ordering| {
            ordering != std::cmp::Ordering::Greater
        }),
        IrOperator::Greater => compare_binary(left, right, options, |ordering| {
            ordering == std::cmp::Ordering::Greater
        }),
        IrOperator::GreaterEqual => compare_binary(left, right, options, |ordering| {
            ordering != std::cmp::Ordering::Less
        }),
        IrOperator::Add => numeric_binary(left, right, |left, right| left + right),
        IrOperator::Subtract => numeric_binary(left, right, |left, right| left - right),
        IrOperator::Multiply => numeric_binary(left, right, |left, right| left * right),
        IrOperator::Divide => numeric_binary(left, right, |left, right| left / right),
        IrOperator::Power => numeric_binary(left, right, |left, right| left.powf(right)),
        _ => Err(EvaluationFailure::Error(
            "binary operator is not executable for runtime values".to_owned(),
        )),
    }
}

fn bool_binary(
    left: EvaluationValue,
    right: EvaluationValue,
    operation: impl Fn(bool, bool) -> bool,
) -> Result<EvaluationValue, EvaluationFailure> {
    match (left, right) {
        (EvaluationValue::Boolean(left), EvaluationValue::Boolean(right)) => {
            Ok(EvaluationValue::Boolean(operation(left, right)))
        }
        _ => Err(EvaluationFailure::Error(
            "Boolean operator received a non-Boolean value".to_owned(),
        )),
    }
}

fn numeric_binary(
    left: EvaluationValue,
    right: EvaluationValue,
    operation: impl Fn(f64, f64) -> f64 + Copy,
) -> Result<EvaluationValue, EvaluationFailure> {
    match (left, right) {
        (EvaluationValue::Collection(left), EvaluationValue::Collection(right)) => {
            if left.len() != right.len() {
                return Err(EvaluationFailure::Error(
                    "collection operands have different cardinalities".to_owned(),
                ));
            }
            Ok(EvaluationValue::Collection(
                left.into_iter()
                    .zip(right)
                    .map(|(left, right)| numeric_binary(left, right, operation))
                    .collect::<Result<Vec<_>, _>>()?,
            ))
        }
        (left, right) => {
            let (left_number, left_quantity) = numeric_value(left)?;
            let (right_number, right_quantity) = numeric_value(right)?;
            if let (Some(left_unit), Some(right_unit)) = (&left_quantity, &right_quantity) {
                if left_unit != right_unit {
                    return Err(EvaluationFailure::Error(format!(
                        "quantity units `{left_unit}` and `{right_unit}` require an explicit conversion"
                    )));
                }
            }
            let unit = left_quantity.or(right_quantity);
            let value = operation(left_number, right_number);
            if !value.is_finite() {
                return Err(EvaluationFailure::Error(
                    "numeric operation produced a non-finite value".to_owned(),
                ));
            }
            Ok(unit.map_or(EvaluationValue::Real(value), |unit| {
                EvaluationValue::Quantity { value, unit }
            }))
        }
    }
}

fn numeric_map(
    value: EvaluationValue,
    operation: impl Fn(f64) -> f64 + Copy,
) -> Result<EvaluationValue, EvaluationFailure> {
    match value {
        EvaluationValue::Collection(values) => Ok(EvaluationValue::Collection(
            values
                .into_iter()
                .map(|value| numeric_map(value, operation))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        value => {
            let (number, unit) = numeric_value(value)?;
            let number = operation(number);
            if !number.is_finite() {
                return Err(EvaluationFailure::Error(
                    "numeric operation produced a non-finite value".to_owned(),
                ));
            }
            Ok(unit.map_or(EvaluationValue::Real(number), |unit| {
                EvaluationValue::Quantity {
                    value: number,
                    unit,
                }
            }))
        }
    }
}

fn numeric_value(value: EvaluationValue) -> Result<(f64, Option<String>), EvaluationFailure> {
    match value {
        EvaluationValue::Integer(value) => Ok((value as f64, None)),
        EvaluationValue::Real(value) => Ok((value, None)),
        EvaluationValue::Quantity { value, unit } => Ok((value, Some(unit))),
        _ => Err(EvaluationFailure::Error(
            "numeric operator received a non-numeric value".to_owned(),
        )),
    }
}

fn compare_binary(
    left: EvaluationValue,
    right: EvaluationValue,
    options: EvaluationOptions,
    operation: impl Fn(std::cmp::Ordering) -> bool + Copy,
) -> Result<EvaluationValue, EvaluationFailure> {
    match (left, right) {
        (EvaluationValue::Collection(left), EvaluationValue::Collection(right)) => {
            if left.len() != right.len() {
                return Err(EvaluationFailure::Error(
                    "collection operands have different cardinalities".to_owned(),
                ));
            }
            let mut result = true;
            for (left, right) in left.into_iter().zip(right) {
                let value = compare_binary(left, right, options, operation)?;
                result &= matches!(value, EvaluationValue::Boolean(true));
            }
            Ok(EvaluationValue::Boolean(result))
        }
        (left, right) => {
            let (left_number, left_unit) = numeric_value(left)?;
            let (right_number, right_unit) = numeric_value(right)?;
            if left_unit != right_unit {
                return Err(EvaluationFailure::Error(
                    "quantity comparison requires canonical matching units".to_owned(),
                ));
            }
            let scale = left_number.abs().max(right_number.abs()).max(1.0);
            let equal = (left_number - right_number).abs()
                <= options.absolute_tolerance + options.relative_tolerance * scale;
            let ordering = if equal {
                std::cmp::Ordering::Equal
            } else if left_number < right_number {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
            Ok(EvaluationValue::Boolean(operation(ordering)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_sysml_ast::SysmlAnalysis;

    #[test]
    fn compiles_one_resolved_constraint_smoke() {
        let analysis = SysmlAnalysis::from_files_without_stdlib([(
            "smoke.sysml",
            "package P { constraint def C { in a : Real; a == 1.0; } }",
        )]);
        let constraint = analysis.constraints().first().expect("constraint");
        let compiled = compile_constraint(&analysis, constraint);

        assert!(compiled.is_valid(), "{:?}", compiled.diagnostics);
        let ir = compiled.constraint.expect("compiled constraint");
        assert_eq!(ir.qualified_name, "P::C");
        assert_eq!(ir.parameters.len(), 1);
        assert_eq!(ir.expressions.len(), 1);
        assert_ne!(ir.fingerprint, 0);
    }
}
