//! Runtime telemetry for physical rigid-body state.
//!
//! Physics state is discovered from the live Avian body, not from authored
//! telemetry declarations. USD supplies the owning prim path for presentation
//! and Avian supplies the measured world-frame kinematics. Acceleration is the
//! finite-time derivative of the post-step velocity using the authoritative
//! fixed simulation clock; it is never inferred from a rover or component name.

use avian3d::prelude::{
    AngularVelocity, Collider, ComputedAngularInertia, ComputedCenterOfMass, ComputedMass,
    ContactGraph, LinearVelocity, Physics, Position, RigidBody, Rotation,
};
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_core::SimTick;
use lunco_mobility::{Suspension, WheelRaycast};
use lunco_signal::{SignalMeta, SignalRef, SignalRegistry, SignalSource};
use lunco_telemetry::TelemetrySettings;
use lunco_time::MissionClock;
use std::collections::{HashMap, HashSet};

use crate::UsdPrimPath;

#[derive(Resource, Default)]
pub struct PhysicsTelemetryState {
    previous: HashMap<Entity, PreviousKinematics>,
    /// Last physics-time batch sample per source. The shared signal registry
    /// still applies the final per-channel rate; this owner-level cursor only
    /// avoids rebuilding the same channel values between due samples.
    last_sample_times: HashMap<Entity, f64>,
    metadata: HashMap<SignalRef, SignalMeta>,
    metadata_group_paths: HashMap<Entity, String>,
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
/// shared registry policy applies the same rate, retention, and channel-limit
/// rules as all other runtime recorders; display deadband never removes a
/// simulation-time point from a history.
pub fn retain_physics_telemetry(
    mut commands: Commands,
    settings: Option<Res<TelemetrySettings>>,
    mut signals: Option<ResMut<SignalRegistry>>,
    mut state: ResMut<PhysicsTelemetryState>,
    tick: Res<SimTick>,
    mission_clock: Res<MissionClock>,
    contact_graph: Option<Res<ContactGraph>>,
    physics_time: Option<Res<Time<Physics>>>,
    mut removed_bodies: RemovedComponents<RigidBody>,
    mut removed_wheels: RemovedComponents<WheelRaycast>,
    sources: Query<(), Or<(With<RigidBody>, With<WheelRaycast>)>>,
    bodies: Query<
        (
            Entity,
            &UsdPrimPath,
            Option<&LinearVelocity>,
            Option<&AngularVelocity>,
            Option<&Position>,
            Option<&Rotation>,
            Option<&ComputedMass>,
            Option<&ComputedCenterOfMass>,
            Option<&ComputedAngularInertia>,
            Option<&Collider>,
        ),
        With<RigidBody>,
    >,
    wheels: Query<(
        Entity,
        &UsdPrimPath,
        &WheelRaycast,
        &Suspension,
        &avian3d::prelude::RayHits,
    )>,
) {
    let Some(settings) = settings else {
        state.previous.clear();
        state.last_sample_times.clear();
        state.metadata.clear();
        state.metadata_group_paths.clear();
        return;
    };
    if !settings.enabled || !settings.default_rate_hz.is_finite() || settings.default_rate_hz <= 0.0
    {
        state.previous.clear();
        state.last_sample_times.clear();
        state.metadata.clear();
        state.metadata_group_paths.clear();
        return;
    }
    let Some(signals) = signals.as_deref_mut() else {
        state.previous.clear();
        state.last_sample_times.clear();
        state.metadata.clear();
        state.metadata_group_paths.clear();
        return;
    };

    // `FixedPostUpdate` may run several times in one rendered frame. The
    // Bevy fixed accumulator is a cadence implementation detail, so its
    // elapsed value is not the simulation timestamp. `SimTick` advances once
    // per integrated fixed step and `MissionClock` owns its seconds mapping;
    // together they give every post-step sample the exact mission time of the
    // state that was just integrated.
    let time = mission_clock.sim_secs(tick.0);
    if !time.is_finite() {
        return;
    }

    // State is keyed by the source entity, not by a display label. Bevy already
    // records component removals, so use that lifecycle signal instead of
    // rebuilding a live-entity set and retaining every map on every fixed tick.
    // A body can also be a wheel, therefore only retire an entity once neither
    // source remains; this preserves telemetry across a source transition.
    let mut retire = |entity| {
        if sources.contains(entity) {
            return;
        }
        state.previous.remove(&entity);
        state.last_sample_times.remove(&entity);
        state.metadata_group_paths.remove(&entity);
        state.metadata.retain(|signal, _| signal.entity != entity);
    };
    for entity in removed_bodies.read() {
        retire(entity);
    }
    for entity in removed_wheels.read() {
        retire(entity);
    }

    let mut retained_any = HashSet::new();
    // The registry owns the channel catalog. Snapshot its size once per fixed
    // pass; checking it by walking every signal for every body sample turns
    // telemetry overhead into an avoidable quadratic scan.
    let mut channel_count = signals.iter_scalar().count();

    for (
        entity,
        prim,
        linear,
        angular,
        position,
        rotation,
        mass,
        center_of_mass,
        inertia,
        collider,
    ) in &bodies
    {
        let metadata_dirty = state
            .metadata_group_paths
            .get(&entity)
            .is_none_or(|path| path != &prim.path);
        let linear_velocity = linear.map(|value| value.0);
        let angular_velocity = angular.map(|value| value.0);
        if linear_velocity.is_some_and(|value| !value.is_finite())
            || angular_velocity.is_some_and(|value| !value.is_finite())
        {
            state.previous.remove(&entity);
            state.last_sample_times.remove(&entity);
            continue;
        }

        let previous = match (linear_velocity, angular_velocity) {
            (Some(linear_velocity), Some(angular_velocity)) => state.previous.insert(
                entity,
                PreviousKinematics {
                    time,
                    linear_velocity,
                    angular_velocity,
                },
            ),
            _ => {
                state.previous.remove(&entity);
                None
            }
        };

        let sample_due = state
            .last_sample_times
            .get(&entity)
            .is_none_or(|last| time < *last || time - *last >= 1.0 / settings.default_rate_hz);
        if !sample_due {
            continue;
        }
        if metadata_dirty {
            state.metadata_group_paths.insert(entity, prim.path.clone());
        }

        let mut samples = Vec::with_capacity(36);
        if let Some(linear_velocity) = linear_velocity {
            samples.extend(vector_channels(
                "linear_velocity",
                linear_velocity,
                "m/s",
                "World-frame linear-velocity component.",
            ));
            samples.push((
                "linear_speed".to_string(),
                linear_velocity.length(),
                "m/s",
                "Magnitude of world-frame linear velocity.",
            ));
        }
        if let Some(angular_velocity) = angular_velocity {
            samples.extend(vector_channels(
                "angular_velocity",
                angular_velocity,
                "rad/s",
                "World-frame angular-rate component.",
            ));
            samples.push((
                "angular_speed".to_string(),
                angular_velocity.length(),
                "rad/s",
                "Magnitude of world-frame angular velocity.",
            ));
        }

        if let (Some(previous), Some(linear_velocity), Some(angular_velocity)) =
            (previous, linear_velocity, angular_velocity)
        {
            let dt = time - previous.time;
            if dt.is_finite() && dt > 0.0 {
                let linear_acceleration = (linear_velocity - previous.linear_velocity) / dt;
                let angular_acceleration = (angular_velocity - previous.angular_velocity) / dt;
                if linear_acceleration.is_finite() && angular_acceleration.is_finite() {
                    samples.extend(vector_channels(
                        "linear_acceleration",
                        linear_acceleration,
                        "m/s^2",
                        "World-frame acceleration component.",
                    ));
                    samples.push((
                        "linear_acceleration".to_string(),
                        linear_acceleration.length(),
                        "m/s^2",
                        "Magnitude of world-frame linear acceleration.",
                    ));
                    samples.extend(vector_channels(
                        "angular_acceleration",
                        angular_acceleration,
                        "rad/s^2",
                        "World-frame angular-acceleration component.",
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

        if let Some(position) = position
            .map(|value| value.0)
            .filter(|value| value.is_finite())
        {
            samples.extend(explicit_vector_channels(
                "position",
                position,
                "m",
                "Physics-frame position component.",
            ));
        }
        if let Some(rotation) = rotation
            .map(|value| value.0)
            .filter(|value| value.is_finite())
        {
            samples.extend([
                (
                    "orientation.quat.x".to_string(),
                    rotation.x,
                    "1",
                    "Physics-body orientation quaternion x component.",
                ),
                (
                    "orientation.quat.y".to_string(),
                    rotation.y,
                    "1",
                    "Physics-body orientation quaternion y component.",
                ),
                (
                    "orientation.quat.z".to_string(),
                    rotation.z,
                    "1",
                    "Physics-body orientation quaternion z component.",
                ),
                (
                    "orientation.quat.w".to_string(),
                    rotation.w,
                    "1",
                    "Physics-body orientation quaternion w component.",
                ),
            ]);
            let (yaw, pitch, roll) = rotation.to_euler(bevy::math::EulerRot::YXZ);
            samples.extend([
                (
                    "yaw".to_string(),
                    yaw,
                    "rad",
                    "Physics-body yaw about the world-up axis.",
                ),
                (
                    "pitch".to_string(),
                    pitch,
                    "rad",
                    "Physics-body pitch in the YXZ attitude convention.",
                ),
                (
                    "roll".to_string(),
                    roll,
                    "rad",
                    "Physics-body roll in the YXZ attitude convention.",
                ),
            ]);
        }
        if let Some(mass) = mass.filter(|value| value.is_finite()) {
            samples.push((
                "mass".to_string(),
                mass.value(),
                "kg",
                "Computed rigid-body mass including admitted colliders.",
            ));
        }
        if let Some(center_of_mass) = center_of_mass
            .map(|value| value.0)
            .filter(|value| value.is_finite())
        {
            samples.extend(explicit_vector_channels(
                "center_of_mass",
                center_of_mass,
                "m",
                "Computed body-local center-of-mass component.",
            ));
        }
        if let Some(inertia) = inertia.map(|value| value.value()) {
            samples.extend([
                (
                    "inertia.xx".to_string(),
                    inertia.m00,
                    "kg.m^2",
                    "Computed body-local angular-inertia tensor diagonal.",
                ),
                (
                    "inertia.xy".to_string(),
                    inertia.m01,
                    "kg.m^2",
                    "Computed body-local angular-inertia tensor component.",
                ),
                (
                    "inertia.xz".to_string(),
                    inertia.m02,
                    "kg.m^2",
                    "Computed body-local angular-inertia tensor component.",
                ),
                (
                    "inertia.yy".to_string(),
                    inertia.m11,
                    "kg.m^2",
                    "Computed body-local angular-inertia tensor diagonal.",
                ),
                (
                    "inertia.yz".to_string(),
                    inertia.m12,
                    "kg.m^2",
                    "Computed body-local angular-inertia tensor component.",
                ),
                (
                    "inertia.zz".to_string(),
                    inertia.m22,
                    "kg.m^2",
                    "Computed body-local angular-inertia tensor diagonal.",
                ),
            ]);
        }
        if let Some(graph) = contact_graph.as_deref().filter(|_| collider.is_some()) {
            let physics_dt = physics_time
                .as_deref()
                .map(Time::delta_secs_f64)
                .unwrap_or(0.0);
            let (contact, contact_force) =
                lunco_cosim::avian::contact_of(graph, physics_dt, entity);
            samples.extend([
                (
                    "contact".to_string(),
                    contact as u8 as f64,
                    "1",
                    "Whether this collider is touching a physical contact pair.",
                ),
                (
                    "contact_force".to_string(),
                    contact_force,
                    "N",
                    "Normal contact force derived from the live Avian contact impulse.",
                ),
            ]);
        }

        if retain_samples(
            signals,
            settings.as_ref(),
            entity,
            &prim.path,
            time,
            samples,
            &mut channel_count,
            &mut state.metadata,
            metadata_dirty,
        ) {
            retained_any.insert(entity);
        }
        state.last_sample_times.insert(entity, time);
    }

    for (entity, prim, wheel, suspension, hits) in &wheels {
        let metadata_dirty = state
            .metadata_group_paths
            .get(&entity)
            .is_none_or(|path| path != &prim.path);
        let sample_due = state
            .last_sample_times
            .get(&entity)
            .is_none_or(|last| time < *last || time - *last >= 1.0 / settings.default_rate_hz);
        if !sample_due {
            continue;
        }
        if metadata_dirty {
            state.metadata_group_paths.insert(entity, prim.path.clone());
        }
        let contact = hits
            .iter_sorted()
            .find(|hit| hit.normal.is_finite() && hit.normal.length_squared() > 1.0e-12);
        let distance = contact.map_or(suspension.rest_length, |hit| hit.distance);
        let samples = [
            (
                "suspension.compression".to_string(),
                (suspension.rest_length - distance).max(0.0),
                "m",
                "Raycast suspension compression from the live support hit.",
            ),
            (
                "suspension.ground_distance".to_string(),
                distance,
                "m",
                "Distance from the authored suspension ray origin to the live support hit.",
            ),
            (
                "suspension.normal_force".to_string(),
                wheel.last_normal_force,
                "N",
                "Suspension normal force applied during the last physics tick.",
            ),
            (
                "suspension.contact".to_string(),
                contact.is_some() as u8 as f64,
                "1",
                "Whether the suspension ray has a valid non-degenerate support hit.",
            ),
            (
                "wheel.axle_angular_velocity".to_string(),
                wheel.axle_angular_velocity(),
                "rad/s",
                "Signed wheel axle angular velocity from the mobility model.",
            ),
            (
                "wheel.surface_speed".to_string(),
                wheel.surface_speed(),
                "m/s",
                "Wheel contact-patch speed implied by axle angular velocity.",
            ),
        ];
        if retain_samples(
            signals,
            settings.as_ref(),
            entity,
            &prim.path,
            time,
            samples,
            &mut channel_count,
            &mut state.metadata,
            metadata_dirty,
        ) {
            commands.entity(entity).try_insert(SignalSource);
        }
        state.last_sample_times.insert(entity, time);
    }

    for entity in retained_any {
        commands.entity(entity).try_insert(SignalSource);
    }
}

fn explicit_vector_channels(
    prefix: &str,
    value: DVec3,
    unit: &'static str,
    description: &'static str,
) -> [(String, f64, &'static str, &'static str); 3] {
    [
        (format!("{prefix}.x"), value.x, unit, description),
        (format!("{prefix}.y"), value.y, unit, description),
        (format!("{prefix}.z"), value.z, unit, description),
    ]
}

fn vector_channels(
    prefix: &str,
    value: DVec3,
    unit: &'static str,
    description: &'static str,
) -> [(String, f64, &'static str, &'static str); 3] {
    explicit_vector_channels(prefix, value, unit, description)
}

type PhysicsSample = (String, f64, &'static str, &'static str);

fn retain_samples(
    signals: &mut SignalRegistry,
    settings: &TelemetrySettings,
    entity: Entity,
    group_path: &str,
    time: f64,
    samples: impl IntoIterator<Item = PhysicsSample>,
    channel_count: &mut usize,
    metadata: &mut HashMap<SignalRef, SignalMeta>,
    metadata_dirty: bool,
) -> bool {
    let mut retained = false;
    for (name, value, unit, description) in samples {
        if !value.is_finite() {
            continue;
        }
        let signal = SignalRef::new(entity, name);
        let known = signals.scalar_history(&signal).is_some();
        if !known && *channel_count >= settings.max_channels {
            warn_once!(
                "physics telemetry: max_channels ({}) reached; additional state is not retained",
                settings.max_channels
            );
            continue;
        }
        if metadata_dirty || !metadata.contains_key(&signal) {
            let signal_meta = SignalMeta {
                description: Some(description.to_string()),
                unit: Some(unit.to_string()),
                provenance: Some("avian".to_string()),
                group_path: Some(group_path.to_string()),
                exposure: Default::default(),
                ..Default::default()
            };
            signals.update_meta(signal.clone(), signal_meta.clone());
            metadata.insert(signal.clone(), signal_meta);
        }
        if signals.record_scalar_at_rate(
            signal,
            time,
            value,
            settings.default_rate_hz,
            settings.default_retention,
        ) {
            retained = true;
            if !known {
                *channel_count += 1;
            }
        }
    }
    retained
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
        let linear = vector_channels(
            "linear_acceleration",
            DVec3::X,
            "m/s^2",
            "World-frame acceleration component.",
        );
        let angular = vector_channels(
            "angular_acceleration",
            DVec3::X,
            "rad/s^2",
            "World-frame angular-acceleration component.",
        );
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
                Position::from_xyz(2.0, 3.0, 4.0),
                Rotation(bevy::math::DQuat::from_rotation_y(0.25)),
                ComputedMass::new(2.0),
                ComputedCenterOfMass::new(0.1, 0.2, 0.3),
                ComputedAngularInertia::new(DVec3::splat(3.0)),
            ))
            .id();

        app.update();
        app.world_mut().resource_mut::<SimTick>().0 = 1;
        app.update();
        assert_eq!(
            app.world()
                .resource::<SignalRegistry>()
                .scalar_history(&SignalRef::new(body, "linear_velocity.x"))
                .unwrap()
                .len(),
            1,
            "a fixed step before the configured telemetry rate must not rebuild a batch"
        );
        app.world_mut().resource_mut::<SimTick>().0 = 6;
        app.world_mut().get_mut::<LinearVelocity>(body).unwrap().0 = DVec3::X;
        app.update();

        let signal = SignalRef::new(body, "linear_acceleration.x");
        let registry = app.world().resource::<SignalRegistry>();
        let sample = registry
            .scalar_history(&signal)
            .and_then(|history| history.samples.back())
            .expect("the second physics state has a measurable acceleration");
        assert!((sample.value - 12.0).abs() < 1.0e-12);
        assert!((sample.time - 0.1).abs() < 1.0e-12);
        assert_eq!(
            registry.meta(&signal).unwrap().group_path.as_deref(),
            Some("/Rover")
        );
        for channel in [
            "position.x",
            "orientation.quat.w",
            "yaw",
            "pitch",
            "roll",
            "mass",
            "center_of_mass.x",
            "inertia.xx",
        ] {
            assert!(
                registry
                    .scalar_history(&SignalRef::new(body, channel))
                    .is_some(),
                "missing generic rigid-body telemetry channel {channel}"
            );
        }
    }

    #[test]
    fn removed_body_retires_transient_state_but_preserves_history() {
        let mut app = App::new();
        app.insert_resource(TelemetrySettings::default())
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
                LinearVelocity(DVec3::X),
                AngularVelocity(DVec3::ZERO),
            ))
            .id();

        app.update();
        let signal = SignalRef::new(body, "linear_velocity.x");
        assert!(app
            .world()
            .resource::<SignalRegistry>()
            .scalar_history(&signal)
            .is_some());

        app.world_mut().entity_mut(body).remove::<RigidBody>();
        app.update();

        let state = app.world().resource::<PhysicsTelemetryState>();
        assert!(!state.previous.contains_key(&body));
        assert!(!state.last_sample_times.contains_key(&body));
        assert!(!state.metadata_group_paths.contains_key(&body));
        assert!(state.metadata.keys().all(|key| key.entity != body));
        assert!(app
            .world()
            .resource::<SignalRegistry>()
            .scalar_history(&signal)
            .is_some());
    }
}
