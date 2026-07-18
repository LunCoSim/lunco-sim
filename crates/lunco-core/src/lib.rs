//! Core types and plugins for the LunCo simulation.
//!
//! This crate provides the foundational components, resources, and systems used
//! across the simulation, including physical properties, celestial timing,
//! and the core plugin registration.

pub mod architecture;
pub mod kernels;
pub mod mocks;
pub mod ports;
pub mod programs;
pub mod telemetry;
pub mod coords;
pub mod log;
/// Architectural marker components for the big_space integration.
pub mod markers;
/// Atomic re-parenting helpers for SOI/Grid migration.
pub mod attach;
/// Debug-build invariant checks for the big_space hierarchy.
pub mod invariants;
/// The persistent big_space world shell (single root + `WorldGrid` + one
/// `FloatingOrigin`) that every scene mounts into.
pub mod world;
/// Unified diagram data model — pure Rust, no Bevy dependency.
pub mod diagram;
/// Shared 53-bit time-sorted id generator backing `GlobalEntityId`
/// and `commands::OpId`.
pub mod ids;
/// M1 — deterministic identity from `Provenance`. The only place network
/// ids are *derived*; the assignment system below is the only place they
/// are *minted*.
pub mod identity;
/// Command envelope — `Mutation<P>`, `Ack`, `Reject`, `SyncChannel`.
/// The shape every locally- or remotely-originated mutation flows
/// through.
pub mod commands;
/// Always-on networking **authority** substrate (no wire dependency):
/// `NetworkRole`, `LocalSession`, `SyncApplyGuard`, `SessionRegistry` + the
/// single `authorize` gate. The seam the optional `lunco-networking` layer
/// drives; trivially inert in single-player.
pub mod session;
/// M4 — pure predict-own reconciliation decision (input-replay, D2). The
/// dependency-free geometry the spawn-domain `reconcile_owned_prediction` system
/// applies; unit-tested without the avian/render build.
pub mod reconcile;

pub mod subsystems;

pub mod derived;

pub mod mobility;

pub mod tools;

pub mod pacing;

pub use architecture::*;
pub use derived::RebuildOnChange;
pub use pacing::KeepAwake;
pub use mobility::Mobility;
pub use mocks::*;
pub use telemetry::*;
// Explicit re-export: bevy 0.19's prelude also names a `Severity`, and the
// crate-root `use bevy::prelude::*` below shadows the glob above for external
// path resolution (`lunco_core::Severity` would hit bevy's private import).
// An explicit item outranks both globs.
pub use telemetry::Severity;
pub use log::*;
pub use commands::{
    Ack, ActiveCommandId, ClientCommandPolicy, CommandOutcome, CommandResults, EditIntent,
    MarkClientLocalExt, Mutation, OpId, Reject, SessionId, SpawnEntity, SyncChannel,
};
pub use markers::{
    ActuatorDrivenJoint, EmbeddedScenarioPath, EmbeddedScenarioSource, FallbackSceneLight,
    GridAnchor, HorizonShadowTerrain, NeedsGroundSettle, NextScene, RestoreFallbackLights, ScenarioProgramPrim, ScriptParams, SoiMigrant, SunAngularDiameter, TriggerZone,
    TRIGGER_COLLISION_LAYER,
};
pub use invariants::BigSpaceInvariantsPlugin;
pub use world::{
    ensure_world_root, OriginAnchor, WorldGrid, WorldGridConfig, WorldRoot, WorldShellPlugin,
    WorldShellSet,
};
pub use identity::Provenance;
pub use reconcile::{reconcile_decision, ReconcileParams, Reconciliation};
pub use session::{
    authorize, AppliedInputSeq, AppliedSlot, MAX_SEQ_JUMP, ArticulatedLink, ArticulatedVehicle, ContactPredictable,
    BodyDivergence, DivergenceStats, PredictionKind,
    IncomingSnapshots, InputFrame,
    LocalSession,
    NetConnectRequest, NetDisconnectRequest,
    NetExcluded, NetReplicate, NetSpawn, PendingConnect, PendingConnectRequest,
    BufferedClientInputs, LocalDriveInput,
    NetStatus, NetworkRole, NotPredictable, OwnedInputLog, OwnedLocally, PendingCorrection,
    PendingReplicatedSpawns,
    PossessionPolicy,
    PredictedDynamic, ReplicatedChassisMotion, ReplicatedSpawn, SessionRegistry, SessionProfiles, SkipContentStamp,
    SnapshotSample,
    VesselInputLog, SyncApplyGuard,
};

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

