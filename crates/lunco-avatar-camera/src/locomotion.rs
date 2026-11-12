//! Collision-aware local avatar locomotion.
//!
//! This module owns the camera embodiment's movement realization: authored
//! input ports are interpreted as a movement vector, the vector is resolved in
//! the active BigSpace/Avian frame, and the resulting position is written back
//! to the avatar's source Grid. Possession and input projection remain owned by
//! their respective packages.

use avian3d::prelude::{Collider, SpatialQueryFilter};
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_avatar_policy::{
    AvatarCollisionSettings, AvatarSoilCollisionPolicy, avatar_soil_collision_policy,
};
use lunco_camera_core::{
    FreeFlightCamera, FreeFlightSettings, SurfaceCamera, SurfaceRelativeMode,
    math::camera_move_direction,
};
use lunco_celestial_spatial_core::{LocalGravityField, gravity_up_in_grid};
use lunco_core::NON_PHYSICAL_QUERY_LAYERS;
use lunco_embodiment_core::roles::{Embodiment, LocalEmbodiment};
use lunco_interaction_core::DragModeActive;
use lunco_port_core::InputPorts;
use lunco_spatial::ActivePhysicsFrame;
use lunco_spatial::attach::local_pose_to_grid_storage;
use lunco_spatial::coords::grid_absolute_seeded;
use lunco_workspace::WorkspaceResource;

use lunco_camera_core::CameraCollisionEnvelope;

/// Publish the enforced envelope on lens/settings/embodiment/policy revisions.
/// The navigation policy also owns participation in terrain publication safety.
pub(crate) fn sync_camera_collision_envelopes(
    mut commands: Commands,
    settings: Res<AvatarCollisionSettings>,
    workspace: Option<Res<WorkspaceResource>>,
    cameras: Query<(
        Entity,
        Ref<Projection>,
        Ref<LocalEmbodiment>,
        Option<&CameraCollisionEnvelope>,
    )>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let policy = avatar_soil_collision_policy(workspace.as_deref());
    if matches!(policy, Ok(AvatarSoilCollisionPolicy::ThroughSoilAllowed)) || policy.is_err() {
        for (entity, _, _, current) in &cameras {
            if current.is_some() {
                commands.entity(entity).remove::<CameraCollisionEnvelope>();
            }
        }
        lunco_camera_core::replace_camera_diagnostic(
            &mut diagnostics,
            "camera-envelope",
            "CameraNavigation",
            policy.err(),
        );
        return;
    }
    let policy_changed = workspace
        .as_ref()
        .is_some_and(|workspace| workspace.is_changed());
    for (entity, projection, local, current) in &cameras {
        if !settings.is_changed()
            && !projection.is_changed()
            && !local.is_added()
            && !policy_changed
        {
            continue;
        }
        match settings.camera_radius(&projection) {
            Ok(radius_m) => {
                let envelope = CameraCollisionEnvelope {
                    radius_m,
                    skin_m: settings.skin_m,
                    sweep_tolerance_m: settings.sweep_tolerance_m,
                };
                if current != Some(&envelope) {
                    commands.entity(entity).try_insert(envelope);
                }
                lunco_camera_core::replace_camera_diagnostic(
                    &mut diagnostics,
                    "camera-envelope",
                    "CameraNavigation",
                    None,
                );
            }
            Err(error) => {
                commands.entity(entity).remove::<CameraCollisionEnvelope>();
                lunco_camera_core::replace_camera_diagnostic(
                    &mut diagnostics,
                    "camera-envelope",
                    "CameraNavigation",
                    Some(error),
                );
            }
        }
    }
}

pub(crate) fn report_avatar_policy_error(
    error: &str,
    last_error: &mut Option<String>,
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    producer: &str,
) {
    if last_error.as_deref() != Some(error) {
        warn!("[avatar] collision policy unavailable: {error}");
        *last_error = Some(error.to_string());
        lunco_camera_core::replace_camera_diagnostic(
            diagnostics,
            producer,
            "CameraNavigation",
            Some(error.into()),
        );
    }
}

/// Source/prop failures and transient visual preparation have different UI semantics.
#[derive(Debug)]
pub(crate) enum CameraMotionError {
    Invalid(String),
    Preparation {
        reason: String,
        target: lunco_spatial::coords::GridPos,
    },
}
impl From<String> for CameraMotionError {
    fn from(reason: String) -> Self {
        Self::Invalid(reason)
    }
}
impl From<&str> for CameraMotionError {
    fn from(reason: &str) -> Self {
        Self::Invalid(reason.into())
    }
}
impl std::fmt::Display for CameraMotionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(reason) | Self::Preparation { reason, .. } => f.write_str(reason),
        }
    }
}

