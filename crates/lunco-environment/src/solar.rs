//! Solar environment domain — the sun's direction as a co-simulation source.
//!
//! The lighting analog of the gravity bridge. Semantic [`SunState`] is the
//! provider contract; the render `DirectionalLight` is only its projection.
//! This module projects the semantic direction directly into the co-sim graph
//! as ordinary `SimComponent` **outputs**, so a sun-tracking model receives it
//! through a plain output→input wire — the ontology's
//! `RadiationProvider → solar model` pipeline.
//!
//! Values are published on explicit [`crate::EnvironmentProbe`] source prims.
//! Models consume them through ordinary USD connections, so provider and
//! consumer remain distinct graph nodes.
//!
//! ## Provider note
//!
//! There is no separate `SolarProvider` component: [`SunState`] is the provider
//! contract (its direction is published by ephemeris or an explicit command).
//! Modelica reads the latest celestial sample at ordinary fixed-step
//! communication points; the frame conversion and output write happen together
//! before cosim propagation so there is no intermediate cache to go stale.

use bevy::{math::DQuat, prelude::*};

use crate::Earthshine;
use lunco_cosim_core::{SUN_MOUNT_X_CONNECTOR, SUN_MOUNT_Y_CONNECTOR, SUN_MOUNT_Z_CONNECTOR};

/// Semantic sun state produced by the selected physical/provider model.
///
/// Render lights and co-simulation ports are projections of this resource;
/// neither is read back as a source of truth. `None` means the provider has not
/// produced a valid direction for the current scene/epoch.
#[derive(Resource, Debug, Clone, PartialEq, Default)]
pub struct SunState {
    /// Unit direction from the observed site toward the sun, in the active
    /// site's ENU axes. Consumers must project it through the bound active
    /// frame before treating it as a world/render direction.
    pub direction_to_sun: Option<Vec3>,
    /// Optional calibrated direct-sun irradiance in lux.
    pub irradiance_lux: Option<f32>,
    /// Monotonic semantic revision for change-gated projections.
    pub revision: u64,
}

impl SunState {
    /// Return the canonical unit form of a non-zero finite direction.
    pub fn normalized_direction(direction: Vec3) -> Option<Vec3> {
        (direction.is_finite() && direction.length_squared() >= 1.0e-12)
            .then(|| direction.normalize())
    }

    /// Publish a new provider sample and advance the projection revision only
    /// when semantic values changed.
    pub fn publish(&mut self, direction_to_sun: Vec3, irradiance_lux: Option<f32>) -> bool {
        let Some(direction_to_sun) = Self::normalized_direction(direction_to_sun) else {
            return false;
        };
        if irradiance_lux.is_some_and(|lux| !lux.is_finite() || lux < 0.0) {
            return false;
        }
        if self.direction_to_sun != Some(direction_to_sun) || self.irradiance_lux != irradiance_lux
        {
            self.direction_to_sun = Some(direction_to_sun);
            self.irradiance_lux = irradiance_lux;
            self.revision = self.revision.wrapping_add(1);
        }
        true
    }