pub use lunco_command_macro::{Command, on_command, register_commands};

/// Re-exported `serde` so the `#[Command]` proc-macro can reference it
/// via an absolute path (`::lunco_core::serde::*`). Crates using
/// `#[Command]` do not need their own `serde` dependency — they get it
/// transitively through `lunco-core`.
pub use serde;

use bevy::prelude::*;
use bevy::ecs::schedule::ScheduleLabel;

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
/// checked for ownership against (e.g. `DriveRover.target`). The wire apply
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

/// Marker component for the user's active avatar/entity in the simulation.
#[derive(Component)]
pub struct Avatar;

/// Marks **this peer's own** avatar — the one its local input drives. Each
/// process has exactly one (its camera); other players' avatars are not
/// replicated (gap G3), so this is what gates raw-input→command mapping to "my"
/// vessel only (gap G1). Inserted by `lunco-avatar`'s `mark_local_avatar`.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct LocalAvatar;

/// The main window's 3D **viewport**: which camera it renders from, whether
/// it's shown, and the sub-rect it occupies. A single reconciler
/// (`lunco_usd_bevy::reconcile_scene_viewport`) turns this into Bevy's
/// per-camera `Camera::is_active` + `Camera::viewport` — the ONE authority over
/// window-camera activation. Models an Omniverse Viewport (which owns an active
/// `camera`), reusing Bevy's own `is_active`/`viewport` rather than inventing a
/// bespoke "view" concept.
///
/// Contributors write DATA here and NEVER touch `Camera::is_active` themselves:
/// - the camera switch (`set_camera(name)` / `KeyC`) rebinds [`active_camera`];
/// - the workbench sets [`visible`] + [`rect`] from its layout perspective.
///
/// [`active_camera`]: SceneViewport::active_camera
/// [`visible`]: SceneViewport::visible
/// [`rect`]: SceneViewport::rect
#[derive(Resource, Debug, Clone)]
pub struct SceneViewport {
    /// The bound (active) camera — which window `Camera3d` renders. Revalidated
    /// each frame by the reconciler; falls back to the local avatar camera
    /// (else any window camera) when unset or stale.
    pub active_camera: Option<Entity>,
    /// Whether the 3D scene renders at all (the workbench Design perspective
    /// sets this `false`). Defaults `true` so tooling/headless binaries with no
    /// workbench Just Work.
    pub visible: bool,
    /// Physical `(position, size)` sub-rect the viewport occupies within the
    /// window, or `None` for the full window (the current default).
    pub rect: Option<(UVec2, UVec2)>,
}

