//! Solar rendering projection for the generic environment direction system.
//!
//! Modelica direction inputs are published by the generic probe system. It
//! resolves each named point target or framed ray in the consumer's own frame.
//! This module applies the same resolved `sun` direction to the render light;
//! [`SunState`] supplies only calibrated irradiance.

use bevy::{math::DQuat, prelude::*};

use crate::Earthshine;

/// Resolved USD photometry before environment and render overrides.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct AuthoredDirectionalLight {
    pub illuminance_lux: f32,
}

/// Directional light identities for the primary scene. Asset previews and lights
/// scoped to a render layer have separate consumers.
#[derive(bevy::ecs::system::SystemParam)]
pub struct MountedDirectionalLights<'w, 's> {
    mount: Option<Res<'w, lunco_core::SceneMountState>>,
    parents: Query<'w, 's, &'static ChildOf>,
    lights: Query<
        'w,
        's,
        (Entity, Has<Earthshine>),
        (
            With<AuthoredDirectionalLight>,
            With<bevy::light::DirectionalLight>,
            Without<bevy::camera::visibility::RenderLayers>,
            Without<lunco_render::EditorPreviewLight>,
        ),
    >,
}

impl MountedDirectionalLights<'_, '_> {
    pub(crate) fn membership(&self, earthshine: bool) -> SourceMembership {
        let Some(mount) = self.mount.as_deref() else {
            return SourceMembership::Missing;
        };
        let Some(root) = mount.active_root() else {
            return SourceMembership::Missing;
        };
        source_membership(self.lights.iter().filter_map(|(entity, fill)| {
            (fill == earthshine
                && std::iter::once(entity)
                    .chain(self.parents.iter_ancestors(entity))
                    .find(|candidate| mount.contains_root(*candidate))
                    == Some(root))
            .then_some(entity)
        }))
    }

    pub fn sun(&self) -> Result<Entity, &'static str> {
        match self.membership(false) {
            SourceMembership::One(entity) => Ok(entity),
            SourceMembership::Missing => Err("active scene has no unscoped authored Sun"),
            SourceMembership::Multiple => Err("active scene has multiple unscoped authored Suns"),
        }
    }

    pub fn earthshine(&self) -> Result<Entity, &'static str> {
        match self.membership(true) {
            SourceMembership::One(entity) => Ok(entity),
            SourceMembership::Missing => Err("active scene has no unscoped earthshine light"),
            SourceMembership::Multiple => {
                Err("active scene has multiple unscoped earthshine lights")
            }
        }
    }
}

/// Semantic sun state produced by the selected physical/provider model.
///
/// Direction belongs to [`crate::EnvironmentDirections`], shared with every
/// other framed direction source. This resource holds separate physical
/// irradiance and rendering illuminance calibrations.
#[derive(Resource, Debug, Clone, PartialEq, Default)]
pub struct SunState {
    /// Optional calibrated direct-sun irradiance in lux.
    pub irradiance_lux: Option<f32>,
    /// Physical direct-normal solar irradiance in W/m². Rendering illuminance
    /// is a separate quantity; no lux-to-watt conversion enters cosimulation.
    pub irradiance_w_m2: Option<f64>,
    /// Monotonic semantic revision for change-gated projections.
    pub revision: u64,
}

impl SunState {
    /// Publish the physical source sample, independently of render calibration.
    pub fn publish_physical(&mut self, sample: Option<f64>) -> Result<bool, &'static str> {
        if sample.is_some_and(|value| !value.is_finite() || value < 0.0) {
            self.irradiance_w_m2 = None;
            self.revision = self.revision.wrapping_add(1);
            return Err("physical solar irradiance must be finite and non-negative");
        }
        if self.irradiance_w_m2 != sample {
            self.irradiance_w_m2 = sample;
            self.revision = self.revision.wrapping_add(1);
            return Ok(true);
        }
        Ok(false)
    }
    /// Publish irradiance and advance the revision only when it changes.
    pub fn publish(&mut self, irradiance_lux: Option<f32>) -> bool {
        if irradiance_lux.is_some_and(|lux| !lux.is_finite() || lux < 0.0) {
            return false;
        }
        if self.irradiance_lux != irradiance_lux {
            self.irradiance_lux = irradiance_lux;
            self.revision = self.revision.wrapping_add(1);
        }
        true
    }

    /// Remove the current provider sample rather than retaining stale lighting.
    pub fn clear(&mut self) {
        let _ = self.publish_physical(None);
        if self.irradiance_lux.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }
}

