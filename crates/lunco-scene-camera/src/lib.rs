//! Render-free camera commands for the active scene frame.
//!
//! This package owns scene-facing camera command adapters. It resolves stable
//! scene identities and authored paths, then hands generic focus requests to
//! `lunco-camera-core`; camera-rig policy and spatial realization stay with the
//! consuming camera runtime.
//!
//! Hosts that expose `FocusEntityByPath` must install
//! `lunco_scene_selection::SceneSelectionPlugin` alongside this plugin. The
//! selection resource is a shared scene contract, not camera-owned state.

use bevy::prelude::*;
use big_space::prelude::Grid;
use lunco_camera_core::{CameraPoseMode, SetCameraLookAt};
use lunco_core::{Command, on_command, register_commands};
use lunco_render::SceneCamera;
use lunco_scene_selection::SelectedEntities;
use lunco_usd_bevy_scene::UsdPrimPath;

/// Point the active presentation camera at an entity (by API id), from a fixed
/// side-on-and-above angle at `distance` metres. Lets API clients (MCP tools,
/// automated screenshots) frame a subject — e.g. a wheel — without hand-driving
/// the camera. `entity_id` is the API id from `ListEntities` (a `u64`), same as
/// the scene's entity-mutation commands.
#[Command(default)]
pub struct FocusEntityById {
    /// API-stable global entity ID from `ListEntities`, resolved to the live
    /// Bevy entity by `ApiEntityRegistry`.
    pub entity_id: u64,
    /// Camera distance from the target, metres. `<= 0` → default 6.
    pub distance: f32,
}

/// Set the render-free runtime focus to the composed USD prim at `path`.
///
/// This is separate from the editor's `SelectUsdPrim`: a headless
/// recorder has no Inspector, gizmo, or picking state to maintain, but
/// runtime-authored surfaces still need a stable subject for scoped telemetry.
/// The authored USD path remains stable across entity ids and scene reloads.
#[Command(default)]
pub struct FocusEntityByPath {
    /// Absolute composed USD prim path (for example `/World/Lander`).
    pub path: String,
}

#[on_command(FocusEntityByPath)]
pub fn on_focus_entity_by_path(
    trigger: On<FocusEntityByPath>,
    q_paths: Query<(Entity, &UsdPrimPath)>,
    mut selected: ResMut<SelectedEntities>,
) {
    let cmd = trigger.event();
    let Some(target) = q_paths
        .iter()
        .find(|(_, prim)| prim.path == cmd.path)
        .map(|(entity, _)| entity)
    else {
        warn!("FOCUS_ENTITY_BY_PATH: no composed prim at `{}`", cmd.path);
        return;
    };

    if selected.entities != [target] {
        selected.entities.clear();
        selected.entities.push(target);
    }
    info!("FOCUS_ENTITY_BY_PATH: focused `{}` ({target:?})", cmd.path);
}

/// Observer: validate and queue the focus; spatial math happens when the active
/// camera realization consumes [`lunco_camera_core::PendingFocus`] at its frame
/// boundary.
#[on_command(FocusEntityById)]
pub fn on_focus_entity_by_id(
    trigger: On<FocusEntityById>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    mut commands: Commands,
) {
    let cmd = trigger.event();
    let global_id = lunco_core::GlobalEntityId::from_raw(cmd.entity_id);
    let Some(target) = registry.resolve(&global_id) else {
        warn!("FOCUS_ENTITY: no api_id={} in registry", cmd.entity_id);
        return;
    };
    commands.insert_resource(lunco_camera_core::PendingFocus {
        target,
        distance: cmd.distance,
    });
    info!(
        "FOCUS_ENTITY: queued focus on {target:?} at {} m",
        cmd.distance
    );
}

