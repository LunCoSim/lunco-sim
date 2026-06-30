//! USD-authored sensors exposed as cosim telemetry ports.
//!
//! Telemetry used to be limited to a body's own kinematic state (the
//! [`crate::avian::RIGID_BODY_GROUP`]) and joint DOFs. A real vehicle also
//! carries *sensors* — an IMU, a range finder, a contact switch — whose outputs
//! a flight-software/controller reads. This module adds three USD-authorable
//! sensor kinds, each a component with cached output fields updated by a small
//! system and surfaced through the same [`AvianGroup`] port mechanism as the
//! rigid body. Gated on the marker component, so a body without the sensor pays
//! nothing.
//!
//! | USD attr (`lunco-usd-sim`) | Component        | Ports                                   |
//! |----------------------------|------------------|-----------------------------------------|
//! | `lunco:sensor:imu`         | [`ImuSensor`]    | `accel_x/y/z`, `spec_force_x/y/z`       |
//! | `lunco:sensor:range`       | [`RangeSensor`]  | `range`                                 |
//! | `lunco:sensor:contact`     | [`ContactSensor`]| `contact`, `contact_force`              |
//!
//! All three honor a **body-local mounting offset** (`lunco:sensor:offset`) so a
//! sensor that isn't at the body origin reports from its true mount point.

use avian3d::prelude::{
    AngularVelocity, Collisions, LinearVelocity, Physics, Rotation, SpatialQuery,
    SpatialQueryFilter, SubstepCount,
};
use bevy::math::{DVec3, Dir3};
use bevy::prelude::*;

use crate::connection::{PortDirection, PortType};
use crate::ports::{AvianGroup, AvianPort};

/// Inertial measurement unit. Reports both world-frame linear acceleration
/// (`accel_*`, coordinate acceleration) and body-frame **specific force**
/// (`spec_force_*`, what a real accelerometer measures: `a − g` rotated into the
/// body) at the sensor's mount point — including the rigid-body lever-arm terms
/// `α × r + ω × (ω × r)` when mounted off the centre of mass. Pairs with the
/// rigid-body group's `angvel_*` + `quat_*` for a full 9-DOF IMU.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component, Default)]
pub struct ImuSensor {
    /// Mount point in the body's local frame (lever arm from the COM).
    pub offset: DVec3,
    /// Local gravity vector (m/s², world frame) — the authoritative value
    /// **fed by `lunco-environment`** from its per-body `LocalGravity` (avian's
    /// own `Gravity` resource is zero here; gravity is applied as an explicit
    /// force). Needed for the body-frame specific force `a − g`. Stays zero if
    /// no gravity provider runs, degrading `spec_force` gracefully to `accel`.
    pub gravity: DVec3,
    /// World-frame linear acceleration at the mount point (m/s²).
    pub accel: DVec3,
    /// Body-frame specific force at the mount point (m/s²) — accelerometer output.
    pub spec_force: DVec3,
    /// Previous tick's linear velocity, for the finite difference.
    prev_vel: DVec3,
    /// Previous tick's angular velocity, for the angular-acceleration term.
    prev_angvel: DVec3,
    /// False until the first tick has stored samples (no accel on frame 0).
    primed: bool,
}

impl ImuSensor {
    /// An IMU mounted at `offset` (body-local lever arm), with zeroed state.
    pub fn mounted(offset: DVec3) -> Self {
        Self {
            offset,
            ..Default::default()
        }
    }
}

/// Behavior when the range sensor does not hit any geometry within its maximum distance.
#[derive(Reflect, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutOfRangeMode {
    /// Report the maximum distance (realistic).
    #[default]
    MaxDistance,
    /// Report -1.0.
    NegativeOne,
    /// Report NaN.
    NaN,
    /// Report the ideal altitude (sensor world Y).
    IdealAltitude,
}

/// Range finder / lidar ray. Casts from the mount point along [`axis`](Self::axis)
/// (body-local, rotated by attitude) and reports distance-to-geometry, or
/// a configured fallback when nothing is hit. Default points down (`-Y`) — an altimeter.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct RangeSensor {
    /// Mount point in the body's local frame (ray origin offset from the COM).
    pub offset: DVec3,
    /// Cast direction in the body's local frame.
    pub axis: DVec3,
    /// Maximum range (m); reported when the ray hits nothing.
    pub max_distance: f64,
    /// Last measured distance (m), updated by [`update_range_sensors`].
    pub distance: f64,
    /// Behavior when the sensor range is exceeded.
    pub out_of_range_mode: OutOfRangeMode,
    /// Whether to draw the laser beam line using Bevy gizmos.
    pub visualize: bool,
}

