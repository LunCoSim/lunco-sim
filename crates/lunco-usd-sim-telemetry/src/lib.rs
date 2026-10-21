//! Runtime telemetry for physical rigid-body state.
//!
//! Physics state is discovered from the live Avian body, not from authored
//! telemetry declarations. USD supplies the owning prim path for presentation
//! and Avian supplies the measured world-frame kinematics. Acceleration is the
//! physics-owned native-interval measurement, shared with co-simulation and
//! sensors; this recorder does not calculate a second derivative.

use avian3d::prelude::{
    AngularVelocity, ComputedAngularInertia, ComputedCenterOfMass, ComputedMass, ContactGraph,
    LinearVelocity, PhysicsSystems, Position, RigidBody, Rotation,
};
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_core::{AppSceneExt, GlobalEntityId, SceneResource};
use lunco_core_runtime::SimTick;
use lunco_mobility::{Suspension, WheelRaycast};
use lunco_physics::kinematics::SolvedBodyAcceleration;
use lunco_signal::{SignalMeta, SignalPresentation, SignalRef, SignalRegistry, SignalSource};
use lunco_telemetry::TelemetrySettings;
use lunco_time::MissionClock;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet, hash_map::Entry};

use lunco_usd_bevy_scene::UsdPrimPath;

/// Installs the post-step physics telemetry recorder.
///
/// Telemetry is a complete projection consumer, not vehicle realization. The
/// runtime composition installs it beside the vehicle and celestial projectors
/// so telemetry changes do not rebuild the vehicle projector.
pub struct PhysicsTelemetryPlugin;

impl Plugin for PhysicsTelemetryPlugin {
    fn build(&self, app: &mut App) {
        app.init_scene_resource::<PhysicsTelemetryState>()
            .add_systems(
                FixedPostUpdate,
                retain_physics_telemetry
                    .after(PhysicsSystems::StepSimulation)
                    .after(lunco_physics::kinematics::BodyAccelerationSampleSet)
                    .after(lunco_mobility::WheelRaycastResultsSet),
            );
    }
}

#[derive(Resource, Default)]
pub struct PhysicsTelemetryState {
    /// Next physics-time batch sample. The shared signal registry still applies
    /// the final per-channel rate; this owner-level gate keeps the body query
    /// out of fixed steps between telemetry batches.
    next_sample_time: Option<f64>,
    last_sample_time: Option<f64>,
    sample_rate_hz: Option<f64>,
    metadata: HashMap<(Entity, PhysicsSignalName), CachedPhysicsSignal>,
    metadata_group_paths: HashMap<Entity, String>,
}

impl SceneResource for PhysicsTelemetryState {}

