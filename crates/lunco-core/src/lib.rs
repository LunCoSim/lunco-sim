//! Core types and plugins for the LunCo simulation.
//!
//! This crate provides the dependency-light engine substrate: foundational
//! components, resources, typed commands, scene lifecycle, and core plugin
//! registration. BigSpace topology and coordinate conversion live in
//! [`lunco_spatial`], so consumers that need only engine semantics do not
//! compile the spatial implementation.

// The shared `#[Command]` macro addresses this crate through
// `::lunco_core`, including when it expands here. This self-alias keeps core
// commands on exactly the same macro/reflection path as plugin commands.
extern crate self as lunco_core;

/// Runtime command markers, reflection, and result storage.
/// The shape every locally- or remotely-originated mutation flows
/// through.
pub mod commands;
pub mod faults;
/// M1 — deterministic identity from `Provenance`. The only place network
/// ids are *derived*; the session identity-admission system is the only place they
/// are *minted*.
pub mod identity;
/// Shared semantic labels for UI, API, and scripting presentation.
pub mod labels;
/// Architectural marker components shared by engine subsystems.
pub mod markers;
pub mod physics_state;
pub mod programs;
/// M4 — pure predict-own reconciliation decision (input-replay, D2). The
/// dependency-free geometry the spawn-domain `reconcile_owned_prediction` system
/// applies; unit-tested without the avian/render build.
pub mod reconcile;
/// Typed requests and lifecycle edges for scene ownership and transitions.
pub mod scene;
pub mod scene_lifecycle;
/// Recoverable locking for shared process state.
pub mod sync;

pub mod derived;

/// Domain-free named engine exposure snapshots for UI, API, and diagnostics.
pub mod exposure;

pub mod events;

pub mod mobility;
/// Generic authored-model invalidation shared by all backend adapters.
pub mod model_state;
pub mod tools;

pub use commands::{
    ActiveCommandId, ApiCommandMarker, ClientCommandPolicy, CommandOutcome, CommandResults,
    EditIntent, MarkClientLocalExt, SpawnEntity,
};
pub use derived::RebuildOnChange;
pub use events::{trigger_runtime_error, CommandOccurred, RuntimeError, SubsystemStateChanged};
pub use faults::{
    DiagnosticSeverity, RuntimeDiagnostic, RuntimeDiagnostics, RuntimeFault, RuntimeFaults,
};
pub use identity::Provenance;
pub use labels::{entity_display_name, humanize_identifier};
pub use markers::NoSelectionBounds;
pub use markers::{
    CatalogEntryId, EmbeddedScenarioPath, EmbeddedScenarioSource, HorizonShadowTerrain,
    PhysicsPoseAuthoritative, ScenarioProgramPrim, ScriptParams, SunAngularDiameter, TriggerZone,
    UsdPrimKind, CELESTIAL_COLLISION_LAYER, NON_PHYSICAL_QUERY_LAYERS, SOLAR_ANGULAR_DIAMETER_DEG,
    TRIGGER_COLLISION_LAYER,
};
pub use mobility::Mobility;
pub use model_state::ModelStateRevision;
pub use physics_state::*;
pub use reconcile::{reconcile_decision, ReconcileParams, Reconciliation};
pub use scene::{
    SceneTransition, SceneTransitionAdmission, SceneTransitionAdmitted, SceneTransitionCompleted,
    SceneTransitionCoordinator, SceneTransitionFailed, SceneTransitionIntent,
    SceneTransitionRequest, SceneTransitionStarted,
};
pub use scene_lifecycle::{run_scene_teardown, SceneMountState, SceneTeardown};
pub use sync::LockExt;

// ── Typed Command Macros ──────────────────────────────────────────────────────
//
// Import these in your crate for clean usage:
//   use lunco_core::{Command, on_command, register_commands};
//
// #[Command]
//   → struct becomes #[derive(Event, Reflect, Clone, Debug)]
//
// #[on_command(StructName)]
//   → fn wrapped with On<T>; emits an internal registration helper
//     (don't call it by hand — list the observer below)
//
// register_commands!(fn_a, mod::fn_b)
//   → generates pub fn register_all_commands(app) that wires every
//     listed observer up. Entries may be bare idents or module paths.

pub use lunco_command_macro::{on_command, register_commands, Command};

/// Re-exported `serde` so the `#[Command]` proc-macro can reference it
/// via an absolute path (`::lunco_core::serde::*`). Crates using
/// `#[Command]` do not need their own `serde` dependency — they get it
/// transitively through `lunco-core`.
pub use serde;

use bevy::prelude::*;

/// Stable identity for entities across the simulation and API.
///
/// A **53-bit** identifier, safe as a raw Number in JavaScript/JSON without
/// precision loss. Ids are no longer minted ad-hoc: the field is **private**
/// and there is no public `new()`/`Default`. An id is produced in exactly one
/// of two ways, both admitted by the session-layer identity system:
/// - **derived** from [`Provenance`] (Content/Derived) — deterministic, same on
///   every peer, no coordination;
/// - **server-allocated** ([`Provenance::Authoritative`]) via `lunco-id`,
///   then replicated down.
///
/// [`from_raw`](Self::from_raw) reconstructs an id from a value already known
/// (the API boundary resolving a wire `u64`, deserialization) — it does not
/// *mint*.
#[derive(
    Component,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Reflect,
    serde::Serialize,
    serde::Deserialize,
)]
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
    /// numbers here (attach a [`Provenance`] and let the session identity-admission system mint).
    pub fn from_raw(v: u64) -> Self {
        Self(v)
    }

    /// Server-only mint for [`Provenance::Authoritative`] entities. The
    /// `lunco-core-session` identity-admission system is the sole production
    /// owner that calls this boundary.
    pub fn allocate_authoritative() -> Self {
        Self(lunco_id::make_id_53())
    }
}

