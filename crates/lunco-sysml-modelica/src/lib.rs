//! Lower the neutral SysML constraint IR into standard Modelica admitted by
//! the repository's Rumoca boundary.
//!
//! This crate is deliberately downstream of [`lunco_sysml_ir`]. It does not
//! inspect SysML text, infer types from names, or implement a second Modelica
//! parser. The backend renders only typed IR nodes, then uses
//! `lunco-modelica-ast::parse_to_ast` so Rumoca owns Modelica syntax and
//! recovery/validation.

use lunco_modelica_ast::{StoredDefinition, parse_to_ast};
use lunco_sysml_ast::SysmlFeaturePath;
use lunco_sysml_ir::{
    CompiledConstraint, ConstraintIr, IrExpression, IrExpressionKind, IrLiteral, IrOperator,
    IrStandardConstant, IrStandardFunction, IrType, IrValueType,
};
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

/// A Modelica lowering failure is terminal for this adapter. Callers must not
/// run a solver on a partially rendered or syntactically recovered model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelicaLoweringError {
    InvalidSysmlIr,
    UnsupportedType(String),
    UnsupportedMultiplicity(String),
    InvalidExpression(String),
    RumocaParse(String),
}

impl Display for ModelicaLoweringError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSysmlIr => formatter.write_str("SysML constraint IR is invalid"),
            Self::UnsupportedType(message)
            | Self::UnsupportedMultiplicity(message)
            | Self::InvalidExpression(message)
            | Self::RumocaParse(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ModelicaLoweringError {}

#[derive(Clone, Copy)]
enum ModelicaStandardFunction {
    Abs,
    Min,
    Max,
    Sqrt,
    Floor,
    Round,
    Sin,
    Cos,
    Tan,
    Cot,
    ArcSin,
    ArcCos,
    ArcTan,
    Deg,
    Rad,
    IsZero,
    IsUnit,
    Size,
    IsEmpty,
    NotEmpty,
    Sum,
}

fn standard_function_lowering(function: IrStandardFunction) -> Option<ModelicaStandardFunction> {
    Some(match function {
        IrStandardFunction::Abs => ModelicaStandardFunction::Abs,
        IrStandardFunction::Min => ModelicaStandardFunction::Min,
        IrStandardFunction::Max => ModelicaStandardFunction::Max,
        IrStandardFunction::Sqrt => ModelicaStandardFunction::Sqrt,
        IrStandardFunction::Floor => ModelicaStandardFunction::Floor,
        IrStandardFunction::Round => ModelicaStandardFunction::Round,
        IrStandardFunction::Sin => ModelicaStandardFunction::Sin,
        IrStandardFunction::Cos => ModelicaStandardFunction::Cos,
        IrStandardFunction::Tan => ModelicaStandardFunction::Tan,
        IrStandardFunction::Cot => ModelicaStandardFunction::Cot,
        IrStandardFunction::ArcSin => ModelicaStandardFunction::ArcSin,
        IrStandardFunction::ArcCos => ModelicaStandardFunction::ArcCos,
        IrStandardFunction::ArcTan => ModelicaStandardFunction::ArcTan,
        IrStandardFunction::Deg => ModelicaStandardFunction::Deg,
        IrStandardFunction::Rad => ModelicaStandardFunction::Rad,
        IrStandardFunction::IsZero => ModelicaStandardFunction::IsZero,
        IrStandardFunction::IsUnit => ModelicaStandardFunction::IsUnit,
        IrStandardFunction::Size => ModelicaStandardFunction::Size,
        IrStandardFunction::IsEmpty => ModelicaStandardFunction::IsEmpty,
        IrStandardFunction::NotEmpty => ModelicaStandardFunction::NotEmpty,
        IrStandardFunction::Sum => ModelicaStandardFunction::Sum,
        IrStandardFunction::Product
        | IrStandardFunction::AllTrue
        | IrStandardFunction::AnyTrue
        | IrStandardFunction::ToStringBoolean
        | IrStandardFunction::ToStringInteger
        | IrStandardFunction::ToStringReal
        | IrStandardFunction::ToStringString => return None,
    })
}

/// Whether this backend can currently lower a supported standard-library call.
/// The same mapping drives both capability reporting and expression lowering.
pub fn supports_standard_function_lowering(function: IrStandardFunction) -> bool {
    standard_function_lowering(function).is_some()
}

/// A typed Modelica module and the exact source admitted through Rumoca.
#[derive(Clone, Debug)]
pub struct LoweredModelicaConstraint {
    pub model_name: String,
    pub source: String,
    pub ast: StoredDefinition,
    /// Exact SysML feature path represented by each generated Modelica input.
    pub feature_bindings: Vec<ModelicaFeatureBinding>,
}

/// Typed mapping from one resolved SysML feature path to its generated input.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelicaFeatureBinding {
    pub path: SysmlFeaturePath,
    pub variable: String,
    pub qualified_name: String,
    pub ty: IrType,
}