/// Fixed directional sources use the physical calibration. Celestial sources
/// are exclusively published by the ephemeris provider, including its 1/r² law.
pub fn publish_fixed_solar_irradiance(
    classification: Query<&crate::CelestialSourceClassification>,
    directions: Res<crate::EnvironmentDirections>,
    calibration: Res<crate::LunarSun>,
    mut sun: ResMut<SunState>,
) {
    match classification.single() {
        Ok(source) if source.has_source => {}
        Ok(_) => {
            let sample = directions
                .get_named(crate::SUN_DIRECTION_SOURCE)
                .map(|_| calibration.irradiance_w_m2);
            match sun.bypass_change_detection().publish_physical(sample) {
                Ok(true) => sun.set_changed(),
                Ok(false) => {}
                Err(error) => {
                    warn!("[solar] {error}");
                    sun.set_changed();
                }
            }
        }
        Err(_) => {
            if sun.bypass_change_detection().publish_physical(None) == Ok(true) {
                sun.set_changed();
            }
        }
    }
}

/// Render-facing snapshot of the finalized scene-sun direction.
///
/// The environment boundary resolves the generic `sun` source in the active
/// physics frame and projects it into the light's local pose before BigSpace
/// propagation. This resource is then published from that light's finalized
/// `GlobalTransform`, so horizon baking and shader wiring consume the same
/// render-space direction as Bevy's shadow extractor. It is never a provider
/// input.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Default)]
pub struct SunRenderState {
    /// Unit direction toward the Sun in the canonical render/world frame.
    pub direction_to_sun_world: Option<Vec3>,
    pub revision: u64,
}

impl SunRenderState {
    fn publish(&mut self, direction_to_sun_world: Vec3) -> bool {
        if self.direction_to_sun_world != Some(direction_to_sun_world) {
            self.direction_to_sun_world = Some(direction_to_sun_world);
            self.revision = self.revision.wrapping_add(1);
            true
        } else {
            false
        }
    }

    pub(crate) fn clear(&mut self) -> bool {
        if self.direction_to_sun_world.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
            true
        } else {
            false
        }
    }
}

/// Project the generic `sun` source and calibrated irradiance onto the unique
/// unscoped render sun's local pose.
///
/// This is the only system that writes the render light's direction from
/// semantic sun state. It runs before BigSpace transform propagation so the
/// finalized light `GlobalTransform` and every shadow consumer describe the
/// same render epoch. Zero or multiple candidate lights is a contract error
/// for the render host; no arbitrary light is selected.
fn replace_sun_diagnostic(
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    finding: Option<lunco_core::RuntimeDiagnostic>,
) {
    if let Some(diagnostics) = diagnostics.as_deref_mut() {
        diagnostics.replace_producer(
            "environment-sun",
            finding
                .into_iter()
                .chain(std::iter::empty::<lunco_core::RuntimeDiagnostic>()),
        );
    }
}

#[derive(Default)]
#[doc(hidden)]
pub struct SunProjectionCache {
    scene_sun: Option<Entity>,
    sun_parent: Option<Entity>,
    source_frame: Option<Entity>,
    render_inputs: SunRenderInputs,
    frame_rotation: Option<DQuat>,
    parent_rotation: Option<DQuat>,
    initialized: bool,
}

/// Only the calibrated illuminance and exact named ray feed this render
/// projection. Physical irradiance and other direction sources have separate
/// consumers and must not invalidate the scene light.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct SunRenderInputs {
    illuminance_bits: Option<u32>,
    ray: Option<crate::FramedDirection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceMembership {
    Missing,
    One(Entity),
    Multiple,
}

impl SourceMembership {
    fn entity(self) -> Option<Entity> {
        if let Self::One(entity) = self {
            Some(entity)
        } else {
            None
        }
    }
}

