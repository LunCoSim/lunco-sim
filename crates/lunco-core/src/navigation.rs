//! Shared rover navigation command law.
//!
//! The command surface is intentionally vehicle-neutral (`throttle`, `steer`,
//! `brake`), while the steering capability is authored by each vehicle. Every
//! driver—native behaviour trees and Rhai helpers—must use this law so the
//! signed steering convention, heading recovery, arrival braking, and
//! invalid-pose handling cannot diverge.

use bevy::math::{DVec3, Vec3};

use crate::{coords::GridPos, SteeringGeometry};

/// One validated command produced by [`nav_setpoint`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavigationCommand {
    pub throttle: f64,
    pub steer: f64,
    pub brake: f64,
    pub arrived: bool,
}

impl NavigationCommand {
    pub const fn brake() -> Self {
        Self {
            throttle: 0.0,
            steer: 0.0,
            brake: 1.0,
            arrived: false,
        }
    }

    pub const fn into_tuple(self) -> (f64, f64, f64) {
        (self.throttle, self.steer, self.brake)
    }
}

/// Compute a vehicle command toward `target` in the active grid frame.
///
/// `None` means the request cannot be evaluated: the radius/speed is invalid,
/// the target has no horizontal direction, or the vehicle has no horizontal
/// heading. Callers must fail closed by holding the brake; they must not invent
/// a vehicle class, heading, or throttle value.
pub fn nav_setpoint(
    pos: GridPos,
    fwd: Vec3,
    target: GridPos,
    speed: f64,
    radius: f32,
    steering_geometry: SteeringGeometry,
) -> Option<NavigationCommand> {
    if !speed.is_finite() || speed < 0.0 || !radius.is_finite() || radius <= 0.0 {
        return None;
    }

    let offset = target - pos;
    let to_xz = DVec3::new(offset.x, 0.0, offset.z);
    let to_len = to_xz.length();
    let fwd_xz = DVec3::new(fwd.x as f64, 0.0, fwd.z as f64);
    let fwd_len = fwd_xz.length();
    if !to_len.is_finite() || !fwd_len.is_finite() || !offset.y.is_finite() {
        return None;
    }

    // Surface-vehicle guidance is a yaw-plane contract.  The authored target
    // may be at terrain height while the authoritative body pose is at its
    // support/contact height, so using `offset.length()` here would keep a
    // rover outside its horizontal arrival radius forever and make it drive
    // through the target before the state machine can advance the leg.
    if to_len < radius as f64 {
        return Some(NavigationCommand {
            throttle: 0.0,
            steer: 0.0,
            brake: 1.0,
            arrived: true,
        });
    }
    if to_len <= f64::EPSILON || fwd_len <= f64::EPSILON {
        return None;
    }

    let to = (to_xz / to_len).as_vec3();
    let fwd = (fwd_xz / fwd_len).as_vec3();
    let cross_yaw = fwd.z * to.x - fwd.x * to.z;
    let dot = fwd.dot(to);

    // A waypoint behind the vehicle is a heading-recovery command, not a
    // request to drive in reverse. Turning in place keeps every rover's
    // longitudinal motion forward and makes the next waypoint the vehicle's
    // actual heading target. The sign tie-breaker is deterministic for the
    // exact 180-degree case, where the cross product has no direction.
    if dot < 0.0 {
        return Some(NavigationCommand {
            throttle: 0.0,
            steer: if cross_yaw >= 0.0 { -1.0 } else { 1.0 },
            brake: 0.0,
            arrived: false,
        });
    }

    let alignment = dot as f64;
    let throttle =
        speed * (0.25 + 0.75 * alignment).clamp(0.25, 1.0) * approach_factor(to_len, radius);
    Some(NavigationCommand {
        throttle,
        steer: steering_command(cross_yaw, to_len, steering_geometry),
        brake: 0.0,
        arrived: false,
    })
}

/// Convert yaw-plane error into the one normalized command convention shared by
/// every authored drive law. Both authored drive laws expose the same public
/// convention (`+steer` turns right); Ackermann's internal heading equation
/// performs its own conversion to the signed knuckle angle. Geometry only
/// affects the physical steering authority, while command sign, forward travel,
/// and arrival handling stay in this shared law.
pub fn steering_command(cross_yaw: f32, distance: f64, steering_geometry: SteeringGeometry) -> f64 {
    // Differential steering has direct yaw authority. Ackermann steering is a
    // finite-radius vehicle, so scale the normalized curvature by horizontal
    // target distance: distant targets request a broad arc and close targets
    // request the authored maximum lock. This is the same pure-pursuit law for
    // every Ackermann vehicle, not a route-specific speed or pose adjustment.
    let gain = match steering_geometry {
        SteeringGeometry::Differential => 2.5,
        SteeringGeometry::Ackermann => (4.0 / distance.max(1.0)).clamp(0.5, 2.5),
    };
    // `cross_yaw` is positive for a target to the vehicle's left. The public
    // drive surface is positive-to-the-right for every authored rover, so the
    // one shared command sign is negative here.
    (cross_yaw as f64 * gain * -1.0).clamp(-1.0, 1.0)
}

/// Scale the command as it enters the authored acceptance radius.
pub fn approach_factor(distance: f64, radius: f32) -> f64 {
    ((distance - radius as f64) / (radius as f64 * 2.0)).clamp(0.15, 1.0)
}
