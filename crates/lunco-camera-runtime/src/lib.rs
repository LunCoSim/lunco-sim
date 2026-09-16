//! Generic pose realization for interactive camera rigs.
//!
//! This package owns camera-mode exclusivity and the pure pose writers that do
//! not need to know whether a rig is an avatar, inspection camera, or another
//! authored operator. Avatar input, possession, vessel collision, and
//! source-specific surface-frame production remain outside this package.

use bevy::ecs::{lifecycle::HookContext, world::DeferredWorld};
use bevy::prelude::*;

use lunco_camera_core::{
    CameraDefaults, CameraFollow, CameraPoseLock, CameraPoseMode, CameraRig, CameraRigIntent,
    CameraRigMode, CameraUpdateSet, FollowAttitude, FreeFlightCamera, FreeFlightSettings,
    OrbitCamera, SpringArmCamera, SurfaceCamera, SurfaceCameraFrame, math::surface_camera_rotation,
};

fn freeflight_camera_added(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    world.commands().queue(move |world: &mut World| {
        if world.get::<FreeFlightCamera>(entity).is_some() {
            world
                .entity_mut(entity)
                .remove::<(SurfaceCamera, SpringArmCamera, OrbitCamera)>();
        }
    });
}

fn surface_camera_added(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    world.commands().queue(move |world: &mut World| {
        if world.get::<SurfaceCamera>(entity).is_some() {
            world.entity_mut(entity).remove::<(
                FreeFlightCamera,
                SpringArmCamera,
                OrbitCamera,
                lunco_time::InteractionEased,
            )>();
        }
    });
}

fn surface_camera_removed(mut world: DeferredWorld, context: HookContext) {
    world
        .commands()
        .entity(context.entity)
        .remove::<SurfaceCameraFrame>();
}

fn spring_arm_camera_added(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    world.commands().queue(move |world: &mut World| {
        if world.get::<SpringArmCamera>(entity).is_some() {
            world
                .entity_mut(entity)
                .remove::<(FreeFlightCamera, SurfaceCamera, OrbitCamera)>();
        }
    });
}

fn orbit_camera_added(mut world: DeferredWorld, context: HookContext) {
    let entity = context.entity;
    world.commands().queue(move |world: &mut World| {
        if world.get::<OrbitCamera>(entity).is_some() {
            world
                .entity_mut(entity)
                .remove::<(FreeFlightCamera, SurfaceCamera, SpringArmCamera)>();
        }
    });
}

fn register_camera_mode_hooks(app: &mut App) {
    app.world_mut()
        .register_component_hooks::<FreeFlightCamera>()
        .on_insert(freeflight_camera_added);
    app.world_mut()
        .register_component_hooks::<SurfaceCamera>()
        .on_insert(surface_camera_added)
        .on_remove(surface_camera_removed);
    app.world_mut()
        .register_component_hooks::<SpringArmCamera>()
        .on_insert(spring_arm_camera_added);
    app.world_mut()
        .register_component_hooks::<OrbitCamera>()
        .on_insert(orbit_camera_added);
}

/// Install generic camera mode exclusivity and pose writers.
pub struct CameraRuntimePlugin;

impl Plugin for CameraRuntimePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_time::TimePlugin>() {
            app.add_plugins(lunco_time::TimePlugin);
        }
        register_camera_mode_hooks(app);
        app.init_resource::<CameraDefaults>()
            .register_type::<CameraRig>()
            .register_type::<CameraPoseLock>()
            .register_type::<CameraFollow>()
            .register_type::<CameraPoseMode>()
            .register_type::<CameraRigIntent>()
            .register_type::<CameraRigMode>()
            .register_type::<FollowAttitude>()
            .register_type::<FreeFlightSettings>()
            .register_type::<SpringArmCamera>()
            .register_type::<OrbitCamera>()
            .register_type::<FreeFlightCamera>()
            .register_type::<SurfaceCamera>()
            .register_type::<SurfaceCameraFrame>();
        app.add_systems(
            lunco_time::InteractionSchedule,
            (
                rebase_freeflight_state,
                freeflight_system,
                surface_camera_system,
            )
                .chain()
                .in_set(CameraUpdateSet),
        );
    }
}