impl Default for SceneViewport {
    fn default() -> Self {
        Self {
            active_camera: None,
            visible: true,
            rect: None,
        }
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
// controllable iff it exposes writable control ports — a `FlightSoftware`
// control surface (rovers via PhysX, or any `lunco:vessel="true"` prim) or a
// Modelica `SimComponent`. The components a body already carries ARE its
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

/// Marker component for terrain/ground entities that should be excluded
/// from vessel possession and editing interactions.
#[derive(Component)]
pub struct Ground;

/// Marker for vessel root bodies that have a meaningful "upright" (rovers,
/// landers): recovery systems may auto-right the body — and its joint-connected
/// assembly — when it comes to rest overturned. Plain props and rocks must NOT
/// carry this; a tipped rock staying tipped is correct.
#[derive(Component)]
pub struct KeepUpright;

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

// `TimeWarpState` was removed (doc 19): "is physics advancing" had three
// redundant encodings (`physics_enabled` ≡ `is_running()` ≡
// `Time<Virtual>.relative_speed > 0`). The single source is now the direct clock
// state on `Time<Virtual>` — the `lunco-time` spine sets `relative_speed`, and
// every gate (the `SimTick` advance below + the physics-stepping systems in
// hardware/mobility/usd-sim) reads `relative_speed_f64() > 0`. One representation,
// no drift.

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

/// Whether egui is currently consuming pointer / keyboard input.
///
/// egui is a second, immediate-mode input world layered on top of Bevy: it
/// reads its own copy of the winit events and never removes anything from
/// Bevy's `ButtonInput`. So a key pressed while an egui text field is focused
/// reaches BOTH egui and Bevy's `ButtonInput<KeyCode>` — and without this gate
/// it would also drive the avatar (typing `w`/`a`/`s`/`d` in the Inspector or a
/// REPL would move the vessel). Likewise a scroll/orbit over a panel would move
/// the camera.
///
/// This resource relays egui's `wants_keyboard_input()` / `wants_pointer_input()`
/// (from the primary egui context) into the ECS so scene-input systems can gate
/// on it without depending on `bevy_egui`. Populated once per frame by
/// `lunco-workbench` (the crate that owns the `PrimaryEguiContext`); on a
/// headless server nothing writes it, so both flags stay `false` and every gate
/// is a no-op.
///
/// Discrete scene *picks* (click-to-select / click-to-spawn) do NOT need this —
/// they flow through `bevy_picking`, where egui occlusion is already handled by
/// the workbench's egui picking backend. This gate is for the *continuous / raw*
/// input systems: keyboard driving, camera orbit, scroll-zoom.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct EguiFocus {
    /// A focused egui widget (text field, drag-value, …) wants the keyboard.
    pub wants_keyboard: bool,
    /// The pointer is over an egui widget that wants pointer input.
    pub wants_pointer: bool,
}

/// Camera ray for a discrete scene click — the SINGLE shared entry point for
/// every scene-click observer (possession, selection, placement).
///
/// Returns the world-space ray from `camera` through `cursor`, or `None` when the
/// click belongs to egui ([`EguiFocus::wants_pointer`]) or the ray can't be built.
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
    focus: &EguiFocus,
    camera: &Camera,
    cam_gtf: &GlobalTransform,
    cursor: Vec2,
) -> Option<Ray3d> {
    if focus.wants_pointer {
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
/// Set by sandbox-edit's terrain-tools system whenever a [`TerrainTool`] is
/// selected in the Tools palette. Read by avatar possession and entity
/// selection to suppress their click handling — while a sculpt brush is armed
/// every scene click applies terrain, not possess/select. Mirrors
/// [`SpawnToolActive`].
///
/// [`TerrainTool`]: (sandbox-edit) crate::terrain_tools::TerrainTool
#[derive(Resource, Default)]
pub struct TerrainToolActive(pub bool);

/// "A cursor-driven editor mode owns the pointer" — the one gate, in one place.
///
/// The waypoint placement/menu, the spawn ghost and the terrain brush all park a mode
/// on the cursor. The click observers already consulted these flags one-by-one; this
/// bundles them so the keyboard ([`CancelIntent`]) honours exactly the same set. A
/// mode must not be click-suppressed but keyboard-transparent (or the reverse).
#[derive(bevy::ecs::system::SystemParam)]
pub struct CursorModeActive<'w> {
    waypoint_tool: Option<Res<'w, WaypointToolActive>>,
    waypoint_menu: Option<Res<'w, WaypointMenuOpen>>,
    spawn_tool: Option<Res<'w, SpawnToolActive>>,
    terrain_tool: Option<Res<'w, TerrainToolActive>>,
}

impl CursorModeActive<'_> {
    /// True while any editor mode is using the cursor.
    pub fn any(&self) -> bool {
        self.waypoint_tool.as_ref().is_some_and(|t| t.0)
            || self.waypoint_menu.as_ref().is_some_and(|m| m.0)
            || self.spawn_tool.as_ref().is_some_and(|t| t.0)
            || self.terrain_tool.as_ref().is_some_and(|t| t.0)
    }
}

