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
pub mod component;
pub mod joint;
pub mod ports;
pub mod sensors;
pub mod suggestion;
pub mod systems;
pub mod connection;

pub use avian::*;
pub use component::*;
pub use joint::*;
pub use ports::*;
pub use suggestion::*;
pub use connection::*;

// Typed-command machinery (re-exported from `lunco-core`, which re-exports
// the `lunco-command-macro` proc-macros). Used by the `SetPorts` command +
// observer defined below — the ONE generic vessel-control command (a batch of
// named input-port writes), driving landers, rovers, and any port-bearing vessel.
use lunco_core::{Command, on_command, register_commands};

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
        app.init_resource::<lunco_core::ports::PortRegistry>();
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
                systems::propagate::propagate_connections.in_set(systems::propagate::CosimSet::Propagate),
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
                    .run_if(lunco_physics::physics_is_live)
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
/// - a wheeled rover exposes `throttle`/`steer`/`brake` (its `CommandInputs`
///   command surface, via the command-input backend); a mix system projects them
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
    /// silently ignored by `PortRegistry` (strict per-backend), so a binding may
    /// name ports a given vessel doesn't have without error.
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
/// inputs, a `CommandInputs` command surface (throttle/steer/brake, …),
/// hardware `Port`s, or any future backend, all by name.
/// `write_port` needs `&mut World`, so we clone the (cheap, `fn`-pointer)
/// registry and defer the writes through a `Commands` world closure.
///
/// TODO(P9): the control-path latency "input at tick N → wheels at tick N" is
/// currently an ACCIDENT OF SCHEDULING, not a declared edge. Two halves:
///
/// 1. **Producer ordering (not in this crate).** `drive_from_bindings`
///    (`lunco-controller`) and `drive_autopilots` (`lunco-autopilot`) are added
///    to `FixedUpdate` with NO ordering relative to
///    [`lunco_core::ControlDacSet`]. They must be
///    `.before(lunco_core::ControlDacSet)` explicitly, so the `SetPorts`
///    they emit is flushed — and the source `Port` written — before propagation
///    carries it across the `Wire` and the wheel systems read it. Without
///    that edge, adding any unrelated `.after()` anywhere in the fixed graph can
///    silently move the actuation a whole tick.
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
        for (port, value) in &writes {
            reg.write_port(world, target, port, *value);
        }
    });
}

register_commands!(on_set_ports);

