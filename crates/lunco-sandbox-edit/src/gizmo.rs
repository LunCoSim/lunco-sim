//! Transform gizmo integration.
//!
//! Uses `transform-gizmo-bevy` which **automatically applies transforms** to
//! entities with `GizmoTarget`. This module handles:
//! - Making bodies kinematic during gizmo drag
//! - Freezing the Floating Origin to break feedback loops with camera follow
//! - Disabling physics interpolation during manual dragging
//! - Restoring dynamic bodies and origin tracking when drag ends
//!
//! **Architectural Note**: This module provides the "Golden Path" for 
//! high-precision manual editing. It ensures the coordinate system 
//! remains stable by temporarily pausing origin re-centering.

use bevy::prelude::*;
use bevy::math::DVec3;
use bevy::camera::RenderTarget;
use big_space::prelude::FloatingOrigin;
use avian3d::prelude::{LinearVelocity, RigidBody, TranslationInterpolation, RotationInterpolation};
use transform_gizmo_bevy::{GizmoCamera, GizmoDragStarted, GizmoDragging, GizmoTarget};

/// Tracks the previous parent-local position and metadata for drag lifecycle.
#[derive(Component)]
pub struct GizmoPrevPos {
    /// Parent-local position in the previous frame (meters).
    pub local_pos: DVec3,
    /// Original RigidBody type before drag started.
    pub original_body: RigidBody,
    /// Whether the entity had TranslationInterpolation.
    pub had_translation_interpolation: bool,
    /// Whether the entity had RotationInterpolation.
    pub had_rotation_interpolation: bool,
}

/// Mirrors each `GizmoTarget`'s active state onto the core
/// [`lunco_core::GizmoDragging`] marker, so render/sim crates (e.g. the avatar
/// camera-follow systems) can react to a drag **without** depending on
/// `transform-gizmo-bevy`. This is the only place the marker is written.
pub fn sync_gizmo_dragging_marker(
    mut commands: Commands,
    q: Query<(Entity, &GizmoTarget)>,
    mut drag_mode: ResMut<lunco_core::DragModeActive>,
) {
    let mut any_active = false;
    for (e, gt) in &q {
        if gt.is_active() {
            any_active = true;
            commands.entity(e).insert(lunco_core::GizmoDragging);
        } else {
            commands.entity(e).remove::<lunco_core::GizmoDragging>();
        }
    }
    // Single writer of `DragModeActive`: possession (plain-click) is blocked ONLY
    // while a gizmo handle is actively dragged — not merely because something is
    // selected. So Shift-selecting an object just highlights it; you can still
    // plain-click to possess a rover.
    drag_mode.active = any_active;
}

/// Makes the selected entity kinematic and freezes the coordinate system when gizmo drag starts.
pub fn capture_gizmo_start(
    gizmo_targets: Query<(Entity, &GizmoTarget)>,
    q_rigid_bodies: Query<&RigidBody>,
    q_prev_pos: Query<&GizmoPrevPos>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    q_interpolation: Query<(Has<TranslationInterpolation>, Has<RotationInterpolation>)>,
    q_floating_origin: Query<Entity, With<FloatingOrigin>>,
    mut commands: Commands,
) {
    let mut captured_any = false;
    for (entity, gizmo_target) in gizmo_targets.iter() {
        if !gizmo_target.is_active() { continue; }
        if q_prev_pos.get(entity).is_ok() { continue; }
        captured_any = true;

        // 2. DISABLE INTERPOLATION
        // Remove interpolation components so the visual mesh doesn't "fight" the gizmo.
        let (had_translation, had_rotation) = q_interpolation.get(entity).unwrap_or((false, false));
        if had_translation { commands.entity(entity).remove::<TranslationInterpolation>(); }
        if had_rotation { commands.entity(entity).remove::<RotationInterpolation>(); }

        let original_body = q_rigid_bodies.get(entity).copied().unwrap_or(RigidBody::Dynamic);

        // Resolve initial parent-local position.
        let Ok((_, tf)) = q_spatial.get(entity) else { continue; };
        let local_pos = tf.translation.as_dvec3();

        info!("GIZMO: drag started for {:?}, local_pos={:?}", entity, local_pos);

        commands.entity(entity)
            .insert(RigidBody::Kinematic)
            .insert(GizmoPrevPos { 
                local_pos, 
                original_body,
                had_translation_interpolation: had_translation,
                had_rotation_interpolation: had_rotation,
            });
    }

    if captured_any {
        // 1. FREEZE COORDINATE SYSTEM
        // Remove FloatingOrigin from the camera. This stops big_space from shifting 
        // the world while we drag, breaking the positive feedback loop with the camera.
        for cam_ent in q_floating_origin.iter() {
            commands.entity(cam_ent).remove::<FloatingOrigin>();
            info!("GIZMO: freezing FloatingOrigin on camera {:?}", cam_ent);
        }
    }
}



