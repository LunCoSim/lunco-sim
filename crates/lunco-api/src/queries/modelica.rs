//! Modelica solver-step input provenance queries.

use super::*;

/// Read the exact inputs and outputs exchanged in each Modelica participant's
/// last accepted solver step, with the solver's input and output times.
pub struct ReadModelicaStepSamplesProvider;

impl ApiQueryProvider for ReadModelicaStepSamplesProvider {
    fn name(&self) -> &'static str {
        "ReadModelicaStepSamples"
    }

    fn schema(&self) -> ApiQuerySchema {
        query_schema(
            self.name(),
            "Read selected inputs and outputs from each Modelica participant's last accepted solver step.",
            vec![required_parameter(
                "targets",
                "{ api_id: u64, input_names: string[], output_names: string[] }[]",
                &format!(
                    "Unique Modelica entities and exact input/output names, between 1 and {} targets.",
                    lunco_port_core::ports::MAX_PORT_BATCH_TARGETS
                ),
            )],
            "{ sim_tick, entities: [{ api_id, sample: null | { session_id, step_id, input_time_s, output_time_s, sampled_inputs, sampled_outputs } }] }",
        )
    }

    fn simulation_read_scope(&self, _params: &ApiValue) -> SimulationQueryReadScope {
        SimulationQueryReadScope::EntityTargets
    }

    fn simulation_entity_reads(&self, params: &ApiValue) -> Vec<lunco_core::GlobalEntityId> {
        api_param_array(params, "targets")
            .into_iter()
            .flatten()
            .filter_map(|target| target.get("api_id").and_then(api_value_u64))
            .map(lunco_core::GlobalEntityId::from_raw)
            .collect()
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let raw_targets = api_param_array(params, "targets").ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "ReadModelicaStepSamples: `targets` ({ api_id: u64, input_names: string[], output_names: string[] }[]) required",
            )
        })?;
        if raw_targets.is_empty()
            || raw_targets.len() > lunco_port_core::ports::MAX_PORT_BATCH_TARGETS
        {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!(
                    "ReadModelicaStepSamples: `targets` must contain 1..={} entities",
                    lunco_port_core::ports::MAX_PORT_BATCH_TARGETS
                ),
            ));
        }

        let mut targets = Vec::with_capacity(raw_targets.len());
        let mut seen = HashSet::with_capacity(raw_targets.len());
        let mut total_requested_rows = 0usize;
        for target in raw_targets {
            let api_id = target
                .get("api_id")
                .and_then(api_value_u64)
                .ok_or_else(|| {
                    ApiQueryError::new(
                        ApiErrorCode::DeserializationError,
                        "ReadModelicaStepSamples: every target requires an unsigned `api_id`",
                    )
                })?;
            if !seen.insert(api_id) {
                return Err(ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    format!("ReadModelicaStepSamples: duplicate api_id {api_id}"),
                ));
            }
            let input_names = parse_selected_names(
                target,
                "input_names",
                "ReadModelicaStepSamples",
            )
            .map_err(|error| {
                ApiQueryError::new(
                    error.code,
                    format!("ReadModelicaStepSamples target {api_id}: {}", error.message),
                )
            })?;
            let output_names = parse_selected_names(
                target,
                "output_names",
                "ReadModelicaStepSamples",
            )
            .map_err(|error| {
                ApiQueryError::new(
                    error.code,
                    format!("ReadModelicaStepSamples target {api_id}: {}", error.message),
                )
            })?;
            total_requested_rows += input_names.len() + output_names.len();
            if total_requested_rows > super::MAX_PORT_READ_ROWS {
                return Err(ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    format!(
                        "ReadModelicaStepSamples: selected input/output rows exceed the {}-row batch limit",
                        super::MAX_PORT_READ_ROWS
                    ),
                ));
            }
            targets.push((api_id, input_names, output_names));
        }

        let sim_tick = world
            .get_resource::<lunco_core_runtime::SimTick>()
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::InternalError,
                    "ReadModelicaStepSamples: SimTick resource is not present",
                )
            })?
            .0;
        let entities = world.get_resource::<ApiEntityRegistry>().ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::InternalError,
                "ReadModelicaStepSamples: ApiEntityRegistry resource is not present",
            )
        })?;

        let rows = targets
            .into_iter()
            .map(|(api_id, input_names, output_names)| {
                let gid = lunco_core::GlobalEntityId::from_raw(api_id);
                let entity = entities.resolve(&gid).ok_or_else(|| {
                    ApiQueryError::new(
                        ApiErrorCode::EntityNotFound,
                        format!("ReadModelicaStepSamples: no entity for api_id {api_id}"),
                    )
                })?;
                let model = world.get::<lunco_modelica_runtime::ModelicaModel>(entity).ok_or_else(
                    || {
                        ApiQueryError::new(
                            ApiErrorCode::EntityNotFound,
                            format!(
                                "ReadModelicaStepSamples: api_id {api_id} is not a Modelica participant"
                            ),
                        )
                    },
                )?;

                let sample = if let Some(sample) = &model.last_accepted_step {
                    let sampled_inputs = input_names
                        .iter()
                        .map(|name| {
                            let value = sample
                                .inputs
                                .iter()
                                .find_map(|(sampled_name, value)| {
                                    (sampled_name == name).then_some(*value)
                                })
                                .ok_or_else(|| {
                                    ApiQueryError::new(
                                        ApiErrorCode::DeserializationError,
                                        format!(
                                            "ReadModelicaStepSamples target {api_id}: last accepted step {} did not consume input `{name}`",
                                            sample.step_id
                                        ),
                                    )
                                })?;
                            if !value.is_finite() {
                                return Err(ApiQueryError::new(
                                    ApiErrorCode::InternalError,
                                    format!(
                                        "ReadModelicaStepSamples target {api_id}: accepted step {} captured a non-finite `{name}` value",
                                        sample.step_id
                                    ),
                                ));
                            }
                            Ok((name.clone(), ApiValue::Float(value)))
                        })
                        .collect::<Result<Vec<_>, ApiQueryError>>()?;
                    let sampled_outputs = output_names
                        .iter()
                        .map(|name| {
                            let value = sample
                                .outputs
                                .iter()
                                .find_map(|(sampled_name, value)| {
                                    (sampled_name == name).then_some(*value)
                                })
                                .ok_or_else(|| {
                                    ApiQueryError::new(
                                        ApiErrorCode::DeserializationError,
                                        format!(
                                            "ReadModelicaStepSamples target {api_id}: last accepted step {} did not produce output `{name}`",
                                            sample.step_id
                                        ),
                                    )
                                })?;
                            if !value.is_finite() {
                                return Err(ApiQueryError::new(
                                    ApiErrorCode::InternalError,
                                    format!(
                                        "ReadModelicaStepSamples target {api_id}: accepted step {} produced a non-finite `{name}` value",
                                        sample.step_id
                                    ),
                                ));
                            }
                            Ok((name.clone(), ApiValue::Float(value)))
                        })
                        .collect::<Result<Vec<_>, ApiQueryError>>()?;
                    ApiValue::map([
                        ("session_id", api_value_from_u64(sample.session_id)),
                        ("step_id", api_value_from_u64(sample.step_id)),
                        ("input_time_s", ApiValue::Float(sample.input_time_s)),
                        ("output_time_s", ApiValue::Float(sample.output_time_s)),
                        ("sampled_inputs", ApiValue::Map(sampled_inputs)),
                        ("sampled_outputs", ApiValue::Map(sampled_outputs)),
                    ])
                } else {
                    ApiValue::Unit
                };

                Ok(ApiValue::map([
                    ("api_id", api_value_from_u64(api_id)),
                    ("sample", sample),
                ]))
            })
            .collect::<Result<Vec<_>, ApiQueryError>>()?;

        Ok(Some(ApiValue::map([
            ("sim_tick", api_value_from_u64(sim_tick)),
            ("entities", ApiValue::Array(rows)),
        ])))
    }
}
