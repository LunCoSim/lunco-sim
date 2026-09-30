//! Read-only port queries with explicit snapshot and actuator contracts.

use super::*;

/// `ReadPortsBatch` returns selected port rows for stable entity identities,
/// all sampled at one simulation tick.
pub struct ReadPortsBatchProvider;

impl ApiQueryProvider for ReadPortsBatchProvider {
    fn name(&self) -> &'static str {
        "ReadPortsBatch"
    }

    fn schema(&self) -> ApiQuerySchema {
        let target_limit = lunco_port_core::ports::MAX_PORT_BATCH_TARGETS;
        query_schema(
            self.name(),
            "Read explicitly selected ports across stable entity identities at one simulation tick.",
            vec![required_parameter(
                "targets",
                "{ api_id: u64, port_names: string[] }[]",
                &format!(
                    "Unique entity targets with selected port names, between 1 and {target_limit} entries."
                ),
            )],
            format!(
                "{{ sim_tick, entities: [{{ api_id, ports: [{}] }}] }}",
                super::port_info_api_schema()
            ),
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
                "ReadPortsBatch: `targets` ({ api_id: u64, port_names: string[] }[]) required",
            )
        })?;
        if raw_targets.is_empty()
            || raw_targets.len() > lunco_port_core::ports::MAX_PORT_BATCH_TARGETS
        {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!(
                    "ReadPortsBatch: `targets` must contain 1..={} entities",
                    lunco_port_core::ports::MAX_PORT_BATCH_TARGETS
                ),
            ));
        }
        let mut targets = Vec::with_capacity(raw_targets.len());
        let mut seen = std::collections::HashSet::with_capacity(raw_targets.len());
        for target in raw_targets {
            let api_id = target
                .get("api_id")
                .and_then(api_value_u64)
                .ok_or_else(|| {
                    ApiQueryError::new(
                        ApiErrorCode::DeserializationError,
                        "ReadPortsBatch: every target requires an unsigned `api_id`",
                    )
                })?;
            if !seen.insert(api_id) {
                return Err(ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    format!("ReadPortsBatch: duplicate api_id {api_id}"),
                ));
            }
            let port_names = super::parse_port_names(target, self.name()).map_err(|error| {
                ApiQueryError::new(
                    error.code,
                    format!("ReadPortsBatch target {api_id}: {}", error.message),
                )
            })?;
            targets.push((api_id, port_names));
        }

        let sim_tick = world
            .get_resource::<lunco_core_runtime::SimTick>()
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::InternalError,
                    "ReadPortsBatch: SimTick resource is not present",
                )
            })?
            .0;
        let entities = world.get_resource::<ApiEntityRegistry>().ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::InternalError,
                "ReadPortsBatch: ApiEntityRegistry resource is not present",
            )
        })?;
        let registry = world
            .get_resource::<lunco_port_core::ports::PortRegistry>()
            .cloned()
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::InternalError,
                    "ReadPortsBatch: PortRegistry resource is not present",
                )
            })?;

        let mut total_rows = 0usize;
        let rows = targets
            .into_iter()
            .map(|(api_id, port_names)| {
                let gid = lunco_core::GlobalEntityId::from_raw(api_id);
                let entity = entities.resolve(&gid).ok_or_else(|| {
                    ApiQueryError::new(
                        ApiErrorCode::EntityNotFound,
                        format!("ReadPortsBatch: no entity for api_id {api_id}"),
                    )
                })?;
                let ports = super::read_entity_ports(world, &registry, entity, &port_names)
                    .map_err(|message| {
                        ApiQueryError::new(
                            ApiErrorCode::DeserializationError,
                            format!("ReadPortsBatch target {api_id}: {message}"),
                        )
                    })?;
                total_rows += ports.len();
                if total_rows > super::MAX_PORT_READ_ROWS {
                    return Err(ApiQueryError::new(
                        ApiErrorCode::DeserializationError,
                        format!(
                            "ReadPortsBatch: selected port rows exceed the {}-row batch limit",
                            super::MAX_PORT_READ_ROWS
                        ),
                    ));
                }
                Ok(ApiValue::map([
                    ("api_id", api_value_from_u64(api_id)),
                    ("ports", ApiValue::Array(ports)),
                ]))
            })
            .collect::<Result<Vec<_>, ApiQueryError>>()?;

        Ok(Some(ApiValue::map([
            ("sim_tick", api_value_from_u64(sim_tick)),
            ("entities", ApiValue::Array(rows)),
        ])))
    }
}

