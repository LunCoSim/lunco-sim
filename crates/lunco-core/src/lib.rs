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

/// Command envelope — `Mutation<P>`, `Ack`, `Reject`, `SyncChannel`.
/// The shape every locally- or remotely-originated mutation flows
/// through.
pub mod commands;
/// M1 — deterministic identity from `Provenance`. The only place network
/// ids are *derived*; the session identity-admission system is the only place they
/// are *minted*.
pub mod identity;
/// Shared 53-bit time-sorted id generator backing `GlobalEntityId`
/// and `commands::OpId`.
pub mod ids;
/// Shared semantic labels for UI, API, and scripting presentation.
pub mod labels;
pub mod log;
/// Architectural marker components shared by engine subsystems.
pub mod markers;
pub mod mocks;
pub mod physics_state;
pub mod programs;
/// M4 — pure predict-own reconciliation decision (input-replay, D2). The
/// dependency-free geometry the spawn-domain `reconcile_owned_prediction` system
/// applies; unit-tested without the avian/render build.
pub mod reconcile;
/// Typed requests and lifecycle edges for scene ownership and transitions.
pub mod scene;
/// Shared scene teardown schedule for all scene-owned subsystems.
mod scene_lifecycle;
pub mod subsystems;
pub mod telemetry;

pub mod derived;

/// Domain-free named engine exposure snapshots for UI, API, and diagnostics.
pub mod exposure;

pub mod faults;

pub mod mobility;
/// Generic authored-model invalidation shared by all backend adapters.
pub mod model_state;
pub mod tools;

pub mod pacing;

/// Run-condition effectiveness — see [`gate::tracked`].
pub mod gate;

pub use derived::RebuildOnChange;
pub use faults::{
    clear_runtime_diagnostics, DiagnosticSeverity, RuntimeDiagnostic, RuntimeDiagnostics,
    RuntimeFault, RuntimeFaults,
};
pub use markers::NoSelectionBounds;
pub use mobility::Mobility;
pub use mocks::*;
pub use model_state::ModelStateRevision;
pub use pacing::{
    KeepAwake, SimulationBarrier, SimulationBarrierParticipants, SimulationExecutionMode,
};
pub use physics_state::*;
pub use telemetry::*;
// Explicit re-export: bevy 0.19's prelude also names a `Severity`, and the
// crate-root `use bevy::prelude::*` below shadows the glob above for external
// path resolution (`lunco_core::Severity` would hit bevy's private import).
// An explicit item outranks both globs.
pub use commands::{
    Ack, ActiveCommandId, ApiCommandMarker, ClientCommandPolicy, CommandOutcome, CommandResults,
    EditIntent, MarkClientLocalExt, Mutation, OpId, Reject, SessionId, SpawnEntity, SyncChannel,
};
pub use identity::Provenance;
pub use labels::{entity_display_name, humanize_identifier};
pub use log::*;
pub use markers::{
    CameraPoseLock, CatalogEntryId, EmbeddedScenarioPath, EmbeddedScenarioSource,
    HorizonShadowTerrain, PhysicsPoseAuthoritative, ScenarioProgramPrim, ScriptParams,
    SunAngularDiameter, TriggerZone, UsdPrimKind, CELESTIAL_COLLISION_LAYER,
    NON_PHYSICAL_QUERY_LAYERS, SOLAR_ANGULAR_DIAMETER_DEG, TRIGGER_COLLISION_LAYER,
};
pub use reconcile::{reconcile_decision, ReconcileParams, Reconciliation};
pub use scene::{
    SceneTransition, SceneTransitionAdmission, SceneTransitionAdmitted, SceneTransitionCompleted,
    SceneTransitionCoordinator, SceneTransitionFailed, SceneTransitionIntent,
    SceneTransitionRequest, SceneTransitionStarted,
};
pub use scene_lifecycle::{run_scene_teardown, SceneMountState, SceneTeardown};
pub use telemetry::Severity;

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

use bevy::ecs::schedule::ScheduleLabel;
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
/// of two ways, both admitted by the session-layer identity system:
/// - **derived** from [`Provenance`] (Content/Derived) — deterministic, same on
///   every peer, no coordination;
/// - **server-allocated** ([`Provenance::Authoritative`]) via [`crate::ids`],
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
        Self(crate::ids::make_id_53())
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

