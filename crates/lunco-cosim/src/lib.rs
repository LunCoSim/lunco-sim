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
//! [`SimConnection`] connects any output to any input, following the FMI/SSP pattern.
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

use std::collections::HashMap;

use bevy::prelude::*;
use avian3d::prelude::PhysicsSystems;

pub mod avian;
pub mod component;
pub mod suggestion;
pub mod systems;
pub mod connection;

pub use avian::*;
pub use component::*;
pub use suggestion::*;
pub use connection::*;

/// Plugin for co-simulation orchestration.
///
/// Registers [`SimComponent`], [`AvianSim`], and [`SimConnection`] types,
/// and adds systems for wire propagation and Avian manual stepping.
///
/// ## Usage
///
/// ```rust,ignore
/// app.add_plugins(CoSimPlugin);
/// ```
///
/// Engine plugins (e.g., `lunco-modelica`) depend on this crate and
/// create [`SimComponent`] instances when models compile.
pub struct CoSimPlugin;

impl Plugin for CoSimPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<SimComponent>()
            .register_type::<AvianSim>()
            .register_type::<SimConnection>();

        app.add_observer(on_add_rigid_body);
        app.add_observer(on_add_rigid_body_forces);

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

        app.add_systems(
            FixedUpdate,
            (
                systems::propagate::propagate_connections.in_set(systems::propagate::CosimSet::Propagate),
                systems::apply_forces::apply_sim_forces.in_set(systems::apply_forces::CosimSet::ApplyForces),
            ),
        );

        // Read Avian outputs AFTER Avian's Writeback (Position → Transform sync).
        // This ensures height/velocity values reflect the physics step that just ran.
        app.add_systems(
            FixedPostUpdate,
            systems::step_avian::read_avian_outputs.after(PhysicsSystems::Writeback),
        );

        app.add_systems(Update, systems::collider::sync_collider);
    }
}

/// Observer: auto-adds [`AvianSim`] to any entity that gets a [`RigidBody`].
///
/// This makes Avian available as a co-simulation model alongside
/// any other model (Modelica, FMU, GMAT) on the same entity.
pub fn on_add_rigid_body(
    trigger: On<Add, avian3d::prelude::RigidBody>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let mut avian = AvianSim {
        inputs: HashMap::default(),
        outputs: HashMap::default(),
    };
    avian.init_outputs();
    commands.entity(entity).try_insert(avian);
}

/// Observer: auto-adds [`Forces`] to any entity that gets a [`RigidBody`].
///
/// This is required for [`apply_sim_forces`] to work — Avian only creates the
/// `Forces` component lazily on first access, but the co-simulation bridge
/// needs it present before the physics step.
pub fn on_add_rigid_body_forces(
    trigger: On<Add, avian3d::prelude::RigidBody>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    // SAFETY: Forces is a zero-sized query data type. try_insert only adds it
    // if absent (won't overwrite), and the observer fires exactly once per entity.
    commands.entity(entity).try_insert(());
}
