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
use transform_gizmo_bevy::{GizmoCamera, GizmoTarget};

// Removed SelectedEntity import

/// Tracks the previous absolute world position and metadata for drag lifecycle.
#[derive(Component)]
pub struct GizmoPrevPos {
    /// Absolute world position in the previous frame (meters).
    pub abs_pos: DVec3,
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
) {
    for (e, gt) in &q {
        if gt.is_active() {
            commands.entity(e).insert(lunco_core::GizmoDragging);
        } else {
            commands.entity(e).remove::<lunco_core::GizmoDragging>();
        }
    }
}

/// Makes the selected entity kinematic and freezes the coordinate system when gizmo drag starts.
pub fn capture_gizmo_start(
    gizmo_targets: Query<(Entity, &GizmoTarget)>,
    q_rigid_bodies: Query<&RigidBody>,
    q_prev_pos: Query<&GizmoPrevPos>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
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

        // Resolve initial absolute world position.
        let Ok((cell, tf)) = q_spatial.get(entity) else { continue; };
        let cell = cell.copied().unwrap_or_default();
        let abs_pos = lunco_core::coords::world_position_seeded(
            entity, &cell, tf, &q_parents, &q_grids, &q_spatial
        );

        info!("GIZMO: drag started for {:?}, abs_pos={:?}", entity, abs_pos);

        commands.entity(entity)
            .insert(RigidBody::Kinematic)
            .insert(GizmoPrevPos { 
                abs_pos, 
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



/// Syncs Avian `Position` and computes velocity from absolute world coordinates.
pub fn sync_gizmo_transforms(
    gizmo_targets: Query<(Entity, &GizmoTarget)>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    mut q_position: Query<&mut avian3d::physics_transform::Position>,
    mut q_rotation: Query<&mut avian3d::physics_transform::Rotation>,
    mut q_lin_vel: Query<&mut LinearVelocity>,
    mut q_prev_pos: Query<&mut GizmoPrevPos>,
    time: Res<Time>,
) {
    for (entity, gizmo_target) in gizmo_targets.iter() {
        if !gizmo_target.is_active() { continue; }

        let Ok((cell, tf)) = q_spatial.get(entity) else { continue; };
        let cell = cell.copied().unwrap_or_default();

        let current_abs_pos = lunco_core::coords::world_position_seeded(
            entity, &cell, tf, &q_parents, &q_grids, &q_spatial
        );

        if let Ok(mut pos) = q_position.get_mut(entity) {
            pos.0 = current_abs_pos;
        }
        
        if let Ok(mut rot) = q_rotation.get_mut(entity) {
            rot.0 = tf.rotation.as_dquat();
        }

        let dt = time.delta_secs();
        if dt > 1e-6 {
            if let Ok(mut prev) = q_prev_pos.get_mut(entity) {
                let delta = current_abs_pos - prev.abs_pos;
                if let Ok(mut lin_vel) = q_lin_vel.get_mut(entity) {
                    lin_vel.0 = delta / dt as f64;
                }
                prev.abs_pos = current_abs_pos;
            }
        }
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

    if restored_any {
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
            abs_pos: DVec3::new(1.0, 2.0, 3.0), 
            original_body: RigidBody::Dynamic,
            had_translation_interpolation: false,
            had_rotation_interpolation: false,
        };
        assert_eq!(pos.abs_pos, DVec3::new(1.0, 2.0, 3.0));
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
                abs_pos: DVec3::ZERO, 
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