//! Project standard SysML measurement-unit definitions into SI dimensions and
//! scale factors without interpreting unit symbols.

use std::collections::{HashMap, HashSet};

use sysml_model::{ElementId, ElementKind, Value};
use sysml_semantics::Workspace;

use crate::{SysmlNumber, SysmlUnitDefinition, semantic_type_id};

#[derive(Clone, Copy)]
struct UnitConversion {
    reference: ElementId,
    factor: f64,
    is_exact: bool,
}

struct UnitDeclaration {
    dimension: [i8; 7],
    conversion: Option<UnitConversion>,
    initializer: Option<ElementId>,
}

#[derive(Clone, Copy)]
struct UnitExpressionValue {
    dimension: [i8; 7],
    scale_to_si: f64,
    is_exact: bool,
}

#[derive(Clone, Copy)]
struct ResolvedScale {
    value: f64,
    is_exact: bool,
}

/// Snapshot-local source of resolved linear units.
pub(super) struct SysmlUnitResolver {
    definitions: HashMap<ElementId, SysmlUnitDefinition>,
}

impl SysmlUnitResolver {
    pub(super) fn new(workspace: &mut Workspace) -> Self {
        let model_ids = workspace.model().ids().collect::<Vec<_>>();
        let measurement_unit =
            semantic_type_id(workspace, "MeasurementReferences::MeasurementUnit");
        let unit_conversion = semantic_type_id(workspace, "MeasurementReferences::UnitConversion");
        let conversion_by_prefix =
            semantic_type_id(workspace, "MeasurementReferences::ConversionByPrefix");
        let dimension_one_unit =
            semantic_type_id(workspace, "MeasurementReferences::DimensionOneUnit");
        let qpf_type = semantic_type_id(workspace, "Quantities::QuantityPowerFactor");
        let qpf_quantity = semantic_type_id(workspace, "Quantities::QuantityPowerFactor::quantity");
        let qpf_exponent = semantic_type_id(workspace, "Quantities::QuantityPowerFactor::exponent");
        let conversion_reference = semantic_type_id(
            workspace,
            "MeasurementReferences::UnitConversion::referenceUnit",
        );
        let conversion_factor = semantic_type_id(
            workspace,
            "MeasurementReferences::UnitConversion::conversionFactor",
        );
        let conversion_is_exact =
            semantic_type_id(workspace, "MeasurementReferences::UnitConversion::isExact");
        let prefix_property = semantic_type_id(
            workspace,
            "MeasurementReferences::ConversionByPrefix::prefix",
        );
        let prefix_factor = semantic_type_id(
            workspace,
            "MeasurementReferences::UnitPrefix::conversionFactor",
        );

        let mut base_quantity_axes = HashMap::new();
        for (qualified_name, axis) in [
            ("ISQBase::International System of Quantities::L", 0),
            ("ISQBase::International System of Quantities::M", 1),
            ("ISQBase::International System of Quantities::T", 2),
            ("ISQBase::International System of Quantities::I", 3),
            ("ISQBase::International System of Quantities::Θ", 4),
            ("ISQBase::International System of Quantities::N", 5),
            ("ISQBase::International System of Quantities::J", 6),
        ] {
            if let Some(quantity) = semantic_type_id(workspace, qualified_name) {
                base_quantity_axes.insert(quantity, axis);
            }
        }

        let mut declarations = HashMap::new();
        let mut invalid = HashSet::new();
        if let (Some(measurement_unit), Some(qpf_type), Some(qpf_quantity), Some(qpf_exponent)) =
            (measurement_unit, qpf_type, qpf_quantity, qpf_exponent)
        {
            for feature in model_ids.iter().copied() {
                if !workspace.model().kind(feature).is_a(ElementKind::Feature) {
                    continue;
                }
                let unit_types = workspace.model().types_of(feature).collect::<Vec<_>>();
                let mut applicable_types = Vec::new();
                for ty in unit_types {
                    if inherits(workspace, ty, measurement_unit) {
                        applicable_types.push(ty);
                    }
                }
                if applicable_types.is_empty() {
                    continue;
                }

                let dimensions = applicable_types
                    .iter()
                    .map(|&ty| {
                        quantity_dimension(
                            workspace,
                            ty,
                            qpf_type,
                            qpf_quantity,
                            qpf_exponent,
                            dimension_one_unit,
                            &base_quantity_axes,
                        )
                    })
                    .collect::<Option<Vec<_>>>();
                let Some(dimensions) = dimensions else {
                    invalid.insert(feature);
                    continue;
                };
                let Some(dimension) = dimensions.first().copied() else {
                    invalid.insert(feature);
                    continue;
                };
                if dimensions.iter().any(|candidate| *candidate != dimension) {
                    invalid.insert(feature);
                    continue;
                }

                let (conversion, malformed_conversion) = unit_conversion_for(
                    workspace,
                    feature,
                    unit_conversion,
                    conversion_by_prefix,
                    conversion_reference,
                    conversion_factor,
                    conversion_is_exact,
                    prefix_property,
                    prefix_factor,
                );
                if malformed_conversion {
                    invalid.insert(feature);
                    continue;
                }
                let initializer = feature_value_expression(workspace.model(), feature);
                declarations.insert(
                    feature,
                    UnitDeclaration {
                        dimension,
                        conversion,
                        initializer,
                    },
                );
            }
        }

        for (&unit, declaration) in &declarations {
            if declaration.conversion.is_some_and(|conversion| {
                declarations
                    .get(&conversion.reference)
                    .is_some_and(|reference| reference.dimension != declaration.dimension)
            }) {
                invalid.insert(unit);
            }
        }

        let mut coherent_units = HashMap::new();
        for (qualified_name, expected_dimension) in [
            ("SI::metre", [1, 0, 0, 0, 0, 0, 0]),
            ("SI::kilogram", [0, 1, 0, 0, 0, 0, 0]),
            ("SI::second", [0, 0, 1, 0, 0, 0, 0]),
            ("SI::ampere", [0, 0, 0, 1, 0, 0, 0]),
            ("SI::kelvin", [0, 0, 0, 0, 1, 0, 0]),
            ("SI::mole", [0, 0, 0, 0, 0, 1, 0]),
            ("SI::candela", [0, 0, 0, 0, 0, 0, 1]),
            ("MeasurementReferences::one", [0; 7]),
        ] {
            if let Some(unit) = semantic_type_id(workspace, qualified_name) {
                match declarations.get(&unit) {
                    Some(declaration) if declaration.dimension == expected_dimension => {
                        coherent_units.insert(
                            unit,
                            ResolvedScale {
                                value: 1.0,
                                is_exact: true,
                            },
                        );
                    }
                    Some(_) => {
                        invalid.insert(unit);
                    }
                    None => {}
                }
            }
        }

        let mut blocked = invalid;
        let scales = loop {
            let (scales, mut conflicts) =
                resolve_scales(workspace, &declarations, &blocked, &coherent_units);
            for (&unit, declaration) in &declarations {
                if blocked.contains(&unit) {
                    continue;
                }
                if declaration.initializer.is_some() && !scales.contains_key(&unit) {
                    conflicts.insert(unit);
                }
            }
            if conflicts.is_empty() {
                break scales;
            }
            let previous_len = blocked.len();
            blocked.extend(conflicts);
            if blocked.len() == previous_len {
                break scales;
            }
        };

        let definitions = declarations
            .into_iter()
            .filter_map(|(feature, declaration)| {
                if blocked.contains(&feature) {
                    return None;
                }
                let scale = *scales.get(&feature)?;
                Some((
                    feature,
                    SysmlUnitDefinition {
                        dimension: declaration.dimension,
                        scale_to_si: SysmlNumber::new(scale.value)?,
                        offset_to_si: SysmlNumber::new(0.0)?,
                        scale_is_exact: scale.is_exact,
                    },
                ))
            })
            .collect();
        Self { definitions }
    }

