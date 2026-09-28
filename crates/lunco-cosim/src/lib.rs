//! # LunCoSim Co-Simulation Orchestration
//!
//! Connects multiple simulation models (Modelica, FMU, GMAT, Avian) via explicit wires.
//! Every engine is treated as a model with named inputs and outputs.
//!
//! ## Architecture
//!
//! Every simulation engine is just a model with named inputs and outputs:
//!
//! | Model       | Inputs                      | Outputs                          |
//! |-------------|-----------------------------|----------------------------------|
//! | **AvianSim**   | `force_y`, `force_x`        | `position_y`, `velocity_y`, ... |
//! | **SimComponent** (Modelica) | `height`, `velocity`, `g` | `netForce`, `volume`, ... |
//! | **SimComponent** (FMU)     | `current_in`            | `soc`, `voltage`, ...         |
//!
//! [`lunco_cosim_core::SimConnection`] connects any output to any input, following the FMI/SSP pattern.
//!
//! ## Example
//!
//! ```rust,ignore
//! // Wire: Modelica netForce → Avian force_y
//! commands.spawn(SimConnection {
//!     start_element: balloon_entity,
//!     start_connector: "netForce".into(),
//!     end_element: balloon_entity,
//!     end_connector: "force_y".into(),
//!     scale: 1.0,
//! });
//!
//! // Wire: Avian height → Modelica height input
//! commands.spawn(SimConnection {
//!     start_element: balloon_entity,
//!     start_connector: "position_y".into(),
//!     end_element: balloon_entity,
//!     end_connector: "height".into(),
//!     scale: 1.0,
//! });
//! ```

use std::sync::Arc;

use bevy::prelude::*;

pub mod avian;
pub mod avian_queries;
pub mod binding;
pub mod joint;
pub mod ports;
pub mod systems;

pub use avian::*;
pub use binding::*;
pub use joint::*;
pub use ports::*;

use lunco_api::executor::{DeferredCommandAppExt, PendingApiRequest, finish_command_result};
use lunco_core_session::ControlAuthorityChanged;
use lunco_cosim_core::{
    BindingRevision, BrokenConnection, ControlWriteFence, CosimDiagnostics, ForceActuator,
    PortHolds, RealtimeSafe, SimComponent, SimConnection, SimStatus, TorqueActuator,
};

// Typed-command machinery: command contracts are separate from the Bevy-backed
// runtime core, while the `Command` macro/reflection path remains in core. Used by the `SetPorts` command +
// observer defined below — the ONE generic vessel-control command (a batch of
// named input-port writes), driving landers, rovers, and any port-bearing vessel.
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{
    ActiveCommandId, CommandOrigin, GlobalEntityId, RuntimeClock, on_command, register_commands,
};
use lunco_cosim_core::commands::{ControlSafeStop, ReleaseControl, ReleasePort, SetPorts};

fn endpoint_ready_on_add<T: Component>(
    trigger: On<Add, T>,
    mut commands: Commands,
    mut revision: ResMut<BindingRevision>,
) {
    commands.entity(trigger.entity).try_insert((
        EndpointLifecycle::Ready,
        // This is the shared admission fact used by USD wiring: a prim is
        // eligible for endpoint resolution only after its owning backend
        // has installed its named surface. It is deliberately published
        // alongside the lifecycle state for every backend, not inferred
        // from a vehicle- or sensor-specific component.
        lunco_port_core::PortSurfaceReady,
    ));
    revision.request();
}

fn endpoint_pending_on_add<T: Component>(
    trigger: On<Add, T>,
    mut commands: Commands,
    mut revision: ResMut<BindingRevision>,
) {
    commands
        .entity(trigger.entity)
        .try_insert(EndpointLifecycle::Pending);
    revision.request();
}

fn mark_causal_state_sink<T: Component>(trigger: On<Add, T>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .try_insert(lunco_port_core::CausalStateSink);
}

fn mark_joint_torque_port(
    trigger: On<Add, lunco_physics::joint::JointTorqueActuator>,
    query: Query<&lunco_physics::joint::JointTorqueActuator>,
    mut commands: Commands,
) {
    let Ok(actuator) = query.get(trigger.entity) else {
        return;
    };
    if actuator.port_entity != Entity::PLACEHOLDER {
        commands
            .entity(actuator.port_entity)
            .try_insert(lunco_port_core::CausalStateSink);
    }
}

/// Publish Modelica endpoint transitions before the end-of-frame binding
/// transaction. This is exclusive so a terminal compiler result is visible in
/// the same frame; binding itself runs in `PostUpdate`, after every USD, asset,
/// and generated-domain projection path has had a chance to publish its ports.
fn sync_model_endpoint_lifecycle(world: &mut World) {
    let transitions: Vec<(Entity, EndpointLifecycle)> = world
        .query_filtered::<(Entity, &SimComponent), Changed<SimComponent>>()
        .iter(world)
        .map(|(entity, component)| {
            let state = match &component.status {
                SimStatus::Compiling => EndpointLifecycle::Pending,
                SimStatus::Error(message) => EndpointLifecycle::Failed(message.clone()),
                _ => EndpointLifecycle::Ready,
            };
            (entity, state)
        })
        .collect();

    let mut changed = false;
    for (entity, state) in transitions {
        if world.get::<EndpointLifecycle>(entity) != Some(&state) {
            world.entity_mut(entity).insert(state);
            changed = true;
        }
    }
    if changed {
        world.resource_mut::<BindingRevision>().request();
    }
}

/// Plugin for co-simulation orchestration.
///
/// Registers [`lunco_cosim_core::SimComponent`], [`crate::AvianSim`], and
/// [`lunco_cosim_core::SimConnection`] types,
/// and adds systems for wire propagation and Avian manual stepping.
///
/// ## Usage
///
/// ```rust,ignore
/// app.add_plugins(CoSimPlugin);
/// ```
///
/// Engine plugins (e.g., `lunco-modelica-core`) depend on this crate and
/// create [`lunco_cosim_core::SimComponent`] instances when models compile.
pub struct CoSimPlugin;

/// Clear co-simulation state owned by the outgoing scene before its entities
/// are reclaimed. Port holds, deferred control fences, binding epochs, and
/// diagnostics all contain entity-scoped state and must not cross a scene
/// replacement boundary.
fn reset_scene_state(
    mut diagnostics: ResMut<CosimDiagnostics>,
    mut holds: ResMut<PortHolds>,
    mut fence: ResMut<ControlWriteFence>,
    mut revision: ResMut<BindingRevision>,
) {
    *diagnostics = CosimDiagnostics::default();
    holds.clear_all();
    *fence = ControlWriteFence::default();
    *revision = BindingRevision::default();
}

