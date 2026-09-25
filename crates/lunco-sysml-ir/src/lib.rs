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
    SysmlAnalysis, SysmlAttribute, SysmlConstraint, SysmlConstraintKind, SysmlElementHandle,
    SysmlExpression, SysmlExpressionData, SysmlExpressionOperator, SysmlFeature,
    SysmlFeatureDirection, SysmlFeatureHandle, SysmlFeaturePath, SysmlMultiplicity,
    SysmlPrimitiveType, SysmlSourceRef, SysmlType, SysmlTypeCategory, SysmlUnsupportedExpression,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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
    /// Exact SysML `Rational`; execution is rejected until the value/evaluator
    /// boundary supports exact rational arithmetic.
    Rational,
    Real,
    /// SysML `Complex`; execution is rejected until the value/evaluator
    /// boundary supports complex arithmetic.
    Complex,
    String,
    Quantity {
        quantity_kind: Option<String>,
    },
    Enumeration {
        type_name: Option<String>,
    },
    Reference {
        type_name: Option<String>,
    },
    Structured {
        type_name: Option<String>,
    },
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
            (Self::Reference { type_name: left }, Self::Reference { type_name: right }) => {
                left == right || left.is_none() || right.is_none()
            }
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

impl IrOperator {
    pub const SUPPORTED: &'static [Self] = &[
        Self::Positive,
        Self::Negative,
        Self::Not,
        Self::Add,
        Self::Subtract,
        Self::Multiply,
        Self::Divide,
        Self::Power,
        Self::Equal,
        Self::NotEqual,
        Self::Less,
        Self::LessEqual,
        Self::Greater,
        Self::GreaterEqual,
        Self::And,
        Self::Or,
        Self::Implies,
        Self::Equivalent,
    ];

    pub fn standard_name(self) -> &'static str {
        match self {
            Self::Positive => "+ (unary)",
            Self::Negative => "- (unary)",
            Self::Not => "not",
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::Divide => "/",
            Self::Power => "**",
            Self::Equal => "==",
            Self::NotEqual => "!=",
            Self::Less => "<",
            Self::LessEqual => "<=",
            Self::Greater => ">",
            Self::GreaterEqual => ">=",
            Self::And => "and",
            Self::Or => "or",
            Self::Implies => "implies",
            Self::Equivalent => "equivalent",
        }
    }

    pub fn arity(self) -> usize {
        match self {
            Self::Positive | Self::Negative | Self::Not => 1,
            _ => 2,
        }
    }
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
        path: SysmlFeaturePath,
    },
    StandardConstant {
        constant: IrStandardConstant,
        feature_element: SysmlElementHandle,
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
    Invocation {
        function: IrStandardFunction,
        function_element: SysmlElementHandle,
        argument_parameters: Vec<SysmlElementHandle>,
        arguments: Vec<IrExpression>,
    },
    PredicateInvocation {
        function_element: SysmlElementHandle,
        argument_parameters: Vec<SysmlElementHandle>,
        arguments: Vec<IrExpression>,
        body: Vec<IrExpression>,
    },
    Index {
        collection: Box<IrExpression>,
        index: Box<IrExpression>,
    },
    Collection(Vec<IrExpression>),
    Group(Box<IrExpression>),
}

/// Standard-library constants recognized by the source projection.
pub use lunco_sysml_ast::SysmlStandardConstant as IrStandardConstant;
/// Executable standard-library operations recognized by the source projection.
pub use lunco_sysml_ast::SysmlStandardFunction as IrStandardFunction;

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
    /// Resolved predicate type of this constraint usage, when uniquely typed.
    pub definition: Option<SysmlElementHandle>,
    pub qualified_name: String,
    pub source: SysmlSourceRef,
    pub parameters: Vec<IrParameter>,
    pub expressions: Vec<IrExpression>,
    pub dependencies: Vec<SysmlFeaturePath>,
    pub fingerprint: u64,
}

/// Severity of a compiler/evaluator diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticSeverity {
    Warning,
    Error,
}

macro_rules! define_ir_diagnostic_codes {
    ($($variant:ident => $code:literal,)+) => {
        /// Stable machine-readable compiler and evaluator diagnostic identity.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum IrDiagnosticCode {
            $(#[serde(rename = $code)] $variant,)+
        }

        impl IrDiagnosticCode {
            /// Serialized stable code for report consumers.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $code,)+
                }
            }
        }
    };
}

define_ir_diagnostic_codes! {
    ConstraintNotFound => "SYSML-IR-001",
    ConstraintHasNoExpressions => "SYSML-IR-002",
    ExpressionDepthExceeded => "SYSML-IR-003",
    FeatureNotInSnapshot => "SYSML-IR-005",
    ConditionalGuardNotBoolean => "SYSML-IR-012",
    ConditionalBranchesIncompatible => "SYSML-IR-013",
    UnsupportedExpression => "SYSML-IR-014",
    ExpressionTypeUnresolved => "SYSML-IR-015",
    InvalidUnaryOperands => "SYSML-IR-016",
    InvalidBinaryOperands => "SYSML-IR-017",
    ConstraintHasNoExecutableBody => "SYSML-IR-021",
    ConstraintBodyDidNotEvaluateToBoolean => "SYSML-IR-022",
    ObservationUnavailable => "SYSML-IR-023",
    ObservationEvaluationFailed => "SYSML-IR-024",
    InvalidObservationPath => "SYSML-IR-025",
    ObservationIsNotRecord => "SYSML-IR-026",
    ObservationProviderOrStateInvalid => "SYSML-IR-027",
    InvalidComparisonTolerance => "SYSML-IR-028",
    UnsupportedStandardFunction => "SYSML-IR-029",
    StandardFunctionArityMismatch => "SYSML-IR-030",
    StandardFunctionArgumentBindingInvalid => "SYSML-IR-031",
    StandardFunctionArgumentTypesInvalid => "SYSML-IR-032",
    InvalidCollectionIndex => "SYSML-IR-034",
    CollectionElementTypesIncompatible => "SYSML-IR-035",
    UnsupportedPrimitiveType => "SYSML-IR-038",
    FeatureChainSourceInvalid => "SYSML-IR-040",
    FeatureChainSnapshotMismatch => "SYSML-IR-041",
    ObservationSnapshotMismatch => "SYSML-IR-042",
    DuplicateObservationPath => "SYSML-IR-043",
    ObservationIsNotDependency => "SYSML-IR-044",
    InvalidBindingContract => "SYSML-IR-045",
    UnsupportedUserFunction => "SYSML-IR-046",
    PredicateDefinitionNotFound => "SYSML-IR-047",
    PredicateArgumentArityMismatch => "SYSML-IR-048",
    PredicateArgumentBindingInvalid => "SYSML-IR-049",
    PredicateArgumentTypesInvalid => "SYSML-IR-050",
    PredicateBodyNotBoolean => "SYSML-IR-051",
    RecursivePredicateInvocation => "SYSML-IR-052",
    PredicateFormalPathUnsupported => "SYSML-IR-053",
}

