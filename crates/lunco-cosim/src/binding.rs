//! Reactive connection binding.
//!
//! [`SimConnection`](crate::SimConnection) is authored topology.  It becomes an
//! executable edge only after both endpoint lifecycles are terminal and their
//! named ports resolve.  This keeps loading order out of the fixed-step master.

use bevy::prelude::*;
use lunco_core::ports::PortRegistry;

use crate::{diagnostics::BrokenConnection, CosimDiagnostics, SimConnection};

/// Runtime lifecycle of a port-owning endpoint.
#[derive(Component, Debug, Clone, PartialEq, Eq, Default)]
pub enum EndpointLifecycle {
    #[default]
    Pending,
    Ready,
    Failed(String),
}

/// Binding result for an immutable [`SimConnection`] specification.
#[derive(Component, Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionBinding {
    #[default]
    Pending,
    Bound,
    Failed,
}

/// Marker consumed by propagation.  A connection specification without this
/// marker is topology waiting to bind, never a half-live wire.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct BoundConnection;

/// A monotonic, event-driven request to reconsider connection specifications.
#[derive(Resource, Debug, Default)]
pub struct BindingRevision {
    revision: u64,
    consumed: u64,
    /// `true` only after the scene/instance projection epoch has settled.
    pub sealed: bool,
}