impl Default for RangeSensor {
    fn default() -> Self {
        Self {
            offset: DVec3::ZERO,
            axis: DVec3::NEG_Y,
            max_distance: 100.0,
            distance: 100.0,
            out_of_range_mode: OutOfRangeMode::MaxDistance,
            visualize: false,
        }
    }
}

/// Contact switch / force sensor. Reports whether the body is touching anything
/// (`contact`, 0/1) and the total contact normal force (`contact_force`, N).
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component, Default)]
pub struct ContactSensor {
    /// Whether the body is in contact this tick.
    pub in_contact: bool,
    /// Total contact normal force (N), updated by [`update_contact_sensors`].
    pub normal_force: f64,
}

/// IMU port group — world-frame acceleration + body-frame specific force, gated
/// on [`ImuSensor`].
pub const IMU_SENSOR_GROUP: AvianGroup = AvianGroup {
    present: |w, e| w.get::<ImuSensor>(e).is_some(),
    ports: &[
        AvianPort {
            name: "accel_x",
            dir: PortDirection::Out,
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<ImuSensor>(e).map(|s| s.accel.x)),
            write: None,
        },
        AvianPort {
            name: "accel_y",
            dir: PortDirection::Out,
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<ImuSensor>(e).map(|s| s.accel.y)),
            write: None,
        },
        AvianPort {
            name: "accel_z",
            dir: PortDirection::Out,
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<ImuSensor>(e).map(|s| s.accel.z)),
            write: None,
        },
        AvianPort {
            name: "spec_force_x",
            dir: PortDirection::Out,
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<ImuSensor>(e).map(|s| s.spec_force.x)),
            write: None,
        },
        AvianPort {
            name: "spec_force_y",
            dir: PortDirection::Out,
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<ImuSensor>(e).map(|s| s.spec_force.y)),
            write: None,
        },
        AvianPort {
            name: "spec_force_z",
            dir: PortDirection::Out,
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<ImuSensor>(e).map(|s| s.spec_force.z)),
            write: None,
        },
    ],
};

/// Range-sensor port group — `range`, gated on [`RangeSensor`].
pub const RANGE_SENSOR_GROUP: AvianGroup = AvianGroup {
    present: |w, e| w.get::<RangeSensor>(e).is_some(),
    ports: &[AvianPort {
        name: "range",
        dir: PortDirection::Out,
        port_type: PortType::Kinematic,
        read: Some(|w, e| w.get::<RangeSensor>(e).map(|s| s.distance)),
        write: None,
    }],
};

/// Contact-sensor port group — `contact` + `contact_force`, gated on
/// [`ContactSensor`].
pub const CONTACT_SENSOR_GROUP: AvianGroup = AvianGroup {
    present: |w, e| w.get::<ContactSensor>(e).is_some(),
    ports: &[
        AvianPort {
            name: "contact",
            dir: PortDirection::Out,
            port_type: PortType::Signal,
            read: Some(|w, e| {
                w.get::<ContactSensor>(e)
                    .map(|s| if s.in_contact { 1.0 } else { 0.0 })
            }),
            write: None,
        },
        AvianPort {
            name: "contact_force",
            dir: PortDirection::Out,
            port_type: PortType::Force,
            read: Some(|w, e| w.get::<ContactSensor>(e).map(|s| s.normal_force)),
            write: None,
        },
    ],
};

/// Update each IMU: world-frame acceleration at the mount point (rigid-body
/// transport: `a_com + α×r + ω×(ω×r)`) and the body-frame specific force
/// (`a − g` rotated into the body). No output on the first tick (no prior
/// sample to difference).
pub fn update_imu_sensors(
    time: Res<Time<Fixed>>,
    mut q: Query<(&mut ImuSensor, &LinearVelocity, &AngularVelocity, &Rotation)>,
) {
    let dt = time.delta_secs_f64();
    if dt <= 0.0 {
        return;
    }
    for (mut imu, v, w, rot) in &mut q {
        let omega = w.0;
        if imu.primed {
            let a_com = (v.0 - imu.prev_vel) / dt;
            let alpha = (omega - imu.prev_angvel) / dt;
            let r = rot.0 * imu.offset;
            let a_point = a_com + alpha.cross(r) + omega.cross(omega.cross(r));
            imu.accel = a_point;
            // Accelerometer measures specific force a − g (g = the local gravity
            // fed by lunco-environment), expressed in the body frame: a static
            // sensor reads +1 g along its local "up".
            imu.spec_force = rot.0.inverse() * (a_point - imu.gravity);
        }
        imu.prev_vel = v.0;
        imu.prev_angvel = omega;
        imu.primed = true;
    }
}