    pub(super) fn definition(&self, feature: ElementId) -> Option<SysmlUnitDefinition> {
        self.definitions.get(&feature).copied()
    }
}

fn record_scale(
    unit: ElementId,
    candidate: ResolvedScale,
    scales: &mut HashMap<ElementId, ResolvedScale>,
    conflicts: &mut HashSet<ElementId>,
) -> bool {
    if !candidate.value.is_finite() || candidate.value <= 0.0 {
        conflicts.insert(unit);
        scales.remove(&unit);
        return true;
    }
    match scales.get(&unit).copied() {
        Some(current) => {
            let tolerance = 1.0e-12 * current.value.abs().max(candidate.value.abs()).max(1.0);
            if (current.value - candidate.value).abs() > tolerance {
                conflicts.insert(unit);
                scales.remove(&unit);
                true
            } else {
                let is_exact = current.is_exact && candidate.is_exact;
                if current.is_exact != is_exact {
                    scales.insert(
                        unit,
                        ResolvedScale {
                            value: current.value,
                            is_exact,
                        },
                    );
                    true
                } else {
                    false
                }
            }
        }
        None => {
            scales.insert(unit, candidate);
            true
        }
    }
}

fn resolve_scales(
    workspace: &mut Workspace,
    declarations: &HashMap<ElementId, UnitDeclaration>,
    blocked: &HashSet<ElementId>,
    coherent_units: &HashMap<ElementId, ResolvedScale>,
) -> (HashMap<ElementId, ResolvedScale>, HashSet<ElementId>) {
    let dimensions = declarations
        .iter()
        .map(|(&unit, declaration)| (unit, declaration.dimension))
        .collect::<HashMap<_, _>>();
    let mut scales = coherent_units
        .iter()
        .filter(|(unit, _)| !blocked.contains(unit))
        .map(|(&unit, &scale)| (unit, scale))
        .collect::<HashMap<_, _>>();
    let mut conflicts = HashSet::new();
    loop {
        let mut changed = false;
        for (&unit, declaration) in declarations {
            if blocked.contains(&unit) || conflicts.contains(&unit) {
                continue;
            }
            if let Some(conversion) = declaration.conversion {
                if declarations.contains_key(&conversion.reference)
                    && !blocked.contains(&conversion.reference)
                    && !conflicts.contains(&conversion.reference)
                {
                    if let Some(reference_scale) = scales.get(&conversion.reference).copied() {
                        changed |= record_scale(
                            unit,
                            ResolvedScale {
                                value: reference_scale.value * conversion.factor,
                                is_exact: reference_scale.is_exact && conversion.is_exact,
                            },
                            &mut scales,
                            &mut conflicts,
                        );
                    }
                    if let Some(unit_scale) = scales.get(&unit).copied() {
                        changed |= record_scale(
                            conversion.reference,
                            ResolvedScale {
                                value: unit_scale.value / conversion.factor,
                                is_exact: unit_scale.is_exact && conversion.is_exact,
                            },
                            &mut scales,
                            &mut conflicts,
                        );
                    }
                }
            }
            if let Some(expression) = declaration.initializer {
                let evaluated = unit_expression_value(
                    workspace,
                    expression,
                    &scales,
                    &dimensions,
                    &mut HashSet::new(),
                );
                if let Some(value) = evaluated {
                    if value.dimension != declaration.dimension {
                        conflicts.insert(unit);
                        scales.remove(&unit);
                        changed = true;
                    } else {
                        changed |= record_scale(
                            unit,
                            ResolvedScale {
                                value: value.scale_to_si,
                                is_exact: value.is_exact,
                            },
                            &mut scales,
                            &mut conflicts,
                        );
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    (scales, conflicts)
}

fn unit_expression_value(
    workspace: &mut Workspace,
    expression: ElementId,
    scales: &HashMap<ElementId, ResolvedScale>,
    dimensions: &HashMap<ElementId, [i8; 7]>,
    active_features: &mut HashSet<ElementId>,
) -> Option<UnitExpressionValue> {
    match workspace.model().kind(expression) {
        ElementKind::LiteralInteger | ElementKind::LiteralRational => Some(UnitExpressionValue {
            dimension: [0; 7],
            scale_to_si: numeric_literal(workspace.model(), expression)?,
            is_exact: true,
        }),
        ElementKind::FeatureReferenceExpression => {
            let target = reference_target(workspace.model(), expression)?;
            if let Some(&dimension) = dimensions.get(&target) {
                let scale = *scales.get(&target)?;
                return Some(UnitExpressionValue {
                    dimension,
                    scale_to_si: scale.value,
                    is_exact: scale.is_exact,
                });
            }
            if !active_features.insert(target) {
                return None;
            }
            let expression = feature_value_expression(workspace.model(), target);
            let value = expression.and_then(|value| {
                unit_expression_value(workspace, value, scales, dimensions, active_features)
            });
            active_features.remove(&target);
            value
        }
        ElementKind::OperatorExpression => {
            let operator = workspace
                .model()
                .maybe(expression, "operator")
                .and_then(Value::as_str)?
                .to_owned();
            let inputs = {
                let model = workspace.model();
                model
                    .owned(expression)
                    .iter()
                    .copied()
                    .filter(|&child| {
                        model.kind(child).is_a(ElementKind::Feature)
                            && model.maybe(child, "direction") == Some(&Value::EnumLit("in"))
                    })
                    .filter_map(|parameter| feature_value_expression(model, parameter))
                    .collect::<Vec<_>>()
            };
            let values = inputs
                .into_iter()
                .map(|input| {
                    unit_expression_value(workspace, input, scales, dimensions, active_features)
                })
                .collect::<Option<Vec<_>>>()?;
            apply_unit_operator(&operator, &values)
        }
        _ => None,
    }
}

fn apply_unit_operator(
    operator: &str,
    values: &[UnitExpressionValue],
) -> Option<UnitExpressionValue> {
    match (operator, values) {
        ("+", [left, right]) | ("-", [left, right])
            if left.dimension == [0; 7] && right.dimension == [0; 7] =>
        {
            Some(UnitExpressionValue {
                dimension: [0; 7],
                scale_to_si: if operator == "+" {
                    left.scale_to_si + right.scale_to_si
                } else {
                    left.scale_to_si - right.scale_to_si
                },
                is_exact: left.is_exact && right.is_exact,
            })
        }
        ("+", [value]) => Some(*value),
        ("-", [value]) if value.dimension == [0; 7] => Some(UnitExpressionValue {
            dimension: value.dimension,
            scale_to_si: -value.scale_to_si,
            is_exact: value.is_exact,
        }),
        ("*", [left, right]) => Some(UnitExpressionValue {
            dimension: combine_dimensions(left.dimension, right.dimension, i8::checked_add)?,
            scale_to_si: left.scale_to_si * right.scale_to_si,
            is_exact: left.is_exact && right.is_exact,
        }),
        ("/", [left, right]) if right.scale_to_si != 0.0 => Some(UnitExpressionValue {
            dimension: combine_dimensions(left.dimension, right.dimension, i8::checked_sub)?,
            scale_to_si: left.scale_to_si / right.scale_to_si,
            is_exact: left.is_exact && right.is_exact,
        }),
        ("^", [base, exponent]) if exponent.dimension == [0; 7] => {
            if exponent.scale_to_si.fract() != 0.0 {
                return None;
            }
            let is_exact = base.is_exact && exponent.is_exact;
            let exponent = i32::try_from(exponent.scale_to_si as i64).ok()?;
            Some(UnitExpressionValue {
                dimension: power_dimension(base.dimension, exponent)?,
                scale_to_si: base.scale_to_si.powi(exponent),
                is_exact,
            })
        }
        _ => None,
    }
}

fn combine_dimensions(
    left: [i8; 7],
    right: [i8; 7],
    operation: fn(i8, i8) -> Option<i8>,
) -> Option<[i8; 7]> {
    let mut dimensions = [0; 7];
    for (index, (left, right)) in left.into_iter().zip(right).enumerate() {
        dimensions[index] = operation(left, right)?;
    }
    Some(dimensions)
}

fn power_dimension(dimension: [i8; 7], exponent: i32) -> Option<[i8; 7]> {
    let mut powered = [0; 7];
    for (index, base) in dimension.into_iter().enumerate() {
        powered[index] = i8::try_from(i32::from(base).checked_mul(exponent)?).ok()?;
    }
    Some(powered)
}

fn quantity_dimension(
    workspace: &mut Workspace,
    unit_type: ElementId,
    qpf_type: ElementId,
    qpf_quantity: ElementId,
    qpf_exponent: ElementId,
    dimension_one_unit: Option<ElementId>,
    base_quantity_axes: &HashMap<ElementId, usize>,
) -> Option<[i8; 7]> {
    let mut dimensions = [0_i32; 7];
    let mut found = false;
    let mut type_lineage = vec![unit_type];
    type_lineage.extend(workspace.supertypes(unit_type));
    for candidate_type in type_lineage {
        let members = workspace.model().owned(candidate_type).to_vec();
        for factor in members {
            if !workspace.model().kind(factor).is_a(ElementKind::Feature) {
                continue;
            }
            let Some(factor_type) = workspace.model().type_of(factor) else {
                continue;
            };
            if !inherits(workspace, factor_type, qpf_type) {
                continue;
            }
            let quantity_feature = redefined_feature(workspace.model(), factor, qpf_quantity)?;
            let exponent_feature = redefined_feature(workspace.model(), factor, qpf_exponent)?;
            let quantity_expression =
                feature_value_expression(workspace.model(), quantity_feature)?;
            let quantity = reference_target(workspace.model(), quantity_expression)?;
            let axis = *base_quantity_axes.get(&quantity)?;
            let exponent_expression =
                feature_value_expression(workspace.model(), exponent_feature)?;
            let exponent = numeric_expression_value(workspace, exponent_expression)?;
            if exponent.fract() != 0.0 {
                return None;
            }
            let exponent = i32::try_from(exponent as i64).ok()?;
            dimensions[axis] = dimensions[axis].checked_add(exponent)?;
            found = true;
        }
    }
    if !found {
        if dimension_one_unit.is_some_and(|root| inherits(workspace, unit_type, root)) {
            return Some([0; 7]);
        }
        return None;
    }
    let mut projected = [0_i8; 7];
    for (index, exponent) in dimensions.into_iter().enumerate() {
        projected[index] = i8::try_from(exponent).ok()?;
    }
    Some(projected)
}

fn inherits(workspace: &mut Workspace, element: ElementId, ancestor: ElementId) -> bool {
    element == ancestor || workspace.supertypes(element).contains(&ancestor)
}

fn unit_conversion_for(
    workspace: &mut Workspace,
    unit: ElementId,
    conversion_root: Option<ElementId>,
    prefix_conversion: Option<ElementId>,
    reference_feature: Option<ElementId>,
    factor_feature: Option<ElementId>,
    exactness_feature: Option<ElementId>,
    prefix_feature: Option<ElementId>,
    prefix_factor_feature: Option<ElementId>,
) -> (Option<UnitConversion>, bool) {
    let Some(conversion_root) = conversion_root else {
        return (None, false);
    };
    let mut conversion_instances = Vec::new();
    for candidate in workspace.model().descendants(unit) {
        if !workspace.model().kind(candidate).is_a(ElementKind::Feature) {
            continue;
        }
        if workspace
            .model()
            .type_of(candidate)
            .is_some_and(|ty| inherits(workspace, ty, conversion_root))
        {
            conversion_instances.push(candidate);
        }
    }
    if conversion_instances.is_empty() {
        return (None, false);
    }
    if conversion_instances.len() != 1 {
        return (None, true);
    }
    let conversion = conversion_instances[0];
    let Some(reference_feature) = reference_feature else {
        return (None, true);
    };
    let Some(reference_value) = redefined_feature(workspace.model(), conversion, reference_feature)
    else {
        return (None, true);
    };
    let Some(reference_expression) = feature_value_expression(workspace.model(), reference_value)
    else {
        return (None, true);
    };
    let Some(reference) = reference_target(workspace.model(), reference_expression) else {
        return (None, true);
    };

    let is_prefix_conversion = prefix_conversion.is_some_and(|root| {
        workspace
            .model()
            .type_of(conversion)
            .is_some_and(|ty| inherits(workspace, ty, root))
    });
    let factor = if is_prefix_conversion {
        let (Some(prefix_feature), Some(prefix_factor_feature)) =
            (prefix_feature, prefix_factor_feature)
        else {
            return (None, true);
        };
        let Some(prefix_value) = redefined_feature(workspace.model(), conversion, prefix_feature)
        else {
            return (None, true);
        };
        let Some(prefix_expression) = feature_value_expression(workspace.model(), prefix_value)
        else {
            return (None, true);
        };
        let Some(prefix) = reference_target(workspace.model(), prefix_expression) else {
            return (None, true);
        };
        let Some(prefix_factor) =
            redefined_feature(workspace.model(), prefix, prefix_factor_feature)
        else {
            return (None, true);
        };
        let Some(expression) = feature_value_expression(workspace.model(), prefix_factor) else {
            return (None, true);
        };
        numeric_expression_value(workspace, expression)
    } else {
        let Some(factor_feature) = factor_feature else {
            return (None, true);
        };
        let Some(factor_value) = redefined_feature(workspace.model(), conversion, factor_feature)
        else {
            return (None, true);
        };
        let Some(expression) = feature_value_expression(workspace.model(), factor_value) else {
            return (None, true);
        };
        numeric_expression_value(workspace, expression)
    };
    let Some(factor) = factor.filter(|factor| factor.is_finite() && *factor > 0.0) else {
        return (None, true);
    };
    let Some(exactness_feature) = exactness_feature else {
        return (None, true);
    };
    let is_exact = match redefined_feature(workspace.model(), conversion, exactness_feature) {
        Some(exactness_value) => {
            let Some(expression) = feature_value_expression(workspace.model(), exactness_value)
            else {
                return (None, true);
            };
            let Some(is_exact) = boolean_literal(workspace.model(), expression) else {
                return (None, true);
            };
            is_exact
        }
        None => true,
    };
    (
        Some(UnitConversion {
            reference,
            factor,
            is_exact,
        }),
        false,
    )
}

fn redefined_feature(
    model: &sysml_model::Model,
    root: ElementId,
    target: ElementId,
) -> Option<ElementId> {
    let mut matches = model
        .descendants(root)
        .into_iter()
        .filter(|&candidate| model.kind(candidate).is_a(ElementKind::Feature))
        .filter(|&candidate| redefines(model, candidate, target));
    let found = matches.next()?;
    matches.next().is_none().then_some(found)
}

fn redefines(model: &sysml_model::Model, feature: ElementId, target: ElementId) -> bool {
    let mut pending = vec![feature];
    let mut visited = HashSet::new();
    while let Some(candidate) = pending.pop() {
        if candidate == target {
            return true;
        }
        if !visited.insert(candidate) {
            continue;
        }
        for relationship in model
            .owned(candidate)
            .iter()
            .copied()
            .filter(|&child| model.kind(child) == ElementKind::Redefinition)
        {
            if let Some(redefined) = model.redefined_feature(relationship) {
                pending.push(redefined);
            }
        }
    }
    false
}

fn feature_value_expression(model: &sysml_model::Model, feature: ElementId) -> Option<ElementId> {
    model.owned(feature).iter().copied().find_map(|child| {
        (model.kind(child) == ElementKind::FeatureValue)
            .then(|| model.maybe(child, "value").and_then(Value::as_id))
            .flatten()
    })
}

fn reference_target(model: &sysml_model::Model, expression: ElementId) -> Option<ElementId> {
    (model.kind(expression) == ElementKind::FeatureReferenceExpression)
        .then(|| model.maybe(expression, "referent").and_then(Value::as_id))
        .flatten()
}

fn numeric_literal(model: &sysml_model::Model, expression: ElementId) -> Option<f64> {
    match model.kind(expression) {
        ElementKind::LiteralInteger => match model.maybe(expression, "value")? {
            Value::Int(value) => Some(*value as f64),
            _ => None,
        },
        ElementKind::LiteralRational => match model.maybe(expression, "value")? {
            Value::Real(value) => value.is_finite().then_some(*value),
            _ => None,
        },
        _ => None,
    }
}

fn boolean_literal(model: &sysml_model::Model, expression: ElementId) -> Option<bool> {
    match model.maybe(expression, "value")? {
        Value::Bool(value) => Some(*value),
        _ => None,
    }
}

fn numeric_expression_value(workspace: &mut Workspace, expression: ElementId) -> Option<f64> {
    let value = unit_expression_value(
        workspace,
        expression,
        &HashMap::new(),
        &HashMap::new(),
        &mut HashSet::new(),
    )?;
    (value.dimension == [0; 7]).then_some(value.scale_to_si)
}
