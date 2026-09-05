//! Shared rover navigation command law.
//!
//! The command surface is intentionally vehicle-neutral (`throttle`, `steer`,
//! `brake`), while the steering capability is authored by each vehicle. Every
//! driver—native behaviour trees and Rhai helpers—must use this law so the
//! signed steering convention, shared travel hysteresis, arrival braking,
//! and invalid-pose handling cannot diverge.

use bevy::math::{DVec3, Vec3};

use crate::{SteeringGeometry, coords::GridPos};

/// Stateful direction selected for one navigation leg.
///
/// The shared steering law needs this state because a goal can cross the
/// lateral plane while the vehicle is turning. Re-evaluating the travel sign
/// independently each tick creates a forward/reverse limit cycle for every
/// drivetrain realization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigationState {
    /// No active leg; the next valid command chooses its initial travel mode.
    Uninitialized,
    /// Forward travel until the goal is decisively behind the vehicle.
    Forward,
    /// Reverse travel until the goal is decisively in front again.
    Reverse,
}

impl Default for NavigationState {
    fn default() -> Self {
        Self::Uninitialized
    }
}

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
    state: &mut NavigationState,
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
        *state = NavigationState::Uninitialized;
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

    if *state == NavigationState::Uninitialized {
        *state = if dot < -0.25 {
            NavigationState::Reverse
        } else {
            NavigationState::Forward
        };
    }

    match state {
        NavigationState::Forward => {
            if dot < -0.35 {
                *state = NavigationState::Reverse;
            }
        }
        NavigationState::Reverse => {
            if dot > 0.25 {
                *state = NavigationState::Forward;
            }
        }
        NavigationState::Uninitialized => {
            unreachable!("navigation state is initialized before its mode is read")
        }
    }

    let travel_sign = match state {
        NavigationState::Reverse => -1.0,
        NavigationState::Forward => 1.0,
        NavigationState::Uninitialized => {
            unreachable!("navigation state is initialized before its mode is read")
        }
    };

    let alignment = dot.abs() as f64;
    let throttle = speed
        * travel_sign
        * (0.25 + 0.75 * alignment).clamp(0.25, 1.0)
        * approach_factor(to_len, radius);
    Some(NavigationCommand {
        throttle,
        steer: steering_command(cross_yaw, travel_sign, to_len, steering_geometry),
        brake: 0.0,
        arrived: false,
    })
}

/// Convert yaw-plane error into the one normalized command convention shared by
/// every authored drive law. Both authored drive laws expose the same public
/// convention (`+steer` turns right); Ackermann's internal heading equation
/// performs its own conversion to the signed knuckle angle. Geometry only
/// affects the physical steering authority, while command sign, travel
/// direction, and arrival handling stay in this shared law.
pub fn steering_command(
    cross_yaw: f32,
    travel_sign: f64,
    distance: f64,
    steering_geometry: SteeringGeometry,
) -> f64 {
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
    // one shared command sign is negative here. Reverse travel mirrors the
    // steering command so the same target-relative turn is maintained.
    (cross_yaw as f64 * gain * travel_sign * -1.0).clamp(-1.0, 1.0)
}

/// Scale the command as it enters the authored acceptance radius.
pub fn approach_factor(distance: f64, radius: f32) -> f64 {
    ((distance - radius as f64) / (radius as f64 * 2.0)).clamp(0.15, 1.0)
}
