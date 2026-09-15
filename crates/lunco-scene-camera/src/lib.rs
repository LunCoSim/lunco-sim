//! Render-free camera commands for the active scene frame.
//!
//! This package owns camera command policy and focus transactions. It is kept
//! separate from the general scene mutation package so camera changes do not
//! rebuild spawn, transform, authoring, or document command code.
//!
//! Hosts that expose `FocusEntityByPath` must install
//! `lunco_scene_selection::SceneSelectionPlugin` alongside this plugin. The
//! selection resource is a shared scene contract, not camera-owned state.

use bevy::prelude::*;
use big_space::prelude::Grid;
use lunco_core::{on_command, register_commands, Command};
use lunco_scene_selection::SelectedEntities;
use lunco_usd_bevy_scene::UsdPrimPath;

/// Point the free-flight avatar camera at an entity (by API id), from a fixed
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

/// A focus request recorded by [`on_focus_entity_by_id`] and applied by
/// [`apply_pending_focus`] at the start of the NEXT frame (`First` schedule).
///
/// The command observer fires wherever the API dispatcher happens to sit in
/// the frame, so this transaction is applied from `First` after any queued
/// orbit-return commands have flushed. Spatial math uses the authoritative
/// `(CellCoord, Transform)` chain through `lunco_spatial::coords`; derived
/// `GlobalTransform` is never a camera-placement input.
#[derive(Resource, Debug, Clone, Copy)]
pub struct PendingFocus {
    pub target: Entity,
    pub distance: f32,
}

fn replace_focus_diagnostic(
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    message: Option<String>,
) {
    if let Some(diagnostics) = diagnostics.as_deref_mut() {
        diagnostics.replace_producer(
            "scene-focus",
            message.map(|message| lunco_core::RuntimeDiagnostic {
                code: "scene-focus".to_string(),
                severity: lunco_core::DiagnosticSeverity::Error,
                producer: "scene-focus".to_string(),
                subject: "PendingFocus".to_string(),
                message,
            }),
        );
    }
}

/// Observer: validate + record the focus; all spatial math happens in
/// [`apply_pending_focus`].
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
    commands.insert_resource(PendingFocus {
        target,
        distance: cmd.distance,
    });
    info!(
        "FOCUS_ENTITY: queued focus on {target:?} at {} m",
        cmd.distance
    );
}

