//! Solar environment domain — the sun's direction as a co-simulation source.
//!
//! The lighting analog of the gravity bridge. Semantic [`SunState`] is the
//! provider contract for physical consumers; detached [`SunRenderPresentation`]
//! may select a celestial-time direction for rendering.
//! This module caches the semantic direction per-entity as [`LocalSolar`] and
//! publishes it into the co-sim graph as ordinary `SimComponent` **outputs**,
//! so a sun-tracking model receives it through a plain output→input wire — the
//! ontology's `RadiationProvider → LocalRadiation → solar models` pipeline.
//!
//! Values are published on explicit [`crate::EnvironmentProbe`] source prims.
//! Models consume them through ordinary USD connections, so provider and
//! consumer remain distinct graph nodes.
//!
//! ## Provider note
//!
//! There is no separate `SolarProvider` component yet: [`SunState`] is the
//! provider contract (its direction is published by ephemeris or an explicit
//! command). A richer provider (irradiance
//! model, eclipse occlusion, per-site horizon visibility) would attach here
//! later, exactly as `GravityProvider` carries the gravity model — the
//! [`LocalSolar`] cache already gives each entity its own slot for that.

use bevy::{
    math::{DQuat, DVec3},
    prelude::*,
};

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

/// Render-facing selection between physical solar state and detached celestial presentation.
///
/// Celestial directions stay in `f64` and in the active physics frame until
/// the environment projects them into the scene light's render transform.
/// This resource never replaces [`SunState`] or feeds physics and
/// co-simulation.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Default)]
pub struct SunRenderPresentation {
    selection: SunRenderSelection,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum SunRenderSelection {
    #[default]
    Semantic,
    Celestial(DVec3),
    Unavailable,
}

/// Schedule boundary after presentation producers and before transform propagation.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SunRenderProjectionSet;

impl SunRenderPresentation {
    /// Publish a finite, nonzero celestial direction in the active physics frame.
    pub fn publish(&mut self, direction_to_sun_active_frame: DVec3) -> bool {
        let length_squared = direction_to_sun_active_frame.length_squared();
        if !direction_to_sun_active_frame.is_finite()
            || !length_squared.is_finite()
            || length_squared < 1.0e-24
        {
            self.invalidate();
            return false;
        }
        self.set_selection(SunRenderSelection::Celestial(
            direction_to_sun_active_frame.normalize(),
        ));
        true
    }

    /// Select physical solar state when the celestial clock tracks simulation time.
    pub fn select_semantic(&mut self) {
        self.set_selection(SunRenderSelection::Semantic);
    }

    /// Mark detached celestial input unavailable without using physical-time state.
    pub fn invalidate(&mut self) {
        self.set_selection(SunRenderSelection::Unavailable);
    }

    /// Reset presentation ownership at scene teardown.
    pub fn clear(&mut self) {
        self.set_selection(SunRenderSelection::Semantic);
    }

