//! Physical presentation uses the simulation clock's completed-state interval.

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy_transform_interpolation::{
    NonlinearRotationEasing, NonlinearTranslationEasing, RotationEasingState,
    TranslationEasingState,
};

/// Compose the native endpoint lifecycle with application-owned render time.
pub(super) struct PhysicsPresentationPlugin;

impl Plugin for PhysicsPresentationPlugin {
    fn build(&self, app: &mut App) {
        // These public extension markers select our easing consumer. Native
        // capture, change invalidation and FixedFirst endpoint restore remain
        // responsible for the interpolation state lifecycle.
        app.register_required_components::<RigidBody, NonlinearTranslationEasing>()
            .register_required_components::<RigidBody, NonlinearRotationEasing>()
            .add_systems(
                RunFixedMainLoop,
                ease_physics_presentation
                    .in_set(bevy::app::RunFixedMainLoopSystems::AfterFixedMainLoop)
                    .after(bevy_transform_interpolation::reset_easing_states_on_transform_change)
                    .before(TransformEasingSystems::Ease),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_transform_interpolation::{
        TransformEasingPlugin, interpolation::TransformInterpolationPlugin,
    };
    use std::time::Duration;

    #[derive(Resource, Default)]
    struct InvalidEndpoint(bool);

    fn fixture_endpoints(
        invalid: Res<InvalidEndpoint>,
        mut endpoints: Query<(&mut TranslationEasingState, &mut RotationEasingState)>,
    ) {
        for (mut translation, mut rotation) in &mut endpoints {
            translation.start = Some(Vec3::ZERO);
            translation.end = Some(Vec3::X * 10.0);
            rotation.start = Some(Quat::IDENTITY);
            rotation.end = Some(if invalid.0 {
                Quat::from_xyzw(0.0, 0.0, 0.0, 0.0)
            } else {
                Quat::from_rotation_x(70.0_f32.to_radians())
            });
        }
    }

    fn fixture() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        app.insert_resource(Time::<Fixed>::from_seconds(0.02))
            .insert_resource(Time::<Virtual>::default())
            .insert_resource(Time::<Physics>::default())
            .insert_resource(lunco_time::MissionClock::anchored(2451545.0, 0))
            .init_resource::<lunco_physics::PhysicsHolds>()
            .init_resource::<lunco_core::RuntimeFaults>()
            .init_resource::<InvalidEndpoint>()
            .add_plugins((
                TransformEasingPlugin,
                TransformInterpolationPlugin::default(),
                PhysicsPresentationPlugin,
            ))
            .add_systems(
                RunFixedMainLoop,
                fixture_endpoints
                    .after(bevy_transform_interpolation::reset_easing_states_on_transform_change)
                    .before(ease_physics_presentation),
            );
        let body = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Transform::default(),
                TranslationInterpolation,
                RotationInterpolation,
                Position(bevy::math::DVec3::splat(123.0)),
            ))
            .id();
        app.finish();
        app.cleanup();
        (app, body)
    }

    #[test]
    fn retained_backlog_holds_completed_endpoints_without_consuming_simulation_time() {
        let (mut app, body) = fixture();
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .accumulate_overstep(Duration::from_secs(1));
        app.world_mut().run_schedule(RunFixedMainLoop); // mission reset
        app.world_mut().run_schedule(RunFixedMainLoop);
        let transform = app.world().get::<Transform>(body).unwrap();
        assert_eq!(transform.translation, Vec3::X * 10.0);
        assert!(
            transform
                .rotation
                .angle_between(Quat::from_rotation_x(70.0_f32.to_radians()))
                < 1e-6
        );
        assert!(transform.rotation.is_normalized());
        assert_eq!(
            app.world().resource::<Time<Fixed>>().overstep(),
            Duration::from_secs(1)
        );
        assert_eq!(
            app.world().get::<Position>(body).unwrap().0,
            bevy::math::DVec3::splat(123.0)
        );

        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .discard_overstep(Duration::from_millis(990));
        app.world_mut().run_schedule(RunFixedMainLoop);
        assert!((app.world().get::<Transform>(body).unwrap().translation.x - 5.0).abs() < 1e-6);
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        app.world_mut().run_schedule(RunFixedMainLoop);
        assert_eq!(
            app.world().get::<Transform>(body).unwrap().translation,
            Vec3::X * 10.0
        );

        // Native FixedFirst restores the latest completed endpoint before the
        // next simulation read; custom easing does not replace that lifecycle.
        app.world_mut().run_schedule(FixedFirst);
        assert_eq!(
            app.world().get::<Transform>(body).unwrap().translation,
            Vec3::X * 10.0
        );
        assert!(!app.world().resource::<lunco_core::RuntimeFaults>().active());
    }

    #[test]
    fn malformed_endpoint_faults_without_calling_asserting_quaternion_math() {
        let (mut app, body) = fixture();
        app.world_mut().resource_mut::<InvalidEndpoint>().0 = true;
        app.world_mut().run_schedule(RunFixedMainLoop);
        assert!(app.world().resource::<lunco_core::RuntimeFaults>().active());
        assert!(
            app.world()
                .resource::<lunco_physics::PhysicsHolds>()
                .holds(lunco_physics::PhysicsHolds::SAFETY_FAILURE)
        );
        assert!(app.world().resource::<Time<Physics>>().is_paused());
        assert_eq!(
            app.world().get::<Transform>(body).unwrap().rotation,
            Quat::IDENTITY
        );
    }
}