/// Applies a [`PendingFocus`] from authoritative BigSpace poses (`First`
/// schedule — see the type doc).
pub fn apply_pending_focus(
    pending: Option<Res<PendingFocus>>,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut big_space::prelude::CellCoord,
            &ChildOf,
            Option<&mut lunco_avatar_core::camera::FreeFlightCamera>,
            Has<lunco_avatar_core::camera::OrbitViewReturn>,
        ),
        (With<lunco_core::Avatar>, With<lunco_core::LocalAvatar>),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<
        (Option<&big_space::prelude::CellCoord>, &Transform),
        Without<lunco_core::Avatar>,
    >,
    q_celestial: Query<(), With<lunco_celestial::CelestialBody>>,
    q_celestial_decl: Query<(), With<lunco_celestial_spatial::CelestialBodyDecl>>,
    q_children: Query<&Children>,
    mut commands: Commands,
    mut orbital_pin: Option<ResMut<lunco_celestial_spatial::OrbitalViewPin>>,
    local_avatar: Option<Res<lunco_core::TheLocalAvatar>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let Some(pending) = pending else { return };
    let (target, distance) = (pending.target, pending.distance);
    // Celestial bodies are ORBIT-scale targets: hand them to the avatar's
    // `FocusTarget` flow (OrbitCamera flies in the body's explicit inertial
    // view grid with current-region arrival). Local framing stays for
    // metre-scale subjects (wheels, rovers, props).
    let mut is_celestial = q_celestial.get(target).is_ok() || q_celestial_decl.get(target).is_ok();
    let mut pending = vec![target];
    for _ in 0..8 {
        let mut next = Vec::new();
        for parent in pending.drain(..) {
            if let Ok(children) = q_children.get(parent) {
                for child in children.iter() {
                    if q_celestial.get(child).is_ok() || q_celestial_decl.get(child).is_ok() {
                        is_celestial = true;
                    }
                    next.push(child);
                }
            }
        }
        if is_celestial || next.is_empty() {
            break;
        }
        pending = next;
    }
    if is_celestial {
        commands.remove_resource::<PendingFocus>();
        commands.trigger(lunco_avatar_core::commands::FocusTarget {
            avatar: None,
            target,
        });
        info!("FOCUS_ENTITY: celestial target {target:?} → orbit focus");
        return;
    }
    // A local target is authored in the pre-orbit scene frame. Restore the
    // avatar's exact orbit-entry transaction first, then retry this retained
    // focus next First frame. Applying a local delta while the camera is still
    // in an inertial body grid mixes semantic frames.
    if let Some(avatar) = local_avatar.as_deref().and_then(|slot| slot.0) {
        if q_avatar
            .get(avatar)
            .is_ok_and(|(_, _, _, _, _, orbit_return)| orbit_return)
        {
            commands.trigger(lunco_avatar_core::commands::ReleaseVessel { target: avatar });
            info!("FOCUS_ENTITY: restored pre-orbit frame; local focus retries next frame");
            return;
        }
    }
    if let Some(pin) = orbital_pin.as_mut() {
        pin.active = false;
    }
    commands.remove_resource::<PendingFocus>();
    let Some(avatar_ent) = local_avatar.as_deref().and_then(|slot| slot.0) else {
        let message = "no authoritative LocalAvatar is available for local focus".to_string();
        warn!("FOCUS_ENTITY: {message}");
        replace_focus_diagnostic(&mut diagnostics, Some(message));
        return;
    };
    let Ok((avatar_ent, mut tf, mut cell, child_of, ff_opt, _)) = q_avatar.get_mut(avatar_ent)
    else {
        let message =
            format!("authoritative LocalAvatar {avatar_ent:?} has no complete focus state");
        warn!("FOCUS_ENTITY: {message}");
        replace_focus_diagnostic(&mut diagnostics, Some(message));
        return;
    };
    replace_focus_diagnostic(&mut diagnostics, None);
    let Ok(grid) = q_grids.get(child_of.parent()) else {
        let message = format!(
            "authoritative LocalAvatar {avatar_ent:?} is not parented directly under a BigSpace Grid"
        );
        warn!("FOCUS_ENTITY: {message}");
        replace_focus_diagnostic(&mut diagnostics, Some(message));
        return;
    };
    let avatar_pos = grid.grid_position_double(&cell, &tf);
    let target_pos = if target == avatar_ent {
        avatar_pos
    } else {
        let Some((target_pos, _)) = lunco_spatial::coords::pose_in_grid(
            target,
            child_of.parent(),
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            let message =
                format!("target {target:?} has no complete pose in the avatar's BigSpace frame");
            warn!("FOCUS_ENTITY: {message}");
            replace_focus_diagnostic(&mut diagnostics, Some(message));
            return;
        };
        target_pos
    };
    let dist = if distance > 0.1 { distance } else { 6.0 };
    // Camera sits mostly to the SIDE (+X, the wheel axle direction → we see
    // the spoke face) plus a little up and forward. (Celestial targets never
    // reach here — they take the orbit-focus early return above.)
    let dir = Vec3::new(1.0, 0.4, 0.25).normalize();
    let offset = dir * dist;
    // Re-split the complete target-relative pose through the owning Grid.
    // This preserves cell precision even when the camera was previously in an
    // inertial orbit grid; no render-space value participates in placement.
    let (new_cell, new_translation) = grid.translation_to_grid(target_pos + offset.as_dvec3());
    cell.set_if_neq(new_cell);
    if tf.translation != new_translation {
        tf.translation = new_translation;
    }
    // Aim back along the framing offset (camera → target).
    let d = (-offset).normalize();
    let (yaw, pitch) = ((-d.x).atan2(-d.z), d.y.clamp(-1.0, 1.0).asin());
    match ff_opt {
        // Free-flight rebuilds rotation from yaw/pitch every frame (YXZ euler), so
        // when it's present we must set those rather than the Transform rotation.
        Some(mut ff) => {
            ff.yaw = yaw;
            ff.pitch = pitch;
        }
        // Non-freeflight camera mode (orbit/spring/surface): the framing is
        // AUTHORITATIVE — leaving the old mode attached lets its system fly
        // the camera right back (an OrbitCamera on Earth reclaimed the camera
        // one frame after "focus rover" and the view never returned). Strip
        // the mode and reinstate free flight at the computed aim.
        None => {
            tf.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
            commands
                .entity(avatar_ent)
                .remove::<lunco_avatar_core::camera::OrbitCamera>()
                .remove::<lunco_avatar_core::camera::SpringArmCamera>()
                .remove::<lunco_avatar_core::camera::SurfaceCamera>()
                .remove::<lunco_avatar_core::camera::SurfaceRelativeMode>()
                .try_insert(lunco_avatar_core::camera::FreeFlightCamera {
                    yaw,
                    pitch,
                    damping: None,
                });
        }
    }
    info!(
        "FOCUS_ENTITY: framed target={target:?} at {:.1} m (avatar={avatar_ent:?})",
        dist
    );
}

