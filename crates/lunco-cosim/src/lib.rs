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
//! | **AvianSim**   | `force_y`, `force_x`        | `height`, `velocity_y`, ...     |
//! | **SimComponent** (Modelica) | `height`, `velocity`, `g` | `netForce`, `volume`, ... |
//! | **SimComponent** (FMU)     | `current_in`            | `soc`, `voltage`, ...         |
//!
//! [`crate::SimConnection`] connects any output to any input, following the FMI/SSP pattern.
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
//!     start_connector: "height".into(),
//!     end_element: balloon_entity,
//!     end_connector: "height".into(),
//!     scale: 1.0,
//! });
//! ```

use bevy::prelude::*;

pub mod avian;
pub mod binding;
pub mod component;
pub mod connection;
pub mod diagnostics;
pub mod joint;
pub mod ports;
pub mod sensors;
pub mod suggestion;
pub mod systems;
pub mod telemetry;

pub use avian::*;
pub use binding::*;
pub use component::*;
pub use connection::*;
pub use diagnostics::{BrokenConnection, CosimDiagnostics};
pub use joint::*;
pub use ports::*;
pub use suggestion::*;

// Typed-command machinery (re-exported from `lunco-core`, which re-exports
// the `lunco-command-macro` proc-macros). Used by the `SetPorts` command +
// observer defined below — the ONE generic vessel-control command (a batch of
// named input-port writes), driving landers, rovers, and any port-bearing vessel.
use lunco_core::{on_command, register_commands, Command};

fn endpoint_ready_on_add<T: Component>(
    trigger: On<Add, T>,
    mut commands: Commands,
    mut revision: ResMut<BindingRevision>,
) {
    commands.entity(trigger.entity).try_insert(EndpointLifecycle::Ready);
    revision.request();
}

fn sync_model_endpoint_lifecycle(
    query: Query<(Entity, &SimComponent), Changed<SimComponent>>,
    mut commands: Commands,
    mut revision: ResMut<BindingRevision>,
) {
    for (entity, component) in &query {
        let state = match &component.status {
            SimStatus::Compiling => EndpointLifecycle::Pending,
            SimStatus::Error(message) => EndpointLifecycle::Failed(message.clone()),
            _ => EndpointLifecycle::Ready,
        };
        commands.entity(entity).try_insert(state);
        revision.request();
    }
}

/// Plugin for co-simulation orchestration.
///
/// Registers [`crate::SimComponent`], [`crate::AvianSim`], and [`crate::SimConnection`] types,
/// and adds systems for wire propagation and Avian manual stepping.
///
/// ## Usage
///
/// ```rust,ignore
/// app.add_plugins(CoSimPlugin);
/// ```
///
/// Engine plugins (e.g., `lunco-modelica`) depend on this crate and
/// create [`crate::SimComponent`] instances when models compile.
pub struct CoSimPlugin;