pub(crate) fn report_camera_motion_error(
    error: &CameraMotionError,
    last_error: &mut Option<String>,
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    producer: &str,
) {
    match error {
        CameraMotionError::Invalid(reason) => {
            report_avatar_policy_error(reason, last_error, diagnostics, producer)
        }
        CameraMotionError::Preparation { reason, .. } => {
            if diagnostics.as_deref().is_some_and(|diagnostics| {
                diagnostics.findings.iter().any(|finding| {
                    finding.producer == producer
                        && finding.message == *reason
                        && finding.severity == lunco_core::DiagnosticSeverity::Info
                })
            }) {
                return;
            }
            *last_error = Some(reason.clone());
            if let Some(diagnostics) = diagnostics.as_deref_mut() {
                diagnostics.replace_producer(
                    producer,
                    [lunco_core::RuntimeDiagnostic {
                        code: "camera-terrain-preparing".into(),
                        severity: lunco_core::DiagnosticSeverity::Info,
                        producer: producer.into(),
                        subject: "CameraNavigation".into(),
                        message: reason.clone(),
                    }],
                );
            }
        }
    }
}

/// Retire a withdrawn visual-preparation request without erasing real failures.
pub(crate) fn clear_camera_preparation(
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    producer: &str,
    last_error: &mut Option<String>,
) {
    let pending = diagnostics.as_deref().is_some_and(|diagnostics| {
        diagnostics.findings.iter().any(|finding| {
            finding.producer == producer && finding.severity == lunco_core::DiagnosticSeverity::Info
        })
    });
    if pending {
        if let Some(diagnostics) = diagnostics.as_deref_mut() {
            diagnostics.replace_producer(producer, std::iter::empty());
        }
        *last_error = None;
    }
}

/// Native collision is the explicit f64 camera-envelope to f32 shape boundary.
pub(crate) fn camera_collider(radius: f64) -> Result<Collider, String> {
    let native_radius = radius as f32;
    if !radius.is_finite() || !native_radius.is_finite() || native_radius <= 0.0 {
        return Err("camera envelope exceeds native shape range".into());
    }
    Ok(Collider::sphere(native_radius))
}

/// Shared query state for free movement, scroll transit and follow rigs.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct NavigationCollisionContext<'w, 's> {
    pub terrain: lunco_terrain_surface::GridSurfaceQuery<'w, 's>,
    pub drawn: lunco_terrain_surface::DrawTerrainQuery<'w, 's>,
    pub targets: ResMut<'w, lunco_terrain_surface::TerrainCameraTargets>,
    pub active_frame: Option<Res<'w, ActivePhysicsFrame>>,
    pub native: Option<lunco_physics::GridSpatialQuery<'w, 's>>,
    pub settings: Res<'w, AvatarCollisionSettings>,
    pub budget: Option<Res<'w, lunco_terrain_surface::SurfaceOcclusionBudget>>,
}