fn source_membership(mut entities: impl Iterator<Item = Entity>) -> SourceMembership {
    match entities.next() {
        None => SourceMembership::Missing,
        Some(entity) if entities.next().is_none() => SourceMembership::One(entity),
        Some(_) => SourceMembership::Multiple,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SunProjectionGateInputs {
    render: SunRenderInputs,
    scene: Option<Entity>,
    frame: Option<Entity>,
    light: SourceMembership,
    light_values: Option<(Option<Entity>, Quat, u32)>,
    target: SourceMembership,
}

impl SunRenderInputs {
    fn capture(sun: Option<&SunState>, directions: Option<&crate::EnvironmentDirections>) -> Self {
        Self {
            illuminance_bits: sun.and_then(|state| state.irradiance_lux).map(f32::to_bits),
            ray: directions.and_then(|state| state.get_named(crate::SUN_DIRECTION_SOURCE)),
        }
    }
}

fn spatial_rotation(
    entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&big_space::prelude::Grid>,
    q_spatial: &Query<
        (Option<&big_space::prelude::CellCoord>, &Transform),
        Without<bevy::light::DirectionalLight>,
    >,
) -> Result<DQuat, lunco_spatial::coords::CoordinateError> {
    lunco_spatial::coords::world_pose(entity, q_parents, q_grids, q_spatial)
        .map(|(_, rotation)| rotation.0)
}

pub fn project_environment_sun_to_light(
    poses: lunco_physics::SimulationPoseQuery<Without<bevy::light::DirectionalLight>>,
    sun: Option<Res<SunState>>,
    directions: Option<Res<crate::EnvironmentDirections>>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    mount: Option<Res<lunco_core::SceneMountState>>,
    q_targets: Query<(Entity, &crate::DirectionTargetId)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_spatial: Query<
        (Option<&big_space::prelude::CellCoord>, &Transform),
        Without<bevy::light::DirectionalLight>,
    >,
    mounted_lights: MountedDirectionalLights,
    mut q_sun: Query<
        (
            Entity,
            &mut Transform,
            &mut bevy::light::DirectionalLight,
            Option<&ChildOf>,
        ),
        (
            Without<Earthshine>,
            Without<bevy::camera::visibility::RenderLayers>,
        ),
    >,
    q_changed: Query<
        (),
        (
            Without<bevy::light::DirectionalLight>,
            Or<(
                Changed<Transform>,
                Changed<big_space::prelude::CellCoord>,
                Changed<ChildOf>,
                Changed<big_space::prelude::Grid>,
                Changed<lunco_physics::PrescribedSimulationPose>,
            )>,
        ),
    >,
    mut projection_cache: Local<SunProjectionCache>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let active_scene = mount
        .as_deref()
        .is_some_and(|mount| mount.active_root().is_some());
    let (scene_sun, mut transform, mut light, parent) = match mounted_lights.sun() {
        Ok(entity) => q_sun
            .get_mut(entity)
            .expect("mounted Sun matches the render query"),
        Err(detail) => {
            *projection_cache = SunProjectionCache::default();
            if active_scene {
                replace_sun_diagnostic(
                    &mut diagnostics,
                    Some(lunco_core::RuntimeDiagnostic {
                        code: "sun-contract".into(),
                        severity: lunco_core::DiagnosticSeverity::Error,
                        producer: "environment-sun".into(),
                        subject: "scene-sun".into(),
                        message: detail.into(),
                    }),
                );
            }
            return;
        }
    };
    let mut diagnostics = diagnostics;
    replace_sun_diagnostic(&mut diagnostics, None);
    let Some(source_frame) = active_frame.as_deref().map(|frame| frame.0) else {
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "physics-frame".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "scene-sun".to_string(),
                    message: "active scene has no ActivePhysicsFrame for direction resolution"
                        .to_string(),
                }),
            );
        }
        return;
    };
    let direction = match crate::resolve_direction_for_frame(
        &crate::DirectionSourceId::parse(crate::SUN_DIRECTION_SOURCE)
            .expect("built-in direction source id is valid"),
        source_frame,
        directions.as_deref(),
        &q_targets,
        &poses,
    ) {
        Ok(direction) => direction,
        Err(error) => {
            if active_scene {
                replace_sun_diagnostic(
                    &mut diagnostics,
                    Some(lunco_core::RuntimeDiagnostic {
                        code: "sun-state".to_string(),
                        severity: lunco_core::DiagnosticSeverity::Error,
                        producer: "environment-sun".to_string(),
                        subject: "sun".to_string(),
                        message: format!("Sun direction cannot be resolved: {error:?}"),
                    }),
                );
            }
            return;
        }
    };
    if sun
        .as_deref()
        .and_then(|state| state.irradiance_lux)
        .is_some_and(|lux| !lux.is_finite() || lux < 0.0)
    {
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "sun-state".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "semantic-sun".to_string(),
                    message: "semantic SunState irradiance is non-finite or negative".to_string(),
                }),
            );
        }
        return;
    }
    let sun_parent = parent.map(ChildOf::parent);
    let render_inputs = SunRenderInputs::capture(sun.as_deref(), directions.as_deref());
    let frame_changed = !projection_cache.initialized
        || projection_cache.source_frame != Some(source_frame)
        || lunco_spatial::coords::world_pose_changed(source_frame, &q_parents, &q_changed);
    let parent_changed = !projection_cache.initialized
        || projection_cache.sun_parent != sun_parent
        || sun_parent.is_some_and(|parent| {
            lunco_spatial::coords::world_pose_changed(parent, &q_parents, &q_changed)
        });
    let sun_changed = !projection_cache.initialized
        || projection_cache.scene_sun != Some(scene_sun)
        || projection_cache.render_inputs != render_inputs
        || transform.is_changed()
        || light.is_changed()
        || q_targets.iter().any(|(entity, id)| {
            id.as_str() == crate::SUN_DIRECTION_SOURCE
                && lunco_spatial::coords::world_pose_changed(entity, &q_parents, &q_changed)
        });
    if !(frame_changed || parent_changed || sun_changed) {
        return;
    }
    let frame_rotation = match (!frame_changed)
        .then_some(projection_cache.frame_rotation)
        .flatten()
    {
        Some(rotation) => rotation,
        None => match spatial_rotation(source_frame, &q_parents, &q_grids, &q_spatial) {
            Ok(rotation) => rotation,
            Err(_) => {
                if active_scene {
                    replace_sun_diagnostic(
                        &mut diagnostics,
                        Some(lunco_core::RuntimeDiagnostic {
                            code: "physics-frame".to_string(),
                            severity: lunco_core::DiagnosticSeverity::Error,
                            producer: "environment-sun".to_string(),
                            subject: "semantic-sun".to_string(),
                            message: format!(
                                "direction source frame {:?} has no complete BigSpace pose",
                                source_frame
                            ),
                        }),
                    );
                }
                return;
            }
        },
    };
    let parent_rotation = match sun_parent {
        None => None,
        Some(parent) => {
            let cached_rotation = (!parent_changed)
                .then_some(projection_cache.parent_rotation)
                .flatten();
            match cached_rotation
                .map(Ok)
                .unwrap_or_else(|| spatial_rotation(parent, &q_parents, &q_grids, &q_spatial))
            {
                Ok(rotation) => Some(rotation),
                Err(_) => {
                    replace_sun_diagnostic(
                        &mut diagnostics,
                        Some(lunco_core::RuntimeDiagnostic {
                            code: "sun-parent".to_string(),
                            severity: lunco_core::DiagnosticSeverity::Error,
                            producer: "environment-sun".to_string(),
                            subject: "scene-sun".to_string(),
                            message: "scene sun's parent has no complete BigSpace pose".to_string(),
                        }),
                    );
                    return;
                }
            }
        }
    };
    let Some(direction_to_sun_world) = direction.rotated(frame_rotation) else {
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "sun-state".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "semantic-sun".to_string(),
                    message: "sun direction failed its source-frame rotation contract".to_string(),
                }),
            );
        }
        return;
    };
    // A root light has no parent-local rotation: its Transform is already in
    // the world frame. A child light must use the live parent projection above.
    let emit_direction = match parent_rotation {
        Some(rotation) => (rotation.inverse() * -direction_to_sun_world.components()).as_vec3(),
        None => (-direction_to_sun_world.components()).as_vec3(),
    };
    let up = if emit_direction.dot(Vec3::Y).abs() > 0.99 {
        Vec3::X
    } else {
        Vec3::Y
    };
    // Compare rendered rotations directly: a tiny source change can cross the
    // horizon even when a float direction dot product rounds to one.
    let rotation = Transform::IDENTITY.looking_to(emit_direction, up).rotation;
    if transform.rotation != rotation {
        transform.rotation = rotation;
    }
    if let Some(irradiance) = sun.as_deref().and_then(|state| state.irradiance_lux) {
        if (light.illuminance - irradiance).abs() > irradiance.abs().max(1.0) * 5.0e-3 {
            light.illuminance = irradiance;
        }
    }
    *projection_cache = SunProjectionCache {
        scene_sun: Some(scene_sun),
        sun_parent,
        source_frame: Some(source_frame),
        render_inputs,
        frame_rotation: Some(frame_rotation),
        parent_rotation,
        initialized: true,
    };
}