impl Plugin for CoSimPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<SimComponent>()
            .register_type::<PendingForces>()
            .register_type::<SimConnection>()
            .register_type::<RealtimeSafe>()
            .register_type::<sensors::ImuSensor>()
            .register_type::<sensors::RangeSensor>()
            .register_type::<sensors::ContactSensor>();

        // The shared port substrate (in `lunco-core`, below every participant).
        // The cosim engine owns the avian/joint/Modelica/hardware backends and
        // registers them here; wires, the API, the inspector, and scripts all
        // read/write through this one registry. Registration order = resolution
        // precedence (Modelica, avian, then single-value hardware ports).
        app.init_resource::<lunco_core::ports::PortRegistry>()
            .init_resource::<BindingRevision>();
        // Machine-readable dangling-wire report, refreshed each propagation tick
        // and surfaced via the API's `GET /api/diagnostics` (`GetBrokenConnections`).
        app.init_resource::<diagnostics::CosimDiagnostics>();
        // Manual setpoints that outrank the wiring fabric until they expire —
        // without it, a `SetPort` on a WIRED input lives less than one tick.
        app.init_resource::<connection::PortHolds>();
        // A lifecycle command may retire a producer after its SetPorts trigger
        // was emitted but before its deferred write lands. Keep that stale write
        // outside the shared control boundary until next tick.
        app.init_resource::<connection::ControlWriteFence>();
        app.add_systems(FixedFirst, connection::clear_control_write_fence);
        app.add_observer(binding::on_add_connection)
            .add_observer(endpoint_ready_on_add::<lunco_core::InputPorts>)
            .add_observer(endpoint_ready_on_add::<lunco_core::architecture::Port>)
            .add_observer(endpoint_ready_on_add::<avian3d::prelude::RigidBody>)
            .add_observer(endpoint_ready_on_add::<avian3d::prelude::RevoluteJoint>)
            .add_observer(endpoint_ready_on_add::<avian3d::prelude::PrismaticJoint>);
        app.add_systems(
            Update,
            (
                sync_model_endpoint_lifecycle
                    .run_if(|q: Query<(), Changed<SimComponent>>| !q.is_empty()),
                binding::bind_connections
                    .run_if(|revision: Res<BindingRevision>| revision.pending()),
            )
                .chain(),
        );
        {
            let mut registry = app
                .world_mut()
                .resource_mut::<lunco_core::ports::PortRegistry>();
            ports::register_builtin_port_backends(&mut registry);
        }

        // No per-kind observers: avian rigid bodies and joints are detected by
        // component presence through the `AVIAN` spec table (backend in this
        // crate, `crates/lunco-cosim`; original design in git history).

        // CoSim runs in FixedUpdate (before Avian's FixedPostUpdate physics step).
        // Order: propagate wires first, then apply forces to Position.
        // Avian's own PhysicsSchedule runs in FixedPostUpdate — we do NOT step it
        // manually here to avoid double-stepping.
        app.configure_sets(
            FixedUpdate,
            (
                systems::propagate::CosimSet::Propagate,
                systems::apply_forces::CosimSet::ApplyForces,
            )
                .chain(),
        );

        // `CosimSet::Propagate` IS the control DAC. Nesting it inside
        // `lunco_core::ControlDacSet` is what gives that anchor its meaning:
        // every actuator that reads a `Port` orders `.after(ControlDacSet)`
        // (lunco-controller, lunco-autopilot, lunco-hardware, lunco-mobility) and
        // those edges must resolve against the system that actually writes the
        // port — this one. A sibling `.before()` relationship would instead leave
        // the anchor empty and every such ordering a silent no-op, letting the
        // actuation slip a whole tick frame-to-frame and diverge host vs client
        // under prediction.
        app.configure_sets(
            FixedUpdate,
            systems::propagate::CosimSet::Propagate.in_set(lunco_core::ControlDacSet),
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
            lunco_core::RollbackReplay,
            systems::propagate::CosimSet::Propagate.in_set(lunco_core::ControlDacSet),
        );
        app.add_systems(
            lunco_core::RollbackReplay,
            systems::propagate::propagate_connections
                .in_set(systems::propagate::CosimSet::Propagate),
        );

        app.add_systems(
            FixedUpdate,
            (
                systems::propagate::propagate_connections
                    .in_set(systems::propagate::CosimSet::Propagate),
                // The single avian force consumer: drains `PendingForces` (filled
                // by propagation's `force_*` writes) into avian's `Forces`. Joint
                // motors are driven inline by the `angle` input port's write
                // closure during propagation, so no separate joint-drive system.
                // Additionally gated on `physics_is_live`: this is the one system
                // here that writes into avian's FORCE ACCUMULATOR, which only the
                // physics step clears. A physics hold (a frozen cinematic beat)
                // leaves `FixedUpdate` running by design, so ungated this kept
                // draining thruster force AND TORQUE into the accumulator with
                // nothing consuming it, then discharged the whole integral on the
                // single step that released the hold. Torque, unlike gravity,
                // accumulates about the COM and so discharges as SPIN — the measured
                // ~25 rad/s transient on episode 1's lander/rover stack. The
                // `propagate_connections` above is deliberately NOT gated here: it
                // moves VALUES around the cosim graph rather than accumulating one,
                // a held beat still wants a live graph, and its network gating is
                // PER TARGET (`peer_simulates`) rather than per process — a client
                // must keep propagating into the bodies it locally predicts, or the
                // predicted rover's command never reaches its actuators.
                //
                // The role gate rides the force accumulator alone: a pure client
                // renders host snapshots for replicated bodies, and adding
                // locally-derived forces to them fights the snapshot stream.
                avian::apply_pending_forces
                    .in_set(systems::apply_forces::CosimSet::ApplyForces)
                    // `resource_exists` FIRST, for the same reason the sensors
                    // below carry it: `physics_is_live` reads `Res<Time<Physics>>`
                    // unconditionally, so without avian the run condition itself
                    // hard-errors instead of gating. Headless cosim with no avian
                    // then skips force application, which is the intent.
                    .run_if(
                        resource_exists::<Time<avian3d::prelude::Physics>>
                            .and_then(lunco_physics::physics_is_live),
                    )
                    .run_if(|role: Option<Res<lunco_core::NetworkRole>>| {
                        // Absent role (single-player, headless tests) → run.
                        // Only a present `Client` role gates it off.
                        !matches!(role.as_deref(), Some(lunco_core::NetworkRole::Client))
                    }),
            ),
        );

        // Avian outputs (position/velocity, joint twist) are read on demand
        // through the resolver — avian's state is stable between physics steps,
        // so no per-tick snapshot system is needed.

        // Sensors refresh their cached outputs before propagation so a wire
        // reading `accel_*`/`range`/`contact*` sees this tick's value. They only
        // touch entities carrying the corresponding sensor component.
        //
        // The IMU sensor needs only `Time<Fixed>` (a core resource), so it runs
        // unconditionally. Range + contact sensors read avian-only system params
        // (`SpatialQuery`, `Collisions` / `SubstepCount` / `Time<Physics>`), which
        // only exist when `PhysicsPlugins` is added. Bevy 0.18 turns a missing
        // `Res`/param into a hard error via the default handler (older versions
        // silently skipped the system), so gate them on physics being active —
        // headless cosim without avian (e.g. integration tests) then just skips
        // them instead of panicking.
        app.add_systems(
            FixedUpdate,
            sensors::update_imu_sensors.before(systems::propagate::CosimSet::Propagate),
        );
        app.add_systems(
            FixedUpdate,
            (
                sensors::update_range_sensors,
                sensors::update_contact_sensors,
            )
                .run_if(resource_exists::<Time<avian3d::prelude::Physics>>)
                .before(systems::propagate::CosimSet::Propagate),
        );

        // The range-sensor BEAM is drawn by `lunco-render-bevy`'s `sensor_beams`,
        // not here: naming `Gizmos`/`GizmoConfigStore` dragged
        // `bevy_gizmos → bevy_render → wgpu + naga` into every build, including the
        // `--no-ui` server and the wasm worker. The SENSING (`update_range_sensors`,
        // a `SpatialQuery` raycast) is simulation, must run headless, and stays here
        // — the render layer reads its stored result and re-casts nothing.
        // See `docs/architecture/render-decoupling.md`.

        app.add_systems(Update, systems::collider::sync_collider);

        // A model's own variables — the INTERNAL Modelica state included — become
        // retained, plottable history without anyone authoring a channel per
        // variable. Runs AFTER propagation so a sample is the post-step value the
        // rest of the frame sees, never the previous tick's. See `telemetry.rs`
        // for the namespace that keeps it out of authored channels' buffers and
        // for the rate/retention/memory arithmetic.
        app.init_resource::<telemetry::CosimTelemetrySettings>();
        app.init_resource::<telemetry::CosimTelemetryClock>();
        app.init_resource::<lunco_signal::SignalRegistry>();
        app.add_systems(
            FixedUpdate,
            telemetry::publish_cosim_variables.after(systems::propagate::CosimSet::Propagate),
        );

        // Register the typed command observers generated below (the
        // `register_commands!` list turns into `register_all_commands(app)`).
        register_all_commands(app);
    }
}