/// The main window's 3D **viewport**: which camera it renders from, whether
/// it's shown, and the sub-rect it occupies. A single reconciler
/// (`lunco_usd_bevy::reconcile_scene_viewport`) turns this into Bevy's
/// per-camera `Camera::is_active` + `Camera::viewport` — the ONE authority over
/// window-camera activation. Models an Omniverse Viewport (which owns an active
/// `camera`), reusing Bevy's own `is_active`/`viewport` rather than inventing a
/// bespoke "view" concept.
///
/// Contributors write DATA here and NEVER touch `Camera::is_active` themselves:
/// - an explicit camera presentation request rebinds [`active_camera`];
/// - the workbench sets [`visible`] + [`rect`] from its layout perspective.
///
/// [`active_camera`]: SceneViewport::active_camera
/// [`visible`]: SceneViewport::visible
/// [`rect`]: SceneViewport::rect
#[derive(Resource, Debug, Clone)]
pub struct SceneViewport {
    /// The bound (active) camera — which window `Camera3d` renders. Revalidated
    /// each frame by the reconciler. `None` is an intentional no-camera state;
    /// it is never replaced by an implicit avatar or first-camera choice.
    pub active_camera: Option<Entity>,
    /// Whether the 3D scene renders at all (the workbench Design perspective
    /// sets this `false`). Defaults `true` so tooling/headless binaries with no
    /// workbench Just Work.
    pub visible: bool,
    /// Physical `(position, size)` sub-rect the viewport occupies within the
    /// window, or `None` for the full window (the current default).
    pub rect: Option<(UVec2, UVec2)>,
}

/// Presentation intent emitted by avatar workflows that explicitly return the
/// operator to the local avatar. The camera subsystem owns resolution and
/// activation; this event keeps avatar mechanics independent of USD camera
/// projection details.
#[derive(Event, Clone, Copy, Debug, Default)]
pub struct RequestLocalAvatarView;

impl Default for SceneViewport {
    fn default() -> Self {
        Self {
            active_camera: None,
            visible: true,
            rect: None,
        }
    }
}

/// Ordering boundary for the main presentation viewport.
///
/// The workbench publishes layout data in [`SceneViewportSet::Publish`]. The
/// USD/render camera owner then reconciles that data in
/// [`SceneViewportSet::Reconcile`] before Bevy updates camera projections. The
/// shared set is the cross-crate schedule contract; neither contributor needs
/// to depend on the other's implementation crate.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SceneViewportSet {
    /// Publish visibility and layout data into [`SceneViewport`].
    Publish,
    /// Reconcile the explicit viewport binding into render-camera state.
    Reconcile,
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

/// Button-specific interaction intent authored by a USD prim.
///
/// This deliberately lives in the render-free core.  USD loading can describe
/// the interaction contract without depending on Bevy's picking backend, while
/// the GUI picking layer can translate `PassThrough` into `Pickable` behavior.
/// Keeping the two policies independent is what lets a transparent marker pass
/// through a primary click and still receive a secondary-click context menu.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PointerInteraction {
    /// The prim is the normal target and blocks lower hits.
    #[default]
    Block,
    /// The prim may receive an event, but does not block geometry behind it.
    PassThrough,
    /// The prim remains a target for a context-menu observer.
    Context,
}

/// USD-authored pointer behavior for a scene prim or its visual mesh.
#[derive(Component, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScenePointerPolicy {
    pub left: PointerInteraction,
    pub right: PointerInteraction,
}

impl ScenePointerPolicy {
    /// Parse the small stable vocabulary used by USD `lunco:interaction:*`
    /// attributes. Unknown values intentionally fall back to `Block`: an
    /// authoring typo must not make an object accidentally click-through.
    pub fn from_usd(left: Option<&str>, right: Option<&str>) -> Option<Self> {
        fn parse(value: Option<&str>) -> Option<PointerInteraction> {
            match value {
                Some("pass_through") => Some(PointerInteraction::PassThrough),
                Some("context") => Some(PointerInteraction::Context),
                Some("block") => Some(PointerInteraction::Block),
                Some(_) => Some(PointerInteraction::Block),
                None => None,
            }
        }

        let left = parse(left);
        let right = parse(right);
        (left.is_some() || right.is_some()).then(|| Self {
            left: left.unwrap_or_default(),
            right: right.unwrap_or_default(),
        })
    }
}

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

