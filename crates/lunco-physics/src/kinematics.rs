//! Completed native-step acceleration at a rigid body's centre of mass.
//!
//! One solver interval supplies sensors, co-simulation ports and telemetry.
//! No derivative is taken over render frames or telemetry sampling intervals.

use avian3d::prelude::{
    AngularVelocity, Gravity, GravityScale, LinearVelocity, Physics, PhysicsSchedule,
    PhysicsStepSystems, PhysicsSystems, RigidBody, RigidBodyDisabled, Rotation,
};
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_core::PhysicsStateReady;
use lunco_core_runtime::SimTick;
use lunco_spatial::ActivePhysicsFrame;
use std::time::Duration;

/// Cached gravitational field in the body's Avian physics frame, m/s².
/// The environment owner computes this field and projects it into Avian's
/// `ConstantLinearAcceleration`. Other constant accelerations are not gravity.
#[derive(Component, Debug, Clone, Copy, Reflect, Default)]
#[reflect(Component)]
pub struct LocalGravity {
    acceleration: DVec3,
    change_origin: GravityChangeOrigin,
}

/// Distinguishes a changed field source from resampling at a moving consumer.
/// Projection can refresh the latter without notifying native force wake-up.
#[derive(Debug, Clone, Copy, Reflect, Default, PartialEq, Eq)]
pub enum GravityChangeOrigin {
    /// The field definition, provider pose, or reference frame changed.
    #[default]
    Source,
    /// An unchanged field was sampled at the consumer's current position.
    ConsumerPose,
}

impl LocalGravity {
    /// An externally supplied field; a changed value may wake native physics.
    pub fn new(acceleration: DVec3) -> Self {
        Self::sampled(acceleration, GravityChangeOrigin::Source)
    }

    /// A computed field with explicit source-versus-consumer provenance.
    pub fn sampled(acceleration: DVec3, change_origin: GravityChangeOrigin) -> Self {
        Self {
            acceleration,
            change_origin,
        }
    }

    /// Exact acceleration in the active physics frame, in m/s².
    pub fn acceleration(&self) -> DVec3 {
        self.acceleration
    }

    /// Why the field changed, for the native acceleration projection.
    pub fn change_origin(&self) -> GravityChangeOrigin {
        self.change_origin
    }

    /// Magnitude in m/s².
    pub fn magnitude(&self) -> f64 {
        self.acceleration.length()
    }

    /// Downward unit vector; a zero field has the conventional -Y direction.
    pub fn direction(&self) -> DVec3 {
        if self.acceleration.length_squared() > 0.0 {
            self.acceleration.normalize()
        } else {
            DVec3::NEG_Y
        }
    }
}

/// A finite-step measurement, not an instantaneous impact peak.
///
/// `linear_world` is Δv/Δt. Specific force is `(Δv/Δt) - gravity`, so a parked
/// accelerometer reads the support load and an ideal free-falling one reads
/// zero. Its magnitude is independent of attitude. The body vector uses the
/// completed attitude. A sensor away from the centre of mass additionally
/// needs its lever-arm terms; that is a mounted-sensor model's responsibility.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component, Default)]
pub struct SolvedBodyAcceleration {
    pub linear_world: DVec3,
    pub angular_world: DVec3,
    pub specific_force_world: Option<DVec3>,
    pub specific_force_body: Option<DVec3>,
    pub interval_s: f64,
    pub tick: Option<u64>,
    /// False before the first complete, finite native solver interval.
    pub valid: bool,
    #[reflect(ignore)]
    start: Option<IntervalStart>,
    #[reflect(ignore)]
    boundary: Option<VelocityBoundary>,
}

#[derive(Debug, Clone, Copy)]
struct IntervalStart {
    linear: DVec3,
    angular: DVec3,
    gravity: Option<DVec3>,
    frame: Option<Entity>,
    end_time: Duration,
    dt_s: f64,
}

/// Completed physics measurements precede telemetry retention and other readers.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BodyAccelerationSampleSet;

#[derive(Debug, Clone, Copy)]
struct VelocityBoundary {
    linear: DVec3,
    angular: DVec3,
    time: Duration,
    frame: Option<Entity>,
}

/// Installed by the native physics owner, independently of UI and co-simulation.
pub(crate) fn install(app: &mut App) {
    app.register_type::<LocalGravity>()
        .register_type::<SolvedBodyAcceleration>()
        .register_required_components::<RigidBody, SolvedBodyAcceleration>()
        .add_observer(retire_body_sample)
        .add_observer(invalidate_unready_sample)
        .add_observer(invalidate_disabled_sample)
        .add_systems(
            PhysicsSchedule,
            begin_interval
                .after(PhysicsStepSystems::First)
                .before(PhysicsStepSystems::BroadPhase),
        )
        .add_systems(
            FixedPostUpdate,
            finish_interval
                .after(PhysicsSystems::Writeback)
                .before(PhysicsSystems::Last)
                .in_set(BodyAccelerationSampleSet),
        );
}