fn ease_physics_presentation(
    fixed: Res<Time<Fixed>>,
    virtual_time: Res<Time<Virtual>>,
    mission: Res<lunco_time::MissionClock>,
    barrier: Option<Res<lunco_core_runtime::SimulationBarrier>>,
    mut bodies: ParamSet<(
        Query<
            (Entity, &mut Transform, &TranslationEasingState),
            (
                With<RigidBody>,
                With<TranslationInterpolation>,
                Without<NoTranslationEasing>,
                Without<TranslationHermiteEasing>,
            ),
        >,
        Query<
            (Entity, &mut Transform, &RotationEasingState),
            (
                With<RigidBody>,
                With<RotationInterpolation>,
                Without<NoRotationEasing>,
                Without<RotationHermiteEasing>,
            ),
        >,
    )>,
    mut holds: ResMut<lunco_physics::PhysicsHolds>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
    mut physics_time: ResMut<Time<Physics>>,
) {
    if faults.active() {
        return;
    }
    // Transform is the explicit f32 render boundary. The clock owner provides
    // a finite [0, 1] interval, so this conversion cannot change its range.
    let render_fraction = lunco_time::simulation_presentation_fraction(
        &fixed,
        &virtual_time,
        barrier.as_deref(),
        mission.is_changed(),
    ) as f32;
    let mut invalid = None;
    for (entity, mut transform, easing) in &mut bodies.p0() {
        if let (Some(start), Some(end)) = (easing.start, easing.end) {
            if !start.is_finite() || !end.is_finite() {
                invalid = Some((entity, "translation"));
                break;
            }
            let next = start.lerp(end, render_fraction);
            if transform.translation != next {
                transform.translation = next;
            }
        }
    }
    if invalid.is_none() {
        for (entity, mut transform, easing) in &mut bodies.p1() {
            if let (Some(start), Some(end)) = (easing.start, easing.end) {
                if !start.is_finite()
                    || !end.is_finite()
                    || !start.is_normalized()
                    || !end.is_normalized()
                {
                    invalid = Some((entity, "rotation"));
                    break;
                }
                let next = start.slerp(end, render_fraction);
                if transform.rotation != next {
                    transform.rotation = next;
                }
            }
        }
    }
    if let Some((entity, channel)) = invalid {
        holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
        physics_time.pause();
        physics_time.advance_by(std::time::Duration::ZERO);
        let detail = format!("physical render {channel} endpoints are invalid");
        if faults.raise(
            "physics-presentation-state-invalid",
            Some(entity),
            "physics presentation",
            &detail,
        ) {
            error!("[physics] {detail}");
        }
    }
}