/// Retain post-step rigid-body kinematics in the shared signal registry.
///
/// The system is installed in `FixedPostUpdate` after Avian's physics step, so
/// one retained sample corresponds to one integrated physics state. The
/// shared registry policy applies the same rate, retention, and channel-limit
/// rules as all other runtime recorders; display deadband never removes a
/// simulation-time point from a history.
fn retain_physics_telemetry(
    mut commands: Commands,
    settings: Option<Res<TelemetrySettings>>,
    mut signals: Option<ResMut<SignalRegistry>>,
    mut state: ResMut<PhysicsTelemetryState>,
    tick: Res<SimTick>,
    mission_clock: Res<MissionClock>,
    contact_graph: Option<Res<ContactGraph>>,
    mut removed_bodies: RemovedComponents<RigidBody>,
    mut removed_wheels: RemovedComponents<WheelRaycast>,
    mut sample_buffer: Local<Vec<PhysicsSample>>,
    mut queued_signal_sources: Local<HashSet<Entity>>,
    sources: Query<(), Or<(With<RigidBody>, With<WheelRaycast>)>>,
    bodies: Query<
        (
            Entity,
            &UsdPrimPath,
            Option<&LinearVelocity>,
            Option<&AngularVelocity>,
            Option<&GlobalEntityId>,
            Option<&SignalSource>,
            Option<&SolvedBodyAcceleration>,
        ),
        With<RigidBody>,
    >,
    body_state: Query<
        (
            Option<&Position>,
            Option<&Rotation>,
            Option<&ComputedMass>,
            Option<&ComputedCenterOfMass>,
            Option<&ComputedAngularInertia>,
            Option<&avian3d::prelude::RigidBodyColliders>,
        ),
        With<RigidBody>,
    >,
    wheels: Query<(
        Entity,
        &UsdPrimPath,
        &WheelRaycast,
        &Suspension,
        &avian3d::prelude::RayHits,
        Option<&GlobalEntityId>,
        Option<&SignalSource>,
    )>,
) {
    queued_signal_sources.clear();
    let Some(settings) = settings else {
        state.next_sample_time = None;
        state.last_sample_time = None;
        state.sample_rate_hz = None;
        state.metadata.clear();
        state.metadata_group_paths.clear();
        return;
    };
    if !settings.enabled || !settings.default_rate_hz.is_finite() || settings.default_rate_hz <= 0.0
    {
        state.next_sample_time = None;
        state.last_sample_time = None;
        state.sample_rate_hz = None;
        state.metadata.clear();
        state.metadata_group_paths.clear();
        return;
    }
    let Some(signals) = signals.as_deref_mut() else {
        state.next_sample_time = None;
        state.last_sample_time = None;
        state.sample_rate_hz = None;
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
        state.metadata_group_paths.remove(&entity);
        state.metadata.retain(|(source, _), _| *source != entity);
    };
    for entity in removed_bodies.read() {
        retire(entity);
    }
    for entity in removed_wheels.read() {
        retire(entity);
    }

    let sample_interval = 1.0 / settings.default_rate_hz;
    if state.sample_rate_hz != Some(settings.default_rate_hz) {
        state.sample_rate_hz = Some(settings.default_rate_hz);
        state.next_sample_time = None;
    }
    if state.last_sample_time.is_some_and(|last| time < last) {
        state.next_sample_time = None;
    }
    let sample_due = !state.next_sample_time.is_some_and(|next_sample_time| {
        // The mission clock is derived from integer ticks, so a nominal
        // 0.1 s boundary can land a few ulps below the decimal value.
        // Treat that representation noise as due rather than delaying
        // the batch by another fixed step.
        time + sample_interval * 1.0e-9 < next_sample_time
    });
    if sample_due {
        state.last_sample_time = Some(time);
        state.next_sample_time = Some(time + sample_interval);
    }

    // The registry owns the channel catalog. Snapshot its size once per fixed
    // pass without walking every retained history.
    let mut channel_count = signals.scalar_count();

    for (entity, prim, linear, angular, global_owner, signal_source, acceleration) in &bodies {
        let metadata_dirty = state
            .metadata_group_paths
            .get(&entity)
            .is_none_or(|path| path != &prim.path);
        let linear_velocity = linear.map(|value| value.0.as_dvec3());
        let angular_velocity = angular.map(|value| value.0.as_dvec3());
        if linear_velocity.is_some_and(|value| !value.is_finite())
            || angular_velocity.is_some_and(|value| !value.is_finite())
        {
            continue;
        }

        if !sample_due && !metadata_dirty {
            continue;
        }

        if metadata_dirty {
            state.metadata_group_paths.insert(entity, prim.path.clone());
        }

        let Ok((position, rotation, mass, center_of_mass, inertia, colliders)) =
            body_state.get(entity)
        else {
            continue;
        };

        let samples = &mut *sample_buffer;
        samples.clear();
        if let Some(linear_velocity) = linear_velocity {
            samples.extend(vector_channels(
                "linear_velocity",
                linear_velocity,
                "m/s",
                "World-frame linear-velocity component.",
            ));
            samples.push(PhysicsSample::summary(
                "linear_speed",
                "linear_velocity",
                "speed",
                "magnitude",
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
            samples.push(PhysicsSample::summary(
                "angular_speed",
                "angular_velocity",
                "speed",
                "magnitude",
                angular_velocity.length(),
                "rad/s",
                "Magnitude of world-frame angular velocity.",
            ));
        }

        // Physics owns the interval, including constraints and contacts.
        // Telemetry retains the completed result at its own configured rate.
        if let Some(acceleration) = acceleration.filter(|s| s.valid && s.tick == Some(tick.0)) {
            for (name, vector, unit, description) in [
                (
                    "linear_acceleration",
                    acceleration.linear_world,
                    "m/s^2",
                    "Net acceleration of the body's centre of mass in the physics frame.",
                ),
                (
                    "angular_acceleration",
                    acceleration.angular_world,
                    "rad/s^2",
                    "Angular acceleration in the physics frame.",
                ),
            ] {
                samples.extend(vector_channels(name, vector, unit, description));
                samples.push(PhysicsSample::summary(
                    name,
                    name,
                    "magnitude",
                    "magnitude",
                    vector.length(),
                    unit,
                    description,
                ));
            }
            if let Some(force) = acceleration.specific_force_world {
                samples.extend(vector_channels(
                    "specific_force",
                    force,
                    "m/s^2",
                    "Centre-of-mass acceleration minus gravitational acceleration, physics frame.",
                ));
                samples.push(PhysicsSample::summary("specific_force", "specific_force", "magnitude", "magnitude", force.length(), "m/s^2", "Accelerometer magnitude |a - g| at the centre of mass. Zero in ideal free fall; support load while parked."));
            }
            if let Some(force) = acceleration.specific_force_body {
                samples.extend(vector_channels("specific_force_body", force, "m/s^2", "Centre-of-mass acceleration minus gravity, expressed in the completed body frame."));
            }
        }

        if let Some(position) = position
            .map(|value| value.0)
            .filter(|value| value.is_finite())
        {
            samples.extend(explicit_vector_channels(
                position,
                "m",
                "Physics-frame position component.",
                "position",
            ));
        }
        if let Some(rotation) = rotation
            .map(|value| value.0.as_dquat())
            .filter(|value| value.is_finite())
        {
            samples.extend([
                PhysicsSample::named_component(
                    "orientation.quat.x",
                    "orientation",
                    "quaternion x",
                    rotation.x,
                    "1",
                    "Physics-body orientation quaternion x component.",
                ),
                PhysicsSample::named_component(
                    "orientation.quat.y",
                    "orientation",
                    "quaternion y",
                    rotation.y,
                    "1",
                    "Physics-body orientation quaternion y component.",
                ),
                PhysicsSample::named_component(
                    "orientation.quat.z",
                    "orientation",
                    "quaternion z",
                    rotation.z,
                    "1",
                    "Physics-body orientation quaternion z component.",
                ),
                PhysicsSample::named_component(
                    "orientation.quat.w",
                    "orientation",
                    "quaternion w",
                    rotation.w,
                    "1",
                    "Physics-body orientation quaternion w component.",
                ),
            ]);
            let (yaw, pitch, roll) = rotation.to_euler(bevy::math::EulerRot::YXZ);
            samples.extend([
                PhysicsSample::named_component(
                    "yaw",
                    "orientation",
                    "yaw",
                    yaw,
                    "rad",
                    "Physics-body yaw about the world-up axis.",
                ),
                PhysicsSample::named_component(
                    "pitch",
                    "orientation",
                    "pitch",
                    pitch,
                    "rad",
                    "Physics-body pitch in the YXZ attitude convention.",
                ),
                PhysicsSample::named_component(
                    "roll",
                    "orientation",
                    "roll",
                    roll,
                    "rad",
                    "Physics-body roll in the YXZ attitude convention.",
                ),
            ]);
        }
        if let Some(mass) = mass.filter(|value| value.is_finite()) {
            samples.push(PhysicsSample::scalar(
                "mass",
                f64::from(mass.value()),
                "kg",
                "Computed rigid-body mass including admitted colliders.",
            ));
        }
        if let Some(center_of_mass) = center_of_mass
            .map(|value| value.0.as_dvec3())
            .filter(|value| value.is_finite())
        {
            samples.extend(explicit_vector_channels(
                center_of_mass,
                "m",
                "Computed body-local center-of-mass component.",
                "center_of_mass",
            ));
        }
        if let Some(inertia) = inertia.map(|value| value.value()) {
            samples.extend([
                PhysicsSample::named_component(
                    "inertia.xx",
                    "inertia",
                    "xx",
                    f64::from(inertia.m00),
                    "kg.m^2",
                    "Computed body-local angular-inertia tensor diagonal.",
                ),
                PhysicsSample::named_component(
                    "inertia.xy",
                    "inertia",
                    "xy",
                    f64::from(inertia.m01),
                    "kg.m^2",
                    "Computed body-local angular-inertia tensor component.",
                ),
                PhysicsSample::named_component(
                    "inertia.xz",
                    "inertia",
                    "xz",
                    f64::from(inertia.m02),
                    "kg.m^2",
                    "Computed body-local angular-inertia tensor component.",
                ),
                PhysicsSample::named_component(
                    "inertia.yy",
                    "inertia",
                    "yy",
                    f64::from(inertia.m11),
                    "kg.m^2",
                    "Computed body-local angular-inertia tensor diagonal.",
                ),
                PhysicsSample::named_component(
                    "inertia.yz",
                    "inertia",
                    "yz",
                    f64::from(inertia.m12),
                    "kg.m^2",
                    "Computed body-local angular-inertia tensor component.",
                ),
                PhysicsSample::named_component(
                    "inertia.zz",
                    "inertia",
                    "zz",
                    f64::from(inertia.m22),
                    "kg.m^2",
                    "Computed body-local angular-inertia tensor diagonal.",
                ),
            ]);
        }
        if let Some(graph) = contact_graph
            .as_deref()
            .filter(|_| colliders.is_some_and(|colliders| !colliders.is_empty()))
        {
            let (contact, contact_force) = lunco_physics::contact_of(
                graph,
                colliders.into_iter().flat_map(|colliders| colliders.iter()),
            );
            samples.extend([
                PhysicsSample::scalar(
                    "contact",
                    contact as u8 as f64,
                    "1",
                    "Whether this collider is touching a physical contact pair.",
                ),
                PhysicsSample::scalar(
                    "contact_force",
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
            global_owner.copied(),
            &prim.path,
            time,
            samples.drain(..),
            &mut channel_count,
            &mut state.metadata,
            metadata_dirty,
        ) && signal_source.is_none()
            && queued_signal_sources.insert(entity)
        {
            commands.entity(entity).try_insert(SignalSource);
        }
    }

    for (entity, prim, wheel, suspension, hits, global_owner, signal_source) in &wheels {
        let metadata_dirty = state
            .metadata_group_paths
            .get(&entity)
            .is_none_or(|path| path != &prim.path);
        if !sample_due && !metadata_dirty {
            continue;
        }
        if metadata_dirty {
            state.metadata_group_paths.insert(entity, prim.path.clone());
        }
        let contact = hits
            .iter_sorted()
            .find(|hit| hit.normal.is_finite() && hit.normal.length_squared() > 1.0e-12);
        let distance = contact.map_or(suspension.rest_length, |hit| f64::from(hit.distance));
        let samples = [
            PhysicsSample::scalar(
                "suspension.compression",
                (suspension.rest_length - distance).max(0.0),
                "m",
                "Raycast suspension compression from the live support hit.",
            ),
            PhysicsSample::scalar(
                "suspension.ground_distance",
                distance,
                "m",
                "Distance from the authored suspension ray origin to the live support hit.",
            ),
            PhysicsSample::scalar(
                "suspension.normal_force",
                wheel.last_normal_force,
                "N",
                "Suspension normal force applied during the last physics tick.",
            ),
            PhysicsSample::scalar(
                "suspension.contact",
                contact.is_some() as u8 as f64,
                "1",
                "Whether the suspension ray has a valid non-degenerate support hit.",
            ),
            PhysicsSample::scalar(
                "wheel.axle_angular_velocity",
                wheel.axle_angular_velocity(),
                "rad/s",
                "Signed wheel axle angular velocity from the mobility model.",
            ),
            PhysicsSample::scalar(
                "wheel.surface_speed",
                wheel.surface_speed(),
                "m/s",
                "Wheel contact-patch speed implied by axle angular velocity.",
            ),
        ];
        if retain_samples(
            signals,
            settings.as_ref(),
            entity,
            global_owner.copied(),
            &prim.path,
            time,
            samples,
            &mut channel_count,
            &mut state.metadata,
            metadata_dirty,
        ) && signal_source.is_none()
            && queued_signal_sources.insert(entity)
        {
            commands.entity(entity).try_insert(SignalSource);
        }
    }
}

fn explicit_vector_channels(
    value: DVec3,
    unit: &'static str,
    description: &'static str,
    group: &'static str,
) -> [PhysicsSample; 3] {
    [
        PhysicsSample::component(group, "x", value.x, unit, description),
        PhysicsSample::component(group, "y", value.y, unit, description),
        PhysicsSample::component(group, "z", value.z, unit, description),
    ]
}

fn vector_channels(
    prefix: &'static str,
    value: DVec3,
    unit: &'static str,
    description: &'static str,
) -> [PhysicsSample; 3] {
    explicit_vector_channels(value, unit, description, prefix)
}

struct PhysicsSample {
    name: PhysicsSignalName,
    value: f64,
    unit: &'static str,
    description: &'static str,
    presentation: PhysicsSamplePresentation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PhysicsSignalName {
    Path(&'static str),
    Component {
        group: &'static str,
        component: &'static str,
    },
}

impl PhysicsSignalName {
    fn as_path(self) -> Cow<'static, str> {
        match self {
            Self::Path(path) => Cow::Borrowed(path),
            Self::Component { group, component } => Cow::Owned(format!("{group}.{component}")),
        }
    }
}

struct CachedPhysicsSignal {
    signal: SignalRef,
    /// Whether this source published its static metadata to the signal registry.
    metadata_published: bool,
    global_owner: Option<GlobalEntityId>,
}

/// Borrowed static presentation facts for a sample; owned labels are built only
/// when the shared signal catalog needs metadata.
#[derive(Clone, Copy)]
enum PhysicsSamplePresentation {
    Scalar,
    Component {
        group: &'static str,
        component: &'static str,
    },
    Summary {
        group: &'static str,
        label: &'static str,
        formula: &'static str,
    },
}

impl PhysicsSamplePresentation {
    fn to_signal_presentation(self) -> SignalPresentation {
        match self {
            Self::Scalar => SignalPresentation::Scalar,
            Self::Component { group, component } => SignalPresentation::Component {
                group: group.to_string(),
                component: component.to_string(),
            },
            Self::Summary {
                group,
                label,
                formula,
            } => SignalPresentation::Summary {
                group: group.to_string(),
                label: label.to_string(),
                formula: formula.to_string(),
            },
        }
    }
}

impl PhysicsSample {
    fn scalar(
        name: &'static str,
        value: f64,
        unit: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            name: PhysicsSignalName::Path(name),
            value,
            unit,
            description,
            presentation: PhysicsSamplePresentation::Scalar,
        }
    }

    fn component(
        group: &'static str,
        component: &'static str,
        value: f64,
        unit: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            name: PhysicsSignalName::Component { group, component },
            value,
            unit,
            description,
            presentation: PhysicsSamplePresentation::Component { group, component },
        }
    }

    fn named_component(
        name: &'static str,
        group: &'static str,
        component: &'static str,
        value: f64,
        unit: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            name: PhysicsSignalName::Path(name),
            value,
            unit,
            description,
            presentation: PhysicsSamplePresentation::Component { group, component },
        }
    }

    fn summary(
        name: &'static str,
        group: &'static str,
        label: &'static str,
        formula: &'static str,
        value: f64,
        unit: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            name: PhysicsSignalName::Path(name),
            value,
            unit,
            description,
            presentation: PhysicsSamplePresentation::Summary {
                group,
                label,
                formula,
            },
        }
    }
}

fn retain_samples(
    signals: &mut SignalRegistry,
    settings: &TelemetrySettings,
    entity: Entity,
    global_owner: Option<GlobalEntityId>,
    group_path: &str,
    time: f64,
    samples: impl IntoIterator<Item = PhysicsSample>,
    channel_count: &mut usize,
    metadata: &mut HashMap<(Entity, PhysicsSignalName), CachedPhysicsSignal>,
    metadata_dirty: bool,
) -> bool {
    let _span = bevy::log::debug_span!(
        "physics_telemetry_source_retention",
        metadata_dirty,
        entity = entity.to_bits(),
        channels = *channel_count
    )
    .entered();
    let mut retained = false;
    for sample in samples {
        if !sample.value.is_finite() {
            continue;
        }
        let key = (entity, sample.name);
        let cached = match metadata.entry(key) {
            Entry::Occupied(entry) => {
                let cached = entry.into_mut();
                if !physics_channel_allowed(signals, &cached.signal, *channel_count, settings) {
                    continue;
                }
                cached
            }
            Entry::Vacant(entry) => {
                let signal = SignalRef::new(entity, sample.name.as_path().into_owned());
                if !physics_channel_allowed(signals, &signal, *channel_count, settings) {
                    continue;
                }
                entry.insert(CachedPhysicsSignal {
                    signal,
                    metadata_published: false,
                    global_owner: None,
                })
            }
        };
        let signal = &cached.signal;
        {
            let _span = (metadata_dirty || !cached.metadata_published).then(|| {
                let known = signals.scalar_history(signal).is_some();
                bevy::log::debug_span!("physics_telemetry_channel_metadata", known).entered()
            });
            if let Some(owner) = global_owner {
                if cached.global_owner != Some(owner) {
                    signals.associate_global_owner(signal, owner);
                    cached.global_owner = Some(owner);
                }
            }
            if metadata_dirty || !cached.metadata_published {
                let signal_meta = SignalMeta {
                    description: Some(sample.description.to_string()),
                    unit: Some(sample.unit.to_string()),
                    provenance: Some("avian".to_string()),
                    group_path: Some(group_path.to_string()),
                    presentation: sample.presentation.to_signal_presentation(),
                    exposure: Default::default(),
                    ..Default::default()
                };
                signals.update_meta(signal.clone(), signal_meta);
                cached.metadata_published = true;
            }
        }
        if signals.record_scalar_at_rate(
            signal,
            time,
            sample.value,
            settings.default_rate_hz,
            settings.default_retention,
        ) {
            retained = true;
            *channel_count = signals.scalar_count();
        }
    }
    retained
}

#[inline]
fn physics_channel_allowed(
    signals: &SignalRegistry,
    signal: &SignalRef,
    channel_count: usize,
    settings: &TelemetrySettings,
) -> bool {
    // Below the limit append can admit any history; only a full catalog needs
    // the extra lookup. Cached producer identity does not prove live history.
    if channel_count >= settings.max_channels && signals.scalar_history(signal).is_none() {
        warn_once!(
            "physics telemetry: max_channels ({}) reached; additional state is not retained",
            settings.max_channels
        );
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Isolates the producer/registry seam with settled metadata and bounded histories.
    #[test]
    #[ignore = "manual retention performance measurement"]
    fn settled_retention_benchmark() {
        let mut world = World::new();
        let entities: Vec<_> = (0..128).map(|_| world.spawn_empty().id()).collect();
        let settings = TelemetrySettings {
            default_rate_hz: 1.0,
            default_retention: 16,
            ..Default::default()
        };
        let mut signals = SignalRegistry::default();
        let mut metadata = HashMap::new();
        let mut count = 0;
        let samples = vector_channels(
            "linear_velocity",
            DVec3::new(1.0, 2.0, 3.0),
            "m/s",
            "Velocity.",
        );
        for &entity in &entities {
            retain_samples(
                &mut signals,
                &settings,
                entity,
                None,
                "/Body",
                0.0,
                vector_channels(
                    "linear_velocity",
                    DVec3::new(1.0, 2.0, 3.0),
                    "m/s",
                    "Velocity.",
                ),
                &mut count,
                &mut metadata,
                true,
            );
        }
        let began = std::time::Instant::now();
        for time in 1..=6000 {
            for &entity in &entities {
                assert!(retain_samples(
                    &mut signals,
                    &settings,
                    entity,
                    None,
                    "/Body",
                    f64::from(time),
                    vector_channels(
                        "linear_velocity",
                        DVec3::new(1.0, 2.0, 3.0),
                        "m/s",
                        "Velocity."
                    ),
                    &mut count,
                    &mut metadata,
                    false
                ));
            }
        }
        let elapsed = began.elapsed();
        assert_eq!(count, 384);
        let mut digest = 0xcbf29ce484222325u64;
        for entity in entities {
            for sample in &samples {
                let signal = SignalRef::new(entity, sample.name.as_path().into_owned());
                let history = signals.scalar_history(&signal).unwrap();
                assert_eq!(history.len(), 16);
                for point in history.iter() {
                    digest = (digest ^ point.time.to_bits()).wrapping_mul(0x100000001b3);
                    digest = (digest ^ point.value.to_bits()).wrapping_mul(0x100000001b3);
                }
            }
        }
        println!(
            "RETENTION_BENCH elapsed_ns={} samples=2304000 channels={} digest={digest:016x}",
            elapsed.as_nanos(),
            signals.scalar_count()
        );
    }

    #[test]
    fn channel_admission_follows_live_registry_after_limit_and_removal() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut settings = TelemetrySettings {
            default_rate_hz: 1.0,
            max_channels: 1,
            ..Default::default()
        };
        let mut signals = SignalRegistry::default();
        let mut metadata = HashMap::new();
        let mut count = 0;
        let mut record = |signals: &mut SignalRegistry, settings: &TelemetrySettings, time| {
            // Production snapshots the live count at the start of each batch.
            count = signals.scalar_count();
            retain_samples(
                signals,
                settings,
                entity,
                None,
                "/Body",
                time,
                vector_channels("linear_velocity", DVec3::X, "m/s", "Velocity."),
                &mut count,
                &mut metadata,
                false,
            )
        };
        assert!(record(&mut signals, &settings, 0.0));
        let signal = SignalRef::new(entity, "linear_velocity.x");
        assert_eq!(signals.scalar_count(), 1);
        assert!(
            signals
                .scalar_history(&SignalRef::new(entity, "linear_velocity.y"))
                .is_none()
        );
        settings.max_channels = 0;
        assert!(record(&mut signals, &settings, 1.0));
        assert_eq!(signals.scalar_history(&signal).unwrap().len(), 2);
        signals.remove_signal(&signal);
        assert!(!record(&mut signals, &settings, 2.0));
        assert_eq!(signals.scalar_count(), 0);
        settings.max_channels = 1;
        assert!(record(&mut signals, &settings, 3.0));
        assert_eq!(signals.scalar_count(), 1);
        let point = signals.scalar_history(&signal).unwrap().back().unwrap();
        assert_eq!(point.time.to_bits(), 3.0f64.to_bits());
        assert_eq!(point.value.to_bits(), 1.0f64.to_bits());
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
        assert!(linear.iter().all(|sample| sample.unit == "m/s^2"));
        assert!(angular.iter().all(|sample| sample.unit == "rad/s^2"));
    }

    #[test]
    fn vector_channels_publish_typed_component_presentation() {
        let samples = vector_channels(
            "linear_velocity",
            DVec3::new(1.0, 2.0, 3.0),
            "m/s",
            "World-frame linear-velocity component.",
        );

        assert_eq!(samples[0].name.as_path().as_ref(), "linear_velocity.x");
        assert_eq!(
            samples[0].presentation.to_signal_presentation(),
            SignalPresentation::Component {
                group: "linear_velocity".into(),
                component: "x".into(),
            }
        );
        assert_eq!(samples[2].name.as_path().as_ref(), "linear_velocity.z");
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
                SolvedBodyAcceleration::default(),
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
        {
            let mut sample = app
                .world_mut()
                .get_mut::<SolvedBodyAcceleration>(body)
                .unwrap();
            sample.linear_world = DVec3::X * 12.0;
            sample.tick = Some(6);
            sample.valid = true;
        }
        app.update();

        let signal = SignalRef::new(body, "linear_acceleration.x");
        let registry = app.world().resource::<SignalRegistry>();
        let sample = registry
            .scalar_history(&signal)
            .and_then(lunco_signal::ScalarHistory::back)
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
        assert!(
            app.world()
                .resource::<SignalRegistry>()
                .scalar_history(&signal)
                .is_some()
        );

        app.world_mut().entity_mut(body).remove::<RigidBody>();
        app.update();

        let state = app.world().resource::<PhysicsTelemetryState>();
        assert!(!state.metadata_group_paths.contains_key(&body));
        assert!(state.metadata.keys().all(|(entity, _)| *entity != body));
        assert!(
            app.world()
                .resource::<SignalRegistry>()
                .scalar_history(&signal)
                .is_some()
        );
    }
}
