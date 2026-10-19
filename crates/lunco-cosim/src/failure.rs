//! Terminal failures at the shared connection admission and exchange boundary.

use bevy::prelude::*;
use lunco_cosim_core::{BrokenConnection, CosimDiagnostics};
use lunco_port_core::ports::PortRegistry;
use std::sync::Arc;

/// Stop a failed communication point before the native solver consumes it.
/// Exchange runs after FixedPreUpdate, so waiting for the next admission pass
/// would allow the current native physics step to integrate rejected inputs.
pub(crate) fn stop(
    world: &mut World,
    kind: &'static str,
    entity: Option<Entity>,
    subject: &str,
    detail: String,
) {
    let has_fault_owner = world.contains_resource::<lunco_core::RuntimeFaults>();
    let first = world
        .get_resource_mut::<lunco_core::RuntimeFaults>()
        .is_some_and(|mut faults| faults.raise(kind, entity, subject, detail.clone()));
    if first || !has_fault_owner {
        warn!("[cosim] {kind}: {subject}: {detail}");
    }
    if let Some(mut holds) = world.get_resource_mut::<lunco_physics::PhysicsHolds>() {
        holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
    }
    // Ports-only hosts have no native clock; physical hosts must stop it now.
    if let Some(mut time) = world.get_resource_mut::<Time<lunco_physics::Physics>>() {
        use lunco_physics::PhysicsTime;
        time.pause();
        time.advance_by(std::time::Duration::ZERO);
    }
}

/// Publish an actual rejected input, including failures after earlier success.
/// The same typed diagnostic is retained by binding and the live exchange.
pub(crate) fn reject_input(
    world: &mut World,
    kind: &'static str,
    entity: Entity,
    port: Arc<str>,
    dropped_value: Option<f64>,
    detail: String,
) -> BrokenConnection {
    let registry = world.resource::<PortRegistry>().clone();
    let failure = BrokenConnection {
        entity,
        global_id: world.get::<lunco_core::GlobalEntityId>(entity).copied(),
        has_port_surface: !registry.entity_port_owners(world, entity).is_empty(),
        port,
        dropped_value,
        failure: Some(detail.clone()),
    };
    world
        .resource_mut::<CosimDiagnostics>()
        .record_fault(failure.clone());
    stop(world, kind, Some(entity), &failure.port, detail);
    failure
}