    fn set_selection(&mut self, selection: SunRenderSelection) {
        if self.selection != selection {
            self.selection = selection;
            self.revision = self.revision.wrapping_add(1);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SunProjectionSource {
    Semantic,
    CelestialPresentation,
}

/// Render-facing snapshot of the finalized scene-sun direction.
///
/// The environment boundary projects the selected render direction into the
/// light's local pose before BigSpace propagation. This resource is then
/// published from that light's finalized `GlobalTransform`, so horizon baking,
/// terrain shadows, and Bevy's shadow extractor consume the same direction. It
/// is never used as a provider input.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Default)]
pub struct SunRenderState {
    /// Unit direction toward the Sun in the canonical render/world frame.
    pub direction_to_sun_world: Option<Vec3>,
    pub revision: u64,
}

impl SunRenderState {
    fn publish(&mut self, direction_to_sun_world: Vec3) {
        if self.direction_to_sun_world != Some(direction_to_sun_world) {
            self.direction_to_sun_world = Some(direction_to_sun_world);
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub(crate) fn clear(&mut self) {
        if self.direction_to_sun_world.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }
}

/// Unit direction toward the Sun in an entity's authored mount frame.
///
/// The lighting analog of `LocalGravity`. Today the value is global (one sun,
/// no occlusion) so every entity gets the same direction, but it is cached
/// per-entity so a future per-site horizon/eclipse model can vary it without
/// touching consumers.
///
/// The convention is explicit and shared with antenna tracking: `+X` right,
/// `+Y` up, `-Z` forward.  The full world→mount rotation is applied before a
/// model selects joint angles, so vehicle yaw, pitch and roll cannot be
/// mistaken for a solar bearing.
#[derive(Component, Debug, Clone, Copy, PartialEq, Reflect, Default)]
#[reflect(Component)]
pub struct LocalSolar {
    /// Complete active-world→mount direction, kept as a vector until a
    /// consumer needs its own coordinates.
    pub direction: Vec3,
}

/// Computes [`LocalSolar`] for every explicit environment probe from the scene sun.
///
/// Semantic [`SunState`] is the provider. Render-layer-scoped preview lights
/// and earthshine never participate in this source path. Writes `LocalSolar`
/// only when the direction actually changes, to avoid a per-frame
/// change-detection storm — mirrors `compute_local_gravity`.
///
/// Targets entities that carry [`crate::EnvironmentProbe`] so the cache lands
/// exactly where [`inject_local_solar_into_cosim`] will publish it.
pub fn compute_local_solar(
    mut commands: Commands,
    sun: Option<Res<SunState>>,
    active_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    q_targets: Query<(Entity, Option<&LocalSolar>), With<crate::EnvironmentProbe>>,
    diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    if q_targets.is_empty() {
        if let Some(mut diagnostics) = diagnostics {
            diagnostics.replace_producer("environment-solar", std::iter::empty());
        }
        return;
    }
    let Some(direction_to_sun) = sun
        .as_deref()
        .and_then(|state| state.direction_to_sun)
        .and_then(SunState::normalized_direction)
    else {
        for (entity, existing) in &q_targets {
            if existing.is_some() {
                commands.entity(entity).remove::<LocalSolar>();
            }
        }
        if let Some(mut diagnostics) = diagnostics {
            diagnostics.replace_producer("environment-solar", std::iter::empty());
        }
        return;
    };

    let Some(active_frame) = active_frame else {
        for (entity, existing) in &q_targets {
            if existing.is_some() {
                commands.entity(entity).remove::<LocalSolar>();
            }
        }
        if let Some(mut diagnostics) = diagnostics {
            diagnostics.replace_producer(
                "environment-solar",
                [lunco_core::RuntimeDiagnostic {
                    code: "solar-frame".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-solar".to_string(),
                    subject: "LocalSolar".to_string(),
                    message: "a semantic SunState exists but no ActivePhysicsFrame is bound"
                        .to_string(),
                }],
            );
        }
        return;
    };
    let Ok((_, frame_rotation)) =
        lunco_spatial::coords::world_pose(active_frame.0, &q_parents, &q_grids, &q_spatial)
    else {
        for (entity, existing) in &q_targets {
            if existing.is_some() {
                commands.entity(entity).remove::<LocalSolar>();
            }
        }
        if let Some(mut diagnostics) = diagnostics {
            diagnostics.replace_producer(
                "environment-solar",
                [lunco_core::RuntimeDiagnostic {
                    code: "solar-frame".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-solar".to_string(),
                    subject: format!("frame:{:?}", active_frame.0),
                    message: "the bound ActivePhysicsFrame has no complete BigSpace pose"
                        .to_string(),
                }],
            );
        }
        return;
    };
    let direction_to_sun_world = frame_rotation.0 * direction_to_sun.as_dvec3();
    if !direction_to_sun_world.is_finite() || direction_to_sun_world.length_squared() < 1.0e-24 {
        for (entity, existing) in &q_targets {
            if existing.is_some() {
                commands.entity(entity).remove::<LocalSolar>();
            }
        }
        if let Some(mut diagnostics) = diagnostics {
            diagnostics.replace_producer(
                "environment-solar",
                [lunco_core::RuntimeDiagnostic {
                    code: "solar-frame".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-solar".to_string(),
                    subject: "LocalSolar".to_string(),
                    message:
                        "the semantic SunState direction is invalid after active-frame projection"
                            .to_string(),
                }],
            );
        }
        return;
    }
    let mut missing_mounts = 0;
    for (entity, existing) in &q_targets {
        let Ok((_, mount_rotation)) =
            lunco_spatial::coords::world_pose(entity, &q_parents, &q_grids, &q_spatial)
        else {
            missing_mounts += 1;
            if existing.is_some() {
                commands.entity(entity).remove::<LocalSolar>();
            }
            continue;
        };
        let next = LocalSolar {
            direction: crate::mount_frame::direction_in_mount_rotation(
                direction_to_sun_world,
                mount_rotation.0,
            ),
        };
        if existing == Some(&next) {
            continue;
        }
        commands.entity(entity).try_insert(next);
    }
    if let Some(mut diagnostics) = diagnostics {
        if missing_mounts == 0 {
            diagnostics.replace_producer("environment-solar", std::iter::empty());
        } else {
            diagnostics.replace_producer(
                "environment-solar",
                [lunco_core::RuntimeDiagnostic {
                    code: "solar-mount".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-solar".to_string(),
                    subject: "EnvironmentProbe".to_string(),
                    message: format!(
                        "{missing_mounts} environment probe(s) have no complete BigSpace pose for solar projection"
                    ),
                }],
            );
        }
    }
}

/// Project the selected render direction into the unique unscoped scene sun.
///
/// This is the only system that writes the scene light's direction. It runs
/// after the celestial presentation producer and before BigSpace transform
/// propagation, so the finalized light and its shadow consumers use the same
/// render epoch. Zero or multiple candidate lights is a contract error for the
/// render host; no arbitrary light is selected.
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
    direction_revision: Option<u64>,
    provider_revision: Option<u64>,
    sun_source: Option<SunProjectionSource>,
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

pub fn project_sun_render_to_light(
    sun: Res<SunState>,
    presentation: Res<SunRenderPresentation>,
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
    let (source, direction_to_sun) = match presentation.selection {
        SunRenderSelection::Celestial(direction) => {
            (SunProjectionSource::CelestialPresentation, direction)
        }
        SunRenderSelection::Semantic => {
            let Some(direction) = sun
                .direction_to_sun
                .and_then(SunState::normalized_direction)
            else {
                if active_scene {
                    replace_sun_diagnostic(
                        &mut diagnostics,
                        Some(lunco_core::RuntimeDiagnostic {
                            code: "sun-state".to_string(),
                            severity: lunco_core::DiagnosticSeverity::Error,
                            producer: "environment-sun".to_string(),
                            subject: "semantic-sun".to_string(),
                            message: "active scene has no valid semantic Sun direction".to_string(),
                        }),
                    );
                }
                return;
            };
            (SunProjectionSource::Semantic, direction.as_dvec3())
        }
        SunRenderSelection::Unavailable => {
            if active_scene {
                replace_sun_diagnostic(
                    &mut diagnostics,
                    Some(lunco_core::RuntimeDiagnostic {
                        code: "sun-presentation".to_string(),
                        severity: lunco_core::DiagnosticSeverity::Error,
                        producer: "environment-sun".to_string(),
                        subject: "celestial-presentation".to_string(),
                        message: "render Sun direction is unavailable from its selected presentation owner".to_string(),
                    }),
                );
            }
            return;
        }
    };
    let Some(active_frame) = active_frame else {
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "physics-frame".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "sun-direction".to_string(),
                    message:
                        "active scene has a selected Sun direction but no bound ActivePhysicsFrame"
                            .to_string(),
                }),
            );
        }
        return;
    };
    let invalid_irradiance = sun
        .irradiance_lux
        .is_some_and(|lux| !lux.is_finite() || lux < 0.0);
    if invalid_irradiance {
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
        if source == SunProjectionSource::Semantic {
            return;
        }
    }
    let sun_parent = parent.map(ChildOf::parent);
    let source_revision = match source {
        SunProjectionSource::CelestialPresentation => presentation.revision,
        SunProjectionSource::Semantic => sun.revision,
    };
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
        || projection_cache.direction_revision != Some(source_revision)
        || projection_cache.provider_revision != Some(sun.revision)
        || projection_cache.sun_source != Some(source)
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
    let direction_to_sun_world = frame_rotation * direction_to_sun;
    if !direction_to_sun_world.is_finite() || direction_to_sun_world.length_squared() < 1.0e-24 {
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "sun-state".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "sun-direction".to_string(),
                    message: "selected render Sun direction is non-finite or zero after active-frame projection".to_string(),
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
    if let Some(irradiance) = sun.irradiance_lux.filter(|_| !invalid_irradiance) {
        if (light.illuminance - irradiance).abs() > irradiance.abs().max(1.0) * 5.0e-3 {
            light.illuminance = irradiance;
        }
    }
    *projection_cache = SunProjectionCache {
        scene_sun: Some(scene_sun),
        sun_parent,
        active_frame: Some(active_frame.0),
        direction_revision: Some(source_revision),
        provider_revision: Some(sun.revision),
        sun_source: Some(source),
        frame_rotation: Some(frame_rotation),
        parent_rotation,
        initialized: true,
    };
}

/// Keep the last committed render-sun sample while a scene transaction is
/// still assembling its transforms. Scene teardown clears the resource at the
/// ownership boundary; during a load, an incomplete frame is therefore a
/// pending presentation product rather than a new "black" sun.
fn clear_render_sun_if_scene_is_idle(
    render_state: &mut SunRenderState,
    coordinator: Option<&lunco_core::SceneTransitionCoordinator>,
) {
    if coordinator.is_none_or(|coordinator| coordinator.active().is_none()) {
        render_state.clear();
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
    sun: Res<SunState>,
    presentation: Res<SunRenderPresentation>,
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
    let direction_to_sun = match presentation.selection {
        SunRenderSelection::Celestial(direction) => Some(direction),
        SunRenderSelection::Semantic => sun
            .direction_to_sun
            .and_then(SunState::normalized_direction)
            .map(Vec3::as_dvec3),
        SunRenderSelection::Unavailable => None,
    };
    let Some(direction_to_sun) = direction_to_sun else {
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

    // The finalized transform is the explicit f32 render boundary. Keep the
    // selected celestial vector in f64 until it is projected through the
    // finalized active-frame rotation.
    let expected = (frame_gt.rotation().as_dquat() * direction_to_sun)
        .as_vec3()
        .normalize_or_zero();
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
    render_state.publish(actual);
}

/// Publishes each entity's [`LocalSolar`] as `SimComponent` **outputs**
/// [`SUN_MOUNT_X_CONNECTOR`] / [`SUN_MOUNT_Y_CONNECTOR`] /
/// [`SUN_MOUNT_Z_CONNECTOR`].
///
/// Runs after [`compute_local_solar`] and before cosim propagation, so the
/// fresh outputs are read the same tick. Writes every tick because a model's
/// own output sync may rewrite its outputs map (same reasoning as the gravity
/// bridge). If no scene sun is available, removes only the solar outputs while
/// retaining the schema-declared source contract for later binding.
pub fn inject_local_solar_into_cosim(
    mut q: Query<
        (Option<&LocalSolar>, &mut lunco_cosim_core::SimComponent),
        With<crate::EnvironmentProbe>,
    >,
) {
    for (solar, mut comp) in &mut q {
        let Some(solar) = solar else {
            comp.outputs.remove(SUN_MOUNT_X_CONNECTOR);
            comp.outputs.remove(SUN_MOUNT_Y_CONNECTOR);
            comp.outputs.remove(SUN_MOUNT_Z_CONNECTOR);
            continue;
        };
        comp.outputs
            .insert(SUN_MOUNT_X_CONNECTOR.to_string(), solar.direction.x as f64);
        comp.outputs
            .insert(SUN_MOUNT_Y_CONNECTOR.to_string(), solar.direction.y as f64);
        comp.outputs
            .insert(SUN_MOUNT_Z_CONNECTOR.to_string(), solar.direction.z as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_sun_removes_cached_local_direction() {
        let mut app = App::new();
        app.add_systems(Update, compute_local_solar);
        let probe = app
            .world_mut()
            .spawn((
                crate::EnvironmentProbe,
                LocalSolar {
                    direction: Vec3::NEG_Z,
                },
            ))
            .id();
        app.update();
        assert!(
            app.world().get::<LocalSolar>(probe).is_none(),
            "a scene without a sun must not retain a stale solar direction"
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
        app.init_resource::<SunRenderPresentation>();
        app.init_resource::<SunRenderState>();
        app.init_resource::<lunco_core::RuntimeDiagnostics>();
        app.add_systems(PostUpdate, project_sun_render_to_light);

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
    fn unavailable_celestial_sun_does_not_fall_back_to_semantic_state() {
        let mut app = App::new();
        let root = app.world_mut().spawn_empty().id();
        let mut mount = lunco_core::SceneMountState::default();
        mount.register_root(root, true);
        app.insert_resource(mount);
        app.insert_resource(SunState {
            direction_to_sun: Some(Vec3::X),
            ..Default::default()
        });
        let mut presentation = SunRenderPresentation::default();
        presentation.invalidate();
        app.insert_resource(presentation);
        app.init_resource::<lunco_core::RuntimeDiagnostics>();
        let sun = app
            .world_mut()
            .spawn((Transform::IDENTITY, DirectionalLight::default()))
            .id();
        app.add_systems(PostUpdate, project_sun_render_to_light);

        app.update();

        let light = app.world().get::<Transform>(sun).unwrap();
        assert!(light.forward().abs_diff_eq(Vec3::NEG_Z, 1.0e-5));
        assert!(
            app.world()
                .resource::<lunco_core::RuntimeDiagnostics>()
                .findings
                .iter()
                .any(|finding| finding.code == "sun-presentation")
        );
    }

    #[test]
    fn missing_solar_direction_removes_only_solar_outputs() {
        let mut app = App::new();
        let mut sim = lunco_cosim_core::SimComponent::default();
        sim.outputs.insert(SUN_MOUNT_X_CONNECTOR.to_owned(), 1.0);
        sim.outputs.insert(SUN_MOUNT_Y_CONNECTOR.to_owned(), 2.0);
        sim.outputs.insert(SUN_MOUNT_Z_CONNECTOR.to_owned(), 3.0);
        sim.outputs
            .insert(lunco_cosim_core::GRAVITY_SOURCE_CONNECTOR.to_owned(), 9.81);
        let entity = app.world_mut().spawn((crate::EnvironmentProbe, sim)).id();
        app.add_systems(Update, inject_local_solar_into_cosim);

        app.update();

        let outputs = &app
            .world()
            .get::<lunco_cosim_core::SimComponent>(entity)
            .unwrap()
            .outputs;
        assert!(!outputs.contains_key(SUN_MOUNT_X_CONNECTOR));
        assert!(!outputs.contains_key(SUN_MOUNT_Y_CONNECTOR));
        assert!(!outputs.contains_key(SUN_MOUNT_Z_CONNECTOR));
        assert_eq!(
            outputs.get(lunco_cosim_core::GRAVITY_SOURCE_CONNECTOR),
            Some(&9.81)
        );
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
        app.add_systems(Update, compute_local_solar);

        let mount_rotation = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let probe = app
            .world_mut()
            .spawn((
                crate::EnvironmentProbe,
                Transform::from_rotation(mount_rotation),
                GlobalTransform::IDENTITY,
            ))
            .id();
        app.update();

        let expected = mount_rotation.inverse() * (site_rotation * Vec3::NEG_Z);
        let got = app
            .world()
            .get::<LocalSolar>(probe)
            .expect("projected solar direction");
        assert!(
            got.direction.abs_diff_eq(expected.normalize(), 1e-5),
            "site ENU must become active-world before mount conversion: got {:?}, expected {:?}",
            got.direction,
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
        app.add_systems(Update, compute_local_solar);
        let probe = app
            .world_mut()
            .spawn((crate::EnvironmentProbe, LocalSolar { direction: Vec3::X }))
            .id();

        app.update();

        assert!(app.world().get::<LocalSolar>(probe).is_none());
        assert!(
            app.world()
                .resource::<lunco_core::RuntimeDiagnostics>()
                .findings
                .iter()
                .any(|finding| finding.code == "solar-mount")
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
        app.init_resource::<SunRenderPresentation>();
        assert!(
            app.world_mut()
                .resource_mut::<SunRenderPresentation>()
                .publish(DVec3::X)
        );
        app.init_resource::<SunRenderState>();
        let sun = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                GlobalTransform::IDENTITY,
                DirectionalLight::default(),
            ))
            .id();
        app.add_systems(PostUpdate, project_sun_render_to_light);

        app.update();

        let light = app.world().get::<Transform>(sun).unwrap();
        let expected = -(frame_rotation * Vec3::X);
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
        assert!(
            app.world_mut()
                .resource_mut::<SunRenderPresentation>()
                .publish(DVec3::Y)
        );
        app.update();

        let light = app.world().get::<Transform>(sun).unwrap();
        assert!(
            light.forward().abs_diff_eq(Vec3::NEG_Y, 1.0e-5),
            "a changed frame ancestor must invalidate the cached pose: got {:?}",
            light.forward()
        );
        assert_eq!(
            app.world().resource::<SunState>().direction_to_sun,
            Some(Vec3::NEG_Z),
            "celestial render selection must not replace physical SunState"
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
        app.init_resource::<SunRenderPresentation>();
        let sun = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                DirectionalLight::default(),
                ChildOf(parent),
            ))
            .id();
        app.add_systems(PostUpdate, project_sun_render_to_light);

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
        app.init_resource::<SunRenderPresentation>();
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
