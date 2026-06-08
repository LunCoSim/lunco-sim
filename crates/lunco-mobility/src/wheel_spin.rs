//! # Tire Spin Integrator
//!
//! Raycast wheels are kinematic proxies — the chassis dynamics live on the
//! parent rigid body, so the wheel mesh carries no angular velocity of its own.
//! This module gives each tire a real rotational state so the spin you *see*
//! matches the physics the rover is actually experiencing.

use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::*;
use lunco_core::RoverVessel;
use lunco_fsw::FlightSoftware;

use crate::WheelRaycast;

/// Torque that would exactly arrest a spin of `w` rad/s in one step `dt`
/// for a wheel of inertia `i` (`τ = I·ω/dt`). The brake applies the negative
/// of this, clamped to its peak, so it can lock the wheel without overshoot.
#[inline]
fn w_stop_torque(w: f64, i: f64, dt: f64) -> f64 {
    i * w / dt
}

/// Integrates realistic tire spin and drives the visual wheel rotation.
///
/// The spin tracks ground speed when rolling, breaks loose into wheelspin when
/// drive torque exceeds traction, locks into a skid under braking, and
/// free-spins from applied torque when the rover is airborne.
///
/// **Model** — per wheel we integrate the axle angular velocity `ω` from a torque
/// balance `I·ω̇ = τ_drive + τ_brake − τ_traction − τ_bearing`. Every coefficient
/// is read from the USD wheel component (mass, friction, motor curve) — see
/// `setup_raycast_wheel` — so the spin you see is grounded in the authored data:
/// - `I = ½·m·r²` — solid-disk inertia from USD `physics:mass` and radius.
/// - `τ_drive = throttle · drive_torque_max` — actuator torque (signed for
///   reverse); `drive_torque_max` comes from the motor power / no-load speed.
/// - **Grounded**: the contact slip `(ω·r − v)` is resisted by tire grip with a
///   stiff longitudinal stiffness, capped by the Coulomb limit `μ·N`. Below the
///   limit the wheel grips (`ω → v/r`, solved implicitly for unconditional
///   stability); above it the tire breaks loose and `ω` runs away from `v/r`
///   (visible wheelspin or lock-up skid). This is the standard slip-ratio model.
/// - **Airborne**: no contact → no traction; `ω` spins up under `τ_drive` and
///   bleeds off through bearing drag, terminating at the motor's no-load speed.
/// - **Braking**: brake torque opposes the spin and, when it beats the available
///   traction, locks the tire into a skid while the chassis keeps moving.
///
/// The integrated angle is composed with the steer yaw to drive the mesh:
/// `R = steer · rollₓ(−θ) · cylinder_base`.
pub(crate) fn update_wheel_spin(
    mut q_wheels: Query<(&mut WheelRaycast, &Transform, &GlobalTransform, &RayHits, &ChildOf)>,
    q_ports: Query<&lunco_core::architecture::PhysicalPort>,
    q_chassis: Query<(
        &LinearVelocity,
        &AngularVelocity,
        &Position,
        Option<&FlightSoftware>,
        &RigidBody,
        // Client proxies are Kinematic with avian velocity zeroed; their real
        // ground speed arrives via this delivered hint (set by `interpolate_proxies`).
        Option<&lunco_core::ReplicatedChassisMotion>,
    ), With<RoverVessel>>,
    mut q_visual: Query<&mut Transform, Without<WheelRaycast>>,
    time: Res<Time>,
) {
    use std::f64::consts::TAU;

    let dt = time.delta_secs_f64();
    if dt <= 0.0 {
        return;
    }

    for (mut wheel, local_tf, global_tf, hits, parent) in q_wheels.iter_mut() {
        // All dynamics coefficients are USD-derived (stored on the component).
        let r = wheel.wheel_radius.max(1e-3);
        let inertia = wheel.axle_inertia();
        let k_slip = wheel.slip_stiffness;
        let c_bearing = wheel.bearing_damping;
        let friction_mu = wheel.friction_mu;

        // Signed throttle: positive drives forward, negative reverses.
        let throttle = q_ports
            .get(wheel.drive_port)
            .map(|p| (p.value as f64).clamp(-1.0, 1.0))
            .unwrap_or(0.0);
        let tau_drive = throttle * wheel.drive_torque_max;

        // Longitudinal ground speed at the contact patch, projected onto the
        // wheel's forward axis. Pulled from the parent chassis rigid body.
        let mut v_long = 0.0;
        let mut braking = false;
        if let Ok((lin, ang, pos, fsw, body, motion)) = q_chassis.get(parent.parent()) {
            braking = fsw.map(|f| f.brake_active).unwrap_or(false);
            // Source the chassis velocity from wherever this peer's chassis
            // actually gets its motion: live avian velocity on a Dynamic body
            // (host / the owned rover), or the delivered snapshot hint on a
            // Kinematic proxy (whose avian velocity is force-zeroed). Without the
            // hint branch a replicated rover rolls visibly across the ground with
            // dead, non-spinning wheels.
            let (vlin, vang) = if matches!(body, RigidBody::Kinematic) {
                motion
                    .map(|m| (m.lin, m.ang))
                    .unwrap_or((DVec3::ZERO, DVec3::ZERO))
            } else {
                (lin.0, ang.0)
            };
            let hub_world = global_tf.translation().as_dvec3();
            let hub_vel = vlin + vang.cross(hub_world - pos.0);
            let forward = global_tf.rotation().mul_vec3(Vec3::NEG_Z).as_dvec3();
            v_long = hub_vel.dot(forward);
        }

        // Brake torque opposes the current spin, clamped to the authored peak.
        // Using the spin-stopping torque as the target lets a strong brake lock
        // the wheel (ω→0) without overshooting past zero and chattering.
        let tau_brake = if braking {
            (-w_stop_torque(wheel.spin_velocity, inertia, dt))
                .clamp(-wheel.brake_torque_max, wheel.brake_torque_max)
        } else {
            0.0
        };

        let on_ground = wheel.last_normal_force >= 1.0 && hits.iter().next().is_some();
        let mut w = wheel.spin_velocity;

        if on_ground {
            let mu_n = friction_mu * wheel.last_normal_force;
            // Implicit grip solve assuming traction is unsaturated. Stiff term
            // k_slip is handled implicitly so ω snaps to ~v/r without exploding.
            let denom = inertia / dt + k_slip * r * r + c_bearing;
            let w_grip = (inertia / dt * w + tau_drive + tau_brake + k_slip * r * v_long) / denom;
            let f_slip = k_slip * (w_grip * r - v_long);

            if f_slip.abs() <= mu_n {
                // Tire grips: rolls at ground speed plus a tiny steady slip.
                w = w_grip;
            } else {
                // Traction broken: kinetic friction saturates at μ·N and opposes
                // the slip direction. Integrate explicitly — the stiff term is
                // gone, so ω runs away from v/r (wheelspin under drive, or a
                // locked skid when the brake torque wins).
                let slip_sign = (w * r - v_long).signum();
                let tau_traction = slip_sign * mu_n * r;
                w += dt * (tau_drive + tau_brake - tau_traction - c_bearing * w) / inertia;
            }
        } else {
            // Airborne: free spin under drive (and any brake) torque vs bearing drag.
            w += dt * (tau_drive + tau_brake - c_bearing * w) / inertia;
        }

        wheel.spin_velocity = w;
        wheel.spin_angle = (wheel.spin_angle + w * dt).rem_euclid(TAU);

        // Compose the visual mesh rotation from the canonical spin state: steer
        // yaw (from the wheel entity's local transform) · roll about the axle ·
        // cylinder-on-its-side base. Rebuilding from the wrapped absolute angle
        // every tick means no incremental quaternion drift and no jitter at the
        // 2π wrap — the same `spin_quat()` any other system would read.
        if let Some(visual_entity) = wheel.visual_entity {
            if let Ok(mut visual_tf) = q_visual.get_mut(visual_entity) {
                let steer = local_tf.rotation;
                let base = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
                visual_tf.rotation = (steer * wheel.spin_quat() * base).normalize();
            }
        }
    }
}