// `TimeWarpState` was removed (doc 19): "is physics advancing" had three
// redundant encodings (`physics_enabled` ≡ `is_running()` ≡
// `Time<Virtual>.relative_speed > 0`). The single source is now the direct clock
// state on `Time<Virtual>` — the `lunco-time` spine sets `relative_speed`, and
// every gate (the `SimTick` advance below + the physics-stepping systems in
// hardware/mobility/usd-sim) reads `relative_speed_f64() > 0`. One representation,
// no drift.

/// Marker resource indicating that entity dragging is active.
///
/// Used by scene editing systems to signal other systems (like avatar possession)
/// to disable conflicting interactions during drag operations.
#[derive(Resource, Default)]
pub struct DragModeActive {
    /// Whether dragging is currently active.
    pub active: bool,
}

/// Marker resource indicating a click-to-place spawn tool is armed.
///
/// Set by scene-edit's spawn placement system whenever `SpawnState`
/// is `Selecting`. Read by avatar possession to suppress vessel
/// possession on the placement click.
#[derive(Resource, Default)]
pub struct SpawnToolActive(pub bool);

/// Which subsystem owns primary scene clicks for the active workbench mode.
///
/// This is a cross-crate interaction contract rather than a workbench UI detail:
/// the editor selection observer and avatar possession observer both receive the
/// same global pointer event, so they must consult the same authoritative mode.
/// Binaries without a workbench keep the simulation default and retain normal
/// possession behavior.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneInteractionMode {
    /// Plain scene clicks may claim a controllable endpoint.
    #[default]
    Simulation,
    /// Plain scene clicks belong to editor selection and manipulation.
    Editor,
}

impl SceneInteractionMode {
    /// Whether the editor selection observer owns this primary scene click.
    ///
    /// Simulation keeps plain clicks for possession, while explicit modifier
    /// clicks remain selection/removal intents in every perspective.
    pub const fn selection_owns_click(self, modified: bool) -> bool {
        modified || matches!(self, Self::Editor)
    }

    /// Whether avatar possession owns this primary scene click.
    ///
    /// Simulation reserves unmodified clicks for possession; explicit modifier
    /// clicks belong to selection/removal in every perspective.
    pub const fn possession_owns_click(self, modified: bool) -> bool {
        !modified && matches!(self, Self::Simulation)
    }
}

#[cfg(test)]
mod scene_interaction_mode_tests {
    use super::SceneInteractionMode;

    #[test]
    fn primary_click_has_one_owner_per_mode() {
        assert!(SceneInteractionMode::Simulation.possession_owns_click(false));
        assert!(!SceneInteractionMode::Simulation.possession_owns_click(true));
        assert!(!SceneInteractionMode::Simulation.selection_owns_click(false));
        assert!(SceneInteractionMode::Simulation.selection_owns_click(true));
        assert!(SceneInteractionMode::Editor.selection_owns_click(false));
        assert!(!SceneInteractionMode::Editor.possession_owns_click(false));
        assert!(!SceneInteractionMode::Editor.possession_owns_click(true));
    }
}

