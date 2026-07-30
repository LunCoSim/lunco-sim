//! Reactive connection binding.
//!
//! [`SimConnection`](crate::SimConnection) is authored topology.  It becomes an
//! executable edge only after both named ports resolve and any asynchronous
//! endpoint lifecycle is terminal. This keeps loading order out of the fixed-step
//! master without making synchronous hardware ports pretend to be async.

use bevy::prelude::*;
use lunco_core::ports::PortRegistry;

use crate::{diagnostics::BrokenConnection, CosimDiagnostics, SimConnection, SimStatus};

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

/// Cheap run condition for the end-of-frame binding transaction.
pub fn binding_requested(revision: Res<BindingRevision>) -> bool {
    revision.pending()
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
        // Modelica status is read directly as well as through the mirrored
        // lifecycle component. The component is useful to other consumers, but
        // it is written by a deferred observer/update path; consulting the
        // authoritative solver status closes the one-frame window on first load
        // before that mirror has been installed.
        let model_status = |entity| world.get::<crate::SimComponent>(entity).map(|m| &m.status);
        let endpoints_failed = matches!(src, Some(EndpointLifecycle::Failed(_)))
            || matches!(dst, Some(EndpointLifecycle::Failed(_)))
            || matches!(model_status(spec.start_element), Some(SimStatus::Error(_)))
            || matches!(model_status(spec.end_element), Some(SimStatus::Error(_)));
        let endpoints_pending = matches!(src, Some(EndpointLifecycle::Pending))
            || matches!(dst, Some(EndpointLifecycle::Pending))
            || matches!(model_status(spec.start_element), Some(SimStatus::Compiling))
            || matches!(model_status(spec.end_element), Some(SimStatus::Compiling));
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
        // `EndpointLifecycle` is an opt-in wait for asynchronous participants
        // (Modelica assets, deferred USD prims). It is deliberately NOT a
        // prerequisite for every port owner: a hardware `Port` is created
        // synchronously with its backing component and has no separate lifecycle
        // marker. Requiring `Ready` here turned a perfectly real actuator-bank
        // port into a terminal "missing value" fault once the scene epoch sealed.
        //
        // The port registry is the authoritative endpoint contract.  Once both
        // named sides resolve and neither endpoint has explicitly failed, bind
        // immediately; unresolved async endpoints still remain pending while the
        // epoch is open below.
        if !endpoints_failed && !endpoints_pending && source_ok && target_ok {
            world
                .entity_mut(edge)
                .insert((ConnectionBinding::Bound, BoundConnection));
            continue;
        }
        // A declared async participant remains pending even when the current USD
        // spawn epoch is otherwise sealed: its interface may have arrived before
        // its solver/model is terminal.  A failed participant below is the only
        // terminal async outcome; a timeout belongs to the readiness policy, not
        // a fabricated missing-port diagnostic.
        if !endpoints_failed && endpoints_pending {
            world
                .entity_mut(edge)
                .insert(ConnectionBinding::Pending)
                .remove::<BoundConnection>();
            continue;
        }
        // A synchronous endpoint with no matching port can still arrive later
        // while USD is spawning. Only turn that into a durable authoring fault
        // after the projection epoch has closed.
        if !endpoints_failed && !sealed {
            world
                .entity_mut(edge)
                .insert(ConnectionBinding::Pending)
                .remove::<BoundConnection>();
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

    #[test]
    fn synchronous_hardware_port_binds_without_lifecycle_marker() {
        let mut world = World::new();
        world.init_resource::<PortRegistry>();
        world.init_resource::<BindingRevision>();
        world.init_resource::<CosimDiagnostics>();
        {
            let mut registry = world.resource_mut::<PortRegistry>();
            crate::ports::register_builtin_port_backends(&mut registry);
        }
        let source = world.spawn(lunco_core::Port { value: 0.75 }).id();
        let target = world.spawn(InputPorts::new(&["drive_left"])).id();
        let edge = world
            .spawn(SimConnection {
                start_element: source,
                start_connector: crate::ports::PORT_NAME.into(),
                start_is_input: false,
                end_element: target,
                end_connector: "drive_left".into(),
                scale: 1.0,
                offset: 0.0,
            })
            .id();

        world.resource_mut::<BindingRevision>().seal_epoch();
        world.run_system_once(bind_connections).unwrap();

        assert!(world.get::<BoundConnection>(edge).is_some());
        assert!(world.resource::<CosimDiagnostics>().faults.is_empty());
    }

    #[test]
    fn async_endpoint_waits_for_terminal_lifecycle_even_when_port_is_declared() {
        let (mut world, source, target) = world_with_ports("target");
        world.entity_mut(source).insert(EndpointLifecycle::Pending);
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
        assert!(world.get::<BoundConnection>(edge).is_none());
        assert!(world.resource::<CosimDiagnostics>().faults.is_empty());

        world.entity_mut(source).insert(EndpointLifecycle::Ready);
        world.resource_mut::<BindingRevision>().request();
        world.run_system_once(bind_connections).unwrap();
        assert!(world.get::<BoundConnection>(edge).is_some());

        world.entity_mut(source).insert(EndpointLifecycle::Pending);
        world.resource_mut::<BindingRevision>().request();
        world.run_system_once(bind_connections).unwrap();
        assert!(world.get::<BoundConnection>(edge).is_none());
        assert_eq!(
            world.get::<ConnectionBinding>(edge),
            Some(&ConnectionBinding::Pending)
        );
    }
}