impl Plugin for CoSimPlugin {
    fn build(&self, app: &mut App) {
        app.register_deferred_command::<SetPorts>();
        app.register_deferred_command::<ReleasePort>();
        app.register_deferred_command::<ReleaseControl>();
        app.init_resource::<lunco_core_session::CommandPolicyRegistry>();
        app.world_mut()
            .resource_mut::<lunco_core_session::CommandPolicyRegistry>()
            .register(
                "ReleasePort",
                lunco_core_session::CommandPolicy::OWNED_CONTROL,
            );
        app.world_mut()
            .resource_mut::<lunco_core_session::CommandPolicyRegistry>()
            .register(
                "ReleaseControl",
                lunco_core_session::CommandPolicy::OWNED_CONTROL,
            );
        app.register_type::<SimComponent>()
            .register_type::<PendingForces>()
            .register_type::<ForceActuator>()
            .register_type::<TorqueActuator>()
            .register_type::<lunco_physics::joint::JointTorqueActuator>()
            .register_type::<PendingActuatorCommand>()
            .register_type::<SimConnection>()
            .register_type::<RealtimeSafe>()
            .register_type::<lunco_physics::raycast::RaycastObservation>();

        lunco_port_core::register_endpoint_types(app);

        // The shared port substrate (in `lunco-port-core`, below every participant).
        // The cosim engine owns the avian/joint/Modelica/hardware backends and
        // registers them here; wires, the API, the inspector, and scripts all
        // read/write through this one registry. Registration order = resolution
        // precedence (Modelica, avian, then single-value hardware ports).
        app.init_resource::<lunco_port_core::ports::PortRegistry>()
            .init_resource::<lunco_port_core::ports::PortTopologyRevision>()
            .init_resource::<lunco_port_core::ports::PortTopologyState>()
            .init_resource::<BindingRevision>();
        // Machine-readable dangling-wire report, refreshed each propagation tick
        // and surfaced via the API's `GET /api/diagnostics` (`GetBrokenConnections`).
        app.init_resource::<CosimDiagnostics>();
        // One compiled wiring/read path is shared by the normal fixed step and
        // rollback replay. The schedules invoke the same transaction, while
        // this resource keeps their change detector and dense endpoint table
        // singular.
        app.init_resource::<systems::propagate::PropagationCache>();
        // Manual control intents that outrank the wiring fabric until an
        // explicit release — without it, a `SetPorts` write on a WIRED input
        // lives less than one tick.
        app.init_resource::<PortHolds>();
        // A lifecycle command may retire a producer after its SetPorts trigger
        // was emitted but before its deferred write lands. Keep that stale write
        // outside the shared control boundary until next tick.
        app.init_resource::<ControlWriteFence>();
        app.add_systems(FixedFirst, lunco_cosim_core::clear_control_write_fence);
        app.add_systems(lunco_core::SceneTeardown, reset_scene_state);
        app.add_observer(binding::on_add_connection)
            .add_observer(binding::on_port_surface_change)
            .add_observer(binding::on_remove_port_surface)
            // Co-sim retains every `SimComponent` output itself, with source
            // metadata. Mark it at lifecycle time so generic port telemetry does
            // not create a second, ungrouped history for the same values.
            .add_observer(endpoint_ready_on_add::<lunco_port_core::InputPorts>)
            .add_observer(endpoint_ready_on_add::<lunco_port_core::Port>)
            .add_observer(endpoint_ready_on_add::<lunco_port_core::PortSurfaceReady>)
            .add_observer(endpoint_pending_on_add::<lunco_port_core::PortSurfacePending>)
            .add_observer(endpoint_ready_on_add::<avian3d::prelude::RigidBody>)
            // Force and torque actuators are native scalar port endpoints too.
            // USD projects them after the authored wiring pass has started and
            // they are authored without a SimComponent, so admission must
            // publish the same lifecycle fact as rigid bodies and joints before
            // connection derivation can admit their wires.
            .add_observer(endpoint_ready_on_add::<ForceActuator>)
            .add_observer(endpoint_ready_on_add::<TorqueActuator>)
            .add_observer(endpoint_ready_on_add::<lunco_physics::raycast::RaycastObservation>)
            .add_observer(on_commit_session_input_ports)
            .add_observer(endpoint_ready_on_add::<avian3d::prelude::RevoluteJoint>)
            .add_observer(endpoint_ready_on_add::<avian3d::prelude::PrismaticJoint>)
            .add_observer(mark_causal_state_sink::<avian3d::prelude::RigidBody>)
            .add_observer(mark_causal_state_sink::<avian3d::prelude::RevoluteJoint>)
            .add_observer(mark_causal_state_sink::<avian3d::prelude::PrismaticJoint>)
            .add_observer(mark_causal_state_sink::<ForceActuator>)
            .add_observer(mark_causal_state_sink::<TorqueActuator>)
            .add_observer(mark_joint_torque_port)
            .add_observer(on_control_authority_changed)
            .add_observer(on_control_safe_stop);
        // Every built-in port owner installs its lifecycle hooks in the backend
        // module. Avian groups additionally carry their hooks beside their
        // predicates and port definitions, so adding a group cannot silently
        // omit invalidation.
        ports::register_builtin_port_topology(app);
        app.add_systems(
            Update,
            sync_model_endpoint_lifecycle
                .run_if(|q: Query<(), Changed<SimComponent>>| !q.is_empty()),
        );
        // Map-backed port owners can change their declared names in place while
        // also updating live values. Compare their identity keys after all
        // frame/physics writers have run; this is a structural check, not a
        // 10 Hz candidate poll. Connection membership is handled by lifecycle
        // observers and endpoint rewrites by the companion structural check.
        app.add_systems(
            PostUpdate,
            (
                ports::check_port_owner_structure,
                ports::check_connection_structure,
            ),
        );
        // One authoritative binding boundary per frame. Observers and async
        // projections only request a reconciliation; running it after `Update`
        // means a first-load connection cannot be sealed between a deferred USD
        // port spawn and its generated Modelica contract.
        app.add_systems(
            PostUpdate,
            binding::bind_connections.run_if(binding::binding_requested),
        );
        {
            let mut registry = app
                .world_mut()
                .resource_mut::<lunco_port_core::ports::PortRegistry>();
            ports::register_builtin_port_backends(&mut registry);
        }

        // Avian state ports are detected by component presence through the
        // `AVIAN` spec table. Actuator components additionally publish the
        // generic endpoint lifecycle above because USD wiring can be derived
        // before their deferred projection has installed the component.

        // CoSim runs in FixedUpdate (before Avian's FixedPostUpdate physics step).
        // Order: propagate wires first, then apply forces to Position.
        // Avian's own PhysicsSchedule runs in FixedPostUpdate — we do NOT step it
        // manually here to avoid double-stepping.
        app.configure_sets(
            FixedUpdate,
            (
                lunco_cosim_core::schedule::CosimSet::Propagate
                    .in_set(lunco_core::RuntimeCycleSet::Simulation),
                lunco_cosim_core::schedule::CosimApplySet::ApplyForces
                    .in_set(lunco_core::RuntimeCycleSet::Simulation),
            )
                .chain(),
        );

        // `CosimSet::Propagate` IS the control DAC. Nesting it inside
        // `lunco_core_runtime::ControlDacSet` is what gives that anchor its meaning:
        // every actuator that reads a `Port` orders `.after(ControlDacSet)`
        // (lunco-controller, lunco-hardware, lunco-mobility) and
        // those edges must resolve against the system that actually writes the
        // port — this one. A sibling `.before()` relationship would instead leave
        // the anchor empty and every such ordering a silent no-op, letting the
        // actuation slip a whole tick frame-to-frame and diverge host vs client
        // under prediction.
        app.configure_sets(
            FixedUpdate,
            lunco_cosim_core::schedule::CosimSet::Propagate
                .in_set(lunco_core_runtime::ControlDacSet),
        );

        // Rollback replay re-simulates the owned rover's unacked inputs by running
        // `RollbackReplay` + `PhysicsSchedule` per replayed input. Propagation is
        // part of the actuation chain that schedule mirrors: without it the
        // replayed actuators read port values nobody re-derived for the replayed
        // tick, so the replay's forces differ from the host's and prediction
        // diverges on exactly the body rollback exists to keep in sync. Same
        // nesting as `FixedUpdate` so the `.after(ControlDacSet)` mirrors in
        // lunco-hardware / lunco-mobility keep their relative order.
        app.configure_sets(
            lunco_core_runtime::RollbackReplay,
            lunco_cosim_core::schedule::CosimSet::Propagate
                .in_set(lunco_core_runtime::ControlDacSet),
        );
        app.add_systems(
            lunco_core_runtime::RollbackReplay,
            systems::propagate::propagate_connections
                .in_set(lunco_cosim_core::schedule::CosimSet::Propagate),
        );
        app.add_systems(
            lunco_core_runtime::RollbackReplay,
            avian::apply_joint_torque_actuators
                .after(lunco_core_runtime::ControlDacSet)
                .run_if(resource_exists::<Time<avian3d::prelude::Physics>>),
        );

        app.add_systems(
            FixedUpdate,
            (
                systems::propagate::propagate_connections
                    .in_set(lunco_cosim_core::schedule::CosimSet::Propagate)
                    .run_if(lunco_time::simulation_is_running),
                // The avian boundary consumers: apply solved joint torques and
                // drain net force/torque ports plus USD-authored point-force
                // mounts into Avian's `Forces`.
                // Additionally gated on `physics_is_live`: this is the one system
                // here that writes into avian's FORCE ACCUMULATOR, which only the
                // physics step clears. A physics hold (a frozen cinematic beat)
                // leaves `FixedUpdate` running by design, so ungated this kept
                // draining thruster force AND TORQUE into the accumulator with
                // nothing consuming it, then discharged the whole integral on the
                // single step that released the hold. Torque, unlike gravity,
                // accumulates about the COM and so discharges as SPIN — the measured
                // ~25 rad/s transient on episode 1's lander/rover stack. The
                // `propagate_connections` above is deliberately not gated on
                // `Time<Physics>`: it moves VALUES around the cosim graph rather
                // than accumulating one, a physics-held beat still wants a live
                // graph, and its network gating is
                // PER TARGET (`peer_simulates`) rather than per process — a client
                // must keep propagating into the bodies it locally predicts, or the
                // predicted rover's command never reaches its actuators.
                //
                // The role gate rides the force accumulator alone: a pure client
                // renders host snapshots for replicated bodies, and adding
                // locally-derived forces to them fights the snapshot stream.
                avian::apply_joint_torque_actuators
                    .in_set(lunco_cosim_core::schedule::CosimApplySet::ApplyForces)
                    .before(avian::apply_pending_forces)
                    .run_if(resource_exists::<Time<avian3d::prelude::Physics>>)
                    .run_if(|role: Option<Res<lunco_core_session::NetworkRole>>| {
                        !matches!(
                            role.as_deref(),
                            Some(lunco_core_session::NetworkRole::Client)
                        )
                    }),
                avian::apply_pending_forces
                    .in_set(lunco_cosim_core::schedule::CosimApplySet::ApplyForces)
                    // `resource_exists` FIRST, for the same reason the sensors
                    // below carry it: `physics_is_live` reads `Res<Time<Physics>>`
                    // unconditionally, so without avian the run condition itself
                    // hard-errors instead of gating. Headless cosim with no avian
                    // then skips force application, which is the intent.
                    // Run while a physics clock exists, including during a
                    // hold: `apply_pending_forces` drains commands without
                    // applying them while the clock is stopped, so a command
                    // sampled during loading cannot become a later impulse.
                    .run_if(resource_exists::<Time<avian3d::prelude::Physics>>)
                    .run_if(|role: Option<Res<lunco_core_session::NetworkRole>>| {
                        // Absent role (single-player, headless tests) → run.
                        // Only a present `Client` role gates it off.
                        !matches!(
                            role.as_deref(),
                            Some(lunco_core_session::NetworkRole::Client)
                        )
                    }),
            ),
        );

        // Avian outputs (position/velocity, attitude, angular velocity, and
        // contacts) are read on demand through the resolver. They are native
        // solver facts, so no semantic sensor snapshot is maintained.
        //
        // A raw ray is the one exception: the query API must run after Avian
        // writeback. It is sampled in FixedPostUpdate and consumed by the next
        // co-simulation propagation tick. The physics resource gate keeps
        // headless cosim without Avian safe.
        app.add_systems(
            FixedPostUpdate,
            (
                lunco_physics::raycast::sample_raycast_observations,
                avian::sample_solved_acceleration,
            )
                .run_if(resource_exists::<Time<avian3d::prelude::Physics>>)
                .after(avian3d::prelude::PhysicsSystems::Writeback),
        );

        // The authored ray BEAM is drawn by `lunco-render-bevy`'s `sensor_beams`,
        // not here: naming `Gizmos`/`GizmoConfigStore` dragged
        // `bevy_gizmos → bevy_render → wgpu + naga` into every build, including the
        // `--no-ui` server and the wasm worker. The raw Avian query is simulation,
        // must run headless, and stays here
        // — the render layer reads its stored result and re-casts nothing.
        // See `docs/architecture/render-decoupling.md`.

        // The acceleration sample is a component-backed sensor fact. Ensure it
        // exists before the USD fabric resolves the body's output surface;
        // sampling itself remains after the solver writeback above.
        app.add_systems(
            Update,
            (
                systems::collider::sync_collider,
                avian::ensure_acceleration_samples,
            ),
        );

        // A model's own variables — the INTERNAL Modelica state included — become
        // retained, plottable history without anyone authoring a channel per
        // variable. Runs AFTER propagation so a sample is the post-step value the
        // rest of the frame sees, never the previous tick's. See `telemetry.rs`
        // for the namespace that keeps it out of authored channels' buffers and
        // for the rate/retention/memory arithmetic.
        // Register the typed command observers generated below (the
        // `register_commands!` list turns into `register_all_commands(app)`).
        register_all_commands(app);
    }
}

