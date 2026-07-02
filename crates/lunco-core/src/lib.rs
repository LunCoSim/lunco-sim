//! Core types and plugins for the LunCo simulation.
//!
//! This crate provides the foundational components, resources, and systems used
//! across the simulation, including physical properties, celestial timing,
//! and the core plugin registration.

pub mod architecture;
pub mod kernels;
pub mod mocks;
pub mod ports;
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

pub use architecture::*;
pub use derived::RebuildOnChange;
pub use mocks::*;
pub use telemetry::*;
pub use log::*;
pub use commands::{
    Ack, ActiveCommandId, CommandOutcome, CommandResults, Mutation, OpId, Reject, SessionId,
    SyncChannel,
};
pub use markers::{
    ActuatorDrivenJoint, EmbeddedScenarioPath, EmbeddedScenarioSource, FallbackSceneLight,
    GridAnchor, HorizonShadowTerrain, NextScene, RestoreFallbackLights, ScriptParams, SoiMigrant, SunAngularDiameter, TriggerZone,
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
    authorize, AppliedInputSeq, ArticulatedLink, ArticulatedVehicle, IncomingSnapshots, InputFrame,
    LocalSession,
    NetConnectRequest, NetDisconnectRequest,
    NetExcluded, NetReplicate, NetSpawn, PendingConnect, PendingConnectRequest,
    NetStatus, NetworkRole, NotPredictable, OwnedInputLog, OwnedLocally, PendingReplicatedSpawns,
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

impl Plugin for LunCoCorePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LunCoLogPlugin);
        app.add_plugins(BigSpaceInvariantsPlugin);
        app.register_type::<GridAnchor>()
           .register_type::<SoiMigrant>()
           .register_type::<ActuatorDrivenJoint>()
           .register_type::<Severity>()
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
        .init_resource::<IsServer>()
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
        .init_resource::<session::AppliedInputSeq>()
        // Command-result substrate: result-reporting `#[on_command]` observers
        // require these to exist (same always-on rule as the session resources
        // above — see the AppliedInputSeq fix).
        .init_resource::<CommandResults>()
        .init_resource::<ActiveCommandId>();
}

/// Advance the discrete [`SimTick`] once per fixed step, *only while time is
/// actually flowing* (so a paused/zero-speed/warping world freezes the tick and
/// peers stay comparable). The gate is the direct clock state
/// `Time<Virtual>.relative_speed > 0` — the same predicate the physics-stepping
/// systems use. `Time<Virtual>` is read optionally: a bare world without Bevy's
/// `TimePlugin` (e.g. a headless unit test) is treated as running.
fn advance_sim_tick(mut tick: ResMut<SimTick>, vtime: Option<Res<Time<Virtual>>>) {
    let running = vtime.map_or(true, |t| t.relative_speed_f64() > 0.0);
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
    is_server: Res<IsServer>,
    mut warned: Local<bool>,
) {
    for (entity, prov, runtime_instance) in q_new.iter() {
        // Runtime-instanced subtree root (gap G2 / B.1): server-allocated unique
        // identity, ignoring any `Content` stamp the USD loader adds. Two
        // instances of the same asset would otherwise derive the *same*
        // content id and collide. Clients receive the id via spawn-replication
        // (they pin `GlobalEntityId::from_raw` directly, so they never reach
        // here for these roots).
        if runtime_instance {
            if is_server.0 {
                commands
                    .entity(entity)
                    .insert(GlobalEntityId::allocate_authoritative());
            }
            continue;
        }
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

    /// App with just the Ph1 systems + resources, nothing else.
    fn ph1_app(is_server: bool) -> App {
        let mut app = App::new();
        app.insert_resource(IsServer(is_server))
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
        assert!(w.get_resource::<IsServer>().is_some());
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

        // Paused (`relative_speed == 0`): tick frozen.
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .set_relative_speed_f64(0.0);
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<SimTick>().0, 2);
    }
}