/// Preserve free-flight orientation when its parent frame changes.
pub fn rebase_freeflight_state(
    mut q_camera: Query<
        (&mut FreeFlightCamera, &Transform),
        (With<CameraRig>, Changed<ChildOf>, Without<CameraPoseLock>),
    >,
) {
    for (mut freeflight, transform) in q_camera.iter_mut() {
        let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
        freeflight.yaw = yaw;
        freeflight.pitch = pitch;
    }
}

/// Write free-flight orientation from the rig's yaw/pitch state.
pub fn freeflight_system(
    mut q_camera: Query<
        (&mut Transform, &FreeFlightCamera),
        (
            With<CameraRig>,
            Without<OrbitCamera>,
            Without<SpringArmCamera>,
            Without<SurfaceCamera>,
            Without<CameraPoseLock>,
        ),
    >,
    drag_mode: Option<Res<lunco_interaction_core::DragModeActive>>,
) {
    if drag_mode.is_some_and(|drag| drag.active) {
        return;
    }
    for (mut transform, freeflight) in q_camera.iter_mut() {
        let rotation = Quat::from_euler(EulerRot::YXZ, freeflight.yaw, freeflight.pitch, 0.0);
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
    }
}

/// Write surface-relative orientation from a supplied generic surface frame.
pub fn surface_camera_system(
    mut q_camera: Query<
        (&mut Transform, &SurfaceCamera, &SurfaceCameraFrame),
        (
            With<CameraRig>,
            Without<SpringArmCamera>,
            Without<FreeFlightCamera>,
            Without<OrbitCamera>,
            Without<CameraPoseLock>,
        ),
    >,
    drag_mode: Option<Res<lunco_interaction_core::DragModeActive>>,
) {
    if drag_mode.is_some_and(|drag| drag.active) {
        return;
    }
    for (mut transform, camera, frame) in q_camera.iter_mut() {
        let rotation = surface_camera_rotation(
            frame.east,
            frame.north,
            frame.up,
            camera.heading,
            camera.pitch,
        );
        if transform.rotation != rotation {
            transform.rotation = rotation;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freeflight_reseeds_after_parent_handoff() {
        let mut app = App::new();
        app.add_systems(Update, rebase_freeflight_state);

        let first_parent = app.world_mut().spawn_empty().id();
        let second_parent = app.world_mut().spawn_empty().id();
        let authored_rotation = Quat::from_euler(EulerRot::YXZ, 1.2, -0.4, 0.0);
        let camera = app
            .world_mut()
            .spawn((
                CameraRig,
                FreeFlightCamera {
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                },
                Transform::from_rotation(authored_rotation),
                ChildOf(first_parent),
            ))
            .id();

        app.update();
        app.world_mut().entity_mut(camera).insert((
            ChildOf(second_parent),
            Transform::from_rotation(authored_rotation),
        ));
        app.update();

        let freeflight = app.world().get::<FreeFlightCamera>(camera).unwrap();
        assert!((freeflight.yaw - 1.2).abs() < 1e-5);
        assert!((freeflight.pitch + 0.4).abs() < 1e-5);
    }

    #[test]
    fn camera_modes_are_exclusive() {
        let mut app = App::new();
        register_camera_mode_hooks(&mut app);

        let camera = app.world_mut().spawn_empty().id();
        app.world_mut().entity_mut(camera).insert((
            FreeFlightCamera {
                yaw: 0.0,
                pitch: 0.0,
                damping: None,
            },
            SurfaceCamera {
                heading: 0.0,
                pitch: 0.0,
            },
            SpringArmCamera {
                target: Entity::PLACEHOLDER,
                distance: 1.0,
                yaw: 0.0,
                pitch: 0.0,
                damping: None,
                vertical_offset: 0.0,
                track_heading: true,
                attitude: FollowAttitude::Heading,
            },
            OrbitCamera {
                target: Entity::PLACEHOLDER,
                distance: 1.0,
                yaw: 0.0,
                pitch: 0.0,
                damping: None,
                vertical_offset: 0.0,
            },
        ));
        app.update();

        let world = app.world();
        let active_modes = [
            world.get::<FreeFlightCamera>(camera).is_some(),
            world.get::<SurfaceCamera>(camera).is_some(),
            world.get::<SpringArmCamera>(camera).is_some(),
            world.get::<OrbitCamera>(camera).is_some(),
        ]
        .into_iter()
        .filter(|active| *active)
        .count();
        assert_eq!(active_modes, 1);
    }
}