#[cfg(test)]
mod binding_lifecycle_tests {
    use super::*;
    use avian3d::prelude::RevoluteJoint;
    use lunco_cosim_core::{BoundConnection, ConnectionBinding};
    use lunco_port_core::ports::PortDirection;

    #[test]
    fn port_topology_revision_tracks_owner_lifecycle_not_live_values() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);

        let before_add = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        let entity = app
            .world_mut()
            .spawn(lunco_port_core::InputPorts::new(&["throttle"]))
            .id();
        let after_add = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        assert_ne!(after_add, before_add);
        app.update();
        let after_initial_check = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        assert_eq!(after_initial_check, after_add);

        let registry = app
            .world()
            .resource::<lunco_port_core::ports::PortRegistry>()
            .clone();
        assert!(registry.write_port(app.world_mut(), entity, "throttle", 0.5));
        app.update();
        assert_eq!(
            app.world()
                .resource::<lunco_port_core::ports::PortTopologyRevision>()
                .0,
            after_initial_check,
            "live port writes must not invalidate the declared surface"
        );

        app.world_mut()
            .get_mut::<lunco_port_core::InputPorts>(entity)
            .unwrap()
            .values
            .insert("arm".into(), 0.0);
        app.update();
        let after_shape_change = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        assert_ne!(after_shape_change, after_initial_check);

        app.world_mut().despawn(entity);
        let after_remove = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        assert_ne!(after_remove, after_shape_change);
    }

    #[test]
    fn port_name_set_key_tracks_sim_component_topology_without_live_samples() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);

        let entity = app.world_mut().spawn(SimComponent::default()).id();
        app.update();
        let after_admission = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;

        app.world_mut()
            .get_mut::<SimComponent>(entity)
            .unwrap()
            .outputs
            .insert("thrust".into(), 1.0);
        app.update();
        let after_port_added = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        assert_ne!(after_port_added, after_admission);

        app.world_mut()
            .get_mut::<SimComponent>(entity)
            .unwrap()
            .outputs
            .insert("thrust".into(), 2.0);
        app.update();
        assert_eq!(
            app.world()
                .resource::<lunco_port_core::ports::PortTopologyRevision>()
                .0,
            after_port_added,
            "a changed scalar sample must not invalidate the port table"
        );

        app.world_mut()
            .get_mut::<SimComponent>(entity)
            .unwrap()
            .outputs
            .remove("thrust");
        app.update();
        assert_ne!(
            app.world()
                .resource::<lunco_port_core::ports::PortTopologyRevision>()
                .0,
            after_port_added,
            "removing a declared port must invalidate the port table"
        );
    }

    #[test]
    fn port_topology_revision_tracks_value_to_membership_mobility_changes() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);

        let entity = app
            .world_mut()
            .spawn((
                lunco_core::Mobility::Dynamic,
                avian3d::prelude::Position::default(),
            ))
            .id();
        app.update();
        let before_transition = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        let registry = app
            .world()
            .resource::<lunco_port_core::ports::PortRegistry>()
            .clone();
        assert!(
            registry
                .entity_ports(app.world(), entity)
                .iter()
                .all(|port| port.name != "position_x" || port.direction != PortDirection::In)
        );

        *app.world_mut()
            .get_mut::<lunco_core::Mobility>(entity)
            .unwrap() = lunco_core::Mobility::Kinematic;
        app.update();

        let after_transition = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        assert_ne!(after_transition, before_transition);
        assert!(
            registry
                .entity_ports(app.world(), entity)
                .iter()
                .any(|port| { port.name == "position_x" && port.direction == PortDirection::In })
        );
    }

    #[test]
    fn avian_topology_key_tracks_optional_backing_components_without_sampling_values() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);

        let entity = app
            .world_mut()
            .spawn((
                avian3d::prelude::RigidBody::Dynamic,
                avian3d::prelude::Position::default(),
            ))
            .id();
        app.update();
        let registry = app
            .world()
            .resource::<lunco_port_core::ports::PortRegistry>()
            .clone();
        let before_remove = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        let initial_key = registry.entity_port_topology_key(app.world(), entity);
        assert!(
            registry
                .entity_ports(app.world(), entity)
                .iter()
                .any(|port| port.name == "position_x")
        );

        app.world_mut()
            .entity_mut(entity)
            .remove::<avian3d::prelude::Position>();
        app.update();
        let after_remove = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        assert_ne!(after_remove, before_remove);
        assert_ne!(
            registry.entity_port_topology_key(app.world(), entity),
            initial_key,
            "a backing-component removal must change the cached candidate key"
        );
        assert!(
            !registry
                .entity_ports(app.world(), entity)
                .iter()
                .any(|port| port.name == "position_x")
        );

        app.world_mut()
            .entity_mut(entity)
            .insert(avian3d::prelude::Position::default());
        app.update();
        let after_position = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        assert_ne!(after_position, after_remove);
        assert_eq!(
            registry.entity_port_topology_key(app.world(), entity),
            initial_key,
            "restoring the same structural surface must restore the same key"
        );
        assert!(
            registry
                .entity_ports(app.world(), entity)
                .iter()
                .any(|port| port.name == "position_x")
        );

        app.world_mut()
            .get_mut::<avian3d::prelude::Position>(entity)
            .unwrap()
            .0
            .x = 10.0;
        app.update();
        assert_eq!(
            app.world()
                .resource::<lunco_port_core::ports::PortTopologyRevision>()
                .0,
            after_position,
            "live Avian samples must not invalidate the declared surface"
        );

        app.world_mut()
            .entity_mut(entity)
            .remove::<avian3d::prelude::Position>();
        app.update();
        let after_second_remove = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        assert_ne!(after_second_remove, after_position);
        assert!(
            !registry
                .entity_ports(app.world(), entity)
                .iter()
                .any(|port| port.name == "position_x")
        );
    }

    #[test]
    fn port_topology_revision_tracks_connection_structure_not_affine_values() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);

        let source = app.world_mut().spawn_empty().id();
        let target = app.world_mut().spawn_empty().id();
        let before_add = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        let edge = app
            .world_mut()
            .spawn(SimConnection {
                start_element: source,
                start_connector: "output".into(),
                start_is_input: false,
                end_element: target,
                end_connector: "input".into(),
                scale: 1.0,
                offset: 0.0,
            })
            .id();
        let after_add = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        assert_ne!(after_add, before_add);
        app.update();
        let after_initial_check = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        assert_eq!(after_initial_check, after_add);

        app.world_mut()
            .get_mut::<SimConnection>(edge)
            .unwrap()
            .scale = 2.0;
        app.update();
        assert_eq!(
            app.world()
                .resource::<lunco_port_core::ports::PortTopologyRevision>()
                .0,
            after_initial_check,
            "changing an affine value must not invalidate connection topology"
        );

        app.world_mut()
            .get_mut::<SimConnection>(edge)
            .unwrap()
            .end_connector = "other_input".into();
        app.update();
        let after_rewire = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        assert_ne!(after_rewire, after_initial_check);

        app.world_mut().despawn(edge);
        let after_remove = app
            .world()
            .resource::<lunco_port_core::ports::PortTopologyRevision>()
            .0;
        assert_ne!(after_remove, after_rewire);
    }

    #[test]
    fn model_ready_transition_binds_waiting_edge_in_the_same_update() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);

        let mut source_component = SimComponent::default();
        source_component.inputs.insert("target_mount_x".into(), 0.0);
        let source = app.world_mut().spawn(source_component).id();

        let mut target_component = SimComponent {
            status: SimStatus::Compiling,
            ..Default::default()
        };
        target_component.inputs.insert("target_mount_x".into(), 0.0);
        let target = app.world_mut().spawn(target_component).id();

        let edge = app
            .world_mut()
            .spawn(SimConnection {
                start_element: source,
                start_connector: "target_mount_x".into(),
                start_is_input: true,
                end_element: target,
                end_connector: "target_mount_x".into(),
                scale: 1.0,
                offset: 0.0,
            })
            .id();

        app.update();
        assert!(app.world().get::<BoundConnection>(edge).is_none());

        app.world_mut()
            .get_mut::<SimComponent>(target)
            .unwrap()
            .status = SimStatus::Idle;
        app.update();

        assert!(app.world().get::<BoundConnection>(edge).is_some());
    }

    #[test]
    fn admitted_revolute_joint_binds_waiting_angle_edge() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);

        let mut controller = SimComponent::default();
        controller.outputs.insert("yaw".into(), 0.0);
        let controller = app.world_mut().spawn(controller).id();
        let body0 = app.world_mut().spawn_empty().id();
        let body1 = app.world_mut().spawn_empty().id();
        let hinge = app.world_mut().spawn(RevoluteJoint::new(body0, body1)).id();
        let edge = app
            .world_mut()
            .spawn(SimConnection {
                start_element: controller,
                start_connector: "yaw".into(),
                start_is_input: false,
                end_element: hinge,
                end_connector: "angle".into(),
                scale: 1.0,
                offset: 0.0,
            })
            .id();

        // The joint's Add observer publishes its lifecycle through Commands;
        // the subsequent Update is the reactive binding transaction, not a
        // fixed-step retry.
        app.update();
        app.update();

        assert!(app.world().get::<BoundConnection>(edge).is_some());
    }

    #[test]
    fn admitted_force_actuator_rebinds_waiting_command_edge() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);

        let mut controller = SimComponent::default();
        controller.outputs.insert("thrust".into(), 0.0);
        let controller = app.world_mut().spawn(controller).id();
        let actuator = app.world_mut().spawn_empty().id();
        let edge = app
            .world_mut()
            .spawn(SimConnection {
                start_element: controller,
                start_connector: "thrust".into(),
                start_is_input: false,
                end_element: actuator,
                end_connector: "force_command".into(),
                scale: 1.0,
                offset: 0.0,
            })
            .id();

        app.update();
        assert!(app.world().get::<BoundConnection>(edge).is_none());

        app.world_mut().entity_mut(actuator).insert(ForceActuator {
            local_position: Vec3::ZERO,
            direction_local: Vec3::Y,
            max_force_n: 100.0,
        });
        app.update();
        app.update();

        assert!(
            app.world()
                .get::<lunco_port_core::PortSurfaceReady>(actuator)
                .is_some()
        );
        assert!(app.world().get::<BoundConnection>(edge).is_some());
    }

    #[test]
    fn late_joint_admission_rebinds_an_edge_after_its_epoch_sealed() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);

        let mut controller = SimComponent::default();
        controller.outputs.insert("yaw".into(), 0.0);
        let controller = app.world_mut().spawn(controller).id();
        let hinge = app.world_mut().spawn_empty().id();
        let edge = app
            .world_mut()
            .spawn(SimConnection {
                start_element: controller,
                start_connector: "yaw".into(),
                start_is_input: false,
                end_element: hinge,
                end_connector: "angle".into(),
                scale: 1.0,
                offset: 0.0,
            })
            .id();

        app.update();
        app.world_mut()
            .resource_mut::<BindingRevision>()
            .seal_epoch();
        binding::bind_connections(app.world_mut());
        assert_eq!(
            app.world().get::<ConnectionBinding>(edge),
            Some(&ConnectionBinding::Failed)
        );

        let body0 = app.world_mut().spawn_empty().id();
        let body1 = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(hinge)
            .insert(RevoluteJoint::new(body0, body1));
        app.update();
        app.update();

        assert!(app.world().get::<BoundConnection>(edge).is_some());
    }

    #[test]
    fn scene_port_surface_ready_enters_the_generic_endpoint_lifecycle() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);

        let entity = app
            .world_mut()
            .spawn(lunco_port_core::PortSurfaceReady)
            .id();
        app.update();

        assert_eq!(
            app.world().get::<EndpointLifecycle>(entity),
            Some(&EndpointLifecycle::Ready)
        );
    }

    #[test]
    fn native_actuator_port_enters_the_generic_endpoint_lifecycle() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);

        let entity = app
            .world_mut()
            .spawn(ForceActuator {
                local_position: Vec3::ZERO,
                direction_local: Vec3::Y,
                max_force_n: 1.0,
            })
            .id();
        app.update();

        assert_eq!(
            app.world().get::<EndpointLifecycle>(entity),
            Some(&EndpointLifecycle::Ready)
        );
        assert!(
            app.world()
                .get::<lunco_port_core::PortSurfaceReady>(entity)
                .is_some()
        );
    }

    #[test]
    fn scene_teardown_clears_scene_owned_cosim_state() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);

        let entity = app.world_mut().spawn_empty().id();
        {
            let mut diagnostics = app.world_mut().resource_mut::<CosimDiagnostics>();
            let broken = BrokenConnection {
                entity,
                global_id: None,
                port: "drive_left".into(),
                has_port_surface: true,
                dropped_value: 1.0,
            };
            diagnostics.record_fault(broken.clone());
            diagnostics.mark_landed(entity, "drive_right");
            diagnostics.pending.push(broken.clone());
            diagnostics.broken.push(broken);
            diagnostics.report_once("target:entity:0:drive_left");
        }
        app.world_mut()
            .resource_mut::<PortHolds>()
            .hold(entity, "throttle", 0.5);
        app.world_mut()
            .resource_mut::<ControlWriteFence>()
            .block(entity);
        app.world_mut().resource_mut::<BindingRevision>().request();

        lunco_core::run_scene_teardown(app.world_mut());

        let diagnostics = app.world().resource::<CosimDiagnostics>();
        assert!(diagnostics.pending.is_empty());
        assert!(diagnostics.broken.is_empty());
        assert!(diagnostics.faults.is_empty());
        assert!(diagnostics.landed.is_empty());
        assert!(
            app.world_mut()
                .resource_mut::<CosimDiagnostics>()
                .report_once("target:entity:0:drive_left")
        );
        assert!(app.world().resource::<PortHolds>().is_empty());
        assert!(!app.world().resource::<ControlWriteFence>().blocks(entity));
        assert!(!app.world().resource::<BindingRevision>().pending());
    }
}

