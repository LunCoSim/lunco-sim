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

        // Pin the two wiring fabrics in a deterministic order. The hardware DAC
        // (`lunco_core::ControlDacSet` → `wire_system`: `DigitalPort` →
        // `PhysicalPort.value`) writes values the cosim resolver now exposes as
        // `"value"` ports, so a `SimConnection` can read a `PhysicalPort` the DAC
        // just drove. Force the DAC to run BEFORE propagate so that read sees
        // *this* tick's DAC output, not last tick's — otherwise the relative
        // order is unspecified, giving a 1-tick skew that varies frame-to-frame
        // and diverges host vs client under prediction (the same class of bug
        // that `ControlDacSet`-on-the-fixed-clock already fixed once). Vacuous
        // when `wire_system` isn't registered (the set is simply empty).
        app.configure_sets(
            FixedUpdate,
            lunco_core::ControlDacSet.before(systems::propagate::CosimSet::Propagate),
        );

        // Diagnostic: a `PhysicalPort` must be driven by ONE fabric, not both.
        // Runs only on frames where new wiring appeared (see `any_new_wiring`).
        app.add_systems(
            Update,
            warn_dual_driven_ports.run_if(any_new_wiring),
        );

        // Server-authoritative networking: a pure client must NOT run cosim on
        // replicated objects — it renders host snapshots. Running cosim here
        // would fight the snapshot (objects drift/jitter when the server is
        // briefly static). Gated off on `NetworkRole::Client`; host + single-
        // player run it normally.
        app.add_systems(
            FixedUpdate,
            (
                systems::propagate::propagate_connections.in_set(systems::propagate::CosimSet::Propagate),
                // The single avian force consumer: drains `PendingForces` (filled
                // by propagation's `force_*` writes) into avian's `Forces`. Joint
                // motors are driven inline by the `angle` input port's write
                // closure during propagation, so no separate joint-drive system.
                avian::apply_pending_forces.in_set(systems::apply_forces::CosimSet::ApplyForces),
            )
                .run_if(|role: Option<Res<lunco_core::NetworkRole>>| {
                    // Absent role (single-player, headless tests) → run cosim.
                    // Only a present `Client` role gates it off.
                    !matches!(role.as_deref(), Some(lunco_core::NetworkRole::Client))
                }),
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
/// - a wheeled rover exposes `throttle`/`steer`/`brake` (its `FlightSoftware`
///   command surface, via the FSW command backend); a mix system projects them
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
/// inputs, a `FlightSoftware`'s command inputs (throttle/steer/brake, …),
/// `PhysicalPort`/`DigitalPort` registers, or any future backend, all by name.
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
///    `.before(lunco_core::ControlDacSet)` explicitly, so the `SetPorts` they
///    emit is flushed — and the `DigitalPort` written — before the DAC
///    propagates it into `PhysicalPort` and the wheel systems read it. Without
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

/// Run condition: did any wiring (a [`SimConnection`] or a hardware
/// [`lunco_core::architecture::Wire`]) get added this frame?
///
/// [`warn_dual_driven_ports`] only needs to re-scan when the wire set changes,
/// so this gates it off on the steady-state frames (the overwhelming majority).
fn any_new_wiring(
    new_conns: Query<(), Added<SimConnection>>,
    new_wires: Query<(), Added<lunco_core::architecture::Wire>>,
) -> bool {
    !new_conns.is_empty() || !new_wires.is_empty()
}

/// Diagnostic: warn when a [`lunco_core::architecture::PhysicalPort`] is driven
/// by **both** wiring fabrics.
///
/// The hardware DAC (`wire_system`, [`lunco_core::architecture::Wire`] →
/// `PhysicalPort.value`) and a cosim [`SimConnection`] targeting that same port
/// (`end_connector == "value"`) are two writers of one slot, with *different*
/// scale semantics — the DAC normalizes `i16/32767 * scale`, the connection
/// applies the affine `src*scale + offset` in raw units. The last writer in
/// schedule order wins, and on a client the DAC runs while cosim is gated off
/// (server-authoritative), so the winner differs host vs client. That is a
/// scene-authoring error, not something to silently resolve — we surface it
/// loudly, once per offending port.
fn warn_dual_driven_ports(
    q_wires: Query<&lunco_core::architecture::Wire>,
    q_conns: Query<&SimConnection>,
    mut warned: Local<std::collections::HashSet<Entity>>,
) {
    for conn in q_conns.iter() {
        // Only the `PhysicalPort` "value" slot is a write-write hazard: the DAC
        // writes it, and so does a connection naming it. (A connection driving a
        // `DigitalPort` "raw" register that a `Wire` then *reads* is a legal
        // chain, not a conflict.)
        if conn.end_connector != PHYSICAL_PORT_NAME {
            continue;
        }
        let target = conn.end_element;
        if target == Entity::PLACEHOLDER || warned.contains(&target) {
            continue;
        }
        if q_wires.iter().any(|w| w.target == target) {
            warn!(
                "[cosim] PhysicalPort on {:?} is driven by BOTH a hardware Wire \
                 (normalized DAC) and a SimConnection ('{}', affine) — two writers \
                 of one slot. Last-in-schedule wins and host/client diverge under \
                 prediction. Drive it from one fabric only.",
                target, PHYSICAL_PORT_NAME
            );
            warned.insert(target);
        }
    }
}
