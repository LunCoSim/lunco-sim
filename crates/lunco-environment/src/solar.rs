//! Solar rendering projection for the generic environment direction system.
//!
//! Modelica direction inputs are published by the generic probe system. It
//! resolves each named point target or framed ray in the consumer's own frame.
//! This module applies the same resolved `sun` direction to the render light;
//! [`SunState`] supplies only calibrated irradiance.

use bevy::{math::DQuat, prelude::*};

use crate::Earthshine;

/// Semantic sun state produced by the selected physical/provider model.
///
/// Direction belongs to [`crate::EnvironmentDirections`], shared with every
/// other framed direction source. This resource holds only solar irradiance
/// used by the render light.
#[derive(Resource, Debug, Clone, PartialEq, Default)]
pub struct SunState {
    /// Optional calibrated direct-sun irradiance in lux.
    pub irradiance_lux: Option<f32>,
    /// Monotonic semantic revision for change-gated projections.
    pub revision: u64,
}

impl SunState {
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
        if self.irradiance_lux.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Change only calibrated irradiance. Direction is resolved independently
    /// from the shared direction-target system.
    pub fn set_irradiance(&mut self, irradiance_lux: Option<f32>) -> bool {
        if irradiance_lux.is_some_and(|lux| !lux.is_finite() || lux < 0.0) {
            return false;
        }
        if self.irradiance_lux != irradiance_lux {
            self.irradiance_lux = irradiance_lux;
            self.revision = self.revision.wrapping_add(1);
        }
        true
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
    sun_revision: Option<u64>,
    direction_revision: Option<u64>,
    frame_rotation: Option<DQuat>,
    parent_rotation: Option<DQuat>,
    initialized: bool,
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
            )>,
        ),
    >,
    mut projection_cache: Local<SunProjectionCache>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let active_scene = mount
        .as_deref()
        .is_some_and(|mount| mount.active_root().is_some());
    let mut suns = q_sun.iter_mut();
    let Some((scene_sun, mut transform, mut light, parent)) = suns.next() else {
        if !active_scene {
            return;
        }
        *projection_cache = SunProjectionCache::default();
        replace_sun_diagnostic(
            &mut diagnostics,
            Some(lunco_core::RuntimeDiagnostic {
                code: "sun-contract".to_string(),
                severity: lunco_core::DiagnosticSeverity::Error,
                producer: "environment-sun".to_string(),
                subject: "scene-sun".to_string(),
                message:
                    "active scene has no unscoped scene sun; author exactly one UsdLux DistantLight"
                        .to_string(),
            }),
        );
        return;
    };
    if suns.next().is_some() {
        *projection_cache = SunProjectionCache::default();
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "sun-contract".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "scene-sun".to_string(),
                    message:
                        "active scene has multiple unscoped scene suns; author exactly one UsdLux DistantLight"
                            .to_string(),
                }),
            );
        }
        return;
    }
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
        &q_parents,
        &q_grids,
        &q_spatial,
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
    let sun_revision = sun.as_deref().map(|state| state.revision);
    let direction_revision = directions.as_deref().map(|state| state.revision());
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
        || projection_cache.sun_revision != sun_revision
        || projection_cache.direction_revision != direction_revision
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
    if transform.forward().angle_between(emit_direction) > 2.0e-5 {
        transform.look_to(emit_direction, up);
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
        sun_revision,
        direction_revision,
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
    q_sun: Query<
        (Entity, Option<&ChildOf>),
        (
            With<bevy::light::DirectionalLight>,
            Without<Earthshine>,
            Without<bevy::camera::visibility::RenderLayers>,
        ),
    >,
    q_targets: Query<(Entity, &crate::DirectionTargetId)>,
    changed_targets: Query<
        (),
        (
            With<crate::DirectionTargetId>,
            Or<(
                Added<crate::DirectionTargetId>,
                Changed<crate::DirectionTargetId>,
            )>,
        ),
    >,
    changed_suns: Query<
        (),
        (
            With<bevy::light::DirectionalLight>,
            Without<Earthshine>,
            Without<bevy::camera::visibility::RenderLayers>,
            Or<(
                Added<bevy::light::DirectionalLight>,
                Added<ChildOf>,
                Changed<ChildOf>,
                Changed<big_space::prelude::CellCoord>,
            )>,
        ),
    >,
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
            )>,
        ),
    >,
) -> bool {
    if sun.is_some_and(|state| state.is_changed())
        || directions
            .as_ref()
            .is_some_and(|directions| directions.is_changed())
        || mount.is_some_and(|state| state.is_changed())
        || active_frame
            .as_ref()
            .is_some_and(|state| state.is_changed())
        || !changed_suns.is_empty()
        || !changed_targets.is_empty()
    {
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
        || q_sun
            .iter()
            .any(|(_, parent)| parent.is_some_and(|parent| changed_pose(parent.parent())))
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
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    q_frames: Query<&GlobalTransform>,
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
    let Ok(sun_gt) = q_sun.single() else {
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
        &q_parents,
        &q_grids,
        &q_spatial,
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
pub fn sun_render_finalize_needed(
    directions: Option<Res<crate::EnvironmentDirections>>,
    q_sun: Query<
        (),
        (
            With<bevy::light::DirectionalLight>,
            Without<Earthshine>,
            Without<bevy::camera::visibility::RenderLayers>,
            Or<(Added<GlobalTransform>, Changed<GlobalTransform>)>,
        ),
    >,
) -> bool {
    directions.is_some_and(|state| state.is_changed()) || !q_sun.is_empty()
}