/// A source-linked diagnostic. Diagnostics are part of the contract and are
/// never reduced to a boolean success flag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrDiagnostic {
    pub severity: DiagnosticSeverity,
    pub code: IrDiagnosticCode,
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
                code: IrDiagnosticCode::ConstraintNotFound,
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
    let mut active_predicates = Vec::new();
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
            analysis,
            attributes,
            &constraint.parameters,
            &mut diagnostics,
            &mut dependencies,
            0,
            &mut active_predicates,
        ) {
            expressions.push(compiled);
        }
    }

    if constraint.expressions.is_empty() {
        diagnostics.push(IrDiagnostic {
            severity: DiagnosticSeverity::Warning,
            code: IrDiagnosticCode::ConstraintHasNoExpressions,
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

    let fingerprint = fingerprint_constraint(
        &constraint.element,
        constraint.definition,
        &parameters,
        &expressions,
    );
    let constraint_ir = ConstraintIr {
        element: constraint.element.handle,
        definition: constraint.definition,
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
    analysis: &SysmlAnalysis,
    attributes: &[SysmlAttribute],
    parameters: &[SysmlFeature],
    diagnostics: &mut Vec<IrDiagnostic>,
    dependencies: &mut Vec<SysmlFeaturePath>,
    depth: usize,
    active_predicates: &mut Vec<SysmlElementHandle>,
) -> Option<IrExpression> {
    if depth > MAX_EXPRESSION_DEPTH {
        diagnostics.push(error(
            IrDiagnosticCode::ExpressionDepthExceeded,
            &expression.source,
            "expression nesting exceeds the compiler safety limit",
        ));
        return None;
    }

    let source = expression.source.clone();
    let kind = match &expression.data {
        SysmlExpressionData::FeatureReference(feature) => {
            let path = SysmlFeaturePath::single(*feature);
            if !feature_is_in_snapshot(analysis, *feature) {
                diagnostics.push(error(
                    IrDiagnosticCode::FeatureNotInSnapshot,
                    &source,
                    "feature reference is not present in the resolved source snapshot",
                ));
                return None;
            }
            let declared_type = feature_declared_type(attributes, parameters, *feature);
            let unsupported_primitive = declared_type.as_ref().and_then(|ty| match &ty.value {
                IrValueType::Rational => Some("Rational"),
                IrValueType::Complex => Some("Complex"),
                _ => None,
            });
            if let Some(primitive) = unsupported_primitive {
                diagnostics.push(error(IrDiagnosticCode::UnsupportedPrimitiveType,
                    &source,
                    &format!(
                        "SysML primitive `{primitive}` is preserved in the type graph but is not executable in the scalar evaluator"
                    ),
                ));
                return None;
            }
            dependencies.push(path.clone());
            IrExpressionKind::FeatureReference { path }
        }
        SysmlExpressionData::FeatureChain { prefix, target } => {
            let Some(prefix_path) = sysml_feature_path(prefix) else {
                diagnostics.push(error(
                    IrDiagnosticCode::FeatureChainSourceInvalid,
                    &source,
                    "feature-chain source must resolve to a feature path",
                ));
                return None;
            };
            let Some(path) = prefix_path.followed_by(*target) else {
                diagnostics.push(error(
                    IrDiagnosticCode::FeatureChainSnapshotMismatch,
                    &source,
                    "feature-chain segments belong to different SysML source snapshots",
                ));
                return None;
            };
            if !feature_is_in_snapshot(analysis, *target) {
                diagnostics.push(error(
                    IrDiagnosticCode::FeatureNotInSnapshot,
                    &source,
                    "feature-chain target is not present in the resolved source snapshot",
                ));
                return None;
            }
            let declared_type = feature_declared_type(attributes, parameters, *target);
            let unsupported_primitive = declared_type.as_ref().and_then(|ty| match &ty.value {
                IrValueType::Rational => Some("Rational"),
                IrValueType::Complex => Some("Complex"),
                _ => None,
            });
            if let Some(primitive) = unsupported_primitive {
                diagnostics.push(error(IrDiagnosticCode::UnsupportedPrimitiveType,
                    &source,
                    &format!(
                        "SysML primitive `{primitive}` is preserved in the type graph but is not executable in the scalar evaluator"
                    ),
                ));
                return None;
            }
            dependencies.push(path.clone());
            IrExpressionKind::FeatureReference { path }
        }
        SysmlExpressionData::StandardConstant { feature, constant } => {
            IrExpressionKind::StandardConstant {
                constant: *constant,
                feature_element: feature.element,
            }
        }
        SysmlExpressionData::IntegerLiteral(value) => {
            IrExpressionKind::Literal(IrLiteral::Integer(*value))
        }
        SysmlExpressionData::RealLiteral(value) => {
            IrExpressionKind::Literal(IrLiteral::Real(value.as_f64()))
        }
        SysmlExpressionData::BooleanLiteral(value) => {
            IrExpressionKind::Literal(IrLiteral::Boolean(*value))
        }
        SysmlExpressionData::StringLiteral(value) => {
            IrExpressionKind::Literal(IrLiteral::String(value.clone()))
        }
        SysmlExpressionData::NullLiteral => IrExpressionKind::Literal(IrLiteral::Null),
        SysmlExpressionData::Unary { operator, operand } => {
            let operator = IrOperator::from(*operator);
            let operand = compile_expression(
                operand,
                analysis,
                attributes,
                parameters,
                diagnostics,
                dependencies,
                depth + 1,
                active_predicates,
            )?;
            validate_unary(operator, &operand.result_type, &source, diagnostics);
            IrExpressionKind::Unary {
                operator,
                operand: Box::new(operand),
            }
        }
        SysmlExpressionData::Binary {
            operator,
            left,
            right,
        } => {
            let operator = IrOperator::from(*operator);
            let left = compile_expression(
                left,
                analysis,
                attributes,
                parameters,
                diagnostics,
                dependencies,
                depth + 1,
                active_predicates,
            )?;
            let right = compile_expression(
                right,
                analysis,
                attributes,
                parameters,
                diagnostics,
                dependencies,
                depth + 1,
                active_predicates,
            )?;
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
        SysmlExpressionData::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            let condition = compile_expression(
                condition,
                analysis,
                attributes,
                parameters,
                diagnostics,
                dependencies,
                depth + 1,
                active_predicates,
            )?;
            let when_true = compile_expression(
                when_true,
                analysis,
                attributes,
                parameters,
                diagnostics,
                dependencies,
                depth + 1,
                active_predicates,
            )?;
            let when_false = compile_expression(
                when_false,
                analysis,
                attributes,
                parameters,
                diagnostics,
                dependencies,
                depth + 1,
                active_predicates,
            )?;
            if !condition.result_type.is_boolean_scalar() {
                diagnostics.push(error(
                    IrDiagnosticCode::ConditionalGuardNotBoolean,
                    &condition.source,
                    "conditional guard must be a scalar Boolean",
                ));
            }
            if !condition_types_compatible(&when_true.result_type, &when_false.result_type) {
                diagnostics.push(error(
                    IrDiagnosticCode::ConditionalBranchesIncompatible,
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
        SysmlExpressionData::Invocation {
            function: function_reference,
            arguments: invocation_arguments,
        } => {
            let Some(function) = function_reference.standard_function else {
                return compile_predicate_invocation(
                    function_reference.element,
                    invocation_arguments,
                    &source,
                    analysis,
                    attributes,
                    parameters,
                    diagnostics,
                    dependencies,
                    depth + 1,
                    active_predicates,
                );
            };
            if invocation_arguments.len() != function.arity() {
                diagnostics.push(error(
                    IrDiagnosticCode::StandardFunctionArityMismatch,
                    &source,
                    &format!(
                        "standard function `{}` requires {} argument(s), found {}",
                        function.standard_name(),
                        function.arity(),
                        invocation_arguments.len()
                    ),
                ));
                return None;
            }
            if invocation_arguments
                .iter()
                .any(|argument| argument.parameter.is_none())
            {
                diagnostics.push(error(
                    IrDiagnosticCode::StandardFunctionArgumentBindingInvalid,
                    &source,
                    "function arguments could not all be bound to resolved input parameters",
                ));
                return None;
            }
            let mut bound_parameters = invocation_arguments
                .iter()
                .filter_map(|argument| argument.parameter)
                .collect::<Vec<_>>();
            bound_parameters.sort_unstable_by_key(|parameter| parameter.element_id);
            if bound_parameters.windows(2).any(|pair| pair[0] == pair[1]) {
                diagnostics.push(error(
                    IrDiagnosticCode::StandardFunctionArgumentBindingInvalid,
                    &source,
                    "more than one invocation argument is bound to the same input parameter",
                ));
                return None;
            }
            let arguments = invocation_arguments
                .iter()
                .map(|argument| {
                    compile_expression(
                        &argument.value,
                        analysis,
                        attributes,
                        parameters,
                        diagnostics,
                        dependencies,
                        depth + 1,
                        active_predicates,
                    )
                })
                .collect::<Option<Vec<_>>>()?;
            validate_standard_function(function, &arguments, &source, diagnostics);
            IrExpressionKind::Invocation {
                function,
                function_element: function_reference.element,
                argument_parameters: invocation_arguments
                    .iter()
                    .filter_map(|argument| argument.parameter)
                    .collect(),
                arguments,
            }
        }
        SysmlExpressionData::Index { collection, index } => {
            let collection = compile_expression(
                collection,
                analysis,
                attributes,
                parameters,
                diagnostics,
                dependencies,
                depth + 1,
                active_predicates,
            )?;
            let index = compile_expression(
                index,
                analysis,
                attributes,
                parameters,
                diagnostics,
                dependencies,
                depth + 1,
                active_predicates,
            )?;
            if !collection.result_type.multiplicity.is_collection()
                || index.result_type.multiplicity.is_collection()
                || !matches!(index.result_type.value, IrValueType::Integer)
            {
                diagnostics.push(error(
                    IrDiagnosticCode::InvalidCollectionIndex,
                    &source,
                    "indexing requires a collection and a scalar Integer index",
                ));
                return None;
            }
            IrExpressionKind::Index {
                collection: Box::new(collection),
                index: Box::new(index),
            }
        }
        SysmlExpressionData::Collection(expression_arguments) => {
            let values = expression_arguments
                .iter()
                .map(|child| {
                    compile_expression(
                        child,
                        analysis,
                        attributes,
                        parameters,
                        diagnostics,
                        dependencies,
                        depth + 1,
                        active_predicates,
                    )
                })
                .collect::<Option<Vec<_>>>()?;
            if collection_result_type(&values).is_none() {
                diagnostics.push(error(
                    IrDiagnosticCode::CollectionElementTypesIncompatible,
                    &source,
                    "collection literal elements must have compatible scalar types and units",
                ));
                return None;
            }
            IrExpressionKind::Collection(values)
        }
        SysmlExpressionData::Group(child) => {
            let child = compile_expression(
                child,
                analysis,
                attributes,
                parameters,
                diagnostics,
                dependencies,
                depth + 1,
                active_predicates,
            )?;
            IrExpressionKind::Group(Box::new(child))
        }
        SysmlExpressionData::Unsupported(reason) => {
            let reason = unsupported_name(*reason);
            diagnostics.push(error(
                IrDiagnosticCode::UnsupportedExpression,
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

fn compile_predicate_invocation(
    function_element: SysmlElementHandle,
    invocation_arguments: &[lunco_sysml_ast::SysmlInvocationArgument],
    source: &SysmlSourceRef,
    analysis: &SysmlAnalysis,
    attributes: &[SysmlAttribute],
    caller_parameters: &[SysmlFeature],
    diagnostics: &mut Vec<IrDiagnostic>,
    dependencies: &mut Vec<SysmlFeaturePath>,
    depth: usize,
    active_predicates: &mut Vec<SysmlElementHandle>,
) -> Option<IrExpression> {
    let Some(definition) = analysis.constraints().iter().find(|candidate| {
        candidate.element.handle == function_element
            && candidate.kind == SysmlConstraintKind::ConstraintDefinition
    }) else {
        diagnostics.push(error(
            IrDiagnosticCode::UnsupportedUserFunction,
            source,
            "only source-projected SysML constraint definitions can be invoked",
        ));
        return None;
    };

    let formal_inputs = definition
        .parameters
        .iter()
        .filter(|parameter| {
            matches!(
                parameter.direction,
                SysmlFeatureDirection::In | SysmlFeatureDirection::InOut
            )
        })
        .collect::<Vec<_>>();
    if invocation_arguments.len() != formal_inputs.len() {
        diagnostics.push(error(
            IrDiagnosticCode::PredicateArgumentArityMismatch,
            source,
            &format!(
                "predicate requires {} input argument(s), found {}",
                formal_inputs.len(),
                invocation_arguments.len()
            ),
        ));
        return None;
    }

    let formal_by_handle = formal_inputs
        .iter()
        .map(|formal| (formal.handle.element, *formal))
        .collect::<HashMap<_, _>>();
    let mut bound_arguments = HashMap::with_capacity(invocation_arguments.len());
    for argument in invocation_arguments {
        let Some(parameter) = argument.parameter else {
            diagnostics.push(error(
                IrDiagnosticCode::PredicateArgumentBindingInvalid,
                source,
                "predicate argument does not resolve to an input feature of its definition",
            ));
            return None;
        };
        if !formal_by_handle.contains_key(&parameter)
            || bound_arguments.insert(parameter, argument).is_some()
        {
            diagnostics.push(error(
                IrDiagnosticCode::PredicateArgumentBindingInvalid,
                source,
                "predicate arguments must bind each declared input feature exactly once",
            ));
            return None;
        }
    }
    if formal_inputs
        .iter()
        .any(|formal| !bound_arguments.contains_key(&formal.handle.element))
    {
        diagnostics.push(error(
            IrDiagnosticCode::PredicateArgumentBindingInvalid,
            source,
            "predicate call is missing a binding for a declared input feature",
        ));
        return None;
    }

    let mut argument_parameters = Vec::with_capacity(formal_inputs.len());
    let mut arguments = Vec::with_capacity(formal_inputs.len());
    for formal in &formal_inputs {
        let actual = bound_arguments[&formal.handle.element];
        let Some(argument) = compile_expression(
            &actual.value,
            analysis,
            attributes,
            caller_parameters,
            diagnostics,
            dependencies,
            depth + 1,
            active_predicates,
        ) else {
            return None;
        };
        let formal_type = formal
            .declared_type
            .as_ref()
            .map(ir_type_from_sysml)
            .unwrap_or_else(|| IrType::scalar(IrValueType::Unknown));
        if !predicate_argument_types_compatible(&formal_type, &argument.result_type) {
            diagnostics.push(error(
                IrDiagnosticCode::PredicateArgumentTypesInvalid,
                &actual.value.source,
                &format!(
                    "argument bound to `{}` has type {:?}, expected {:?}",
                    formal.name, argument.result_type, formal_type
                ),
            ));
            return None;
        }
        argument_parameters.push(formal.handle.element);
        arguments.push(argument);
    }

    if active_predicates.contains(&function_element) {
        diagnostics.push(error(
            IrDiagnosticCode::RecursivePredicateInvocation,
            source,
            "recursive constraint-definition invocation is not executable",
        ));
        return None;
    }
    if definition.expressions.is_empty() {
        diagnostics.push(error(
            IrDiagnosticCode::PredicateDefinitionNotFound,
            source,
            "invoked constraint definition has no executable predicate body",
        ));
        return None;
    }

    active_predicates.push(function_element);
    let mut body_dependencies = Vec::new();
    let mut body = Vec::with_capacity(definition.expressions.len());
    let mut body_failed = false;
    for expression in &definition.expressions {
        match compile_expression(
            expression,
            analysis,
            attributes,
            &definition.parameters,
            diagnostics,
            &mut body_dependencies,
            depth + 1,
            active_predicates,
        ) {
            Some(compiled) if compiled.result_type.is_boolean_scalar() => body.push(compiled),
            Some(compiled) => {
                diagnostics.push(error(
                    IrDiagnosticCode::PredicateBodyNotBoolean,
                    &compiled.source,
                    "every expression in a constraint-definition body must be a scalar Boolean",
                ));
                body_failed = true;
            }
            None => body_failed = true,
        }
    }
    active_predicates.pop();
    if body_failed {
        return None;
    }

    let formal_handles = formal_inputs
        .iter()
        .map(|formal| formal.handle)
        .collect::<HashSet<_>>();
    for dependency in &body_dependencies {
        if dependency
            .features()
            .first()
            .is_some_and(|feature| formal_handles.contains(feature))
            && dependency.features().len() > 1
        {
            diagnostics.push(error(
                IrDiagnosticCode::PredicateFormalPathUnsupported,
                source,
                "feature navigation through a bound predicate parameter requires structured-value evaluation",
            ));
            return None;
        }
    }
    body_dependencies.retain(|dependency| {
        !(dependency.features().len() == 1 && formal_handles.contains(&dependency.target()))
    });
    dependencies.extend(body_dependencies);

    Some(IrExpression {
        source: source.clone(),
        result_type: IrType::scalar(IrValueType::Boolean),
        kind: IrExpressionKind::PredicateInvocation {
            function_element,
            argument_parameters,
            arguments,
            body,
        },
    })
}

fn predicate_argument_types_compatible(formal: &IrType, actual: &IrType) -> bool {
    if formal.value == IrValueType::Unknown || actual.value == IrValueType::Unknown {
        return false;
    }
    formal.multiplicity == actual.multiplicity
        && formal.unit == actual.unit
        && formal.value == actual.value
}

fn feature_is_in_snapshot(analysis: &SysmlAnalysis, feature: SysmlFeatureHandle) -> bool {
    analysis
        .elements()
        .iter()
        .any(|element| element.feature_handle == Some(feature))
}

fn feature_declared_type(
    attributes: &[SysmlAttribute],
    parameters: &[SysmlFeature],
    feature: SysmlFeatureHandle,
) -> Option<IrType> {
    attributes
        .iter()
        .find(|attribute| attribute.handle == feature)
        .and_then(|attribute| attribute.declared_type.as_ref())
        .or_else(|| {
            parameters
                .iter()
                .find(|parameter| parameter.handle == feature)
                .and_then(|parameter| parameter.declared_type.as_ref())
        })
        .map(ir_type_from_sysml)
}

fn sysml_feature_path(expression: &SysmlExpression) -> Option<SysmlFeaturePath> {
    match &expression.data {
        SysmlExpressionData::FeatureReference(feature) => Some(SysmlFeaturePath::single(*feature)),
        SysmlExpressionData::FeatureChain { prefix, target } => {
            sysml_feature_path(prefix)?.followed_by(*target)
        }
        SysmlExpressionData::Group(child) => sysml_feature_path(child),
        _ => None,
    }
}

fn validate_standard_function(
    function: IrStandardFunction,
    arguments: &[IrExpression],
    source: &SysmlSourceRef,
    diagnostics: &mut Vec<IrDiagnostic>,
) {
    let types = arguments
        .iter()
        .map(|argument| &argument.result_type)
        .collect::<Vec<_>>();
    let scalar_numeric = |ty: &IrType| !ty.multiplicity.is_collection() && ty.value.is_numeric();
    let scalar_real = |ty: &IrType| {
        !ty.multiplicity.is_collection()
            && matches!(ty.value, IrValueType::Integer | IrValueType::Real)
    };
    let valid = match function {
        IrStandardFunction::Abs => types.first().is_some_and(|ty| scalar_numeric(ty)),
        IrStandardFunction::Min | IrStandardFunction::Max => {
            types.len() == 2
                && scalar_numeric(types[0])
                && scalar_numeric(types[1])
                && types[0].value.is_comparable(&types[1].value)
                && types[0].unit == types[1].unit
        }
        IrStandardFunction::Sqrt
        | IrStandardFunction::Floor
        | IrStandardFunction::Round
        | IrStandardFunction::Sin
        | IrStandardFunction::Cos
        | IrStandardFunction::Tan
        | IrStandardFunction::Cot
        | IrStandardFunction::ArcSin
        | IrStandardFunction::ArcCos
        | IrStandardFunction::ArcTan
        | IrStandardFunction::Deg
        | IrStandardFunction::Rad => types.first().is_some_and(|ty| scalar_real(ty)),
        IrStandardFunction::Sum => types
            .first()
            .is_some_and(|ty| ty.multiplicity.is_collection() && ty.value.is_numeric()),
        IrStandardFunction::Product => types.first().is_some_and(|ty| {
            ty.multiplicity.is_collection()
                && matches!(ty.value, IrValueType::Integer | IrValueType::Real)
        }),
        IrStandardFunction::IsZero | IrStandardFunction::IsUnit => {
            types.first().is_some_and(|ty| scalar_numeric(ty))
        }
        IrStandardFunction::Size | IrStandardFunction::IsEmpty | IrStandardFunction::NotEmpty => {
            types
                .first()
                .is_some_and(|ty| ty.multiplicity.is_collection())
        }
        IrStandardFunction::AllTrue | IrStandardFunction::AnyTrue => {
            types.first().is_some_and(|ty| {
                ty.multiplicity.is_collection() && matches!(ty.value, IrValueType::Boolean)
            })
        }
        IrStandardFunction::ToStringBoolean => types.first().is_some_and(|ty| {
            !ty.multiplicity.is_collection()
                && ty.unit.is_none()
                && ty.value == IrValueType::Boolean
        }),
        IrStandardFunction::ToStringInteger => types.first().is_some_and(|ty| {
            !ty.multiplicity.is_collection()
                && ty.unit.is_none()
                && ty.value == IrValueType::Integer
        }),
        IrStandardFunction::ToStringReal => types.first().is_some_and(|ty| {
            !ty.multiplicity.is_collection()
                && ty.unit.is_none()
                && matches!(ty.value, IrValueType::Integer | IrValueType::Real)
        }),
        IrStandardFunction::ToStringString => types.first().is_some_and(|ty| {
            !ty.multiplicity.is_collection() && ty.unit.is_none() && ty.value == IrValueType::String
        }),
    };
    if !valid {
        diagnostics.push(error(IrDiagnosticCode::StandardFunctionArgumentTypesInvalid,
            source,
            &format!(
                "standard function {function:?} received argument types outside its supported typed subset"
            ),
        ));
    }
}

fn collection_result_type(elements: &[IrExpression]) -> Option<IrType> {
    let first = elements.first()?;
    if elements
        .iter()
        .any(|element| element.result_type.multiplicity.is_collection())
    {
        return None;
    }
    let mut result = first.result_type.clone();
    for element in &elements[1..] {
        if !result.value.is_comparable(&element.result_type.value)
            || result.unit != element.result_type.unit
        {
            return None;
        }
        if result.value.is_numeric() && element.result_type.value.is_numeric() {
            result = numeric_result_type(&result, &element.result_type);
        }
    }
    result.multiplicity = IrMultiplicity {
        lower: elements.len(),
        upper: Some(elements.len()),
        ordered: true,
        unique: false,
    };
    Some(result)
}

fn infer_result_type(
    kind: &IrExpressionKind,
    attributes: &[SysmlAttribute],
    parameters: &[SysmlFeature],
    source_expression: &SysmlExpression,
    diagnostics: &mut Vec<IrDiagnostic>,
) -> IrType {
    match kind {
        IrExpressionKind::FeatureReference { path, .. } => attributes
            .iter()
            .find(|attribute| attribute.handle == path.target())
            .and_then(|attribute| attribute.declared_type.as_ref())
            .or_else(|| {
                parameters
                    .iter()
                    .find(|parameter| parameter.handle == path.target())
                    .and_then(|parameter| parameter.declared_type.as_ref())
            })
            .map(ir_type_from_sysml)
            .unwrap_or_else(|| IrType::scalar(IrValueType::Unknown)),
        IrExpressionKind::StandardConstant { .. } => IrType::scalar(IrValueType::Real),
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
        IrExpressionKind::Invocation {
            function,
            arguments,
            ..
        } => infer_standard_function_result(*function, arguments),
        IrExpressionKind::PredicateInvocation { .. } => IrType::scalar(IrValueType::Boolean),
        IrExpressionKind::Index { collection, .. } => {
            let mut result = collection.result_type.clone();
            result.multiplicity = IrMultiplicity::one();
            result
        }
        IrExpressionKind::Collection(elements) => {
            collection_result_type(elements).unwrap_or_else(|| IrType::scalar(IrValueType::Unknown))
        }
        IrExpressionKind::Group(child) => child.result_type.clone(),
    }
    .tap_unknown_diagnostic(source_expression, diagnostics)
}

fn infer_standard_function_result(
    function: IrStandardFunction,
    arguments: &[IrExpression],
) -> IrType {
    let argument_type = || {
        arguments
            .first()
            .map(|argument| argument.result_type.clone())
            .unwrap_or_else(|| IrType::scalar(IrValueType::Unknown))
    };
    match function {
        IrStandardFunction::Abs => argument_type(),
        IrStandardFunction::Min | IrStandardFunction::Max => arguments
            .first()
            .zip(arguments.get(1))
            .map(|(left, right)| numeric_result_type(&left.result_type, &right.result_type))
            .unwrap_or_else(|| IrType::scalar(IrValueType::Unknown)),
        IrStandardFunction::Sum | IrStandardFunction::Product => {
            let mut result = argument_type();
            result.multiplicity = IrMultiplicity::one();
            result
        }
        IrStandardFunction::Floor | IrStandardFunction::Round | IrStandardFunction::Size => {
            IrType::scalar(IrValueType::Integer)
        }
        IrStandardFunction::Sqrt
        | IrStandardFunction::Sin
        | IrStandardFunction::Cos
        | IrStandardFunction::Tan
        | IrStandardFunction::Cot
        | IrStandardFunction::ArcSin
        | IrStandardFunction::ArcCos
        | IrStandardFunction::ArcTan
        | IrStandardFunction::Deg
        | IrStandardFunction::Rad => IrType::scalar(IrValueType::Real),
        IrStandardFunction::IsZero
        | IrStandardFunction::IsUnit
        | IrStandardFunction::IsEmpty
        | IrStandardFunction::NotEmpty
        | IrStandardFunction::AllTrue
        | IrStandardFunction::AnyTrue => IrType::scalar(IrValueType::Boolean),
        IrStandardFunction::ToStringBoolean
        | IrStandardFunction::ToStringInteger
        | IrStandardFunction::ToStringReal
        | IrStandardFunction::ToStringString => IrType::scalar(IrValueType::String),
    }
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
                code: IrDiagnosticCode::ExpressionTypeUnresolved,
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
            Some(SysmlPrimitiveType::Integer) => IrValueType::Integer,
            Some(SysmlPrimitiveType::Rational) => IrValueType::Rational,
            Some(SysmlPrimitiveType::Real) => IrValueType::Real,
            Some(SysmlPrimitiveType::Complex) => IrValueType::Complex,
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
        SysmlTypeCategory::Reference => IrValueType::Reference {
            type_name: value
                .resolved_type
                .as_ref()
                .map(|value| value.qualified_name.clone()),
        },
        SysmlTypeCategory::Structured
        | SysmlTypeCategory::Part
        | SysmlTypeCategory::Item
        | SysmlTypeCategory::Port => IrValueType::Structured {
            type_name: value
                .resolved_type
                .as_ref()
                .map(|value| value.qualified_name.clone()),
        },
        // Collection multiplicity belongs to `IrType`; this case only means
        // the resolved element type itself is not scalar/structured.
        SysmlTypeCategory::Collection | SysmlTypeCategory::Unknown => IrValueType::Unknown,
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
            IrDiagnosticCode::InvalidUnaryOperands,
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
            IrDiagnosticCode::InvalidBinaryOperands,
            source,
            "operator is not defined for the operand types, units, or multiplicities",
        ));
    }
}

fn unsupported_name(value: SysmlUnsupportedExpression) -> &'static str {
    match value {
        SysmlUnsupportedExpression::UnresolvedReference => "unresolved reference",
        SysmlUnsupportedExpression::NonFeatureReference => "non-feature reference",
        SysmlUnsupportedExpression::Operator => "unsupported operator",
        SysmlUnsupportedExpression::Call => "call expression",
        SysmlUnsupportedExpression::NonFunctionCall => "call target is not a function",
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

fn error(code: IrDiagnosticCode, source: &SysmlSourceRef, message: &str) -> IrDiagnostic {
    IrDiagnostic {
        severity: DiagnosticSeverity::Error,
        code,
        source: Some(source.clone()),
        message: message.to_owned(),
    }
}

fn fingerprint_constraint(
    element: &lunco_sysml_ast::SysmlElement,
    definition: Option<SysmlElementHandle>,
    parameters: &[IrParameter],
    expressions: &[IrExpression],
) -> u64 {
    let mut hash = Fnv1a::new();
    hash.write_bytes(b"lunco.sysml.constraint-ir.v2");
    hash.write_bytes(element.qualified_name.as_bytes());
    hash.write_u64(element.handle.source_revision);
    hash.write_u64(element.handle.source_fingerprint);
    hash.write_u64(element.handle.element_id as u64);
    if let Some(definition) = definition {
        hash.write_bytes(b"constraint-definition");
        hash.write_u64(definition.source_revision);
        hash.write_u64(definition.source_fingerprint);
        hash.write_u64(definition.element_id as u64);
    } else {
        hash.write_bytes(b"no-constraint-definition");
    }
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
        IrExpressionKind::FeatureReference { path } => {
            hash.write_bytes(b"feature");
            for feature in path.features() {
                hash.write_u64(feature.element.source_revision);
                hash.write_u64(feature.element.source_fingerprint);
                hash.write_u64(feature.element.element_id as u64);
            }
        }
        IrExpressionKind::StandardConstant {
            constant,
            feature_element,
        } => {
            hash.write_bytes(b"standard-constant");
            hash.write_u64(*constant as u64);
            hash.write_u64(feature_element.source_revision);
            hash.write_u64(feature_element.source_fingerprint);
            hash.write_u64(feature_element.element_id as u64);
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
        IrExpressionKind::Invocation {
            function,
            function_element,
            argument_parameters,
            arguments,
        } => {
            hash.write_bytes(b"invocation");
            hash.write_u64(*function as u64);
            hash.write_u64(function_element.source_revision);
            hash.write_u64(function_element.source_fingerprint);
            hash.write_u64(function_element.element_id as u64);
            for parameter in argument_parameters {
                hash.write_u64(parameter.source_revision);
                hash.write_u64(parameter.source_fingerprint);
                hash.write_u64(parameter.element_id as u64);
            }
            for argument in arguments {
                fingerprint_expression(hash, argument);
            }
        }
        IrExpressionKind::PredicateInvocation {
            function_element,
            argument_parameters,
            arguments,
            body,
        } => {
            hash.write_bytes(b"predicate-invocation");
            hash.write_u64(function_element.source_revision);
            hash.write_u64(function_element.source_fingerprint);
            hash.write_u64(function_element.element_id as u64);
            hash.write_u64(argument_parameters.len() as u64);
            for parameter in argument_parameters {
                hash.write_u64(parameter.source_revision);
                hash.write_u64(parameter.source_fingerprint);
                hash.write_u64(parameter.element_id as u64);
            }
            hash.write_u64(arguments.len() as u64);
            for argument in arguments {
                fingerprint_expression(hash, argument);
            }
            hash.write_u64(body.len() as u64);
            for expression in body {
                fingerprint_expression(hash, expression);
            }
        }
        IrExpressionKind::Index { collection, index } => {
            hash.write_bytes(b"index");
            fingerprint_expression(hash, collection);
            fingerprint_expression(hash, index);
        }
        IrExpressionKind::Collection(elements) => {
            hash.write_bytes(b"collection");
            for element in elements {
                fingerprint_expression(hash, element);
            }
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
    Enumeration {
        type_name: Option<String>,
        literal: String,
    },
    /// Snapshot-scoped semantic identity resolved by the SysML model.
    Reference(SysmlElementHandle),
    Quantity {
        value: f64,
        unit: String,
    },
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
    pub path: SysmlFeaturePath,
    pub provider: BindingProvider,
    pub required: bool,
    pub unit: Option<String>,
    pub frame: Option<String>,
    pub time_basis: Option<String>,
    #[serde(default)]
    pub source_revision: Option<u64>,
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
    pub path: SysmlFeaturePath,
    pub provider: BindingProvider,
    pub state: ObservationState,
    pub value: Option<IrValue>,
    pub detail: Option<String>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub frame: Option<String>,
    #[serde(default)]
    pub time_basis: Option<String>,
    #[serde(default)]
    pub source_revision: Option<u64>,
    #[serde(default)]
    pub contract: Option<BindingContract>,
}

/// Read-only evaluation input. Providers outside this crate map USD,
/// telemetry, Modelica outputs, or derived values into this contract.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EvaluationContext {
    pub observations: Vec<FeatureObservation>,
}

impl EvaluationContext {
    pub fn observation(&self, path: &SysmlFeaturePath) -> Option<&FeatureObservation> {
        let mut matches = self
            .observations
            .iter()
            .filter(|observation| &observation.path == path);
        let observation = matches.next()?;
        matches.next().is_none().then_some(observation)
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
    if !options.absolute_tolerance.is_finite()
        || options.absolute_tolerance < 0.0
        || !options.relative_tolerance.is_finite()
        || options.relative_tolerance < 0.0
    {
        return EvaluationReport {
            verdict: VerificationVerdict::Error,
            expression_results: Vec::new(),
            diagnostics: vec![IrDiagnostic {
                severity: DiagnosticSeverity::Error,
                code: IrDiagnosticCode::InvalidComparisonTolerance,
                source: None,
                message: "comparison tolerances must be finite and non-negative".to_owned(),
            }],
        };
    }
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
    let context_diagnostics = validate_evaluation_context(constraint, context);
    if !context_diagnostics.is_empty() {
        let mut diagnostics = compiled.diagnostics.clone();
        diagnostics.extend(context_diagnostics);
        return EvaluationReport {
            verdict: VerificationVerdict::Error,
            expression_results: vec![None; constraint.expressions.len()],
            diagnostics,
        };
    }
    if constraint.expressions.is_empty() {
        return EvaluationReport {
            verdict: VerificationVerdict::Inconclusive,
            expression_results: Vec::new(),
            diagnostics: vec![IrDiagnostic {
                severity: DiagnosticSeverity::Warning,
                code: IrDiagnosticCode::ConstraintHasNoExecutableBody,
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
                    IrDiagnosticCode::ConstraintBodyDidNotEvaluateToBoolean,
                    &expression.source,
                    "constraint body expression did not evaluate to Boolean",
                ));
            }
            Err(EvaluationFailure::Inconclusive(detail)) => {
                has_inconclusive = true;
                results.push(None);
                diagnostics.push(IrDiagnostic {
                    severity: DiagnosticSeverity::Warning,
                    code: IrDiagnosticCode::ObservationUnavailable,
                    source: Some(expression.source.clone()),
                    message: detail,
                });
            }
            Err(EvaluationFailure::Error(detail)) => {
                has_error = true;
                results.push(None);
                diagnostics.push(error(
                    IrDiagnosticCode::ObservationEvaluationFailed,
                    &expression.source,
                    &detail,
                ));
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

fn validate_evaluation_context(
    constraint: &ConstraintIr,
    context: &EvaluationContext,
) -> Vec<IrDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut seen = Vec::with_capacity(context.observations.len());
    for observation in &context.observations {
        if !observation.path.belongs_to(
            constraint.element.source_revision,
            constraint.element.source_fingerprint,
        ) {
            diagnostics.push(IrDiagnostic {
                severity: DiagnosticSeverity::Error,
                code: IrDiagnosticCode::ObservationSnapshotMismatch,
                source: Some(constraint.source.clone()),
                message: "provider observation belongs to a different SysML source snapshot"
                    .to_owned(),
            });
        }
        if seen.contains(&observation.path) {
            diagnostics.push(IrDiagnostic {
                severity: DiagnosticSeverity::Error,
                code: IrDiagnosticCode::DuplicateObservationPath,
                source: Some(constraint.source.clone()),
                message: "provider supplied duplicate observations for one SysML feature path"
                    .to_owned(),
            });
        } else {
            seen.push(observation.path.clone());
        }
    }
    diagnostics
}

#[derive(Clone, Debug, PartialEq)]
enum EvaluationValue {
    Integer(i64),
    Real(f64),
    Boolean(bool),
    String(String),
    Enumeration {
        type_name: Option<String>,
        literal: String,
    },
    Reference(SysmlElementHandle),
    Quantity {
        value: f64,
        unit: String,
    },
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
    evaluate_expression_with_bindings(expression, context, options, &HashMap::new())
}

fn evaluate_expression_with_bindings(
    expression: &IrExpression,
    context: &EvaluationContext,
    options: EvaluationOptions,
    bindings: &HashMap<SysmlFeatureHandle, EvaluationValue>,
) -> Result<EvaluationValue, EvaluationFailure> {
    match &expression.kind {
        IrExpressionKind::FeatureReference { path, .. } => {
            if let Some(root) = path.features().first() {
                if let Some(value) = bindings.get(root) {
                    if path.features().len() != 1 {
                        return Err(EvaluationFailure::Error(
                            "feature navigation through a bound predicate parameter is unsupported for structured values".to_owned(),
                        ));
                    }
                    return if runtime_value_matches_type(value, &expression.result_type) {
                        Ok(value.clone())
                    } else {
                        Err(EvaluationFailure::Error(
                            "bound predicate argument does not match its declared type".to_owned(),
                        ))
                    };
                }
            }
            let Some(observation) = context.observation(path) else {
                return Err(EvaluationFailure::Inconclusive(
                    "no provider observation exists for a referenced feature".to_owned(),
                ));
            };
            validate_observation_contract(observation, path)?;
            match observation.state {
                ObservationState::Value => observation
                    .value
                    .as_ref()
                    .map(runtime_value)
                    .ok_or_else(|| {
                        EvaluationFailure::Error(
                            "provider marked observation as Value without a value".to_owned(),
                        )
                    })
                .and_then(|value| {
                    if runtime_value_matches_type(&value, &expression.result_type) {
                            if runtime_references_match_snapshot(&value, path) {
                                Ok(value)
                            } else {
                                Err(EvaluationFailure::Error(
                                    "reference observation belongs to a different SysML source snapshot".to_owned(),
                                ))
                            }
                        } else {
                            Err(EvaluationFailure::Error(format!(
                                "provider value does not match referenced SysML type {:?}",
                                expression.result_type.value
                            )))
                        }
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
        IrExpressionKind::StandardConstant { constant, .. } => match constant {
            IrStandardConstant::Pi => Ok(EvaluationValue::Real(std::f64::consts::PI)),
        },
        IrExpressionKind::Literal(value) => Ok(match value {
            IrLiteral::Integer(value) => EvaluationValue::Integer(*value),
            IrLiteral::Real(value) => EvaluationValue::Real(*value),
            IrLiteral::Boolean(value) => EvaluationValue::Boolean(*value),
            IrLiteral::String(value) => EvaluationValue::String(value.clone()),
            IrLiteral::Null => EvaluationValue::Null,
        }),
        IrExpressionKind::Unary { operator, operand } => {
            let value = evaluate_expression_with_bindings(operand, context, options, bindings)?;
            evaluate_unary(*operator, value)
        }
        IrExpressionKind::Binary {
            operator,
            left,
            right,
        } => {
            let left = evaluate_expression_with_bindings(left, context, options, bindings)?;
            let right = evaluate_expression_with_bindings(right, context, options, bindings)?;
            evaluate_binary(*operator, left, right, options)
        }
        IrExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => match evaluate_expression_with_bindings(condition, context, options, bindings)? {
            EvaluationValue::Boolean(true) => {
                evaluate_expression_with_bindings(when_true, context, options, bindings)
            }
            EvaluationValue::Boolean(false) => {
                evaluate_expression_with_bindings(when_false, context, options, bindings)
            }
            _ => Err(EvaluationFailure::Error(
                "conditional guard did not evaluate to Boolean".to_owned(),
            )),
        },
        IrExpressionKind::Invocation {
            function,
            arguments,
            ..
        } => {
            let values = arguments
                .iter()
                .map(|argument| {
                    evaluate_expression_with_bindings(argument, context, options, bindings)
                })
                .collect::<Result<Vec<_>, _>>()?;
            evaluate_standard_function(*function, values, &expression.result_type)
        }
        IrExpressionKind::PredicateInvocation {
            argument_parameters,
            arguments,
            body,
            ..
        } => {
            let values = arguments
                .iter()
                .map(|argument| {
                    evaluate_expression_with_bindings(argument, context, options, bindings)
                })
                .collect::<Result<Vec<_>, _>>()?;
            if argument_parameters.len() != values.len() {
                return Err(EvaluationFailure::Error(
                    "predicate argument bindings do not match evaluated arguments".to_owned(),
                ));
            }
            let mut scope = bindings.clone();
            for (parameter, value) in argument_parameters.iter().zip(values) {
                scope.insert(
                    SysmlFeatureHandle {
                        element: *parameter,
                    },
                    value,
                );
            }
            evaluate_predicate_body(body, context, options, &scope)
        }
        IrExpressionKind::Index { collection, index } => {
            let collection =
                evaluate_expression_with_bindings(collection, context, options, bindings)?;
            let index = evaluate_expression_with_bindings(index, context, options, bindings)?;
            let EvaluationValue::Collection(values) = collection else {
                return Err(EvaluationFailure::Error(
                    "indexing requires a collection value".to_owned(),
                ));
            };
            let EvaluationValue::Integer(index) = index else {
                return Err(EvaluationFailure::Error(
                    "collection index must evaluate to Integer".to_owned(),
                ));
            };
            let offset = usize::try_from(index)
                .ok()
                .and_then(|index| index.checked_sub(1))
                .ok_or_else(|| {
                    EvaluationFailure::Error(
                        "SysML collection indices are one-based positive integers".to_owned(),
                    )
                })?;
            values.get(offset).cloned().ok_or_else(|| {
                EvaluationFailure::Error(format!(
                    "collection index {index} is outside its 1..={} extent",
                    values.len()
                ))
            })
        }
        IrExpressionKind::Collection(elements) => elements
            .iter()
            .map(|element| evaluate_expression_with_bindings(element, context, options, bindings))
            .collect::<Result<Vec<_>, _>>()
            .map(EvaluationValue::Collection),
        IrExpressionKind::Group(child) => {
            evaluate_expression_with_bindings(child, context, options, bindings)
        }
    }
}

fn evaluate_predicate_body(
    body: &[IrExpression],
    context: &EvaluationContext,
    options: EvaluationOptions,
    bindings: &HashMap<SysmlFeatureHandle, EvaluationValue>,
) -> Result<EvaluationValue, EvaluationFailure> {
    let mut has_false = false;
    let mut inconclusive = None;
    let mut error = None;
    for expression in body {
        match evaluate_expression_with_bindings(expression, context, options, bindings) {
            Ok(EvaluationValue::Boolean(value)) => has_false |= !value,
            Ok(_) => {
                error.get_or_insert_with(|| {
                    "constraint-definition body expression did not evaluate to Boolean".to_owned()
                });
            }
            Err(EvaluationFailure::Inconclusive(detail)) => {
                inconclusive.get_or_insert(detail);
            }
            Err(EvaluationFailure::Error(detail)) => {
                error.get_or_insert(detail);
            }
        }
    }
    if let Some(detail) = error {
        Err(EvaluationFailure::Error(detail))
    } else if has_false {
        Ok(EvaluationValue::Boolean(false))
    } else if let Some(detail) = inconclusive {
        Err(EvaluationFailure::Inconclusive(detail))
    } else {
        Ok(EvaluationValue::Boolean(true))
    }
}

fn evaluate_standard_function(
    function: IrStandardFunction,
    mut arguments: Vec<EvaluationValue>,
    result_type: &IrType,
) -> Result<EvaluationValue, EvaluationFailure> {
    let mut unary = || {
        arguments
            .pop()
            .ok_or_else(|| EvaluationFailure::Error("function argument is missing".to_owned()))
    };
    match function {
        IrStandardFunction::Abs => match unary()? {
            EvaluationValue::Integer(value) => value
                .checked_abs()
                .map(EvaluationValue::Integer)
                .ok_or_else(|| EvaluationFailure::Error("integer abs overflow".to_owned())),
            EvaluationValue::Real(value) if value.is_finite() => {
                Ok(EvaluationValue::Real(value.abs()))
            }
            EvaluationValue::Quantity { value, unit } if value.is_finite() => {
                Ok(EvaluationValue::Quantity {
                    value: value.abs(),
                    unit,
                })
            }
            _ => Err(EvaluationFailure::Error(
                "abs requires a finite numeric scalar".to_owned(),
            )),
        },
        IrStandardFunction::Min | IrStandardFunction::Max => {
            let right = unary()?;
            let left = unary()?;
            let ordering = numeric_ordering(&left, &right)?;
            let select_left = match function {
                IrStandardFunction::Min => ordering != std::cmp::Ordering::Greater,
                IrStandardFunction::Max => ordering != std::cmp::Ordering::Less,
                _ => unreachable!(),
            };
            coerce_numeric_result(if select_left { left } else { right }, result_type)
        }
        IrStandardFunction::Sqrt
        | IrStandardFunction::Floor
        | IrStandardFunction::Round
        | IrStandardFunction::Sin
        | IrStandardFunction::Cos
        | IrStandardFunction::Tan
        | IrStandardFunction::Cot
        | IrStandardFunction::ArcSin
        | IrStandardFunction::ArcCos
        | IrStandardFunction::ArcTan
        | IrStandardFunction::Deg
        | IrStandardFunction::Rad => {
            let value = unary()?;
            let (number, unit) = numeric_value(value)?;
            if unit.is_some() || !number.is_finite() {
                return Err(EvaluationFailure::Error(
                    "real standard function requires a finite unitless scalar".to_owned(),
                ));
            }
            match function {
                IrStandardFunction::Sqrt if number >= 0.0 => finite_real(number.sqrt(), "sqrt"),
                IrStandardFunction::Floor => checked_integer(number.floor(), "floor"),
                IrStandardFunction::Round => checked_integer(number.round(), "round"),
                IrStandardFunction::Sin => finite_real(number.sin(), "sin"),
                IrStandardFunction::Cos => finite_real(number.cos(), "cos"),
                IrStandardFunction::Tan => finite_real(number.tan(), "tan"),
                IrStandardFunction::Cot => finite_real(1.0 / number.tan(), "cot"),
                IrStandardFunction::ArcSin if (-1.0..=1.0).contains(&number) => {
                    finite_real(number.asin(), "arcsin")
                }
                IrStandardFunction::ArcCos if (-1.0..=1.0).contains(&number) => {
                    finite_real(number.acos(), "arccos")
                }
                IrStandardFunction::ArcTan => finite_real(number.atan(), "arctan"),
                IrStandardFunction::Deg => finite_real(number.to_degrees(), "deg"),
                IrStandardFunction::Rad => finite_real(number.to_radians(), "rad"),
                _ => Err(EvaluationFailure::Error(format!(
                    "{function:?} argument is outside its mathematical domain"
                ))),
            }
        }
        IrStandardFunction::Sum | IrStandardFunction::Product => {
            let collection = match unary()? {
                EvaluationValue::Collection(values) => values,
                _ => {
                    return Err(EvaluationFailure::Error(
                        "aggregate function requires a collection value".to_owned(),
                    ));
                }
            };
            evaluate_numeric_aggregate(function, collection, result_type)
        }
        IrStandardFunction::IsZero | IrStandardFunction::IsUnit => {
            let (number, _) = numeric_value(unary()?)?;
            if !number.is_finite() {
                return Err(EvaluationFailure::Error(
                    "numeric predicate requires a finite scalar".to_owned(),
                ));
            }
            Ok(EvaluationValue::Boolean(
                if function == IrStandardFunction::IsZero {
                    number == 0.0
                } else {
                    number == 1.0
                },
            ))
        }
        IrStandardFunction::Size => match unary()? {
            EvaluationValue::Collection(values) => i64::try_from(values.len())
                .map(EvaluationValue::Integer)
                .map_err(|_| {
                    EvaluationFailure::Error("collection size exceeds Integer range".to_owned())
                }),
            _ => Err(EvaluationFailure::Error(
                "size requires a collection value".to_owned(),
            )),
        },
        IrStandardFunction::IsEmpty | IrStandardFunction::NotEmpty => match unary()? {
            EvaluationValue::Collection(values) => Ok(EvaluationValue::Boolean(
                if function == IrStandardFunction::IsEmpty {
                    values.is_empty()
                } else {
                    !values.is_empty()
                },
            )),
            _ => Err(EvaluationFailure::Error(
                "collection predicate requires a collection value".to_owned(),
            )),
        },
        IrStandardFunction::AllTrue | IrStandardFunction::AnyTrue => match unary()? {
            EvaluationValue::Collection(values) => {
                let mut booleans = values.into_iter().map(|value| match value {
                    EvaluationValue::Boolean(value) => Ok(value),
                    _ => Err(EvaluationFailure::Error(
                        "Boolean aggregate contains a non-Boolean value".to_owned(),
                    )),
                });
                let identity = function == IrStandardFunction::AllTrue;
                let result = booleans.try_fold(identity, |accumulator, value| {
                    value.map(|value| {
                        if function == IrStandardFunction::AllTrue {
                            accumulator && value
                        } else {
                            accumulator || value
                        }
                    })
                })?;
                Ok(EvaluationValue::Boolean(result))
            }
            _ => Err(EvaluationFailure::Error(
                "Boolean aggregate requires a collection value".to_owned(),
            )),
        },
        IrStandardFunction::ToStringBoolean
        | IrStandardFunction::ToStringInteger
        | IrStandardFunction::ToStringReal
        | IrStandardFunction::ToStringString => {
            let value = unary()?;
            let string = match (function, value) {
                (IrStandardFunction::ToStringBoolean, EvaluationValue::Boolean(value)) => {
                    value.to_string()
                }
                (IrStandardFunction::ToStringInteger, EvaluationValue::Integer(value)) => {
                    value.to_string()
                }
                (IrStandardFunction::ToStringReal, EvaluationValue::Real(value))
                    if value.is_finite() =>
                {
                    value.to_string()
                }
                (IrStandardFunction::ToStringReal, EvaluationValue::Integer(value)) => {
                    value.to_string()
                }
                (IrStandardFunction::ToStringString, EvaluationValue::String(value)) => value,
                _ => {
                    return Err(EvaluationFailure::Error(
                        "ToString argument does not match its resolved standard overload"
                            .to_owned(),
                    ));
                }
            };
            Ok(EvaluationValue::String(string))
        }
    }
}

fn numeric_ordering(
    left: &EvaluationValue,
    right: &EvaluationValue,
) -> Result<std::cmp::Ordering, EvaluationFailure> {
    let (left_value, left_unit) = numeric_value(left.clone())?;
    let (right_value, right_unit) = numeric_value(right.clone())?;
    if !left_value.is_finite() || !right_value.is_finite() {
        return Err(EvaluationFailure::Error(
            "numeric comparison requires finite values".to_owned(),
        ));
    }
    if left_unit != right_unit {
        return Err(EvaluationFailure::Error(
            "numeric comparison requires matching units".to_owned(),
        ));
    }
    left_value
        .partial_cmp(&right_value)
        .ok_or_else(|| EvaluationFailure::Error("numeric comparison is undefined".to_owned()))
}

fn evaluate_numeric_aggregate(
    function: IrStandardFunction,
    mut values: Vec<EvaluationValue>,
    result_type: &IrType,
) -> Result<EvaluationValue, EvaluationFailure> {
    let is_sum = function == IrStandardFunction::Sum;
    let Some(mut accumulator) = values.drain(..).next() else {
        let identity = if is_sum { 0.0 } else { 1.0 };
        return match (&result_type.value, &result_type.unit) {
            (IrValueType::Integer, _) => Ok(EvaluationValue::Integer(identity as i64)),
            (IrValueType::Real, _) => Ok(EvaluationValue::Real(identity)),
            (IrValueType::Quantity { .. }, Some(unit)) if is_sum => Ok(EvaluationValue::Quantity {
                value: identity,
                unit: unit.clone(),
            }),
            (IrValueType::Quantity { .. }, _) => Err(EvaluationFailure::Error(
                "empty quantity aggregate has no supported unit identity".to_owned(),
            )),
            _ => Err(EvaluationFailure::Error(
                "numeric aggregate result type is not scalar".to_owned(),
            )),
        };
    };

    if matches!(&result_type.value, IrValueType::Integer) {
        for value in values {
            let (EvaluationValue::Integer(left), EvaluationValue::Integer(right)) =
                (accumulator, value)
            else {
                return Err(EvaluationFailure::Error(
                    "integer aggregate received a non-Integer value".to_owned(),
                ));
            };
            let value = if is_sum {
                left.checked_add(right)
            } else {
                left.checked_mul(right)
            }
            .ok_or_else(|| EvaluationFailure::Error("Integer aggregate overflow".to_owned()))?;
            accumulator = EvaluationValue::Integer(value);
        }
    } else {
        for value in values {
            accumulator = if is_sum {
                numeric_binary(accumulator, value, |left, right| left + right)?
            } else {
                numeric_binary(accumulator, value, |left, right| left * right)?
            };
        }
    }
    coerce_numeric_result(accumulator, result_type)
}

fn coerce_numeric_result(
    value: EvaluationValue,
    result_type: &IrType,
) -> Result<EvaluationValue, EvaluationFailure> {
    match (&result_type.value, value) {
        (IrValueType::Integer, EvaluationValue::Integer(value)) => {
            Ok(EvaluationValue::Integer(value))
        }
        (IrValueType::Real, value) => {
            let (value, unit) = numeric_value(value)?;
            if unit.is_some() || !value.is_finite() {
                return Err(EvaluationFailure::Error(
                    "Real result received a non-unitless or non-finite value".to_owned(),
                ));
            }
            Ok(EvaluationValue::Real(value))
        }
        (IrValueType::Quantity { .. }, EvaluationValue::Quantity { value, unit })
            if result_type
                .unit
                .as_deref()
                .is_none_or(|expected| expected == unit) =>
        {
            Ok(EvaluationValue::Quantity { value, unit })
        }
        _ => Err(EvaluationFailure::Error(
            "standard function result did not match its inferred SysML type".to_owned(),
        )),
    }
}

fn finite_real(value: f64, function: &str) -> Result<EvaluationValue, EvaluationFailure> {
    if value.is_finite() {
        Ok(EvaluationValue::Real(value))
    } else {
        Err(EvaluationFailure::Error(format!(
            "{function} produced a non-finite result"
        )))
    }
}

fn checked_integer(value: f64, function: &str) -> Result<EvaluationValue, EvaluationFailure> {
    const I64_UPPER_EXCLUSIVE: f64 = 9_223_372_036_854_775_808.0;
    if value.is_finite() && value >= i64::MIN as f64 && value < I64_UPPER_EXCLUSIVE {
        Ok(EvaluationValue::Integer(value as i64))
    } else {
        Err(EvaluationFailure::Error(format!(
            "{function} result is outside the Integer range"
        )))
    }
}

fn validate_observation_contract(
    observation: &FeatureObservation,
    path: &SysmlFeaturePath,
) -> Result<(), EvaluationFailure> {
    let Some(contract) = &observation.contract else {
        return Ok(());
    };
    if contract.path != *path {
        return Err(EvaluationFailure::Error(
            "binding contract refers to a different SysML feature path".to_owned(),
        ));
    }
    if contract.provider != observation.provider {
        return Err(EvaluationFailure::Error(format!(
            "binding provider {:?} does not satisfy contract provider {:?}",
            observation.provider, contract.provider
        )));
    }
    for (label, expected, actual) in [
        ("unit", contract.unit.as_ref(), observation.unit.as_ref()),
        ("frame", contract.frame.as_ref(), observation.frame.as_ref()),
        (
            "time basis",
            contract.time_basis.as_ref(),
            observation.time_basis.as_ref(),
        ),
    ] {
        if let Some(expected) = expected {
            if actual != Some(expected) {
                return Err(EvaluationFailure::Error(format!(
                    "observation {label} {:?} does not satisfy binding contract {:?}",
                    actual, expected
                )));
            }
        }
    }
    if let Some(expected) = contract.source_revision {
        match observation.source_revision {
            Some(actual) if actual == expected => {}
            Some(actual) => {
                return Err(EvaluationFailure::Inconclusive(format!(
                    "provider revision {actual} is stale; contract requires {expected}"
                )));
            }
            None => {
                return Err(EvaluationFailure::Inconclusive(
                    "provider omitted the source revision required by its binding contract"
                        .to_owned(),
                ));
            }
        }
    }
    if let Some(IrValue::Quantity { unit, .. }) = observation.value.as_ref() {
        if observation
            .unit
            .as_ref()
            .is_some_and(|reported| reported != unit)
        {
            return Err(EvaluationFailure::Error(format!(
                "observation metadata unit {:?} disagrees with quantity value unit `{unit}`",
                observation.unit
            )));
        }
    }
    Ok(())
}

fn runtime_value(value: &IrValue) -> EvaluationValue {
    match value {
        IrValue::Integer(value) => EvaluationValue::Integer(*value),
        IrValue::Real(value) => EvaluationValue::Real(*value),
        IrValue::Boolean(value) => EvaluationValue::Boolean(*value),
        IrValue::String(value) => EvaluationValue::String(value.clone()),
        IrValue::Enumeration { type_name, literal } => EvaluationValue::Enumeration {
            type_name: type_name.clone(),
            literal: literal.clone(),
        },
        IrValue::Reference(target) => EvaluationValue::Reference(*target),
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

fn runtime_value_matches_type(value: &EvaluationValue, ty: &IrType) -> bool {
    if let EvaluationValue::Collection(values) = value {
        if !ty.multiplicity.is_collection()
            || values.len() < ty.multiplicity.lower
            || ty
                .multiplicity
                .upper
                .is_some_and(|upper| values.len() > upper)
        {
            return false;
        }
        return values
            .iter()
            .all(|value| runtime_value_matches_scalar_type(value, &ty.value, ty.unit.as_deref()));
    }
    !ty.multiplicity.is_collection()
        && runtime_value_matches_scalar_type(value, &ty.value, ty.unit.as_deref())
}

fn runtime_value_matches_scalar_type(
    value: &EvaluationValue,
    ty: &IrValueType,
    unit: Option<&str>,
) -> bool {
    match (ty, value) {
        (IrValueType::Unknown, _) => true,
        (IrValueType::Boolean, EvaluationValue::Boolean(_)) => true,
        (IrValueType::Integer, EvaluationValue::Integer(_)) => true,
        (IrValueType::Real, EvaluationValue::Integer(_) | EvaluationValue::Real(_)) => true,
        (IrValueType::String, EvaluationValue::String(_)) => true,
        (IrValueType::Quantity { .. }, EvaluationValue::Quantity { unit: actual, .. }) => {
            unit.is_none_or(|expected| expected == actual)
        }
        (
            IrValueType::Enumeration {
                type_name: expected,
            },
            EvaluationValue::Enumeration {
                type_name: actual, ..
            },
        ) => expected
            .as_ref()
            .is_none_or(|expected| actual.as_ref() == Some(expected)),
        (IrValueType::Reference { .. }, EvaluationValue::Reference(_)) => true,
        _ => false,
    }
}

fn runtime_references_match_snapshot(value: &EvaluationValue, path: &SysmlFeaturePath) -> bool {
    let snapshot = path.features()[0].element;
    match value {
        EvaluationValue::Reference(target) => {
            target.source_revision == snapshot.source_revision
                && target.source_fingerprint == snapshot.source_fingerprint
        }
        EvaluationValue::Collection(values) => values
            .iter()
            .all(|value| runtime_references_match_snapshot(value, path)),
        _ => true,
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
        IrOperator::Equal => compare_binary(operator, left, right, options, |ordering| {
            ordering == std::cmp::Ordering::Equal
        }),
        IrOperator::NotEqual => compare_binary(operator, left, right, options, |ordering| {
            ordering != std::cmp::Ordering::Equal
        }),
        IrOperator::Less => compare_binary(operator, left, right, options, |ordering| {
            ordering == std::cmp::Ordering::Less
        }),
        IrOperator::LessEqual => compare_binary(operator, left, right, options, |ordering| {
            ordering != std::cmp::Ordering::Greater
        }),
        IrOperator::Greater => compare_binary(operator, left, right, options, |ordering| {
            ordering == std::cmp::Ordering::Greater
        }),
        IrOperator::GreaterEqual => compare_binary(operator, left, right, options, |ordering| {
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
    operator: IrOperator,
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
                let value = compare_binary(operator, left, right, options, operation)?;
                result &= matches!(value, EvaluationValue::Boolean(true));
            }
            Ok(EvaluationValue::Boolean(result))
        }
        (left, right) => {
            let ordering = match (left, right) {
                (EvaluationValue::String(left), EvaluationValue::String(right)) => left.cmp(&right),
                (EvaluationValue::Boolean(left), EvaluationValue::Boolean(right)) => {
                    left.cmp(&right)
                }
                (
                    EvaluationValue::Enumeration {
                        type_name: left_type,
                        literal: left,
                    },
                    EvaluationValue::Enumeration {
                        type_name: right_type,
                        literal: right,
                    },
                ) if left_type == right_type
                    && matches!(operator, IrOperator::Equal | IrOperator::NotEqual) =>
                {
                    left.cmp(&right)
                }
                (EvaluationValue::Reference(left), EvaluationValue::Reference(right))
                    if matches!(operator, IrOperator::Equal | IrOperator::NotEqual) =>
                {
                    if left == right {
                        std::cmp::Ordering::Equal
                    } else {
                        std::cmp::Ordering::Greater
                    }
                }
                (EvaluationValue::Null, EvaluationValue::Null) => std::cmp::Ordering::Equal,
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
                    if equal {
                        std::cmp::Ordering::Equal
                    } else if left_number < right_number {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    }
                }
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
