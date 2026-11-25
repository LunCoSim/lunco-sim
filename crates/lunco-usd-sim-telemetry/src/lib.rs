//! Runtime telemetry for physical rigid-body state.
//!
//! Physics state is discovered from the live Avian body, not from authored
//! telemetry declarations. USD supplies the owning prim path for presentation
//! and Avian supplies the measured world-frame kinematics. Acceleration is the
//! physics-owned native-interval measurement, shared with co-simulation and
//! sensors; this recorder does not calculate a second derivative.

use avian3d::prelude::{
    AngularVelocity, ComputedAngularInertia, ComputedCenterOfMass, ComputedMass, ContactGraph,
    LinearVelocity, Position, RigidBody, Rotation,
};
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_core::{AppSceneExt, GlobalEntityId, SceneResource};
use lunco_core_runtime::SimTick;
use lunco_mobility::{Suspension, WheelRaycast};
use lunco_physics::kinematics::SolvedBodyAcceleration;
use lunco_signal::{
    ScalarRecordingBinding, SignalMeta, SignalPresentation, SignalRef, SignalRegistry, SignalSource,
};
use lunco_telemetry::TelemetrySettings;
use lunco_time::MissionClock;
use std::collections::{HashMap, HashSet};

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
                FixedLast,
                retain_physics_telemetry
                    .after(lunco_time::FixedCycleVerificationSet)
                    .run_if(lunco_time::fixed_cycle_integrated),
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
    sources: HashMap<Entity, SourceRecorders>,
}

impl SceneResource for PhysicsTelemetryState {}

