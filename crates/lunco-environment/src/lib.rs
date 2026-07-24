//! # lunco-environment
//!
//! Per-entity environmental state computed from celestial body providers.
//!
//! See `README.md` for the full architecture, rationale, and how to add new
//! environment domains (atmosphere, radiation, magnetic field, etc.).
//!
//! Currently implements **gravity only**. Other domains follow the same
//! pattern — see the README for templates.

use avian3d::prelude::{Forces, Mass, RigidBody, WriteRigidBodyForces};
use bevy::math::DVec3;
use bevy::prelude::*;
// All render-FREE: `CascadeShadowConfig` / `GlobalAmbientLight` are `bevy_light`,
// `Exposure` is `bevy_camera`. Neither depends on `bevy_render`. The one knob in
// `SetEnvironmentLight` that IS render-bound — `bloom_intensity` — is applied by a
// second observer in `lunco-render-bevy` (`env_light.rs`), so this crate names no
// post-processing type. See docs/architecture/render-decoupling.md.
use bevy::camera::Exposure;
use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder, GlobalAmbientLight};
use lunco_core::{on_command, register_commands, Command};

/// USD prim type for the scene-level **environment settings** prim (a singleton
/// under the default prim, e.g. `/World/Environment`). It carries the render
/// knobs that have no natural light-prim home — `lunco:env:exposureEv100`,
/// `bloomIntensity`, `earthshineIntensity`, `earthshineColor`.
///
/// **Ambient is not among them.** Uniform environment illumination is standard
/// UsdLux — an untextured `DomeLight` — and `GlobalAmbientLight` is composed as
/// the sum over those domes. The ambient slider therefore persists onto a
/// `DomeLight` child of this prim (`<Environment>/AmbientFill`), not onto a
/// custom attribute here; a custom attribute would be a second spelling of a
/// standard thing, and the two spellings fought over the same field.
/// The sandbox persists a `SetEnvironmentLight` render tweak onto this prim and a
/// projector reads it back on stage change — so those knobs journal + round-trip
/// like every other USD edit, WITHOUT coupling the light loader to global/camera
/// render state (they live on their own prim, read by their own system).
pub const LUNCO_ENVIRONMENT_PRIM_TYPE: &str = "LunCoEnvironment";

/// Gravity configuration types (`Gravity`, `GravityBody`, `GravityProvider`,
/// `GravityModel`) — environmental-state vocabulary owned here. The gravity
/// *systems* in `lunco_celestial` import these.
pub mod gravity_types;
pub use gravity_types::{
    Gravity, GravityBody, GravityModel, GravityProvider, EARTH_SURFACE_GRAVITY,
    MOON_SURFACE_GRAVITY,
};

/// Physical lighting parameters of the lunar sky (`LunarSun`, `EarthshineParams`)
/// — environmental state, the lighting analog of gravity. See the module docs.
pub mod lighting;
pub use lighting::{EarthshineParams, LunarSun};

/// Solar direction as a co-simulation source (`LocalSolar` + the sun→cosim
/// bridge). The lighting-direction analog of the gravity bridge.
///
/// **Render-free.** It reads the scene `DirectionalLight` (`bevy_light`) and
/// filters on `RenderLayers` (`bevy_camera`) — neither depends on `bevy_render`,
/// so the sun→cosim feed works on a headless server exactly as it does in the
/// GUI. See `docs/architecture/render-decoupling.md`.
pub mod solar;
pub use solar::{compute_local_solar, inject_local_solar_into_cosim, LocalSolar};

/// Earth's direction as a co-simulation source (`LocalEarth` + the earth→cosim
/// bridge) — what a high-gain antenna points at, the twin of [`solar`] for the
/// other body in a lunar sky.
///
/// Unlike the sun there is no scene light to read, so the direction arrives in
/// the [`earth::EarthDirectionWorld`] resource, written by `lunco-celestial`
/// from the ephemeris. See the module docs for why the dependency runs that way.
pub mod earth;
pub use earth::{
    compute_local_earth, inject_local_earth_into_cosim, EarthDirectionWorld, LocalEarth,
};