/// Camera ray for a discrete scene click — the SINGLE shared entry point for
/// every scene-click observer (possession, selection, placement).
///
/// Returns the world-space ray from `camera` through `cursor`, or `None` when
/// `pointer_blocked` is true or the ray can't be built.
///
/// `wants_pointer` is a **global** signal, and now (fed by the workbench's
/// egui-authoritative `pointer_over_scene` hit test) it is `false` over the
/// transparent docked `ViewportPanel` leaf yet `true` over ANY real chrome. That
/// globality matters: a `Pointer<Click>` over chrome can fire on more than one
/// entity (the egui host AND an underlying scene entity), so a per-target chrome
/// check would leak the second fire through — gating on the global flag stands the
/// observer down for every fire that frame.
///
/// The chrome guard is `wants_pointer`, NOT `click.hit.position.is_none()`: that
/// old check was overloaded — it silently rejected valid scene clicks whenever
/// bevy_picking found no mesh under the cursor (streamed terrain with no pickable
/// tile that frame, or an analytic celestial/spacecraft body — the "can't place
/// on the ground" bug). Callers cast the returned ray themselves — against avian
/// colliders (`SpatialQuery`, e.g. the terrain) or their own analytic shapes
/// (hit-spheres).
pub fn scene_click_ray(
    pointer_blocked: bool,
    camera: &Camera,
    cam_gtf: &GlobalTransform,
    cursor: Vec2,
) -> Option<Ray3d> {
    if pointer_blocked {
        return None;
    }
    // `cursor` is bevy_picking's pointer position: LOGICAL pixels from the WINDOW
    // top-left. `Camera::viewport_to_world` expects a position in the camera's own
    // VIEWPORT space (it divides by `logical_viewport_size` and never adds the
    // viewport origin), so it is only correct when the camera's viewport starts at
    // the window origin. Today `apply_workbench_viewport` keeps the scene camera
    // full-window (`SceneViewport.rect = None`), so the offset is zero and this is a
    // no-op. It is a guard for the planned sub-rect confinement noted there ("a
    // future sub-rect would derive it from the ViewportPanel's recorded rect"): the
    // instant the camera is confined to the offset ViewportPanel leaf, feeding the
    // raw WINDOW cursor here would skew every ray by the chrome offset and silently
    // break spawn/select/possess in the middle of the Build view. Subtracting the
    // logical viewport origin keeps both modes on one correct path.
    let local = camera
        .logical_viewport_rect()
        .map_or(cursor, |rect| cursor - rect.min);
    camera.viewport_to_world(cam_gtf, local).ok()
}

/// Marker resource indicating a terrain-sculpt tool is armed.
///
/// Set by scene-edit's terrain-tools system whenever a [`TerrainTool`] is
/// selected in the Tools palette. Read by avatar possession and entity
/// selection to suppress their click handling — while a sculpt brush is armed
/// every scene click applies terrain, not possess/select. Mirrors
/// [`SpawnToolActive`].
///
/// [`TerrainTool`]: (scene-edit) crate::terrain_tools::TerrainTool
#[derive(Resource, Default)]
pub struct TerrainToolActive(pub bool);

/// The SCRIPT-AUTHORED click tool currently armed, by tool name (`"recover"`),
/// or `None`.
///
/// A script tool is any registered `lunco_tools` tool exposing `on_click/1`;
/// the editor lists them in the Tools palette and hands a structured scene-click
/// context to the tool's own handler. So this is deliberately a NAME and not an enum:
/// the set of tools is data (a `.rhai` file), not a Rust type, and adding one
/// must not require editing this crate.
///
/// Read by avatar possession and entity selection so a click while a tool is
/// armed goes to the tool instead of possessing or selecting. Mirrors
/// [`SpawnToolActive`] / [`TerrainToolActive`], which gate the Rust-side tools.
#[derive(Resource, Default)]
pub struct ArmedScriptTool(pub Option<String>);

impl ArmedScriptTool {
    /// Whether any script tool is armed.
    pub fn armed(&self) -> bool {
        self.0.is_some()
    }

    /// Whether `name` is the armed tool.
    pub fn is(&self, name: &str) -> bool {
        self.0.as_deref() == Some(name)
    }
}

/// "A cursor-driven editor mode owns the pointer" — the one gate, in one place.
///
/// The spawn ghost, terrain brush, and authored script tools can own the cursor.
/// The click observers already consult these flags one-by-one; this bundles them
/// so the shared cancel intent honours exactly the same set.
#[derive(bevy::ecs::system::SystemParam)]
pub struct CursorModeActive<'w> {
    spawn_tool: Option<Res<'w, SpawnToolActive>>,
    terrain_tool: Option<Res<'w, TerrainToolActive>>,
    script_tool: Option<Res<'w, ArmedScriptTool>>,
}

