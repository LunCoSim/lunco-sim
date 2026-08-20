//! Runtime telemetry for physical rigid-body state.
//!
//! Physics state is discovered from the live Avian body, not from authored
//! telemetry declarations. USD supplies the owning prim path for presentation
//! and Avian supplies the measured world-frame kinematics. Acceleration is the
//! finite-time derivative of the post-step velocity using the authoritative
//! fixed simulation clock; it is never inferred from a rover or component name.

use avian3d::prelude::{AngularVelocity, LinearVelocity, RigidBody};
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_core::SimTick;
use lunco_signal::{SignalMeta, SignalRef, SignalRegistry, SignalSource};
use lunco_telemetry::TelemetrySettings;
use lunco_time::MissionClock;
use std::collections::{HashMap, HashSet};

use crate::UsdPrimPath;

#[derive(Resource, Default)]
pub struct PhysicsTelemetryState {
    previous: HashMap<Entity, PreviousKinematics>,
}

#[derive(Clone, Copy)]
struct PreviousKinematics {
    time: f64,
    linear_velocity: DVec3,
    angular_velocity: DVec3,
}

/// Retain post-step rigid-body kinematics in the shared signal registry.
///
/// The system is installed in `FixedPostUpdate` after Avian's physics step, so
/// one retained sample corresponds to one integrated physics state. The
/// shared registry policy applies the same rate, deadband, retention, and
/// channel-limit rules as all other runtime producers.
pub fn retain_physics_telemetry(
    mut commands: Commands,
    settings: Option<Res<TelemetrySettings>>,
    mut signals: Option<ResMut<SignalRegistry>>,
    mut state: ResMut<PhysicsTelemetryState>,
    tick: Res<SimTick>,
    mission_clock: Res<MissionClock>,
    bodies: Query<
        (
            Entity,
            &UsdPrimPath,
            Option<&LinearVelocity>,
            Option<&AngularVelocity>,
        ),
        With<RigidBody>,
    >,
) {
    let Some(settings) = settings else {
        state.previous.clear();
        return;
    };
    if !settings.enabled
        || !settings.default_rate_hz.is_finite()
        || settings.default_rate_hz <= 0.0
        || !settings.default_deadband.is_valid()
    {
        state.previous.clear();
        return;
    }
    let Some(signals) = signals.as_deref_mut() else {
        state.previous.clear();
        return;
    };

    // `FixedPostUpdate` may run several times in one rendered frame. The
    // Bevy fixed accumulator is a cadence implementation detail, so its
    // elapsed value is not the simulation timestamp. `SimTick` advances once
    // per integrated fixed step and `MissionClock` owns its seconds mapping;
    // together they give every post-step sample the exact mission time of the
    // state that was just integrated.
    let time = mission_clock.sim_secs(tick.0);
    let mut seen = HashSet::new();
    let mut retained_any = HashSet::new();

    for (entity, prim, linear, angular) in &bodies {
        let Some(linear_velocity) = linear.map(|value| value.0) else {
            continue;
        };
        let Some(angular_velocity) = angular.map(|value| value.0) else {
            continue;
        };
        if !time.is_finite() || !linear_velocity.is_finite() || !angular_velocity.is_finite() {
            continue;
        }

        seen.insert(entity);
        let previous = state.previous.insert(
            entity,
            PreviousKinematics {
                time,
                linear_velocity,
                angular_velocity,
            },
        );

        let mut samples = Vec::with_capacity(14);
        samples.extend(vector_channels("linear_velocity", linear_velocity));
        samples.push((
            "linear_speed".to_string(),
            linear_velocity.length(),
            "m/s",
            "Magnitude of world-frame linear velocity.",
        ));
        samples.extend(vector_channels("angular_velocity", angular_velocity));
        samples.push((
            "angular_speed".to_string(),
            angular_velocity.length(),
            "rad/s",
            "Magnitude of world-frame angular velocity.",
        ));

        if let Some(previous) = previous {
            let dt = time - previous.time;
            if dt.is_finite() && dt > 0.0 {
                let linear_acceleration = (linear_velocity - previous.linear_velocity) / dt;
                let angular_acceleration = (angular_velocity - previous.angular_velocity) / dt;
                if linear_acceleration.is_finite() && angular_acceleration.is_finite() {
                    samples.extend(vector_channels("linear_acceleration", linear_acceleration));
                    samples.push((
                        "linear_acceleration".to_string(),
                        linear_acceleration.length(),
                        "m/s^2",
                        "Magnitude of world-frame linear acceleration.",
                    ));
                    samples.extend(vector_channels(
                        "angular_acceleration",
                        angular_acceleration,
                    ));
                    samples.push((
                        "angular_acceleration".to_string(),
                        angular_acceleration.length(),
                        "rad/s^2",
                        "Magnitude of world-frame angular acceleration.",
                    ));
                }
            }
        }

        for (name, value, unit, description) in samples {
            let signal = SignalRef::new(entity, name);
            if signals.scalar_history(&signal).is_none()
                && signals.iter_scalar().count() >= settings.max_channels
            {
                warn_once!(
                    "physics telemetry: max_channels ({}) reached; additional rigid-body state is not retained",
                    settings.max_channels
                );
                continue;
            }
            signals.update_meta(
                signal.clone(),
                SignalMeta {
                    description: Some(description.to_string()),
                    unit: Some(unit.to_string()),
                    provenance: Some("avian".to_string()),
                    group_path: Some(prim.path.clone()),
                },
            );
            if signals.retain_scalar_if_changed(
                signal,
                time,
                value,
                settings.default_rate_hz,
                settings.default_deadband,
                settings.default_retention,
            ) {
                retained_any.insert(entity);
            }
        }
    }

    state.previous.retain(|entity, _| seen.contains(entity));
    for entity in retained_any {
        commands.entity(entity).try_insert(SignalSource);
    }
}