// Empty-bounds fallbacks for `SetEnvironmentLight`'s cascade rebuild. These
// mirror `lunco_render::LunarSunShadow`'s defaults but are kept locally so this
// crate need not depend on `lunco-render` (lighting → render would invert the
// layering: render is presentation, below environment). Keep in sync by hand if
// the render defaults change — they rarely do, and a drift only affects the
// runtime tuner's fallback when no live cascade bounds exist.
const FALLBACK_FIRST_CASCADE_FAR_BOUND: f32 = 40.0;
const FALLBACK_MAX_SHADOW_DISTANCE: f32 = 1500.0;

/// Baked horizon-map terrain self-shadowing (the long-range half of the
/// two-system shadow design). **Render-free**: the heightfield bakes and the
/// sun-visibility cache run headless; the material wiring they feed lives in
/// `lunco-render-bevy::horizon_shade`. See the module docs.
pub mod horizon;
pub use horizon::{
    install_horizon_map_from_field, pick_sun, HeightField, HorizonMap, HorizonShadowCache,
    HorizonShadowCacheConfig, HorizonShadowPlugin, SunQuery,
};

/// System sets for environment computation and consumption.
///
/// Ordered chain in [`FixedUpdate`]:
/// 1. [`Compute`](EnvironmentSet::Compute) — write `Local*` components from providers
/// 2. [`Apply`](EnvironmentSet::Apply) — consumers like Avian gravity force application
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnvironmentSet {
    /// Computes per-entity environment components from body providers.
    Compute,
    /// Applies environment effects (e.g., gravity force on RigidBodies).
    Apply,
}

// ─────────────────────────────────────────────────────────────────────────────
// LocalGravity — the gravity vector at an entity's position
// ─────────────────────────────────────────────────────────────────────────────

/// Gravity vector at this entity's position, in world space (m/s²).
///
/// Computed each [`FixedUpdate`] from the [`Gravity`] resource and (for
/// surface gravity) the [`GravityProvider`] on the entity's gravitational
/// parent body (linked via [`GravityBody`]).
///
/// - **Magnitude:** `length()` gives `g` in m/s²
/// - **Direction:** `normalize()` gives the gravity unit vector
///
/// Read this instead of querying the [`Gravity`] resource directly — it's
/// position-dependent and cached. Multiple consumers (Avian force application,
/// cosim input injection, UI display) can read it without recomputation.
#[derive(Component, Debug, Clone, Copy, Reflect, Default)]
#[reflect(Component)]
pub struct LocalGravity(pub DVec3);

impl LocalGravity {
    /// Magnitude in m/s² (always non-negative).
    pub fn magnitude(&self) -> f64 {
        self.0.length()
    }

    /// Unit vector in the direction of gravity (downward).
    /// Returns [`DVec3::NEG_Y`] if the gravity vector is zero.
    pub fn direction(&self) -> DVec3 {
        if self.0.length_squared() > 0.0 {
            self.0.normalize()
        } else {
            DVec3::NEG_Y
        }
    }
}

