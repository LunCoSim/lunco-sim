//! # lunco-environment
//!
//! Per-entity environmental state computed from celestial body providers.
//!
//! See `README.md` for the full architecture, rationale, and how to add new
//! environment domains (atmosphere, radiation, magnetic field, etc.).
//!
//! Currently implements **gravity only**. Other domains follow the same
//! pattern — see the README for templates.

use avian3d::prelude::{ConstantLinearAcceleration, RigidBody};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
// All render-FREE: `CascadeShadowConfig` / `GlobalAmbientLight` are `bevy_light`,
// `Exposure` is `bevy_camera`. Neither depends on `bevy_render`. The one knob in
// `SetEnvironmentLight` that IS render-bound — `bloom_intensity` — is applied by a
// second observer in `lunco-render-bevy` (`env_light.rs`), so this crate names no
// post-processing type. See docs/architecture/render-decoupling.md.
use bevy::camera::visibility::RenderLayers;
use bevy::camera::Exposure;
use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder, GlobalAmbientLight};
use lunco_core::{on_command, register_commands, Command};

/// USD prim type for the scene-level **environment settings** prim (a singleton
/// under the default prim, e.g. `/World/Environment`). It carries the render
/// knobs that have no natural light-prim home — `lunco:env:exposureEv100` and
/// `lunco:env:bloomIntensity`.
///
/// **Ambient and earthshine are not among them.** Earthshine is an authored
/// `DistantLight` nested under the body it reflects from, so its tint is
/// `inputs:color` on that prim — standard UsdLux, read back by the standard
/// light loader. Its brightness is not persisted anywhere: it is derived from
/// Earth's phase by [`drive_earthshine_from_phase`] every frame.
/// Uniform environment illumination is standard
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
    Gravity, GravityBody, GravityModel, GravityProvider, PhysicsSceneGravity,
    EARTH_SURFACE_GRAVITY, MOON_SURFACE_GRAVITY,
};

/// Physical lighting parameters of the lunar sky (`LunarSun`, `FULL_EARTH_EARTHSHINE_LUX`)
/// — environmental state, the lighting analog of gravity. See the module docs.
pub mod lighting;
pub use lighting::{drive_earthshine_from_phase, LunarSun, FULL_EARTH_EARTHSHINE_LUX};

/// Solar direction as a co-simulation source (`LocalSolar` + the sun→cosim
/// bridge). `LocalSolar` always reads semantic [`SunState`]. The scene-light
/// projection may additionally consume the typed render-only
/// [`SunRenderPresentation`] without feeding that direction into physics or
/// co-simulation.
pub mod solar;
pub use solar::{
    compute_local_solar, finalize_sun_render_state, inject_local_solar_into_cosim,
    project_sun_render_to_light, LocalSolar, SunRenderPresentation, SunRenderState, SunState,
};

/// Explicit USD-authored source of mount-local environmental signals.
///
/// `lunco-usd-sim` projects `LunCoEnvironmentProbeAPI` prims to this marker plus
/// a source-only `SimComponent`. Environment systems publish onto probes; models
/// consume them through ordinary USD connections. Keeping provider and consumer
/// on distinct entities avoids environment self-wires and false feedback cycles.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct EnvironmentProbe;

/// Runtime projection of a composed USD fact: an environment probe has at least
/// one connected Earth-vector output that a downstream model consumes.
///
/// This is deliberately separate from [`EnvironmentProbe`]. The probe publishes
/// gravity and solar data for many models, but Earth direction is an opt-in
/// demand. Keeping the demand as a projected component prevents the provider
/// from treating every atmosphere/gravity probe as an Earth tracker.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct EarthDirectionRequired;

/// Earth's direction as a co-simulation source (`LocalEarth` + the earth→cosim
/// bridge) — what a high-gain antenna points at, the twin of [`solar`] for the
/// other body in a lunar sky.
///
/// Unlike the sun there is no scene light to read, so the direction arrives in
/// the [`earth::EarthDirectionWorld`] resource, written by `lunco-celestial`
/// from the ephemeris. See the module docs for why the dependency runs that way.
pub mod earth;
mod mount_frame;
pub use earth::{
    compute_local_earth, inject_local_earth_into_cosim, EarthDirectionWorld, LocalEarth,
};

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
/// 2. [`Apply`](EnvironmentSet::Apply) — consumers like Avian gravity projection
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnvironmentSet {
    /// Computes per-entity environment components from body providers.
    Compute,
    /// Applies environment effects (e.g., gravity acceleration on RigidBodies).
    Apply,
}

