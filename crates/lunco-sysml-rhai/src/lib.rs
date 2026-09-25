//! Read-only Rhai reporting for SysML v2 analysis.
//!
//! Rhai owns test/report policy in LunCoSim. This adapter only exposes a
//! native semantic snapshot; it never parses source or mutates a document,
//! keeping the language boundary small and deterministic.

use bevy::math::{DQuat, DVec2, DVec3};
use lunco_core::DTransform;
use lunco_sysml_ast::{
    SysmlAnalysis, SysmlAttribute, SysmlDiagnostic, SysmlElement, SysmlElementHandle,
    SysmlEnumValue, SysmlExpression, SysmlExpressionKind, SysmlExpressionOperator, SysmlFeature,
    SysmlFeatureDirection, SysmlFeatureHandle, SysmlFeaturePath, SysmlFunctionReference,
    SysmlModelicaType, SysmlMultiplicity, SysmlPrimitiveType, SysmlQuantityValue, SysmlRecord,
    SysmlSourceRef, SysmlStandardConstant, SysmlSubject, SysmlType, SysmlTypeCategory,
    SysmlTypeRef, SysmlUnsupportedExpression,
};
use lunco_sysml_ir::{
    BindingContract, BindingProvider, CompiledConstraint, ConstraintIr, DiagnosticSeverity,
    EvaluationContext, EvaluationOptions, EvaluationReport, FeatureObservation, IrDiagnostic,
    IrDiagnosticCode, IrExpression, IrExpressionKind, IrFeatureDirection, IrOperator, IrParameter,
    IrStandardFunction, IrType, IrValue, IrValueType, ObservationState, VerificationVerdict,
    compile_constraint_by_name, evaluate_constraint,
};
use lunco_sysml_modelica::{lower_constraint, supports_standard_function_lowering};
use rhai::{Array, Dynamic, Engine, Map};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A source-backed SysML record exposed as a native Rhai object.
///
/// Fields are resolved from typed AST attributes, so a script never needs to
/// split CSV strings or parse a source comment to obtain a component value.
#[derive(Clone, Debug)]
pub struct SysmlRecordValue {
    inner: SysmlRecord,
}

/// Snapshot-local, typed requirement coverage edges exposed to Rhai.
///
/// Requirement and verification names are resolved at the report boundary;
/// coverage itself is keyed only by semantic handles, never concatenated
/// strings.
#[derive(Clone, Debug, Default)]
pub struct SysmlRequirementCoverageValue {
    links: HashSet<(SysmlElementHandle, SysmlElementHandle)>,
}

impl SysmlRequirementCoverageValue {
    fn covers(&self, verification: SysmlElementHandle, requirement: SysmlElementHandle) -> bool {
        self.links.contains(&(verification, requirement))
    }
}

fn coverage_value_error(message: impl Into<String>) -> Box<rhai::EvalAltResult> {
    rhai::EvalAltResult::ErrorRuntime(message.into().into(), rhai::Position::NONE).into()
}

fn dynamic_element_handle(value: &Dynamic) -> Option<SysmlElementHandle> {
    if let Some(handle) = value.clone().try_cast::<SysmlElementHandle>() {
        return Some(handle);
    }
    let fields = value.clone().try_cast::<Map>()?;
    Some(SysmlElementHandle {
        source_revision: dynamic_u64(fields.get("source_revision")?)?,
        source_fingerprint: dynamic_u64(fields.get("source_fingerprint")?)?,
        element_id: u32::try_from(dynamic_u64(fields.get("element_id")?)?).ok()?,
    })
}

fn requirement_coverage_from_records(
    verifications: rhai::Array,
) -> Result<SysmlRequirementCoverageValue, Box<rhai::EvalAltResult>> {
    let mut links = HashSet::new();
    for verification in verifications {
        let record = verification
            .try_cast::<Map>()
            .ok_or_else(|| coverage_value_error("verification record is not a map"))?;
        let element = record
            .get("element")
            .cloned()
            .and_then(|value| value.try_cast::<Map>())
            .ok_or_else(|| coverage_value_error("verification record has no element"))?;
        let verification_handle = element
            .get("handle")
            .and_then(dynamic_element_handle)
            .ok_or_else(|| coverage_value_error("verification element has no typed handle"))?;
        let targets = record
            .get("verified_requirements")
            .cloned()
            .and_then(|value| value.try_cast::<rhai::Array>())
            .ok_or_else(|| coverage_value_error("verification record has no resolved targets"))?;
        for target in targets {
            let requirement_handle = dynamic_element_handle(&target)
                .ok_or_else(|| coverage_value_error("verified requirement has no typed handle"))?;
            links.insert((verification_handle, requirement_handle));
        }
    }
    Ok(SysmlRequirementCoverageValue { links })
}

/// One immutable, revision-pinned SysML source session exposed to Rhai.
///
/// The session owns the already-resolved semantic snapshot instead of making
/// every attribute lookup call back through the generic query bridge. This is
/// deliberately read-only: source edits produce a new analysis and therefore
/// a new session identity. Keeping the path and the analysis together also
/// prevents a value from one Twin/source revision being silently reused with
/// another report.
#[derive(Clone, Debug)]
pub struct SysmlModelValue {
    path: String,
    analysis: Arc<SysmlAnalysis>,
}

impl SysmlModelValue {
    /// Create a source session from the validated analysis owned by the
    /// document/Twin resolver.
    pub fn new(path: impl Into<String>, analysis: Arc<SysmlAnalysis>) -> Self {
        Self {
            path: path.into(),
            analysis,
        }
    }

    /// Return the source URI/path used to resolve this session.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Return the immutable semantic snapshot behind this session.
    pub fn analysis(&self) -> &SysmlAnalysis {
        &self.analysis
    }
}

/// A source-backed requirement record selected from a [`SysmlModelValue`].
#[derive(Clone, Debug)]
pub struct SysmlRequirementValue {
    inner: lunco_sysml_ast::SysmlRequirementRecord,
}

/// A source-backed verification case selected from a [`SysmlModelValue`].
#[derive(Clone, Debug)]
pub struct SysmlVerificationValue {
    inner: lunco_sysml_ast::SysmlVerificationRecord,
}

fn record_value(record: &mut SysmlRecordValue, name: &str) -> Dynamic {
    record
        .inner
        .fields
        .iter()
        .find(|field| field.name == name)
        .and_then(typed_attribute_value_dynamic)
        .unwrap_or(Dynamic::UNIT)
}

fn record_has_field(record: &mut SysmlRecordValue, name: &str) -> bool {
    record.inner.fields.iter().any(|field| field.name == name)
}

fn model_attribute(model: &mut SysmlModelValue, name: &str) -> Dynamic {
    model
        .analysis
        .attributes()
        .iter()
        .find(|attribute| attribute.qualified_name == name)
        .cloned()
        .map(Dynamic::from)
        .unwrap_or(Dynamic::UNIT)
}

fn model_value(model: &mut SysmlModelValue, name: &str) -> Dynamic {
    model
        .analysis
        .attributes()
        .iter()
        .find(|attribute| attribute.qualified_name == name)
        .and_then(typed_attribute_value_dynamic)
        .unwrap_or(Dynamic::UNIT)
}

fn model_requirement(model: &mut SysmlModelValue, name: &str) -> Dynamic {
    model
        .analysis
        .requirements()
        .iter()
        .find(|record| record.element.qualified_name == name)
        .cloned()
        .map(|inner| Dynamic::from(SysmlRequirementValue { inner }))
        .unwrap_or(Dynamic::UNIT)
}

fn model_verification(model: &mut SysmlModelValue, name: &str) -> Dynamic {
    model
        .analysis
        .verifications()
        .iter()
        .find(|record| record.element.qualified_name == name)
        .cloned()
        .map(|inner| Dynamic::from(SysmlVerificationValue { inner }))
        .unwrap_or(Dynamic::UNIT)
}

fn model_requirements(model: &mut SysmlModelValue) -> Dynamic {
    Dynamic::from_array(
        model
            .analysis
            .requirements()
            .iter()
            .cloned()
            .map(|inner| Dynamic::from(SysmlRequirementValue { inner }))
            .collect(),
    )
}

