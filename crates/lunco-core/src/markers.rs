//! Architectural marker components for the big_space integration.
//!
//! These markers carry semantic intent that the raw big_space components
//! (`Grid`, `CellCoord`) don't express. They're the contract between the
//! coords helpers, the SOI plugin, the gizmo system, and the loaders.

use bevy::prelude::*;
use big_space::prelude::CellCoord;

/// A spatial entity that moves as a single unit — rover, ball, vessel, avatar,
/// terrain tile, scene-level light.
///
/// **Invariant**: a `GridAnchor` is a direct child of a big_space `Grid`. It
/// carries `CellCoord` (auto-inserted via `#[require]`) and its own
/// `Transform`. Its descendants are plain-`Transform` children whose
/// `GlobalTransform` propagates via big_space's `propagate_low_precision`.
///
/// Selection, dragging, possession, and SOI migration all operate on
/// `GridAnchor` entities — never on their descendants.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[require(CellCoord)]
#[reflect(Component)]
pub struct GridAnchor;

/// Marker: this revolute joint's **motor is owned by an external actuator**
/// (a velocity drive or a frame-steer), not by the cosim joint backend.
///
/// Every `RevoluteJoint` is auto-exposed as a cosim model with an `angle` port,
/// and [`lunco_cosim::apply_joint_drives`] position-holds that joint's
/// `motor.target_position` toward the commanded `angle`. That is correct for a
/// mast/panel posed by a wire or the Inspector slider, but **wrong** for a rover
/// wheel: those are spun by `lunco_hardware::MotorActuator` (a velocity motor)
/// and steered by `SteeringActuator` (a frame rotation). If both wrote the same
/// `motor`, the position-hold would zero the velocity command every tick and
/// freeze the wheel.
///
/// So `apply_joint_drives` skips any joint carrying this marker; the actuator is
/// the single owner of its motor. `lunco_hardware` stamps it automatically when
/// a `MotorActuator`/`SteeringActuator` is added. Lives in `lunco-core` so the
/// cosim backend and the hardware actuators can agree on the contract without
/// depending on each other (same pattern as [`HorizonShadowTerrain`]).
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct ActuatorDrivenJoint;

/// A `GridAnchor` that participates in cross-Grid SOI migration.
///
/// Rovers, spacecraft, free-flying probes — anything whose dominant
/// gravitational body can change at runtime. Static terrain and decoration
/// are explicitly *not* `SoiMigrant`.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct SoiMigrant;

/// Tag for a binary's built-in default sun (or other default lights).
/// The USD loader despawns every `FallbackSceneLight` the moment a scene
/// authors its own light prim — scene lighting is the source of truth.
/// Lives in `lunco-core` so every light-spawning crate (binaries,
/// `lunco-celestial`'s solar-system bootstrap) can tag without depending
/// on the USD stack.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct FallbackSceneLight;

/// Angular **diameter** of a sun (`DirectionalLight`) in degrees, from the
/// UsdLux `inputs:angle` attribute (Sol from Earth/Moon ≈ 0.53°). Drives
/// physically-scaled penumbra width in the horizon-shadow ray-march:
/// shadows are razor-sharp next to the caster and soften with distance.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct SunAngularDiameter(pub f32);

impl Default for SunAngularDiameter {
    fn default() -> Self {
        Self(0.53)
    }
}

/// Opt-in marker for static terrain that self-shadows via a baked
/// multi-azimuth **horizon map** instead of the realtime cascade shadow
/// map (which cannot resolve kilometre-scale terrain shadows).
///
/// Stamped by loaders (the USD loader reads
/// `custom bool lunco:terrain:horizonShadows`); consumed by
/// `lunco-environment`'s horizon-shadow system, which bakes horizon
/// elevation angles from the terrain's `Mesh3d` for `azimuths` compass
/// directions over a `resolution`² grid. Universal across bodies: the
/// bake is sun-agnostic — any sun direction is evaluated against it at
/// runtime, so it works wherever the terrain (and its star) is.
///
/// Lives in `lunco-core` so loader crates and the environment crate can
/// share it without depending on each other (same pattern as
/// [`Provenance`](crate::Provenance)).
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct HorizonShadowTerrain {
    /// Side length of the square heightmap / visibility grid baked over
    /// the terrain's local XZ bounding box. Default 512 — matched to
    /// typical DEM vertex spacing; raise for finer source data.
    pub resolution: u32,
    /// Number of compass directions horizon angles are baked for.
    /// Runtime sun azimuths interpolate between adjacent slices.
    pub azimuths: u32,
}