/// Observer for [`SetPorts`]: external live inputs are validated and admitted
/// through the session queue; simulation-clock Rhai and direct triggers without
/// a producer identity retain their existing immediate path. Both paths apply named
/// values through the [`PortRegistry`], the single dispatch for Modelica
/// `SimComponent` inputs, `InputPorts`, hardware `Port`s, and future backends.
/// `write_port` needs `&mut World`, so the observer clones the cheap registry
/// and defers mutation through a `Commands` world closure. External results
/// acknowledge admission; the fixed-tick owner applies the value later.
///
/// On control-path latency ("input at tick N → wheels at tick N"), two halves:
///
/// 1. **Producer ordering (not in this crate) — DECLARED.** `drive_from_bindings`
///    (`lunco-controller`) and other generic input producers register
///    with an explicit `.before(lunco_core_runtime::ControlDacSet)` edge, so the
///    `SetPorts` they emit is flushed — and the source `Port` written — before
///    propagation carries it across the `Wire` and the wheel systems read it.
///    Any NEW input-producer system must carry the same edge, or an unrelated
///    `.after()` anywhere in the fixed graph can silently move its actuation a
///    whole tick.
/// 2. **This write-through.** The observer cannot apply the writes itself:
///    `PortRegistry::write_port` takes `&mut World`, and an EXCLUSIVE system
///    cannot be an observer in Bevy (`bevy_ecs`'s own
///    `exclusive_system_cannot_be_observer` test asserts the panic), while
///    `DeferredWorld` gives no `&mut World`. Removing the second defer therefore
///    requires a `DeferredWorld`-shaped backend signature in
///    `lunco_port_core::ports` — a port-substrate change, out of scope here. Note the queued
///    closure is appended to the SAME command queue that is being flushed, so it
///    lands within that flush; the ordering risk is (1), not this hop.
#[on_command(SetPorts)]
fn on_set_ports(
    trigger: On<SetPorts>,
    registry: Res<lunco_port_core::ports::PortRegistry>,
    active_id: Res<ActiveCommandId>,
    pending_request: Option<Res<PendingApiRequest>>,
    mut commands: Commands,
) {
    let reg = registry.clone();
    let target = cmd.target;
    let writes = cmd.writes.clone();
    let producer_id = cmd.producer_id;
    let command_id = active_id.get();
    let origin = active_id.origin();
    let correlation_id = pending_request
        .map(|request| request.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        let admission_correlation_id = correlation_id
            .or(command_id)
            .unwrap_or_else(|| OpId::new().0);
        let should_admit = should_admit_port_input(world, target, origin, producer_id);
        if should_admit {
            let result = if world
                .get_resource::<ControlWriteFence>()
                .is_some_and(|fence| fence.blocks(target))
            {
                Err((
                    "control endpoint is retiring during a lifecycle transition".to_owned(),
                    lunco_api_core::ApiErrorCode::CommandRejected,
                ))
            } else {
                validate_port_input_writes(world, &reg, target, &writes)
                    .map_err(|message| (message, lunco_api_core::ApiErrorCode::CommandRejected))
                    .and_then(|()| {
                        admit_port_input(
                            world,
                            target,
                            "SetPorts",
                            producer_id,
                            origin,
                            admission_correlation_id,
                            lunco_core_session::SessionInputPayload::PortInputWrites {
                                writes,
                                correlation_id: admission_correlation_id,
                            },
                        )
                    })
            };
            match result {
                Ok(ack) => finish_command_result(
                    world,
                    command_id,
                    correlation_id,
                    Ok(ack),
                    lunco_api_core::ApiErrorCode::CommandRejected,
                ),
                Err((message, error_code)) => finish_command_result(
                    world,
                    command_id,
                    correlation_id,
                    Err(message),
                    error_code,
                ),
            }
            return;
        }

        // Simulation-clock Rhai and fixed-step controller/network producers
        // remain in their existing deterministic pass. Their source inputs
        // are captured at the owning boundary instead of logging resolved
        // actuator writes a second time.
        match apply_port_input_writes(world, &reg, target, &writes) {
            Ok(()) => finish_command_result(
                world,
                command_id,
                correlation_id,
                Ok(Ack::new(OpId::new())),
                lunco_api_core::ApiErrorCode::InternalError,
            ),
            Err((message, error_code)) => {
                finish_command_result(world, command_id, correlation_id, Err(message), error_code)
            }
        }
    });
}