    /// Remove the current provider sample rather than retaining stale lighting.
    pub fn clear(&mut self) {
        if self.direction_to_sun.take().is_some() || self.irradiance_lux.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Change only the calibrated irradiance while preserving the provider's
    /// direction. Runtime lighting commands use this instead of mutating a
    /// render light and asking the semantic provider to discover the change.
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
/// The environment boundary projects [`SunState`] into the light's local pose
/// before BigSpace propagation. This resource is then published from that
/// light's finalized `GlobalTransform`, so horizon baking and shader wiring
/// consume the same render-space direction as Bevy's shadow extractor. It is
/// never used as a provider input.
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

fn clear_solar_outputs(comp: &mut lunco_cosim_core::SimComponent) {
    comp.outputs.remove(SUN_MOUNT_X_CONNECTOR);
    comp.outputs.remove(SUN_MOUNT_Y_CONNECTOR);
    comp.outputs.remove(SUN_MOUNT_Z_CONNECTOR);
}

fn solar_diagnostic(code: &str, subject: String, message: String) -> lunco_core::RuntimeDiagnostic {
    lunco_core::RuntimeDiagnostic {
        code: code.to_string(),
        severity: lunco_core::DiagnosticSeverity::Error,
        producer: "environment-solar".to_string(),
        subject,
        message,
    }
}

/// Projects semantic [`SunState`] directly into each environment probe's
/// `SimComponent` outputs before [`CosimSet::Propagate`](lunco_cosim_core::schedule::CosimSet).
///
/// The source is the shared celestial sample published to [`SunState`]. This
/// system converts active-frame direction to each probe's mount frame and
/// writes the `f64` cosim outputs in one pass at ordinary FixedUpdate cadence.
/// Missing provider or frame data clears only these three outputs and leaves a
/// persistent runtime diagnostic; Modelica must never retain a stale direction.
pub fn publish_solar_inputs_to_cosim(
    sun: Option<Res<SunState>>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    mut q_targets: Query<
        (Entity, Option<&mut lunco_cosim_core::SimComponent>),
        With<crate::EnvironmentProbe>,
    >,
    diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    if q_targets.is_empty() {
        if let Some(mut diagnostics) = diagnostics {
            diagnostics.replace_producer("environment-solar", std::iter::empty());
        }
        return;
    }

    let mut findings = Vec::new();
    let direction = sun
        .as_deref()
        .and_then(|state| state.direction_to_sun)
        .and_then(SunState::normalized_direction);
    let direction_world = match (direction, active_frame.as_deref()) {
        (None, _) => {
            findings.push(solar_diagnostic(
                "solar-source-missing",
                "EnvironmentProbe".to_string(),
                "a solar environment probe has no valid semantic SunState; solar Modelica inputs were cleared"
                    .to_string(),
            ));
            None
        }
        (Some(_), None) => {
            findings.push(solar_diagnostic(
                "solar-frame-missing",
                "ActivePhysicsFrame".to_string(),
                "SunState exists but no ActivePhysicsFrame is bound; solar Modelica inputs were cleared"
                    .to_string(),
            ));
            None
        }
        (Some(direction), Some(active_frame)) => {
            match lunco_spatial::coords::world_pose(
                active_frame.0,
                &q_parents,
                &q_grids,
                &q_spatial,
            ) {
                Ok((_, frame_rotation)) => {
                    let world = frame_rotation.0 * direction.as_dvec3();
                    (world.is_finite() && world.length_squared() >= 1.0e-24).then_some(world)
                }
                Err(_) => {
                    findings.push(solar_diagnostic(
                        "solar-frame-invalid",
                        format!("frame:{:?}", active_frame.0),
                        "the bound ActivePhysicsFrame has no complete BigSpace pose; solar Modelica inputs were cleared"
                            .to_string(),
                    ));
                    None
                }
            }
        }
    };
    if direction.is_some() && direction_world.is_none() && findings.is_empty() {
        findings.push(solar_diagnostic(
            "solar-direction-invalid",
            "SunState".to_string(),
            "the semantic SunState direction is invalid after active-frame projection; solar Modelica inputs were cleared"
                .to_string(),
        ));
    }

    let mut missing_mounts = 0;
    let mut missing_interfaces = 0;
    for (entity, sim) in &mut q_targets {
        let Some(mut sim) = sim else {
            missing_interfaces += 1;
            continue;
        };
        let Some(direction_world) = direction_world else {
            clear_solar_outputs(&mut sim);
            continue;
        };
        let Ok((_, mount_rotation)) =
            lunco_spatial::coords::world_pose(entity, &q_parents, &q_grids, &q_spatial)
        else {
            clear_solar_outputs(&mut sim);
            missing_mounts += 1;
            continue;
        };
        let direction_mount =
            crate::mount_frame::direction_in_mount_rotation(direction_world, mount_rotation.0);
        if !direction_mount.is_finite() || direction_mount.length_squared() < 1.0e-12 {
            clear_solar_outputs(&mut sim);
            missing_mounts += 1;
            continue;
        }
        sim.outputs
            .insert(SUN_MOUNT_X_CONNECTOR.to_string(), direction_mount.x as f64);
        sim.outputs
            .insert(SUN_MOUNT_Y_CONNECTOR.to_string(), direction_mount.y as f64);
        sim.outputs
            .insert(SUN_MOUNT_Z_CONNECTOR.to_string(), direction_mount.z as f64);
    }

    if missing_mounts > 0 {
        findings.push(solar_diagnostic(
            "solar-mount-invalid",
            "EnvironmentProbe".to_string(),
            format!(
                "{missing_mounts} environment probe(s) have no complete BigSpace pose; their solar Modelica inputs were cleared"
            ),
        ));
    }
    if missing_interfaces > 0 {
        findings.push(solar_diagnostic(
            "solar-probe-interface-missing",
            "EnvironmentProbe".to_string(),
            format!(
                "{missing_interfaces} environment probe(s) have no SimComponent output interface"
            ),
        ));
    }
    if let Some(mut diagnostics) = diagnostics {
        diagnostics.replace_producer("environment-solar", findings);
    }
}

/// Project semantic [`SunState`] into the unique unscoped render sun's local pose.
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
    active_frame: Option<Entity>,
    sun_revision: Option<u64>,
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

pub fn project_sun_state_to_light(
    sun: Option<Res<SunState>>,
    mount: Option<Res<lunco_core::SceneMountState>>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
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
    let Some(direction_to_sun) = sun.as_deref().and_then(|state| state.direction_to_sun) else {
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "sun-state".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "semantic-sun".to_string(),
                    message: "active scene has no valid semantic SunState direction".to_string(),
                }),
            );
        }
        return;
    };
    let Some(active_frame) = active_frame else {
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "physics-frame".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "semantic-sun".to_string(),
                    message: "active scene has semantic sun state but no bound ActivePhysicsFrame"
                        .to_string(),
                }),
            );
        }
        return;
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
    let Some(direction_to_sun) = SunState::normalized_direction(direction_to_sun) else {
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "sun-state".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "semantic-sun".to_string(),
                    message: "semantic SunState direction is non-finite or zero".to_string(),
                }),
            );
        }
        return;
    };
    let sun_parent = parent.map(ChildOf::parent);
    let sun_revision = sun.as_deref().map(|state| state.revision);
    let frame_changed = !projection_cache.initialized
        || projection_cache.active_frame != Some(active_frame.0)
        || lunco_spatial::coords::world_pose_changed(active_frame.0, &q_parents, &q_changed);
    let parent_changed = !projection_cache.initialized
        || projection_cache.sun_parent != sun_parent
        || sun_parent.is_some_and(|parent| {
            lunco_spatial::coords::world_pose_changed(parent, &q_parents, &q_changed)
        });
    let sun_changed = !projection_cache.initialized
        || projection_cache.scene_sun != Some(scene_sun)
        || projection_cache.sun_revision != sun_revision
        || transform.is_changed()
        || light.is_changed();
    if !(frame_changed || parent_changed || sun_changed) {
        return;
    }
    let frame_rotation = match (!frame_changed)
        .then_some(projection_cache.frame_rotation)
        .flatten()
    {
        Some(rotation) => rotation,
        None => match spatial_rotation(active_frame.0, &q_parents, &q_grids, &q_spatial) {
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
                                "ActivePhysicsFrame {:?} has no complete BigSpace pose",
                                active_frame.0
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
    let direction_to_sun_world = frame_rotation * direction_to_sun.as_dvec3();
    if !direction_to_sun_world.is_finite() || direction_to_sun_world.length_squared() < 1.0e-24 {
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "sun-state".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "semantic-sun".to_string(),
                    message:
                        "semantic SunState direction is non-finite or zero after frame projection"
                            .to_string(),
                }),
            );
        }
        return;
    }
    let direction_to_sun_world = direction_to_sun_world.normalize();
    // A root light has no parent-local rotation: its Transform is already in
    // the world frame. A child light must use the live parent projection above.
    let emit_direction = match parent_rotation {
        Some(rotation) => (rotation.inverse() * -direction_to_sun_world).as_vec3(),
        None => (-direction_to_sun_world).as_vec3(),
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
        active_frame: Some(active_frame.0),
        sun_revision,
        frame_rotation: Some(frame_rotation),
        parent_rotation,
        initialized: true,
    };
}