/// Lower one valid SysML constraint into a standalone Modelica model.
///
/// Feature references become Modelica components, scalar/quantity values map
/// to `Real`, and fixed SysML multiplicities map to Modelica array dimensions.
/// Quantity metadata is retained as standard Modelica type attributes; unit
/// conversion is intentionally not invented here and remains the provider or
/// engineering-values catalog's responsibility.
pub fn lower_constraint(
    compiled: &CompiledConstraint,
) -> Result<LoweredModelicaConstraint, ModelicaLoweringError> {
    if !compiled.is_valid() {
        return Err(ModelicaLoweringError::InvalidSysmlIr);
    }
    let constraint = compiled
        .constraint
        .as_ref()
        .ok_or(ModelicaLoweringError::InvalidSysmlIr)?;
    lower_constraint_ir(constraint)
}

fn lower_constraint_ir(
    constraint: &ConstraintIr,
) -> Result<LoweredModelicaConstraint, ModelicaLoweringError> {
    if constraint.expressions.is_empty() {
        return Err(ModelicaLoweringError::InvalidExpression(
            "cannot lower a constraint with no executable body".to_owned(),
        ));
    }

    let model_name = model_name(constraint);
    let mut features = BTreeMap::new();
    for expression in &constraint.expressions {
        collect_features(expression, &mut features)?;
    }

    let mut source = String::new();
    source.push_str("within LunCoSysML;\n");
    source.push_str("model ");
    source.push_str(&model_name);
    source.push_str("\n");
    for binding in features.values() {
        source.push_str("  ");
        source.push_str(&modelica_declaration(&binding.ty, &binding.variable)?);
        source.push_str(";\n");
    }
    source.push_str("equation\n");
    for expression in &constraint.expressions {
        source.push_str("  ");
        source.push_str(&modelica_equation(
            expression,
            &features,
            &constraint.qualified_name,
        )?);
        source.push_str(";\n");
    }
    source.push_str("end ");
    source.push_str(&model_name);
    source.push_str(";\n");

    let ast = parse_to_ast(&source, "sysml-constraint-ir.mo").map_err(|error| {
        ModelicaLoweringError::RumocaParse(format!("Rumoca rejected lowered Modelica: {error:#}"))
    })?;
    Ok(LoweredModelicaConstraint {
        model_name,
        source,
        ast,
        feature_bindings: features.into_values().collect(),
    })
}

fn collect_features(
    expression: &IrExpression,
    features: &mut BTreeMap<SysmlFeaturePath, ModelicaFeatureBinding>,
) -> Result<(), ModelicaLoweringError> {
    match &expression.kind {
        IrExpressionKind::FeatureReference {
            path,
            qualified_name,
        } => {
            features
                .entry(path.clone())
                .or_insert_with(|| ModelicaFeatureBinding {
                    path: path.clone(),
                    variable: feature_path_identifier(path),
                    qualified_name: qualified_name.clone(),
                    ty: expression.result_type.clone(),
                });
        }
        IrExpressionKind::StandardConstant { .. } | IrExpressionKind::Literal(_) => {}
        IrExpressionKind::Unary { operand, .. } | IrExpressionKind::Group(operand) => {
            collect_features(operand, features)?;
        }
        IrExpressionKind::Binary { left, right, .. } => {
            collect_features(left, features)?;
            collect_features(right, features)?;
        }
        IrExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            collect_features(condition, features)?;
            collect_features(when_true, features)?;
            collect_features(when_false, features)?;
        }
        IrExpressionKind::Invocation { arguments, .. } => {
            for argument in arguments {
                collect_features(argument, features)?;
            }
        }
        IrExpressionKind::Index { collection, index } => {
            collect_features(collection, features)?;
            collect_features(index, features)?;
        }
        IrExpressionKind::Collection(elements) => {
            for element in elements {
                collect_features(element, features)?;
            }
        }
    }
    Ok(())
}