fn model_verifications(model: &mut SysmlModelValue) -> Dynamic {
    Dynamic::from_array(
        model
            .analysis
            .verifications()
            .iter()
            .cloned()
            .map(|inner| Dynamic::from(SysmlVerificationValue { inner }))
            .collect(),
    )
}

pub fn constraint_ir_value(model: &mut SysmlModelValue, name: &str) -> Dynamic {
    compiled_constraint_dynamic(&compile_constraint_by_name(&model.analysis, name))
}

fn standard_functions_dynamic() -> Dynamic {
    Dynamic::from_array(
        IrStandardFunction::SUPPORTED
            .iter()
            .copied()
            .map(|function| {
                let mut value = Map::new();
                value.insert("function".into(), Dynamic::from(function));
                value.insert(
                    "name".into(),
                    Dynamic::from(function.standard_name().to_owned()),
                );
                if let Some(qualified_name) = function.qualified_name() {
                    value.insert(
                        "qualified_name".into(),
                        Dynamic::from(qualified_name.to_owned()),
                    );
                }
                value.insert("arity".into(), Dynamic::from_int(function.arity() as i64));
                value.insert("evaluator".into(), Dynamic::from_bool(true));
                value.insert(
                    "modelica_lowering".into(),
                    Dynamic::from_bool(supports_standard_function_lowering(function)),
                );
                Dynamic::from_map(value)
            })
            .collect(),
    )
}

fn standard_constants_dynamic() -> Dynamic {
    Dynamic::from_array(
        SysmlStandardConstant::SUPPORTED
            .iter()
            .copied()
            .map(|constant| {
                let mut value = Map::new();
                value.insert("constant".into(), Dynamic::from(constant));
                value.insert(
                    "name".into(),
                    Dynamic::from(constant.standard_name().to_owned()),
                );
                Dynamic::from_map(value)
            })
            .collect(),
    )
}

fn standard_operators_dynamic() -> Dynamic {
    Dynamic::from_array(
        IrOperator::SUPPORTED
            .iter()
            .copied()
            .map(Dynamic::from)
            .collect(),
    )
}

pub fn modelica_constraint_value(model: &mut SysmlModelValue, name: &str) -> Dynamic {
    let compiled = compile_constraint_by_name(&model.analysis, name);
    let mut value = Map::new();
    value.insert("ir".into(), compiled_constraint_dynamic(&compiled));
    match lower_constraint(&compiled) {
        Ok(lowered) => {
            value.insert("ok".into(), Dynamic::from_bool(true));
            value.insert("model_name".into(), Dynamic::from(lowered.model_name));
            value.insert("source".into(), Dynamic::from(lowered.source));
            value.insert(
                "feature_bindings".into(),
                Dynamic::from_array(
                    lowered
                        .feature_bindings
                        .into_iter()
                        .map(|binding| {
                            let mut feature = Map::new();
                            feature.insert("path".into(), feature_path_dynamic(&binding.path));
                            feature.insert("variable".into(), Dynamic::from(binding.variable));
                            feature.insert(
                                "qualified_name".into(),
                                Dynamic::from(binding.qualified_name),
                            );
                            feature.insert("type".into(), ir_type_dynamic(&binding.ty));
                            Dynamic::from_map(feature)
                        })
                        .collect(),
                ),
            );
        }
        Err(error) => {
            value.insert("ok".into(), Dynamic::from_bool(false));
            value.insert("error".into(), Dynamic::from(error.to_string()));
        }
    }
    Dynamic::from_map(value)
}

/// Evaluate one compiled constraint through the neutral IR using observation
/// records supplied by an authored Rhai policy. Every record identifies its
/// source feature path with snapshot-scoped handles; Rust owns type checking,
/// contract validation, and the four-state verification result.
pub fn evaluate_constraint_value(
    model: &mut SysmlModelValue,
    name: &str,
    observations: Array,
    absolute_tolerance: f64,
    relative_tolerance: f64,
) -> Dynamic {
    let compiled = compile_constraint_by_name(&model.analysis, name);
    let mut context = EvaluationContext::default();
    let mut input_diagnostics = Vec::new();

    let allowed_paths = compiled
        .constraint
        .as_ref()
        .map(|constraint| constraint.dependencies.as_slice())
        .unwrap_or_default();
    let mut observed_paths = HashSet::new();
    for dynamic_observation in observations {
        let Some(record) = dynamic_observation.try_cast::<Map>() else {
            input_diagnostics.push(IrDiagnostic {
                severity: DiagnosticSeverity::Error,
                code: IrDiagnosticCode::ObservationIsNotRecord,
                source: None,
                message: "provider observation must be a Rhai map".to_owned(),
            });
            continue;
        };
        let path = record.get("path").and_then(dynamic_feature_path);
        let Some(path) = path else {
            input_diagnostics.push(IrDiagnostic {
                severity: DiagnosticSeverity::Error,
                code: IrDiagnosticCode::InvalidObservationPath,
                source: None,
                message: "provider observation needs a non-empty typed SysML feature path"
                    .to_owned(),
            });
            continue;
        };
        let feature_name = feature_path_label(&model.analysis, &path)
            .unwrap_or_else(|| format!("feature-path {:?}", path.features()));
        if !path.belongs_to(
            model.analysis.source_revision(),
            model.analysis.source_fingerprint(),
        ) {
            input_diagnostics.push(IrDiagnostic {
                severity: DiagnosticSeverity::Error,
                code: IrDiagnosticCode::ObservationSnapshotMismatch,
                source: None,
                message: format!(
                    "observation path for `{feature_name}` belongs to another source snapshot"
                ),
            });
            continue;
        }
        if !allowed_paths.contains(&path) {
            input_diagnostics.push(IrDiagnostic {
                severity: DiagnosticSeverity::Error,
                code: IrDiagnosticCode::ObservationIsNotDependency,
                source: None,
                message: format!(
                    "observation path for `{feature_name}` is not a dependency of `{name}`"
                ),
            });
            continue;
        }
        if !observed_paths.insert(path.clone()) {
            input_diagnostics.push(IrDiagnostic {
                severity: DiagnosticSeverity::Error,
                code: IrDiagnosticCode::DuplicateObservationPath,
                source: None,
                message: format!(
                    "provider supplied more than one observation for `{feature_name}`"
                ),
            });
            continue;
        }
        let provider = record
            .get("provider")
            .and_then(|value| value.clone().into_string().ok())
            .and_then(|value| parse_binding_provider(&value));
        let state = record
            .get("state")
            .and_then(|value| value.clone().into_string().ok())
            .and_then(|value| parse_observation_state(&value));
        let (Some(provider), Some(state)) = (provider, state) else {
            input_diagnostics.push(IrDiagnostic {
                severity: DiagnosticSeverity::Error,
                code: IrDiagnosticCode::ObservationProviderOrStateInvalid,
                source: None,
                message: format!(
                    "observation for `{feature_name}` needs recognized provider and state"
                ),
            });
            continue;
        };
        let value = record
            .get("value")
            .and_then(dynamic_ir_value)
            .or_else(|| (state == ObservationState::Value).then_some(IrValue::Null));
        let detail = record
            .get("detail")
            .and_then(|value| value.clone().into_string().ok());
        let unit = record
            .get("unit")
            .and_then(|value| value.clone().into_string().ok());
        let frame = record
            .get("frame")
            .and_then(|value| value.clone().into_string().ok());
        let time_basis = record
            .get("time_basis")
            .and_then(|value| value.clone().into_string().ok());
        let source_revision = record.get("source_revision").and_then(dynamic_u64);
        let contract = if let Some(dynamic_contract) = record.get("contract") {
            let Some(contract_record) = dynamic_contract.clone().try_cast::<Map>() else {
                input_diagnostics.push(IrDiagnostic {
                    severity: DiagnosticSeverity::Error,
                    code: IrDiagnosticCode::InvalidBindingContract,
                    source: None,
                    message: format!("binding contract for `{feature_name}` must be a Rhai map"),
                });
                continue;
            };
            let contract_provider = contract_record
                .get("provider")
                .and_then(|value| value.clone().into_string().ok())
                .and_then(|value| parse_binding_provider(&value));
            let Some(contract_provider) = contract_provider else {
                input_diagnostics.push(IrDiagnostic {
                    severity: DiagnosticSeverity::Error,
                    code: IrDiagnosticCode::InvalidBindingContract,
                    source: None,
                    message: format!(
                        "binding contract for `{feature_name}` needs a recognized provider"
                    ),
                });
                continue;
            };
            let required = contract_record
                .get("required")
                .and_then(|value| value.as_bool().ok())
                .unwrap_or(true);
            Some(BindingContract {
                path: path.clone(),
                provider: contract_provider,
                required,
                unit: contract_record
                    .get("unit")
                    .and_then(|value| value.clone().into_string().ok()),
                frame: contract_record
                    .get("frame")
                    .and_then(|value| value.clone().into_string().ok()),
                time_basis: contract_record
                    .get("time_basis")
                    .and_then(|value| value.clone().into_string().ok()),
                source_revision: contract_record.get("source_revision").and_then(dynamic_u64),
            })
        } else {
            None
        };
        context.observations.push(FeatureObservation {
            path,
            provider,
            state,
            value,
            detail,
            unit,
            frame,
            time_basis,
            source_revision,
            contract,
        });
    }

    let mut report = evaluate_constraint(
        &compiled,
        &context,
        EvaluationOptions {
            absolute_tolerance,
            relative_tolerance,
        },
    );
    report.diagnostics.extend(input_diagnostics);
    if report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
    {
        report.verdict = VerificationVerdict::Error;
    }
    evaluation_report_dynamic(&report)
}