/// "The user asked to back out" — the [`UserIntent::Cancel`] intent.
///
/// Read this instead of sniffing `KeyCode::Escape`/`Backspace`: the bindings are DATA
/// (`assets/config/keybindings.json`), so a rebind works everywhere at once and every
/// mode agrees on what cancelling means. Suppressed while an egui field has keyboard
/// focus, so Backspace typed into a text box edits text rather than backing out.
#[derive(bevy::ecs::system::SystemParam)]
pub struct CancelIntent<'w, 's> {
    avatars: Query<'w, 's, &'static IntentState, With<Avatar>>,
    egui_focus: Res<'w, EguiFocus>,
}

impl CancelIntent<'_, '_> {
    /// True on the frame the user pressed Cancel.
    pub fn just_pressed(&self) -> bool {
        if self.egui_focus.wants_keyboard {
            return false;
        }
        self.avatars.iter().any(|i| i.just_pressed(&UserIntent::Cancel))
    }
}

/// True while a waypoint's right-click context menu is open.
///
/// Read by avatar mouse-look to hold the camera still. `Look` is bound to raw
/// `MouseMove` (always-on, FPS-style) and is only suppressed once the pointer is
/// already OVER egui — so without this the camera spins while you travel the cursor
/// to the menu, which made the menu effectively unusable. Set/cleared by
/// sandbox-edit's waypoint menu. Deliberately separate from [`WaypointToolActive`]:
/// during ground-placement you still WANT to look around.
#[derive(Resource, Default)]
pub struct WaypointMenuOpen(pub bool);

/// True while the waypoint editor is waiting for a "click the ground to place"
/// (Move / Insert-after, armed from a waypoint's right-click menu). Read by avatar
/// possession and entity selection to suppress their click handling — that click
/// belongs to the placement, not to possess/select. Mirrors [`SpawnToolActive`] and
/// [`TerrainToolActive`]; set/cleared by sandbox-edit's waypoint systems.
#[derive(Resource, Default)]
pub struct WaypointToolActive(pub bool);

/// Per-entity marker: this entity is currently being dragged by the editor
/// transform gizmo.
///
/// Set/cleared by sandbox-edit's gizmo systems (an editor/UI concern that lives
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

/// Control-signal propagation set (the DAC step): `DigitalPort` → `PhysicalPort`
/// via [`Wire`]. Runs on the **fixed** clock so the actuation path is
/// frame-rate-independent and identical on every peer.
///
/// This is load-bearing for client-prediction determinism. The DAC used to run
/// in `Update` (render rate) while its producer (flight-software command
/// observers) and consumers (wheel/hardware actuators) run in `FixedUpdate`. The
/// latency between "input applied" and "force applied" was therefore coupled to
/// frame rate, so the same input `seq` landed on the wheels a *different* number
/// of physics ticks apart on host vs client (which render at independent rates),
/// and the client's prediction never matched the host — every snapshot ack
/// corrected, showing up as steering jitter. Keeping the DAC on the sim clock
/// removes that coupling. Actuators that read `PhysicalPort` order `.after` this.
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
/// crates**: the spawn half (`apply_replicated_spawns`, in `lunco-sandbox-edit`,
/// because it instantiates from the spawn catalog) must run before the prediction
/// half (interp / kinematic-pin / reconcile / rollback, in `lunco-networking`).
/// The two used to sit in one `.chain()` in a single file; a plain `.chain()` can't
/// express the ordering across the crate boundary, and neither crate may depend on
/// the other (`lunco-networking` must never gain a `lunco-sandbox-edit` edge — see
/// its Cargo.toml, review A6). `lunco-core` is the one crate both already depend on,
/// so the shared set lives here.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetcodeSet {
    /// Instantiate host-replicated spawns (`apply_replicated_spawns`, sandbox-edit).
    InstantiateSpawns,
    /// The client-prediction pipeline (`lunco-networking::prediction`), after the
    /// spawns it may act on exist.
    Predict,
}