/// Build the change gate for the Bevy scene-light projection. The physical
/// provider publishes `SunState`; the light is updated only when that sample,
/// its physics frame, or the light's spatial ancestry changes.
pub fn tracked_sun_light_projection() -> impl bevy::ecs::schedule::SystemCondition<()> {
    lunco_core_runtime::gate::tracked(
        "environment_sun_light_projection",
        sun_light_projection_needed,
    )
}

fn sun_light_projection_needed(
    sun: Option<Res<SunState>>,
    mount: Option<Res<lunco_core::SceneMountState>>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    q_sun: Query<
        (Entity, Option<&ChildOf>),
        (
            With<bevy::light::DirectionalLight>,
            Without<Earthshine>,
            Without<bevy::camera::visibility::RenderLayers>,
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
        || mount.is_some_and(|state| state.is_changed())
        || active_frame
            .as_ref()
            .is_some_and(|frame| frame.is_changed())
        || !changed_suns.is_empty()
    {
        return true;
    }

    let changed_pose =
        |entity| lunco_spatial::coords::world_pose_changed(entity, &q_parents, &changed_spatial);
    active_frame
        .as_ref()
        .is_some_and(|frame| changed_pose(frame.0))
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
    sun: Option<Res<SunState>>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
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
    let Some(direction_to_sun) = sun
        .as_deref()
        .and_then(|state| state.direction_to_sun)
        .and_then(SunState::normalized_direction)
    else {
        clear_render_sun_if_scene_is_idle(&mut render_state, coordinator.as_deref());
        return;
    };
    let Some(active_frame) = active_frame else {
        clear_render_sun_if_scene_is_idle(&mut render_state, coordinator.as_deref());
        return;
    };
    let Ok(frame_gt) = q_frames.get(active_frame.0) else {
        clear_render_sun_if_scene_is_idle(&mut render_state, coordinator.as_deref());
        return;
    };
    let Ok(sun_gt) = q_sun.single() else {
        clear_render_sun_if_scene_is_idle(&mut render_state, coordinator.as_deref());
        return;
    };

    let expected = (frame_gt.rotation() * direction_to_sun).normalize_or_zero();
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
    sun: Option<Res<SunState>>,
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
    sun.is_some_and(|state| state.is_changed()) || !q_sun.is_empty()
}

// Direct SunState→SimComponent publication is implemented above with the
// active-frame and probe-mount conversions in the same FixedUpdate pass.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_sun_clears_solar_outputs_and_reports_the_missing_provider() {
        let mut app = App::new();
        app.init_resource::<lunco_core::RuntimeDiagnostics>();
        let mut sim = lunco_cosim_core::SimComponent::default();
        sim.outputs.insert(SUN_MOUNT_X_CONNECTOR.to_owned(), 1.0);
        sim.outputs.insert(SUN_MOUNT_Y_CONNECTOR.to_owned(), 2.0);
        sim.outputs.insert(SUN_MOUNT_Z_CONNECTOR.to_owned(), 3.0);
        sim.outputs
            .insert(lunco_cosim_core::GRAVITY_SOURCE_CONNECTOR.to_owned(), 9.81);
        let probe = app.world_mut().spawn((crate::EnvironmentProbe, sim)).id();
        app.add_systems(Update, publish_solar_inputs_to_cosim);

        app.update();
        let outputs = &app
            .world()
            .get::<lunco_cosim_core::SimComponent>(probe)
            .unwrap()
            .outputs;
        assert!(!outputs.contains_key(SUN_MOUNT_X_CONNECTOR));
        assert!(!outputs.contains_key(SUN_MOUNT_Y_CONNECTOR));
        assert!(!outputs.contains_key(SUN_MOUNT_Z_CONNECTOR));
        assert_eq!(
            outputs.get(lunco_cosim_core::GRAVITY_SOURCE_CONNECTOR),
            Some(&9.81)
        );
        assert!(
            app.world()
                .resource::<lunco_core::RuntimeDiagnostics>()
                .findings
                .iter()
                .any(|finding| finding.code == "solar-source-missing")
        );
    }

    #[test]
    fn active_scene_without_a_sun_is_a_persistent_diagnostic() {
        let mut app = App::new();
        let root = app.world_mut().spawn_empty().id();
        let mut mount = lunco_core::SceneMountState::default();
        mount.register_root(root, true);
        app.insert_resource(mount);
        app.init_resource::<SunState>();
        app.init_resource::<SunRenderState>();
        app.init_resource::<lunco_core::RuntimeDiagnostics>();
        app.add_systems(
            Update,
            project_sun_state_to_light.run_if(tracked_sun_light_projection()),
        );

        app.update();

        let diagnostics = app.world().resource::<lunco_core::RuntimeDiagnostics>();
        assert_eq!(diagnostics.findings.len(), 1);
        assert_eq!(diagnostics.findings[0].code, "sun-contract");
        assert_eq!(
            diagnostics.findings[0].severity,
            lunco_core::DiagnosticSeverity::Error
        );
    }

    #[test]
    fn sun_light_projection_gate_skips_unchanged_frames() {
        let mut app = App::new();
        app.init_resource::<SunState>();
        app.init_resource::<lunco_core_runtime::gate::GateActivity>();
        app.world_mut().spawn((
            Transform::IDENTITY,
            GlobalTransform::IDENTITY,
            DirectionalLight::default(),
        ));
        app.add_systems(
            Update,
            project_sun_state_to_light.run_if(tracked_sun_light_projection()),
        );

        app.update();
        app.update();

        let activity = app
            .world()
            .resource::<lunco_core_runtime::gate::GateActivity>()
            .get("environment_sun_light_projection")
            .expect("tracked light projection gate");
        assert_eq!(activity.evaluations, 2);
        assert_eq!(activity.fired, 1);
    }

    #[test]
    fn rotated_site_frame_is_projected_before_mount_conversion() {
        let mut app = App::new();
        let site_rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let frame = app
            .world_mut()
            .spawn((
                big_space::prelude::Grid::default(),
                Transform::from_rotation(site_rotation),
                // Deliberately stale: this is the value that was previously
                // read before BigSpace's PostUpdate propagation.
                GlobalTransform::IDENTITY,
            ))
            .id();
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(frame));
        app.insert_resource(SunState {
            direction_to_sun: Some(Vec3::NEG_Z),
            ..Default::default()
        });
        app.add_systems(Update, publish_solar_inputs_to_cosim);

        let mount_rotation = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let probe = app
            .world_mut()
            .spawn((
                crate::EnvironmentProbe,
                lunco_cosim_core::SimComponent::default(),
                Transform::from_rotation(mount_rotation),
                GlobalTransform::IDENTITY,
            ))
            .id();
        app.update();

        let expected = mount_rotation.inverse() * (site_rotation * Vec3::NEG_Z);
        let outputs = &app
            .world()
            .get::<lunco_cosim_core::SimComponent>(probe)
            .expect("cosim output interface")
            .outputs;
        let got = Vec3::new(
            *outputs.get(SUN_MOUNT_X_CONNECTOR).expect("sun x") as f32,
            *outputs.get(SUN_MOUNT_Y_CONNECTOR).expect("sun y") as f32,
            *outputs.get(SUN_MOUNT_Z_CONNECTOR).expect("sun z") as f32,
        );
        assert!(
            got.abs_diff_eq(expected.normalize(), 1e-5),
            "site ENU must become active-world before mount conversion: got {:?}, expected {:?}",
            got,
            expected
        );
    }

    #[test]
    fn a_probe_without_mount_transform_reports_an_error() {
        let mut app = App::new();
        let frame = app
            .world_mut()
            .spawn((
                big_space::prelude::Grid::default(),
                Transform::default(),
                GlobalTransform::IDENTITY,
            ))
            .id();
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(frame));
        app.insert_resource(SunState {
            direction_to_sun: Some(Vec3::NEG_Z),
            ..Default::default()
        });
        app.init_resource::<lunco_core::RuntimeDiagnostics>();
        app.add_systems(Update, publish_solar_inputs_to_cosim);
        let mut sim = lunco_cosim_core::SimComponent::default();
        sim.outputs.insert(SUN_MOUNT_X_CONNECTOR.to_owned(), 0.0);
        sim.outputs.insert(SUN_MOUNT_Y_CONNECTOR.to_owned(), 0.0);
        sim.outputs.insert(SUN_MOUNT_Z_CONNECTOR.to_owned(), 1.0);
        let probe = app.world_mut().spawn((crate::EnvironmentProbe, sim)).id();

        app.update();

        let outputs = &app
            .world()
            .get::<lunco_cosim_core::SimComponent>(probe)
            .unwrap()
            .outputs;
        assert!(!outputs.contains_key(SUN_MOUNT_X_CONNECTOR));
        assert!(!outputs.contains_key(SUN_MOUNT_Y_CONNECTOR));
        assert!(!outputs.contains_key(SUN_MOUNT_Z_CONNECTOR));
        assert!(
            app.world()
                .resource::<lunco_core::RuntimeDiagnostics>()
                .findings
                .iter()
                .any(|finding| finding.code == "solar-mount-invalid")
        );
    }

    #[test]
    fn sun_light_projection_uses_the_current_big_space_frame_pose() {
        let mut app = App::new();
        let frame_rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let frame = app
            .world_mut()
            .spawn((
                big_space::prelude::Grid::default(),
                Transform::from_rotation(frame_rotation),
                // The render-relative transform is intentionally stale. A
                // high-rate rotating site must not aim from this value.
                GlobalTransform::IDENTITY,
            ))
            .id();
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(frame));
        app.insert_resource(SunState {
            direction_to_sun: Some(Vec3::NEG_Z),
            ..Default::default()
        });
        app.init_resource::<SunRenderState>();
        let sun = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
                DirectionalLight::default(),
            ))
            .id();
        app.add_systems(
            Update,
            project_sun_state_to_light.run_if(tracked_sun_light_projection()),
        );

        app.update();

        let light = app.world().get::<Transform>(sun).unwrap();
        let expected = -(frame_rotation * Vec3::NEG_Z);
        assert!(
            light.forward().abs_diff_eq(expected.normalize(), 1.0e-5),
            "sun light must use the current f64 frame pose: got {:?}, expected {:?}",
            light.forward(),
            expected
        );

        app.world_mut()
            .get_mut::<Transform>(frame)
            .unwrap()
            .rotation = Quat::IDENTITY;
        app.update();

        let light = app.world().get::<Transform>(sun).unwrap();
        assert!(
            light.forward().abs_diff_eq(Vec3::Z, 1.0e-5),
            "a changed frame ancestor must invalidate the cached pose: got {:?}",
            light.forward()
        );
    }

    #[test]
    fn sun_light_projection_resolves_a_big_space_root_parent() {
        let mut app = App::new();
        let frame = app
            .world_mut()
            .spawn((big_space::prelude::Grid::default(), Transform::default()))
            .id();
        let parent_rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let parent = app
            .world_mut()
            .spawn((
                big_space::prelude::Grid::default(),
                Transform::from_rotation(parent_rotation),
            ))
            .id();
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(frame));
        app.insert_resource(SunState {
            direction_to_sun: Some(Vec3::NEG_Z),
            ..Default::default()
        });
        let sun = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                DirectionalLight::default(),
                ChildOf(parent),
            ))
            .id();
        app.add_systems(Update, project_sun_state_to_light);

        app.update();

        let light = app.world().get::<Transform>(sun).unwrap();
        let expected = parent_rotation.inverse() * Vec3::Z;
        assert!(
            light.forward().abs_diff_eq(expected, 1.0e-5),
            "a valid BigSpace root parent must participate in the light projection: got {:?}, expected {:?}",
            light.forward(),
            expected
        );
    }

    #[test]
    fn finalized_render_state_uses_the_propagated_light_pose() {
        let mut app = App::new();
        let frame = app
            .world_mut()
            .spawn((
                GlobalTransform::IDENTITY,
                big_space::prelude::Grid::default(),
            ))
            .id();
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(frame));
        app.insert_resource(SunState {
            direction_to_sun: Some(Vec3::X),
            ..Default::default()
        });
        app.init_resource::<SunRenderState>();
        app.insert_resource(lunco_core::RuntimeDiagnostics {
            findings: vec![lunco_core::RuntimeDiagnostic {
                code: "sun-state".to_string(),
                severity: lunco_core::DiagnosticSeverity::Error,
                producer: "environment-sun".to_string(),
                subject: "semantic-sun".to_string(),
                message: "upstream validation failure".to_string(),
            }],
        });

        // The local Transform is deliberately stale. The render snapshot must
        // follow the finalized GlobalTransform that shadow extraction uses.
        app.world_mut().spawn((
            Transform::IDENTITY,
            GlobalTransform::from_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
            DirectionalLight::default(),
        ));
        app.add_systems(PostUpdate, finalize_sun_render_state);

        app.update();

        let actual = app
            .world()
            .resource::<SunRenderState>()
            .direction_to_sun_world
            .expect("finalized render sun direction");
        assert!(actual.abs_diff_eq(Vec3::X, 1.0e-6), "got {actual:?}");
        let diagnostics = app.world().resource::<lunco_core::RuntimeDiagnostics>();
        assert_eq!(diagnostics.findings.len(), 1);
        assert_eq!(diagnostics.findings[0].code, "sun-state");
    }
}