/// Build the change gate for the Bevy scene-light projection. The light is
/// updated when its direction source, irradiance, physics frame, or spatial
/// ancestry changes.
pub fn tracked_sun_light_projection() -> impl bevy::ecs::schedule::SystemCondition<()> {
    lunco_core_runtime::gate::tracked(
        "environment_sun_light_projection",
        sun_light_projection_needed,
    )
}

fn sun_light_projection_needed(
    sun: Option<Res<SunState>>,
    directions: Option<Res<crate::EnvironmentDirections>>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    mount: Option<Res<lunco_core::SceneMountState>>,
    mounted_lights: MountedDirectionalLights,
    q_sun: Query<
        (
            Entity,
            &Transform,
            &bevy::light::DirectionalLight,
            Option<&ChildOf>,
        ),
        (
            Without<Earthshine>,
            Without<bevy::camera::visibility::RenderLayers>,
        ),
    >,
    q_targets: Query<(Entity, &crate::DirectionTargetId)>,
    q_parents: Query<&ChildOf>,
    changed_spatial: Query<
        (),
        (
            Without<bevy::light::DirectionalLight>,
            Or<(
                Changed<Transform>,
                Changed<big_space::prelude::CellCoord>,
                Changed<ChildOf>,
                Changed<big_space::prelude::Grid>,
                Changed<lunco_physics::PrescribedSimulationPose>,
            )>,
        ),
    >,
    mut previous_inputs: Local<Option<SunProjectionGateInputs>>,
) -> bool {
    // Bevy may mark a component changed even when a writer stores the same
    // value. Only the exact fields read by the sun projection are inputs;
    // its own writes must not sustain a self-invalidating condition.
    let membership = mounted_lights.membership(false);
    let scene_light = membership
        .entity()
        .and_then(|entity| q_sun.get(entity).ok());
    let inputs = SunProjectionGateInputs {
        render: SunRenderInputs::capture(sun.as_deref(), directions.as_deref()),
        scene: mount.as_deref().and_then(|state| state.active_root()),
        frame: active_frame.as_deref().map(|frame| frame.0),
        light: membership,
        light_values: scene_light.map(|(_, transform, light, parent)| {
            (
                parent.map(ChildOf::parent),
                transform.rotation,
                light.illuminance.to_bits(),
            )
        }),
        target: source_membership(q_targets.iter().filter_map(|(entity, target)| {
            (target.as_str() == crate::SUN_DIRECTION_SOURCE).then_some(entity)
        })),
    };
    let inputs_changed = previous_inputs.replace(inputs) != Some(inputs);
    if inputs_changed {
        return true;
    }

    let changed_pose =
        |entity| lunco_spatial::coords::world_pose_changed(entity, &q_parents, &changed_spatial);
    active_frame
        .as_deref()
        .is_some_and(|frame| changed_pose(frame.0))
        || directions
            .as_deref()
            .and_then(|directions| directions.get_named(crate::SUN_DIRECTION_SOURCE))
            .is_some_and(|sample| changed_pose(sample.frame))
        || q_targets.iter().any(|(entity, target)| {
            target.as_str() == crate::SUN_DIRECTION_SOURCE && changed_pose(entity)
        })
        || scene_light.is_some_and(|(_, _, _, parent)| {
            parent.is_some_and(|parent| changed_pose(parent.parent()))
        })
}

