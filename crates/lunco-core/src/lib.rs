//! Core types and plugins for the LunCo simulation.
//!
//! This crate provides the foundational components, resources, and systems used
//! across the simulation, including physical properties, celestial timing,
//! and the core plugin registration.

pub mod architecture;
pub mod mocks;
pub mod telemetry;
pub mod coords;
pub mod log;
/// Architectural marker components for the big_space integration.
pub mod markers;
/// Atomic re-parenting helpers for SOI/Grid migration.
pub mod attach;
/// Debug-build invariant checks for the big_space hierarchy.
pub mod invariants;
/// Unified diagram data model — pure Rust, no Bevy dependency.
pub mod diagram;
/// Shared 53-bit time-sorted id generator backing `GlobalEntityId`
/// and `commands::OpId`.
pub mod ids;
/// M1 — deterministic identity from `Provenance`. The only place network
/// ids are *derived*; the assignment system below is the only place they
/// are *minted*.
pub mod identity;
/// Command envelope — `Mutation<P>`, `Ack`, `Reject`, `Replication`.
/// The shape every locally- or remotely-originated mutation flows
/// through.
pub mod commands;

pub use architecture::*;
pub use mocks::*;
pub use telemetry::*;
pub use log::*;
pub use commands::{Ack, Mutation, OpId, Reject, Replication, SessionId};
pub use markers::{GridAnchor, SoiMigrant};
pub use invariants::BigSpaceInvariantsPlugin;
pub use identity::Provenance;

// ── Typed Command Macros ──────────────────────────────────────────────────────
//
// Import these in your crate for clean usage:
//   use lunco_core::{Command, on_command, register_commands};
//
// #[Command]
//   → struct becomes #[derive(Event, Reflect, Clone, Debug)]
//
// #[on_command(StructName)]
//   → fn wrapped with On<T>, generates __register_<fn>(app)
//
// register_commands!(fn_a, fn_b)
//   → generates pub fn register_all_commands(app) that wires everything up

pub use lunco_command_macro::{Command, on_command, register_commands};

/// Re-exported `serde` so the `#[Command]` proc-macro can reference it
/// via an absolute path (`::lunco_core::serde::*`). Crates using
/// `#[Command]` do not need their own `serde` dependency — they get it
/// transitively through `lunco-core`.
pub use serde;

use bevy::prelude::*;

/// The central plugin for the LunCo simulation core.
///
/// Registers all core types for reflection and initializes essential systems
/// like the physical/digital port wiring.
pub struct LunCoCorePlugin;

/// Stable identity for entities across the simulation and API.
///
/// A **53-bit** identifier, safe as a raw Number in JavaScript/JSON without
/// precision loss. Ids are no longer minted ad-hoc: the field is **private**
/// and there is no public `new()`/`Default`. An id is produced in exactly one
/// of two ways, both routed through [`assign_global_entity_ids`]:
/// - **derived** from [`Provenance`] (Content/Derived) — deterministic, same on
///   every peer, no coordination;
/// - **server-allocated** ([`Provenance::Authoritative`]) via [`crate::ids`],
///   then replicated down.
///
/// [`from_raw`](Self::from_raw) reconstructs an id from a value already known
/// (the API boundary resolving a wire `u64`, deserialization) — it does not
/// *mint*.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect, serde::Serialize, serde::Deserialize)]
#[reflect(Component)]
pub struct GlobalEntityId(u64);

impl GlobalEntityId {
    /// Read the raw 53-bit value (e.g. to put on the wire or into JSON).
    pub fn get(&self) -> u64 {
        self.0
    }

    /// Reconstruct an id from a value that already exists — a wire/JSON `u64`
    /// the API layer is resolving back to an [`Entity`], or serde. This is
    /// *reconstruction*, not minting: callers must not pass freshly-invented
    /// numbers here (attach a [`Provenance`] and let the assignment system mint).
    pub fn from_raw(v: u64) -> Self {
        Self(v)
    }

    /// Server-only mint for [`Provenance::Authoritative`] entities. Wraps
    /// [`crate::ids::make_id_53`]; crate-internal so the assignment system is
    /// the sole caller.
    pub(crate) fn allocate_authoritative() -> Self {
        Self(crate::ids::make_id_53())
    }
}

impl std::fmt::Display for GlobalEntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for GlobalEntityId {
    type Err = std::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>().map(GlobalEntityId)
    }
}