/// Computes [`LocalGravity`] for every entity that has a [`Transform`].
///
/// Sources the gravity vector from:
/// - [`Gravity::Flat`] — same vector for all entities (sandbox / flat-world)
/// - [`Gravity::Surface`] — per-entity, requires [`GravityBody`] +
///   [`GravityProvider`] on the linked body
pub fn compute_local_gravity(
    mut commands: Commands,
    gravity: Res<Gravity>,
    q_bodies: Query<&GravityProvider>,
    q_entities: Query<(
        Entity,
        Ref<Transform>,
        Option<&GravityBody>,
        Option<&LocalGravity>,
    )>,
) {
    // Recompute an entity's gravity only when something it depends on changed:
    // the global `Gravity` definition (Flat vector / Flat↔Surface switch) or
    // this entity's own Transform (Surface gravity is position-dependent; Flat
    // is not). Entities that don't yet have a `LocalGravity` always run once.
    // This stops both the per-frame provider lookups and the change-detection
    // storm caused by blindly re-inserting an identical value every frame.
    let gravity_changed = gravity.is_changed();
    for (entity, tf, gravity_body, existing) in &q_entities {
        if existing.is_some() && !gravity_changed && !tf.is_changed() {
            continue;
        }
        let g = match gravity.as_ref() {
            Gravity::Flat { g, direction } => *direction * *g,
            Gravity::Surface => {
                let Some(body_link) = gravity_body else {
                    continue;
                };
                let Ok(provider) = q_bodies.get(body_link.body_entity) else {
                    continue;
                };
                provider.model.acceleration(tf.translation.as_dvec3())
            }
        };
        // Don't re-insert (and re-trigger change detection) when the value is
        // unchanged — e.g. a `gravity_changed` pass that recomputes the same g.
        if let Some(LocalGravity(prev)) = existing {
            if *prev == g {
                continue;
            }
        }
        commands.entity(entity).try_insert(LocalGravity(g));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Consumer: apply gravity force to Avian RigidBodies
// ─────────────────────────────────────────────────────────────────────────────

/// Applies the cached [`LocalGravity`] vector as a force on every entity that
/// has a [`RigidBody`] and a [`Mass`].
///
/// Replaces the recomputing-each-tick `gravity_system` that previously lived
/// in `lunco-celestial`. Reading `LocalGravity` instead of recomputing means
/// every consumer (this system, cosim injection, future systems) sees the same
/// authoritative value with no duplicated work.
pub fn apply_gravity_to_rigid_bodies(
    q: Query<(Entity, &LocalGravity, &Mass), With<RigidBody>>,
    // Force must land only on a body the solver will integrate. A disabled body
    // (frozen while its program compiles, say) never has its accumulators
    // cleared, so force applied to it is stored, not spent, and discharges in
    // full on the step that eventually runs — see `lunco_physics::Integrable`.
    mut forces: Query<Forces, lunco_physics::Integrable>,
) {
    for (entity, gravity, mass) in &q {
        let force = gravity.0 * mass.0 as f64;
        if let Ok(mut f) = forces.get_mut(entity) {
            f.apply_force(force);
        }
    }
}

/// Feeds each body's authoritative [`LocalGravity`] into its
/// [`lunco_cosim::sensors::ImuSensor`] so the accelerometer's specific-force
/// output (`a − g`) uses the real local gravity. avian's own `Gravity` resource
/// is zero here — gravity is applied as an explicit force — so the IMU cannot
/// read it directly; this is the same "reuse the one authoritative value"
/// principle as [`inject_local_gravity_into_cosim`]. Change-guarded so it doesn't
/// dirty the component every tick.
pub fn feed_gravity_into_imu_sensors(
    mut q: Query<(&LocalGravity, &mut lunco_cosim::sensors::ImuSensor)>,
) {
    for (gravity, mut imu) in &mut q {
        if imu.gravity != gravity.0 {
            imu.gravity = gravity.0;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Consumer: feed local gravity into the co-simulation graph
// ─────────────────────────────────────────────────────────────────────────────

/// Publishes each entity's [`LocalGravity`] magnitude as a [`SimComponent`]
/// **output** named [`lunco_cosim::GRAVITY_SOURCE_CONNECTOR`], so co-sim models
/// that take a gravity input (`g`, `gravity`, …) receive the *real* local value
/// through an ordinary output→input wire.
///
/// This is the domain half of keeping `lunco-cosim` pure: the master
/// propagation algorithm has no gravity special-case and no hardcoded constant
/// (it used to inject Earth's `9.81` for a magic `__gravity__` source, which was
/// wrong on the Moon). Gravity now flows like any other signal, correct on any
/// body, because the value comes from the position-dependent `LocalGravity`.
///
/// Runs in [`EnvironmentSet::Apply`] (after `LocalGravity` is computed) and
/// before cosim's propagation, so the freshly-written output is read the same
/// tick. Writes every tick because a model's own output sync may rewrite its
/// outputs map.
pub fn inject_local_gravity_into_cosim(
    mut q: Query<(&LocalGravity, &mut lunco_cosim::SimComponent)>,
) {
    for (gravity, mut comp) in &mut q {
        comp.outputs.insert(
            lunco_cosim::GRAVITY_SOURCE_CONNECTOR.to_string(),
            gravity.magnitude(),
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SetEnvironmentLight — runtime sun direction + ambient brightness
// ─────────────────────────────────────────────────────────────────────────────

/// Sets scene environment lighting at runtime: the sun's direction and the
/// global ambient level.
///
/// All three fields are optional — only the ones provided change, the rest
/// keep their current value. So a curl that just lowers the sun looks like:
///
/// ```jsonc
/// {"type":"SetEnvironmentLight","sun_pitch":-0.15}
/// ```
///
/// - **`sun_yaw` / `sun_pitch`** — direction of the single `DirectionalLight`
///   in radians, using the same `EulerRot::YXZ` (yaw-then-pitch) convention as
///   the sandbox settings panel. A small negative `sun_pitch` (e.g. `-0.15`,
///   ~8.5° above the horizon) gives long, raking lunar shadows; `-0.8` is a
///   high ~46° sun with short shadows.
/// - **`ambient_brightness`** — the [`GlobalAmbientLight`] level (the *real*
///   scene-wide fill; the per-camera `AmbientLight` component is only an
///   override). Lower it (~30–60) for deep, high-contrast lunar shadow cores;
///   the airless Moon has near-black shadows.
#[Command(default)]
pub struct SetEnvironmentLight {
    /// Sun azimuth in radians (`EulerRot::YXZ` yaw). `None` keeps current.
    pub sun_yaw: Option<f32>,
    /// Sun elevation in radians (`EulerRot::YXZ` pitch); negative tilts the
    /// light down. `None` keeps current.
    pub sun_pitch: Option<f32>,
    /// Sun illuminance in lux. `None` keeps current.
    pub illuminance: Option<f32>,
    /// Sun color as linear RGB. `None` keeps current.
    pub sun_color: Option<[f32; 3]>,
    /// Whether the sun casts shadows. `None` keeps current.
    pub shadow_maps_enabled: Option<bool>,
    /// Far bound of the first (sharpest) shadow cascade, metres.
    /// `None` keeps current.
    pub shadow_first_cascade_bound: Option<f32>,
    /// Total shadow-casting range, metres. Smaller ⇒ denser shadow-map
    /// texels ⇒ crisper shadows. `None` keeps current.
    pub shadow_max_distance: Option<f32>,
    /// Shadow depth bias — raise to suppress self-shadow acne stripes
    /// (cost: shadows detach slightly). `None` keeps current.
    pub shadow_depth_bias: Option<f32>,
    /// Shadow normal bias, in shadow-texel units — the main acne killer on
    /// terrain under grazing light (Bevy default 1.8). `None` keeps current.
    pub shadow_normal_bias: Option<f32>,
    /// Global ambient brightness (cd/m²-scaled). `None` keeps current.
    pub ambient_brightness: Option<f32>,
    /// Camera physical exposure, EV100 (≈15 = sunlight, 9.7 = Blender default).
    /// Moves with `illuminance`: brighter sun ⇒ higher EV. `None` keeps current.
    pub exposure_ev100: Option<f32>,
    /// [`Earthshine`] fill illuminance, lux (~10–15 typical). `None` keeps current.
    pub earthshine_illuminance: Option<f32>,
    /// [`Earthshine`] fill color, linear RGB (cool blue ≈ 0.6,0.75,1.0).
    /// `None` keeps current.
    pub earthshine_color: Option<[f32; 3]>,
    /// Bloom intensity on the scene cameras (airless ⇒ low, ~0.15). `None`
    /// keeps current.
    ///
    /// **Applied render-side** (`lunco_render_bevy::env_light`) — bloom is
    /// `bevy_post_process`, and this crate must not name it. That observer
    /// writes `lunco_render::SceneCamera::bloom`, whose binder REFUSES bloom on
    /// a non-HDR camera (review `R4`) — and `hdr` is deliberately still off
    /// everywhere, so this knob renders nothing today, exactly as before.
    pub bloom_intensity: Option<f32>,
}

/// Marks the **earthshine** fill light — a second, *shadowless*, cool-blue
/// `DirectionalLight` standing in for Earth's reflected light. It is summed by
/// Bevy's normal light loop, so
/// it lifts sun-shadowed regolith into faint blue relief without washing the
/// shadow cores grey the way a flat `GlobalAmbientLight` would.
///
/// Its own marker (not `FallbackSceneLight`) keeps it **persistent** — the real
/// Moon always has earthshine, so it survives the USD light-import that
/// despawns fallback suns. The `SetEnvironmentLight` sun loop excludes it via
/// `Without<Earthshine>` so a sun tweak never overwrites the fill.
///
/// **Render-free**: a `DirectionalLight` is `bevy_light`, which does not depend
/// on `bevy_render`. The marker (and the light it tags) exist headless too.
#[derive(Component, Debug, Clone, Copy, Reflect, Default)]
#[reflect(Component)]
pub struct Earthshine;

/// Applies a [`SetEnvironmentLight`] command to the live `DirectionalLight`,
/// its `CascadeShadowConfig`, `GlobalAmbientLight` and camera `Exposure` — all
/// render-FREE types, so the command works headless. Resources/queries are
/// tolerant of absence so it is a no-op in contexts that have no lights.
///
/// The one render-bound field, `bloom_intensity`, is applied by a SECOND
/// observer on this same command in `lunco-render-bevy` (`env_light.rs`) — a
/// command may have as many observers as it has effects, and that is what keeps
/// `bevy_post_process` out of this crate.
///
/// This observer is the SINGLE mutation path for environment lighting —
/// the HTTP/MCP API, the Inspector's Environment section, and any future
/// script hooks all dispatch this same command. (The USD loader is the
/// *creation* path: it spawns the light entity from `DistantLight` prims;
/// every later change flows through here.)
#[on_command(SetEnvironmentLight)]
fn on_set_environment_light(
    trigger: On<SetEnvironmentLight>,
    // The sun(s): every directional light EXCEPT the earthshine fill, so an
    // illuminance/color/direction tweak never clobbers the fill light.
    mut q_sun: Query<
        (
            &mut Transform,
            &mut DirectionalLight,
            Option<&mut CascadeShadowConfig>,
        ),
        (With<DirectionalLight>, Without<Earthshine>),
    >,
    mut q_earthshine: Query<&mut DirectionalLight, With<Earthshine>>,
    mut q_exposure: Query<&mut Exposure>,
    ambient: Option<ResMut<GlobalAmbientLight>>,
) {
    for (mut tf, mut light, cascades) in &mut q_sun {
        if cmd.sun_yaw.is_some() || cmd.sun_pitch.is_some() {
            // Preserve the unspecified axis by reading it back off the current
            // rotation (same YXZ order the Inspector writes with).
            let (cur_yaw, cur_pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
            let yaw = cmd.sun_yaw.unwrap_or(cur_yaw);
            let pitch = cmd.sun_pitch.unwrap_or(cur_pitch);
            tf.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
        }

        if let Some(lux) = cmd.illuminance {
            light.illuminance = lux;
        }
        if let Some([r, g, b]) = cmd.sun_color {
            light.color = Color::linear_rgb(r, g, b);
        }
        if let Some(s) = cmd.shadow_maps_enabled {
            light.shadow_maps_enabled = s;
        }
        if let Some(b) = cmd.shadow_depth_bias {
            light.shadow_depth_bias = b;
        }
        if let Some(b) = cmd.shadow_normal_bias {
            light.shadow_normal_bias = b;
        }

        if cmd.shadow_first_cascade_bound.is_some() || cmd.shadow_max_distance.is_some() {
            if let Some(mut cfg) = cascades {
                // Rebuild from the live config, overriding only the two
                // range knobs (cascade count / overlap / near are kept).
                // The empty-bounds fallbacks are local consts mirroring the
                // canonical lunar-sun cascade defaults (see their declaration).
                let cur_first = cfg
                    .bounds
                    .first()
                    .copied()
                    .unwrap_or(FALLBACK_FIRST_CASCADE_FAR_BOUND);
                let cur_max = cfg
                    .bounds
                    .last()
                    .copied()
                    .unwrap_or(FALLBACK_MAX_SHADOW_DISTANCE);
                // The first bound has to stay a metre inside the max, so the max is
                // floored at 2 m first: clamping the other way round (bound, then
                // max) lets a sub-2 m max pull the first bound to zero or negative,
                // which produces a degenerate cascade and a black shadow pass.
                let max = cmd.shadow_max_distance.unwrap_or(cur_max).max(2.0);
                let first = cmd
                    .shadow_first_cascade_bound
                    .unwrap_or(cur_first)
                    .clamp(1.0, max - 1.0);
                *cfg = CascadeShadowConfigBuilder {
                    num_cascades: cfg.bounds.len().max(1),
                    minimum_distance: cfg.minimum_distance,
                    first_cascade_far_bound: first,
                    maximum_distance: max,
                    overlap_proportion: cfg.overlap_proportion,
                }
                .build();
            }
        }
    }

    if let (Some(b), Some(mut ambient)) = (cmd.ambient_brightness, ambient) {
        ambient.brightness = b;
    }

    // Camera exposure (all cameras that carry an Exposure component).
    if let Some(ev) = cmd.exposure_ev100 {
        for mut exposure in &mut q_exposure {
            exposure.ev100 = ev;
        }
    }

    // Earthshine fill light.
    for mut fill in &mut q_earthshine {
        if let Some(lux) = cmd.earthshine_illuminance {
            fill.illuminance = lux;
        }
        if let Some([r, g, b]) = cmd.earthshine_color {
            fill.color = Color::linear_rgb(r, g, b);
        }
    }

    // `bloom_intensity` is handled render-side (`lunco_render_bevy::env_light`),
    // which writes `SceneCamera::bloom`. Bloom is `bevy_post_process` → wgpu.
}

register_commands!(on_set_environment_light);

// ─────────────────────────────────────────────────────────────────────────────
// Plugin
// ─────────────────────────────────────────────────────────────────────────────

/// Registers environment components, computation, and consumption systems.
///
/// Add after [`lunco_celestial::GravityPlugin`]. Ordering in `FixedUpdate`:
/// 1. [`EnvironmentSet::Compute`] — writes `LocalGravity` (and future `Local*`)
/// 2. [`EnvironmentSet::Apply`] — applies gravity forces to Avian RigidBodies
pub struct EnvironmentPlugin;

/// Spawns the persistent [`Earthshine`] fill light once at startup (skipped if
/// one already exists). Direction is roughly opposite the default sun azimuth,
/// just above the horizon — fixed for v1 (ephemeris-correct Earth direction is
/// a later refinement); live-tunable level/color via `SetEnvironmentLight`.
///
/// Native only: the web build renders on WebGL2, which supports a single
/// `DirectionalLight`. A second light there culls the sun, so earthshine is not
/// spawned on wasm (see the gated registration in `EnvironmentPlugin`).
#[cfg(not(target_arch = "wasm32"))]
fn spawn_earthshine(mut commands: Commands, existing: Query<(), With<Earthshine>>) {
    if !existing.is_empty() {
        return;
    }
    // Illuminance + colour from the canonical params (see `lighting` module);
    // direction is the render-side placeholder (roughly opposite the sun).
    let es = EarthshineParams::default();
    commands.spawn((
        Earthshine,
        DirectionalLight {
            illuminance: es.illuminance_lux,
            color: Color::linear_rgb(es.color[0], es.color[1], es.color[2]),
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 3.6, -0.25, 0.0)),
        Name::new("Earthshine"),
    ));
}

impl Plugin for EnvironmentPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<LocalGravity>();

        // The one active-scene sun (lux + matched camera EV). Pure data
        // (no render types), so it's available even on a headless server —
        // `lunco-usd-sim` reads it as an `Option<Res<LunarSun>>`. Canonical
        // lunar default unless a scene `insert_resource`d its own studio
        // values first (`init_resource` is a no-op when already present).
        app.init_resource::<LunarSun>();

        app.configure_sets(
            FixedUpdate,
            (EnvironmentSet::Compute, EnvironmentSet::Apply).chain(),
        );

        // Sim core — render-free. Gravity computation, force application, and
        // the gravity→cosim bridge.
        //
        // `apply_gravity_to_rigid_bodies` is gated on `physics_is_live`; nothing
        // else here is. Gravity is the only system in this set that writes into
        // avian's FORCE ACCUMULATOR, and that accumulator is cleared by the physics
        // step — so a tick where the step is skipped leaves the force in place to be
        // added to again next tick. Ungated, it integrated to ~4 MN across episode
        // 2's 28 s of frozen shots and fired the rover through the ground at
        // 224.20 m/s the instant the hold released. The other systems in this set
        // publish a VALUE (cosim input, IMU field) rather than accumulate one, so
        // they must keep running while physics is held — a frozen beat still wants a
        // correct gravity reading.
        app.add_systems(
            FixedUpdate,
            (
                compute_local_gravity.in_set(EnvironmentSet::Compute),
                // Pinned BEFORE `ControlDacSet` (and therefore before the wheel
                // actuators, which all order `.after` it). Not a data edge —
                // gravity reads no port — but a SUMMATION-ORDER edge: this and
                // the suspension/drive systems all `apply_force` into the same
                // f64 accumulator, and f64 addition is not associative. The
                // `RollbackReplay` mirror below pins the same edge, so the
                // replayed tick accumulates its forces in the same order the
                // live tick did. Left implicit, the two schedules would each
                // pick their own topological order and the replay would differ
                // from the host in the last bit of every body's force.
                apply_gravity_to_rigid_bodies
                    .in_set(EnvironmentSet::Apply)
                    .before(lunco_core::ControlDacSet)
                    .run_if(lunco_physics::physics_is_live),
                // Publish gravity into the cosim graph after it's computed and
                // before cosim copies outputs→inputs, so models read the real
                // local value the same tick.
                inject_local_gravity_into_cosim
                    .in_set(EnvironmentSet::Apply)
                    .before(lunco_cosim::systems::propagate::CosimSet::Propagate),
                // Feed gravity into IMU sensors before they compute specific
                // force (both run before propagation, same tick).
                feed_gravity_into_imu_sensors
                    .in_set(EnvironmentSet::Apply)
                    .before(lunco_cosim::systems::propagate::CosimSet::Propagate),
            ),
        );

        // ── Rollback replay ──────────────────────────────────────────────────
        // WEIGHT IS PART OF THE REPLAYED TICK. The shipped app sets avian's own
        // `Gravity::ZERO` (`lunco-sandbox`) — gravity reaches a rigid body ONLY
        // through this force write. `replay_one_tick` runs `RollbackReplay` and
        // then steps `PhysicsSchedule`, and the physics step CLEARS the force
        // accumulator; so without this system in the schedule every replayed
        // tick solved a WEIGHTLESS rover. Normal force, and with it wheel
        // traction, then differ from the host's on the one body rollback exists
        // to keep in sync — the replay diverges hardest exactly when the rover
        // is doing something (accelerating, cresting, braking).
        //
        // No `physics_is_live` gate here, unlike the `FixedUpdate` copy. That
        // gate exists because a held physics step leaves the accumulator
        // unconsumed, so an ungated live tick would integrate weight across the
        // hold and discharge it in one step. Replay has no such hazard: it
        // ALWAYS pairs this write with its own `PhysicsSchedule` step. Gating it
        // would instead drop gravity from a replay that still solves — turning
        // the hold into the divergence.
        //
        // `compute_local_gravity` is deliberately NOT mirrored: it is a
        // change-driven `Commands` writer, and re-deriving `LocalGravity` mid-
        // replay would move archetypes inside the schedule. The cached value is
        // the one the host used for these ticks (constant under `Gravity::Flat`,
        // and varying by well under an ULP across one tick's motion under
        // `Gravity::Surface`).
        app.add_systems(
            lunco_core::RollbackReplay,
            apply_gravity_to_rigid_bodies.before(lunco_core::ControlDacSet),
        );

        // Lighting half — RENDER-FREE. `DirectionalLight` is `bevy_light` and
        // `RenderLayers` is `bevy_camera`; neither depends on `bevy_render`, so
        // the earthshine fill and the sun→cosim direction feed run headless too
        // (a sun-tracking Modelica model on the `--no-ui` server needs them).
        app.register_type::<LocalSolar>();
        app.register_type::<LocalEarth>();
        app.register_type::<Earthshine>();
        // Declared here, WRITTEN by lunco-celestial (which depends on this crate,
        // so the dependency cannot run the other way). Init'd unconditionally and
        // left at ZERO — the "not known" state — so a scene with no celestial
        // hierarchy reads as no-data rather than as a missing resource.
        app.init_resource::<EarthDirectionWorld>();

        // The cool-blue earthshine fill (persistent, shadowless). Skipped
        // on web: WebGL2 supports only ONE `DirectionalLight`, and a second
        // one culls the sun — keep the sun, drop the fill.
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Startup, spawn_earthshine);

        // Solar source: mirror gravity. Compute the per-entity sun
        // direction, then publish it as cosim outputs before propagation
        // so a sun-tracking model reads it the same tick.
        app.add_systems(
            FixedUpdate,
            (
                compute_local_solar.in_set(EnvironmentSet::Compute),
                inject_local_solar_into_cosim
                    .in_set(EnvironmentSet::Apply)
                    .before(lunco_cosim::systems::propagate::CosimSet::Propagate),
                // Earth pointing rides the same three-phase ordering: an antenna
                // model must read the angles the same tick they were computed.
                compute_local_earth.in_set(EnvironmentSet::Compute),
                inject_local_earth_into_cosim
                    .in_set(EnvironmentSet::Apply)
                    .before(lunco_cosim::systems::propagate::CosimSet::Propagate),
            ),
        );

        // Horizon-map terrain self-shadowing — the BAKE half (heightfield +
        // sun-visibility cache). Render-free: it produces `Image` assets and CPU
        // fields, and never names a material. The wiring that feeds them into the
        // terrain shader is `lunco_render_bevy::LuncoRenderPlugin`'s job, and it
        // is simply absent headless. Inert until a terrain carries the
        // `HorizonShadowTerrain` marker (USD-stamped).
        app.add_plugins(HorizonShadowPlugin);

        // Register environment commands (SetEnvironmentLight). The macro-built
        // `register_all_commands` does `register_type` + `add_observer` so the
        // HTTP/MCP API can dispatch it by reflected type name. The command is
        // render-free now — its `bloom_intensity` field is applied by a second
        // observer over in `lunco-render-bevy`.
        register_all_commands(app);
    }
}