fn dynamic_feature_path(value: &Dynamic) -> Option<SysmlFeaturePath> {
    let segments = value.clone().try_cast::<Array>()?;
    let features = segments
        .iter()
        .map(|segment| {
            segment
                .clone()
                .try_cast::<SysmlFeatureHandle>()
                .or_else(|| {
                    dynamic_element_handle(segment).map(|element| SysmlFeatureHandle { element })
                })
        })
        .collect::<Option<Vec<_>>>()?;
    SysmlFeaturePath::new(features)
}

fn feature_path_label(analysis: &SysmlAnalysis, path: &SysmlFeaturePath) -> Option<String> {
    let target = path.target();
    analysis
        .attributes()
        .iter()
        .find(|attribute| attribute.handle == target)
        .map(|attribute| attribute.qualified_name.clone())
        .or_else(|| {
            analysis
                .elements()
                .iter()
                .find(|element| element.feature_handle == Some(target))
                .map(|element| element.qualified_name.clone())
        })
}

fn dynamic_ir_value(value: &Dynamic) -> Option<IrValue> {
    if let Ok(value) = value.as_bool() {
        return Some(IrValue::Boolean(value));
    }
    if let Ok(value) = value.as_int() {
        return Some(IrValue::Integer(value));
    }
    if let Ok(value) = value.as_float() {
        return value.is_finite().then_some(IrValue::Real(value));
    }
    if value.is_string() {
        return value.clone().into_string().ok().map(IrValue::String);
    }
    if let Some(target) = value.clone().try_cast::<SysmlElementHandle>() {
        return Some(IrValue::Reference(target));
    }
    if let Some(value) = value.clone().try_cast::<SysmlEnumValue>() {
        return Some(IrValue::Enumeration {
            type_name: value.type_ref.map(|reference| reference.qualified_name),
            literal: value.literal,
        });
    }
    if let Some(values) = value.clone().try_cast::<rhai::Array>() {
        return values
            .iter()
            .map(dynamic_ir_value)
            .collect::<Option<Vec<_>>>()
            .map(IrValue::Collection);
    }
    let map = value.clone().try_cast::<Map>()?;
    let nested = map.get("value").and_then(dynamic_ir_value)?;
    if let Some(unit) = map
        .get("unit")
        .and_then(|value| value.clone().into_string().ok())
    {
        let scalar = match nested {
            IrValue::Integer(value) => value as f64,
            IrValue::Real(value) => value,
            _ => return None,
        };
        return scalar.is_finite().then_some(IrValue::Quantity {
            value: scalar,
            unit,
        });
    }
    Some(nested)
}

fn dynamic_u64(value: &Dynamic) -> Option<u64> {
    value.clone().try_cast::<u64>().or_else(|| {
        value
            .as_int()
            .ok()
            .and_then(|value| u64::try_from(value).ok())
    })
}

fn parse_binding_provider(value: &str) -> Option<BindingProvider> {
    match value {
        "source_literal" => Some(BindingProvider::SourceLiteral),
        "usd" => Some(BindingProvider::Usd),
        "modelica" => Some(BindingProvider::Modelica),
        "telemetry" => Some(BindingProvider::Telemetry),
        "derived" => Some(BindingProvider::Derived),
        "external" => Some(BindingProvider::External),
        _ => None,
    }
}

fn parse_observation_state(value: &str) -> Option<ObservationState> {
    match value {
        "value" => Some(ObservationState::Value),
        "unavailable" => Some(ObservationState::Unavailable),
        "invalid" => Some(ObservationState::Invalid),
        "stale" => Some(ObservationState::Stale),
        "provider_error" => Some(ObservationState::ProviderError),
        _ => None,
    }
}

fn evaluation_report_dynamic(report: &EvaluationReport) -> Dynamic {
    let mut value = Map::new();
    value.insert(
        "verdict".into(),
        Dynamic::from(match report.verdict {
            VerificationVerdict::Pass => "pass",
            VerificationVerdict::Fail => "fail",
            VerificationVerdict::Inconclusive => "inconclusive",
            VerificationVerdict::Error => "error",
        }),
    );
    value.insert(
        "expression_results".into(),
        Dynamic::from_array(
            report
                .expression_results
                .iter()
                .map(|result| result.map(Dynamic::from_bool).unwrap_or(Dynamic::UNIT))
                .collect(),
        ),
    );
    value.insert(
        "diagnostics".into(),
        Dynamic::from_array(
            report
                .diagnostics
                .iter()
                .map(ir_diagnostic_dynamic)
                .collect(),
        ),
    );
    Dynamic::from_map(value)
}

fn requirement_attribute(requirement: &mut SysmlRequirementValue, name: &str) -> Dynamic {
    requirement
        .inner
        .attributes
        .iter()
        .find(|attribute| attribute.name == name || attribute.qualified_name == name)
        .cloned()
        .map(Dynamic::from)
        .unwrap_or(Dynamic::UNIT)
}

fn requirement_has_attribute(requirement: &mut SysmlRequirementValue, name: &str) -> bool {
    requirement
        .inner
        .attributes
        .iter()
        .any(|attribute| attribute.name == name || attribute.qualified_name == name)
}