// ─────────────────────────────────────────────────────────────────────────────
// LocalGravity — the gravity vector at an entity's position
// ─────────────────────────────────────────────────────────────────────────────

/// Gravity vector at this entity's position, in the entity's Avian physics
/// frame (m/s²).
///
/// Computed each [`FixedUpdate`] from the [`Gravity`] resource and (for
/// surface gravity) the [`GravityProvider`] on the entity's gravitational
/// parent body (linked via [`GravityBody`]).
///
/// - **Magnitude:** `length()` gives `g` in m/s²
/// - **Direction:** `normalize()` gives the gravity unit vector
///
/// Read this instead of querying the [`Gravity`] resource directly — it's
/// position-dependent and cached. Multiple consumers (Avian acceleration,
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

fn clear_unresolved_local_gravity(
    commands: &mut Commands,
    entity: Entity,
    existing: Option<&LocalGravity>,
) {
    if existing.is_some() {
        commands.entity(entity).remove::<LocalGravity>();
    }
}

fn update_local_gravity_for_entity(
    commands: &mut Commands,
    gravity: &Gravity,
    frame_rotation: Option<DQuat>,
    entity: Entity,
    gravity_body: Option<Ref<GravityBody>>,
    existing: Option<&LocalGravity>,
    q_bodies: &Query<&GravityProvider>,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
) {
    let g = match gravity {
        // A flat field is authored by UsdPhysicsScene in the stage's
        // physics frame. The scene mount makes that frame the active
        // Avian frame, so its direction is already expressed in the
        // coordinate system consumed by the body solver. Converting it
        // through the active-frame rotation would apply the site pose a
        // second time and create a spurious horizontal acceleration.
        Gravity::Flat { g, direction } => *direction * *g,
        Gravity::Surface => {
            let Some(body_link) = gravity_body.as_deref() else {
                clear_unresolved_local_gravity(commands, entity, existing);
                return;
            };
            let Ok(provider) = q_bodies.get(body_link.body_entity) else {
                clear_unresolved_local_gravity(commands, entity, existing);
                return;
            };
            let Some((entity_world, _)) =
                lunco_spatial::coords::world_pose(entity, q_parents, q_grids, q_spatial).ok()
            else {
                clear_unresolved_local_gravity(commands, entity, existing);
                return;
            };
            let Some((body_world, body_rotation)) = lunco_spatial::coords::world_pose(
                body_link.body_entity,
                q_parents,
                q_grids,
                q_spatial,
            )
            .ok() else {
                clear_unresolved_local_gravity(commands, entity, existing);
                return;
            };
            let relative_body = body_rotation.0.inverse() * (entity_world - body_world);
            let acceleration = provider.model.acceleration(relative_body);
            let g_world = body_rotation.0 * acceleration;
            // Surface gravity is evaluated in the celestial body's
            // body-fixed frame and therefore needs the one explicit
            // conversion into the active Avian frame.
            frame_rotation.map_or(g_world, |rotation| rotation.inverse() * g_world)
        }
    };
    // Don't re-insert (and re-trigger) when the value is unchanged — e.g. a
    // global invalidation that recomputes the same field.
    if existing.is_some_and(|LocalGravity(previous)| *previous == g) {
        return;
    }
    commands.entity(entity).try_insert(LocalGravity(g));
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
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    q_bodies: Query<&GravityProvider>,
    q_changed_providers: Query<(), Changed<GravityProvider>>,
    mut removed_providers: RemovedComponents<GravityProvider>,
    mut removed_body_links: RemovedComponents<GravityBody>,
    mut q_entities: ParamSet<(
        Query<
            (Entity, Option<Ref<GravityBody>>, Option<&LocalGravity>),
            (
                With<Transform>,
                Or<(
                    Changed<Transform>,
                    Changed<GravityBody>,
                    Without<LocalGravity>,
                )>,
            ),
        >,
        Query<(Entity, Option<Ref<GravityBody>>, Option<&LocalGravity>), With<Transform>>,
    )>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
) {
    // The field is entity-local, so a quiet frame must visit only entities
    // whose inputs changed. A provider edit/removal or a global/frame change
    // invalidates every cached field and deliberately selects the full query.
    // This is structural change detection, not a timer: transforms, body links,
    // provider structure, and the authored gravity/frame resources remain the
    // invalidation owners.
    let provider_changed =
        !q_changed_providers.is_empty() || removed_providers.read().next().is_some();
    let body_link_removed = removed_body_links.read().next().is_some();
    let global_invalidation = gravity.is_changed()
        || active_frame.as_ref().is_some_and(Res::is_changed)
        || provider_changed
        || body_link_removed;
    let has_work = global_invalidation || !q_entities.p0().is_empty();
    if !has_work {
        return;
    }
    let frame_rotation = matches!(gravity.as_ref(), Gravity::Surface)
        .then(|| {
            active_frame.as_deref().and_then(|frame| {
                lunco_spatial::coords::world_pose(frame.0, &q_parents, &q_grids, &q_spatial)
                    .ok()
                    .map(|(_, rotation)| rotation.0)
            })
        })
        .flatten();
    if global_invalidation {
        for (entity, gravity_body, existing) in q_entities.p1().iter() {
            update_local_gravity_for_entity(
                &mut commands,
                gravity.as_ref(),
                frame_rotation,
                entity,
                gravity_body,
                existing,
                &q_bodies,
                &q_parents,
                &q_grids,
                &q_spatial,
            );
        }
    } else {
        for (entity, gravity_body, existing) in q_entities.p0().iter() {
            update_local_gravity_for_entity(
                &mut commands,
                gravity.as_ref(),
                frame_rotation,
                entity,
                gravity_body,
                existing,
                &q_bodies,
                &q_parents,
                &q_grids,
                &q_spatial,
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Consumer: project gravity into Avian's persistent acceleration contract
// ─────────────────────────────────────────────────────────────────────────────

/// Projects the cached [`LocalGravity`] vector onto Avian's standard
/// [`ConstantLinearAcceleration`] component. The component is consumed by
/// Avian's own integrator, so gravity is applied in live physics and rollback
/// alike without force-accumulator bookkeeping or a per-tick wake-up.
///
/// Replaces the recomputing-each-tick `gravity_system` that previously lived
/// in `lunco-celestial`. Reading `LocalGravity` instead of recomputing means
/// every consumer (this system, cosim injection, future systems) sees the same
/// authoritative value with no duplicated work. Updating the standard
/// component only when the cached field changes lets Avian's native
/// sleeping/waking rules remain authoritative.
pub fn sync_local_gravity_to_avian(
    mut commands: Commands,
    changed: Query<
        (Entity, &LocalGravity, Option<&ConstantLinearAcceleration>),
        (
            With<RigidBody>,
            Or<(Changed<LocalGravity>, Changed<RigidBody>)>,
        ),
    >,
    mut removed_gravity: RemovedComponents<LocalGravity>,
    mut removed_bodies: RemovedComponents<RigidBody>,
) {
    for (entity, gravity, existing) in &changed {
        let acceleration = ConstantLinearAcceleration(gravity.0);
        if existing.is_none_or(|current| current.0 != acceleration.0) {
            commands.entity(entity).try_insert(acceleration);
        }
    }
    for entity in removed_gravity.read().chain(removed_bodies.read()) {
        commands
            .entity(entity)
            .try_remove::<ConstantLinearAcceleration>();
    }
}

// Modelica sensor conversions consume the same `LocalGravity` vector through
// the environment-probe output ports. Avian's own global `Gravity` resource is
// zero here — the per-body standard acceleration component is the physics
// realization — so the environment bridge publishes both magnitude and vector
// components through ordinary wires.
// ─────────────────────────────────────────────────────────────────────────────
// Consumer: feed local gravity into the co-simulation graph
// ─────────────────────────────────────────────────────────────────────────────

/// Publishes each entity's [`LocalGravity`] magnitude as a [`SimComponent`]
/// **output** named [`lunco_cosim_core::GRAVITY_SOURCE_CONNECTOR`], so co-sim models
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
/// outputs map. In surface-gravity scenes where no provider has resolved yet,
/// removes the gravity output rather than exposing a stale value.
pub fn inject_local_gravity_into_cosim(
    mut q: Query<
        (Option<&LocalGravity>, &mut lunco_cosim_core::SimComponent),
        With<EnvironmentProbe>,
    >,
) {
    for (gravity, mut comp) in &mut q {
        if let Some(gravity) = gravity {
            comp.outputs.insert(
                lunco_cosim_core::GRAVITY_SOURCE_CONNECTOR.to_string(),
                gravity.magnitude(),
            );
            comp.outputs.insert(
                lunco_cosim_core::GRAVITY_X_SOURCE_CONNECTOR.to_string(),
                gravity.0.x,
            );
            comp.outputs.insert(
                lunco_cosim_core::GRAVITY_Y_SOURCE_CONNECTOR.to_string(),
                gravity.0.y,
            );
            comp.outputs.insert(
                lunco_cosim_core::GRAVITY_Z_SOURCE_CONNECTOR.to_string(),
                gravity.0.z,
            );
        } else {
            comp.outputs
                .remove(lunco_cosim_core::GRAVITY_SOURCE_CONNECTOR);
            comp.outputs
                .remove(lunco_cosim_core::GRAVITY_X_SOURCE_CONNECTOR);
            comp.outputs
                .remove(lunco_cosim_core::GRAVITY_Y_SOURCE_CONNECTOR);
            comp.outputs
                .remove(lunco_cosim_core::GRAVITY_Z_SOURCE_CONNECTOR);
        }
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
/// {"type":"ExecuteCommand","command":"SetEnvironmentLight","params":{"sun_pitch":-0.15}}
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
    // Shadow depth/normal bias are deliberately absent: they are engine policy
    // in `lunco_render::LunarSunShadow`. A knob here would be tunable but not
    // persistable, since the USD loader reads neither.
    /// Global ambient brightness (cd/m²-scaled). `None` keeps current.
    pub ambient_brightness: Option<f32>,
    /// Camera physical exposure, EV100 (≈15 = sunlight, 9.7 = Blender default).
    /// Moves with `illuminance`: brighter sun ⇒ higher EV. `None` keeps current.
    pub exposure_ev100: Option<f32>,
    // Earthshine ILLUMINANCE is deliberately absent: it is derived from Earth's
    // phase by `drive_earthshine_from_phase`, which is its one writer. A knob
    // beside a driver is two writers on one field — the shape of the
    // `ambientBrightness` bug — and it would be overwritten within the frame.
    /// [`Earthshine`] fill color, linear RGB (cool blue ≈ 0.6,0.75,1.0).
    /// `None` keeps current.
    pub earthshine_color: Option<[f32; 3]>,
    /// Bloom intensity on the scene cameras. `None` keeps current; zero disables
    /// bloom and a non-zero value enables the HDR target required by the effect.
    ///
    /// **Applied render-side** (`lunco_render_bevy::env_light`) — bloom is
    /// `bevy_post_process`, and this crate must not name it. That observer
    /// writes the render intent, whose binder owns the concrete post-process
    /// component.
    pub bloom_intensity: Option<f32>,
}

/// Marks the optional earthshine `DirectionalLight`.
///
/// The entity starts at zero illuminance: a scene must provide a physically
/// meaningful Earth direction and phase before it contributes. This avoids an
/// implicit, unshadowed fill source changing the appearance of Sun shadows.
///
/// It carries its own marker because it is **persistent** scene-independent
/// state — the real Moon always has earthshine. The `SetEnvironmentLight` sun
/// loop excludes it via `Without<Earthshine>` so a sun tweak never overwrites
/// the fill, and the sun-steering pick must likewise never mistake this ~12 lx
/// fill for the ~128 klx key light.
///
/// **Render-free**: a `DirectionalLight` is `bevy_light`, which does not depend
/// on `bevy_render`. The marker (and the light it tags) exist headless too.
#[derive(Component, Debug, Clone, Copy, Reflect, Default)]
#[reflect(Component)]
pub struct Earthshine;

/// Validate a live shadow-range edit against the current cascade configuration.
///
/// A runtime command is an explicit request, not a quality preset. Invalid
/// values are therefore rejected rather than clamped or replaced with a
/// renderer default. Returning the current values for omitted fields also
/// makes a partial command preserve the other authored/live range exactly.
fn validated_shadow_ranges(
    minimum_distance: f32,
    current_first_cascade_bound: f32,
    current_maximum_distance: f32,
    requested_first_cascade_bound: Option<f32>,
    requested_maximum_distance: Option<f32>,
) -> Option<(f32, f32)> {
    if requested_first_cascade_bound.is_none() && requested_maximum_distance.is_none() {
        return None;
    }

    let first = requested_first_cascade_bound.unwrap_or(current_first_cascade_bound);
    let maximum = requested_maximum_distance.unwrap_or(current_maximum_distance);
    (minimum_distance.is_finite()
        && first.is_finite()
        && maximum.is_finite()
        && minimum_distance < first
        && first < maximum)
        .then_some((first, maximum))
}

/// Applies a [`SetEnvironmentLight`] command to semantic [`SunState`] first,
/// then to the render projection and the other environment projections. The
/// render light is never the source of direction or irradiance: commands and
/// ephemeris both publish the semantic state, and one projection system writes
/// the light from it.
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
    mut sun_state: ResMut<SunState>,
    // The sun(s): every directional light EXCEPT the earthshine fill, so an
    // illuminance/color/direction tweak never clobbers the fill light.
    mut q_sun: Query<
        (
            &mut Transform,
            &mut DirectionalLight,
            Option<&mut CascadeShadowConfig>,
        ),
        (
            With<DirectionalLight>,
            Without<Earthshine>,
            Without<RenderLayers>,
        ),
    >,
    mut q_earthshine: Query<&mut DirectionalLight, With<Earthshine>>,
    mut q_exposure: Query<&mut Exposure>,
    ambient: Option<ResMut<GlobalAmbientLight>>,
) {
    let cmd = trigger.event();

    // The command has one authoritative scene-sun target. Refuse ambiguity
    // rather than applying a user command to an arbitrary set of lights.
    if q_sun.iter().count() == 1 {
        let Ok((mut _tf, mut light, cascades)) = q_sun.single_mut() else {
            unreachable!("a counted scene sun must remain queryable");
        };
        if cmd.sun_yaw.is_some() || cmd.sun_pitch.is_some() {
            let Some(direction) = sun_state.direction_to_sun else {
                warn!(
                    "SetEnvironmentLight direction request rejected: semantic SunState has no provider sample"
                );
                return;
            };
            // Preserve the unspecified axis from semantic state. Reading the
            // render transform here would create a second direction authority.
            let Some(direction) = SunState::normalized_direction(direction) else {
                warn!(
                    "SetEnvironmentLight direction request rejected: semantic SunState direction is invalid"
                );
                return;
            };
            let rotation = Quat::from_rotation_arc(Vec3::Z, direction);
            let (cur_yaw, cur_pitch, _) = rotation.to_euler(EulerRot::YXZ);
            let yaw = cmd.sun_yaw.unwrap_or(cur_yaw);
            let pitch = cmd.sun_pitch.unwrap_or(cur_pitch);
            let next = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0)
                .mul_vec3(Vec3::Z)
                .normalize();
            let irradiance = sun_state.irradiance_lux;
            sun_state.publish(next, irradiance);
        }

        if let Some(lux) = cmd.illuminance {
            if !lux.is_finite() || lux < 0.0 {
                warn!(
                    "SetEnvironmentLight illuminance request rejected: expected a finite non-negative value"
                );
                return;
            }
            sun_state.set_irradiance(Some(lux));
        }
        if let Some([r, g, b]) = cmd.sun_color {
            light.color = Color::linear_rgb(r, g, b);
        }
        if let Some(s) = cmd.shadow_maps_enabled {
            light.shadow_maps_enabled = s;
        }
        if cmd.shadow_first_cascade_bound.is_some() || cmd.shadow_max_distance.is_some() {
            let Some(mut cfg) = cascades else {
                warn!(
                    "SetEnvironmentLight shadow-range request ignored: the scene sun has no cascade configuration"
                );
                return;
            };
            let Some((&cur_first, &cur_max)) = cfg.bounds.first().zip(cfg.bounds.last()) else {
                warn!(
                    "SetEnvironmentLight shadow-range request ignored: the scene sun has no cascade bounds"
                );
                return;
            };
            let Some((first, maximum)) = validated_shadow_ranges(
                cfg.minimum_distance,
                cur_first,
                cur_max,
                cmd.shadow_first_cascade_bound,
                cmd.shadow_max_distance,
            ) else {
                warn!(
                    "SetEnvironmentLight shadow-range request ignored: first cascade bound must be finite and greater than the minimum distance, and maximum distance must be finite and greater than the first bound"
                );
                return;
            };
            // Rebuild from the live config, overriding only the two requested
            // range knobs. No renderer default or clamp is allowed to replace
            // an invalid explicit request.
            *cfg = CascadeShadowConfigBuilder {
                num_cascades: cfg.bounds.len(),
                minimum_distance: cfg.minimum_distance,
                first_cascade_far_bound: first,
                maximum_distance: maximum,
                overlap_proportion: cfg.overlap_proportion,
            }
            .build();
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

    // Earthshine fill light — TINT only; brightness is phase-derived.
    //
    // The fill exists only in a scene that DECLARES the body it reflects from
    // (`lunco://lighting/earthshine.usda`, nested under that body's prim), so
    // this query is legitimately empty in a scene with no sky. Report it rather
    // than let the request evaporate: nothing here can conjure the light, since
    // which body it belongs to — and therefore its direction and phase — is
    // exactly what the scene did not say. Spawning a stand-in would be an
    // unshadowed fill nobody authored, aimed nowhere in particular.
    if cmd.earthshine_color.is_some() && q_earthshine.is_empty() {
        warn_once!(
            "[environment] earthshine requested, but this scene declares no body to \
             reflect it — nothing to apply. Reference a celestial body that carries \
             the fill (`lunco://celestial/solar_system.usda` brings Earth's), or nest \
             `lunco://lighting/earthshine.usda` under the body prim yourself."
        );
    }
    for mut fill in &mut q_earthshine {
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
/// Add after [`lunco_celestial_spatial::GravityPlugin`]. Ordering in `FixedUpdate`:
/// 1. [`EnvironmentSet::Compute`] — writes `LocalGravity` (and future `Local*`)
/// 2. [`EnvironmentSet::Apply`] — projects gravity onto Avian RigidBodies
pub struct EnvironmentPlugin;

fn clear_environment_sun_state(mut sun: ResMut<SunState>, mut render_sun: ResMut<SunRenderState>) {
    sun.clear();
    render_sun.clear();
}

// NOTE: earthshine is not spawned here. It is authored USD, nested under the
// body it comes from (`lunco://lighting/earthshine.usda`, referenced by the
// Earth prim in `lunco://celestial/solar_system.usda`), so a scene gets the fill
// by declaring the body rather than by the engine adding a light nobody asked
// for. `lunco-usd-sim` stamps [`Earthshine`] from that namespace structure.
//
// ⚠ WEB: WebGL2 supports a single `DirectionalLight`, and a second one culls the
// sun. A wasm build must therefore not compose a body fill — the gate now lives
// where the light is instantiated rather than where it used to be spawned.

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

        // Sim core — render-free. Gravity computation, acceleration projection, and
        // the gravity→cosim bridge.
        //
        // Gravity is a persistent per-body acceleration. The projection is not
        // gated on the live clock: Avian owns its lifetime and applies the
        // current component whenever a physics step actually runs. The other
        // systems in this set publish a VALUE (cosim input, IMU field) rather
        // than accumulate into the force buffer, so they also keep running
        // while physics is held.
        app.add_systems(
            FixedUpdate,
            (
                compute_local_gravity.in_set(EnvironmentSet::Compute),
                sync_local_gravity_to_avian.in_set(EnvironmentSet::Apply),
                // Publish gravity into the cosim graph after it's computed and
                // before cosim copies outputs→inputs, so models read the real
                // local value the same tick.
                inject_local_gravity_into_cosim
                    .in_set(EnvironmentSet::Apply)
                    .before(lunco_cosim_core::schedule::CosimSet::Propagate),
            ),
        );

        // Lighting half — RENDER-FREE. `DirectionalLight` is `bevy_light` and
        // `RenderLayers` is `bevy_camera`; neither depends on `bevy_render`, so
        // the earthshine fill and the sun→cosim direction feed run headless too
        // (a sun-tracking Modelica model on the `--no-ui` server needs them).
        app.register_type::<LocalSolar>();
        app.register_type::<LocalEarth>();
        app.register_type::<EnvironmentProbe>();
        app.register_type::<EarthDirectionRequired>();
        app.register_type::<Earthshine>();
        // Declared here, WRITTEN by lunco-celestial (which depends on this crate,
        // so the dependency cannot run the other way). Init'd unconditionally and
        // left at ZERO — the "not known" state — so a scene with no celestial
        // hierarchy reads as no-data rather than as a missing resource.
        app.init_resource::<EarthDirectionWorld>();
        app.init_resource::<SunState>();
        app.init_resource::<SunRenderState>();
        app.init_resource::<SunRenderPresentation>();

        // SunState is scene-owned semantic state. Clear it at the same
        // lifecycle edge as the authored light entities so a replacement
        // scene cannot inherit the outgoing scene's direction.
        app.add_systems(lunco_core::SceneTeardown, clear_environment_sun_state);

        // Semantic sun state is the provider boundary. The light's local pose
        // is projected before BigSpace propagation; its finalized world
        // direction is published afterwards for render consumers.
        app.add_systems(Update, project_sun_render_to_light);
        app.add_systems(
            PostUpdate,
            finalize_sun_render_state
                .after(big_space::prelude::BigSpaceSystems::PropagateLowPrecision),
        );

        // Earthshine follows Earth's phase — the ONE writer of the fill's
        // illuminance. In `Update` rather than `FixedUpdate`: it is a render
        // quantity read by the extract, not something a physics step consumes,
        // and the phase moves ~0.5°/day so it is nowhere near rate-sensitive.
        app.add_systems(Update, lighting::drive_earthshine_from_phase);

        // Solar source: mirror gravity. Compute the per-entity sun
        // direction, then publish it as cosim outputs before propagation
        // so a sun-tracking model reads it the same tick.
        app.add_systems(
            FixedUpdate,
            (
                compute_local_solar.in_set(EnvironmentSet::Compute),
                inject_local_solar_into_cosim
                    .in_set(EnvironmentSet::Apply)
                    .before(lunco_cosim_core::schedule::CosimSet::Propagate),
                // Earth pointing rides the same three-phase ordering: an antenna
                // model must read the angles the same tick they were computed.
                compute_local_earth.in_set(EnvironmentSet::Compute),
                inject_local_earth_into_cosim
                    .in_set(EnvironmentSet::Apply)
                    .before(lunco_cosim_core::schedule::CosimSet::Propagate),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_range_validation_rejects_invalid_explicit_values_without_clamping() {
        assert_eq!(
            validated_shadow_ranges(0.1, 40.0, 1500.0, Some(900.0), Some(50.0)),
            None
        );
        assert_eq!(
            validated_shadow_ranges(0.1, 40.0, 1500.0, Some(f32::NAN), None),
            None
        );
        assert_eq!(
            validated_shadow_ranges(0.1, 40.0, 1500.0, None, Some(f32::INFINITY)),
            None
        );
    }

    #[test]
    fn shadow_range_validation_preserves_omitted_live_value() {
        assert_eq!(
            validated_shadow_ranges(0.1, 40.0, 1500.0, Some(80.0), None),
            Some((80.0, 1500.0))
        );
        assert_eq!(
            validated_shadow_ranges(0.1, 40.0, 1500.0, None, Some(3000.0)),
            Some((40.0, 3000.0))
        );
    }

    #[test]
    fn flat_gravity_stays_in_the_active_physics_frame() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin));
        app.insert_resource(Gravity::flat(1.62, DVec3::NEG_Y));
        app.add_systems(Update, compute_local_gravity);

        let frame = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                Transform::from_rotation(Quat::from_rotation_z(0.8)),
                GlobalTransform::default(),
            ))
            .id();
        let body = app
            .world_mut()
            .spawn((
                Transform::default(),
                GlobalTransform::default(),
                ChildOf(frame),
            ))
            .id();
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(frame));

        app.update();

        let LocalGravity(actual) = *app
            .world()
            .get::<LocalGravity>(body)
            .expect("gravity is projected onto every spatial entity");
        assert!(actual.abs_diff_eq(DVec3::new(0.0, -1.62, 0.0), 1.0e-12));
    }
}