impl std::fmt::Display for GlobalEntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Field marker for the wire codec: an `Entity` field tagged with this is a
/// **local-only** reference (e.g. a peer's camera avatar) that must never carry
/// real local entity bits onto the wire — the codec substitutes
/// `Entity::PLACEHOLDER` instead of globalizing it. Attach it on a `#[Command]`
/// field via `#[sync_local]` (sugar that expands to
/// `#[reflect(@::lunco_core::SyncLocal)]`); the codec reads it back with
/// `NamedField::has_attribute::<SyncLocal>()`. Derives `Reflect` because
/// reflect custom-attribute values must be `Reflect + 'static`.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncLocal;

/// Field marker for host authorization: the gid field a networked command is
/// checked for ownership against (e.g. `SetPorts.target`). The wire apply
/// path finds it via `has_attribute::<AuthzTarget>()` to read which global id
/// to authorize, instead of hardcoding a `"target"` field name. Attach via
/// `#[authz_target]` on a `#[Command]` field. Derives `Reflect` because reflect
/// custom-attribute values must be `Reflect + 'static`.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuthzTarget;

impl std::str::FromStr for GlobalEntityId {
    type Err = std::num::ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>().map(GlobalEntityId)
    }
}

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

// NOTE: there is intentionally NO `Vessel` / `RoverVessel` / `LanderVessel`
// marker. "Possessable / controllable" is derived from TOPOLOGY: an entity is
// controllable iff it exposes writable control ports — an authored command
// surface, declared by its authored `Controls` scope (→ a control binding),
// or a Modelica `SimComponent`. The components a body already carries ARE its
// definition; possession, control routing, prediction membership, and UI
// labels read those capabilities directly instead of a redundant taxonomy tag.

/// Marker component indicating an entity can be selected as a root object
/// in editing tools (e.g., rover bodies, props, ramps, solar panels).
///
/// Child entities like wheels, colliders, and visuals do NOT have this marker,
/// preventing them from being independently selected. Selection systems should
/// query for this component rather than filtering by name strings.
#[derive(Component)]
pub struct SelectableRoot;

/// Marks the topology-derived root of a mobility realization.
///
/// This shared capability lets mobility, networking, and USD projection agree
/// on the vehicle owner without overloading an actuator-value registry. It is
/// not a vehicle taxonomy: the applied USD mobility schema is the authority,
/// while produced port values remain a separate surface.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct MobilityRoot;

/// Marker component for terrain/ground entities that should be excluded
/// from vessel possession and editing interactions.
#[derive(Component)]
pub struct Ground;

/// Marker for entities a *system* owns and churns: streamed terrain LOD tiles,
/// the collider ring, scattered rocks — spawned and despawned continuously as
/// the camera moves, and never authored by a user.
///
/// Author-facing lists (the Entity list, pickers) hide these by default: they
/// are runtime detail, not scene content, and there can be hundreds live at
/// once. Query for this marker rather than matching on generated names
/// (`"LodTile d3 4,7"`) — names are display text and will drift.
#[derive(Component)]
pub struct SystemManaged;

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

/// **The** lunar radius, in metres. Every place that needs "how big is the
/// Moon" — body placement, colliders, ground-relative altitude, the body-radius
/// citation stamped into a baked GeoTIFF — refers to this.
///
/// Source: IAU/WGCCRE mean radius of the Moon, 1737.4 km
/// (Archinal et al., *Report of the IAU Working Group on Cartographic
/// Coordinates and Rotational Elements*). The same value the LOLA/LRO products
/// the terrain pipeline ingests are referenced to.
///
/// It exists because the tree carried three disagreeing values (`1737.0e3`,
/// `1.7374e6` in the sandbox UI, `1_737_400.0` in tests) — a 400 m spread, i.e.
/// a real altitude/georeferencing bias the moment anything reports a height.
/// Do not re-type the number; a fourth copy is the bug coming back.
///
/// It is LOAD-BEARING for site placement, not just visuals:
/// `geodetic_to_body_fixed` derives the site anchor from it and the DEM frame
/// contract ties that anchor to the baked grid, so perturbing it desyncs the anchor
/// from the baked DEM (shrinking it by 10 km to lower the rendered globe moved the
/// anchor ~772 m and tripped the frame check on load). To move the globe shell,
/// offset the GLOBE RENDER radius (see `lunco_celestial_spatial::globe_lod`), never this.
///
/// It lives HERE, in the one dependency-light leaf every consumer already sees,
/// rather than in `lunco-celestial`: the offline `lunco-assets` build tool needs
/// the same datum for the GeoTIFF it writes and must not take a Bevy-heavy
/// simulation dependency to get it. `lunco_celestial::registry` re-exports it.
pub const MOON_MEAN_RADIUS_M: f64 = 1_737_400.0;