fn should_admit_port_input(
    world: &World,
    target: Entity,
    origin: Option<CommandOrigin>,
    producer_id: Option<u64>,
) -> bool {
    match origin {
        Some(CommandOrigin::ApiTransport) => true,
        Some(CommandOrigin::LocalUser { .. }) => world.get::<GlobalEntityId>(target).is_some(),
        Some(CommandOrigin::Rhai { context, .. }) => {
            context.clock != RuntimeClock::Simulation
                && (context
                    .route
                    .is_some_and(|route| route.scope == lunco_core::RuntimeScope::Twin)
                    || world.get::<GlobalEntityId>(target).is_some())
        }
        None => producer_id.is_some(),
    }
}

fn admit_port_input(
    world: &mut World,
    target: Entity,
    input_name: &str,
    requested_producer_id: Option<u64>,
    origin: Option<CommandOrigin>,
    correlation_id: u64,
    payload: lunco_core_session::SessionInputPayload,
) -> Result<Ack, (String, lunco_api_core::ApiErrorCode)> {
    if correlation_id == 0 {
        return Err((
            format!("{input_name} input correlation id must be nonzero"),
            lunco_api_core::ApiErrorCode::CommandRejected,
        ));
    }
    let producer = lunco_core_session::SessionInputProducer::from_command_origin(
        origin,
        requested_producer_id,
        input_name,
    )
    .map_err(|message| (message, lunco_api_core::ApiErrorCode::CommandRejected))?;
    let target_gid = world
        .get::<GlobalEntityId>(target)
        .copied()
        .ok_or_else(|| {
            (
                format!("live {input_name} admission requires a stable target identity"),
                lunco_api_core::ApiErrorCode::CommandRejected,
            )
        })?;
    let mut matches = world
        .iter_entities()
        .filter(|entity| entity.get::<GlobalEntityId>() == Some(&target_gid));
    let uniquely_resolved = matches
        .next()
        .is_some_and(|entity| entity.id() == target && matches.next().is_none());
    if !uniquely_resolved {
        return Err((
            format!("live {input_name} target identity {target_gid} does not resolve uniquely"),
            lunco_api_core::ApiErrorCode::CommandRejected,
        ));
    }
    drop(matches);
    let scene_generation = world
        .get_resource::<lunco_core::SceneTransitionCoordinator>()
        .and_then(lunco_core::SceneTransitionCoordinator::completed_generation)
        .ok_or_else(|| {
            (
                format!(
                    "{input_name} live-session admission requires a committed scene generation"
                ),
                lunco_api_core::ApiErrorCode::InternalError,
            )
        })?;
    let effective_tick = world
        .get_resource::<lunco_core_runtime::SimTick>()
        .map(|tick| tick.0)
        .ok_or_else(|| {
            (
                format!("{input_name} live-session admission requires SimTick"),
                lunco_api_core::ApiErrorCode::InternalError,
            )
        })?
        .checked_add(1)
        .ok_or_else(|| {
            (
                format!("{input_name} effective simulation tick exhausted"),
                lunco_api_core::ApiErrorCode::InternalError,
            )
        })?;
    if !world.contains_resource::<lunco_control_core::SimulationInputOrderAllocator>() {
        return Err((
            format!(
                "{input_name} live-session admission requires the shared input-order allocator"
            ),
            lunco_api_core::ApiErrorCode::InternalError,
        ));
    }
    if !world.contains_resource::<lunco_core_session::PendingSessionInputs>() {
        return Err((
            format!("{input_name} live-session admission queue is unavailable"),
            lunco_api_core::ApiErrorCode::InternalError,
        ));
    }
    let admission = world
        .resource_scope(
            |world, mut pending: Mut<lunco_core_session::PendingSessionInputs>| {
                let mut order =
                    world.resource_mut::<lunco_control_core::SimulationInputOrderAllocator>();
                pending.admit(
                    &mut order,
                    producer,
                    target_gid,
                    scene_generation,
                    effective_tick,
                    payload,
                    origin,
                )
            },
        )
        .map_err(|message| (message, lunco_api_core::ApiErrorCode::CommandRejected))?;

    let admission = lunco_hooks::HookValue::map([
        (
            "scene_generation",
            lunco_hooks::HookValue::UInt(admission.scene_generation),
        ),
        (
            "effective_tick",
            lunco_hooks::HookValue::UInt(admission.effective_tick),
        ),
        ("sequence", lunco_hooks::HookValue::UInt(admission.sequence)),
    ]);
    Ok(Ack::with_data(
        OpId::new(),
        lunco_hooks::HookValue::map([
            ("target_gid", lunco_hooks::HookValue::UInt(target_gid.get())),
            (
                "producer_kind",
                lunco_hooks::HookValue::str(producer.kind()),
            ),
            (
                "producer_id",
                producer
                    .stable_id()
                    .map_or(lunco_hooks::HookValue::Unit, lunco_hooks::HookValue::UInt),
            ),
            (
                "correlation_id",
                lunco_hooks::HookValue::UInt(correlation_id),
            ),
            ("admission", admission),
        ]),
    ))
}