impl BindingRevision {
    pub fn request(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn pending(&self) -> bool {
        self.consumed != self.revision
    }
    pub fn open_epoch(&mut self) {
        self.sealed = false;
        self.request();
    }
    pub fn seal_epoch(&mut self) {
        self.sealed = true;
        self.request();
    }
    fn take_request(&mut self) -> bool {
        if self.consumed == self.revision {
            return false;
        }
        self.consumed = self.revision;
        true
    }
}

/// Lifecycle observers call this after publishing a new endpoint state.
pub fn request_binding(mut revision: ResMut<BindingRevision>) {
    revision.request();
}

/// Every producer, including tests and runtime-created wheel edges, enters the
/// same pending state. There is no direct active-wire construction path.
pub fn on_add_connection(
    trigger: On<Add, SimConnection>,
    mut commands: Commands,
    mut revision: ResMut<BindingRevision>,
) {
    commands
        .entity(trigger.entity)
        .try_insert(ConnectionBinding::Pending);
    commands
        .entity(trigger.entity)
        .try_remove::<BoundConnection>();
    revision.request();
    // The connection can arrive from Update or PhysicsSchedule. Queue the
    // transaction on that producer's own command boundary instead of relying
    // on a later Update (which a deterministic fixed-step run need not make).
    commands.queue(bind_connections);
}

/// Bind only after a lifecycle transition requested a new revision.  Missing
/// endpoints remain pending while the epoch is open; after sealing, an exact
/// named-port miss is terminal and recorded once here, never in propagation.
pub fn bind_connections(world: &mut World) {
    let should_bind = {
        let mut revision = world.resource_mut::<BindingRevision>();
        revision.take_request()
    };
    if !should_bind {
        return;
    }
    let sealed = world.resource::<BindingRevision>().sealed;
    let registry = world.resource::<PortRegistry>().clone();
    let specs: Vec<(Entity, SimConnection)> = world
        .query::<(Entity, &SimConnection)>()
        .iter(world)
        .map(|(e, spec)| (e, spec.clone()))
        .collect();
    for (edge, spec) in specs {
        let src = world.get::<EndpointLifecycle>(spec.start_element);
        let dst = world.get::<EndpointLifecycle>(spec.end_element);
        let endpoints_ready = matches!(src, Some(EndpointLifecycle::Ready))
            && matches!(dst, Some(EndpointLifecycle::Ready));
        let endpoints_failed = matches!(src, Some(EndpointLifecycle::Failed(_)))
            || matches!(dst, Some(EndpointLifecycle::Failed(_)));
        if !endpoints_ready && !endpoints_failed && !sealed {
            continue;
        }
        // `resolve_*` answers whether a backend offers a cached slot.  Map
        // backends (Modelica) deliberately have no slot, so `None` there means
        // "use the canonical name path", not "the port is absent". Binding is
        // validating authored names, not compiling the propagation fast path.
        let source_ok = if spec.start_is_input {
            registry
                .read_input_port(world, spec.start_element, &spec.start_connector)
                .is_some()
        } else {
            registry
                .read_output_port(world, spec.start_element, &spec.start_connector)
                .is_some()
        };
        let target_ok = registry
            .read_input_port(world, spec.end_element, &spec.end_connector)
            .is_some();
        if endpoints_ready && source_ok && target_ok {
            world
                .entity_mut(edge)
                .insert((ConnectionBinding::Bound, BoundConnection));
            continue;
        }
        if !sealed && !endpoints_failed {
            continue;
        }
        let (entity, port) = if !target_ok {
            (spec.end_element, spec.end_connector.clone())
        } else {
            (spec.start_element, spec.start_connector.clone())
        };
        let failure = BrokenConnection {
            entity,
            global_id: world.get::<lunco_core::GlobalEntityId>(entity).copied(),
            port: port.clone(),
            has_port_surface: !registry.entity_ports(world, entity).is_empty(),
            dropped_value: 0.0,
        };
        let inserted = world
            .resource_mut::<CosimDiagnostics>()
            .faults
            .insert((entity, port.clone()), failure)
            .is_none();
        if inserted {
            let authored_edge = world
                .get::<Name>(edge)
                .map(Name::as_str)
                .unwrap_or("runtime-created connection");
            warn!(
                "[cosim] connection binding failed: {authored_edge}; endpoint {:?} has no required port '{}'",
                entity, port,
            );
        }
        world
            .entity_mut(edge)
            .insert(ConnectionBinding::Failed)
            .remove::<BoundConnection>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    use lunco_core::InputPorts;

    fn world_with_ports(target_port: &str) -> (World, Entity, Entity) {
        let mut world = World::new();
        world.init_resource::<PortRegistry>();
        world.init_resource::<BindingRevision>();
        world.init_resource::<CosimDiagnostics>();
        let source = world
            .spawn((InputPorts::new(&["source"]), EndpointLifecycle::Ready))
            .id();
        let target = world
            .spawn((InputPorts::new(&[target_port]), EndpointLifecycle::Ready))
            .id();
        (world, source, target)
    }

    #[test]
    fn pending_connection_binds_only_after_both_endpoints_are_ready() {
        let (mut world, source, target) = world_with_ports("target");
        let edge = world
            .spawn(SimConnection {
                start_element: source,
                start_connector: "source".into(),
                start_is_input: true,
                end_element: target,
                end_connector: "target".into(),
                scale: 1.0,
                offset: 0.0,
            })
            .id();
        world.resource_mut::<BindingRevision>().request();
        world.run_system_once(bind_connections).unwrap();
        assert!(world.get::<BoundConnection>(edge).is_some());
    }

    #[test]
    fn terminal_port_miss_faults_once_at_binding_not_propagation() {
        let (mut world, source, target) = world_with_ports("other");
        let edge = world
            .spawn(SimConnection {
                start_element: source,
                start_connector: "source".into(),
                start_is_input: true,
                end_element: target,
                end_connector: "target".into(),
                scale: 1.0,
                offset: 0.0,
            })
            .id();
        world.resource_mut::<BindingRevision>().seal_epoch();
        world.run_system_once(bind_connections).unwrap();
        assert_eq!(
            world.get::<ConnectionBinding>(edge),
            Some(&ConnectionBinding::Failed)
        );
        assert_eq!(world.resource::<CosimDiagnostics>().faults.len(), 1);
        world.resource_mut::<BindingRevision>().request();
        world.run_system_once(bind_connections).unwrap();
        assert_eq!(world.resource::<CosimDiagnostics>().faults.len(), 1);
    }
}