/// Retain post-step rigid-body kinematics in the shared signal registry.
///
/// The system runs in `FixedLast` after native clock verification, so
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
        state.sources.clear();
        return;
    };
    if !settings.enabled || !settings.default_rate_hz.is_finite() || settings.default_rate_hz <= 0.0
    {
        state.next_sample_time = None;
        state.last_sample_time = None;
        state.sample_rate_hz = None;
        state.sources.clear();
        return;
    }
    let Some(signals) = signals.as_deref_mut() else {
        state.next_sample_time = None;
        state.last_sample_time = None;
        state.sample_rate_hz = None;
        state.sources.clear();
        return;
    };

    // `FixedLast` may run several times in one rendered frame. The
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
        state.sources.remove(&entity);
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

    for (
        entity,
        prim,
        linear,
        angular,
        global_owner,
        signal_source,
        acceleration,
        position,
        rotation,
        mass,
        center_of_mass,
        inertia,
        colliders,
    ) in &bodies
    {
        let metadata_dirty = state
            .sources
            .get(&entity)
            .is_none_or(|source| source.group_path != prim.path);
        if !sample_due && !metadata_dirty {
            continue;
        }
        let linear_velocity = linear.map(|value| value.0.as_dvec3());
        let angular_velocity = angular.map(|value| value.0.as_dvec3());
        if linear_velocity.is_some_and(|value| !value.is_finite())
            || angular_velocity.is_some_and(|value| !value.is_finite())
        {
            continue;
        }

        let samples = &mut *sample_buffer;
        samples.clear();
        if let Some(linear_velocity) = linear_velocity {
            samples.extend(vector_channels(LINEAR_VELOCITY, linear_velocity));
            samples.push(PhysicsSample::new(
                PhysicsChannel::LinearSpeed,
                linear_velocity.length(),
            ));
        }
        if let Some(angular_velocity) = angular_velocity {
            samples.extend(vector_channels(ANGULAR_VELOCITY, angular_velocity));
            samples.push(PhysicsSample::new(
                PhysicsChannel::AngularSpeed,
                angular_velocity.length(),
            ));
        }

        // Physics owns the interval, including constraints and contacts.
        // Telemetry retains the completed result at its own configured rate.
        if let Some(acceleration) = acceleration.filter(|s| s.valid && s.tick == Some(tick.0)) {
            for (channels, magnitude, vector) in [
                (
                    LINEAR_ACCELERATION,
                    PhysicsChannel::LinearAcceleration,
                    acceleration.linear_world,
                ),
                (
                    ANGULAR_ACCELERATION,
                    PhysicsChannel::AngularAcceleration,
                    acceleration.angular_world,
                ),
            ] {
                samples.extend(vector_channels(channels, vector));
                samples.push(PhysicsSample::new(magnitude, vector.length()));
            }
            if let Some(force) = acceleration.specific_force_world {
                samples.extend(vector_channels(SPECIFIC_FORCE, force));
                samples.push(PhysicsSample::new(
                    PhysicsChannel::SpecificForce,
                    force.length(),
                ));
            }
            if let Some(force) = acceleration.specific_force_body {
                samples.extend(vector_channels(SPECIFIC_FORCE_BODY, force));
            }
        }

        if let Some(position) = position
            .map(|value| value.0)
            .filter(|value| value.is_finite())
        {
            samples.extend(vector_channels(POSITION, position));
        }
        if let Some(rotation) = rotation
            .map(|value| value.0.as_dquat())
            .filter(|value| value.is_finite())
        {
            samples.extend([
                PhysicsSample::new(PhysicsChannel::OrientationQuatX, rotation.x),
                PhysicsSample::new(PhysicsChannel::OrientationQuatY, rotation.y),
                PhysicsSample::new(PhysicsChannel::OrientationQuatZ, rotation.z),
                PhysicsSample::new(PhysicsChannel::OrientationQuatW, rotation.w),
            ]);
            let (yaw, pitch, roll) = rotation.to_euler(bevy::math::EulerRot::YXZ);
            samples.extend([
                PhysicsSample::new(PhysicsChannel::Yaw, yaw),
                PhysicsSample::new(PhysicsChannel::Pitch, pitch),
                PhysicsSample::new(PhysicsChannel::Roll, roll),
            ]);
        }
        if let Some(mass) = mass.filter(|value| value.is_finite()) {
            samples.push(PhysicsSample::new(
                PhysicsChannel::Mass,
                f64::from(mass.value()),
            ));
        }
        if let Some(center_of_mass) = center_of_mass
            .map(|value| value.0.as_dvec3())
            .filter(|value| value.is_finite())
        {
            samples.extend(vector_channels(CENTER_OF_MASS, center_of_mass));
        }
        if let Some(inertia) = inertia.map(|value| value.value()) {
            samples.extend([
                PhysicsSample::new(PhysicsChannel::InertiaXx, f64::from(inertia.m00)),
                PhysicsSample::new(PhysicsChannel::InertiaXy, f64::from(inertia.m01)),
                PhysicsSample::new(PhysicsChannel::InertiaXz, f64::from(inertia.m02)),
                PhysicsSample::new(PhysicsChannel::InertiaYy, f64::from(inertia.m11)),
                PhysicsSample::new(PhysicsChannel::InertiaYz, f64::from(inertia.m12)),
                PhysicsSample::new(PhysicsChannel::InertiaZz, f64::from(inertia.m22)),
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
                PhysicsSample::new(PhysicsChannel::Contact, contact as u8 as f64),
                PhysicsSample::new(PhysicsChannel::ContactForce, contact_force),
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
            &mut state.sources,
        ) && signal_source.is_none()
            && queued_signal_sources.insert(entity)
        {
            commands.entity(entity).try_insert(SignalSource);
        }
    }

    for (entity, prim, wheel, suspension, hits, global_owner, signal_source) in &wheels {
        let metadata_dirty = state
            .sources
            .get(&entity)
            .is_none_or(|source| source.group_path != prim.path);
        if !sample_due && !metadata_dirty {
            continue;
        }
        let contact = hits
            .iter_sorted()
            .find(|hit| hit.normal.is_finite() && hit.normal.length_squared() > 1.0e-12);
        let distance = contact.map_or(suspension.rest_length, |hit| f64::from(hit.distance));
        let samples = [
            PhysicsSample::new(
                PhysicsChannel::SuspensionCompression,
                (suspension.rest_length - distance).max(0.0),
            ),
            PhysicsSample::new(PhysicsChannel::SuspensionGroundDistance, distance),
            PhysicsSample::new(
                PhysicsChannel::SuspensionNormalForce,
                wheel.last_normal_force,
            ),
            PhysicsSample::new(
                PhysicsChannel::SuspensionContact,
                contact.is_some() as u8 as f64,
            ),
            PhysicsSample::new(
                PhysicsChannel::WheelAxleAngularVelocity,
                wheel.axle_angular_velocity(),
            ),
            PhysicsSample::new(PhysicsChannel::WheelSurfaceSpeed, wheel.surface_speed()),
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
            &mut state.sources,
        ) && signal_source.is_none()
            && queued_signal_sources.insert(entity)
        {
            commands.entity(entity).try_insert(SignalSource);
        }
    }
}

/// Authored signal names and presentation are fixed by the physics recorder.
/// Numeric samples carry only a dense channel identity and an f64 value.
macro_rules! physics_channels {
    (vectors { $( $group:ident: [$x:ident, $y:ident, $z:ident] => ($path:literal, $unit:literal, $description:literal) ),* $(,)? }
     scalars { $( $channel:ident => ($name:literal, $scalar_unit:literal, $scalar_description:literal, $presentation:expr) ),* $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(usize)]
        enum PhysicsChannel { $($x, $y, $z,)* $($channel,)* }
        $(const $group: [PhysicsChannel; 3] = [PhysicsChannel::$x, PhysicsChannel::$y, PhysicsChannel::$z];)*
        impl PhysicsChannel {
            const COUNT: usize = [$(Self::$x, Self::$y, Self::$z,)* $(Self::$channel,)*].len();
            fn descriptor(self) -> PhysicsChannelDescriptor {
                match self {
                    $(Self::$x => PhysicsChannelDescriptor::component(concat!($path, ".x"), $path, "x", $unit, $description),
                      Self::$y => PhysicsChannelDescriptor::component(concat!($path, ".y"), $path, "y", $unit, $description),
                      Self::$z => PhysicsChannelDescriptor::component(concat!($path, ".z"), $path, "z", $unit, $description),)*
                    $(Self::$channel => PhysicsChannelDescriptor { path: $name, unit: $scalar_unit, description: $scalar_description, presentation: $presentation },)*
                }
            }
        }
    };
}

physics_channels! {
    vectors {
        LINEAR_VELOCITY: [LinearVelocityX, LinearVelocityY, LinearVelocityZ] => ("linear_velocity", "m/s", "World-frame linear-velocity component."),
        ANGULAR_VELOCITY: [AngularVelocityX, AngularVelocityY, AngularVelocityZ] => ("angular_velocity", "rad/s", "World-frame angular-rate component."),
        LINEAR_ACCELERATION: [LinearAccelerationX, LinearAccelerationY, LinearAccelerationZ] => ("linear_acceleration", "m/s^2", "Net acceleration of the body's centre of mass in the physics frame."),
        ANGULAR_ACCELERATION: [AngularAccelerationX, AngularAccelerationY, AngularAccelerationZ] => ("angular_acceleration", "rad/s^2", "Net angular acceleration of the body in the physics frame."),
        SPECIFIC_FORCE: [SpecificForceX, SpecificForceY, SpecificForceZ] => ("specific_force", "m/s^2", "Centre-of-mass acceleration minus gravitational acceleration, physics frame."),
        SPECIFIC_FORCE_BODY: [SpecificForceBodyX, SpecificForceBodyY, SpecificForceBodyZ] => ("specific_force_body", "m/s^2", "Centre-of-mass acceleration minus gravity, expressed in the completed body frame."),
        POSITION: [PositionX, PositionY, PositionZ] => ("position", "m", "Physics-frame position component."),
        CENTER_OF_MASS: [CenterOfMassX, CenterOfMassY, CenterOfMassZ] => ("center_of_mass", "m", "Computed body-local center-of-mass component."),
    }
    scalars {
        LinearSpeed => ("linear_speed", "m/s", "Magnitude of world-frame linear velocity.", PhysicsSamplePresentation::Summary { group: "linear_velocity", label: "speed", formula: "magnitude" }),
        AngularSpeed => ("angular_speed", "rad/s", "Magnitude of world-frame angular velocity.", PhysicsSamplePresentation::Summary { group: "angular_velocity", label: "speed", formula: "magnitude" }),
        SpecificForce => ("specific_force", "m/s^2", "Accelerometer magnitude |a - g| at the centre of mass. Zero in ideal free fall; support load while parked.", PhysicsSamplePresentation::Summary { group: "specific_force", label: "magnitude", formula: "magnitude" }),
        OrientationQuatX => ("orientation.quat.x", "1", "Physics-body orientation quaternion x component.", PhysicsSamplePresentation::Component { group: "orientation", component: "quaternion x" }),
        OrientationQuatY => ("orientation.quat.y", "1", "Physics-body orientation quaternion y component.", PhysicsSamplePresentation::Component { group: "orientation", component: "quaternion y" }),
        OrientationQuatZ => ("orientation.quat.z", "1", "Physics-body orientation quaternion z component.", PhysicsSamplePresentation::Component { group: "orientation", component: "quaternion z" }),
        OrientationQuatW => ("orientation.quat.w", "1", "Physics-body orientation quaternion w component.", PhysicsSamplePresentation::Component { group: "orientation", component: "quaternion w" }),
        Yaw => ("yaw", "rad", "Physics-body yaw about the world-up axis.", PhysicsSamplePresentation::Component { group: "orientation", component: "yaw" }),
        Pitch => ("pitch", "rad", "Physics-body pitch in the YXZ attitude convention.", PhysicsSamplePresentation::Component { group: "orientation", component: "pitch" }),
        Roll => ("roll", "rad", "Physics-body roll in the YXZ attitude convention.", PhysicsSamplePresentation::Component { group: "orientation", component: "roll" }),
        Mass => ("mass", "kg", "Computed rigid-body mass including admitted colliders.", PhysicsSamplePresentation::Scalar),
        InertiaXx => ("inertia.xx", "kg.m^2", "Computed body-local angular-inertia tensor diagonal.", PhysicsSamplePresentation::Component { group: "inertia", component: "xx" }),
        InertiaXy => ("inertia.xy", "kg.m^2", "Computed body-local angular-inertia tensor component.", PhysicsSamplePresentation::Component { group: "inertia", component: "xy" }),
        InertiaXz => ("inertia.xz", "kg.m^2", "Computed body-local angular-inertia tensor component.", PhysicsSamplePresentation::Component { group: "inertia", component: "xz" }),
        InertiaYy => ("inertia.yy", "kg.m^2", "Computed body-local angular-inertia tensor diagonal.", PhysicsSamplePresentation::Component { group: "inertia", component: "yy" }),
        InertiaYz => ("inertia.yz", "kg.m^2", "Computed body-local angular-inertia tensor component.", PhysicsSamplePresentation::Component { group: "inertia", component: "yz" }),
        InertiaZz => ("inertia.zz", "kg.m^2", "Computed body-local angular-inertia tensor diagonal.", PhysicsSamplePresentation::Component { group: "inertia", component: "zz" }),
        Contact => ("contact", "1", "Whether this collider is touching a physical contact pair.", PhysicsSamplePresentation::Scalar),
        ContactForce => ("contact_force", "N", "Normal contact force derived from the live Avian contact impulse.", PhysicsSamplePresentation::Scalar),
        SuspensionCompression => ("suspension.compression", "m", "Raycast suspension compression from the live support hit.", PhysicsSamplePresentation::Scalar),
        SuspensionGroundDistance => ("suspension.ground_distance", "m", "Distance from the authored suspension ray origin to the live support hit.", PhysicsSamplePresentation::Scalar),
        SuspensionNormalForce => ("suspension.normal_force", "N", "Suspension normal force applied during the last physics tick.", PhysicsSamplePresentation::Scalar),
        SuspensionContact => ("suspension.contact", "1", "Whether the suspension ray has a valid non-degenerate support hit.", PhysicsSamplePresentation::Scalar),
        WheelAxleAngularVelocity => ("wheel.axle_angular_velocity", "rad/s", "Signed wheel axle angular velocity from the mobility model.", PhysicsSamplePresentation::Scalar),
        WheelSurfaceSpeed => ("wheel.surface_speed", "m/s", "Wheel contact-patch speed implied by axle angular velocity.", PhysicsSamplePresentation::Scalar),
        LinearAcceleration => ("linear_acceleration", "m/s^2", "Net acceleration of the body's centre of mass in the physics frame.", PhysicsSamplePresentation::Summary { group: "linear_acceleration", label: "magnitude", formula: "magnitude" }),
        AngularAcceleration => ("angular_acceleration", "m/s^2", "Net angular acceleration of the body in the physics frame.", PhysicsSamplePresentation::Summary { group: "angular_acceleration", label: "magnitude", formula: "magnitude" }),
    }
}

#[derive(Clone, Copy)]
struct PhysicsChannelDescriptor {
    path: &'static str,
    unit: &'static str,
    description: &'static str,
    presentation: PhysicsSamplePresentation,
}

impl PhysicsChannelDescriptor {
    fn component(
        path: &'static str,
        group: &'static str,
        component: &'static str,
        unit: &'static str,
        description: &'static str,
    ) -> Self {
        Self {
            path,
            unit,
            description,
            presentation: PhysicsSamplePresentation::Component { group, component },
        }
    }
}

fn vector_channels(channels: [PhysicsChannel; 3], value: DVec3) -> [PhysicsSample; 3] {
    [
        PhysicsSample::new(channels[0], value.x),
        PhysicsSample::new(channels[1], value.y),
        PhysicsSample::new(channels[2], value.z),
    ]
}

#[derive(Clone, Copy)]
struct PhysicsSample {
    channel: PhysicsChannel,
    value: f64,
}

impl PhysicsSample {
    fn new(channel: PhysicsChannel, value: f64) -> Self {
        Self { channel, value }
    }
}

#[derive(Default)]
struct ChannelRecorder {
    binding: Option<ScalarRecordingBinding>,
    refused: Option<(u64, usize)>,
    published_group: Option<u64>,
    global_owner: Option<GlobalEntityId>,
}

struct SourceRecorders {
    group_path: String,
    group_revision: u64,
    channels: Box<[ChannelRecorder; PhysicsChannel::COUNT]>,
}

impl SourceRecorders {
    fn new(group_path: &str) -> Self {
        Self {
            group_path: group_path.to_owned(),
            group_revision: 0,
            channels: Box::new(std::array::from_fn(|_| ChannelRecorder::default())),
        }
    }
    fn observe_path(&mut self, group_path: &str) {
        if self.group_path != group_path {
            self.group_path = group_path.to_owned();
            self.group_revision = self.group_revision.wrapping_add(1);
        }
    }
}

/// Static presentation metadata; owned labels are built only when the shared
/// signal catalog needs them.
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

fn retain_samples(
    signals: &mut SignalRegistry,
    settings: &TelemetrySettings,
    entity: Entity,
    global_owner: Option<GlobalEntityId>,
    group_path: &str,
    time: f64,
    samples: impl IntoIterator<Item = PhysicsSample>,
    channel_count: &mut usize,
    sources: &mut HashMap<Entity, SourceRecorders>,
) -> bool {
    let _span = bevy::log::debug_span!("physics_telemetry_source_retention").entered();
    let source = sources
        .entry(entity)
        .or_insert_with(|| SourceRecorders::new(group_path));
    source.observe_path(group_path);
    let mut retained = false;
    for sample in samples {
        if !sample.value.is_finite() {
            continue;
        }
        let slot = &mut source.channels[sample.channel as usize];
        if let Some(binding) = slot.binding.as_ref() {
            if slot.published_group == Some(source.group_revision)
                && global_owner.is_none_or(|owner| slot.global_owner == Some(owner))
            {
                match signals.record_bound_scalar(
                    binding,
                    time,
                    sample.value,
                    settings.default_rate_hz,
                    settings.default_retention,
                ) {
                    Ok(written) => {
                        retained |= written;
                        continue;
                    }
                    Err(_) => {
                        *slot = ChannelRecorder::default();
                    }
                }
            } else if !signals.recording_is_current(binding) {
                *slot = ChannelRecorder::default();
            }
        }
        // Cold path only: resolve stable names and publish metadata on admission,
        // owner change or changed USD group. A capacity refusal stays deferred
        // until the shared recorder membership or configured cap changes.
        let stamp = (signals.recording_revision(), settings.max_channels);
        if slot.binding.is_none() && slot.refused == Some(stamp) {
            continue;
        }
        let descriptor = sample.channel.descriptor();
        let signal = SignalRef::new(entity, descriptor.path);
        if slot.binding.is_none()
            && !physics_channel_allowed(signals, &signal, *channel_count, settings)
        {
            slot.refused = Some(stamp);
            continue;
        }
        slot.refused = None;
        if let Some(owner) = global_owner
            && slot.global_owner != Some(owner)
        {
            signals.associate_global_owner(&signal, owner);
            slot.global_owner = Some(owner);
        }
        if slot.published_group != Some(source.group_revision) {
            let _span = bevy::log::debug_span!("physics_telemetry_channel_metadata").entered();
            signals.update_meta(
                signal.clone(),
                SignalMeta {
                    description: Some(descriptor.description.to_owned()),
                    unit: Some(descriptor.unit.to_owned()),
                    provenance: Some("avian".to_owned()),
                    group_path: Some(source.group_path.clone()),
                    presentation: descriptor.presentation.to_signal_presentation(),
                    exposure: Default::default(),
                    ..Default::default()
                },
            );
            slot.published_group = Some(source.group_revision);
        }
        if let Some(binding) = slot.binding.as_ref() {
            match signals.record_bound_scalar(
                binding,
                time,
                sample.value,
                settings.default_rate_hz,
                settings.default_retention,
            ) {
                Ok(written) => retained |= written,
                Err(error) => error!("physics telemetry: {error}"),
            }
        } else {
            // Preserve exact named admission/rate semantics for a new or retired
            // history. Only take the binding after the history is live.
            let written = signals.record_scalar_at_rate(
                &signal,
                time,
                sample.value,
                settings.default_rate_hz,
                settings.default_retention,
            );
            retained |= written;
            if written {
                *channel_count = signals.scalar_count();
            }
            if signals.scalar_history(&signal).is_some() && signals.is_active(&signal) {
                slot.binding = signals.admit_scalar(
                    &signal,
                    settings.default_retention,
                    settings.max_channels,
                );
            }
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
        let samples = vector_channels(LINEAR_VELOCITY, DVec3::new(1.0, 2.0, 3.0));
        for &entity in &entities {
            retain_samples(
                &mut signals,
                &settings,
                entity,
                None,
                "/Body",
                0.0,
                vector_channels(LINEAR_VELOCITY, DVec3::new(1.0, 2.0, 3.0)),
                &mut count,
                &mut metadata,
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
                    vector_channels(LINEAR_VELOCITY, DVec3::new(1.0, 2.0, 3.0)),
                    &mut count,
                    &mut metadata,
                ));
            }
        }
        let elapsed = began.elapsed();
        assert_eq!(count, 384);
        let mut digest = 0xcbf29ce484222325u64;
        for entity in entities {
            for sample in &samples {
                let signal = SignalRef::new(entity, sample.channel.descriptor().path);
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
                vector_channels(LINEAR_VELOCITY, DVec3::X),
                &mut count,
                &mut metadata,
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
        assert_eq!(
            signals.meta(&signal).unwrap().group_path.as_deref(),
            Some("/Body")
        );
        let point = signals.scalar_history(&signal).unwrap().back().unwrap();
        assert_eq!(point.time.to_bits(), 3.0f64.to_bits());
        assert_eq!(point.value.to_bits(), 1.0f64.to_bits());
    }

    #[test]
    fn acceleration_channels_report_acceleration_units() {
        let linear = vector_channels(LINEAR_ACCELERATION, DVec3::X);
        let angular = vector_channels(ANGULAR_ACCELERATION, DVec3::X);
        assert!(
            linear
                .iter()
                .all(|sample| sample.channel.descriptor().unit == "m/s^2")
        );
        assert!(
            angular
                .iter()
                .all(|sample| sample.channel.descriptor().unit == "rad/s^2")
        );
    }

    #[test]
    fn vector_channels_publish_typed_component_presentation() {
        let samples = vector_channels(LINEAR_VELOCITY, DVec3::new(1.0, 2.0, 3.0));

        assert_eq!(samples[0].channel.descriptor().path, "linear_velocity.x");
        assert_eq!(
            samples[0]
                .channel
                .descriptor()
                .presentation
                .to_signal_presentation(),
            SignalPresentation::Component {
                group: "linear_velocity".into(),
                component: "x".into(),
            }
        );
        assert_eq!(samples[2].channel.descriptor().path, "linear_velocity.z");
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
                LinearVelocity(Vec3::ZERO),
                AngularVelocity(Vec3::ZERO),
                SolvedBodyAcceleration::default(),
                Position::from_xyz(2.0, 3.0, 4.0),
                Rotation(Quat::from_rotation_y(0.25)),
                ComputedMass::new(2.0),
                ComputedCenterOfMass::new(0.1, 0.2, 0.3),
                ComputedAngularInertia::new(Vec3::splat(3.0)),
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
        app.world_mut().get_mut::<LinearVelocity>(body).unwrap().0 = Vec3::X;
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
                LinearVelocity(Vec3::X),
                AngularVelocity(Vec3::ZERO),
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
        assert!(!state.sources.contains_key(&body));
        assert!(
            app.world()
                .resource::<SignalRegistry>()
                .scalar_history(&signal)
                .is_some()
        );
    }
}