fn validate_port_input_writes(
    world: &mut World,
    registry: &lunco_port_core::ports::PortRegistry,
    target: Entity,
    writes: &[(String, f64)],
) -> Result<(), String> {
    if writes.is_empty() {
        return Err("SetPorts requires at least one input-port write".to_owned());
    }
    if writes.iter().any(|(name, _)| name.trim().is_empty()) {
        return Err("SetPorts input-port names must not be empty".to_owned());
    }
    if writes.iter().any(|(_, value)| !value.is_finite()) {
        return Err("SetPorts input values must be finite".to_owned());
    }
    let invalid_writes = writes
        .iter()
        .filter(|(port, _)| !registry.has_input_port(world, target, port))
        .cloned()
        .collect::<Vec<_>>();
    if invalid_writes.is_empty() {
        return Ok(());
    }

    let has_port_surface = !registry.entity_ports(world, target).is_empty();
    let label = world
        .get::<Name>(target)
        .map(|name| name.to_string())
        .unwrap_or_else(|| format!("{target:?}"));
    if has_port_surface {
        let global_id = world.get::<GlobalEntityId>(target).copied();
        let mut diagnostics = world.resource_mut::<CosimDiagnostics>();
        for (port, value) in &invalid_writes {
            if diagnostics.has_landed(target, port) {
                continue;
            }
            let inserted = diagnostics.record_fault(BrokenConnection {
                entity: target,
                global_id,
                port: Arc::from(port.as_str()),
                has_port_surface: true,
                dropped_value: *value,
            });
            if inserted {
                warn!(
                    "[cosim] SetPorts targets unknown input port '{}' on {} ({:?}) — batch rejected",
                    port, label, target
                );
            }
        }
    }
    Err(if has_port_surface {
        format!(
            "unknown input port(s) on {label}: {}",
            invalid_writes
                .iter()
                .map(|(port, _)| port.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    } else {
        format!("{label} has no writable input-port surface")
    })
}

fn apply_port_input_writes(
    world: &mut World,
    registry: &lunco_port_core::ports::PortRegistry,
    target: Entity,
    writes: &[(String, f64)],
) -> Result<(), (String, lunco_api_core::ApiErrorCode)> {
    if world
        .get_resource::<ControlWriteFence>()
        .is_some_and(|fence| fence.blocks(target))
    {
        return Err((
            "control endpoint is retiring during a lifecycle transition".to_owned(),
            lunco_api_core::ApiErrorCode::CommandRejected,
        ));
    }
    // TimeTransport is the authoritative user play/pause owner. Modelica's
    // internal readiness pause is not user intent and must not reject controls.
    let user_paused = world
        .get_resource::<lunco_time::TimeTransport>()
        .is_some_and(|transport| !transport.is_running());
    if user_paused && world.get::<GlobalEntityId>(target).is_some() {
        return Err((
            "simulation is paused".to_owned(),
            lunco_api_core::ApiErrorCode::CommandRejected,
        ));
    }
    validate_port_input_writes(world, registry, target, writes)
        .map_err(|message| (message, lunco_api_core::ApiErrorCode::CommandRejected))?;

    // A setpoint on a wired input outranks the wire until an explicit release
    // or lifecycle clear.
    for (port, value) in writes {
        if !registry.write_port(world, target, port, *value) {
            return Err((
                format!("port backend refused declared input '{port}' on {target:?}"),
                lunco_api_core::ApiErrorCode::InternalError,
            ));
        }
        if let Some(mut holds) = world.get_resource_mut::<PortHolds>() {
            holds.hold(target, port.clone(), *value);
        }
        let mut diagnostics = world.resource_mut::<CosimDiagnostics>();
        diagnostics.remove_fault(target, port);
        diagnostics.mark_landed(target, port);
    }
    Ok(())
}

fn on_commit_session_input_ports(
    trigger: On<lunco_core_session::SessionInputCommit>,
    registry: Res<lunco_port_core::ports::PortRegistry>,
    mut commands: Commands,
) {
    let commit = trigger.event();
    let payload = commit.record().payload.clone();
    let target = commit.target();
    let target_gid = commit.record().target;
    let registry = registry.clone();
    commands.queue(move |world: &mut World| {
        let result = match payload {
            lunco_core_session::SessionInputPayload::PortInputWrites {
                writes,
                correlation_id,
            } => apply_port_input_writes(world, &registry, target, &writes).map_err(
                |(message, _error_code)| (correlation_id, "SetPorts", message),
            ),
            lunco_core_session::SessionInputPayload::PortInputRelease {
                name,
                correlation_id: _,
            } => {
                world.resource_mut::<PortHolds>().release(target, &name);
                Ok(())
            }
            lunco_core_session::SessionInputPayload::ControlInputRelease { correlation_id } => {
                apply_release_control(world, target, &registry)
                    .map_err(|message| (correlation_id, "ReleaseControl", message))
            }
            lunco_core_session::SessionInputPayload::ControlSafeStop => {
                if let Err(message) = apply_release_control(world, target, &registry) {
                    world.trigger(lunco_core::RuntimeError {
                        name: "cosim-session-input".to_owned(),
                        message: format!(
                            "ControlSafeStop for target {target_gid} failed at its admitted tick: {message}"
                        ),
                    });
                }
                return;
            }
            _ => return,
        };
        if let Err((correlation_id, command, message)) = result {
            world.trigger(lunco_core::RuntimeError {
                name: "cosim-session-input".to_owned(),
                message: format!(
                    "{command} correlation {correlation_id} for target {target_gid} failed at its admitted tick: {message}"
                ),
            });
        };
    });
}

#[on_command(ReleasePort)]
fn on_release_port(
    trigger: On<ReleasePort>,
    active_id: Option<Res<ActiveCommandId>>,
    pending_request: Option<Res<PendingApiRequest>>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let target = cmd.target;
    let name = cmd.name.clone();
    let producer_id = cmd.producer_id;
    let command_id = active_id.as_ref().and_then(|active_id| active_id.get());
    let origin = active_id.as_ref().and_then(|active_id| active_id.origin());
    let correlation_id = pending_request
        .map(|request| request.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        let admission_correlation_id = correlation_id
            .or(command_id)
            .unwrap_or_else(|| OpId::new().0);
        let result = if should_admit_port_input(world, target, origin, producer_id) {
            if world
                .get_resource::<ControlWriteFence>()
                .is_some_and(|fence| fence.blocks(target))
            {
                Err((
                    "control endpoint is retiring during a lifecycle transition".to_owned(),
                    lunco_api_core::ApiErrorCode::CommandRejected,
                ))
            } else {
                admit_port_input(
                    world,
                    target,
                    "ReleasePort",
                    producer_id,
                    origin,
                    admission_correlation_id,
                    lunco_core_session::SessionInputPayload::PortInputRelease {
                        name,
                        correlation_id: admission_correlation_id,
                    },
                )
            }
        } else {
            cancel_superseded_port_writes(world, target, Some(&name), "ReleasePort");
            world.resource_mut::<PortHolds>().release(target, &name);
            Ok(Ack::new(OpId::new()))
        };
        match result {
            Ok(ack) => finish_command_result(
                world,
                command_id,
                correlation_id,
                Ok(ack),
                lunco_api_core::ApiErrorCode::CommandRejected,
            ),
            Err((message, error_code)) => {
                finish_command_result(world, command_id, correlation_id, Err(message), error_code)
            }
        }
    });
}

/// Apply the one authoritative safe-stop transaction for every vehicle control
/// surface. The command-world closure is necessary because port backends use
/// `&mut World`; it also gives a stale `SetPorts` write the same lifecycle fence
/// as possession and controller release.
#[on_command(ReleaseControl)]
fn on_release_control(
    trigger: On<ReleaseControl>,
    registry: Res<lunco_port_core::ports::PortRegistry>,
    active_id: Option<Res<ActiveCommandId>>,
    pending_request: Option<Res<PendingApiRequest>>,
    mut commands: Commands,
) {
    let command = trigger.event();
    let target = command.target;
    let producer_id = command.producer_id;
    let registry = registry.clone();
    let command_id = active_id.as_ref().and_then(|active_id| active_id.get());
    let origin = active_id.as_ref().and_then(|active_id| active_id.origin());
    let correlation_id = pending_request
        .map(|request| request.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        let admission_correlation_id = correlation_id
            .or(command_id)
            .unwrap_or_else(|| OpId::new().0);
        let result = if should_admit_port_input(world, target, origin, producer_id) {
            if world
                .get_resource::<ControlWriteFence>()
                .is_some_and(|fence| fence.blocks(target))
            {
                Err((
                    "control endpoint is retiring during a lifecycle transition".to_owned(),
                    lunco_api_core::ApiErrorCode::CommandRejected,
                ))
            } else {
                admit_port_input(
                    world,
                    target,
                    "ReleaseControl",
                    producer_id,
                    origin,
                    admission_correlation_id,
                    lunco_core_session::SessionInputPayload::ControlInputRelease {
                        correlation_id: admission_correlation_id,
                    },
                )
            }
        } else {
            cancel_superseded_port_writes(world, target, None, "ReleaseControl");
            apply_release_control(world, target, &registry)
                .map(|()| Ack::new(OpId::new()))
                .map_err(|message| (message, lunco_api_core::ApiErrorCode::InternalError))
        };
        match result {
            Ok(ack) => finish_command_result(
                world,
                command_id,
                correlation_id,
                Ok(ack),
                lunco_api_core::ApiErrorCode::CommandRejected,
            ),
            Err((message, error_code)) => {
                finish_command_result(world, command_id, correlation_id, Err(message), error_code)
            }
        }
    });
}

