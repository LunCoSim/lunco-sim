//! # lunco-environment
//!
//! Per-entity environmental state computed from celestial body providers.
//!
//! See `README.md` for the full architecture, rationale, and how to add new
//! environment domains (atmosphere, radiation, magnetic field, etc.).
//!
//! Currently implements gravity, solar-direction inputs, lunar lighting, and
//! horizon self-shadowing. See the README for ownership and extension guidance.

use avian3d::prelude::{ConstantLinearAcceleration, Position, RigidBody, Rotation};
use bevy::ecs::{change_detection::Tick, system::SystemChangeTick};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_core::AppSceneExt;
use lunco_core::Command;
use lunco_physics::{PhysicsPoseSeeded, PrescribedSimulationPose, SimulationPoseQuery};
use lunco_spatial::coords::GridPos;
use std::collections::HashMap;

/// USD prim type for the scene-level **environment settings** prim (a singleton
/// under the default prim, e.g. `/World/Environment`). It carries the render
/// knobs that have no natural light-prim home — `lunco:env:exposureEv100` and
/// `lunco:env:bloomIntensity`.
///
/// Earthshine tint is the authored DistantLight color. Its intensity follows
/// the Earth's phase. Untextured DomeLights contribute to global ambient light.
/// A persistent ambient edit sets the Environment/AmbientFill intensity needed
/// to reach the requested total after accounting for other composed domes.
pub const LUNCO_ENVIRONMENT_PRIM_TYPE: &str = "LunCoEnvironment";

/// Gravity configuration types (`Gravity`, `GravityBody`, `GravityProvider`,
/// `GravityModel`) — environmental-state vocabulary owned here. The gravity
/// *systems* in `lunco_celestial` import these.
pub mod gravity_types;
pub use gravity_types::{
    EARTH_SURFACE_GRAVITY, Gravity, GravityBody, GravityModel, GravityProvider,
    MOON_SURFACE_GRAVITY, PhysicsSceneGravity,
};

/// Physical lighting parameters of the lunar sky (`LunarSun`, `FULL_EARTH_EARTHSHINE_LUX`)
/// — environmental state, the lighting analog of gravity. See the module docs.
pub mod lighting;
pub use lighting::{FULL_EARTH_EARTHSHINE_LUX, LunarSun, drive_earthshine_from_phase};

/// Solar render projection over the generic direction source system.
pub mod solar;
use solar::sun_render_finalize_needed;
pub use solar::{
    SunRenderState, SunState, finalize_sun_render_state, project_environment_sun_to_light,
    tracked_sun_light_projection,
};

