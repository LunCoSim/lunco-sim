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
//! | USD attr (`lunco-usd-sim`) | Component       | Ports                         |
//! |----------------------------|-----------------|-------------------------------|
//! | `lunco:sensor:imu`         | [`ImuSensor`]   | `accel_x/y/z`                 |
//! | `lunco:sensor:range`       | [`RangeSensor`] | `range`                       |
//! | `lunco:sensor:contact`     | [`ContactSensor`]| `contact`, `contact_force`   |
//!
//! The IMU's accel pairs with the rigid-body group's existing `angvel_*` +
//! `quat_*` to form a full 9-DOF IMU. (Accel is world-frame coordinate
//! acceleration via finite difference; body-frame specific force — subtracting
//! gravity, rotating into the body — is a future refinement.)

use avian3d::prelude::{
    Collisions, LinearVelocity, Position, Rotation, SpatialQuery, SpatialQueryFilter,
};
use bevy::math::{DVec3, Dir3};
use bevy::prelude::*;

use crate::connection::{PortDirection, PortType};
use crate::ports::{AvianGroup, AvianPort};

/// Inertial measurement unit. Exposes world-frame linear acceleration
/// (`accel_x/y/z`), finite-differenced from [`LinearVelocity`] each fixed tick.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component, Default)]
pub struct ImuSensor {
    /// World-frame linear acceleration (m/s²), updated by [`update_imu_sensors`].
    pub accel: DVec3,
    /// Previous tick's velocity, for the finite difference.
    prev_vel: DVec3,
    /// False until the first tick has stored a velocity (no accel on frame 0).
    primed: bool,
}

/// Range finder / lidar ray. Casts along [`axis`](Self::axis) (body-local,
/// rotated by attitude) and reports distance-to-geometry, or `max_distance` when
/// nothing is hit. Default points down (`-Y`) — an altimeter.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct RangeSensor {
    /// Cast direction in the body's local frame.
    pub axis: DVec3,
    /// Maximum range (m); reported when the ray hits nothing.
    pub max_distance: f64,
    /// Last measured distance (m), updated by [`update_range_sensors`].
    pub distance: f64,
}

impl Default for RangeSensor {
    fn default() -> Self {
        Self {
            axis: DVec3::NEG_Y,
            max_distance: 100.0,
            distance: 100.0,
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

/// IMU port group — world-frame linear acceleration, gated on [`ImuSensor`].
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

/// Finite-difference [`LinearVelocity`] into world-frame acceleration each fixed
/// tick. No acceleration is reported on the first tick (no prior sample).
pub fn update_imu_sensors(time: Res<Time<Fixed>>, mut q: Query<(&mut ImuSensor, &LinearVelocity)>) {
    let dt = time.delta_secs_f64();
    if dt <= 0.0 {
        return;
    }
    for (mut imu, v) in &mut q {
        if imu.primed {
            imu.accel = (v.0 - imu.prev_vel) / dt;
        }
        imu.prev_vel = v.0;
        imu.primed = true;
    }
}

/// Cast each range sensor's ray and record the hit distance (or `max_distance`).
/// The sensor's own body is excluded so it never ranges itself.
pub fn update_range_sensors(
    spatial: SpatialQuery,
    mut q: Query<(Entity, &mut RangeSensor, &Position, &Rotation)>,
) {
    for (e, mut s, pos, rot) in &mut q {
        let dir_world = rot.0 * s.axis;
        let Ok(dir) = Dir3::new(dir_world.as_vec3()) else {
            continue;
        };
        let filter = SpatialQueryFilter::from_excluded_entities([e]);
        s.distance = match spatial.cast_ray(pos.0, dir, s.max_distance, true, &filter) {
            Some(hit) => hit.distance,
            None => s.max_distance,
        };
    }
}

/// Sum contact normal impulses on each contact sensor and convert to force
/// (impulse / dt). `in_contact` is set whenever any contact pair touches.
pub fn update_contact_sensors(
    time: Res<Time<Fixed>>,
    collisions: Collisions,
    mut q: Query<(Entity, &mut ContactSensor)>,
) {
    let dt = time.delta_secs_f64().max(1e-9);
    for (e, mut s) in &mut q {
        let mut impulse = 0.0;
        let mut touching = false;
        for pair in collisions.collisions_with(e) {
            touching = true;
            impulse += pair.total_normal_impulse_magnitude();
        }
        s.in_contact = touching;
        s.normal_force = impulse / dt;
    }
}