fn cancel_superseded_port_writes(
    world: &mut World,
    target: Entity,
    port: Option<&str>,
    release_command: &str,
) {
    let Some(target_gid) = world.get::<GlobalEntityId>(target).copied() else {
        return;
    };
    let canceled = world
        .get_resource_mut::<lunco_core_session::PendingSessionInputs>()
        .map(|mut pending| pending.cancel_port_input_writes(target_gid, port))
        .unwrap_or_default();
    for input in canceled {
        let lunco_core_session::SessionInputPayload::PortInputWrites { correlation_id, .. } =
            input.payload
        else {
            continue;
        };
        warn!(
            "[cosim] SetPorts correlation {correlation_id} for target {target_gid} was superseded by {release_command} before its admitted tick"
        );
    }
}

fn apply_release_control(
    world: &mut World,
    target: Entity,
    registry: &lunco_port_core::ports::PortRegistry,
) -> Result<(), String> {
    world.resource_mut::<ControlWriteFence>().block(target);
    let mut input_names = world.resource::<PortHolds>().entity_port_names(target);
    input_names.extend(
        registry
            .entity_ports(world, target)
            .into_iter()
            .filter(|port| {
                matches!(
                    port.direction,
                    lunco_port_core::ports::PortDirection::In
                        | lunco_port_core::ports::PortDirection::InOut
                )
            })
            .map(|port| port.name),
    );
    world.resource_mut::<PortHolds>().clear_entity(target);
    input_names.sort();
    input_names.dedup();
    for name in &input_names {
        // `brake` is the only declared command with an engaged safe value;
        // every other command is neutral.
        let value = if name == "brake" { 1.0 } else { 0.0 };
        if !registry.write_port(world, target, name, value) {
            return Err(format!("safe-stop backend refused declared input '{name}'"));
        }
        world
            .resource_mut::<PortHolds>()
            .hold(target, name.clone(), value);
    }
    if let Some(mut command_surface) = world.get_mut::<lunco_port_core::InputPorts>(target) {
        command_surface.safe_stop();
    }

    // The logical input backend is covered by the registry above. Output
    // actuator ports are also neutralized immediately when the authored
    // endpoint publishes them, matching the existing hard-stop contract.
    let actuator_ports = world
        .get::<lunco_port_core::OutputPorts>(target)
        .map(|outputs| outputs.ports.clone())
        .unwrap_or_default();
    for (name, entity) in actuator_ports.iter() {
        if let Some(mut port) = world.get_mut::<lunco_port_core::Port>(*entity) {
            port.value = if name == "brake" { 1.0 } else { 0.0 };
        }
    }
    Ok(())
}

/// Translate released session authority into the backend's safe-stop command.
///
/// Session authority is expressed in stable global ids, while the co-simulation
/// backend applies the stop to the live endpoint entity. Keeping that translation
/// here lets every higher-level controller reuse the same authority transition.
fn on_control_authority_changed(
    trigger: On<ControlAuthorityChanged>,
    q_endpoints: Query<(Entity, &lunco_core::GlobalEntityId), With<lunco_port_core::InputPorts>>,
    mut commands: Commands,
) {
    let released = &trigger.event().released;
    for (entity, gid) in q_endpoints.iter() {
        if released.contains(&gid.get()) {
            commands.trigger(ControlSafeStop { target: entity });
        }
    }
}

fn on_control_safe_stop(
    trigger: On<ControlSafeStop>,
    registry: Res<lunco_port_core::ports::PortRegistry>,
    mut commands: Commands,
) {
    let target = trigger.event().target;
    let registry = registry.clone();
    commands.queue(move |world: &mut World| {
        cancel_superseded_port_writes(world, target, None, "control-authority safe stop");
        match apply_release_control(world, target, &registry) {
            Ok(()) => {
                if let Err(message) = admit_lifecycle_safe_stop(world, target) {
                    world.trigger(lunco_core::RuntimeError {
                        name: "session-input-recording".to_owned(),
                        message,
                    });
                }
            }
            Err(message) => {
                fail_session_input_capture(world, format!("control safe-stop failed: {message}"));
                world.trigger(lunco_core::RuntimeError {
                    name: "cosim-control-safe-stop".to_owned(),
                    message,
                });
            }
        }
    });
}

fn admit_lifecycle_safe_stop(world: &mut World, target: Entity) -> Result<(), String> {
    if !world
        .get_resource::<lunco_core_session::SessionInputStream>()
        .is_some_and(lunco_core_session::SessionInputStream::is_recording)
    {
        return Ok(());
    }

    let record = (|| {
        let target_id = world
            .get::<GlobalEntityId>(target)
            .copied()
            .ok_or_else(|| "lifecycle safe-stop target has no stable GlobalEntityId".to_owned())?;
        let scene_generation = world
            .get_resource::<lunco_core::SceneTransitionCoordinator>()
            .and_then(lunco_core::SceneTransitionCoordinator::completed_generation)
            .ok_or_else(|| {
                "lifecycle safe-stop capture requires a committed scene generation".to_owned()
            })?;
        let effective_tick = world
            .get_resource::<lunco_core_runtime::SimTick>()
            .map(|tick| tick.0)
            .ok_or_else(|| "lifecycle safe-stop capture requires SimTick".to_owned())?
            .checked_add(1)
            .ok_or_else(|| "lifecycle safe-stop effective tick exhausted".to_owned())?;
        if !world.contains_resource::<lunco_control_core::SimulationInputOrderAllocator>() {
            return Err(
                "lifecycle safe-stop capture requires the shared input-order allocator".to_owned(),
            );
        }
        if !world.contains_resource::<lunco_core_session::PendingSessionInputs>() {
            return Err("lifecycle safe-stop capture queue is unavailable".to_owned());
        }

        world.resource_scope(
            |world, mut pending: Mut<lunco_core_session::PendingSessionInputs>| {
                let mut order =
                    world.resource_mut::<lunco_control_core::SimulationInputOrderAllocator>();
                pending.admit(
                    &mut order,
                    lunco_core_session::SessionInputProducer::RuntimeLifecycle,
                    target_id,
                    scene_generation,
                    effective_tick,
                    lunco_core_session::SessionInputPayload::ControlSafeStop,
                    None,
                )
            },
        )
    })();

    if let Err(message) = record {
        fail_session_input_capture(world, message.clone());
        return Err(message);
    }
    Ok(())
}

fn fail_session_input_capture(world: &mut World, message: String) {
    if let Some(mut stream) = world.get_resource_mut::<lunco_core_session::SessionInputStream>() {
        stream.fail(message);
    }
}

register_commands!(on_set_ports, on_release_port, on_release_control);

#[cfg(test)]
mod control_intent_tests {
    use super::*;
    use std::collections::HashMap;

    fn install_capture_state(app: &mut App, with_committed_scene: bool) {
        app.init_resource::<lunco_core_session::SessionInputStream>()
            .init_resource::<lunco_core_session::PendingSessionInputs>()
            .init_resource::<lunco_control_core::SimulationInputOrderAllocator>()
            .insert_resource(lunco_core_runtime::SimTick(40));
        if with_committed_scene {
            let mut coordinator = lunco_core::SceneTransitionCoordinator::default();
            let request = lunco_core::SceneTransitionRequest::load("capture.usda", "/World");
            coordinator.admit(request);
            coordinator.take_admitted().expect("admitted scene request");
            let id = coordinator.start(lunco_core::SceneTransition::load("capture.usda", "/World"));
            assert!(coordinator.complete(id));
            app.insert_resource(coordinator);
        }
        app.world_mut()
            .resource_mut::<lunco_core_session::SessionInputStream>()
            .begin(4)
            .expect("capture starts");
    }