fn retire_body_sample(event: On<Remove<RigidBody>>, mut commands: Commands) {
    commands
        .entity(event.entity)
        .try_remove::<SolvedBodyAcceleration>();
}

fn invalidate_unready_sample(
    event: On<Remove<PhysicsStateReady>>,
    mut samples: Query<&mut SolvedBodyAcceleration>,
) {
    if let Ok(mut sample) = samples.get_mut(event.entity) {
        *sample = SolvedBodyAcceleration::default();
    }
}

fn invalidate_disabled_sample(
    event: On<Add<RigidBodyDisabled>>,
    mut samples: Query<&mut SolvedBodyAcceleration>,
) {
    if let Ok(mut sample) = samples.get_mut(event.entity) {
        *sample = SolvedBodyAcceleration::default();
    }
}

fn begin_interval(
    gravity: Option<Res<Gravity>>,
    time: Res<Time<Physics>>,
    frame: Option<Res<ActivePhysicsFrame>>,
    mut bodies: Query<
        (
            &LinearVelocity,
            &AngularVelocity,
            Option<&LocalGravity>,
            Option<&GravityScale>,
            Has<RigidBodyDisabled>,
            &mut SolvedBodyAcceleration,
        ),
        With<RigidBody>,
    >,
) {
    for (linear, angular, local, scale, disabled, mut sample) in &mut bodies {
        if disabled || !linear.0.is_finite() || !angular.0.is_finite() {
            *sample = SolvedBodyAcceleration::default();
            continue;
        }
        let frame = frame.as_ref().map(|f| f.0);
        if sample
            .boundary
            .is_some_and(|b| b.time == time.elapsed() && b.frame == frame)
        {
            sample.start = None;
            continue;
        }
        sample.start = {
            // Avian applies GravityScale only to its global gravity. The
            // environment's local field is a separate constant acceleration.
            let field = gravity
                .as_ref()
                .map(|global| {
                    global.0.as_dvec3() * f64::from(scale.map_or(1.0, |s| s.0))
                        + local.map_or(DVec3::ZERO, |g| g.acceleration())
                })
                .filter(|g| g.is_finite());
            // Include impulses between completed steps. New bodies, clock
            // discontinuities and frame migrations seed a local baseline.
            let previous = sample.boundary.filter(|b| {
                b.frame == frame && time.elapsed().checked_sub(time.delta()) == Some(b.time)
            });
            Some(IntervalStart {
                linear: previous.map_or(linear.0.as_dvec3(), |b| b.linear),
                angular: previous.map_or(angular.0.as_dvec3(), |b| b.angular),
                gravity: field,
                frame,
                end_time: time.elapsed(),
                dt_s: time.delta_secs_f64(),
            })
        };
    }
}

fn finish_interval(
    tick: Option<Res<SimTick>>,
    faults: Option<Res<lunco_core::RuntimeFaults>>,
    frame: Option<Res<ActivePhysicsFrame>>,
    mut bodies: Query<
        (
            &LinearVelocity,
            &AngularVelocity,
            &Rotation,
            &mut SolvedBodyAcceleration,
        ),
        (With<RigidBody>, Without<RigidBodyDisabled>),
    >,
) {
    for (linear, angular, rotation, mut sample) in &mut bodies {
        let Some(start) = sample.start.take() else {
            continue;
        };
        sample.valid = false;
        sample.specific_force_world = None;
        sample.specific_force_body = None;
        sample.boundary = None;
        let dt = start.dt_s;
        if !dt.is_finite()
            || dt <= 0.0
            || faults.as_ref().is_some_and(|f| f.active())
            || start.frame != frame.as_ref().map(|f| f.0)
        {
            continue;
        }
        let a = (linear.0.as_dvec3() - start.linear) / dt;
        let alpha = (angular.0.as_dvec3() - start.angular) / dt;
        if !a.is_finite() || !alpha.is_finite() {
            continue;
        }
        sample.linear_world = a;
        sample.angular_world = alpha;
        sample.interval_s = dt;
        sample.tick = tick.as_ref().map(|t| t.0);
        sample.valid = true;
        sample.boundary = Some(VelocityBoundary {
            linear: linear.0.as_dvec3(),
            angular: angular.0.as_dvec3(),
            time: start.end_time,
            frame: start.frame,
        });
        sample.specific_force_world = start.gravity.map(|g| a - g).filter(|f| f.is_finite());
        sample.specific_force_body = sample
            .specific_force_world
            .filter(|_| rotation.0.is_finite() && rotation.0.is_normalized())
            // Native unit tolerance is f32; the widened frame rotation must be unit in f64.
            .map(|force| rotation.0.as_dquat().normalize().inverse() * force);
    }
}