/// Keep the last committed render-sun sample while a scene transaction is
/// still assembling its transforms. Scene teardown clears the resource at the
/// ownership boundary; during a load, an incomplete frame is therefore a
/// pending presentation product rather than a new "black" sun.
fn clear_render_sun_if_scene_is_idle(
    render_state: &mut ResMut<SunRenderState>,
    coordinator: Option<&lunco_core::SceneTransitionCoordinator>,
) {
    if coordinator.is_none_or(|coordinator| coordinator.active().is_none())
        && render_state.bypass_change_detection().clear()
    {
        render_state.set_changed();
    }
}

/// Publish the render sun only after BigSpace has finalized the scene light's
/// `GlobalTransform`.
///
/// The semantic provider and the light-local projection are intentionally
/// separate from this render snapshot. Publishing the pre-propagation
/// direction would allow terrain materials and the shadow map to observe
/// different floating-origin render epochs during recentering.
pub fn finalize_sun_render_state(
    directions: Option<Res<crate::EnvironmentDirections>>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    q_targets: Query<(Entity, &crate::DirectionTargetId)>,
    poses: lunco_physics::SimulationPoseQuery,
    q_frames: Query<&GlobalTransform>,
    mounted_lights: MountedDirectionalLights,
    q_sun: Query<
        &GlobalTransform,
        (
            With<bevy::light::DirectionalLight>,
            Without<Earthshine>,
            Without<bevy::camera::visibility::RenderLayers>,
        ),
    >,
    mut render_state: ResMut<SunRenderState>,
    coordinator: Option<Res<lunco_core::SceneTransitionCoordinator>>,
    diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let Some(frame) = active_frame.as_deref().map(|frame| frame.0) else {
        clear_render_sun_if_scene_is_idle(&mut render_state, coordinator.as_deref());
        return;
    };
    let Ok(frame_gt) = q_frames.get(frame) else {
        clear_render_sun_if_scene_is_idle(&mut render_state, coordinator.as_deref());
        return;
    };
    let Some(sun_gt) = mounted_lights
        .sun()
        .ok()
        .and_then(|entity| q_sun.get(entity).ok())
    else {
        clear_render_sun_if_scene_is_idle(&mut render_state, coordinator.as_deref());
        return;
    };

    let source_id = crate::DirectionSourceId::parse(crate::SUN_DIRECTION_SOURCE)
        .expect("built-in direction source id is valid");
    let expected_direction = crate::resolve_direction_for_frame(
        &source_id,
        frame,
        directions.as_deref(),
        &q_targets,
        &poses,
    );
    let Ok(expected_direction) = expected_direction else {
        clear_render_sun_if_scene_is_idle(&mut render_state, coordinator.as_deref());
        return;
    };
    // This is an explicit render comparison: canonical direction math stays
    // f64, and GlobalTransform/Vec3 is the rendering boundary.
    let expected =
        (frame_gt.rotation() * expected_direction.components().as_vec3()).normalize_or_zero();
    let actual = -(sun_gt.rotation() * Vec3::NEG_Z).normalize_or_zero();
    if expected.length_squared() < 0.5
        || actual.length_squared() < 0.5
        || !expected.is_finite()
        || !actual.is_finite()
        || (expected - actual).length() > 1.0e-3
    {
        if let Some(mut diagnostics) = diagnostics {
            diagnostics.replace_producer(
                "environment-sun-render",
                [lunco_core::RuntimeDiagnostic {
                    code: "sun-render-frame".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun-render".to_string(),
                    subject: "scene-sun".to_string(),
                    message: "finalized scene sun pose disagrees with the active physics frame"
                        .to_string(),
                }],
            );
        }
        clear_render_sun_if_scene_is_idle(&mut render_state, coordinator.as_deref());
        return;
    }

    if let Some(mut diagnostics) = diagnostics {
        diagnostics.replace_producer("environment-sun-render", std::iter::empty());
    }
    if render_state.bypass_change_detection().publish(actual) {
        render_state.set_changed();
    }
}

