//! # Tire Spin Integrator
//!
//! Raycast wheels are kinematic proxies — the chassis dynamics live on the
//! parent rigid body, so the wheel mesh carries no angular velocity of its own.
//! This module gives each tire a real rotational state so the spin you *see*
//! matches the physics the rover is actually experiencing.

use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::*;
use lunco_fsw::FlightSoftware;

use crate::wheel_kinematics::{wheel_hub_pose, wheel_hub_velocity};
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
        &Rotation,
        Option<&FlightSoftware>,
        &RigidBody,
        // Client proxies are Kinematic with avian velocity zeroed; their real
        // ground speed arrives via this delivered hint (set by `interpolate_proxies`).
        Option<&lunco_core::ReplicatedChassisMotion>,
    ), With<FlightSoftware>>,
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
        if let Ok((lin, ang, pos, rot, fsw, body, motion)) = q_chassis.get(parent.parent()) {
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
            // Reconstruct the hub in the AVIAN cell-local frame from the chassis
            // body pose + the wheel's chassis-local transform (the wheel is a
            // `ChildOf` the chassis, so `local_tf` *is* that transform). Reading
            // `global_tf.translation()` here mixed the big_space render frame into
            // the lever arm `hub − pos.0` and drifted the slip term once the rover
            // drove away from the floating origin (CQ-201). Rotation is frame-safe
            // (big_space only translates), so `forward` can keep using `global_tf`.
            let (hub_pos, _) = wheel_hub_pose(
                pos.0,
                rot.0,
                local_tf.translation.as_dvec3(),
                local_tf.rotation.as_dquat(),
            );
            let hub_vel = wheel_hub_velocity(vlin, vang, hub_pos, pos.0);
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

#[cfg(test)]
mod tests {
    use super::update_wheel_spin;
    use crate::{WheelRaycast, Suspension};
    use avian3d::prelude::*;
    use bevy::math::DVec3;
    use bevy::prelude::*;
    use bevy::time::Time;
    use lunco_fsw::FlightSoftware;
    use std::time::Duration;

    /// Drive `update_wheel_spin` one tick on a single grounded raycast wheel and
    /// return the resulting axle `spin_velocity`.
    ///
    /// The chassis is a Dynamic body at avian `Position`/`Rotation` = origin/identity
    /// with angular velocity `ang`. The wheel is a `ChildOf` the chassis with
    /// chassis-local transform `wheel_local`; its **`GlobalTransform.translation`** is
    /// `wheel_gtf_translation` — the value big_space rebases away from the floating
    /// origin. Pre-fix the integrator built the contact-slip lever as
    /// `wheel_gtf − chassis_pos` (render-frame minus avian-frame), so the spin depended
    /// on `wheel_gtf_translation`; post-fix it reconstructs the hub from the chassis
    /// pose (`pos + rot·wheel_local`, pure avian), so spin is invariant to it.
    fn run_raycast_spin(ang: DVec3, wheel_local: Vec3, wheel_gtf_translation: Vec3) -> f64 {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f64(0.1));
        app.insert_resource(time);

        // Port with no `PhysicalPort` → throttle reads 0 (free-rolling, so the spin
        // is driven purely by ground speed / the lever arm under test).
        let port = app.world_mut().spawn_empty().id();
        let chassis = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Position(DVec3::ZERO),
                Rotation::default(),
                LinearVelocity(DVec3::ZERO),
                AngularVelocity(ang),
                FlightSoftware::default(),
            ))
            .id();
        let visual = app.world_mut().spawn(Transform::default()).id();
         app.world_mut().spawn((
            WheelRaycast {
                suspension_port: port,
                drive_port: port,
                steer_port: port,
                steer_axis: DVec3::Y,
                wheel_radius: 0.5,
                ray_origin_y: 0.0,
                visual_entity: Some(visual),
                last_normal_force: 100.0, // ≥1 ⇒ on_ground (with a hit present)
                spin_angle: 0.0,
                spin_velocity: 0.0,
                mass: 8.0,
                moment_of_inertia: 1.0, // overrides ½mr² ⇒ inertia = 1.0 (clean)
                drive_torque_max: 0.0,
                max_rotation_speed: 12.0,
                bearing_damping: 0.0,
                friction_mu: 1.0,
                slip_stiffness: 1000.0,
                contact_grip_stiffness: 1000.0,
                brake_torque_max: 0.0,
                drive_force_per_normal: 2.0,
            },
            Suspension {
                rest_length: 1.0,
                spring_k: 1000.0,
                damping_c: 100.0,
                local_axis: DVec3::Y,
            },
            Transform::from_translation(wheel_local),
            GlobalTransform::from(Transform::from_translation(wheel_gtf_translation)),
            // One hit ⇒ the wheel is on the ground (the integrator only checks
            // presence, not distance/normal, for the grip path).
            RayHits(vec![RayHitData { entity: chassis, distance: 0.5, normal: DVec3::Y }]),
            ChildOf(chassis),
        ));

        app.add_systems(Update, update_wheel_spin);
        app.update();

        app.world_mut()
            .query::<&WheelRaycast>()
            .iter(app.world())
            .next()
            .unwrap()
            .spin_velocity
    }

    #[test]
    fn raycast_spin_is_floating_origin_invariant() {
        // CQ-201 regression for the authoritative (raycast) rover. Chassis yaws
        // about +Y at 1 rad/s; the hub sits 1 m out along +X, so the lever arm
        // feeds the contact slip and thus the gripped axle rate. The ONLY change
        // between runs is the wheel's GlobalTransform translation: "near origin"
        // (true world hub pos) vs "≈1 km away" along the sensitive axis (a big_space
        // rebase). A frame-correct integrator gives the SAME spin; the old
        // `gtf − pos.0` lever gave a wildly different one (the bug, invisible near
        // origin).
        let ang = DVec3::Y;
        let mount = Vec3::new(1.0, 0.0, 0.0);

        let near = run_raycast_spin(ang, mount, mount);
        let far = run_raycast_spin(ang, mount, mount - Vec3::new(1000.0, 0.0, 0.0));

        assert!(
            (near - far).abs() < 1e-6,
            "raycast spin must be floating-origin invariant: near={near} far={far} (Δ={})",
            (near - far).abs()
        );
        // And physically correct, not just self-consistent. v_long = 1 m/s (as in
        // the proxy test); the implicit grip solve with inertia/dt=10, k_slip·r²=250
        // gives ω = (k_slip·r·v_long)/(inertia/dt + k_slip·r²) = 500/260 ≈ 1.923,
        // and |f_slip|≈38 < μN=100 so the tire grips (no saturation).
        assert!((near - 1.9231).abs() < 1e-2, "expected gripped ω≈1.923, got {near}");
    }
}