// ── Typed Command: generic port actuation ─────────────────────────────────────

/// The ONE generic control command: write a batch of named input ports on
/// `target`, applied through [`PortRegistry::write_port`]. This is the whole of
/// vessel control — there is no `DriveRover`/`BrakeRover`/`DriveLander` and no
/// axis/`VesselIntent` vocabulary. "Controlling" anything means writing its
/// command input ports:
/// - a wheeled rover exposes `throttle`/`steer`/`brake` (its `InputPorts`
///   input surface, via the core input-port backend); a mix system projects them
///   onto its actuator ports,
/// - a cosim-flown lander exposes its Modelica command inputs (`throttle`/`pitch`/
///   `roll`/`yaw`) via the [`SimComponent`] backend,
/// - a crane/door/factory arm exposes whatever input ports it declares.
///
/// The same command is emitted by the keyboard input path
/// (`lunco-controller`), the HTTP/MCP API, scripts, and replayed remote peers —
/// so every surface drives every controllable thing identically. `seq`/`tick`
/// carry the prediction bookkeeping (host ack + client input log), replacing
/// `DriveRover`'s; it rides `SyncChannel::ControlStream` over the network.
#[Command]
pub struct SetPorts {
    /// The entity whose input ports are written.
    #[authz_target]
    pub target: Entity,
    /// `(port_name, value)` writes to apply this tick. Undeclared names are
    /// dropped by `PortRegistry` (strict per-backend) — the write stays a no-op,
    /// but when the target exposes a port surface WITHOUT that name the drop is
    /// recorded once per `(entity, port)` in [`CosimDiagnostics::faults`] (M12),
    /// so a typo'd port from the API/script/autopilot surfaces instead of
    /// vanishing. A binding may still name ports a given vessel doesn't have.
    pub writes: Vec<(String, f64)>,
    #[serde(default)]
    #[reflect(default)]
    pub seq: u32,
    #[serde(default)]
    #[reflect(default)]
    pub tick: u64,
}