/// Compare a writable command sample with a measured output in the command
/// port's declared unit and frame.
pub struct ReadActuatorStatusProvider;

impl ApiQueryProvider for ReadActuatorStatusProvider {
    fn name(&self) -> &'static str {
        "ReadActuatorStatus"
    }

    fn schema(&self) -> ApiQuerySchema {
        query_schema(
            self.name(),
            "Compare an actuator target, measured value, and measured rate at the current simulation tick.",
            vec![
                required_parameter("api_id", "u64", "Stable entity identity for the actuator."),
                required_parameter(
                    "command_port",
                    "string",
                    "Writable input port containing the target.",
                ),
                required_parameter(
                    "measured_port",
                    "string",
                    "Output port containing the measured value.",
                ),
                required_parameter(
                    "rate_port",
                    "string",
                    "Output port containing the measured time derivative of the actuator value.",
                ),
                required_parameter(
                    "tolerance",
                    "f64",
                    "Nonnegative tolerance expressed in the command port's unit.",
                ),
                required_parameter(
                    "rate_tolerance",
                    "f64",
                    "Nonnegative stationary-rate threshold expressed in the rate port's unit.",
                ),
            ],
            format!(
                "{{ api_id, sim_tick, target, measured, error, rate, rate_tolerance, motion_state, tolerance, limits, unit: {}, rate_unit: {}, frame: string | (), settled, settlement_state }}",
                super::port_unit_api_schema(),
                super::port_unit_api_schema(),
            ),
        )
    }

    fn simulation_read_scope(&self, _params: &ApiValue) -> SimulationQueryReadScope {
        SimulationQueryReadScope::EntityTargets
    }

    fn simulation_entity_reads(&self, params: &ApiValue) -> Vec<lunco_core::GlobalEntityId> {
        api_param_u64(params, "api_id")
            .map(lunco_core::GlobalEntityId::from_raw)
            .into_iter()
            .collect()
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let api_id = api_param_u64(params, "api_id").ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "ReadActuatorStatus: `api_id` (u64) required",
            )
        })?;
        let command_port = nonempty_string_parameter(params, "command_port")?;
        let measured_port = nonempty_string_parameter(params, "measured_port")?;
        let rate_port = nonempty_string_parameter(params, "rate_port")?;
        let tolerance = api_param_f64(params, "tolerance")
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "ReadActuatorStatus: `tolerance` must be finite and nonnegative",
                )
            })?;
        let rate_tolerance = api_param_f64(params, "rate_tolerance")
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "ReadActuatorStatus: `rate_tolerance` must be finite and nonnegative",
                )
            })?;

        let gid = lunco_core::GlobalEntityId::from_raw(api_id);
        let entity = world
            .get_resource::<ApiEntityRegistry>()
            .and_then(|registry| registry.resolve(&gid))
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::EntityNotFound,
                    format!("ReadActuatorStatus: no entity for api_id {api_id}"),
                )
            })?;
        let registry = world
            .get_resource::<lunco_port_core::ports::PortRegistry>()
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::InternalError,
                    "ReadActuatorStatus: PortRegistry resource is not present",
                )
            })?;
        let command_metadata = registry
            .input_port_metadata(world, entity, command_port)
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::EntityNotFound,
                    format!("ReadActuatorStatus: input port `{command_port}` does not exist"),
                )
            })?;
        if !command_metadata.writable {
            return Err(ApiQueryError::new(
                ApiErrorCode::CommandRejected,
                format!("ReadActuatorStatus: input port `{command_port}` is not writable"),
            ));
        }
        let measured_metadata = registry
            .output_port_metadata(world, entity, measured_port)
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::EntityNotFound,
                    format!("ReadActuatorStatus: output port `{measured_port}` does not exist"),
                )
            })?;
        let rate_metadata = registry
            .output_port_metadata(world, entity, rate_port)
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::EntityNotFound,
                    format!("ReadActuatorStatus: rate output port `{rate_port}` does not exist"),
                )
            })?;
        if command_metadata.value_type != measured_metadata.value_type
            || command_metadata.frame != measured_metadata.frame
        {
            return Err(ApiQueryError::new(
                ApiErrorCode::CommandRejected,
                format!(
                    "ReadActuatorStatus: `{command_port}` and `{measured_port}` have incompatible value type or frame metadata"
                ),
            ));
        }
        if rate_metadata.value_type != measured_metadata.value_type
            || rate_metadata.frame != measured_metadata.frame
        {
            return Err(ApiQueryError::new(
                ApiErrorCode::CommandRejected,
                format!(
                    "ReadActuatorStatus: rate output `{rate_port}` does not share the measured actuator value's type and frame"
                ),
            ));
        }
        let expected_rate_dimension = measured_metadata
            .unit
            .as_ref()
            .and_then(|unit| unit.definition())
            .map(|unit| {
                unit.dimension()
                    .checked_quotient(lunco_engineering_values::Dimension([0, 0, 1, 0, 0, 0, 0]))
            })
            .transpose()
            .map_err(|error| {
                ApiQueryError::new(
                    ApiErrorCode::InternalError,
                    format!("ReadActuatorStatus: actuator unit dimension is invalid: {error}"),
                )
            })?;
        let rate_dimension = rate_metadata
            .unit
            .as_ref()
            .and_then(|unit| unit.definition())
            .map(lunco_engineering_values::Unit::dimension);
        if expected_rate_dimension.is_none() || rate_dimension != expected_rate_dimension {
            return Err(ApiQueryError::new(
                ApiErrorCode::CommandRejected,
                format!(
                    "ReadActuatorStatus: rate output `{rate_port}` requires a resolved unit equal to the measured value's unit per second"
                ),
            ));
        }

        let target = registry
            .read_owned_input_port(world, entity, command_port)
            .filter(|value| value.is_finite())
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::CommandRejected,
                    format!("ReadActuatorStatus: `{command_port}` has no finite target sample"),
                )
            })?;
        let measured_sample = registry
            .read_owned_output_port(world, entity, measured_port)
            .filter(|value| value.is_finite())
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::CommandRejected,
                    format!("ReadActuatorStatus: `{measured_port}` has no finite measured sample"),
                )
            })?;
        let rate = registry
            .read_owned_output_port(world, entity, rate_port)
            .filter(|value| value.is_finite())
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::CommandRejected,
                    format!("ReadActuatorStatus: `{rate_port}` has no finite rate sample"),
                )
            })?;
        let measured = match (&measured_metadata.unit, &command_metadata.unit) {
            (Some(measured_unit), Some(command_unit)) => measured_unit
                .convert_value_to(measured_sample, command_unit)
                .map_err(|error| {
                    ApiQueryError::new(
                        ApiErrorCode::CommandRejected,
                        format!("ReadActuatorStatus: cannot compare actuator units: {error}"),
                    )
                })?,
            (None, None) => measured_sample,
            _ => {
                return Err(ApiQueryError::new(
                    ApiErrorCode::CommandRejected,
                    format!(
                        "ReadActuatorStatus: `{command_port}` and `{measured_port}` must both declare a unit or neither declare one"
                    ),
                ));
            }
        };
        let sim_tick = world
            .get_resource::<lunco_core_runtime::SimTick>()
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::InternalError,
                    "ReadActuatorStatus: SimTick resource is not present",
                )
            })?
            .0;
        let error = target - measured;
        let moving = rate.abs() > rate_tolerance;
        let settled = error.abs() <= tolerance && !moving;
        let limits = range_value(command_metadata.min, command_metadata.max);
        Ok(Some(ApiValue::map([
            ("api_id", api_value_from_u64(api_id)),
            ("sim_tick", api_value_from_u64(sim_tick)),
            ("command_port", ApiValue::str(command_port)),
            ("measured_port", ApiValue::str(measured_port)),
            ("rate_port", ApiValue::str(rate_port)),
            ("target", ApiValue::Float(target)),
            ("measured", ApiValue::Float(measured)),
            ("measured_sample", ApiValue::Float(measured_sample)),
            ("error", ApiValue::Float(error)),
            ("rate", ApiValue::Float(rate)),
            ("rate_tolerance", ApiValue::Float(rate_tolerance)),
            (
                "motion_state",
                ApiValue::str(if moving { "moving" } else { "stationary" }),
            ),
            ("tolerance", ApiValue::Float(tolerance)),
            ("limits", limits),
            (
                "unit",
                super::unit_reference_to_api_value(command_metadata.unit.as_ref()),
            ),
            (
                "frame",
                super::coordinate_frame_to_api_value(command_metadata.frame.as_ref()),
            ),
            (
                "rate_unit",
                super::unit_reference_to_api_value(rate_metadata.unit.as_ref()),
            ),
            ("settled", ApiValue::Bool(settled)),
            (
                "settlement_state",
                ApiValue::str(if settled { "settled" } else { "unsettled" }),
            ),
        ])))
    }
}

