//! Generic pose realization for interactive camera rigs.
//!
//! This package owns camera-mode exclusivity and the pure pose writers that do
//! not need to know whether a rig is an avatar, inspection camera, or another
//! authored operator. Embodiment input, possession, vessel collision, and
//! source-specific surface-frame production remain outside this package. It
//! also owns the generic rule that direct-pose modes cannot retain interaction
//! easing, so every camera realization has one transform writer.

use bevy::ecs::{lifecycle::HookContext, world::DeferredWorld};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use lunco_camera_core::{
    CameraDefaults, CameraFollow, CameraPoseLock, CameraPoseMode, CameraRig, CameraUpdateSet,
    FollowAttitude, FreeFlightCamera, FreeFlightSettings, OrbitCamera, SpringArmCamera,
    SurfaceCamera, SurfaceCameraFrame, math::surface_camera_rotation,
};
use lunco_camera_core::SetCameraInput;
use lunco_core::{on_command, register_commands};
use lunco_settings::{AppSettingsExt, SettingsSection};

/// Persisted pointer response shared by every interactive camera rig.
///
/// The input layer supplies semantic look deltas; this resource only defines
/// how much camera motion one accepted delta produces. Orbit scaling remains a
/// pure camera policy and is independent of avatar or celestial ownership.
#[derive(Resource, Reflect, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[reflect(Resource)]
#[serde(default)]
pub struct CameraInputSettings {
    /// Camera radians per pointer-motion unit before behavior-specific scaling.
    pub look_radians_per_pointer_unit: f32,
    /// Lower bound for orbital rotation at the body's surface.
    pub orbit_surface_min_scale: f64,
    /// Shapes the geometric visible-horizon response.
    pub orbit_distance_curve_exponent: f64,
}

impl Default for CameraInputSettings {
    fn default() -> Self {
        Self {
            look_radians_per_pointer_unit: 0.001125,
            orbit_surface_min_scale: 0.04,
            orbit_distance_curve_exponent: 0.75,
        }
    }
}

impl SettingsSection for CameraInputSettings {
    const KEY: &'static str = "camera_input";
}

#[on_command(SetCameraInput)]
fn on_set_camera_input(trigger: On<SetCameraInput>, mut settings: ResMut<CameraInputSettings>) {
    let command = trigger.event();
    if let Some(value) = command.look_radians_per_pointer_unit {
        if value.is_finite() && value >= 0.0 {
            settings.look_radians_per_pointer_unit = value;
        } else {
            warn!("SetCameraInput rejected non-finite/negative look sensitivity: {value}");
        }
    }
    if let Some(value) = command.orbit_surface_min_scale {
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            settings.orbit_surface_min_scale = value;
        } else {
            warn!("SetCameraInput rejected surface scale outside [0, 1]: {value}");
        }
    }
    if let Some(value) = command.orbit_distance_curve_exponent {
        if value.is_finite() && value > 0.0 {
            settings.orbit_distance_curve_exponent = value;
        } else {
            warn!("SetCameraInput rejected non-positive distance exponent: {value}");
        }
    }
}

/// Scale an orbit gesture from the target body's apparent geometry.
pub fn body_orbit_look_scale(
    distance_m: f64,
    radius_m: f64,
    settings: &CameraInputSettings,
) -> f64 {
    let min_scale = settings.orbit_surface_min_scale.clamp(0.0, 1.0);
    let exponent = settings.orbit_distance_curve_exponent.max(f64::EPSILON);
    if !distance_m.is_finite() || !radius_m.is_finite() || radius_m <= 0.0 {
        return 1.0;
    }
    let ratio = (radius_m / distance_m.max(radius_m)).clamp(0.0, 1.0);
    let visible_horizon = (1.0 - ratio * ratio).max(0.0).sqrt();
    min_scale + (1.0 - min_scale) * visible_horizon.powf(exponent)
}

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
        register_all_commands(app);
        app.init_resource::<CameraDefaults>()
            .init_resource::<CameraInputSettings>()
            .register_type::<CameraRig>()
            .register_type::<CameraPoseLock>()
            .register_type::<CameraFollow>()
            .register_type::<CameraPoseMode>()
            .register_type::<FollowAttitude>()
            .register_type::<FreeFlightSettings>()
            .register_type::<SpringArmCamera>()
            .register_type::<OrbitCamera>()
            .register_type::<FreeFlightCamera>()
            .register_type::<SurfaceCamera>()
            .register_type::<SurfaceCameraFrame>()
            .register_type::<CameraInputSettings>();
        app.register_settings_section::<CameraInputSettings>();
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
        app.add_systems(Update, sync_camera_easing);
    }
}

