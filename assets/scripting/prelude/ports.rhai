// Generic scalar-port helpers. Rust owns port discovery, metadata validation,
// same-tick snapshots, and actuator convergence; authored scripts choose which
// entities and mechanisms to inspect.

/// Read selected ports and owner metadata. `port_names` are exact names
/// discovered through ListPorts; callers must state which values they need.
fn read_ports(entity, port_names) {
    query("ReadPorts", #{ api_id: entity, port_names: port_names })
}

/// Read selected ports across entities from one immutable simulation-tick snapshot.
/// Each target is #{api_id: stable_entity_id, port_names: [exact_port_names]}.
fn read_ports_batch(targets) {
    query("ReadPortsBatch", #{ targets: targets })
}

/// Read selected inputs and outputs from each Modelica participant's last
/// accepted step. A target is #{api_id: stable_entity_id,
/// input_names: [names], output_names: [names]}.
fn read_modelica_step_samples(targets) {
    query("ReadModelicaStepSamples", #{ targets: targets })
}

/// Compare an actuator target, measured value, and measured rate. `tolerance`
/// uses the actuator value's unit; `rate_tolerance` uses the rate output's unit.
fn actuator_status(entity, command_port, measured_port, rate_port,
                   tolerance, rate_tolerance) {
    query("ReadActuatorStatus", #{
        api_id: entity,
        command_port: command_port,
        measured_port: measured_port,
        rate_port: rate_port,
        tolerance: tolerance,
        rate_tolerance: rate_tolerance,
    })
}

/// Atomically write named inputs on one entity through the shared SetPorts
/// admission and metadata-validation path. `writes` is an array of
/// `[port_name, scalar]` pairs.
fn write_ports(entity, writes) {
    cmd("SetPorts", #{ target: entity, writes: writes })
}

/// Atomically write inputs on multiple entities. Each batch is a map with a
/// stable `target` id and an array of `[port_name, scalar]` pairs in `writes`.
/// The command commits only after every target and write passes validation.
fn write_ports_batch(batches) {
    cmd("SetPortsBatch", #{ batches: batches })
}