/// Syncs Avian `Position` and computes velocity from local coordinates.
pub fn sync_gizmo_transforms(
    gizmo_targets: Query<(Entity, &GizmoTarget)>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    mut q_position: Query<&mut avian3d::physics_transform::Position>,
    mut q_rotation: Query<&mut avian3d::physics_transform::Rotation>,
    mut q_lin_vel: Query<&mut LinearVelocity>,
    mut q_prev_pos: Query<&mut GizmoPrevPos>,
    time: Res<Time>,
) {
    for (entity, gizmo_target) in gizmo_targets.iter() {
        if !gizmo_target.is_active() { continue; }

        let Ok((_, tf)) = q_spatial.get(entity) else { continue; };
        let local_pos = tf.translation.as_dvec3();

        if let Ok(mut pos) = q_position.get_mut(entity) {
            pos.0 = local_pos;
        }
        
        if let Ok(mut rot) = q_rotation.get_mut(entity) {
            rot.0 = tf.rotation.as_dquat();
        }

        // `restore_dragged_transform` clamps the mesh back to `prev.local_pos`
        // every frame (to cancel Avian's integrator writeback), so `prev` MUST
        // advance to the gizmo's current position every frame — including while
        // PAUSED. When paused, time is frozen and `delta_secs()` is 0; the old
        // `if dt > 1e-6` gate wrapped the whole block, so `prev` went stale and
        // the restore snapped the object back to its drag-start spot — the gizmo
        // couldn't move anything while paused. Only the velocity estimate (which
        // drags joint-coupled child bodies along and is meaningless at dt = 0)
        // stays gated on dt.
        if let Ok(mut prev) = q_prev_pos.get_mut(entity) {
            let dt = time.delta_secs();
            if dt > 1e-6 {
                let delta = local_pos - prev.local_pos;
                if let Ok(mut lin_vel) = q_lin_vel.get_mut(entity) {
                    lin_vel.0 = delta / dt as f64;
                }
            }
            prev.local_pos = local_pos;
        }
    }
}

/// TODO: Architectural Workaround for User vs. Physics Authority Conflict
///
/// **Why we have this:**
/// During active user-drag (gizmo), the visual editor (`transform-gizmo-bevy`)
/// is the absolute authority on `Transform`. However, because the entity has
/// `LinearVelocity` set (so joint-coupled dynamic child bodies are dragged along
/// by the solver), Avian's integrator updates the physics `Position` and its
/// writeback system overwrites `Transform` with `local_pos + delta`. Without this
/// system restoring the visual position, `transform-gizmo-bevy` would read the
/// overwritten value and add the new mouse delta on top of it, creating a 2x
/// speed feedback loop (runaway/multiplication of movement).
///
/// **The Proper Fix:**
/// Once Avian3D introduces a first-class Kinematic Drive/Teleport API (allowing
/// manual positioning of kinematic bodies with implicit velocity calculation for
/// joints *without* running the integrator step) or a way to disable writeback
/// on a per-entity basis, this system should be replaced with that native API.
pub fn restore_dragged_transform(
    mut q: Query<(&mut Transform, &GizmoPrevPos)>,
) {
    for (mut tf, prev) in q.iter_mut() {
        tf.translation = prev.local_pos.as_vec3();
    }
}