/// Cast each range sensor's ray from its mount point and record the hit distance
/// or the configured out-of-range fallback. The sensor's own entity and its parent
/// are excluded so it never ranges itself or the vehicle it is mounted to.
pub fn update_range_sensors(
    spatial: SpatialQuery,
    q_parents: Query<&ChildOf>,
    mut q: Query<(Entity, &mut RangeSensor, &GlobalTransform)>,
    mut gizmos: Gizmos,
) {
    for (e, mut s, transform) in &mut q {
        let origin = transform.translation().as_dvec3() + transform.rotation().as_dquat() * s.offset;
        let dir_world = transform.rotation().as_dquat() * s.axis;
        let Ok(dir) = Dir3::new(dir_world.as_vec3()) else {
            continue;
        };
        let mut filter = SpatialQueryFilter::from_mask(
            avian3d::prelude::LayerMask(!lunco_core::TRIGGER_COLLISION_LAYER),
        );
        filter.excluded_entities.insert(e);
        if let Ok(parent) = q_parents.get(e) {
            filter.excluded_entities.insert(parent.0);
        }
        let mut hit_dist = s.max_distance;
        let hit_something = match spatial.cast_ray(origin, dir, s.max_distance, true, &filter) {
            Some(hit) => {
                hit_dist = hit.distance;
                s.distance = hit.distance;
                true
            }
            None => {
                s.distance = match s.out_of_range_mode {
                    OutOfRangeMode::MaxDistance => s.max_distance,
                    OutOfRangeMode::NegativeOne => -1.0,
                    OutOfRangeMode::NaN => f64::NAN,
                    OutOfRangeMode::IdealAltitude => origin.y,
                };
                false
            }
        };

        if s.visualize {
            let end = origin + dir_world * hit_dist;
            let color = if hit_something {
                Color::srgb(1.0, 0.1, 0.1) // Bright red when hit-locked
            } else {
                Color::srgba(1.0, 0.1, 0.1, 0.4) // Faint translucent red when out of range
            };
            gizmos.line(origin.as_vec3(), end.as_vec3(), color);
            if hit_something {
                gizmos.sphere(end.as_vec3(), 0.15, color);
            }
        }
    }
}

/// Contact normal force from the **converged per-substep contact impulse**.
///
/// Subtlety (why not `total_normal_impulse / dt`): avian's
/// `ContactPoint::normal_impulse` is a solver *accumulator* — its own doc says it
/// sums "across substeps **and restitution**", i.e. over both the biased
/// `solve_contacts::<true>` and the unbiased relax `solve_contacts::<false>`
/// passes (plus restitution). Dividing it by `dt` does **not** give Newtons (it
/// over-reads ≈2× at rest, more with restitution). Instead we read
/// `ContactPoint::warm_start_normal_impulse` — *"the clamped accumulated impulse
/// from the last substep"* — the actual physical impulse delivered in one
/// substep, and divide by the substep duration `dt / SubstepCount`. This is
/// solver-config-robust (independent of substep count and restitution) rather
/// than a tuned divisor.
pub fn update_contact_sensors(
    time: Res<Time<Physics>>,
    substeps: Res<SubstepCount>,
    collisions: Collisions,
    mut q: Query<(Entity, &mut ContactSensor)>,
) {
    let dt = time.delta_secs_f64().max(1e-9);
    let substep_dt = dt / (substeps.0.max(1) as f64);
    for (e, mut s) in &mut q {
        let mut warm_impulse = 0.0;
        let mut touching = false;
        for pair in collisions.collisions_with(e) {
            for manifold in &pair.manifolds {
                for point in &manifold.points {
                    warm_impulse += point.warm_start_normal_impulse;
                }
            }
            touching = true;
        }
        s.in_contact = touching;
        // Per-substep impulse / substep duration = the physical normal force.
        s.normal_force = warm_impulse / substep_dt;
    }
}