impl CursorModeActive<'_> {
    /// True while any editor mode is using the cursor.
    pub fn any(&self) -> bool {
        self.spawn_tool.as_ref().is_some_and(|t| t.0)
            || self.terrain_tool.as_ref().is_some_and(|t| t.0)
            || self.script_tool.as_ref().is_some_and(|t| t.armed())
    }
}

/// Per-entity marker: this entity is currently being dragged by the editor
/// transform gizmo.
///
/// Set/cleared by scene-edit's gizmo systems (an editor/UI concern that lives
/// behind the `ui` feature). It exists in `lunco-core` so render/sim systems can
/// react to a drag **without** depending on `transform-gizmo-bevy`: e.g. the
/// avatar camera-follow systems pause following a target while it's dragged.
/// On a headless server nothing inserts it, so those checks are simply always-false.
#[derive(Component, Default)]
pub struct GizmoDragging;

/// The fixed-simulation rate, in Hz. The **single source of truth** for every
/// fixed-step clock in the system: it drives `Time::<Fixed>` (set by each app
/// binary), [`SimTick`] advancement ([`advance_sim_tick`], one tick per fixed
/// step), and the lightyear tick. The snapshot interpolation converts host ticks
/// → seconds via [`SECS_PER_TICK`], so every one of these MUST agree — hence one
/// constant rather than a `60.0` literal sprinkled across crates.
pub const FIXED_HZ: f64 = 60.0;

/// Seconds per fixed tick / per [`SimTick`] (= `1.0 / FIXED_HZ`). Used to place
/// snapshot samples on the interpolation timebase.
pub const SECS_PER_TICK: f64 = 1.0 / FIXED_HZ;

/// Monotonic discrete **simulation tick** — the netcode time substrate (M6).
///
/// The `lunco-time` spine (`WorldTime`/`TimeTransport`) gives *continuous* sim
/// time + warp; netcode
/// also needs a monotonic integer counter that prediction, rollback,
/// input-stamping and the shared clock all key off. Advanced once per
/// `FixedUpdate` step (see [`advance_sim_tick`]). Warp-independent: warp scales
/// `dt`, not the tick count, so peers can compare ticks directly. Not yet
/// consumed anywhere — it's the substrate the networking layer (Ph3/Ph4) drives.
#[derive(
    Resource,
    Default,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Reflect,
    serde::Serialize,
    serde::Deserialize,
)]
#[reflect(Resource)]
pub struct SimTick(pub u64);

impl SimTick {
    /// Signed tick distance `self - other`, wrapping-safe.
    pub fn wrapping_diff(self, other: SimTick) -> i64 {
        self.0.wrapping_sub(other.0) as i64
    }
}

/// Control-signal propagation set: values move along `SimConnection`s from
/// source port to target. Runs on the **fixed** clock so the actuation path
/// is frame-rate-independent and identical on every peer.
///
/// This is load-bearing for client-prediction determinism. Propagation must not
/// run in `Update` (render rate) while its producer (flight-software command
/// observers) and consumers (wheel/hardware actuators) run in `FixedUpdate`: the
/// latency between "input applied" and "force applied" would be coupled to frame
/// rate, so the same input `seq` would land on the wheels a *different* number of
/// physics ticks apart on host vs client (which render at independent rates), the
/// client's prediction would never match the host, and every snapshot ack would
/// correct — showing up as steering jitter.
///
/// The set is the ordering ANCHOR: actuators that read a port order `.after`
/// it, and `lunco_cosim`'s `CosimSet::Propagate` is nested INSIDE it so those
/// orderings keep their meaning. Adding a propagation system elsewhere without
/// putting it in this set silently breaks that contract.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ControlDacSet;

/// The **rollback replay** schedule: exactly the actuation chain of one simulation
/// tick (FSW command surface → drive mix → DAC → wheel/hardware actuators), and
/// NOTHING else — no tick advance, no scenario scripts, no sensors, no networking,
/// no journaling.
///
/// Deterministic rollback re-simulates the owned rover's unacked inputs by running
/// this schedule + avian's `PhysicsSchedule` once per replayed input. We cannot
/// simply re-run `FixedMain`: Bevy's `run_schedule` takes the schedule *out* of the
/// world, so re-entering `FixedMain` from inside it is impossible — and it would
/// also re-run every unrelated fixed-tick system (scripts, sensors, the sim-tick
/// advance) N times per correction. Mirroring only the actuation chain here keeps
/// replay faithful AND side-effect free.
///
/// INVARIANT: every system a rover's actuation depends on in `FixedUpdate` must be
/// registered here too, in the same relative order (see `ControlDacSet`). A system
/// added to the live chain but forgotten here silently makes replay diverge from
/// the host — the exact class of bug rollback exists to eliminate.
#[derive(ScheduleLabel, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RollbackReplay;