/// Resolve native props, source terrain and current finest draw geometry in one
/// bounded contact-plane loop. Coarse presentation requests preparation; it never
/// redirects the camera. Every projected segment is recast against all owners.
pub(crate) fn resolve_camera_segment(
    camera: Entity,
    frame: Entity,
    start: DVec3,
    delta: DVec3,
    radius: f64,
    settings: &AvatarCollisionSettings,
    terrain: &lunco_terrain_surface::GridSurfaceQuery<'_, '_>,
    terrain_budget: usize,
    drawn: &lunco_terrain_surface::DrawTerrainQuery<'_, '_>,
    targets: &mut lunco_terrain_surface::TerrainCameraTargets,
    native: &lunco_physics::GridSpatialQuery<'_, '_>,
    filter: &SpatialQueryFilter,
) -> Result<DVec3, CameraMotionError> {
    use lunco_spatial::coords::GridPos;
    use lunco_terrain_surface::DrawTerrainSweep;
    let mut position = start;
    let mut remaining = delta;
    let mut planes = Vec::new();
    for _ in 0..settings.max_slides {
        let motion = match cast_camera_source_step(
            position,
            remaining,
            radius,
            settings,
            terrain,
            terrain_budget,
            native,
            filter,
        ) {
            Ok(result) => result,
            Err(error) => {
                targets.clear(camera);
                return Err(error);
            }
        };
        let result = motion.position;
        let actual = result - position;
        // An explicitly requested source-up step can leave an initial overlap;
        // source monotonicity and native props certify that recovery.
        let deliberate_recovery = motion.escaping_terrain
            && actual.dot(delta) > 0.0
            && actual.cross(delta).length()
                <= settings.sweep_tolerance_m * delta.length().max(settings.sweep_tolerance_m);
        if deliberate_recovery {
            targets.clear(camera);
            return Ok(result);
        }
        let preparation = match drawn.sweep(
            camera,
            GridPos(position),
            GridPos(result),
            radius,
            settings.skin_m,
            settings.sweep_tolerance_m,
            terrain_budget,
        ) {
            DrawTerrainSweep::Clear => {
                let rest = remaining - actual;
                position = result;
                let Some(normal) = motion.contact_normal else {
                    targets.clear(camera);
                    return Ok(position);
                };
                remaining = rest;
                planes.push(
                    Dir3::new(normal.as_vec3())
                        .map_err(|_| "camera source contact normal exceeds native range")?,
                );
                None
            }
            DrawTerrainSweep::Hit {
                time,
                normal,
                refinement_required,
                ..
            } => {
                if !time.is_finite() || !(0.0..=1.0).contains(&time) {
                    targets.clear(camera);
                    return Err("presented terrain returned an invalid contact time".into());
                }
                if refinement_required {
                    Some((
                        "camera is waiting for visible terrain refinement along its requested path"
                            .into(),
                        GridPos(position + actual * time),
                        false,
                    ))
                } else {
                    let length = actual.length();
                    if !normal.is_finite() || !(0.5..=1.5).contains(&normal.length_squared()) {
                        targets.clear(camera);
                        return Err("presented terrain returned an invalid contact normal".into());
                    }
                    let normal = normal.normalize();
                    let approach = if length > 0.0 {
                        -(actual / length).dot(normal)
                    } else {
                        0.0
                    };
                    // The margin is normal separation, including at grazing
                    // incidence; travel-only pullback can be smaller than a
                    // native camera-storage rounding unit.
                    let fraction = if approach > 0.0 {
                        (time - settings.sweep_tolerance_m / (length * approach)).max(0.0)
                    } else {
                        0.0
                    };
                    let prefix = match cast_camera_source_step(
                        position,
                        actual * fraction,
                        radius,
                        settings,
                        terrain,
                        terrain_budget,
                        native,
                        filter,
                    ) {
                        Ok(step) => step.position,
                        Err(error) => {
                            targets.clear(camera);
                            return Err(error);
                        }
                    };
                    remaining -= prefix - position;
                    position = prefix;
                    planes.push(
                        Dir3::new(normal.as_vec3())
                            .map_err(|_| "camera draw contact normal exceeds native range")?,
                    );
                    None
                }
            }
            DrawTerrainSweep::InitialOverlap { .. } => Some((
                "camera is waiting for its near-field terrain presentation to match the source"
                    .into(),
                GridPos(position),
                false,
            )),
            DrawTerrainSweep::Unavailable { reason, .. } => Some((reason, GridPos(result), false)),
            DrawTerrainSweep::Refused { reason, target, .. } => Some((reason, target, true)),
            DrawTerrainSweep::Invalid { reason, .. } => {
                targets.clear(camera);
                return Err(CameraMotionError::Invalid(reason));
            }
        };
        if let Some((reason, target, refused)) = preparation {
            let Some(source) = terrain.snapshot() else {
                targets.clear(camera);
                return Err("camera preparation has no admitted terrain source".into());
            };
            if source.frame() != frame {
                targets.clear(camera);
                return Err("camera preparation source belongs to a retired physics frame".into());
            }
            targets.request(
                camera,
                source,
                GridPos(start),
                target,
                radius + settings.skin_m,
            )?;
            return if refused {
                Err(CameraMotionError::Invalid(reason))
            } else {
                Err(CameraMotionError::Preparation { reason, target })
            };
        }
        if remaining.length() <= settings.sweep_tolerance_m {
            targets.clear(camera);
            return Ok(position);
        }
        if !lunco_physics::avian_backend_vector_is_valid(remaining) {
            targets.clear(camera);
            return Err("camera slide displacement exceeds native solver range".into());
        }
        // Avian owns multi-plane projection at the checked native boundary.
        // Source, prop and finest-render contacts constrain this same request.
        remaining = avian3d::prelude::MoveAndSlide::project_velocity(remaining.as_vec3(), &planes)
            .as_dvec3();
    }
    targets.clear(camera);
    Ok(position)
}