impl Plugin for LunCoCorePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LunCoLogPlugin);
        app.add_plugins(BigSpaceInvariantsPlugin);
        app.register_type::<GridAnchor>()
           .register_type::<NeedsGroundSettle>()
           .register_type::<SoiMigrant>()
           .register_type::<ActuatorDrivenJoint>()
           // `telemetry::` — bevy 0.19's prelude exports its own `Severity`
           // (log-level type), which shadows ours in glob-import scopes.
           .register_type::<crate::telemetry::Severity>()
           .register_type::<TelemetryValue>()
           .register_type::<TelemetryEvent>()
           .register_type::<Parameter>()
           .register_type::<SampledParameter>()
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
           .register_type::<RestoreFallbackLights>()
           .register_type::<kernels::DriveMix>()
           .register_type::<CameraFollow>()
           .register_type::<SimTick>();

        // NOTE: the `ControlKernelRegistry` resource is owned/seeded by the plugin
        // that actually runs the allocation systems (lunco-mobility, like
        // `PortRegistry`), so a minimal app that runs drive systems without the full
        // core plugin still has it. Core only DEFINES the kernels + `DriveMix` type.
        // All always-on core/substrate resources live in one function so a
        // unit test can assert the full set is present without building the
        // heavier LunCoCorePlugin (log + big-space). See its doc comment for
        // the invariant this enforces.
        register_core_resources(app);
        // Runtime subsystem toggles (progressive-fidelity substrate) +
        // `SetSubsystemEnabled` command.
        subsystems::build_subsystems(app);
        // DAC (DigitalPort → PhysicalPort) on the FIXED clock — see `ControlDacSet`
        // for why this must not run in `Update` (prediction determinism).
        app.add_systems(FixedUpdate, wire_system.in_set(ControlDacSet))
           .add_systems(FixedUpdate, advance_sim_tick)
           .add_systems(PostUpdate, assign_global_entity_ids);
        // Host: keep the per-gid input-ack watermarks keyed to their CURRENT owner.
        // A re-possessed vessel must not keep acking the previous owner's `seq`
        // stream — see `AppliedInputSeq`. Change-detected on the registry, so it
        // costs nothing on a steady frame; always-on substrate (no wire dep).
        app.add_systems(FixedFirst, sync_applied_seq_owners);
        // Rollback replay mirrors the DAC (and ONLY the DAC from this crate —
        // `advance_sim_tick` is deliberately excluded: a replayed tick must not
        // advance the simulation's tick counter).
        app.init_resource::<RollbackInProgress>();
        app.add_systems(RollbackReplay, wire_system.in_set(ControlDacSet));
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
        .init_resource::<session::NetworkRole>()
        .init_resource::<session::LocalSession>()
        .init_resource::<session::SyncApplyGuard>()
        .init_resource::<session::NetStatus>()
        .init_resource::<session::PendingConnect>()
        .init_resource::<session::SessionRegistry>()
        .init_resource::<session::SessionProfiles>()
        .init_resource::<session::SessionRbac>()
        .init_resource::<session::CommandPolicyRegistry>()
        .init_resource::<session::PendingReplicatedSpawns>()
        .init_resource::<session::IncomingSnapshots>()
        // Input-sequence bookkeeping is always-on substrate: the
        // lunco-controller observers read/write these every frame whether
        // or not the optional networking wire is present.
        .init_resource::<session::OwnedInputLog>()
        .init_resource::<session::BufferedClientInputs>()
        .init_resource::<session::LocalDriveInput>()
        .init_resource::<session::AppliedInputSeq>()
        // Client desync gauge (review N3) — written by the reconcilers, read by the
        // netcode diagnostics. Always-on substrate: the reconcilers run whether or
        // not the wire feature is compiled in.
        .init_resource::<session::DivergenceStats>()
        // Command-result substrate: result-reporting `#[on_command]` observers
        // require these to exist (same always-on rule as the session resources
        // above — see the AppliedInputSeq fix).
        .init_resource::<CommandResults>()
        .init_resource::<ActiveCommandId>();
}