/// Marker component for the user's active avatar/entity in the simulation.
#[derive(Component)]
pub struct Avatar;

/// Defines a spacecraft entity with its ephemeris and physical constraints.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct Spacecraft {
    /// Human-readable name of the spacecraft.
    pub name: String,
    /// ID used for ephemeris lookups (e.g., SPICE ID).
    pub ephemeris_id: i32,
    /// Reference body ID (e.g., Earth, Moon).
    pub reference_id: i32,
    /// Start of valid data range in Julian Date.
    pub start_epoch_jd: Option<f64>,
    /// End of valid data range in Julian Date.
    pub end_epoch_jd: Option<f64>,
    /// Collision/interaction radius for simple math-based proximity checks.
    pub hit_radius_m: f32,
    /// Whether this spacecraft should be rendered and listed in the UI.
    pub user_visible: bool,
}

/// Marker component for generic vessels.
#[derive(Component)]
pub struct Vessel;

/// Marker component specifically for surface exploration rovers.
#[derive(Component, Clone, Copy, Reflect, Default)]
#[reflect(Component, Default)]
pub struct RoverVessel;

/// Marker component indicating an entity can be selected as a root object
/// in editing tools (e.g., rover bodies, props, ramps, solar panels).
///
/// Child entities like wheels, colliders, and visuals do NOT have this marker,
/// preventing them from being independently selected. Selection systems should
/// query for this component rather than filtering by name strings.
#[derive(Component)]
pub struct SelectableRoot;

/// Marker component for terrain/ground entities that should be excluded
/// from vessel possession and editing interactions.
#[derive(Component)]
pub struct Ground;

/// Physical properties used for gravity, collision, and mass-based calculations.
///
/// These properties use double precision (`f64`) to maintain simulation integrity
/// over astronomical scales as mandated by the project constitution.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct PhysicalProperties {
    /// Radius of the body in meters.
    pub radius_m: f64,
    /// Mass of the body in kilograms.
    pub mass_kg: f64,
}

/// Represents a major celestial body (planet, moon, asteroid) in the simulation.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct CelestialBody {
    /// Name of the celestial body.
    pub name: String,
    /// Unique identifier for ephemeris data retrieval.
    pub ephemeris_id: i32,
    /// Mean radius in meters, used for rendering and approximate physics.
    pub radius_m: f64,
}

/// Global simulation speed and physics state control.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct TimeWarpState {
    /// Multiplier for simulation time (e.g., 2.0 = 2x speed).
    pub speed: f64,
    /// Whether the physics engine should be active (paused during warp).
    pub physics_enabled: bool,
}

/// Marker resource indicating that entity dragging is active.
///
/// Used by sandbox editing systems to signal other systems (like avatar possession)
/// to disable conflicting interactions during drag operations.
#[derive(Resource)]
pub struct DragModeActive {
    /// Whether dragging is currently active.
    pub active: bool,
}

impl Default for DragModeActive {
    fn default() -> Self {
        Self { active: false }
    }
}

/// Marker resource indicating a click-to-place spawn tool is armed.
///
/// Set by sandbox-edit's spawn placement system whenever `SpawnState`
/// is `Selecting`. Read by avatar possession to suppress vessel
/// possession on the placement click.
#[derive(Resource, Default)]
pub struct SpawnToolActive(pub bool);

/// Represents the current "wall clock" time in the simulation universe.
///
/// Uses Julian Date for astronomical precision and provides a mechanism
/// for non-linear time progression.
#[derive(Resource, Debug, Clone, Copy, Reflect)]
#[reflect(Resource)]
pub struct CelestialClock {
    /// Current Julian Date (TDB - Terrestrial Dynamic Time).
    pub epoch: f64,
    /// Multiplier relative to real-time progression.
    pub speed_multiplier: f64,
    /// Pause state for the simulation clock.
    pub paused: bool,
}

impl Default for CelestialClock {
    fn default() -> Self {
        Self {
            epoch: 2451545.0, // J2000.0
            speed_multiplier: 1.0,
            paused: false,
        }
    }
}