/// Restores dynamic state and re-enables origin tracking when gizmo drag ends.
pub fn restore_gizmo_dynamic(
    gizmo_targets: Query<&GizmoTarget>,
    q_prev_pos: Query<(Entity, &GizmoPrevPos)>,
    mut q_lin_vel: Query<&mut LinearVelocity>,
    q_avatar: Query<Entity, With<lunco_core::Avatar>>,
    q_floating_origin: Query<Entity, With<FloatingOrigin>>,
    mut commands: Commands,
) {
    let mut restored_any = false;
    for (entity, prev) in q_prev_pos.iter() {
        if let Ok(gizmo_target) = gizmo_targets.get(entity) {
            if gizmo_target.is_active() { continue; }
        }
        
        restored_any = true;

        info!("GIZMO: drag ended for {:?}, restoring coordinate systems", entity);

        // 2. RESTORE INTERPOLATION
        if prev.had_translation_interpolation { commands.entity(entity).insert(TranslationInterpolation); }
        if prev.had_rotation_interpolation { commands.entity(entity).insert(RotationInterpolation); }

        if let Ok(mut vel) = q_lin_vel.get_mut(entity) {
            vel.0 = DVec3::ZERO;
        }

        commands.entity(entity)
            .insert(prev.original_body)
            .remove::<GizmoPrevPos>();
    }

    // Re-attach FloatingOrigin to the avatar camera ONLY when the LAST drag
    // ends — i.e. no `GizmoTarget` is still active. With several entities
    // shift-selected and dragged together, restoring the origin the instant the
    // first one releases would un-freeze big_space *underneath* the entities
    // still being dragged (re-introducing the camera/origin feedback loop the
    // capture-time freeze exists to prevent).
    let any_still_active = gizmo_targets.iter().any(|gt| gt.is_active());
    if restored_any && !any_still_active {
        // 1. RESTORE ORIGIN TRACKING
        // Claim FloatingOrigin from the fallback anchor.
        for origin in q_floating_origin.iter() {
            commands.entity(origin).remove::<FloatingOrigin>();
        }
        // Re-attach FloatingOrigin to the avatar camera.
        for av_ent in q_avatar.iter() {
            commands.entity(av_ent).insert(FloatingOrigin);
            info!("GIZMO: restored FloatingOrigin on avatar camera {:?}", av_ent);
        }
    }
}

/// TEMP DIAGNOSTIC — logs, on every selection change, what each selected
/// entity actually carries: whether it has a `GizmoTarget`, its LOCAL
/// `Transform.translation` (what transform-gizmo-bevy reads and treats as
/// world) versus its `GlobalTransform` world position, and the GridAnchor/
/// SelectableRoot markers. Rovers get a gizmo; static USD buildings/landers
/// don't — this pinpoints whether the difference is a missing target, a
/// missing Transform, or a local≠world frame mismatch. Remove once resolved.
pub fn debug_gizmo_selection(
    selected: Res<crate::SelectedEntities>,
    q: Query<(
        Has<GizmoTarget>,
        Option<&Transform>,
        Option<&GlobalTransform>,
        Has<lunco_core::GridAnchor>,
        Has<lunco_core::SelectableRoot>,
        Option<&Name>,
    )>,
    q_parents: Query<&ChildOf>,
) {
    if !selected.is_changed() {
        return;
    }
    info!("[gizmo-dbg] selection changed: {} entities", selected.entities.len());
    for e in &selected.entities {
        match q.get(*e) {
            Ok((gt, tf, gtf, ga, sr, name)) => info!(
                "[gizmo-dbg]  SELECTED {e:?} name={:?} gizmo_target={gt} grid_anchor={ga} selectable_root={sr} local={:?} world={:?}",
                name.map(|n| n.as_str()),
                tf.map(|t| t.translation),
                gtf.map(|g| g.translation()),
            ),
            Err(_) => info!("[gizmo-dbg]  SELECTED {e:?} NOT MATCHED — missing Transform/GlobalTransform"),
        }
        // Walk the ancestor chain so we can see WHERE (if anywhere) a
        // SelectableRoot / GridAnchor sits above a leaf, and at what depth.
        let mut cur = *e;
        let mut depth = 0;
        while let Ok(parent) = q_parents.get(cur) {
            let p = parent.parent();
            depth += 1;
            if let Ok((_, tf, _, ga, sr, name)) = q.get(p) {
                info!(
                    "[gizmo-dbg]    ^{depth} {p:?} name={:?} grid_anchor={ga} selectable_root={sr} local={:?}",
                    name.map(|n| n.as_str()),
                    tf.map(|t| t.translation),
                );
            }
            cur = p;
            if depth >= 16 {
                break;
            }
        }
    }
}