/// Universal framed direction sources, coordinate conversion, and direct
/// source-to-cosim publication.
pub mod directions;
pub use directions::{
    CelestialSourceClassification, DirectionResolutionError, DirectionSourceId,
    DirectionSourceRequirements, DirectionTargetId, EARTH_DIRECTION_SOURCE, EnvironmentDirections,
    FramedDirection, SUN_DIRECTION_SOURCE, publish_direction_sources_to_cosim,
    resolve_direction_for_frame,
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

/// Explicit, equally weighted radiation sample locations in the probe frame.
/// USD PointBased.points owns these positions; an Xform probe is a point sample.
#[derive(Component, Debug, Clone)]
pub struct ProbeSampling(pub Vec<bevy::math::DVec3>);

/// Explicit optical proxy geometry selected by standard CollectionAPI:occluders.
/// Membership is composed USD prim identity; collision shapes supply the f64 proxy.
#[derive(Component, Clone, Debug, Default)]
pub struct ProbeOccluderPaths(pub Vec<String>);

/// Baked horizon-map terrain self-shadowing (the long-range half of the
/// two-system shadow design). **Render-free**: the heightfield bakes and the
/// sun-visibility cache run headless; the material wiring they feed lives in
/// `lunco-render-bevy::horizon_shade`. See the module docs.
pub mod horizon;
pub use horizon::{
    HeightField, HorizonMap, HorizonShadowCache, HorizonShadowCacheConfig, HorizonShadowPlugin,
    SunQuery, horizon_march_parameters, install_horizon_map_from_field, pick_sun,
};

/// System sets for environment computation and consumption.
///
/// Ordered chain in [`FixedUpdate`]:
/// 1. [`Compute`](EnvironmentSet::Compute) — compute per-entity `Local*` values
/// 2. [`Apply`](EnvironmentSet::Apply) — project gravity and publish direct cosim outputs
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnvironmentSet {
    /// Computes per-entity environment components from body providers.
    Compute,
    /// Applies environment effects (e.g., gravity acceleration on RigidBodies).
    Apply,
}

// Gravity computation publishes the physics-owned field and sample provenance.
use lunco_physics::kinematics::{GravityChangeOrigin, LocalGravity};

#[derive(Clone, Copy)]
struct SampledGravityProviderPose {
    pose: (GridPos, DQuat),
    sampled_at: Tick,
    pose_changed: bool,
}

/// Provider samples retained between gravity passes within one scene.
#[derive(Resource, Default)]
pub struct GravityProviderPoseCache(HashMap<Entity, SampledGravityProviderPose>);

impl lunco_core::SceneResource for GravityProviderPoseCache {}

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
    updates: &mut Vec<(Entity, LocalGravity)>,
    gravity: &Gravity,
    poses: &SimulationPoseQuery,
    entity: Entity,
    gravity_body: Option<Ref<GravityBody>>,
    existing: Option<&LocalGravity>,
    body_pose_cache: &mut HashMap<Entity, SampledGravityProviderPose>,
    q_bodies: &Query<(Entity, Ref<GravityProvider>)>,
    sampled_at: Tick,
    source_invalidated: bool,
) {
    let mut source_changed =
        source_invalidated || gravity_body.as_ref().is_some_and(|link| link.is_changed());
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
            let Ok((_, provider)) = q_bodies.get(body_link.body_entity) else {
                clear_unresolved_local_gravity(commands, entity, existing);
                return;
            };
            source_changed |= provider.is_changed();
            // The shared pose owner includes mounted native ancestors and
            // rejects unseeded state before a field can enter either solver.
            let Some(GridPos(entity_position)) = poses.position(entity) else {
                clear_unresolved_local_gravity(commands, entity, existing);
                return;
            };
            let previous = body_pose_cache.get(&body_link.body_entity).copied();
            let body_pose = if previous.is_some_and(|sample| sample.sampled_at == sampled_at) {
                previous
            } else {
                poses
                    .pose(body_link.body_entity)
                    .map(|(position, rotation)| {
                        let pose = (position, rotation.0);
                        let sample = SampledGravityProviderPose {
                            pose,
                            sampled_at,
                            pose_changed: previous.is_none_or(|previous| previous.pose != pose),
                        };
                        body_pose_cache.insert(body_link.body_entity, sample);
                        sample
                    })
            };
            let Some(sample) = body_pose else {
                body_pose_cache.remove(&body_link.body_entity);
                clear_unresolved_local_gravity(commands, entity, existing);
                return;
            };
            source_changed |= sample.pose_changed;
            let (body_position, body_rotation) = sample.pose;
            // Relative-grid composition stops at the shared ancestor. Its
            // astronomical translation and render-time rotation cannot alter
            // a local body's force or introduce cancellation noise.
            let relative_body = body_rotation.inverse() * (entity_position - body_position.0);
            body_rotation * provider.model.acceleration(relative_body)
        }
    };
    // Don't re-insert (and re-trigger) when the value is unchanged — e.g. a
    // global invalidation that recomputes the same field.
    if existing.is_some_and(|previous| previous.acceleration() == g) {
        return;
    }
    updates.push((
        entity,
        LocalGravity::sampled(
            g,
            if source_changed {
                GravityChangeOrigin::Source
            } else {
                GravityChangeOrigin::ConsumerPose
            },
        ),
    ));
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
    q_bodies: Query<(Entity, Ref<GravityProvider>)>,
    q_changed_providers: Query<(), Changed<GravityProvider>>,
    mut removed_providers: RemovedComponents<GravityProvider>,
    mut removed_body_links: RemovedComponents<GravityBody>,
    mut removed_prescribed: RemovedComponents<PrescribedSimulationPose>,
    mut removed_poses: ParamSet<(
        RemovedComponents<PhysicsPoseSeeded>,
        RemovedComponents<RigidBody>,
        RemovedComponents<Position>,
        RemovedComponents<Rotation>,
        RemovedComponents<ChildOf>,
        RemovedComponents<Transform>,
        RemovedComponents<CellCoord>,
        RemovedComponents<Grid>,
    )>,
    mut q_entities: ParamSet<(
        Query<
            (Entity, Option<Ref<GravityBody>>, Option<&LocalGravity>),
            (With<Transform>, Or<(With<GravityBody>, With<LocalGravity>)>),
        >,
        Query<(Entity, Option<Ref<GravityBody>>, Option<&LocalGravity>), With<Transform>>,
        Query<Entity, (With<Transform>, Without<LocalGravity>)>,
        Query<Entity, (With<LocalGravity>, Without<Transform>)>,
    )>,
    q_parents: Query<&ChildOf>,
    q_changed_poses: Query<
        (),
        Or<(
            Changed<Position>,
            Changed<Rotation>,
            Changed<PhysicsPoseSeeded>,
            Changed<PrescribedSimulationPose>,
            Changed<RigidBody>,
            Changed<Transform>,
            Changed<CellCoord>,
            Changed<Grid>,
            Changed<ChildOf>,
        )>,
    >,
    poses: SimulationPoseQuery,
    mut body_pose_cache: ResMut<GravityProviderPoseCache>,
    system_tick: SystemChangeTick,
) {
    // A mount's local Transform stays unchanged while its native body moves.
    // Follow the shared hierarchy invalidation path before reusing its field.
    // Readiness withdrawal invalidates descendants as well as the body itself.
    // Drain each removal stream independently; one invalidation must not defer
    // another stream's cleanup into a later simulation tick.
    let provider_removed = removed_providers.read().count() > 0;
    let transform_removed = removed_poses.p5().read().count() > 0;
    let pose_input_removed = [
        removed_poses.p0().read().count() > 0,
        removed_poses.p1().read().count() > 0,
        removed_poses.p2().read().count() > 0,
        removed_poses.p3().read().count() > 0,
        removed_poses.p4().read().count() > 0,
        transform_removed,
        removed_poses.p6().read().count() > 0,
        removed_poses.p7().read().count() > 0,
        removed_prescribed.read().count() > 0,
    ]
    .into_iter()
    .any(|removed| removed);
    if transform_removed {
        // Entities that lost Transform are outside every spatial work query.
        // Withdraw their field before the cosimulation publication boundary.
        for entity in &q_entities.p3() {
            commands.entity(entity).remove::<LocalGravity>();
        }
    }
    let provider_changed = !q_changed_providers.is_empty()
        || provider_removed
        || q_bodies.iter().any(|(entity, _)| {
            lunco_spatial::coords::world_pose_changed(entity, &q_parents, &q_changed_poses)
        });
    let body_link_removed = removed_body_links.read().count() > 0;
    let global_invalidation = gravity.is_changed()
        || active_frame.as_ref().is_some_and(Res::is_changed)
        || (matches!(gravity.as_ref(), Gravity::Surface) && active_frame.is_none())
        || provider_changed
        || body_link_removed
        || (matches!(gravity.as_ref(), Gravity::Surface) && pose_input_removed);
    let source_invalidated =
        gravity.is_changed() || active_frame.as_ref().is_some_and(Res::is_changed);
    // A flat field has no position dependence. Once admitted, moving entities
    // retain the same vector until an owner-level invalidation. The missing
    // field query is archetypal, so steady motion does not scan change ticks.
    if !global_invalidation {
        if let Gravity::Flat { g, direction } = gravity.as_ref() {
            let missing = q_entities.p2();
            if !missing.is_empty() {
                let acceleration = *direction * *g;
                let updates: Vec<_> = missing
                    .iter()
                    .map(|entity| (entity, LocalGravity::new(acceleration)))
                    .collect();
                commands.try_insert_batch(updates);
            }
            return;
        }
    }
    let has_work = global_invalidation || !q_entities.p0().is_empty();
    if !has_work {
        return;
    }
    let mut updates = Vec::new();
    // Several moving consumers can share one gravity body. Its active-frame pose is
    // stable for this read-only system pass, so resolve it once per provider.
    body_pose_cache
        .0
        .retain(|entity, _| q_bodies.contains(*entity));
    // A surface field can only exist on a linked consumer. Even a provider
    // invalidation need not visit unrelated visual entities or terrain tiles.
    if global_invalidation && matches!(gravity.as_ref(), Gravity::Flat { .. }) {
        for (entity, gravity_body, existing) in q_entities.p1().iter() {
            update_local_gravity_for_entity(
                &mut commands,
                &mut updates,
                gravity.as_ref(),
                &poses,
                entity,
                gravity_body,
                existing,
                &mut body_pose_cache.0,
                &q_bodies,
                system_tick.this_run(),
                source_invalidated,
            );
        }
    } else {
        for (entity, gravity_body, existing) in q_entities.p0().iter() {
            if !global_invalidation
                && existing.is_some()
                && !gravity_body.as_ref().is_some_and(|link| link.is_changed())
                && !lunco_spatial::coords::world_pose_changed(entity, &q_parents, &q_changed_poses)
            {
                continue;
            }
            update_local_gravity_for_entity(
                &mut commands,
                &mut updates,
                gravity.as_ref(),
                &poses,
                entity,
                gravity_body,
                existing,
                &mut body_pose_cache.0,
                &q_bodies,
                system_tick.this_run(),
                source_invalidated,
            );
        }
    }
    if !updates.is_empty() {
        commands.try_insert_batch(updates);
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
/// Every consumer reads the shared `LocalGravity` sample. A moving body
/// samples its position-dependent field without publishing an external-force
/// change to Avian's wake observer, including the final motion sample when a body
/// falls asleep. Source changes retain the standard notification. External pose
/// changes have their own native wake detection. The exact acceleration is always
/// available to the native integrator, including rollback.
pub fn sync_local_gravity_to_avian(
    mut commands: Commands,
    mut changed: Query<
        (
            Entity,
            &LocalGravity,
            Option<&mut ConstantLinearAcceleration>,
        ),
        (
            With<RigidBody>,
            Or<(Changed<LocalGravity>, Changed<RigidBody>)>,
        ),
    >,
    mut removed_gravity: RemovedComponents<LocalGravity>,
    mut removed_bodies: RemovedComponents<RigidBody>,
) {
    let mut accelerations = Vec::new();
    for (entity, gravity, existing) in &mut changed {
        let acceleration = gravity.acceleration();
        if !lunco_physics::avian_backend_vector_is_valid(acceleration) {
            commands.queue(move |world: &mut World| {
                world
                    .resource_mut::<lunco_physics::PhysicsHolds>()
                    .set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
                world.resource_mut::<lunco_core::RuntimeFaults>().raise(
                    "physics-gravity-range-invalid",
                    Some(entity),
                    "local gravity",
                    "gravity acceleration exceeds the native solver range",
                );
            });
            continue;
        }
        let native_acceleration = acceleration.as_vec3();
        if let Some(mut existing) = existing {
            if existing.0 == native_acceleration {
                continue;
            }
            if gravity.change_origin() == GravityChangeOrigin::ConsumerPose {
                existing.bypass_change_detection().0 = native_acceleration;
            } else {
                existing.0 = native_acceleration;
            }
        } else {
            accelerations.push((entity, ConstantLinearAcceleration(native_acceleration)));
        }
    }
    if !accelerations.is_empty() {
        commands.try_insert_batch(accelerations);
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
/// The co-simulation master propagates ordinary signals; the environment owner
/// supplies position-dependent gravity through the same wired port contract.
///
/// Runs in [`EnvironmentSet::Apply`] (after `LocalGravity` is computed) and
/// before cosim's propagation, so the freshly-written output is read the same
/// tick. Stable values do not allocate connector names or mark the participant
/// changed. In surface-gravity scenes where no provider has resolved yet, the
/// gravity outputs are removed rather than exposing stale values.
pub fn inject_local_gravity_into_cosim(
    mut q: Query<
        (Option<&LocalGravity>, &mut lunco_cosim_core::SimComponent),
        With<EnvironmentProbe>,
    >,
) {
    for (gravity, mut comp) in &mut q {
        let changed = {
            let outputs = &mut comp.bypass_change_detection().outputs;
            if let Some(gravity) = gravity {
                let mut changed = outputs.set(
                    lunco_cosim_core::GRAVITY_SOURCE_CONNECTOR,
                    gravity.magnitude(),
                );
                changed |= outputs.set(
                    lunco_cosim_core::GRAVITY_X_SOURCE_CONNECTOR,
                    gravity.acceleration().x,
                );
                changed |= outputs.set(
                    lunco_cosim_core::GRAVITY_Y_SOURCE_CONNECTOR,
                    gravity.acceleration().y,
                );
                changed |= outputs.set(
                    lunco_cosim_core::GRAVITY_Z_SOURCE_CONNECTOR,
                    gravity.acceleration().z,
                );
                changed
            } else {
                let mut changed = false;
                changed |= outputs
                    .remove(lunco_cosim_core::GRAVITY_SOURCE_CONNECTOR)
                    .is_some();
                changed |= outputs
                    .remove(lunco_cosim_core::GRAVITY_X_SOURCE_CONNECTOR)
                    .is_some();
                changed |= outputs
                    .remove(lunco_cosim_core::GRAVITY_Y_SOURCE_CONNECTOR)
                    .is_some();
                changed |= outputs
                    .remove(lunco_cosim_core::GRAVITY_Z_SOURCE_CONNECTOR)
                    .is_some();
                changed
            }
        };
        if changed {
            comp.set_changed();
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SetEnvironmentLight — runtime sun direction + ambient brightness
// ─────────────────────────────────────────────────────────────────────────────

/// Sets scene environment lighting at runtime: the sun's direction and the
/// global ambient level.
///
/// Supply the fields to change. The owner validates the complete request before
/// applying a live update or committing an explicit persistent edit.
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
    pub bloom_intensity: Option<f32>,
    /// Live by default; Persistent explicitly authors supported USD light settings.
    #[serde(default)]
    #[reflect(default)]
    pub intent: lunco_core::EditIntent,
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

/// Admitted targets for one lighting command. Rotation is expressed in the active physics frame.
pub struct EnvironmentLightTargets {
    pub sun: Option<Entity>,
    pub rotation: Option<DQuat>,
    pub earthshine: Option<Entity>,
}

/// Validate the entire request before applying a live update or returning its document targets.
pub fn update_environment_light(
    In(cmd): In<SetEnvironmentLight>,
    mut sun_state: ResMut<SunState>,
    mut directions: ResMut<EnvironmentDirections>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    scene_mount: Option<Res<lunco_core::SceneMountState>>,
    poses: SimulationPoseQuery<Without<DirectionalLight>>,
    q_direction_targets: Query<(Entity, &DirectionTargetId)>,
    q_classifications: Query<&CelestialSourceClassification>,
    mounted_lights: solar::MountedDirectionalLights,
    mut suns: Query<
        (
            Entity,
            &mut DirectionalLight,
            Option<&mut CascadeShadowConfig>,
        ),
        (
            Without<Earthshine>,
            Without<lunco_render::EditorPreviewLight>,
        ),
    >,
    mut fills: Query<
        (Entity, &mut DirectionalLight),
        (With<Earthshine>, Without<lunco_render::EditorPreviewLight>),
    >,
    mut exposures: Query<&mut Exposure, Without<lunco_render::EditorPreviewCamera>>,
    ambient: Option<ResMut<GlobalAmbientLight>>,
    bloom: (
        Option<Res<lunco_render::RenderingQualitySettings>>,
        Option<ResMut<lunco_render::SceneBloomOverride>>,
    ),
    mut cameras: Query<
        &mut lunco_render::camera::SceneCamera,
        Without<lunco_render::EditorPreviewCamera>,
    >,
) -> Result<EnvironmentLightTargets, String> {
    let direction_requested = cmd.sun_yaw.is_some() || cmd.sun_pitch.is_some();
    let range_requested =
        cmd.shadow_first_cascade_bound.is_some() || cmd.shadow_max_distance.is_some();
    let sun_requested = direction_requested
        || range_requested
        || cmd.illuminance.is_some()
        || cmd.shadow_maps_enabled.is_some()
        || cmd.sun_color.is_some();
    if [cmd.sun_pitch, cmd.sun_yaw, cmd.exposure_ev100]
        .into_iter()
        .flatten()
        .any(|v| !v.is_finite())
        || [cmd.sun_color, cmd.earthshine_color]
            .into_iter()
            .flatten()
            .flatten()
            .any(|v| !v.is_finite() || v < 0.0)
        || [cmd.illuminance, cmd.ambient_brightness, cmd.bloom_intensity]
            .into_iter()
            .flatten()
            .any(|v| !v.is_finite() || v < 0.0)
    {
        return Err(
            "Lighting requires finite settings and non-negative intensity and color".into(),
        );
    }
    let sun = if sun_requested {
        Some(
            suns.get_mut(mounted_lights.sun()?)
                .map_err(|_| "Mounted Sun light is unavailable")?,
        )
    } else {
        None
    };
    let fill = if cmd.earthshine_color.is_some() {
        Some(
            fills
                .get_mut(mounted_lights.earthshine()?)
                .map_err(|_| "Mounted earthshine light is unavailable")?,
        )
    } else {
        None
    };
    let ranges = if range_requested {
        let cfg = sun
            .as_ref()
            .and_then(|(_, _, cfg)| cfg.as_deref())
            .ok_or("Sun has no shadow cascade configuration")?;
        let first = *cfg.bounds.first().ok_or("Sun has empty cascade bounds")?;
        let maximum = *cfg.bounds.last().ok_or("Sun has empty cascade bounds")?;
        Some(
            validated_shadow_ranges(
                cfg.minimum_distance,
                first,
                maximum,
                cmd.shadow_first_cascade_bound,
                cmd.shadow_max_distance,
            )
            .ok_or("Invalid shadow range: require minimum < first bound < maximum")?,
        )
    } else {
        None
    };
    if direction_requested || cmd.illuminance.is_some() {
        let root = scene_mount
            .as_deref()
            .and_then(|mount| mount.active_root())
            .ok_or("No active scene")?;
        if q_classifications
            .get(root)
            .map_err(|_| "Scene Sun source has not been classified")?
            .has_source
        {
            return Err("Celestial bodies own Sun direction and illuminance in this scene".into());
        }
    }
    let direction = if direction_requested {
        if q_direction_targets
            .iter()
            .any(|(_, target)| target.as_str() == SUN_DIRECTION_SOURCE)
        {
            return Err("Sun is an authored position target in this scene".into());
        }
        let frame = active_frame.as_deref().ok_or("No active physics frame")?.0;
        let source =
            DirectionSourceId::parse(SUN_DIRECTION_SOURCE).expect("valid built-in direction id");
        let current = resolve_direction_for_frame(
            &source,
            frame,
            Some(&directions),
            &q_direction_targets,
            &poses,
        )
        .map_err(|error| format!("Cannot resolve Sun direction: {error:?}"))?;
        let (yaw, pitch, _) =
            DQuat::from_rotation_arc(DVec3::Z, current.components()).to_euler(EulerRot::YXZ);
        let rotation = DQuat::from_euler(
            EulerRot::YXZ,
            cmd.sun_yaw.map(f64::from).unwrap_or(yaw),
            cmd.sun_pitch.map(f64::from).unwrap_or(pitch),
            0.0,
        );
        let direction = lunco_spatial::coords::UnitDirection3::normalized(rotation * DVec3::Z)
            .ok_or("Invalid Sun direction")?;
        Some((rotation, FramedDirection { frame, direction }))
    } else {
        None
    };
    let targets = EnvironmentLightTargets {
        sun: sun.as_ref().map(|(entity, ..)| *entity),
        rotation: direction.as_ref().map(|(rotation, _)| *rotation),
        earthshine: fill.as_ref().map(|(entity, _)| *entity),
    };
    if cmd.intent.is_persistent() {
        return Ok(targets);
    }
    if cmd.ambient_brightness.is_some() && ambient.is_none() {
        return Err("Ambient lighting is unavailable".into());
    }
    let bloom = if let Some(intensity) = cmd.bloom_intensity {
        let settings = bloom.0.ok_or("Graphics settings are unavailable")?;
        let profile = settings
            .validated_profile()
            .map_err(|error| error.to_string())?;
        Some((
            intensity,
            profile.camera_bloom_low_frequency_boost,
            bloom.1.ok_or("Scene bloom is unavailable")?,
        ))
    } else {
        None
    };
    if let Some((_, next)) = direction {
        directions.set_named(SUN_DIRECTION_SOURCE, Some(next));
    }
    if let Some((_, mut light, cfg)) = sun {
        if let Some(lux) = cmd.illuminance {
            sun_state.publish(Some(lux));
        }
        if let Some([r, g, b]) = cmd.sun_color {
            light.color = Color::linear_rgb(r, g, b);
        }
        if let Some(enabled) = cmd.shadow_maps_enabled {
            light.shadow_maps_enabled = enabled;
        }
        if let (Some(mut cfg), Some((first, maximum))) = (cfg, ranges) {
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
    if let (Some((_, mut fill)), Some([r, g, b])) = (fill, cmd.earthshine_color) {
        fill.color = Color::linear_rgb(r, g, b);
    }
    if let (Some(mut ambient), Some(value)) = (ambient, cmd.ambient_brightness) {
        ambient.brightness = value;
    }
    if let Some(ev) = cmd.exposure_ev100 {
        for mut exposure in &mut exposures {
            exposure.ev100 = ev;
        }
    }
    if let Some((intensity, boost, mut state)) = bloom {
        state.intensity = Some(intensity);
        for mut camera in &mut cameras {
            let boost = camera.bloom.map_or(boost, |look| look.low_frequency_boost);
            let next =
                (intensity > 0.0).then(|| lunco_render::camera::BloomLook::new(intensity, boost));
            if camera.bloom != next {
                camera.bloom = next;
            }
            if camera.hdr != next.is_some() {
                camera.hdr = next.is_some();
            }
        }
    }
    Ok(targets)
}

// ─────────────────────────────────────────────────────────────────────────────
// Plugin
// ─────────────────────────────────────────────────────────────────────────────

/// Registers environment components, computation, and consumption systems.
///
/// Add after [`lunco_celestial_spatial::GravityPlugin`]. Ordering in `FixedUpdate`:
/// 1. [`EnvironmentSet::Compute`] — writes `LocalGravity`
/// 2. [`EnvironmentSet::Apply`] — applies gravity and publishes environment cosim outputs
pub struct EnvironmentPlugin;

fn clear_environment_sun_state(
    mut sun: ResMut<SunState>,
    mut directions: ResMut<EnvironmentDirections>,
    mut render_sun: ResMut<SunRenderState>,
) {
    sun.clear();
    directions.clear();
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
        app.init_scene_resource::<GravityProviderPoseCache>();

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

        app.init_schedule(lunco_cosim_core::schedule::InitializationSampling);
        app.configure_sets(
            lunco_cosim_core::schedule::InitializationSampling,
            (EnvironmentSet::Compute, EnvironmentSet::Apply).chain(),
        );
        app.add_systems(
            lunco_cosim_core::schedule::InitializationSampling,
            (
                compute_local_gravity.in_set(EnvironmentSet::Compute),
                inject_local_gravity_into_cosim.in_set(EnvironmentSet::Apply),
                solar::publish_fixed_solar_irradiance.in_set(EnvironmentSet::Compute),
                publish_direction_sources_to_cosim.in_set(EnvironmentSet::Apply),
            ),
        );

        // Lighting half — RENDER-FREE. `DirectionalLight` is `bevy_light` and
        // `RenderLayers` is `bevy_camera`; neither depends on `bevy_render`, so
        // the earthshine fill and generic target-direction publication run headless too
        // (a sun-tracking Modelica model on the `--no-ui` server needs them).
        app.register_type::<EnvironmentProbe>();
        app.register_type::<Earthshine>();
        // Direction samples are source-identified and frame-tagged; providers
        // publish Sun, Earth, Moon, and other bodies into this one store.
        app.init_resource::<EnvironmentDirections>();
        app.init_resource::<SunState>();
        app.init_resource::<SunRenderState>();

        // SunState is scene-owned semantic state. Clear it at the same
        // lifecycle edge as the authored light entities so a replacement
        // scene cannot inherit the outgoing scene's direction.
        app.add_systems(lunco_core::SceneTeardown, clear_environment_sun_state);

        // Semantic sun state is the provider boundary. The light's local pose
        // is projected before BigSpace propagation; its finalized world
        // direction is published afterwards for render consumers.
        app.add_systems(
            PostUpdate,
            project_environment_sun_to_light
                .run_if(tracked_sun_light_projection())
                .before(big_space::prelude::BigSpaceSystems::RecenterLargeTransforms)
                .before(bevy::transform::TransformSystems::Propagate),
        );
        app.add_systems(
            PostUpdate,
            finalize_sun_render_state
                .after(big_space::prelude::BigSpaceSystems::PropagateLowPrecision)
                .run_if(sun_render_finalize_needed),
        );

        // Earthshine follows Earth's phase — the ONE writer of the fill's
        // illuminance. In `Update` rather than `FixedUpdate`: it is a render
        // quantity read by the extract, not something a physics step consumes,
        // and the phase moves ~0.5°/day so it is nowhere near rate-sensitive.
        app.add_systems(Update, lighting::drive_earthshine_from_phase);

        // Solar source: convert the latest semantic sun sample into each probe's
        // mount frame and publish it directly to cosim outputs before propagation.
        app.add_systems(
            FixedUpdate,
            (
                solar::publish_fixed_solar_irradiance.in_set(EnvironmentSet::Compute),
                publish_direction_sources_to_cosim
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct SimComponentChangeCount(usize);

    fn count_sim_component_changes(
        changed: Query<(), Changed<lunco_cosim_core::SimComponent>>,
        mut count: ResMut<SimComponentChangeCount>,
    ) {
        count.0 += changed.iter().count();
    }

    #[derive(Resource, Default)]
    struct AccelerationChangeCount(usize);

    fn count_acceleration_changes(
        changed: Query<(), Changed<ConstantLinearAcceleration>>,
        mut count: ResMut<AccelerationChangeCount>,
    ) {
        count.0 += changed.iter().count();
    }

    #[test]
    fn gravity_projection_preserves_vectors_and_external_wake_notifications() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<AccelerationChangeCount>()
            .add_systems(
                Update,
                (sync_local_gravity_to_avian, count_acceleration_changes).chain(),
            );
        let entity = app
            .world_mut()
            .spawn((RigidBody::Dynamic, LocalGravity::new(DVec3::NEG_Y * 1.62)))
            .id();
        app.update();
        assert_eq!(app.world().resource::<AccelerationChangeCount>().0, 1);
        app.world_mut().resource_mut::<AccelerationChangeCount>().0 = 0;
        app.update();
        assert_eq!(app.world().resource::<AccelerationChangeCount>().0, 0);

        let sampled = DVec3::new(1.0e-12, -1.6200000000001, 0.0);
        app.world_mut()
            .entity_mut(entity)
            .insert(LocalGravity::sampled(
                sampled,
                GravityChangeOrigin::ConsumerPose,
            ));
        app.update();
        assert_eq!(
            app.world()
                .get::<ConstantLinearAcceleration>(entity)
                .unwrap()
                .0,
            sampled.as_vec3()
        );
        assert_eq!(
            app.world().resource::<AccelerationChangeCount>().0,
            0,
            "a position-derived field is not an external force change"
        );

        app.world_mut()
            .entity_mut(entity)
            .insert(LocalGravity::new(DVec3::NEG_Y * 1.7));
        app.update();
        assert_eq!(
            app.world()
                .get::<ConstantLinearAcceleration>(entity)
                .unwrap()
                .0,
            Vec3::NEG_Y * 1.7
        );
        assert_eq!(
            app.world().resource::<AccelerationChangeCount>().0,
            1,
            "a changed source must notify native wake detection"
        );

        let sampled = DVec3::new(0.0, -1.700000000001, 1.0e-12);
        app.world_mut().entity_mut(entity).insert((
            avian3d::prelude::Sleeping,
            LocalGravity::sampled(sampled, GravityChangeOrigin::ConsumerPose),
        ));
        app.update();
        assert_eq!(
            app.world()
                .get::<ConstantLinearAcceleration>(entity)
                .unwrap()
                .0,
            sampled.as_vec3()
        );
        assert_eq!(
            app.world().resource::<AccelerationChangeCount>().0,
            1,
            "the final native movement sample must not wake a newly sleeping body"
        );
        app.world_mut()
            .entity_mut(entity)
            .insert(LocalGravity::new(DVec3::NEG_Y * 1.8));
        app.update();
        assert_eq!(
            app.world().resource::<AccelerationChangeCount>().0,
            2,
            "a changed source must notify sleeping bodies too"
        );
        app.world_mut().entity_mut(entity).remove::<LocalGravity>();
        app.update();
        assert!(
            app.world()
                .get::<ConstantLinearAcceleration>(entity)
                .is_none()
        );
    }

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
    fn native_surface_gravity_ignores_render_pose_and_astronomical_ancestors() {
        struct Radial;
        impl GravityModel for Radial {
            fn acceleration(&self, p: DVec3) -> DVec3 {
                -p.normalize() * 1.62
            }
        }
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin));
        app.insert_resource(Gravity::surface());
        app.init_scene_resource::<GravityProviderPoseCache>();
        app.add_systems(Update, compute_local_gravity);
        let world = app
            .world_mut()
            .spawn((
                Grid::new(1.0e6, 0.0),
                Transform::from_xyz(1.0e8, 2.0e8, -3.0e8),
                GlobalTransform::default(),
            ))
            .id();
        let planet = app
            .world_mut()
            .spawn((
                Grid::new(1.0e6, 0.0),
                Transform::default(),
                GlobalTransform::default(),
                ChildOf(world),
                GravityProvider {
                    model: Box::new(Radial),
                },
            ))
            .id();
        let frame = app
            .world_mut()
            .spawn((
                Grid::new(1000.0, 0.0),
                Transform::from_xyz(0.0, 1000.0, 0.0),
                GlobalTransform::default(),
                ChildOf(planet),
            ))
            .id();
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(frame));
        let position = DVec3::new(0.125, 20.0, 0.25);
        let body = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Position(position),
                lunco_physics::PhysicsPoseSeeded,
                Transform::from_xyz(0.5, 40.0, 0.7),
                GlobalTransform::default(),
                ChildOf(frame),
                GravityBody {
                    body_entity: planet,
                },
            ))
            .id();
        let mount_offset = DVec3::new(0.75, 0.5, -0.25);
        let probe = app
            .world_mut()
            .spawn((
                Transform::from_translation(mount_offset.as_vec3()),
                GlobalTransform::default(),
                ChildOf(body),
                GravityBody {
                    body_entity: planet,
                },
            ))
            .id();
        app.update();
        let actual = app
            .world()
            .get::<LocalGravity>(body)
            .unwrap()
            .acceleration();
        let expected = -(position + DVec3::Y * 1000.0).normalize() * 1.62;
        assert_eq!(
            actual, expected,
            "gravity consumed the rendered pose rather than native Position"
        );
        let probe_expected = -(position + mount_offset + DVec3::Y * 1000.0).normalize() * 1.62;
        assert_eq!(
            app.world()
                .get::<LocalGravity>(probe)
                .unwrap()
                .acceleration(),
            probe_expected
        );
        app.world_mut()
            .get_mut::<Transform>(body)
            .unwrap()
            .translation = Vec3::new(-9.0, 50.0, 8.0);
        *app.world_mut().get_mut::<Transform>(world).unwrap() =
            Transform::from_xyz(-2.0e8, 4.0e8, 1.0e8).with_rotation(Quat::from_rotation_z(0.8));
        app.update();
        assert_eq!(
            app.world()
                .get::<LocalGravity>(body)
                .unwrap()
                .acceleration(),
            expected,
            "presentation or an ancestor above the gravity body changed local gravity"
        );
        assert_eq!(
            app.world()
                .get::<LocalGravity>(probe)
                .unwrap()
                .acceleration(),
            probe_expected
        );
        app.world_mut().get_mut::<Position>(body).unwrap().0.x += 1.0;
        app.update();
        assert_ne!(
            app.world()
                .get::<LocalGravity>(body)
                .unwrap()
                .acceleration(),
            expected,
            "native movement did not invalidate gravity without a render write"
        );
        let moved = app
            .world()
            .get::<LocalGravity>(body)
            .unwrap()
            .acceleration();
        for entity in [body, probe] {
            assert_eq!(
                app.world()
                    .get::<LocalGravity>(entity)
                    .unwrap()
                    .change_origin(),
                GravityChangeOrigin::ConsumerPose
            );
        }
        let moved_position = app.world().get::<Position>(body).unwrap().0;
        let moved_probe = -(moved_position + mount_offset + DVec3::Y * 1000.0).normalize() * 1.62;
        assert_eq!(
            app.world()
                .get::<LocalGravity>(probe)
                .unwrap()
                .acceleration(),
            moved_probe,
            "a parent native-position change did not invalidate its mounted probe"
        );
        let rotation = Quat::from_rotation_z(0.3);
        app.world_mut().get_mut::<Rotation>(body).unwrap().0 = rotation;
        app.update();
        let moved_probe =
            -(moved_position + rotation.as_dquat() * mount_offset + DVec3::Y * 1000.0).normalize()
                * 1.62;
        assert_eq!(
            app.world()
                .get::<LocalGravity>(probe)
                .unwrap()
                .acceleration(),
            moved_probe,
            "a parent native-rotation change did not invalidate its mounted probe"
        );
        assert_eq!(
            app.world()
                .get::<LocalGravity>(body)
                .unwrap()
                .acceleration(),
            moved
        );
        app.world_mut()
            .entity_mut(body)
            .remove::<lunco_physics::PhysicsPoseSeeded>();
        app.update();
        assert!(app.world().get::<LocalGravity>(body).is_none());
        assert!(app.world().get::<LocalGravity>(probe).is_none());
        app.world_mut()
            .entity_mut(body)
            .insert(lunco_physics::PhysicsPoseSeeded);
        app.update();
        assert_eq!(
            app.world()
                .get::<LocalGravity>(probe)
                .unwrap()
                .acceleration(),
            moved_probe
        );
        app.world_mut().entity_mut(probe).remove::<ChildOf>();
        app.update();
        assert!(app.world().get::<LocalGravity>(probe).is_none());
        app.world_mut().entity_mut(probe).insert(ChildOf(body));
        app.update();
        assert_eq!(
            app.world()
                .get::<LocalGravity>(probe)
                .unwrap()
                .acceleration(),
            moved_probe
        );
        app.world_mut().entity_mut(probe).remove::<Transform>();
        app.update();
        assert!(app.world().get::<LocalGravity>(probe).is_none());
        app.world_mut()
            .entity_mut(probe)
            .insert(Transform::from_translation(mount_offset.as_vec3()));
        app.update();
        assert_eq!(
            app.world()
                .get::<LocalGravity>(probe)
                .unwrap()
                .acceleration(),
            moved_probe
        );
        app.world_mut()
            .remove_resource::<lunco_spatial::ActivePhysicsFrame>();
        app.update();
        assert!(app.world().get::<LocalGravity>(body).is_none());
        assert!(app.world().get::<LocalGravity>(probe).is_none());
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(frame));
        app.update();
        assert_eq!(
            app.world()
                .get::<LocalGravity>(probe)
                .unwrap()
                .acceleration(),
            moved_probe
        );
        app.world_mut().entity_mut(body).remove::<GravityBody>();
        app.update();
        assert!(app.world().get::<LocalGravity>(body).is_none());
        app.world_mut().entity_mut(body).insert(GravityBody {
            body_entity: planet,
        });
        app.update();
        assert_eq!(
            app.world()
                .get::<LocalGravity>(body)
                .unwrap()
                .acceleration(),
            moved
        );

        // Changing the provider's model or actual active-frame pose is an
        // external field change even when the consumer also moves this tick.
        app.world_mut()
            .get_mut::<GravityProvider>(planet)
            .unwrap()
            .set_changed();
        app.world_mut().get_mut::<Position>(body).unwrap().0.x += 1.0;
        app.update();
        assert_eq!(
            app.world()
                .get::<LocalGravity>(body)
                .unwrap()
                .change_origin(),
            GravityChangeOrigin::Source
        );
        app.world_mut().get_mut::<Position>(body).unwrap().0.x += 1.0;
        app.update();
        assert_eq!(
            app.world()
                .get::<LocalGravity>(body)
                .unwrap()
                .change_origin(),
            GravityChangeOrigin::ConsumerPose
        );
        app.insert_resource(lunco_core_runtime::SimTick(1));
        app.world_mut()
            .entity_mut(planet)
            .insert(PrescribedSimulationPose::Sample {
                frame,
                tick: lunco_core_runtime::SimTick(1),
                position: GridPos(DVec3::new(1.0, -1000.0, 0.0)),
                rotation: lunco_spatial::coords::GridRot(DQuat::IDENTITY),
            });
        app.update();
        assert_eq!(
            app.world()
                .get::<LocalGravity>(body)
                .unwrap()
                .change_origin(),
            GravityChangeOrigin::Source
        );
        // A timestamp refresh with an identical physical pose is not source
        // motion. Consumer motion must still publish its exact new vector.
        app.insert_resource(lunco_core_runtime::SimTick(2));
        app.world_mut()
            .entity_mut(planet)
            .insert(PrescribedSimulationPose::Sample {
                frame,
                tick: lunco_core_runtime::SimTick(2),
                position: GridPos(DVec3::new(1.0, -1000.0, 0.0)),
                rotation: lunco_spatial::coords::GridRot(DQuat::IDENTITY),
            });
        app.world_mut().get_mut::<Position>(body).unwrap().0.x += 1.0;
        app.update();
        assert_eq!(
            app.world()
                .get::<LocalGravity>(body)
                .unwrap()
                .change_origin(),
            GravityChangeOrigin::ConsumerPose
        );
        app.insert_resource(lunco_core_runtime::SimTick(3));
        app.world_mut()
            .entity_mut(planet)
            .insert(PrescribedSimulationPose::Sample {
                frame,
                tick: lunco_core_runtime::SimTick(3),
                position: GridPos(DVec3::new(2.0, -1000.0, 0.0)),
                rotation: lunco_spatial::coords::GridRot(DQuat::IDENTITY),
            });
        app.update();
        assert_eq!(
            app.world()
                .get::<LocalGravity>(body)
                .unwrap()
                .change_origin(),
            GravityChangeOrigin::Source
        );
        assert!(
            !app.world()
                .resource::<GravityProviderPoseCache>()
                .0
                .is_empty()
        );
        lunco_core::run_scene_teardown(app.world_mut());
        assert!(
            app.world()
                .resource::<GravityProviderPoseCache>()
                .0
                .is_empty()
        );
    }

    #[test]
    fn flat_gravity_stays_in_the_active_physics_frame() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin));
        app.insert_resource(Gravity::flat(1.62, DVec3::NEG_Y));
        app.init_scene_resource::<GravityProviderPoseCache>();
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

        let actual = app
            .world()
            .get::<LocalGravity>(body)
            .expect("gravity is projected onto every spatial entity")
            .acceleration();
        assert!(actual.abs_diff_eq(DVec3::new(0.0, -1.62, 0.0), 1.0e-12));

        app.world_mut()
            .get_mut::<Transform>(body)
            .unwrap()
            .translation = Vec3::X;
        let admitted = app.world_mut().spawn(Transform::default()).id();
        app.update();
        for entity in [body, admitted] {
            assert_eq!(
                app.world()
                    .get::<LocalGravity>(entity)
                    .unwrap()
                    .acceleration(),
                actual
            );
        }

        app.world_mut().entity_mut(body).remove::<LocalGravity>();
        app.update();
        assert_eq!(
            app.world()
                .get::<LocalGravity>(body)
                .unwrap()
                .acceleration(),
            actual
        );

        app.insert_resource(Gravity::flat(2.3, DVec3::X));
        app.update();
        for entity in [body, admitted] {
            assert_eq!(
                app.world()
                    .get::<LocalGravity>(entity)
                    .unwrap()
                    .acceleration(),
                DVec3::X * 2.3
            );
        }

        app.insert_resource(Gravity::surface());
        app.update();
        for entity in [body, admitted] {
            assert!(
                app.world().get::<LocalGravity>(entity).is_none(),
                "unresolved surface state retained flat gravity"
            );
        }
        app.insert_resource(Gravity::flat(1.62, DVec3::NEG_Y));
        app.update();
        for entity in [body, admitted] {
            assert_eq!(
                app.world()
                    .get::<LocalGravity>(entity)
                    .unwrap()
                    .acceleration(),
                actual
            );
        }
    }

    #[test]
    fn stable_gravity_outputs_do_not_redirty_sim_component() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SimComponentChangeCount>()
            .add_systems(
                Update,
                (inject_local_gravity_into_cosim, count_sim_component_changes).chain(),
            );
        let entity = app
            .world_mut()
            .spawn((
                EnvironmentProbe,
                LocalGravity::new(DVec3::new(0.0, -1.62, 0.0)),
                lunco_cosim_core::SimComponent::default(),
            ))
            .id();

        app.update();
        let initial = app
            .world()
            .get::<lunco_cosim_core::SimComponent>(entity)
            .unwrap();
        let initial_magnitude = initial
            .outputs
            .get(lunco_cosim_core::GRAVITY_SOURCE_CONNECTOR)
            .copied()
            .unwrap();
        assert!((initial_magnitude - 1.62).abs() < 1.0e-12);
        app.world_mut().resource_mut::<SimComponentChangeCount>().0 = 0;

        app.update();
        assert_eq!(
            app.world().resource::<SimComponentChangeCount>().0,
            0,
            "identical gravity values must not invalidate the shared component"
        );

        *app.world_mut().get_mut::<LocalGravity>(entity).unwrap() =
            LocalGravity::new(DVec3::new(0.0, -1.63, 0.0));
        app.update();
        assert_eq!(
            app.world().resource::<SimComponentChangeCount>().0,
            1,
            "a changed gravity sample must still invalidate the shared component"
        );
    }
}