/// Monotonic discrete **simulation tick** — the netcode time substrate (M6).
///
/// `CelestialClock`/`TimeWarpState` give *continuous* sim time + warp; netcode
/// also needs a monotonic integer counter that prediction, rollback,
/// input-stamping and the shared clock all key off. Advanced once per
/// `FixedUpdate` step (see [`advance_sim_tick`]). Warp-independent: warp scales
/// `dt`, not the tick count, so peers can compare ticks directly. Not yet
/// consumed anywhere — it's the substrate the networking layer (Ph3/Ph4) drives.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq, Reflect,
         serde::Serialize, serde::Deserialize)]
#[reflect(Resource)]
pub struct SimTick(pub u64);

impl SimTick {
    /// Signed tick distance `self - other`, wrapping-safe.
    pub fn wrapping_diff(self, other: SimTick) -> i64 {
        self.0.wrapping_sub(other.0) as i64
    }
}

/// Does this process mint [`Provenance::Authoritative`] ids? One bool, set once
/// at startup; the networking layer (Ph1+) owns *how* it's set (host/server =
/// `true`, pure client = `false`). Single-process today ⇒ `true` ⇒ behavior
/// matches the pre-Ph1 "everything gets an id" world, except `Local`-tagged
/// entities now correctly opt out and Content/Derived become deterministic.
#[derive(Resource, Clone, Copy, Debug)]
pub struct IsServer(pub bool);

impl Default for IsServer {
    fn default() -> Self {
        Self(true)
    }
}

impl Plugin for LunCoCorePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LunCoLogPlugin);
        app.add_plugins(BigSpaceInvariantsPlugin);
        app.register_type::<GridAnchor>()
           .register_type::<SoiMigrant>()
           .register_type::<Severity>()
           .register_type::<TelemetryValue>()
           .register_type::<TelemetryEvent>()
           .register_type::<Parameter>()
           .register_type::<SampledParameter>()
           .register_type::<CelestialClock>()
           .register_type::<UserIntent>()
           .register_type::<IntentAnalogState>()
           .register_type::<PhysicalPort>()
           .register_type::<DigitalPort>()
           .register_type::<Wire>()
           .register_type::<PhysicalProperties>()
           .register_type::<CelestialBody>()
           .register_type::<Spacecraft>()
           .register_type::<ActiveAction>()
           .register_type::<ActionStatus>()
           .register_type::<GlobalEntityId>()
           .register_type::<Provenance>()
           .register_type::<SimTick>()
           .init_resource::<SimTick>()
           .init_resource::<IsServer>()
           .add_systems(Update, wire_system)
           .add_systems(FixedUpdate, advance_sim_tick)
           .add_systems(PostUpdate, assign_global_entity_ids);
    }
}

/// Advance the discrete [`SimTick`] once per fixed step, *only while physics is
/// actually running* (so a paused/zero-speed world freezes the tick and peers
/// stay comparable). `TimeWarpState` is read optionally: if a binary hasn't
/// inserted it, we treat the world as running and advance.
fn advance_sim_tick(mut tick: ResMut<SimTick>, warp: Option<Res<TimeWarpState>>) {
    let running = warp.map_or(true, |w| w.physics_enabled && w.speed > 0.0);
    if running {
        tick.0 = tick.0.wrapping_add(1);
    }
}

/// The **only** place [`GlobalEntityId`]s are minted. [`Provenance`] decides how:
/// Content/Derived → deterministic hash (same on every peer); Authoritative →
/// server-allocated (clients receive it via replication); Local → no id at all.
///
/// **Migration (safe/incremental):** entities not yet tagged with a `Provenance`
/// keep the pre-Ph1 behavior — an auto-allocated id — and we `warn!` once. This
/// lands the machinery with zero day-one breakage; spawners get migrated to
/// attach `Provenance` over time, after which the fallback arm can flip to a
/// hard skip.
fn assign_global_entity_ids(
    mut commands: Commands,
    q_new: Query<(Entity, Option<&Provenance>), Without<GlobalEntityId>>,
    is_server: Res<IsServer>,
    mut warned: Local<bool>,
) {
    for (entity, prov) in q_new.iter() {
        match prov {
            Some(Provenance::Local) => { /* never networked, no id */ }
            Some(p @ (Provenance::Content { .. } | Provenance::Derived { .. })) => {
                if let Some(id) = identity::derive_id(p) {
                    commands.entity(entity).insert(GlobalEntityId::from_raw(id));
                }
            }
            Some(Provenance::Authoritative) => {
                // Only the server mints; clients receive the id via replication.
                if is_server.0 {
                    commands
                        .entity(entity)
                        .insert(GlobalEntityId::allocate_authoritative());
                }
            }
            None => {
                // Untagged entity — preserve pre-Ph1 behavior (auto-allocate),
                // warn once. Migrate spawners to attach `Provenance` to opt into
                // deterministic identity / Local opt-out.
                if !*warned {
                    warn!(
                        "entity without `Provenance` got an auto-allocated \
                         GlobalEntityId (Ph1 migration fallback). Tag spawners \
                         with a Provenance to opt into deterministic identity."
                    );
                    *warned = true;
                }
                commands
                    .entity(entity)
                    .insert(GlobalEntityId::allocate_authoritative());
            }
        }
    }
}