/// App-owned replacement for transform-gizmo-bevy's default `mouse_interaction`
/// driver (disabled via Cargo features). The crate's version wrote
/// `GizmoDragStarted`/`GizmoDragging` on EVERY left press/hold — so the
/// **Shift+left-click** used to *select* an object also armed a drag, and once
/// the gizmo renders ON the object (its handles under the cursor) that grab
/// fired immediately. Gating on `!Shift` keeps Shift+click for selection only;
/// a **plain** left-drag on a handle still moves the object (the gizmo only
/// engages when `hovered`, i.e. the cursor is actually over a handle). Matches
/// the app's shift=select / plain=possess partition (see `on_scene_click_select`).
pub fn drive_gizmo_drag_no_shift(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut drag_started: MessageWriter<GizmoDragStarted>,
    mut dragging: MessageWriter<GizmoDragging>,
) {
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
        return;
    }
    if mouse.just_pressed(MouseButton::Left) {
        drag_started.write_default();
    }
    if mouse.pressed(MouseButton::Left) {
        dragging.write_default();
    }
}

/// Ensures the primary window camera carries the GizmoCamera marker.
pub fn sync_gizmo_camera(
    q_cameras: Query<(Entity, &RenderTarget), (With<Camera3d>, Without<GizmoCamera>)>,
    mut commands: Commands,
) {
    for (camera, target) in q_cameras.iter() {
        if matches!(target, RenderTarget::Window(_)) {
            commands.entity(camera).insert(GizmoCamera);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_controller::ControllerLink;

    #[test]
    fn test_gizmo_prev_pos_component() {
        let pos = GizmoPrevPos { 
            local_pos: DVec3::new(1.0, 2.0, 3.0), 
            original_body: RigidBody::Dynamic,
            had_translation_interpolation: false,
            had_rotation_interpolation: false,
        };
        assert_eq!(pos.local_pos, DVec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn test_possessed_entity_gizmo_restoration() {
        use crate::SelectedEntities;
        
        let mut app = App::new();
        app.init_resource::<SelectedEntities>();
        app.add_systems(Update, restore_gizmo_dynamic);

        let vessel = app.world_mut().spawn((
            Transform::from_translation(Vec3::ZERO),
            RigidBody::Kinematic,
            GizmoTarget::default(),
            GizmoPrevPos { 
                local_pos: DVec3::ZERO, 
                original_body: RigidBody::Dynamic,
                had_translation_interpolation: false,
                had_rotation_interpolation: false,
            },
            LinearVelocity::default(),
        )).id();
        
        app.world_mut().spawn(ControllerLink { vessel_entity: vessel });
        app.world_mut().resource_mut::<SelectedEntities>().entities.push(vessel);

        app.update();
        
        assert_eq!(app.world().get::<RigidBody>(vessel), Some(&RigidBody::Dynamic));
        assert!(app.world().get::<GizmoPrevPos>(vessel).is_none());
    }
}