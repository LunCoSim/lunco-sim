// Generic scalar-port helpers. Rust owns port discovery, metadata validation,
// same-tick snapshots, and actuator convergence; authored scripts choose which
// entities and mechanisms to inspect.

/// Read one entity's ports and owner metadata. `entity` is a stable entity id
/// returned by `find()` or the entity hierarchy.
fn read_ports(entity) {
    query("ReadPorts", #{ api_id: entity })
}

/// Read several entities' ports from one immutable simulation-tick snapshot.
fn read_ports_batch(entities) {
    query("ReadPortsBatch", #{ api_ids: entities })
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