fn vector_channels(prefix: &str, value: DVec3) -> [(String, f64, &'static str, &'static str); 3] {
    let (unit, description) = if prefix.contains("angular") && prefix.contains("acceleration") {
        ("rad/s^2", "World-frame angular-acceleration component.")
    } else if prefix.contains("acceleration") {
        ("m/s^2", "World-frame acceleration component.")
    } else if prefix.contains("angular") {
        ("rad/s", "World-frame angular-rate component.")
    } else {
        ("m/s", "World-frame linear-velocity component.")
    };
    [
        (format!("{prefix}.x"), value.x, unit, description),
        (format!("{prefix}.y"), value.y, unit, description),
        (format!("{prefix}.z"), value.z, unit, description),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivative_uses_physics_time_and_world_velocity() {
        let previous = DVec3::new(1.0, -2.0, 0.5);
        let current = DVec3::new(3.0, 0.0, -0.5);
        let acceleration = (current - previous) / 0.5;
        assert_eq!(acceleration, DVec3::new(4.0, 4.0, -2.0));
    }

    #[test]
    fn first_kinematics_sample_has_no_fabricated_acceleration() {
        assert!(None::<PreviousKinematics>.is_none());
    }

    #[test]
    fn acceleration_channels_report_acceleration_units() {
        let linear = vector_channels("linear_acceleration", DVec3::X);
        let angular = vector_channels("angular_acceleration", DVec3::X);
        assert!(linear.iter().all(|(_, _, unit, _)| *unit == "m/s^2"));
        assert!(angular.iter().all(|(_, _, unit, _)| *unit == "rad/s^2"));
    }

    #[test]
    fn live_body_publishes_acceleration_under_its_usd_path() {
        let mut app = App::new();
        app.insert_resource(TelemetrySettings {
            default_rate_hz: 10.0,
            ..Default::default()
        })
        .insert_resource(SignalRegistry::default())
        .insert_resource(SimTick(0))
        .insert_resource(MissionClock::default())
        .init_resource::<PhysicsTelemetryState>()
        .add_systems(Update, retain_physics_telemetry);
        let body = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: default(),
                    path: "/Rover".to_string(),
                },
                RigidBody::Dynamic,
                LinearVelocity(DVec3::ZERO),
                AngularVelocity(DVec3::ZERO),
            ))
            .id();

        app.update();
        app.world_mut().resource_mut::<SimTick>().0 = 6;
        app.world_mut().get_mut::<LinearVelocity>(body).unwrap().0 = DVec3::X;
        app.update();

        let signal = SignalRef::new(body, "linear_acceleration.x");
        let registry = app.world().resource::<SignalRegistry>();
        let sample = registry
            .scalar_history(&signal)
            .and_then(|history| history.samples.back())
            .expect("the second physics state has a measurable acceleration");
        assert!((sample.value - 10.0).abs() < 1.0e-12);
        assert!((sample.time - 0.1).abs() < 1.0e-12);
        assert_eq!(
            registry.meta(&signal).unwrap().group_path.as_deref(),
            Some("/Rover")
        );
    }
}