/// Admission facts for one straight physical camera step.
pub(crate) struct CameraSourceStep {
    pub position: DVec3,
    pub contact_normal: Option<DVec3>,
    pub escaping_terrain: bool,
}

/// Cast the physical sphere against native props and the full source. This owner
/// applies directional skin pullback; the shared movement loop owns projection.
pub(crate) fn cast_camera_source_step(
    start: DVec3,
    delta: DVec3,
    radius: f64,
    settings: &AvatarCollisionSettings,
    terrain: &lunco_terrain_surface::GridSurfaceQuery<'_, '_>,
    terrain_budget: usize,
    native: &lunco_physics::GridSpatialQuery<'_, '_>,
    filter: &SpatialQueryFilter,
) -> Result<CameraSourceStep, CameraMotionError> {
    use lunco_spatial::coords::GridPos;
    use lunco_terrain_surface::TerrainSphereSweep;
    let shape = camera_collider(radius)?;
    let length = delta.length();
    if length <= settings.sweep_tolerance_m {
        return Ok(CameraSourceStep {
            position: start,
            contact_normal: None,
            escaping_terrain: false,
        });
    }
    if !start.is_finite() || !length.is_finite() {
        return Err("camera source step exceeds finite coordinates".into());
    }
    let ray = delta / length;
    let cast_length = length + settings.skin_m;
    let direction = Dir3::new(ray.as_vec3()).map_err(|_| "camera motion direction is invalid")?;
    let prop = native.cast_shape_grid(
        &shape,
        GridPos(start),
        Quat::IDENTITY,
        direction,
        cast_length,
        0.0,
        filter,
    )?;
    if let Some(hit) = &prop {
        let separation = (hit.point2 - hit.point1).dot(hit.normal1.as_dvec3());
        if hit.distance == 0.0 && separation < -settings.sweep_tolerance_m {
            return Err(
                "camera starts inside a native prop; its pose must be moved outside the collider"
                    .into(),
            );
        }
    }
    let mut contact = prop.map(|hit| (f64::from(hit.distance), hit.normal1.as_dvec3()));
    let mut escaping_terrain = false;
    match terrain.sweep_sphere(
        GridPos(start),
        GridPos(start + ray * cast_length),
        radius,
        settings.sweep_tolerance_m,
        terrain_budget,
    ) {
        TerrainSphereSweep::Clear => {}
        TerrainSphereSweep::Hit(hit) => {
            let distance = hit.time * cast_length;
            if contact.is_none_or(|(previous, _)| distance < previous) {
                contact = Some((distance, hit.normal));
            }
        }
        TerrainSphereSweep::InitialOverlap { escape_allowed, .. } => {
            if !escape_allowed {
                return Err("camera starts inside terrain; explicit terrain-up movement is required to escape".into());
            }
            escaping_terrain = true;
        }
        TerrainSphereSweep::Unavailable { reason } => return Err(reason.into()),
    }
    let Some((distance, normal)) = contact else {
        return Ok(CameraSourceStep {
            position: start + delta,
            contact_normal: None,
            escaping_terrain,
        });
    };
    if !normal.is_finite()
        || !(0.5..=1.5).contains(&normal.length_squared())
        || !distance.is_finite()
        || distance < 0.0
        || distance > cast_length + settings.sweep_tolerance_m
    {
        return Err("camera volume cast returned invalid contact facts".into());
    }
    let normal = normal.normalize();
    let approach = -ray.dot(normal);
    let travel = if approach > 0.0 {
        (distance - settings.skin_m / approach - settings.sweep_tolerance_m).clamp(0.0, length)
    } else {
        0.0
    };
    Ok(CameraSourceStep {
        position: start + ray * travel,
        contact_normal: Some(normal),
        escaping_terrain,
    })
}