/// True only while the client is re-simulating the owned rover inside a rollback.
///
/// Systems that must NOT run during a replay step guard on this: the input source
/// (`drive_from_bindings` — replay feeds *recorded* inputs, not the live keyboard),
/// the proxy drivers, and the reconcilers/recorders (which would otherwise fold a
/// correction into the very trajectory they are correcting). Everything scheduled
/// in `Update` is naturally exempt — replay only runs `RollbackReplay` +
/// `PhysicsSchedule`.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollbackInProgress(pub bool);

/// Run condition: true when NOT inside a rollback replay. Attach to any fixed-tick
/// system whose side effects must happen exactly once per real tick.
pub fn not_rolling_back(rb: Option<Res<RollbackInProgress>>) -> bool {
    !rb.is_some_and(|r| r.0)
}

/// Ordering anchor for the client-netcode `Update` pipeline, which now **spans two
/// crates**: the spawn half (`apply_replicated_spawns`, in `lunco-scene-commands`,
/// because it instantiates from the spawn catalog) must run before the prediction
/// half (interp / kinematic-pin / reconcile / rollback, in `lunco-networking`).
/// The two used to sit in one `.chain()` in a single file; a plain `.chain()` can't
/// express the ordering across the crate boundary, and neither crate may depend on
/// the other (`lunco-networking` must never gain an editor-package edge — see
/// its Cargo.toml, review A6). `lunco-core` is the one crate both already depend on,
/// so the shared set lives here.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetcodeSet {
    /// Instantiate host-replicated spawns (`apply_replicated_spawns`, scene-edit).
    InstantiateSpawns,
    /// The client-prediction pipeline (`lunco-networking::prediction`), after the
    /// spawns it may act on exist.
    Predict,
}

impl Plugin for LunCoCorePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LunCoLogPlugin);
        // Scene projection has an ownership fence independent of the USD
        // plugins.  Load/restart/clear invalidate it synchronously, while the
        // deferred root spawner registers the replacement after creation.
        app.init_resource::<SceneMountState>();
        app.register_type::<CameraPoseLock>()
            .register_type::<PhysicsPoseAuthoritative>()
            // `telemetry::` — bevy 0.19's prelude exports its own `Severity`
            // (log-level type), which shadows ours in glob-import scopes.
            .register_type::<crate::telemetry::Severity>()
            .register_type::<TelemetryValue>()
            .register_type::<TelemetryEvent>()
            .register_type::<Parameter>()
            .register_type::<SampledParameter>()
            .register_type::<ModelStateRevision>()
            .register_type::<PhysicalProperties>()
            .register_type::<CelestialBody>()
            .register_type::<Spacecraft>()
            .register_type::<MobilityRoot>()
            .register_type::<GlobalEntityId>()
            .register_type::<Provenance>()
            .register_type::<SimTick>();

        // All always-on core/substrate resources live in one function so a
        // unit test can assert the full set is present without building the
        // heavier LunCoCorePlugin (log + core registrations). See its doc comment for
        // the invariant this enforces.
        register_core_resources(app);
        app.add_systems(
            SceneTeardown,
            (clear_runtime_diagnostics, reset_core_scene_state),
        );
        // Runtime subsystem toggles (progressive-fidelity substrate) +
        // `SetSubsystemEnabled` command.
        subsystems::build_subsystems(app);
        app.add_systems(FixedUpdate, advance_sim_tick);
        // Port propagation on rollback replay is registered by `lunco_cosim`
        // (its `CosimSet::Propagate` nests inside `ControlDacSet`). This crate
        // contributes nothing to the replayed chain — `advance_sim_tick` is
        // deliberately excluded, since a replayed tick must not advance the
        // simulation's tick counter.
        app.init_resource::<RollbackInProgress>();
    }
}