fn modelica_declaration(ty: &IrType, name: &str) -> Result<String, ModelicaLoweringError> {
    let (base, attributes) = match &ty.value {
        IrValueType::Boolean => ("Boolean".to_owned(), Vec::new()),
        IrValueType::Integer => ("Integer".to_owned(), Vec::new()),
        IrValueType::Real => ("Real".to_owned(), Vec::new()),
        IrValueType::Quantity { quantity_kind } => {
            let mut attributes = Vec::new();
            if let Some(quantity_kind) = quantity_kind {
                attributes.push(format!("quantity=\"{}\"", modelica_string(quantity_kind)));
            }
            if let Some(unit) = &ty.unit {
                attributes.push(format!("unit=\"{}\"", modelica_string(unit)));
            }
            ("Real".to_owned(), attributes)
        }
        IrValueType::String => ("String".to_owned(), Vec::new()),
        IrValueType::Enumeration { .. }
        | IrValueType::Reference { .. }
        | IrValueType::Structured { .. }
        | IrValueType::Rational
        | IrValueType::Complex
        | IrValueType::Unknown => {
            return Err(ModelicaLoweringError::UnsupportedType(format!(
                "feature `{name}` has no scalar Modelica representation"
            )));
        }
    };
    let dimensions = match ty.multiplicity.upper {
        Some(1) if ty.multiplicity.lower == 1 => String::new(),
        Some(upper) if ty.multiplicity.lower == upper && upper > 0 => format!("[{upper}]"),
        Some(upper) => {
            return Err(ModelicaLoweringError::UnsupportedMultiplicity(format!(
                "feature `{name}` has non-fixed multiplicity {}..{upper:?}",
                ty.multiplicity.lower
            )));
        }
        None => {
            return Err(ModelicaLoweringError::UnsupportedMultiplicity(format!(
                "feature `{name}` has unbounded multiplicity"
            )));
        }
    };
    let attributes = if attributes.is_empty() {
        String::new()
    } else {
        format!("({})", attributes.join(", "))
    };
    Ok(format!("{base} {name}{dimensions}{attributes}"))
}

fn modelica_equation(
    expression: &IrExpression,
    features: &BTreeMap<SysmlFeaturePath, ModelicaFeatureBinding>,
    constraint_name: &str,
) -> Result<String, ModelicaLoweringError> {
    if let IrExpressionKind::Binary {
        operator: IrOperator::Equal,
        left,
        right,
    } = &expression.kind
    {
        return Ok(format!(
            "{} = {}",
            modelica_expression(left, features)?,
            modelica_expression(right, features)?
        ));
    }
    Ok(format!(
        "assert({}, \"SysML constraint {}\")",
        modelica_expression(expression, features)?,
        modelica_string(constraint_name)
    ))
}