/// Realize camera movement in the active frame using one camera envelope and
/// one terrain source. Native casts exclude the separately sampled physics terrain.
pub(crate) fn move_avatar_with_collision(
    avatar: Entity,
    source_grid: Entity,
    cell: &CellCoord,
    transform: &Transform,
    desired_delta: DVec3,
    active_frame: Option<Entity>,
    native: Option<&lunco_physics::GridSpatialQuery<'_, '_>>,
    settings: &AvatarCollisionSettings,
    projection: &Projection,
    terrain: &lunco_terrain_surface::GridSurfaceQuery<'_, '_>,
    terrain_budget: Option<usize>,
    drawn: &lunco_terrain_surface::DrawTerrainQuery<'_, '_>,
    targets: &mut lunco_terrain_surface::TerrainCameraTargets,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), Without<Embodiment>>,
) -> Result<DVec3, CameraMotionError> {
    let frame = active_frame.ok_or("camera movement has no active physics frame")?;
    let native = native.ok_or("camera movement has no native prop query")?;
    let radius = settings.camera_radius(projection)?;
    let source_position = grid_absolute_seeded(avatar, Some(cell), transform, q_parents, q_grids)
        .ok_or("camera movement has no source-grid pose")?
        .0;
    if !source_position.is_finite() || !desired_delta.is_finite() {
        return Err("camera movement has invalid pose or displacement".into());
    }
    let source_to_physics = lunco_spatial::coords::grid_transform_between_grids(
        source_grid,
        frame,
        q_parents,
        q_grids,
        q_spatial,
    )
    .ok_or("camera movement cannot resolve its physics frame")?;
    let physics_to_source = lunco_spatial::coords::grid_transform_between_grids(
        frame,
        source_grid,
        q_parents,
        q_grids,
        q_spatial,
    )
    .ok_or("camera movement cannot resolve its source frame")?;
    let mut filter = SpatialQueryFilter::from_excluded_entities([avatar]);
    filter
        .excluded_entities
        .extend(terrain.native_terrain_colliders());
    filter.mask = avian3d::prelude::LayerMask(!NON_PHYSICAL_QUERY_LAYERS);
    let terrain_work = if terrain.has_terrain() {
        terrain_budget.ok_or("camera movement has no terrain sweep work budget")?
    } else {
        0
    };
    let result = resolve_camera_segment(
        avatar,
        frame,
        source_to_physics.transform_position(source_position),
        source_to_physics.transform_vector(desired_delta),
        radius,
        settings,
        terrain,
        terrain_work,
        drawn,
        targets,
        native,
        &filter,
    )?;
    Ok(physics_to_source.transform_position(result))
}

/// Apply one complete Grid-absolute position without duplicating the
/// `CellCoord`/local-Transform split at each avatar movement entry point.
pub(crate) fn write_avatar_grid_position(
    grid: &Grid,
    cell: &mut CellCoord,
    transform: &mut Transform,
    position: DVec3,
) {
    let (new_cell, new_transform) =
        local_pose_to_grid_storage(grid, position, transform.rotation.as_dquat());
    if *cell != new_cell {
        *cell = new_cell;
    }
    if transform.translation != new_transform.translation {
        transform.translation = new_transform.translation;
    }
}