/// HOST: re-key the input-ack watermarks against the authoritative ownership
/// table whenever it changes (a claim, a release, a disconnect).
///
/// Without this, a vessel re-possessed by a second client keeps stamping the FIRST
/// client's `seq` (e.g. 5000) into every snapshot. The new owner's client latches
/// that as `last_reconciled`, then early-returns on every subsequent (lower) ack —
/// its prediction is never reconciled again, and the rover drifts without bound.
/// This is the failure users hit in ordinary play, with no attacker involved.
///
/// Also frees the slot of a vessel nobody owns any more, so the map tracks the
/// live ownership table rather than growing across possession churn.
pub fn sync_applied_seq_owners(
    role: Res<session::NetworkRole>,
    registry: Res<session::SessionRegistry>,
    mut applied: ResMut<session::AppliedInputSeq>,
    mut buffered: ResMut<session::BufferedClientInputs>,
) {
    if !role.is_host() || !registry.is_changed() {
        return;
    }
    // Any gid whose owner changed loses BOTH its ack watermark and whatever the
    // previous owner had queued but unintegrated — replaying A's inputs into B's
    // vessel would be a control leak, not just a stale ack.
    for gid in applied
        .changed_owner_gids(&registry)
        .into_iter()
        .collect::<Vec<_>>()
    {
        buffered.clear_gid(gid);
    }
    applied.sync_owners(&registry);
}

