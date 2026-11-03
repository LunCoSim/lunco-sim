//! Checked scalar interface projection and compiler-private parameter admission.

use lunco_modelica_ast::resolved_interface::{
    ModelicaCausality, ModelicaInputDefaults, ModelicaInputGroup, ModelicaScalarBound,
    ModelicaScalarDeclaration, ModelicaScalarType, ModelicaScalarValue, ModelicaVariability,
    ResolvedModelicaInterface,
};
use rumoca_eval_dae::{NumericEvaluationErrorKind, NumericEvaluator};
use rumoca_ir_dae::{
    Dae, DaeView, ExprId, ExpressionVariability, ScalarType, VariableRole, VariableView,
};

use super::ModelicaCompiler;

impl ModelicaCompiler {
    pub(super) fn checked_scalar_type(kind: ScalarType) -> Option<ModelicaScalarType> {
        match kind {
            ScalarType::Real => Some(ModelicaScalarType::Real),
            ScalarType::Integer => Some(ModelicaScalarType::Integer),
            ScalarType::Boolean => Some(ModelicaScalarType::Boolean),
            ScalarType::String => Some(ModelicaScalarType::String),
            ScalarType::Enumeration => Some(ModelicaScalarType::Enumeration),
            ScalarType::Record => None,
        }
    }

    pub(super) fn raw_parameter_value(
        target: &rumoca_compile::compile::ClassComponentReferenceDeclaration,
        value: f64,
    ) -> Result<ModelicaScalarValue, String> {
        if target.is_final || target.is_protected {
            return Err("parameter binding requires a public, non-final declaration".into());
        }
        let declaration = &target.declaration;
        if !target.is_indexed
            && (declaration.has_type_dimensions
                || !declaration.component.shape.is_empty()
                || !declaration.component.shape_expr.is_empty())
        {
            return Err(
                "array parameter binding requires an exact scalar element reference".into(),
            );
        }
        let kind = match declaration.scalar_type {
            Some(rumoca_core::PredefinedComponentType::Real) => ModelicaScalarType::Real,
            Some(rumoca_core::PredefinedComponentType::Integer) => ModelicaScalarType::Integer,
            Some(rumoca_core::PredefinedComponentType::Boolean) => ModelicaScalarType::Boolean,
            _ => return Err("parameter binding requires a numeric scalar declaration".into()),
        };
        ModelicaScalarValue::from_numeric(kind, value)
    }

    /// Project the actual scalar instance before solver construction. Bound
    /// evaluation uses the admitted parameter values, including tunables on
    /// reused DAE artifacts; unsolved dependencies remain explicit facts.
    pub fn interface_from_checked_dae(
        model_name: &str,
        dae: &Dae,
        parameter_overrides: &[(String, f64)],
    ) -> Result<ResolvedModelicaInterface, String> {
        Self::validate_parameter_bindings(dae, parameter_overrides)?;
        let overrides = parameter_overrides
            .iter()
            .map(|(name, value)| (name.as_str(), *value))
            .collect::<std::collections::HashMap<_, _>>();
        dae.inspect(|view| {
            let mut evaluator = NumericEvaluator::with_parameter_bindings(
                view,
                |variable: VariableView<'_>, scalar| {
                    if variable.role() == VariableRole::Parameter && variable.is_tunable() {
                        variable
                            .scalar_name(scalar)
                            .and_then(|name| overrides.get(name.as_str()).copied())
                    } else {
                        None
                    }
                },
            );
            let mut declarations = std::collections::BTreeMap::new();
            let mut input_groups = Vec::new();
            for (_, variable) in view.variables() {
                let kind = variable.value_type().scalar_type();
                let scalar_type = Self::checked_scalar_type(kind);
                let observable = matches!(
                    kind,
                    ScalarType::Real
                        | ScalarType::Integer
                        | ScalarType::Boolean
                        | ScalarType::Enumeration
                ) && matches!(
                    variable.role(),
                    VariableRole::State
                        | VariableRole::Algebraic
                        | VariableRole::Output
                        | VariableRole::DiscreteReal
                        | VariableRole::DiscreteValue
                );
                let causality = if variable.role() == VariableRole::Input {
                    ModelicaCausality::Input
                } else if variable.causality() == rumoca_ir_dae::VariableCausality::Output {
                    ModelicaCausality::Output
                } else {
                    ModelicaCausality::None
                };
                let variability = match variable.variability() {
                    ExpressionVariability::Constant => ModelicaVariability::Constant,
                    ExpressionVariability::Parameter => ModelicaVariability::Parameter,
                    ExpressionVariability::Discrete => ModelicaVariability::Discrete,
                    ExpressionVariability::Continuous => ModelicaVariability::Continuous,
                };
                if variable.role() == VariableRole::Input {
                    let default_values = match variable.binding() {
                        None => None,
                        Some(_)
                            if !matches!(
                                kind,
                                ScalarType::Real | ScalarType::Integer | ScalarType::Boolean
                            ) =>
                        {
                            Some(ModelicaInputDefaults::Unsupported)
                        }
                        Some(binding) => {
                            match static_values(view, variable, binding, &mut evaluator) {
                                Ok(Some(values)) => Some(ModelicaInputDefaults::Known(values)),
                                Ok(None) => Some(ModelicaInputDefaults::InitializationDependent),
                                Err(StaticValueError::Unsupported) => {
                                    Some(ModelicaInputDefaults::Unsupported)
                                }
                                Err(StaticValueError::Invalid(error)) => return Err(error),
                            }
                        }
                    };
                    input_groups.push(ModelicaInputGroup {
                        name: variable.name().as_str().to_owned(),
                        dimensions: variable.value_type().dimensions().to_vec(),
                        scalar_names: variable.scalar_names().collect(),
                        scalar_type,
                        aggregate_name_is_indexed: rumoca_core::qualified_name_segments(
                            variable.name().as_str(),
                        )
                        .any(|segment| rumoca_core::parse_scalar_name(segment).is_some()),
                        default_values,
                    });
                }
                let minimum = scalar_bounds(view, variable, variable.minimum(), &mut evaluator)?;
                let maximum = scalar_bounds(view, variable, variable.maximum(), &mut evaluator)?;
                for (index, name) in variable.scalar_names().enumerate() {
                    let declaration = ModelicaScalarDeclaration {
                        scalar_type,
                        causality,
                        variability,
                        is_parameter: variable.role() == VariableRole::Parameter,
                        is_tunable: variable.is_tunable(),
                        observable,
                        description: variable.description().map(str::to_owned),
                        unit: variable.unit().map(str::to_owned),
                        minimum: minimum.as_ref().map(|values| values[index].clone()),
                        maximum: maximum.as_ref().map(|values| values[index].clone()),
                    };
                    if declarations.insert(name.clone(), declaration).is_some() {
                        return Err(format!(
                            "checked Modelica interface duplicates scalar `{name}`"
                        ));
                    }
                }
            }
            Ok(ResolvedModelicaInterface {
                model_name: model_name.to_owned(),
                declarations,
                input_groups,
            })
        })
    }
}