fn modelica_expression(
    expression: &IrExpression,
    features: &BTreeMap<SysmlFeaturePath, ModelicaFeatureBinding>,
) -> Result<String, ModelicaLoweringError> {
    match &expression.kind {
        IrExpressionKind::FeatureReference {
            path,
            qualified_name,
        } => {
            let Some(binding) = features.get(path) else {
                return Err(ModelicaLoweringError::InvalidExpression(format!(
                    "feature path for `{qualified_name}` was not declared in the lowered model"
                )));
            };
            Ok(binding.variable.clone())
        }
        IrExpressionKind::StandardConstant { constant, .. } => Ok(match constant {
            IrStandardConstant::Pi => "Modelica.Constants.pi".to_owned(),
        }),
        IrExpressionKind::Literal(literal) => Ok(match literal {
            IrLiteral::Integer(value) => value.to_string(),
            IrLiteral::Real(value) => format!("{value:.17}"),
            IrLiteral::Boolean(value) => value.to_string(),
            IrLiteral::String(value) => format!("\"{}\"", modelica_string(value)),
            IrLiteral::Null => {
                return Err(ModelicaLoweringError::InvalidExpression(
                    "null cannot be lowered to a Modelica scalar expression".to_owned(),
                ));
            }
        }),
        IrExpressionKind::Unary { operator, operand } => {
            let operand = modelica_expression(operand, features)?;
            let operator = match operator {
                IrOperator::Positive => "+",
                IrOperator::Negative => "-",
                IrOperator::Not => "not ",
                _ => {
                    return Err(ModelicaLoweringError::InvalidExpression(
                        "invalid unary operator in Modelica lowering".to_owned(),
                    ));
                }
            };
            Ok(format!("({operator}{operand})"))
        }
        IrExpressionKind::Binary {
            operator,
            left,
            right,
        } => {
            let left = modelica_expression(left, features)?;
            let right = modelica_expression(right, features)?;
            if *operator == IrOperator::Implies {
                return Ok(format!("((not {left}) or {right})"));
            }
            if *operator == IrOperator::Equivalent {
                return Ok(format!(
                    "(({left} and {right}) or ((not {left}) and (not {right})))"
                ));
            }
            let operator = match operator {
                IrOperator::Add => "+",
                IrOperator::Subtract => "-",
                IrOperator::Multiply => "*",
                IrOperator::Divide => "/",
                IrOperator::Power => "^",
                IrOperator::Equal => "==",
                IrOperator::NotEqual => "<>",
                IrOperator::Less => "<",
                IrOperator::LessEqual => "<=",
                IrOperator::Greater => ">",
                IrOperator::GreaterEqual => ">=",
                IrOperator::And => "and",
                IrOperator::Or => "or",
                _ => {
                    return Err(ModelicaLoweringError::InvalidExpression(
                        "invalid binary operator in Modelica lowering".to_owned(),
                    ));
                }
            };
            Ok(format!("({left} {operator} {right})"))
        }
        IrExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => Ok(format!(
            "(if {} then {} else {})",
            modelica_expression(condition, features)?,
            modelica_expression(when_true, features)?,
            modelica_expression(when_false, features)?
        )),
        IrExpressionKind::Index { collection, index } => Ok(format!(
            "{}[{}]",
            modelica_expression(collection, features)?,
            modelica_expression(index, features)?
        )),
        IrExpressionKind::Collection(elements) => {
            let elements = elements
                .iter()
                .map(|element| modelica_expression(element, features))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!("{{{}}}", elements.join(", ")))
        }
        IrExpressionKind::Invocation {
            function,
            arguments,
            ..
        } => {
            let lowering = standard_function_lowering(*function).ok_or_else(|| {
                ModelicaLoweringError::InvalidExpression(format!(
                    "Modelica lowering does not yet implement the {function:?} standard function"
                ))
            })?;
            let arguments = arguments
                .iter()
                .map(|argument| modelica_expression(argument, features))
                .collect::<Result<Vec<_>, _>>()?;
            let unary = || {
                arguments.first().cloned().ok_or_else(|| {
                    ModelicaLoweringError::InvalidExpression(
                        "standard function argument is missing".to_owned(),
                    )
                })
            };
            Ok(match lowering {
                ModelicaStandardFunction::Abs => format!("abs({})", unary()?),
                ModelicaStandardFunction::Min | ModelicaStandardFunction::Max => {
                    let left = arguments.first().ok_or_else(|| {
                        ModelicaLoweringError::InvalidExpression(
                            "standard function first argument is missing".to_owned(),
                        )
                    })?;
                    let right = arguments.get(1).ok_or_else(|| {
                        ModelicaLoweringError::InvalidExpression(
                            "standard function second argument is missing".to_owned(),
                        )
                    })?;
                    let name = if matches!(lowering, ModelicaStandardFunction::Min) {
                        "min"
                    } else {
                        "max"
                    };
                    format!("{name}({left}, {right})")
                }
                ModelicaStandardFunction::Sqrt => format!("sqrt({})", unary()?),
                ModelicaStandardFunction::Floor => format!("floor({})", unary()?),
                ModelicaStandardFunction::Round => {
                    let value = unary()?;
                    format!(
                        "(if ({value}) >= 0 then floor(({value}) + 0.5) else -floor(-({value}) + 0.5))"
                    )
                }
                ModelicaStandardFunction::Sin => format!("sin({})", unary()?),
                ModelicaStandardFunction::Cos => format!("cos({})", unary()?),
                ModelicaStandardFunction::Tan => format!("tan({})", unary()?),
                ModelicaStandardFunction::Cot => {
                    let value = unary()?;
                    format!("(cos({value}) / sin({value}))")
                }
                ModelicaStandardFunction::ArcSin => format!("asin({})", unary()?),
                ModelicaStandardFunction::ArcCos => format!("acos({})", unary()?),
                ModelicaStandardFunction::ArcTan => format!("atan({})", unary()?),
                ModelicaStandardFunction::Deg => {
                    format!("({} * 180 / Modelica.Constants.pi)", unary()?)
                }
                ModelicaStandardFunction::Rad => {
                    format!("({} * Modelica.Constants.pi / 180)", unary()?)
                }
                ModelicaStandardFunction::IsZero => format!("({} == 0)", unary()?),
                ModelicaStandardFunction::IsUnit => format!("({} == 1)", unary()?),
                ModelicaStandardFunction::Size => format!("size({}, 1)", unary()?),
                ModelicaStandardFunction::IsEmpty => format!("(size({}, 1) == 0)", unary()?),
                ModelicaStandardFunction::NotEmpty => format!("(size({}, 1) <> 0)", unary()?),
                ModelicaStandardFunction::Sum => format!("sum({})", unary()?),
            })
        }
        IrExpressionKind::Group(child) => {
            Ok(format!("({})", modelica_expression(child, features)?))
        }
    }
}