/// Keep render-rate interaction easing exclusive to incremental free flight.
///
/// Target-following, orbital, surface-relative, and explicitly driven cameras
/// write their complete pose through their owning realization. Letting one of
/// those modes retain [`lunco_time::InteractionEased`] would create a second
/// transform writer and interpolate cell-local poses across a BigSpace rebase.
pub fn sync_camera_easing(
    mut commands: Commands,
    q: Query<
        (
            Entity,
            Has<SpringArmCamera>,
            Has<OrbitCamera>,
            Has<SurfaceCamera>,
            Has<lunco_time::InteractionEased>,
            Has<CameraPoseLock>,
        ),
        With<CameraRig>,
    >,
) {
    for (entity, spring_arm, orbit, surface_camera, eased, cinematic_lock) in q.iter() {
        if cinematic_lock {
            if eased {
                commands
                    .entity(entity)
                    .remove::<lunco_time::InteractionEased>();
            }
            continue;
        }
        if spring_arm || orbit || surface_camera {
            if eased {
                commands
                    .entity(entity)
                    .remove::<lunco_time::InteractionEased>();
            }
        } else if !eased {
            commands
                .entity(entity)
                .try_insert(lunco_time::InteractionEased::default());
        }
    }
}

#[cfg(test)]
mod camera_input_tests {
    use super::*;

    #[test]
    fn orbit_look_scale_is_continuous_monotonic_and_body_size_independent() {
        let settings = CameraInputSettings::default();
        let moon = 1_737_400.0;
        let surface = body_orbit_look_scale(moon, moon, &settings);
        let low = body_orbit_look_scale(moon + 100.0, moon, &settings);
        let high = body_orbit_look_scale(moon + 100_000.0, moon, &settings);
        let far = body_orbit_look_scale(moon * 100.0, moon, &settings);

        assert_eq!(surface, settings.orbit_surface_min_scale);
        assert!(
            surface < low && low < high && high < far,
            "{surface} {low} {high} {far}"
        );
        assert!(far < 1.0);

        let same_ratio_on_earth = body_orbit_look_scale(6_378_137.0 * 2.0, 6_378_137.0, &settings);
        let same_ratio_on_moon = body_orbit_look_scale(moon * 2.0, moon, &settings);
        assert!((same_ratio_on_earth - same_ratio_on_moon).abs() < 1.0e-12);
    }

    #[test]
    fn orbit_look_scale_honours_the_configured_surface_floor() {
        let settings = CameraInputSettings {
            orbit_surface_min_scale: 0.125,
            ..default()
        };
        assert_eq!(body_orbit_look_scale(10.0, 10.0, &settings), 0.125);
    }
}

register_commands!(on_set_camera_input);

#[cfg(test)]
mod camera_easing_tests {
    use super::*;

    #[test]
    fn pose_owners_exclude_direct_modes_and_restore_easing_for_free_flight() {
        let mut app = App::new();
        app.add_systems(Update, sync_camera_easing);

        let spring = app
            .world_mut()
            .spawn((
                SpringArmCamera {
                    target: Entity::PLACEHOLDER,
                    distance: 10.0,
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                    vertical_offset: 2.0,
                    track_heading: true,
                    attitude: FollowAttitude::Heading,
                },
                lunco_time::InteractionEased::default(),
            ))
            .id();
        let orbit = app
            .world_mut()
            .spawn((
                OrbitCamera {
                    target: Entity::PLACEHOLDER,
                    distance: 1_800_000.0,
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                    vertical_offset: 0.0,
                },
                lunco_time::InteractionEased::default(),
            ))
            .id();
        let surface = app
            .world_mut()
            .spawn((
                SurfaceCamera {
                    heading: 0.0,
                    pitch: -0.2,
                },
                lunco_time::InteractionEased::default(),
            ))
            .id();
        let free = app
            .world_mut()
            .spawn(FreeFlightCamera {
                yaw: 0.0,
                pitch: 0.0,
                damping: None,
            })
            .id();
        let locked = app
            .world_mut()
            .spawn((
                CameraRig,
                CameraPoseLock,
                lunco_time::InteractionEased::default(),
            ))
            .id();

        app.update();

        for entity in [spring, orbit, surface, locked] {
            assert!(
                app.world()
                    .get::<lunco_time::InteractionEased>(entity)
                    .is_none(),
                "a complete or explicitly locked pose must have one writer"
            );
        }
        assert!(
            app.world()
                .get::<lunco_time::InteractionEased>(free)
                .is_some(),
            "free flight regains interaction easing"
        );

        app.world_mut()
            .entity_mut(spring)
            .remove::<SpringArmCamera>();
        app.world_mut().entity_mut(orbit).remove::<OrbitCamera>();
        app.world_mut()
            .entity_mut(surface)
            .remove::<SurfaceCamera>();
        app.update();

        for entity in [spring, orbit, surface] {
            assert!(
                app.world()
                    .get::<lunco_time::InteractionEased>(entity)
                    .is_some(),
                "a rig without a direct-pose mode regains easing"
            );
        }
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