fn nonempty_string_parameter<'a>(
    params: &'a ApiValue,
    name: &str,
) -> Result<&'a str, ApiQueryError> {
    api_param_str(params, name)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!("ReadActuatorStatus: non-empty `{name}` (string) required"),
            )
        })
}

fn range_value(min: Option<f64>, max: Option<f64>) -> ApiValue {
    match (min, max) {
        (Some(min), Some(max)) => {
            ApiValue::map([("min", ApiValue::Float(min)), ("max", ApiValue::Float(max))])
        }
        (Some(min), None) => ApiValue::map([("min", ApiValue::Float(min))]),
        (None, Some(max)) => ApiValue::map([("max", ApiValue::Float(max))]),
        (None, None) => ApiValue::Unit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_engineering_values::{Dimension, Unit, UnitReference};
    use lunco_port_core::ports::{
        PortBackend, PortDeclaration, PortDirection, PortMetadata, PortRegistry,
    };

    #[derive(Component)]
    struct TestActuator {
        target_cm: f64,
        measured_m: f64,
        rate_mps: f64,
    }

    const TEST_BACKEND: PortBackend = PortBackend {
        list_entities: |world, out| {
            out.extend(
                world
                    .query_filtered::<Entity, With<TestActuator>>()
                    .iter(world),
            );
        },
        topology_key: |_world, _entity| 1,
        list: |world, entity, out| {
            if world.get::<TestActuator>(entity).is_some() {
                out.extend([
                    PortDeclaration {
                        name: "target".to_owned(),
                        direction: PortDirection::In,
                    },
                    PortDeclaration {
                        name: "measured".to_owned(),
                        direction: PortDirection::Out,
                    },
                    PortDeclaration {
                        name: "rate".to_owned(),
                        direction: PortDirection::Out,
                    },
                ]);
            }
        },
        metadata: |_world, _entity, name, direction| {
            let (unit, min, max, writable) = match name {
                "target" => (
                    unit("cm", Dimension::LENGTH, 0.01),
                    Some(-500.0),
                    Some(500.0),
                    true,
                ),
                "measured" => (unit("m", Dimension::LENGTH, 1.0), None, None, false),
                "rate" => (
                    unit("m/s", Dimension([1, 0, -1, 0, 0, 0, 0]), 1.0),
                    None,
                    None,
                    false,
                ),
                _ => unreachable!("test backend only declares the listed ports"),
            };
            PortMetadata::scalar(
                direction,
                Some(unit),
                min,
                max,
                "test actuator",
                if writable { "controller" } else { "solver" },
                writable,
                None,
            )
        },
        read_output: |world, entity, name| {
            let actuator = world.get::<TestActuator>(entity)?;
            match name {
                "measured" => Some(actuator.measured_m),
                "rate" => Some(actuator.rate_mps),
                _ => None,
            }
        },
        read_input: |world, entity, name| {
            (name == "target")
                .then(|| {
                    world
                        .get::<TestActuator>(entity)
                        .map(|actuator| actuator.target_cm)
                })
                .flatten()
        },
        resolve_output: Some(|_world, _entity, name| match name {
            "measured" => Some(1),
            "rate" => Some(2),
            _ => None,
        }),
        resolve_input: Some(|_world, _entity, name| (name == "target").then_some(0)),
        read_slot: Some(|world, entity, slot| {
            let actuator = world.get::<TestActuator>(entity)?;
            match slot {
                1 => Some(actuator.measured_m),
                2 => Some(actuator.rate_mps),
                _ => None,
            }
        }),
        read_input_slot: Some(|world, entity, slot| {
            (slot == 0)
                .then(|| {
                    world
                        .get::<TestActuator>(entity)
                        .map(|actuator| actuator.target_cm)
                })
                .flatten()
        }),
        write_slot: Some(|world, entity, slot, value| {
            assert_eq!(slot, 0);
            world
                .get_mut::<TestActuator>(entity)
                .expect("test input owner remains live")
                .target_cm = value;
        }),
    };

    fn unit(symbol: &str, dimension: Dimension, scale_to_si: f64) -> UnitReference {
        UnitReference::resolved(
            Unit::new(symbol, dimension, scale_to_si, 0.0).expect("valid test unit"),
        )
    }

    fn install_actuator(world: &mut World, api_id: u64) -> Entity {
        let entity = world
            .spawn(TestActuator {
                target_cm: 100.0,
                measured_m: 0.995,
                rate_mps: 0.0001,
            })
            .id();
        world
            .resource_mut::<ApiEntityRegistry>()
            .assign(entity, lunco_core::GlobalEntityId::from_raw(api_id));
        entity
    }

    fn install_port_registry(world: &mut World) {
        let mut registry = PortRegistry::default();
        registry.register(TEST_BACKEND);
        world.insert_resource(registry);
    }

    #[test]
    fn read_ports_batch_keeps_requested_identity_order_and_one_tick() {
        let mut world = World::new();
        world.insert_resource(ApiEntityRegistry::default());
        world.insert_resource(lunco_core_runtime::SimTick(23));
        install_port_registry(&mut world);
        install_actuator(&mut world, 22);
        install_actuator(&mut world, 11);

        let result = ReadPortsBatchProvider
            .execute(
                &world,
                &ApiValue::map([(
                    "targets",
                    ApiValue::Array(vec![
                        ApiValue::map([
                            ("api_id", api_value_from_u64(22)),
                            (
                                "port_names",
                                ApiValue::Array(vec![
                                    ApiValue::str("target"),
                                    ApiValue::str("measured"),
                                    ApiValue::str("rate"),
                                ]),
                            ),
                        ]),
                        ApiValue::map([
                            ("api_id", api_value_from_u64(11)),
                            (
                                "port_names",
                                ApiValue::Array(vec![ApiValue::str("measured")]),
                            ),
                        ]),
                    ]),
                )]),
            )
            .expect("valid batch query")
            .expect("batch query has a result");

        assert_eq!(result.get("sim_tick"), Some(&api_value_from_u64(23)));
        let Some(ApiValue::Array(entities)) = result.get("entities") else {
            panic!("batch response includes an entity array");
        };
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0].get("api_id"), Some(&api_value_from_u64(22)));
        assert_eq!(entities[1].get("api_id"), Some(&api_value_from_u64(11)));
        let Some(ApiValue::Array(ports)) = entities[0].get("ports") else {
            panic!("each entity response includes its port samples");
        };
        assert_eq!(ports.len(), 3);
        let target = ports
            .iter()
            .find(|port| port.get("name") == Some(&ApiValue::str("target")))
            .expect("target port is present");
        assert_eq!(target.get("value"), Some(&ApiValue::Float(100.0)));
        let Some(ApiValue::Array(second_ports)) = entities[1].get("ports") else {
            panic!("second entity response includes its selected port samples");
        };
        assert_eq!(second_ports.len(), 1);
        assert_eq!(
            second_ports[0].get("name"),
            Some(&ApiValue::str("measured"))
        );
    }

    #[test]
    fn read_ports_returns_only_requested_names_and_rejects_missing_names() {
        let mut world = World::new();
        world.insert_resource(ApiEntityRegistry::default());
        install_port_registry(&mut world);
        install_actuator(&mut world, 42);

        let selected = ReadPortsProvider
            .execute(
                &world,
                &ApiValue::map([
                    ("api_id", api_value_from_u64(42)),
                    (
                        "port_names",
                        ApiValue::Array(vec![ApiValue::str("measured")]),
                    ),
                ]),
            )
            .expect("valid selected port query")
            .expect("selected port query has a result");
        let Some(ApiValue::Array(ports)) = selected.get("ports") else {
            panic!("selected port response includes a port array");
        };
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].get("name"), Some(&ApiValue::str("measured")));

        let missing = ReadPortsProvider
            .execute(
                &world,
                &ApiValue::map([
                    ("api_id", api_value_from_u64(42)),
                    (
                        "port_names",
                        ApiValue::Array(vec![ApiValue::str("missing")]),
                    ),
                ]),
            )
            .expect_err("unknown port names are rejected");
        assert!(missing.message.contains("missing"));
    }

    #[test]
    fn actuator_status_converts_units_and_requires_both_error_and_rate_tolerances() {
        let mut world = World::new();
        world.insert_resource(ApiEntityRegistry::default());
        world.insert_resource(lunco_core_runtime::SimTick(31));
        install_port_registry(&mut world);
        install_actuator(&mut world, 42);

        let params = ApiValue::map([
            ("api_id", api_value_from_u64(42)),
            ("command_port", ApiValue::str("target")),
            ("measured_port", ApiValue::str("measured")),
            ("rate_port", ApiValue::str("rate")),
            ("tolerance", ApiValue::Float(1.0)),
            ("rate_tolerance", ApiValue::Float(0.001)),
        ]);
        let result = ReadActuatorStatusProvider
            .execute(&world, &params)
            .expect("valid actuator query")
            .expect("actuator query has a result");

        assert_eq!(result.get("sim_tick"), Some(&api_value_from_u64(31)));
        assert_eq!(result.get("target"), Some(&ApiValue::Float(100.0)));
        assert_eq!(result.get("measured"), Some(&ApiValue::Float(99.5)));
        assert_eq!(result.get("error"), Some(&ApiValue::Float(0.5)));
        assert_eq!(result.get("settled"), Some(&ApiValue::Bool(true)));

        let not_settled = ReadActuatorStatusProvider
            .execute(
                &world,
                &ApiValue::map([
                    ("api_id", api_value_from_u64(42)),
                    ("command_port", ApiValue::str("target")),
                    ("measured_port", ApiValue::str("measured")),
                    ("rate_port", ApiValue::str("rate")),
                    ("tolerance", ApiValue::Float(0.1)),
                    ("rate_tolerance", ApiValue::Float(0.001)),
                ]),
            )
            .expect("valid actuator query")
            .expect("actuator query has a result");
        assert_eq!(not_settled.get("settled"), Some(&ApiValue::Bool(false)));
    }
}