/// Initialize every always-on core/substrate resource.
///
/// **Invariant:** any resource consumed via `Res`/`ResMut` by a system or
/// observer that is registered unconditionally (i.e. not behind the
/// `networking` feature or some other optional plugin) MUST be initialized
/// here — never only inside a feature-gated plugin like
/// `lunco_networking::SyncPlugin`. Otherwise builds without that feature
/// panic at runtime with "Resource does not exist". `lunco-core` is a
/// dependency of every crate, so initializing here guarantees presence
/// everywhere. The `core_substrate_resources_present` test guards this.
pub(crate) fn register_core_resources(app: &mut App) {
    app.init_resource::<SimTick>()
        // The scene viewport's active-camera binding (+ visibility/rect). The
        // single source of truth the viewport-camera reconciler actuates; the
        // switch and workbench write it. Core-guaranteed so every windowed
        // binary has it without ordering worries.
        .init_resource::<SceneViewport>()
        .init_resource::<SceneMountState>()
        // Command-result substrate: result-reporting `#[on_command]` observers
        // require these to exist (the same always-on resource rule enforced by
        // LunCoCoreSessionPlugin for the session layer).
        .init_resource::<CommandResults>()
        .init_resource::<ActiveCommandId>()
        .init_resource::<exposure::EngineExposures>()
        .init_resource::<exposure::ExposureRefresh>()
        .init_resource::<RuntimeFaults>()
        .init_resource::<RuntimeDiagnostics>()
        .init_resource::<pacing::SimulationBarrier>()
        .init_resource::<pacing::SimulationBarrierParticipants>();
}

/// Reset core-owned scene state before the outgoing entities are despawned.
fn reset_core_scene_state(mut rollback: ResMut<RollbackInProgress>) {
    rollback.0 = false;
}

/// Advance the discrete [`SimTick`] once per fixed step, *only while time is
/// actually flowing* (so a paused/zero-speed/warping world freezes the tick and
/// peers stay comparable). The gate is the direct clock state
/// `Time<Virtual>.effective_speed > 0` — the same predicate the physics-stepping
/// systems use. `effective_speed`, not `relative_speed`: the spine expresses
/// "frozen" with Bevy's paused flag (which zeroes the former but not the latter),
/// because `relative_speed` is a rate that consumers divide by.
/// `Time<Virtual>` is the mandatory admission clock. A schedule that omitted
/// it (for example, a partially constructed host) fails closed instead of
/// advancing the master tick outside the shared time spine.
fn advance_sim_tick(mut tick: ResMut<SimTick>, vtime: Option<Res<Time<Virtual>>>) {
    // The core time spine is mandatory in a running app. A bare schedule that
    // omitted Time<Virtual> must not silently advance the master tick.
    let running = vtime.is_some_and(|t| !t.is_paused() && t.relative_speed_f64() > 0.0);
    if running {
        tick.0 = tick.0.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_pointer_policy_has_fail_safe_usd_semantics() {
        assert_eq!(
            ScenePointerPolicy::from_usd(Some("pass_through"), Some("context")),
            Some(ScenePointerPolicy {
                left: PointerInteraction::PassThrough,
                right: PointerInteraction::Context,
            })
        );
        assert_eq!(ScenePointerPolicy::from_usd(None, None), None);
        assert_eq!(
            ScenePointerPolicy::from_usd(Some("typo"), None),
            Some(ScenePointerPolicy {
                left: PointerInteraction::Block,
                right: PointerInteraction::Block,
            })
        );
    }

    #[test]
    fn sim_tick_advances_under_run_paused_does_not() {
        let mut app = App::new();
        app.init_resource::<SimTick>()
            .add_systems(FixedUpdate, advance_sim_tick)
            .insert_resource(Time::<Virtual>::default());

        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<SimTick>().0, 1);
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<SimTick>().0, 2);

        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<SimTick>().0, 2);

        app.world_mut().resource_mut::<Time<Virtual>>().unpause();
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<SimTick>().0, 3);
    }
}