/// Project all semantic facts as native Rhai values without copying the full
/// source text into the runtime snapshot. The source remains available from
/// the owning document; policies normally need source names, spans and the
/// content revision, not another copy of every file's bytes. Report shape and
/// selection policy belong to authored Rhai tools.
pub fn semantic_snapshot_dynamic(analysis: &SysmlAnalysis) -> Dynamic {
    let elements_by_handle = analysis
        .elements()
        .iter()
        .map(|element| (element.handle, element))
        .collect::<HashMap<_, _>>();
    let mut report = Map::new();
    let requirement_coverage = SysmlRequirementCoverageValue {
        links: analysis
            .verifications()
            .iter()
            .flat_map(|verification| {
                verification
                    .verified_requirements
                    .iter()
                    .copied()
                    .map(|requirement| (verification.element.handle, requirement))
            })
            .collect(),
    };
    report.insert(
        "source_revision".into(),
        Dynamic::from(analysis.source_revision()),
    );
    report.insert(
        "source_fingerprint".into(),
        Dynamic::from(analysis.source_fingerprint()),
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
                .map(|reference| reference_dynamic(reference, &elements_by_handle))
                .collect(),
        ),
    );
    report.insert(
        "relationships".into(),
        Dynamic::from_array(
            analysis
                .relationships()
                .iter()
                .map(relationship_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "constraints".into(),
        Dynamic::from_array(
            analysis
                .constraints()
                .iter()
                .map(constraint_dynamic)
                .collect(),
        ),
    );
    report.insert(
        "attributes".into(),
        Dynamic::from_array(
            analysis
                .attributes()
                .iter()
                .map(|attribute| {
                    attribute_dynamic_at_revision(attribute, analysis.source_revision())
                })
                .collect(),
        ),
    );
    report.insert(
        "records".into(),
        Dynamic::from_array(
            analysis
                .records()
                .iter()
                .cloned()
                .map(|record| Dynamic::from(SysmlRecordValue { inner: record }))
                .collect(),
        ),
    );
    report.insert(
        "requirements".into(),
        Dynamic::from_array(
            analysis
                .requirements()
                .iter()
                .map(|record| requirement_dynamic(record, analysis.source_revision()))
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
        "requirement_coverage".into(),
        Dynamic::from(requirement_coverage),
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

/// Register the native semantic values used by the SysML adapter.
///
/// Spatial values deliberately reuse the exact f64 Bevy/glam types already
/// registered by `lunco-scripting-rhai-core::rhai_math`: `DVec3` and `DQuat`.
/// This function only registers the SysML-specific semantic wrappers, so it
/// can safely be called by a report adapter without creating a second vector
/// or quaternion type family.
pub fn register_sysml_types(engine: &mut Engine) {
    engine
        .register_fn("sysml_model_is", |value: Dynamic| {
            value.try_cast::<SysmlModelValue>().is_some()
        })
        .register_fn("sysml_quantity_is", |value: Dynamic| {
            value.try_cast::<SysmlQuantityValue>().is_some()
        })
        .register_fn("sysml_enum_is", |value: Dynamic| {
            value.try_cast::<SysmlEnumValue>().is_some()
        })
        .register_type_with_name::<SysmlElementHandle>("SysmlElementHandle")
        .register_get("element_id", |handle: &mut SysmlElementHandle| {
            u64::from(handle.element_id)
        })
        .register_get("source_revision", |handle: &mut SysmlElementHandle| {
            handle.source_revision
        })
        .register_get("source_fingerprint", |handle: &mut SysmlElementHandle| {
            handle.source_fingerprint
        })
        .register_fn(
            "==",
            |left: SysmlElementHandle, right: SysmlElementHandle| left == right,
        )
        .register_fn(
            "!=",
            |left: SysmlElementHandle, right: SysmlElementHandle| left != right,
        )
        .register_type_with_name::<SysmlRequirementCoverageValue>("SysmlRequirementCoverageValue")
        .register_fn(
            "covers",
            |coverage: &mut SysmlRequirementCoverageValue,
             verification: Dynamic,
             requirement: Dynamic|
             -> Result<bool, Box<rhai::EvalAltResult>> {
                let verification = dynamic_element_handle(&verification)
                    .ok_or_else(|| coverage_value_error("verification handle is malformed"))?;
                let requirement = dynamic_element_handle(&requirement)
                    .ok_or_else(|| coverage_value_error("requirement handle is malformed"))?;
                Ok(coverage.covers(verification, requirement))
            },
        )
        .register_fn(
            "sysml_requirement_coverage",
            requirement_coverage_from_records,
        )
        .register_type_with_name::<SysmlFeatureHandle>("SysmlFeatureHandle")
        .register_get("element", |handle: &mut SysmlFeatureHandle| handle.element)
        .register_fn(
            "==",
            |left: SysmlFeatureHandle, right: SysmlFeatureHandle| left == right,
        )
        .register_fn(
            "!=",
            |left: SysmlFeatureHandle, right: SysmlFeatureHandle| left != right,
        )
        .register_type_with_name::<IrStandardFunction>("SysmlStandardFunction")
        .register_get("name", |function: &mut IrStandardFunction| {
            function.standard_name().to_owned()
        })
        .register_get("arity", |function: &mut IrStandardFunction| {
            function.arity() as i64
        })
        .register_fn(
            "==",
            |left: IrStandardFunction, right: IrStandardFunction| left == right,
        )
        .register_fn(
            "!=",
            |left: IrStandardFunction, right: IrStandardFunction| left != right,
        )
        .register_type_with_name::<IrOperator>("SysmlConstraintOperator")
        .register_get("name", |operator: &mut IrOperator| {
            operator.standard_name().to_owned()
        })
        .register_get("arity", |operator: &mut IrOperator| operator.arity() as i64)
        .register_fn("==", |left: IrOperator, right: IrOperator| left == right)
        .register_fn("!=", |left: IrOperator, right: IrOperator| left != right)
        .register_type_with_name::<SysmlFeature>("SysmlFeature")
        .register_get("handle", |value: &mut SysmlFeature| value.handle)
        .register_get("owner", |value: &mut SysmlFeature| value.owner.clone())
        .register_get("direction", |value: &mut SysmlFeature| value.direction)
        .register_get("name", |value: &mut SysmlFeature| value.name.clone())
        .register_get("qualified_name", |value: &mut SysmlFeature| {
            value.qualified_name.clone()
        })
        .register_get("type_name", |value: &mut SysmlFeature| {
            value.type_name.clone().unwrap_or_default()
        })
        .register_get("declared_type", |value: &mut SysmlFeature| {
            value
                .declared_type
                .clone()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("file", |value: &mut SysmlFeature| value.file.clone())
        .register_get("start", |value: &mut SysmlFeature| value.start as i64)
        .register_get("end", |value: &mut SysmlFeature| value.end as i64)
        .register_type_with_name::<SysmlFeatureDirection>("SysmlFeatureDirection")
        .register_type_with_name::<SysmlExpressionKind>("SysmlExpressionKind")
        .register_type_with_name::<SysmlExpressionOperator>("SysmlExpressionOperator")
        .register_fn("is_add", |op: SysmlExpressionOperator| {
            op == SysmlExpressionOperator::Add
        })
        .register_fn("is_equal", |op: SysmlExpressionOperator| {
            op == SysmlExpressionOperator::Equal
        })
        .register_fn("modelica_symbol", |op: SysmlExpressionOperator| {
            op.modelica_symbol()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_type_with_name::<SysmlUnsupportedExpression>("SysmlUnsupportedExpression")
        .register_type_with_name::<SysmlFunctionReference>("SysmlFunctionReference")
        .register_get("element", |value: &mut SysmlFunctionReference| {
            value.element
        })
        .register_get("standard_function", |value: &mut SysmlFunctionReference| {
            value
                .standard_function
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_type_with_name::<SysmlStandardConstant>("SysmlStandardConstant")
        .register_get("name", |constant: &mut SysmlStandardConstant| {
            constant.standard_name().to_owned()
        })
        .register_fn(
            "==",
            |left: SysmlStandardConstant, right: SysmlStandardConstant| left == right,
        )
        .register_fn(
            "!=",
            |left: SysmlStandardConstant, right: SysmlStandardConstant| left != right,
        )
        .register_type_with_name::<SysmlExpression>("SysmlExpression")
        .register_get("source", |value: &mut SysmlExpression| value.source.clone())
        .register_get("kind", |value: &mut SysmlExpression| value.kind())
        .register_get("feature", |value: &mut SysmlExpression| {
            value.feature().map(Dynamic::from).unwrap_or(Dynamic::UNIT)
        })
        .register_get("function", |value: &mut SysmlExpression| {
            value
                .function()
                .cloned()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("standard_constant", |value: &mut SysmlExpression| {
            value
                .standard_constant()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("argument_parameters", |value: &mut SysmlExpression| {
            Dynamic::from_array(
                value
                    .argument_parameters()
                    .iter()
                    .map(|parameter| {
                        parameter
                            .as_ref()
                            .map(|parameter| Dynamic::from(*parameter))
                            .unwrap_or(Dynamic::UNIT)
                    })
                    .collect(),
            )
        })
        .register_get("operator", |value: &mut SysmlExpression| {
            value.operator().map(Dynamic::from).unwrap_or(Dynamic::UNIT)
        })
        .register_get("integer_value", |value: &mut SysmlExpression| {
            value
                .integer_value()
                .map(Dynamic::from_int)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("real_value", |value: &mut SysmlExpression| {
            value
                .real_value()
                .map(|number| Dynamic::from_float(number.as_f64()))
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("boolean_value", |value: &mut SysmlExpression| {
            value
                .boolean_value()
                .map(Dynamic::from_bool)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("string_value", |value: &mut SysmlExpression| {
            value
                .string_value()
                .map(str::to_owned)
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("unsupported", |value: &mut SysmlExpression| {
            value
                .unsupported()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("children", |value: &mut SysmlExpression| {
            Dynamic::from_array(
                value
                    .children()
                    .into_iter()
                    .cloned()
                    .map(Dynamic::from)
                    .collect(),
            )
        })
        .register_type_with_name::<SysmlTypeCategory>("SysmlTypeCategory")
        .register_type_with_name::<SysmlPrimitiveType>("SysmlPrimitiveType")
        .register_type_with_name::<SysmlModelicaType>("SysmlModelicaType")
        .register_type_with_name::<SysmlTypeRef>("SysmlTypeRef")
        .register_get("qualified_name", |value: &mut SysmlTypeRef| {
            value.qualified_name.clone()
        })
        .register_type_with_name::<SysmlType>("SysmlType")
        .register_get("base", |value: &mut SysmlType| value.base.clone())
        .register_get("category", |value: &mut SysmlType| value.category)
        .register_get("value_category", |value: &mut SysmlType| {
            value.value_category
        })
        .register_get("is_collection", |value: &mut SysmlType| {
            value.category == SysmlTypeCategory::Collection
        })
        .register_get("is_quantity", |value: &mut SysmlType| {
            value.value_category == SysmlTypeCategory::Quantity
        })
        .register_get("resolved_type", |value: &mut SysmlType| {
            value
                .resolved_type
                .clone()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("primitive", |value: &mut SysmlType| {
            value.primitive.map(Dynamic::from).unwrap_or(Dynamic::UNIT)
        })
        .register_get("dimensions", |value: &mut SysmlType| {
            Dynamic::from_array(
                value
                    .dimensions
                    .iter()
                    .map(|dimension| Dynamic::from_int(*dimension as i64))
                    .collect(),
            )
        })
        .register_get("multiplicity", |value: &mut SysmlType| value.multiplicity)
        .register_get("quantity_kind", |value: &mut SysmlType| {
            value
                .quantity_kind
                .clone()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("unit", |value: &mut SysmlType| {
            value.unit.clone().unwrap_or_default()
        })
        .register_get("modelica_type", |value: &mut SysmlType| {
            value.modelica_type()
        })
        .register_type_with_name::<SysmlAttribute>("SysmlAttribute")
        .register_get("owner", |value: &mut SysmlAttribute| value.owner.clone())
        .register_get("name", |value: &mut SysmlAttribute| value.name.clone())
        .register_get("qualified_name", |value: &mut SysmlAttribute| {
            value.qualified_name.clone()
        })
        .register_get("type_name", |value: &mut SysmlAttribute| {
            value.type_name.clone().unwrap_or_default()
        })
        .register_get("declared_type", |value: &mut SysmlAttribute| {
            value
                .declared_type
                .clone()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("value", |value: &mut SysmlAttribute| {
            value
                .value
                .as_ref()
                .map(literal_dynamic)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("typed_value", |value: &mut SysmlAttribute| {
            typed_attribute_value_dynamic(value).unwrap_or(Dynamic::UNIT)
        })
        .register_get("file", |value: &mut SysmlAttribute| value.file.clone())
        .register_get("start", |value: &mut SysmlAttribute| value.start as i64)
        .register_get("end", |value: &mut SysmlAttribute| value.end as i64)
        .register_type_with_name::<SysmlMultiplicity>("Multiplicity")
        .register_get("lower", |value: &mut SysmlMultiplicity| value.lower as i64)
        .register_get("upper", |value: &mut SysmlMultiplicity| {
            value
                .upper
                .map(|upper| Dynamic::from_int(upper as i64))
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("ordered", |value: &mut SysmlMultiplicity| value.ordered)
        .register_get("unique", |value: &mut SysmlMultiplicity| value.unique)
        .register_type_with_name::<SysmlQuantityValue>("Quantity")
        .register_get("value", |quantity: &mut SysmlQuantityValue| {
            quantity.value.as_f64()
        })
        .register_get("unit", |quantity: &mut SysmlQuantityValue| {
            quantity.unit.clone()
        })
        .register_get("kind", |quantity: &mut SysmlQuantityValue| {
            quantity
                .quantity_kind
                .clone()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_type_with_name::<SysmlEnumValue>("EnumValue")
        .register_get("type_ref", |value: &mut SysmlEnumValue| {
            value
                .type_ref
                .clone()
                .map(Dynamic::from)
                .unwrap_or(Dynamic::UNIT)
        })
        .register_get("literal", |value: &mut SysmlEnumValue| {
            value.literal.clone()
        })
        .register_type_with_name::<SysmlSourceRef>("SourceRef")
        .register_get("file", |value: &mut SysmlSourceRef| value.file.clone())
        .register_get("start", |value: &mut SysmlSourceRef| value.start as i64)
        .register_get("end", |value: &mut SysmlSourceRef| value.end as i64)
        .register_get("revision", |value: &mut SysmlSourceRef| value.revision)
        .register_type_with_name::<SysmlRecordValue>("SysmlRecord")
        .register_get("type_name", |value: &mut SysmlRecordValue| {
            value.inner.type_name.clone()
        })
        .register_get("field_names", |value: &mut SysmlRecordValue| {
            Dynamic::from_array(
                value
                    .inner
                    .fields
                    .iter()
                    .map(|field| Dynamic::from(field.name.clone()))
                    .collect(),
            )
        })
        .register_get("source", |value: &mut SysmlRecordValue| {
            value.inner.source.clone()
        })
        .register_fn("value", record_value)
        .register_fn("has_field", record_has_field)
        .register_type_with_name::<SysmlModelValue>("SysmlModel")
        .register_get("path", |model: &mut SysmlModelValue| model.path.clone())
        .register_get("source_revision", |model: &mut SysmlModelValue| {
            model.analysis.source_revision()
        })
        .register_get("source_fingerprint", |model: &mut SysmlModelValue| {
            model.analysis.source_fingerprint()
        })
        .register_fn("attribute", model_attribute)
        .register_fn("value", model_value)
        .register_fn("requirement", model_requirement)
        .register_fn("verification", model_verification)
        .register_fn("requirements", model_requirements)
        .register_fn("verifications", model_verifications)
        .register_fn("constraint_ir", constraint_ir_value)
        .register_fn("modelica_constraint", modelica_constraint_value)
        .register_fn("evaluate_constraint", evaluate_constraint_value)
        .register_fn("sysml_standard_functions", standard_functions_dynamic)
        .register_fn("sysml_standard_constants", standard_constants_dynamic)
        .register_fn("sysml_constraint_operators", standard_operators_dynamic)
        .register_type_with_name::<SysmlRequirementValue>("SysmlRequirement")
        .register_get(
            "qualified_name",
            |requirement: &mut SysmlRequirementValue| {
                requirement.inner.element.qualified_name.clone()
            },
        )
        .register_get(
            "documentation",
            |requirement: &mut SysmlRequirementValue| {
                string_array(&requirement.inner.documentation)
            },
        )
        .register_get("subjects", |requirement: &mut SysmlRequirementValue| {
            subject_array(&requirement.inner.subjects)
        })
        .register_get("verifies", |requirement: &mut SysmlRequirementValue| {
            string_array(&requirement.inner.verifies)
        })
        .register_get("satisfies", |requirement: &mut SysmlRequirementValue| {
            string_array(&requirement.inner.satisfies)
        })
        .register_fn("attribute", requirement_attribute)
        .register_fn("has_attribute", requirement_has_attribute)
        .register_type_with_name::<SysmlVerificationValue>("SysmlVerification")
        .register_get(
            "qualified_name",
            |verification: &mut SysmlVerificationValue| {
                verification.inner.element.qualified_name.clone()
            },
        )
        .register_get(
            "documentation",
            |verification: &mut SysmlVerificationValue| {
                string_array(&verification.inner.documentation)
            },
        )
        .register_get("subjects", |verification: &mut SysmlVerificationValue| {
            subject_array(&verification.inner.subjects)
        })
        .register_get("verifies", |verification: &mut SysmlVerificationValue| {
            string_array(&verification.inner.verifies)
        })
        .register_get(
            "realizations",
            |verification: &mut SysmlVerificationValue| {
                string_array(&verification.inner.realizations)
            },
        );
}

/// Lower one resolved SysML literal into the native value used by Rhai and
/// the Editor.  This is intentionally a read-side conversion: unresolved
/// expressions return `None` and must be evaluated by the owning semantic
/// engine instead of being guessed by a string parser.
pub fn typed_attribute_value_dynamic(attribute: &SysmlAttribute) -> Option<Dynamic> {
    let literal = attribute.value.as_ref()?;
    let declared = attribute.declared_type.as_ref();
    typed_literal_dynamic(literal, declared)
}

/// Extract a typed literal from the opaque native attribute value at a Rhai
/// adapter boundary. The world bridge need not depend on the SysML AST crate.
pub fn typed_dynamic_attribute_value(attribute: Dynamic) -> Option<Dynamic> {
    let attribute = attribute.try_cast::<SysmlAttribute>()?;
    typed_attribute_value_dynamic(&attribute)
}

fn sysml_element_type(declared: Option<&SysmlType>) -> Option<SysmlType> {
    let mut element = declared?.clone();
    if element.dimensions.is_empty() {
        return None;
    }
    element.dimensions.remove(0);
    element.multiplicity = element
        .dimensions
        .first()
        .copied()
        .map(SysmlMultiplicity::fixed)
        .unwrap_or_else(SysmlMultiplicity::one);
    element.category = if element.dimensions.is_empty() && !element.multiplicity.is_collection() {
        element.value_category
    } else {
        SysmlTypeCategory::Collection
    };
    Some(element)
}

fn typed_literal_dynamic(
    literal: &lunco_sysml_ast::SysmlLiteral,
    declared: Option<&SysmlType>,
) -> Option<Dynamic> {
    if let Some(elements) = &literal.elements {
        let base = declared
            .map(|value| value.base.rsplit("::").next().unwrap_or(&value.base))
            .unwrap_or_default();
        let element_type = sysml_element_type(declared);
        let values: Vec<Dynamic> = elements
            .iter()
            .map(|element| typed_literal_dynamic(element, element_type.as_ref()))
            .collect::<Option<_>>()?;
        if matches!(
            base,
            "Vec2"
                | "CartesianTwoVectorValue"
                | "CartesianVectorValue"
                | "NumericalVectorValue"
                | "VectorValue"
        ) && values.len() == 2
        {
            let coordinates = values
                .iter()
                .map(numeric_dynamic_f64)
                .collect::<Option<Vec<_>>>()?;
            let vector = DVec2::new(coordinates[0], coordinates[1]);
            return vector.is_finite().then_some(Dynamic::from(vector));
        }
        if matches!(
            base,
            "Vec3"
                | "Position"
                | "Direction"
                | "Dimensions"
                | "CartesianThreeVectorValue"
                | "ThreeVectorValue"
                | "CartesianVectorValue"
                | "NumericalVectorValue"
                | "VectorValue"
        ) && values.len() == 3
        {
            let coordinates = values
                .iter()
                .map(numeric_dynamic_f64)
                .collect::<Option<Vec<_>>>()?;
            return finite_vec3(coordinates[0], coordinates[1], coordinates[2]);
        }
        if matches!(base, "Quat" | "Quaternion") && values.len() == 4 {
            let components = values
                .iter()
                .map(numeric_dynamic_f64)
                .collect::<Option<Vec<_>>>()?;
            return normalized_quat(DQuat::from_xyzw(
                components[0],
                components[1],
                components[2],
                components[3],
            ));
        }
        if base == "Transform" && values.len() == 3 {
            return native_transform(&values);
        }
        return Some(Dynamic::from_array(values));
    }

    if literal.unit.is_some()
        || declared.is_some_and(|value| {
            value.value_category == lunco_sysml_ast::SysmlTypeCategory::Quantity
        })
    {
        return Some(Dynamic::from(SysmlQuantityValue {
            value: literal.number_value?,
            unit: literal
                .unit
                .clone()
                .or_else(|| declared.and_then(|value| value.unit.clone()))
                .unwrap_or_default(),
            quantity_kind: declared.and_then(|value| value.quantity_kind.clone()),
        }));
    }

    if declared.is_some_and(|value| {
        value.value_category == lunco_sysml_ast::SysmlTypeCategory::Enumeration
    }) {
        return Some(Dynamic::from(SysmlEnumValue {
            type_ref: declared?.resolved_type.clone(),
            literal: literal
                .string_value
                .clone()
                .unwrap_or_else(|| literal.literal.clone()),
        }));
    }

    if let Some(value) = literal.integer_value {
        return Some(Dynamic::from_int(value));
    }
    if let Some(value) = literal.number_value {
        return Some(Dynamic::from_float(value.as_f64()));
    }
    if let Some(value) = literal.boolean_value {
        return Some(Dynamic::from_bool(value));
    }
    literal.string_value.clone().map(Dynamic::from)
}

fn numeric_dynamic_f64(value: &Dynamic) -> Option<f64> {
    value
        .as_float()
        .ok()
        .or_else(|| value.as_int().ok().map(|integer| integer as f64))
        .filter(|number| number.is_finite())
}

fn native_transform(values: &[Dynamic]) -> Option<Dynamic> {
    let translation = dynamic_vec3(&values[0])?;
    let rotation = dynamic_quat(&values[1])?;
    let scale = dynamic_vec3(&values[2])?;
    Some(Dynamic::from(DTransform::new(
        translation,
        rotation,
        scale,
    )?))
}

fn dynamic_vec3(value: &Dynamic) -> Option<DVec3> {
    if let Some(value) = value.clone().try_cast::<DVec3>() {
        return value.is_finite().then_some(value);
    }
    let values = value.clone().try_cast::<rhai::Array>()?;
    if values.len() != 3 {
        return None;
    }
    let components = values
        .iter()
        .map(|value| value.as_float().ok())
        .collect::<Option<Vec<_>>>()?;
    let vector = DVec3::new(components[0], components[1], components[2]);
    vector.is_finite().then_some(vector)
}

fn dynamic_quat(value: &Dynamic) -> Option<DQuat> {
    if let Some(value) = value.clone().try_cast::<DQuat>() {
        return normalized_quat_value(value);
    }
    let values = value.clone().try_cast::<rhai::Array>()?;
    if values.len() != 4 {
        return None;
    }
    let components = values
        .iter()
        .map(|value| value.as_float().ok())
        .collect::<Option<Vec<_>>>()?;
    normalized_quat_value(DQuat::from_xyzw(
        components[0],
        components[1],
        components[2],
        components[3],
    ))
}

fn normalized_quat_value(value: DQuat) -> Option<DQuat> {
    if !value.is_finite() || value.length_squared() <= f64::EPSILON {
        return None;
    }
    Some(value.normalize())
}

fn finite_vec3(x: f64, y: f64, z: f64) -> Option<Dynamic> {
    let value = DVec3::new(x, y, z);
    value.is_finite().then(|| Dynamic::from(value))
}

fn normalized_quat(value: DQuat) -> Option<Dynamic> {
    if !value.is_finite() || value.length_squared() <= f64::EPSILON {
        return None;
    }
    let normalized = value.normalize();
    normalized.is_finite().then(|| Dynamic::from(normalized))
}

fn string_array(values: &[String]) -> Dynamic {
    Dynamic::from_array(values.iter().cloned().map(Dynamic::from).collect())
}

fn element_dynamic(element: &SysmlElement) -> Dynamic {
    let mut value = Map::new();
    value.insert("handle".into(), Dynamic::from(element.handle));
    value.insert(
        "owner_handle".into(),
        element
            .owner_handle
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT),
    );
    value.insert(
        "feature_handle".into(),
        element
            .feature_handle
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT),
    );
    value.insert("id".into(), Dynamic::from(element.id));
    value.insert("file".into(), Dynamic::from(element.file.clone()));
    value.insert(
        "qualified_name".into(),
        Dynamic::from(element.qualified_name.clone()),
    );
    value.insert(
        "short_name".into(),
        element
            .short_name
            .clone()
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT),
    );
    value.insert("kind".into(), Dynamic::from(element.kind.clone()));
    value.insert("start".into(), Dynamic::from_int(element.start as i64));
    value.insert("end".into(), Dynamic::from_int(element.end as i64));
    Dynamic::from_map(value)
}

fn reference_dynamic(
    reference: &lunco_sysml_ast::SysmlReference,
    elements: &HashMap<lunco_sysml_ast::SysmlElementHandle, &SysmlElement>,
) -> Dynamic {
    let from_feature = elements.get(&reference.from).copied();
    let from_owner = reference
        .from_owner
        .and_then(|handle| elements.get(&handle).copied());
    let target = elements.get(&reference.target).copied();
    let mut value = Map::new();
    value.insert("file".into(), Dynamic::from(reference.file.clone()));
    value.insert("start".into(), Dynamic::from_int(reference.start as i64));
    value.insert("end".into(), Dynamic::from_int(reference.end as i64));
    value.insert("name".into(), Dynamic::from(reference.name.clone()));
    value.insert("from".into(), Dynamic::from(reference.from));
    value.insert(
        "from_owner".into(),
        reference
            .from_owner
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT),
    );
    value.insert(
        "from_feature_name".into(),
        from_feature
            .and_then(|element| element.qualified_name.rsplit("::").next())
            .map(|name| Dynamic::from(name.to_owned()))
            .unwrap_or(Dynamic::UNIT),
    );
    value.insert(
        "from_owner_name".into(),
        from_owner
            .map(|element| Dynamic::from(element.qualified_name.clone()))
            .unwrap_or(Dynamic::UNIT),
    );
    value.insert("target".into(), Dynamic::from(reference.target));
    value.insert(
        "target_name".into(),
        target
            .map(|element| Dynamic::from(element.qualified_name.clone()))
            .unwrap_or(Dynamic::UNIT),
    );
    value.insert(
        "target_short_name".into(),
        target
            .and_then(|element| element.short_name.clone())
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT),
    );
    value.insert(
        "target_feature".into(),
        reference
            .target_feature
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT),
    );
    Dynamic::from_map(value)
}

fn relationship_dynamic(relationship: &lunco_sysml_ast::SysmlRelationship) -> Dynamic {
    let mut value = Map::new();
    value.insert("element".into(), element_dynamic(&relationship.element));
    value.insert(
        "properties".into(),
        Dynamic::from_array(
            relationship
                .properties
                .iter()
                .map(|property| {
                    let mut value = Map::new();
                    value.insert("name".into(), Dynamic::from(property.name.clone()));
                    value.insert(
                        "targets".into(),
                        Dynamic::from_array(
                            property
                                .targets
                                .iter()
                                .copied()
                                .map(Dynamic::from)
                                .collect(),
                        ),
                    );
                    value.insert(
                        "feature_targets".into(),
                        Dynamic::from_array(
                            property
                                .feature_targets
                                .iter()
                                .copied()
                                .map(Dynamic::from)
                                .collect(),
                        ),
                    );
                    Dynamic::from_map(value)
                })
                .collect(),
        ),
    );
    Dynamic::from_map(value)
}

fn constraint_dynamic(constraint: &lunco_sysml_ast::SysmlConstraint) -> Dynamic {
    let mut value = Map::new();
    value.insert("element".into(), element_dynamic(&constraint.element));
    value.insert(
        "parameters".into(),
        Dynamic::from_array(
            constraint
                .parameters
                .iter()
                .cloned()
                .map(Dynamic::from)
                .collect(),
        ),
    );
    value.insert(
        "expressions".into(),
        Dynamic::from_array(
            constraint
                .expressions
                .iter()
                .cloned()
                .map(Dynamic::from)
                .collect(),
        ),
    );
    Dynamic::from_map(value)
}

fn compiled_constraint_dynamic(compiled: &CompiledConstraint) -> Dynamic {
    let mut value = Map::new();
    value.insert("valid".into(), Dynamic::from_bool(compiled.is_valid()));
    value.insert(
        "status".into(),
        Dynamic::from(if compiled.constraint.is_none() {
            "not_found"
        } else if compiled.is_valid() {
            "valid"
        } else {
            "invalid"
        }),
    );
    value.insert(
        "diagnostics".into(),
        Dynamic::from_array(
            compiled
                .diagnostics
                .iter()
                .map(ir_diagnostic_dynamic)
                .collect(),
        ),
    );
    if let Some(constraint) = &compiled.constraint {
        value.insert("constraint".into(), constraint_ir_dynamic(constraint));
    } else {
        value.insert("constraint".into(), Dynamic::UNIT);
    }
    Dynamic::from_map(value)
}

fn constraint_ir_dynamic(constraint: &ConstraintIr) -> Dynamic {
    let mut value = Map::new();
    value.insert(
        "qualified_name".into(),
        Dynamic::from(constraint.qualified_name.clone()),
    );
    value.insert("source".into(), Dynamic::from(constraint.source.clone()));
    value.insert(
        "parameters".into(),
        Dynamic::from_array(
            constraint
                .parameters
                .iter()
                .map(ir_parameter_dynamic)
                .collect(),
        ),
    );
    value.insert("fingerprint".into(), Dynamic::from(constraint.fingerprint));
    value.insert(
        "dependencies".into(),
        Dynamic::from_array(
            constraint
                .dependencies
                .iter()
                .map(feature_path_dynamic)
                .collect(),
        ),
    );
    value.insert(
        "expressions".into(),
        Dynamic::from_array(
            constraint
                .expressions
                .iter()
                .map(ir_expression_dynamic)
                .collect(),
        ),
    );
    Dynamic::from_map(value)
}

fn ir_parameter_dynamic(parameter: &IrParameter) -> Dynamic {
    let mut value = Map::new();
    value.insert("feature".into(), Dynamic::from(parameter.feature));
    value.insert(
        "owner".into(),
        parameter.owner.map(Dynamic::from).unwrap_or(Dynamic::UNIT),
    );
    value.insert(
        "direction".into(),
        Dynamic::from(match parameter.direction {
            IrFeatureDirection::In => "in",
            IrFeatureDirection::Out => "out",
            IrFeatureDirection::InOut => "inout",
            IrFeatureDirection::None => "none",
        }),
    );
    value.insert("name".into(), Dynamic::from(parameter.name.clone()));
    value.insert(
        "qualified_name".into(),
        Dynamic::from(parameter.qualified_name.clone()),
    );
    value.insert("type".into(), ir_type_dynamic(&parameter.ty));
    value.insert("source".into(), Dynamic::from(parameter.source.clone()));
    Dynamic::from_map(value)
}

fn ir_diagnostic_dynamic(diagnostic: &IrDiagnostic) -> Dynamic {
    let mut value = Map::new();
    value.insert(
        "severity".into(),
        Dynamic::from(match diagnostic.severity {
            DiagnosticSeverity::Warning => "warning",
            DiagnosticSeverity::Error => "error",
        }),
    );
    value.insert("code".into(), Dynamic::from(diagnostic.code.as_str()));
    value.insert("message".into(), Dynamic::from(diagnostic.message.clone()));
    value.insert(
        "source".into(),
        diagnostic
            .source
            .clone()
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT),
    );
    Dynamic::from_map(value)
}

fn ir_expression_dynamic(expression: &IrExpression) -> Dynamic {
    let mut value = Map::new();
    value.insert("source".into(), Dynamic::from(expression.source.clone()));
    value.insert("type".into(), ir_type_dynamic(&expression.result_type));
    match &expression.kind {
        IrExpressionKind::FeatureReference {
            path,
            qualified_name,
        } => {
            value.insert("kind".into(), Dynamic::from("feature_reference"));
            value.insert("path".into(), feature_path_dynamic(path));
            value.insert(
                "qualified_name".into(),
                Dynamic::from(qualified_name.clone()),
            );
        }
        IrExpressionKind::StandardConstant {
            constant,
            feature_element,
        } => {
            value.insert("kind".into(), Dynamic::from("standard_constant"));
            value.insert("constant".into(), Dynamic::from(*constant));
            value.insert("feature_element".into(), Dynamic::from(*feature_element));
        }
        IrExpressionKind::Literal(literal) => {
            value.insert("kind".into(), Dynamic::from("literal"));
            value.insert("literal".into(), Dynamic::from(format!("{literal:?}")));
        }
        IrExpressionKind::Unary { operator, operand } => {
            value.insert("kind".into(), Dynamic::from("unary"));
            value.insert("operator".into(), Dynamic::from(format!("{operator:?}")));
            value.insert("operand".into(), ir_expression_dynamic(operand));
        }
        IrExpressionKind::Binary {
            operator,
            left,
            right,
        } => {
            value.insert("kind".into(), Dynamic::from("binary"));
            value.insert("operator".into(), Dynamic::from(format!("{operator:?}")));
            value.insert("left".into(), ir_expression_dynamic(left));
            value.insert("right".into(), ir_expression_dynamic(right));
        }
        IrExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            value.insert("kind".into(), Dynamic::from("conditional"));
            value.insert("condition".into(), ir_expression_dynamic(condition));
            value.insert("when_true".into(), ir_expression_dynamic(when_true));
            value.insert("when_false".into(), ir_expression_dynamic(when_false));
        }
        IrExpressionKind::Invocation {
            function,
            function_element,
            argument_parameters,
            arguments,
        } => {
            value.insert("kind".into(), Dynamic::from("invocation"));
            value.insert("function".into(), Dynamic::from(*function));
            value.insert("function_element".into(), Dynamic::from(*function_element));
            value.insert(
                "argument_parameters".into(),
                Dynamic::from_array(
                    argument_parameters
                        .iter()
                        .copied()
                        .map(Dynamic::from)
                        .collect(),
                ),
            );
            value.insert(
                "arguments".into(),
                Dynamic::from_array(arguments.iter().map(ir_expression_dynamic).collect()),
            );
        }
        IrExpressionKind::Index { collection, index } => {
            value.insert("kind".into(), Dynamic::from("index"));
            value.insert("collection".into(), ir_expression_dynamic(collection));
            value.insert("index".into(), ir_expression_dynamic(index));
        }
        IrExpressionKind::Collection(elements) => {
            value.insert("kind".into(), Dynamic::from("collection"));
            value.insert(
                "elements".into(),
                Dynamic::from_array(elements.iter().map(ir_expression_dynamic).collect()),
            );
        }
        IrExpressionKind::Group(child) => {
            value.insert("kind".into(), Dynamic::from("group"));
            value.insert("child".into(), ir_expression_dynamic(child));
        }
    }
    Dynamic::from_map(value)
}

fn feature_path_dynamic(path: &SysmlFeaturePath) -> Dynamic {
    Dynamic::from_array(path.features().iter().copied().map(Dynamic::from).collect())
}

fn ir_type_dynamic(ty: &IrType) -> Dynamic {
    let mut value = Map::new();
    value.insert("value".into(), Dynamic::from(ir_value_type_name(&ty.value)));
    value.insert(
        "lower".into(),
        Dynamic::from_int(ty.multiplicity.lower as i64),
    );
    value.insert(
        "upper".into(),
        ty.multiplicity
            .upper
            .map(|upper| Dynamic::from_int(upper as i64))
            .unwrap_or(Dynamic::UNIT),
    );
    value.insert(
        "ordered".into(),
        Dynamic::from_bool(ty.multiplicity.ordered),
    );
    value.insert("unique".into(), Dynamic::from_bool(ty.multiplicity.unique));
    value.insert(
        "unit".into(),
        ty.unit.clone().map(Dynamic::from).unwrap_or(Dynamic::UNIT),
    );
    Dynamic::from_map(value)
}

fn ir_value_type_name(value: &IrValueType) -> &'static str {
    match value {
        IrValueType::Boolean => "Boolean",
        IrValueType::Integer => "Integer",
        IrValueType::Rational => "Rational",
        IrValueType::Real => "Real",
        IrValueType::Complex => "Complex",
        IrValueType::String => "String",
        IrValueType::Quantity { .. } => "Quantity",
        IrValueType::Enumeration { .. } => "Enumeration",
        IrValueType::Reference { .. } => "Reference",
        IrValueType::Structured { .. } => "Structured",
        IrValueType::Unknown => "Unknown",
    }
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
    if let Some(integer) = literal.integer_value {
        value.insert("integer_value".into(), Dynamic::from_int(integer));
    }
    if let Some(boolean) = literal.boolean_value {
        value.insert("boolean_value".into(), Dynamic::from_bool(boolean));
    }
    if let Some(string) = &literal.string_value {
        value.insert("string_value".into(), Dynamic::from(string.clone()));
    }
    if let Some(unit) = &literal.unit {
        value.insert("unit".into(), Dynamic::from(unit.clone()));
    }
    value.insert(
        "literal_kind".into(),
        Dynamic::from(format!("{:?}", literal.literal_kind)),
    );
    if let Some(elements) = &literal.elements {
        value.insert(
            "elements".into(),
            Dynamic::from_array(elements.iter().map(literal_dynamic).collect()),
        );
    }
    Dynamic::from_map(value)
}

fn attribute_dynamic_at_revision(attribute: &SysmlAttribute, revision: u64) -> Dynamic {
    let mut value = Map::new();
    value.insert("handle".into(), Dynamic::from(attribute.handle));
    value.insert(
        "owner_handle".into(),
        attribute
            .owner_handle
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT),
    );
    value.insert("owner".into(), Dynamic::from(attribute.owner.clone()));
    value.insert("name".into(), Dynamic::from(attribute.name.clone()));
    value.insert(
        "qualified_name".into(),
        Dynamic::from(attribute.qualified_name.clone()),
    );
    if let Some(type_name) = optional_string(&attribute.type_name) {
        value.insert("type_name".into(), type_name);
    }
    if let Some(declared_type) = &attribute.declared_type {
        value.insert("declared_type".into(), Dynamic::from(declared_type.clone()));
    }
    if let Some(literal) = &attribute.value {
        value.insert("value".into(), literal_dynamic(literal));
    }
    if let Some(typed_value) = typed_attribute_value_dynamic(attribute) {
        value.insert("typed_value".into(), typed_value);
    }
    value.insert("file".into(), Dynamic::from(attribute.file.clone()));
    value.insert("start".into(), Dynamic::from_int(attribute.start as i64));
    value.insert("end".into(), Dynamic::from_int(attribute.end as i64));
    value.insert(
        "source".into(),
        Dynamic::from(attribute.source_ref(revision)),
    );
    Dynamic::from_map(value)
}

fn requirement_dynamic(record: &lunco_sysml_ast::SysmlRequirementRecord, revision: u64) -> Dynamic {
    let mut value = Map::new();
    value.insert("element".into(), element_dynamic(&record.element));
    value.insert("documentation".into(), string_array(&record.documentation));
    value.insert("subjects".into(), subject_array(&record.subjects));
    value.insert(
        "attributes".into(),
        Dynamic::from_array(
            record
                .attributes
                .iter()
                .map(|attribute| attribute_dynamic_at_revision(attribute, revision))
                .collect(),
        ),
    );
    value.insert(
        "constraints".into(),
        Dynamic::from_array(
            record
                .constraints
                .iter()
                .map(|constraint| {
                    let mut item = Map::new();
                    item.insert("kind".into(), Dynamic::from(constraint.kind.clone()));
                    item.insert(
                        "qualified_name".into(),
                        Dynamic::from(constraint.usage.qualified_name.clone()),
                    );
                    item.insert("usage".into(), element_dynamic(&constraint.usage));
                    if let Some(definition) = &constraint.definition {
                        item.insert(
                            "definition_name".into(),
                            Dynamic::from(definition.qualified_name.clone()),
                        );
                        item.insert("definition".into(), element_dynamic(definition));
                    }
                    Dynamic::from_map(item)
                })
                .collect(),
        ),
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
    value.insert(
        "verified_requirements".into(),
        Dynamic::from_array(
            record
                .verified_requirements
                .iter()
                .copied()
                .map(Dynamic::from)
                .collect(),
        ),
    );
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