/// Advance the discrete [`SimTick`] once per fixed step, *only while time is
/// actually flowing* (so a paused/zero-speed/warping world freezes the tick and
/// peers stay comparable). The gate is the direct clock state
/// `Time<Virtual>.effective_speed > 0` — the same predicate the physics-stepping
/// systems use. `effective_speed`, not `relative_speed`: the spine expresses
/// "frozen" with Bevy's paused flag (which zeroes the former but not the latter),
/// because `relative_speed` is a rate that consumers divide by.
/// `Time<Virtual>` is read optionally: a bare world without Bevy's
/// `TimePlugin` (e.g. a headless unit test) is treated as running.
fn advance_sim_tick(mut tick: ResMut<SimTick>, vtime: Option<Res<Time<Virtual>>>) {
    let running = vtime.map_or(true, |t| !t.is_paused() && t.relative_speed_f64() > 0.0);
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
    q_new: Query<
        (Entity, Option<&Provenance>, Has<session::SkipContentStamp>),
        Without<GlobalEntityId>,
    >,
    // Authority is derived from the role, not a separate `IsServer` flag — the two
    // used to drift (a `Standalone` sandbox with `IsServer(false)` minted no ids
    // for runtime spawns). `Host` and `Standalone` mint; a pure `Client` never
    // reaches the minting arms because it pins host-allocated ids via replication.
    role: Res<session::NetworkRole>,
    mut warned: Local<bool>,
) {
    let is_authoritative = role.is_authoritative();
    for (entity, prov, runtime_instance) in q_new.iter() {
        // Runtime-instanced subtree root (gap G2 / B.1): server-allocated unique
        // identity, ignoring any `Content` stamp the USD loader adds. Two
        // instances of the same asset would otherwise derive the *same*
        // content id and collide. Clients receive the id via spawn-replication
        // (they pin `GlobalEntityId::from_raw` directly, so they never reach
        // here for these roots).
        if runtime_instance {
            if is_authoritative {
                commands
                    .entity(entity)
                    .try_insert(GlobalEntityId::allocate_authoritative());
            }
            continue;
        }
        match prov {
            Some(Provenance::Local) => { /* never networked, no id */ }
            Some(p @ (Provenance::Content { .. } | Provenance::Derived { .. })) => {
                if let Some(id) = identity::derive_id(p) {
                    commands.entity(entity).try_insert(GlobalEntityId::from_raw(id));
                }
            }
            Some(Provenance::Authoritative) => {
                // Only an authoritative peer mints; clients receive the id via replication.
                if is_authoritative {
                    commands
                        .entity(entity)
                        .try_insert(GlobalEntityId::allocate_authoritative());
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
                    .try_insert(GlobalEntityId::allocate_authoritative());
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
                // CQ-514: skip a zero/non-finite scale (warn once) so a
                // misconfigured wire can't push NaN/inf into PhysicalPort.
                if !wire.scale.is_finite() || wire.scale == 0.0 {
                    warn_once!("Wire scale is zero or non-finite ({}); skipping", wire.scale);
                    continue;
                }
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

    /// App with just the Ph1 systems + resources, nothing else. `is_server` maps
    /// onto the authority role: an authoritative peer is `Standalone` (equally
    /// `Host` — same minting arm), a non-authoritative one is a pure `Client`.
    fn ph1_app(is_server: bool) -> App {
        let role = if is_server {
            session::NetworkRole::Standalone
        } else {
            session::NetworkRole::Client
        };
        let mut app = App::new();
        app.insert_resource(role)
            .init_resource::<SimTick>()
            .add_systems(FixedUpdate, advance_sim_tick)
            .add_systems(PostUpdate, assign_global_entity_ids);
        app
    }

    /// Guards the "substrate resource initialized only behind an optional
    /// feature" bug class (the `AppliedInputSeq` single-player panic,
    /// 2026-06-03). Builds the resource set exactly as `LunCoCorePlugin`
    /// does — via `register_core_resources` — and asserts every always-on
    /// substrate resource exists. If an init is moved out into a
    /// feature-gated plugin (e.g. `SyncPlugin`), this fails in CI (default
    /// features = networking off) long before a real single-player run can
    /// panic.
    #[test]
    fn core_substrate_resources_present() {
        let mut app = App::new();
        register_core_resources(&mut app);
        let w = app.world();
        assert!(w.get_resource::<SimTick>().is_some());
        assert!(w.get_resource::<session::NetworkRole>().is_some());
        assert!(w.get_resource::<session::LocalSession>().is_some());
        assert!(w.get_resource::<session::SyncApplyGuard>().is_some());
        assert!(w.get_resource::<session::NetStatus>().is_some());
        assert!(w.get_resource::<session::SessionRegistry>().is_some());
        assert!(w.get_resource::<session::PendingReplicatedSpawns>().is_some());
        assert!(w.get_resource::<session::IncomingSnapshots>().is_some());
        // The two resources that caused the original panic — nailed down
        // explicitly so a regression names them.
        assert!(w.get_resource::<session::OwnedInputLog>().is_some());
        assert!(w.get_resource::<session::AppliedInputSeq>().is_some());
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

    /// Authority is a pure function of the role — the single source of truth that
    /// replaced the drift-prone `IsServer` flag.
    #[test]
    fn authority_derives_from_role() {
        assert!(session::NetworkRole::Standalone.is_authoritative());
        assert!(session::NetworkRole::Host.is_authoritative());
        assert!(!session::NetworkRole::Client.is_authoritative());
    }

    /// Regression guard (2026-07-15): a palette/API spawn tags its root
    /// `SkipContentStamp` (a "runtime instance"). A `Standalone` single-player
    /// sandbox is authoritative and MUST mint an id for it — without one,
    /// possession can't claim ownership and a `piloted`-gated lander goes dead
    /// (the whole reason for this refactor). A pure `Client` must NOT mint — it
    /// pins the host's id via replication.
    #[test]
    fn runtime_instance_root_minted_only_when_authoritative() {
        let mut standalone = ph1_app(true);
        let e = standalone.world_mut().spawn(session::SkipContentStamp).id();
        standalone.world_mut().run_schedule(PostUpdate);
        assert!(
            id_of(&mut standalone, e).is_some(),
            "Standalone must mint a GlobalEntityId for a runtime-instanced spawn"
        );

        let mut client = ph1_app(false);
        let ce = client.world_mut().spawn(session::SkipContentStamp).id();
        client.world_mut().run_schedule(PostUpdate);
        assert_eq!(
            id_of(&mut client, ce),
            None,
            "a pure Client must not mint — it pins the host-allocated id"
        );
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

        // Running world: `Time<Virtual>` default `relative_speed` is 1.0 (> 0), so
        // the tick advances each fixed step. This is the single gate — no separate
        // `TimeWarpState`.
        app.insert_resource(Time::<Virtual>::default());
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<SimTick>().0, 1);
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<SimTick>().0, 2);

        // Paused (Bevy's paused flag — `effective_speed == 0` while
        // `relative_speed` stays a positive rate): tick frozen.
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<SimTick>().0, 2);

        // Resumed: the tick advances again.
        app.world_mut().resource_mut::<Time<Virtual>>().unpause();
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<SimTick>().0, 3);
    }
}