/// Realize the avatar's authored flight-controller ports as local movement.
///
/// The controller reads `forward`, `side`, `up`, and `speed_boost` from the
/// shared port surface. It runs on the wall-clock interaction schedule so the
/// local camera remains usable while virtual simulation time is paused.
pub(crate) fn apply_fly(
    mut commands: Commands,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut CellCoord,
            &ChildOf,
            &InputPorts,
            &FreeFlightSettings,
            Has<FreeFlightCamera>,
            Has<SurfaceCamera>,
            Option<&SurfaceRelativeMode>,
            &Projection,
            Has<crate::scroll_transit::PendingCameraScroll>,
        ),
        (
            With<Embodiment>,
            With<LocalEmbodiment>,
            Without<Grid>,
            Without<lunco_camera_core::CameraPoseLock>,
        ),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Embodiment>>,
    gravity: Res<LocalGravityField>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    drag_mode: Option<Res<DragModeActive>>,
    mut collision: NavigationCollisionContext,
    workspace: Option<Res<WorkspaceResource>>,
    mut policy_error: Local<Option<String>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    if drag_mode.is_some_and(|drag| drag.active) {
        return;
    }
    let policy = match avatar_soil_collision_policy(workspace.as_deref()) {
        Ok(policy) => policy,
        Err(error) => {
            report_avatar_policy_error(
                &error,
                &mut policy_error,
                &mut diagnostics,
                "camera-locomotion",
            );
            return;
        }
    };
    let ctrl_pressed = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    for (
        entity,
        mut tf,
        mut cell,
        child_of,
        inputs,
        flight_settings,
        has_freeflight,
        has_surface_camera,
        surface_mode,
        projection,
        pending_scroll,
    ) in q_avatar.iter_mut()
    {
        let Ok(grid) = q_grids.get(child_of.0) else {
            continue;
        };
        let Some(current_pos) =
            grid_absolute_seeded(entity, Some(&cell), &tf, &q_parents, &q_grids)
                .map(|position| position.0)
        else {
            continue;
        };
        if !has_freeflight && !has_surface_camera && !ctrl_pressed {
            continue;
        }

        let forward = inputs.cmd("forward") as f32;
        let side = inputs.cmd("side") as f32;
        let elevation = inputs.cmd("up") as f32;
        let deadzone = flight_settings.input_deadzone as f32;
        let boost = if inputs.cmd("speed_boost") > flight_settings.boost_threshold {
            flight_settings.boost_multiplier
        } else {
            1.0
        };
        if forward.abs() < deadzone && side.abs() < deadzone && elevation.abs() < deadzone {
            if !pending_scroll {
                collision.targets.clear(entity);
            }
            clear_camera_preparation(&mut diagnostics, "camera-locomotion", &mut policy_error);
            continue;
        }

        if pending_scroll {
            commands
                .entity(entity)
                .remove::<crate::scroll_transit::PendingCameraScroll>();
            clear_camera_preparation(&mut diagnostics, "camera-scroll", &mut None);
        }
        let up_dir = if surface_mode.is_some() {
            let Some(up) =
                gravity_up_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial)
            else {
                continue;
            };
            up
        } else {
            Vec3::Y
        };
        let move_vec = camera_move_direction(&tf, forward, side, elevation, up_dir);
        let desired_delta =
            move_vec.as_dvec3() * flight_settings.speed_mps * boost * time.delta_secs_f64();
        let next_pos = if policy == AvatarSoilCollisionPolicy::ThroughSoilAllowed {
            collision.targets.clear(entity);
            current_pos + desired_delta
        } else {
            let next_pos = match move_avatar_with_collision(
                entity,
                child_of.parent(),
                &cell,
                &tf,
                desired_delta,
                collision.active_frame.as_deref().map(|frame| frame.0),
                collision.native.as_ref(),
                &collision.settings,
                projection,
                &collision.terrain,
                collision
                    .budget
                    .as_deref()
                    .map(|budget| budget.max_intervals),
                &collision.drawn,
                &mut collision.targets,
                &q_parents,
                &q_grids,
                &q_spatial,
            ) {
                Ok(position) => position,
                Err(error) => {
                    report_camera_motion_error(
                        &error,
                        &mut policy_error,
                        &mut diagnostics,
                        "camera-locomotion",
                    );
                    continue;
                }
            };
            next_pos
        };
        if policy_error.take().is_some() {
            lunco_camera_core::replace_camera_diagnostic(
                &mut diagnostics,
                "camera-locomotion",
                "CameraNavigation",
                None,
            );
        }
        write_avatar_grid_position(grid, &mut cell, &mut tf, next_pos);
    }
}

fn retire_camera_navigation(
    camera: Entity,
    targets: &mut lunco_terrain_surface::TerrainCameraTargets,
    local: &Query<(), With<LocalEmbodiment>>,
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    targets.clear(camera);
    if local.get(camera).is_ok() {
        if let Some(diagnostics) = diagnostics.as_deref_mut() {
            for producer in ["camera-locomotion", "camera-scroll", "camera-follow"] {
                diagnostics.replace_producer(producer, std::iter::empty());
            }
        }
    }
}

pub(crate) fn retire_navigation<T: Component>(
    event: On<Remove<T>>,
    mut targets: ResMut<lunco_terrain_surface::TerrainCameraTargets>,
    local: Query<(), With<LocalEmbodiment>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    retire_camera_navigation(event.entity, &mut targets, &local, &mut diagnostics);
}

pub(crate) fn lock_navigation(
    event: On<Add<lunco_camera_core::CameraPoseLock>>,
    mut targets: ResMut<lunco_terrain_surface::TerrainCameraTargets>,
    local: Query<(), With<LocalEmbodiment>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    retire_camera_navigation(event.entity, &mut targets, &local, &mut diagnostics);
}