    #[test]
    fn lifecycle_safe_stop_is_captured_at_the_next_fixed_tick() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);
        install_capture_state(&mut app, true);
        let target = app
            .world_mut()
            .spawn((
                GlobalEntityId::from_raw(42),
                lunco_port_core::InputPorts::with_defaults([
                    ("throttle".to_owned(), 0.75),
                    ("brake".to_owned(), 0.0),
                ]),
            ))
            .id();

        app.world_mut().trigger(ControlSafeStop { target });
        app.world_mut().flush();

        let pending = app
            .world()
            .resource::<lunco_core_session::PendingSessionInputs>();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending.entries().next().unwrap().record().effective_tick,
            41
        );
        let inputs = app
            .world()
            .get::<lunco_port_core::InputPorts>(target)
            .unwrap();
        assert_eq!(inputs.cmd("throttle"), 0.0);
        assert_eq!(inputs.cmd("brake"), 1.0);
        assert!(
            app.world()
                .resource::<lunco_core_session::SessionInputStream>()
                .records()
                .is_empty()
        );
        {
            let mut inputs = app
                .world_mut()
                .get_mut::<lunco_port_core::InputPorts>(target)
                .unwrap();
            inputs.values.insert("throttle".to_owned(), 0.75);
            inputs.values.insert("brake".to_owned(), 0.0);
        }
        app.world_mut()
            .resource_mut::<lunco_core_runtime::SimTick>()
            .0 = 41;
        lunco_core_session::commit_due_session_inputs(app.world_mut());

        let stream = app
            .world()
            .resource::<lunco_core_session::SessionInputStream>();
        assert_eq!(
            stream.state(),
            lunco_core_session::SessionInputStreamState::Recording
        );
        assert_eq!(stream.records().len(), 1);
        let record = &stream.records()[0];
        assert_eq!(
            record.producer,
            lunco_core_session::SessionInputProducer::RuntimeLifecycle
        );
        assert_eq!(record.target, GlobalEntityId::from_raw(42));
        assert_eq!(record.scene_generation, 1);
        assert_eq!(record.effective_tick, 41);
        assert_eq!(record.sequence, 1);
        assert_eq!(
            record.payload,
            lunco_core_session::SessionInputPayload::ControlSafeStop
        );

        let inputs = app
            .world()
            .get::<lunco_port_core::InputPorts>(target)
            .unwrap();
        assert_eq!(inputs.cmd("throttle"), 0.0);
        assert_eq!(inputs.cmd("brake"), 1.0);
    }

    #[test]
    fn lifecycle_safe_stop_fails_capture_without_committed_scene_generation() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);
        install_capture_state(&mut app, false);
        let target = app
            .world_mut()
            .spawn((
                GlobalEntityId::from_raw(42),
                lunco_port_core::InputPorts::new(&["throttle", "brake"]),
            ))
            .id();

        app.world_mut().trigger(ControlSafeStop { target });
        app.world_mut().flush();

        let stream = app
            .world()
            .resource::<lunco_core_session::SessionInputStream>();
        assert_eq!(
            stream.state(),
            lunco_core_session::SessionInputStreamState::Failed
        );
        assert!(
            stream
                .failure()
                .unwrap()
                .contains("committed scene generation")
        );
        assert!(stream.records().is_empty());
        let inputs = app
            .world()
            .get::<lunco_port_core::InputPorts>(target)
            .unwrap();
        assert_eq!(inputs.cmd("throttle"), 0.0);
        assert_eq!(inputs.cmd("brake"), 1.0);
    }

    #[test]
    fn release_control_latches_intent_until_explicit_release() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);
        app.init_resource::<ActiveCommandId>();

        let throttle_output = app.world_mut().spawn(lunco_port_core::Port::default()).id();
        let brake_output = app.world_mut().spawn(lunco_port_core::Port::default()).id();
        let target = app
            .world_mut()
            .spawn((
                lunco_port_core::InputPorts::new(&[
                    "throttle",
                    "steer",
                    "brake",
                    "external_throttle",
                    "pitch",
                    "roll",
                    "yaw",
                    "rcs_translation",
                ]),
                lunco_port_core::OutputPorts::new(HashMap::from([
                    ("throttle".to_string(), throttle_output),
                    ("brake".to_string(), brake_output),
                ])),
            ))
            .id();

        let writes = vec![
            ("throttle".to_string(), 0.75),
            ("steer".to_string(), -0.25),
            ("brake".to_string(), 0.0),
            ("external_throttle".to_string(), 0.6),
            ("pitch".to_string(), -0.3),
            ("roll".to_string(), 0.2),
            ("yaw".to_string(), -0.1),
            ("rcs_translation".to_string(), 0.4),
        ];
        app.world_mut().trigger(SetPorts {
            target,
            writes: writes.clone(),
            seq: 0,
            tick: 1,
            producer_id: None,
        });
        app.world_mut().flush();

        for (name, value) in &writes {
            assert_eq!(
                app.world()
                    .get::<lunco_port_core::InputPorts>(target)
                    .unwrap()
                    .cmd(name),
                *value,
                "SetPorts must apply the initial command"
            );
        }

        // The receiver-side intent survives ordinary updates; it is not a
        // one-tick write that the propagation schedule can erase.
        app.update();
        app.update();
        assert_eq!(
            app.world()
                .get::<lunco_port_core::InputPorts>(target)
                .unwrap()
                .cmd("external_throttle"),
            0.6
        );

        // A release fences a queued stale SetPorts before it can re-arm the
        // outgoing value, then leaves the neutral values held for the wiring
        // fabric.
        app.world_mut().trigger(SetPorts {
            target,
            writes,
            seq: 0,
            tick: 2,
            producer_id: None,
        });
        app.world_mut().trigger(ControlSafeStop { target });
        app.world_mut().flush();

        let inputs = app
            .world()
            .get::<lunco_port_core::InputPorts>(target)
            .unwrap();
        assert_eq!(inputs.cmd("throttle"), 0.0);
        assert_eq!(inputs.cmd("steer"), 0.0);
        assert_eq!(inputs.cmd("brake"), 1.0);
        assert_eq!(inputs.cmd("external_throttle"), 0.0);
        assert_eq!(inputs.cmd("pitch"), 0.0);
        assert_eq!(inputs.cmd("roll"), 0.0);
        assert_eq!(inputs.cmd("yaw"), 0.0);
        assert_eq!(inputs.cmd("rcs_translation"), 0.0);
        assert!(inputs.brake_active);
        assert_eq!(
            app.world()
                .get::<lunco_port_core::Port>(throttle_output)
                .unwrap()
                .value,
            0.0
        );
        assert_eq!(
            app.world()
                .get::<lunco_port_core::Port>(brake_output)
                .unwrap()
                .value,
            1.0
        );

        let holds = app.world().resource::<PortHolds>().snapshot();
        assert_eq!(holds.get(&(target, "throttle".to_string())), Some(&0.0));
        assert_eq!(holds.get(&(target, "brake".to_string())), Some(&1.0));
        assert_eq!(
            holds.get(&(target, "external_throttle".to_string())),
            Some(&0.0)
        );

        // The fence opens at the next fixed/update boundary; an explicit new
        // owner command can then replace the safe intent.
        app.world_mut().run_schedule(FixedFirst);
        app.world_mut().trigger(SetPorts {
            target,
            writes: vec![("throttle".to_string(), 0.5)],
            seq: 0,
            tick: 3,
            producer_id: None,
        });
        app.world_mut().flush();
        assert_eq!(
            app.world()
                .get::<lunco_port_core::InputPorts>(target)
                .unwrap()
                .cmd("throttle"),
            0.5
        );
    }

    #[test]
    fn set_ports_allows_solver_barrier_but_rejects_user_pause() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);
        app.init_resource::<ActiveCommandId>();
        let target = app
            .world_mut()
            .spawn((
                lunco_core::GlobalEntityId::from_raw(7),
                lunco_port_core::InputPorts::new(&["headlights"]),
            ))
            .id();
        app.insert_resource(lunco_time::TimeTransport {
            mode: lunco_time::TransportMode::Paused,
            rate: 1.0,
        });

        app.world_mut().trigger(SetPorts {
            target,
            writes: vec![("headlights".into(), 1.0)],
            seq: 0,
            tick: 1,
            producer_id: None,
        });
        app.world_mut().flush();
        assert_eq!(
            app.world()
                .get::<lunco_port_core::InputPorts>(target)
                .unwrap()
                .cmd("headlights"),
            0.0,
            "an explicit user pause must reject scene control writes"
        );

        app.world_mut()
            .resource_mut::<lunco_time::TimeTransport>()
            .mode = lunco_time::TransportMode::Playing;
        app.init_resource::<lunco_core_runtime::SimulationBarrier>();
        app.world_mut()
            .resource_mut::<lunco_core_runtime::SimulationBarrier>()
            .held = true;
        app.world_mut().trigger(SetPorts {
            target,
            writes: vec![("headlights".into(), 1.0)],
            seq: 0,
            tick: 2,
            producer_id: None,
        });
        app.world_mut().flush();
        assert_eq!(
            app.world()
                .get::<lunco_port_core::InputPorts>(target)
                .unwrap()
                .cmd("headlights"),
            1.0,
            "a solver synchronization barrier must not discard a control intent"
        );
    }

    #[test]
    fn release_control_neutralizes_declared_modelica_inputs_after_port_release() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(CoSimPlugin);

        let mut component = SimComponent::default();
        component.inputs.insert("throttle".into(), 0.625);
        let target = app.world_mut().spawn(component).id();
        app.world_mut()
            .resource_mut::<PortHolds>()
            .hold(target, "throttle", 0.625);
        app.world_mut()
            .resource_mut::<PortHolds>()
            .release(target, "throttle");

        let registry = app
            .world()
            .resource::<lunco_port_core::ports::PortRegistry>()
            .clone();
        apply_release_control(app.world_mut(), target, &registry)
            .expect("safe stop writes every declared input through the registry");

        assert_eq!(
            app.world()
                .get::<SimComponent>(target)
                .unwrap()
                .inputs
                .get("throttle"),
            Some(&0.0)
        );
        assert_eq!(
            app.world()
                .resource::<PortHolds>()
                .snapshot()
                .get(&(target, "throttle".to_owned())),
            Some(&0.0)
        );
    }
}