/// Observer for [`SetPorts`]: applies each `(name, value)` via the
/// [`PortRegistry`] — the single dispatch that reaches Modelica `SimComponent`
/// inputs, an `InputPorts` surface (throttle/steer/brake, …),
/// hardware `Port`s, or any future backend, all by name.
/// `write_port` needs `&mut World`, so we clone the (cheap, `fn`-pointer)
/// registry and defer the writes through a `Commands` world closure.
///
/// On control-path latency ("input at tick N → wheels at tick N"), two halves:
///
/// 1. **Producer ordering (not in this crate) — DECLARED.** `drive_from_bindings`
///    (`lunco-controller`) and `drive_autopilots` (`lunco-autopilot`) register
///    with an explicit `.before(lunco_core::ControlDacSet)` edge, so the
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
///    `lunco_core::ports` — a core change, out of scope here. Note the queued
///    closure is appended to the SAME command queue that is being flushed, so it
///    lands within that flush; the ordering risk is (1), not this hop.
#[on_command(SetPorts)]
fn on_set_ports(
    trigger: On<SetPorts>,
    registry: Res<lunco_core::ports::PortRegistry>,
    mut commands: Commands,
) {
    let reg = registry.clone();
    let target = cmd.target;
    let writes = cmd.writes.clone();
    commands.queue(move |world: &mut World| {
        if world
            .get_resource::<connection::ControlWriteFence>()
            .is_some_and(|fence| fence.blocks(target))
        {
            return;
        }
        // A setpoint on a WIRED input has to outrank the wire, or the next
        // propagation tick overwrites it and the caller sees a write that
        // "succeeded" and did nothing. The hold expires on its own
        // (`DEFAULT_HOLD_SECS`), so a stream of setpoints keeps control while an
        // abandoned one hands the port back to its wiring.
        let now_real = world
            .get_resource::<Time<bevy::time::Real>>()
            .map(|time| time.elapsed_secs_f64())
            .unwrap_or(0.0);
        for (port, value) in &writes {
            if reg.write_port(world, target, port, *value) {
                if let Some(mut holds) = world.get_resource_mut::<connection::PortHolds>() {
                    holds.hold(
                        target,
                        port.clone(),
                        *value,
                        now_real + connection::DEFAULT_HOLD_SECS,
                    );
                }
                // A landing write retracts any earlier fault for this port —
                // the backend may have come up late (model load order), which
                // is not an authoring error — and RECORDS the proof, exactly as
                // the wire master does. Recording is what makes the retraction
                // stick: the `landed` check below is the only thing that stops a
                // port from being re-reported the next time a write happens to
                // drop (a backend momentarily absent mid-reload), and a port
                // proven by `SetPorts` was not in that set at all, so its fault
                // could come back after being cleared.
                let diag = world.resource::<diagnostics::CosimDiagnostics>();
                let key = (target, port.clone());
                if !diag.faults.is_empty() || !diag.landed.contains(&key) {
                    let mut diag = world.resource_mut::<diagnostics::CosimDiagnostics>();
                    diag.faults.remove(&key);
                    diag.landed.insert(key);
                }
                continue;
            }
            // M12: the write dropped. Same triage as the wire master's
            // (`propagate_connections`): an entity exposing NO ports at all is a
            // structural or still-loading endpoint — load order, not a fault —
            // while an entity that has ports but not THIS name is the genuine
            // case (a typo'd port from the API/script/autopilot). Ledger entry
            // deduped per `(entity, port)`, exactly like the wiring faults, and
            // never re-asserted over a port proven to have landed.
            let has_port_surface = !reg.entity_ports(world, target).is_empty();
            if !has_port_surface {
                continue;
            }
            let global_id = world.get::<lunco_core::GlobalEntityId>(target).copied();
            // `Name` carries the USD prim path (the reader stamps it on every prim
            // entity). An entity id in a warning — `1277v0` — is unactionable in a
            // tester's log; `/SandboxScene/Avatar` is a bug report.
            let label = world
                .get::<Name>(target)
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("{target:?}"));
            let mut diag = world.resource_mut::<diagnostics::CosimDiagnostics>();
            let key = (target, port.clone());
            if diag.landed.contains(&key) {
                continue;
            }
            if let std::collections::hash_map::Entry::Vacant(e) = diag.faults.entry(key) {
                warn!(
                    "[cosim] SetPorts targets unknown input port '{}' on {} ({:?}) — value \
                     dropped (declare the port or fix the caller)",
                    port, label, target
                );
                e.insert(diagnostics::BrokenConnection {
                    entity: target,
                    global_id,
                    port: port.clone(),
                    has_port_surface: true,
                    dropped_value: *value,
                });
            }
        }
    });
}

register_commands!(on_set_ports);
