//! Lower the neutral SysML constraint IR into standard Modelica admitted by
//! the repository's Rumoca boundary.
//!
//! This crate is deliberately downstream of [`lunco_sysml_ir`]. It does not
//! inspect SysML text, infer types from names, or implement a second Modelica
//! parser. The backend renders only typed IR nodes, then uses
//! `lunco-modelica-ast::parse_to_ast` so Rumoca owns Modelica syntax and
//! recovery/validation.

use lunco_modelica_ast::{parse_to_ast, StoredDefinition};
use lunco_sysml_ir::{
    CompiledConstraint, ConstraintIr, IrExpression, IrExpressionKind, IrLiteral, IrOperator,
    IrType, IrValueType,
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

/// A typed Modelica module and the exact source admitted through Rumoca.
#[derive(Clone, Debug)]
pub struct LoweredModelicaConstraint {
    pub model_name: String,
    pub source: String,
    pub ast: StoredDefinition,
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
    for (name, (_, ty)) in &features {
        source.push_str("  ");
        source.push_str(&modelica_declaration(ty, name)?);
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
    })
}

fn collect_features(
    expression: &IrExpression,
    features: &mut BTreeMap<String, (String, IrType)>,
) -> Result<(), ModelicaLoweringError> {
    match &expression.kind {
        IrExpressionKind::FeatureReference { qualified_name, .. } => {
            let name = identifier(qualified_name);
            if let Some((existing, _)) = features.get(&name) {
                if existing != qualified_name {
                    return Err(ModelicaLoweringError::InvalidExpression(format!(
                        "SysML features `{existing}` and `{qualified_name}` collide as Modelica identifier `{name}`"
                    )));
                }
            } else {
                features.insert(
                    name,
                    (qualified_name.clone(), expression.result_type.clone()),
                );
            }
        }
        IrExpressionKind::Literal(_) => {}
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
        IrValueType::Enumeration { .. } | IrValueType::Structured { .. } | IrValueType::Unknown => {
            return Err(ModelicaLoweringError::UnsupportedType(format!(
                "feature `{name}` has no scalar Modelica representation"
            )))
        }
    };
    let dimensions = match ty.multiplicity.upper {
        Some(1) if ty.multiplicity.lower == 1 => String::new(),
        Some(upper) if ty.multiplicity.lower == upper && upper > 0 => format!("[{upper}]"),
        Some(upper) => {
            return Err(ModelicaLoweringError::UnsupportedMultiplicity(format!(
                "feature `{name}` has non-fixed multiplicity {}..{upper:?}",
                ty.multiplicity.lower
            )))
        }
        None => {
            return Err(ModelicaLoweringError::UnsupportedMultiplicity(format!(
                "feature `{name}` has unbounded multiplicity"
            )))
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
    features: &BTreeMap<String, (String, IrType)>,
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
    features: &BTreeMap<String, (String, IrType)>,
) -> Result<String, ModelicaLoweringError> {
    match &expression.kind {
        IrExpressionKind::FeatureReference { qualified_name, .. } => {
            let name = identifier(qualified_name);
            if !features.contains_key(&name) {
                return Err(ModelicaLoweringError::InvalidExpression(format!(
                    "feature `{qualified_name}` was not declared in the lowered model"
                )));
            }
            Ok(name)
        }
        IrExpressionKind::Literal(literal) => Ok(match literal {
            IrLiteral::Integer(value) => value.to_string(),
            IrLiteral::Real(value) => format!("{value:.17}"),
            IrLiteral::Boolean(value) => value.to_string(),
            IrLiteral::String(value) => format!("\"{}\"", modelica_string(value)),
            IrLiteral::Null => {
                return Err(ModelicaLoweringError::InvalidExpression(
                    "null cannot be lowered to a Modelica scalar expression".to_owned(),
                ))
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
                    ))
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
                    ))
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
