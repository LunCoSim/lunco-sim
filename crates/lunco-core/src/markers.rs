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

/// A freshly-activated physics body that needs a ONE-TIME drop-onto-terrain
/// placement before it settles.
///
/// Added when a body flips to `RigidBody::Dynamic` (`activate_dynamic_bodies`),
/// consumed by the terrain ground-settle system, which lifts the whole
/// joint-connected assembly so its lowest member clears the terrain surface, then
/// removes the marker.
///
/// **Why**: authored physical rovers put their chassis at the surface with the
/// wheels hanging BELOW it. avian terrain colliders are one-sided parry
/// heightfields — a body that starts even slightly below the surface gets no
/// upward contact and sinks forever. Command-spawned rovers avoid this via a
/// collision-AABB rest-depth lift; raycast rovers avoid it because their ray finds
/// the surface regardless. Authored physical rovers get neither, so they need this
/// one-time settle. This is correct initial PLACEMENT — not a per-frame rescue.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct NeedsGroundSettle;

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

/// Trigger event/command to request restoration of default fallback lights.
/// This is fired when the active scene is cleared or reloaded, to guarantee
/// that a fallback sun is present if the incoming scene authors no lighting.
#[derive(Event, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Debug, Default)]
pub struct RestoreFallbackLights;

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

/// Opt-in marker for static terrain that self-shadows by **ray-marching a
/// baked heightfield**, instead of the realtime cascade shadow map (which
/// cannot resolve kilometre-scale terrain shadows: at a grazing sun the
/// required depth bias scales as `1/tan(elevation)`, so any bias that stops
/// acne peter-pans the shadow tens of metres).
///
/// Stamped by loaders (the USD loader reads
/// `custom bool lunco:terrain:horizonShadows`); consumed by
/// `lunco-environment`'s horizon-shadow system, which bakes a `resolution`²
/// heightfield of the terrain's local XZ bounding box and marches it per
/// pixel. Universal across bodies: the bake is sun-agnostic — geometry only —
/// so any sun direction is evaluated against it at runtime.
///
/// **Not a horizon-angle map**, despite the name. Storing horizon *angles* per
/// grid point was tried and rejected: it low-pass-filters the casting crests
/// and smears the terminator over tens to hundreds of metres (see
/// `lunco-environment/src/horizon.rs`). This doc used to describe that
/// rejected design, and an `azimuths` field survived it — declared, parsed
/// from `lunco:terrain:horizonMapAzimuths`, and read by nothing. Both are
/// gone; a ray-march has no azimuth slices to interpolate between.
///
/// Lives in `lunco-core` so loader crates and the environment crate can
/// share it without depending on each other (same pattern as
/// [`Provenance`](crate::Provenance)).
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct HorizonShadowTerrain {
    /// Side length of the square heightfield baked over the terrain's local XZ
    /// bounding box. Default 512 — matched to typical DEM vertex spacing;
    /// raise for finer source data.
    pub resolution: u32,
}

impl Default for HorizonShadowTerrain {
    fn default() -> Self {
        Self { resolution: 512 }
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

/// A per-entity scenario referenced by FILE PATH in USD — the
/// `uniform asset lunco:program:sourceAsset = @scenarios/foo.rhai@` of a
/// `LunCoProgram` prim — awaiting load.
///
/// The asset-relative path to a `.rhai` source. The USD loader
/// (`lunco-usd-bevy`) stamps this on the prim that OWNS the program;
/// `lunco-scripting` loads the file through the `AssetServer` (wasm-safe — no
/// `std::fs`) and, once ready, replaces it with an [`EmbeddedScenarioSource`]
/// so the normal attach path runs. Keeps scenarios as editable, hot-reloadable,
/// reusable `.rhai` files instead of strings baked into the scene. Lives in
/// `lunco-core` so the loader and scripting runtime share the contract without
/// depending on each other (same as [`EmbeddedScenarioSource`]).
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct EmbeddedScenarioPath(pub String);

/// The USD path of the `LunCoProgram` prim a running scenario came from.
///
/// A script's `me` is its OWNER — the vessel it acts for — so the runtime hangs the
/// scenario on the owner's entity. But the program is a prim of its own, and that is
/// where its source belongs: saving a live-edited scenario authors
/// `lunco:program:sourceCode` back onto THIS path, not onto the vessel. Without it a
/// save has no idea which of an owner's programs it is saving, and would write the
/// source onto a prim that runs nothing.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct ScenarioProgramPrim(pub String);

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

/// The scene to load next when this scene's mission completes — the tutorial
/// CHAIN, declared as DATA in USD (`custom string lunco:nextScene =
/// "scenes/foo.usda"` on any prim, conventionally the scenario prim). A generic
/// handler loads it on `MISSION_COMPLETE`, so a course flows scene→scene with no
/// per-tutorial Rust and no central campaign object — each tutorial names its own
/// successor. Empty/absent = end of chain. Lives in `lunco-core` so the USD loader
/// and the tutorial handler share the contract (same pattern as [`TriggerZone`]).
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct NextScene(pub String);