/// Admit the finalized-light projection only when its semantic source or the
/// propagated scene-sun transform changes.
pub(crate) fn sun_render_finalize_needed(
    directions: Option<Res<crate::EnvironmentDirections>>,
    mounted_lights: MountedDirectionalLights,
    q_sun: Query<
        (Entity, Ref<GlobalTransform>),
        (
            With<bevy::light::DirectionalLight>,
            Without<Earthshine>,
            Without<bevy::camera::visibility::RenderLayers>,
        ),
    >,
    mut previous: Local<Option<(Option<crate::FramedDirection>, SourceMembership)>>,
) -> bool {
    let ray = directions
        .as_deref()
        .and_then(|state| state.get_named(crate::SUN_DIRECTION_SOURCE));
    let key = (ray, mounted_lights.membership(false));
    previous.replace(key) != Some(key)
        || key
            .1
            .entity()
            .and_then(|entity| q_sun.get(entity).ok())
            .is_some_and(|(_, transform)| transform.is_changed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct ProjectionRuns(usize);

    #[test]
    fn projection_gate_checks_light_values_not_equal_value_change_ticks() {
        let mut app = App::new();
        app.init_resource::<ProjectionRuns>().add_systems(
            Update,
            (|mut runs: ResMut<ProjectionRuns>| runs.0 += 1).run_if(sun_light_projection_needed),
        );
        let root = app.world_mut().spawn_empty().id();
        app.init_resource::<lunco_core::SceneMountState>()
            .world_mut()
            .resource_mut::<lunco_core::SceneMountState>()
            .register_root(root, true);
        let light = app
            .world_mut()
            .spawn((
                ChildOf(root),
                AuthoredDirectionalLight {
                    illuminance_lux: 100_000.0,
                },
                Transform::IDENTITY,
                bevy::light::DirectionalLight::default(),
            ))
            .id();
        app.update();
        assert_eq!(app.world().resource::<ProjectionRuns>().0, 1);
        let original = app
            .world()
            .get::<bevy::light::DirectionalLight>(light)
            .unwrap()
            .illuminance;
        app.world_mut()
            .get_mut::<bevy::light::DirectionalLight>(light)
            .unwrap()
            .illuminance = original;
        app.world_mut()
            .get_mut::<Transform>(light)
            .unwrap()
            .rotation = Quat::IDENTITY;
        app.update();
        assert_eq!(app.world().resource::<ProjectionRuns>().0, 1);
        app.world_mut()
            .get_mut::<Transform>(light)
            .unwrap()
            .rotation = Quat::from_rotation_x(0.2);
        app.update();
        assert_eq!(app.world().resource::<ProjectionRuns>().0, 2);
        app.update();
        assert_eq!(app.world().resource::<ProjectionRuns>().0, 2);
        app.world_mut().despawn(light);
        app.update();
        assert_eq!(app.world().resource::<ProjectionRuns>().0, 3);
    }

    #[test]
    fn render_projection_key_ignores_physical_irradiance_and_unrelated_rays() {
        let mut sun = SunState::default();
        let mut directions = crate::EnvironmentDirections::default();
        let before = SunRenderInputs::capture(Some(&sun), Some(&directions));
        sun.publish_physical(Some(1361.0)).unwrap();
        let ray = crate::FramedDirection {
            frame: Entity::PLACEHOLDER,
            direction: lunco_spatial::coords::UnitDirection3::from_unit(bevy::math::DVec3::X)
                .unwrap(),
        };
        directions.set_named(crate::EARTH_DIRECTION_SOURCE, Some(ray));
        assert_eq!(
            before,
            SunRenderInputs::capture(Some(&sun), Some(&directions))
        );
        directions.set_named(crate::SUN_DIRECTION_SOURCE, Some(ray));
        let after_ray = SunRenderInputs::capture(Some(&sun), Some(&directions));
        assert_ne!(before, after_ray);
        assert!(sun.publish(Some(100_000.0)));
        assert_ne!(
            after_ray,
            SunRenderInputs::capture(Some(&sun), Some(&directions))
        );
        assert_eq!(
            source_membership(std::iter::empty()),
            SourceMembership::Missing
        );
        assert_eq!(
            source_membership(std::iter::once(ray.frame)),
            SourceMembership::One(ray.frame)
        );
        assert_eq!(
            source_membership([ray.frame, ray.frame].into_iter()),
            SourceMembership::Multiple
        );
    }
}