impl Default for HorizonShadowTerrain {
    fn default() -> Self {
        Self { resolution: 512, azimuths: 16 }
    }
}

/// A per-entity scenario source EMBEDDED in USD (`custom string lunco:script`),
/// awaiting attachment to the runtime.
///
/// The USD loader (`lunco-usd-bevy`) stamps this when a prim carries a
/// `lunco:script` attribute; `lunco-scripting` drains it — attaching the source
/// as a rhai `ScriptedModel` and removing the marker — so a scenario travels
/// WITH the Twin/scene and starts running when its entity spawns.
///
/// Lives in `lunco-core` so the loader and the scripting runtime share the
/// contract without depending on each other (same pattern as
/// [`HorizonShadowTerrain`] / [`FallbackSceneLight`]).
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct EmbeddedScenarioSource(pub String);

/// A per-entity scenario referenced by FILE PATH in USD
/// (`custom string lunco:scriptPath = "scenarios/foo.rhai"`), awaiting load.
///
/// The asset-relative path to a `.rhai` source. The USD loader
/// (`lunco-usd-bevy`) stamps this when a prim carries `lunco:scriptPath`;
/// `lunco-scripting` loads the file through the `AssetServer` (wasm-safe — no
/// `std::fs`) and, once ready, replaces it with an [`EmbeddedScenarioSource`]
/// so the normal attach path runs. Keeps scenarios as editable, hot-reloadable,
/// reusable `.rhai` files instead of strings baked into the scene. Lives in
/// `lunco-core` so the loader and scripting runtime share the contract without
/// depending on each other (same as [`EmbeddedScenarioSource`]).
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct EmbeddedScenarioPath(pub String);

/// A named overlap **trigger zone** (geofence) — the discrete-event twin of a
/// continuous port signal.
///
/// Stamped by the USD loader (`lunco-usd-avian`) on a prim carrying
/// `custom string lunco:triggerZone = "<name>"` (alongside an avian `Sensor` +
/// collider shape). `lunco-mobility`'s collision bridge fires
/// `enter:<name>` / `exit:<name>` [`TelemetryEvent`]s (payload = the entrant's
/// gid) when a body crosses the volume; scenarios react in rhai via
/// `wait_for("enter:<name>")` / `entered_zone(evt, "<name>")` — no per-tick
/// distance polling, detection happens in avian.
///
/// Decouples the event/signal NAME from the entity's `Name` (its USD path) so
/// zone names stay short and stable. Lives in `lunco-core` so the loader and the
/// collision bridge share the contract without depending on each other (same
/// pattern as [`EmbeddedScenarioPath`]).
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct TriggerZone(pub String);

/// Avian collision-layer BIT reserved for [`TriggerZone`] sensor volumes.
///
/// A trigger must fire OVERLAP events for the rover, yet never be a physical or
/// ray-castable obstacle to anything. avian sensors are non-solid for *contacts*
/// but ARE still hit by spatial queries (the rover's wheel-suspension raycasts
/// and the chase-camera anti-clip raycast), so without this the rover rides up
/// on the invisible sphere and the camera clips on it. The avian-using crates
/// put trigger colliders on this layer and MASK IT OUT of those raycasts
/// (`LayerMask(!TRIGGER_COLLISION_LAYER)`), while keeping it in the contact
/// pipeline so overlap events still fire. Kept as a bare `u32` because
/// `lunco-core` has no avian dependency. Bit 7 is outside the default gameplay
/// layers (avian default = all bits).
pub const TRIGGER_COLLISION_LAYER: u32 = 1 << 7;

/// Per-prim numeric **script parameters**, authored in USD as
/// `custom string lunco:params = "wmax=1.05, lmax=3.6, flick=1.0"` and read by a
/// script via the native `param(me, "wmax", default)` verb (a HashMap lookup —
/// fast, typed, no fragile `name(me).contains(...)` string scanning).
///
/// This is how a reusable script gets PER-INSTANCE config: the same `flame.rhai`
/// drives many cones, each carrying its own `lunco:params`, instead of inferring
/// its role from its name. Stamped by the USD loader (`lunco-usd-bevy`); lives in
/// `lunco-core` so loader and scripting runtime share it (same pattern as
/// [`TriggerZone`]).
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct ScriptParams(pub std::collections::HashMap<String, f64>);