/// Observer for [`SetCameraLookAt`].
#[on_command(SetCameraLookAt)]
pub fn on_set_camera_look_at(
    trigger: On<SetCameraLookAt>,
    mut q_camera: Query<
        (
            &mut Transform,
            Option<&mut big_space::prelude::CellCoord>,
            &ChildOf,
            Option<&mut CameraPoseMode>,
        ),
        With<SceneCamera>,
    >,
    active_frame: Res<lunco_spatial::ActivePhysicsFrame>,
    q_grids: Query<&Grid>,
    mut commands: Commands,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let cmd = trigger.event();
    let entity = cmd.camera;
    let Ok((mut tf, cell, child_of, pose)) = q_camera.get_mut(cmd.camera) else {
        let message = format!("camera {:?} has no complete spatial pose", cmd.camera);
        warn!("SET_CAMERA: {message}");
        lunco_camera_core::replace_camera_diagnostic(
            &mut diagnostics,
            "scene-focus",
            "PendingFocus",
            Some(message),
        );
        return;
    };
    let current_pose = pose.as_deref().copied().unwrap_or_default();
    if matches!(current_pose, CameraPoseMode::Mounted | CameraPoseMode::Path) {
        let owner = match current_pose {
            CameraPoseMode::Mounted => "mounted camera follower",
            CameraPoseMode::Path => "authored camera path",
            _ => unreachable!(),
        };
        let message = format!("camera {entity:?} pose is owned by {owner}");
        warn!("SET_CAMERA: {message}");
        lunco_camera_core::replace_camera_diagnostic(
            &mut diagnostics,
            "scene-focus",
            "PendingFocus",
            Some(message),
        );
        return;
    }
    lunco_camera_core::replace_camera_diagnostic(
        &mut diagnostics,
        "scene-focus",
        "PendingFocus",
        None,
    );
    // Explicit camera coordinates use the same active physics frame as
    // MoveEntity and route projection. Never select a grid by marker/component
    // type here: render and physics roots may legitimately differ.
    let root = active_frame.0;
    let Ok(grid) = q_grids.get(root) else {
        let message = format!("active physics frame {root:?} has no Grid component");
        warn!("SET_CAMERA: {message}");
        lunco_camera_core::replace_camera_diagnostic(
            &mut diagnostics,
            "scene-focus",
            "PendingFocus",
            Some(message),
        );
        return;
    };
    let look = cmd.target - cmd.eye;
    let (yaw, pitch) = if look.length() > 1e-4 {
        let d = look.normalize();
        ((-d.x).atan2(-d.z), d.y.clamp(-1.0, 1.0).asin())
    } else {
        let (y, p, _) = tf.rotation.to_euler(EulerRot::YXZ);
        (y, p)
    };
    let rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
    let (new_cell, new_transform) = lunco_spatial::attach::local_pose_to_grid_storage(
        grid,
        cmd.eye.as_dvec3(),
        rotation.as_dquat(),
    );
    if child_of.parent() == root {
        if let Some(mut cell) = cell {
            cell.set_if_neq(new_cell);
            tf.set_if_neq(new_transform);
        } else {
            lunco_spatial::attach::migrate_to_grid(
                &mut commands,
                entity,
                root,
                new_cell,
                new_transform,
            );
        }
    } else {
        lunco_spatial::attach::migrate_to_grid(
            &mut commands,
            entity,
            root,
            new_cell,
            new_transform,
        );
    }

    // A direct pose command starts an explicit camera view. Clear interactive
    // mode state and let the generic pose lock fence every competing writer.
    if let Some(mut pose) = pose {
        *pose = CameraPoseMode::Explicit;
    } else {
        commands.entity(entity).try_insert(CameraPoseMode::Explicit);
    }
    commands
        .entity(entity)
        .try_insert(lunco_camera_core::CameraPoseLock);
    info!(
        "SET_CAMERA: camera={:?} eye=({:.2},{:.2},{:.2}) target=({:.2},{:.2},{:.2})",
        cmd.camera, cmd.eye.x, cmd.eye.y, cmd.eye.z, cmd.target.x, cmd.target.y, cmd.target.z
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn set_camera_uses_the_active_physics_frame_for_noncanonical_grid() {
        use super::*;
        use big_space::prelude::{CellCoord, Grid};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_observer(on_set_camera_look_at);

        let canonical_render_grid = app
            .world_mut()
            .spawn((
                Grid::new(2_000.0, 0.0),
                lunco_spatial::WorldGrid,
                GlobalTransform::default(),
            ))
            .id();
        let active_physics_grid = app
            .world_mut()
            .spawn((Grid::new(2_000.0, 0.0), GlobalTransform::default()))
            .id();
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(active_physics_grid));
        let avatar = app
            .world_mut()
            .spawn((
                SceneCamera::default(),
                CameraPoseMode::Authored,
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
                ChildOf(active_physics_grid),
                lunco_camera_core::FreeFlightCamera {
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                },
            ))
            .id();

        app.world_mut().trigger(SetCameraLookAt {
            camera: avatar,
            eye: Vec3::new(0.0, 2_500.0, 0.0),
            target: Vec3::ZERO,
        });
        app.update();

        assert_eq!(
            app.world().get::<ChildOf>(avatar).unwrap().parent(),
            active_physics_grid,
            "camera placement must not migrate into the render-only WorldGrid"
        );
        let cell = *app.world().get::<CellCoord>(avatar).unwrap();
        let translation = app.world().get::<Transform>(avatar).unwrap().translation;
        let active_grid = app.world().get::<Grid>(active_physics_grid).unwrap();
        let composed_y = active_grid
            .grid_position_double(&cell, &Transform::from_translation(translation))
            .y;
        assert!((composed_y - 2_500.0).abs() < 1.0e-3);
        assert_ne!(canonical_render_grid, active_physics_grid);
        assert_eq!(
            app.world().get::<CameraPoseMode>(avatar),
            Some(&CameraPoseMode::Explicit)
        );
        assert!(
            app.world()
                .get::<lunco_camera_core::CameraPoseLock>(avatar)
                .is_some()
        );
    }
}

register_commands!(
    on_focus_entity_by_id,
    on_focus_entity_by_path,
    on_set_camera_look_at,
);

/// Installs the scene camera command and focus systems.
pub struct SceneCameraCommandPlugin;

impl Plugin for SceneCameraCommandPlugin {
    fn build(&self, app: &mut App) {
        register_all_commands(app);
    }
}