enum StaticValueError {
    Unsupported,
    Invalid(String),
}

/// Evaluate a checked expression in the admitted parameter context. None means
/// numerical initialization is still required; no start-value substitution occurs.
fn static_values<'dae, F>(
    view: DaeView<'dae>,
    variable: VariableView<'dae>,
    expression: ExprId<'dae>,
    evaluator: &mut NumericEvaluator<'dae, F>,
) -> Result<Option<Vec<f64>>, StaticValueError>
where
    F: FnMut(VariableView<'dae>, usize) -> Option<f64>,
{
    let expression_view = view.expression(expression).ok_or_else(|| {
        StaticValueError::Invalid(format!(
            "checked expression for `{}` is missing",
            variable.name()
        ))
    })?;
    if expression_view.variability() > ExpressionVariability::Parameter {
        return Ok(None);
    }
    match evaluator.expression(expression) {
        Ok(mut values) => {
            variable.broadcast_values(&mut values);
            if values.len() != variable.scalar_count() {
                return Err(StaticValueError::Invalid(format!(
                    "checked expression for `{}` has {} values for {} scalars",
                    variable.name(),
                    values.len(),
                    variable.scalar_count()
                )));
            }
            Ok(Some(values))
        }
        Err(error)
            if matches!(
                error.kind(),
                NumericEvaluationErrorKind::MissingValue
                    | NumericEvaluationErrorKind::NonStaticCoordinate
                    | NumericEvaluationErrorKind::CyclicDependency
            ) =>
        {
            Ok(None)
        }
        Err(error) if error.kind() == NumericEvaluationErrorKind::UnsupportedOperation => {
            Err(StaticValueError::Unsupported)
        }
        Err(error) => Err(StaticValueError::Invalid(format!(
            "could not evaluate checked expression for `{}`: {error}",
            variable.name()
        ))),
    }
}

fn scalar_bounds<'dae, F>(
    view: DaeView<'dae>,
    variable: VariableView<'dae>,
    expression: Option<ExprId<'dae>>,
    evaluator: &mut NumericEvaluator<'dae, F>,
) -> Result<Option<Vec<ModelicaScalarBound>>, String>
where
    F: FnMut(VariableView<'dae>, usize) -> Option<f64>,
{
    let Some(expression) = expression else {
        return Ok(None);
    };
    match static_values(view, variable, expression, evaluator) {
        Ok(Some(values)) => Ok(Some(
            values.into_iter().map(ModelicaScalarBound::Known).collect(),
        )),
        Ok(None) => Ok(Some(vec![
            ModelicaScalarBound::InitializationDependent;
            variable.scalar_count()
        ])),
        Err(StaticValueError::Unsupported) => Err(format!(
            "numeric evaluation does not support the checked bound for `{}`",
            variable.name()
        )),
        Err(StaticValueError::Invalid(error)) => Err(error),
    }
}
