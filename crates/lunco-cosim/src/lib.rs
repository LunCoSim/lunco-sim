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
pub mod suggestion;
pub mod systems;
pub mod connection;

pub use avian::*;
pub use component::*;
pub use joint::*;
pub use ports::*;
pub use suggestion::*;
pub use connection::*;

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
            .register_type::<SimConnection>();

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

        app.add_systems(Update, systems::collider::sync_collider);
    }
}

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