fn identifier(qualified_name: &str) -> String {
    let leaf = qualified_name.rsplit("::").next().unwrap_or(qualified_name);
    let mut output = String::with_capacity(leaf.len());
    for character in leaf.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            output.push(character);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        output.push_str("feature");
    }
    if output
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit())
    {
        output.insert(0, '_');
    }
    output
}

fn feature_path_identifier(path: &SysmlFeaturePath) -> String {
    let first = path
        .features()
        .first()
        .expect("validated SysML feature paths are non-empty")
        .element;
    let mut name = format!(
        "sysml_r{}_s{:016x}_f{}",
        first.source_revision, first.source_fingerprint, first.element_id
    );
    for feature in path.features().iter().skip(1) {
        name.push_str("_f");
        name.push_str(&feature.element.element_id.to_string());
    }
    name
}

fn model_name(constraint: &ConstraintIr) -> String {
    let mut name = format!("SysmlConstraint_{}", identifier(&constraint.qualified_name));
    name.push('_');
    name.push_str(&format!("{:016x}", constraint.fingerprint));
    name
}

fn modelica_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_sysml_ast::SysmlAnalysis;
    use lunco_sysml_ir::compile_constraint;

    #[test]
    fn lowers_one_resolved_constraint_through_rumoca_smoke() {
        let analysis = SysmlAnalysis::from_files_without_stdlib([(
            "smoke.sysml",
            "package P { constraint def C { in a : Real; a == 1.0; } }",
        )]);
        let constraint = analysis.constraints().first().expect("constraint");
        let compiled = compile_constraint(&analysis, constraint);
        let lowered = lower_constraint(&compiled).expect("Rumoca Modelica AST");

        assert!(lowered.source.contains("equation"));
        assert_eq!(lowered.ast.classes.len(), 1);
    }
}