/// Syncs digital port values to physical actuators/sensors through wires.
///
/// This system bridges the gap between discrete digital control (i16) and
/// continuous physical forces (f32).
fn wire_system(
    q_wires: Query<&Wire>,
    q_digital: Query<&DigitalPort>,
    mut q_physical: Query<&mut PhysicalPort>,
) {
    for wire in q_wires.iter() {
        if let Ok(digital) = q_digital.get(wire.source) {
            if let Ok(mut physical) = q_physical.get_mut(wire.target) {
                // Normalize i16 (-32768..32767) to -1.0..1.0 approximately, then apply scale
                physical.value = (digital.raw_value as f32 / 32767.0) * wire.scale;
            }
        }
    }
}

#[cfg(test)]
mod ph1_identity_tests {
    //! Ph1 Bevy-wiring layer over the pure logic already proven in
    //! `lunco-networking/proto-tests`. Runs on a bare headless `App` (no
    //! rendering, no backend) — we invoke the schedules directly so no time
    //! plumbing is needed.
    use super::*;

    /// App with just the Ph1 systems + resources, nothing else.
    fn ph1_app(is_server: bool) -> App {
        let mut app = App::new();
        app.insert_resource(IsServer(is_server))
            .init_resource::<SimTick>()
            .add_systems(FixedUpdate, advance_sim_tick)
            .add_systems(PostUpdate, assign_global_entity_ids);
        app
    }

    fn id_of(app: &mut App, e: Entity) -> Option<u64> {
        app.world().get::<GlobalEntityId>(e).map(GlobalEntityId::get)
    }

    #[test]
    fn content_entity_gets_deterministic_id() {
        let mut app = ph1_app(true);
        let prov = identity::content("usd", "scene.usda", "/World/Rover");
        let expected = identity::derive_id(&prov).unwrap();
        let e = app.world_mut().spawn(prov).id();
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(id_of(&mut app, e), Some(expected));
    }

    #[test]
    fn local_entity_gets_no_id() {
        let mut app = ph1_app(true);
        let e = app.world_mut().spawn(Provenance::Local).id();
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(id_of(&mut app, e), None);
    }

    #[test]
    fn authoritative_minted_only_on_server() {
        // Pure client: no id.
        let mut client = ph1_app(false);
        let ce = client.world_mut().spawn(Provenance::Authoritative).id();
        client.world_mut().run_schedule(PostUpdate);
        assert_eq!(id_of(&mut client, ce), None);

        // Server: id present.
        let mut server = ph1_app(true);
        let se = server.world_mut().spawn(Provenance::Authoritative).id();
        server.world_mut().run_schedule(PostUpdate);
        assert!(id_of(&mut server, se).is_some());
    }

    #[test]
    fn derived_id_matches_parent_role() {
        let mut app = ph1_app(true);
        let parent_prov = identity::content("usd", "scene.usda", "/World/Rover");
        let parent_id = identity::derive_id(&parent_prov).unwrap();
        let child_prov = Provenance::Derived {
            parent: parent_id,
            role: "wheel.fl".into(),
        };
        let expected = identity::derive_id(&child_prov).unwrap();
        let child = app.world_mut().spawn(child_prov).id();
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(id_of(&mut app, child), Some(expected));
    }

    #[test]
    fn sim_tick_advances_under_run_paused_does_not() {
        let mut app = ph1_app(true);

        // Running world: tick advances each fixed step.
        app.insert_resource(TimeWarpState {
            speed: 1.0,
            physics_enabled: true,
        });
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<SimTick>().0, 1);
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<SimTick>().0, 2);

        // Paused: tick frozen.
        app.insert_resource(TimeWarpState {
            speed: 0.0,
            physics_enabled: false,
        });
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<SimTick>().0, 2);
    }
}