/// Aim the free-flight avatar camera: place it at `eye` and look at `target`
/// (both absolute world-space). The flexible primitive — the client computes the
/// angle (e.g. approach a wheel from its outboard side) and distance.
///
/// Authoritative: whatever camera mode the avatar is in (orbit focus on a
/// planet, spring-arm follow, surface mode), this strips it and reinstates a
/// `FreeFlightCamera` at the requested pose — an API client asking for a
/// specific view must always get it. `eye` and `target` speak the semantic
/// [`lunco_spatial::ActivePhysicsFrame`]; the concrete grid is resolved from that
/// resource so a previous orbit focus or a canonical render-only grid cannot
/// put the camera in a different frame.
#[Command(default)]
pub struct SetCameraLookAt {
    pub eye: Vec3,
    pub target: Vec3,
}

/// Observer for [`SetCameraLookAt`].
#[on_command(SetCameraLookAt)]
pub fn on_set_camera_look_at(
    trigger: On<SetCameraLookAt>,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut big_space::prelude::CellCoord,
            &ChildOf,
            Option<&mut lunco_avatar_core::camera::FreeFlightCamera>,
        ),
        (With<lunco_core::Avatar>, With<lunco_core::LocalAvatar>),
    >,
    active_frame: Res<lunco_spatial::ActivePhysicsFrame>,
    q_grids: Query<&Grid>,
    mut commands: Commands,
    mut orbital_pin: Option<ResMut<lunco_celestial_spatial::OrbitalViewPin>>,
    local_avatar: Option<Res<lunco_core::TheLocalAvatar>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let cmd = trigger.event();
    let Some(entity) = local_avatar.as_deref().and_then(|slot| slot.0) else {
        let message = "no authoritative LocalAvatar is available for SetCameraLookAt".to_string();
        warn!("SET_CAMERA: {message}");
        replace_focus_diagnostic(&mut diagnostics, Some(message));
        return;
    };
    let Ok((entity, mut tf, mut cell, child_of, ff_opt)) = q_avatar.get_mut(entity) else {
        let message = format!("authoritative LocalAvatar {entity:?} has no complete camera state");
        warn!("SET_CAMERA: {message}");
        replace_focus_diagnostic(&mut diagnostics, Some(message));
        return;
    };
    replace_focus_diagnostic(&mut diagnostics, None);
    // Explicit camera coordinates use the same active physics frame as
    // MoveEntity and route projection. Never select a grid by marker/component
    // type here: render and physics roots may legitimately differ.
    if let Some(pin) = orbital_pin.as_mut() {
        pin.active = false;
    }
    let root = active_frame.0;
    let Ok(grid) = q_grids.get(root) else {
        warn!(
            ?root,
            "SET_CAMERA: active physics frame has no Grid component"
        );
        return;
    };
    let (new_cell, new_translation) = grid.translation_to_grid(cmd.eye.as_dvec3());
    if child_of.parent() == root {
        cell.set_if_neq(new_cell);
        if tf.translation != new_translation {
            tf.translation = new_translation;
        }
    } else {
        lunco_spatial::attach::migrate_to_grid(
            &mut commands,
            entity,
            root,
            new_cell,
            Transform::from_translation(new_translation).with_rotation(tf.rotation),
        );
    }
    // An explicit world-space camera command starts a new free-flight view; it
    // does not retain a hidden return transaction or surface gravity binding.
    commands
        .entity(entity)
        .remove::<lunco_avatar_core::camera::OrbitViewReturn>()
        .remove::<lunco_avatar_core::camera::SurfaceRelativeMode>()
        .remove::<lunco_environment::GravityBody>();
    let look = cmd.target - cmd.eye;
    let (yaw, pitch) = if look.length() > 1e-4 {
        let d = look.normalize();
        ((-d.x).atan2(-d.z), d.y.clamp(-1.0, 1.0).asin())
    } else {
        let (y, p, _) = tf.rotation.to_euler(EulerRot::YXZ);
        (y, p)
    };
    if let Some(mut ff) = ff_opt {
        ff.yaw = yaw;
        ff.pitch = pitch;
    } else {
        commands
            .entity(entity)
            .remove::<lunco_avatar_core::camera::OrbitCamera>()
            .remove::<lunco_avatar_core::camera::OrbitViewReturn>()
            .remove::<lunco_avatar_core::camera::SpringArmCamera>()
            .remove::<lunco_avatar_core::camera::SurfaceCamera>()
            .remove::<lunco_avatar_core::camera::SurfaceRelativeMode>()
            .remove::<lunco_environment::GravityBody>()
            .try_insert(lunco_avatar_core::camera::FreeFlightCamera {
                yaw,
                pitch,
                damping: None,
            });
    }
    info!(
        "SET_CAMERA: eye=({:.2},{:.2},{:.2}) target=({:.2},{:.2},{:.2})",
        cmd.eye.x, cmd.eye.y, cmd.eye.z, cmd.target.x, cmd.target.y, cmd.target.z
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
        app.insert_resource(lunco_core::TheLocalAvatar::default());

        let avatar = app
            .world_mut()
            .spawn((
                lunco_core::Avatar,
                lunco_core::LocalAvatar,
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
                ChildOf(active_physics_grid),
                lunco_avatar_core::camera::FreeFlightCamera {
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                },
            ))
            .id();

        app.world_mut().trigger(SetCameraLookAt {
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
        let composed_y = cell.y as f64 * 2_000.0 + translation.y as f64;
        assert!((composed_y - 2_500.0).abs() < 1.0e-3);
        assert_ne!(canonical_render_grid, active_physics_grid);
    }

    #[test]
    fn focus_uses_authoritative_grid_pose_not_render_global_transform() {
        use super::*;
        use bevy::math::DVec3;
        use big_space::prelude::{CellCoord, Grid};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);

        let grid = app
            .world_mut()
            .spawn((
                Grid::new(1_000.0, 0.0),
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        let target = app
            .world_mut()
            .spawn((
                CellCoord::new(2, 0, -1),
                Transform::from_xyz(25.0, 3.0, -10.0),
                // This deliberately stale render pose must not affect focus.
                GlobalTransform::from(Transform::from_xyz(-1.0e11, 2.0e11, 3.0e11)),
                ChildOf(grid),
            ))
            .id();
        let avatar = app
            .world_mut()
            .spawn((
                lunco_core::Avatar,
                lunco_core::LocalAvatar,
                CellCoord::new(1, 0, 0),
                Transform::from_xyz(4.0, 6.0, 8.0),
                GlobalTransform::from(Transform::from_xyz(7.0e10, -8.0e10, 9.0e10)),
                ChildOf(grid),
                lunco_avatar_core::camera::FreeFlightCamera {
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                },
            ))
            .id();

        app.insert_resource(PendingFocus {
            target,
            distance: 6.0,
        });
        app.insert_resource(lunco_core::TheLocalAvatar(Some(avatar)));
        app.add_systems(bevy::app::First, apply_pending_focus);
        app.update();

        let grid = app.world().get::<Grid>(grid).unwrap();
        let target_pos = DVec3::new(2_025.0, 3.0, -1_010.0);
        let offset = Vec3::new(1.0, 0.4, 0.25).normalize() * 6.0;
        let actual = {
            let cell = app.world().get::<CellCoord>(avatar).unwrap();
            let transform = app.world().get::<Transform>(avatar).unwrap();
            grid.grid_position_double(cell, transform)
        };
        assert!((actual - (target_pos + offset.as_dvec3())).length() < 1.0e-3);

        let freeflight = app
            .world()
            .get::<lunco_avatar_core::camera::FreeFlightCamera>(avatar)
            .unwrap();
        let direction = (-offset).normalize();
        assert!((freeflight.yaw - (-direction.x).atan2(-direction.z)).abs() < 1.0e-6);
        assert!((freeflight.pitch - direction.y.asin()).abs() < 1.0e-6);
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
        app.add_systems(bevy::app::First, apply_pending_focus);
    }
}
