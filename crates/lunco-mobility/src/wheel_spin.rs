//! # Tire Spin Integrator
//!
//! Raycast wheels are kinematic proxies — the chassis dynamics live on the
//! parent rigid body, so the wheel mesh carries no angular velocity of its own.
//! This module gives each tire a real rotational state so the spin you *see*
//! matches the physics the rover is actually experiencing.

use avian3d::prelude::*;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_core::coords::{GridPos, GridRot};
use lunco_core::InputPorts;

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
/// - `I = ½·m·r²` — solid-disk inertia from USD `physxVehicleWheel:mass` and radius.
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
    mut q_wheels: Query<(
        Entity,
        &mut WheelRaycast,
        &Transform,
        &GlobalTransform,
        &RayHits,
        &ChildOf,
    )>,
    q_ports: Query<&lunco_core::architecture::Port>,
    q_chassis: Query<
        (
            &LinearVelocity,
            &AngularVelocity,
            &Position,
            &Rotation,
            Option<&InputPorts>,
            &RigidBody,
            // Client proxies are Kinematic with avian velocity zeroed; their real
            // ground speed arrives via this delivered hint (set by `interpolate_proxies`).
            Option<&lunco_core::ReplicatedChassisMotion>,
        ),
        With<lunco_core::ActuatorPorts>,
    >,
    mut q_visual: Query<&mut Transform, Without<WheelRaycast>>,
    // The USD-projected motor target for each raycast wheel. The native readback
    // lives on the authored motor entity, not the wheel proxy, so every motor's
    // electrical, thermal, and Avian/raycast drivetrain values share one owner.
    q_targets: Query<&lunco_hardware::MotorReadbackTarget>,
    mut q_readback: Query<&mut lunco_hardware::MotorReadback>,
    q_child_of: Query<&ChildOf>,
    q_inputs: Query<&InputPorts>,
    // THE FIXED CLOCK BY TYPE, NOT BY PLACEMENT. This integrator is only correct
    // on a fixed step (the implicit grip solve and the `τ = I·ω/dt` servo/brake
    // targets are all written against a constant `dt`), so it asks for
    // `Time<Fixed>` rather than the ambient generic clock. Registering it into
    // `Update` — where the generic clock is the variable-dt render clock — now
    // fails to compile instead of silently integrating with a frame-rate-dependent
    // step. Rollback replay is unaffected: `replay_one_tick` runs
    // `RollbackReplay` with `Time<Fixed>`'s own delta.
    time: Res<Time<Fixed>>,
) {
    use std::f64::consts::TAU;

    let dt = time.delta_secs_f64();
    if dt <= 0.0 {
        return;
    }

    for (entity, mut wheel, local_tf, global_tf, hits, parent) in q_wheels.iter_mut() {
        // A ray can report a zero-normal hit when its origin is inside a
        // collider. Suspension rejects that as non-contact; the spin solver
        // must use the same contact selection or it will solve grip against a
        // fabricated flat normal for one tick at the terrain transition.
        let contact = hits
            .iter_sorted()
            .find(|hit| hit.normal.is_finite() && hit.normal.length_squared() > 1.0e-12);

        // All dynamics coefficients are USD-derived (stored on the component).
        let r = wheel.wheel_radius.max(1e-3);
        let inertia = wheel.axle_inertia();
        let k_slip = wheel.slip_stiffness;
        let c_bearing = wheel.bearing_damping;
        let friction_mu = wheel.friction_mu;

        // Signed throttle: positive drives forward, negative reverses.
        let throttle = q_ports
            .get(wheel.drive_port)
            .map(|p| p.value.clamp(-1.0, 1.0))
            .unwrap_or(0.0);
        // THE MOTOR CURVE, ON THE AXLE — [`lunco_hardware::axle_torque`], the one
        // definition both realizations evaluate. `drive_torque_max` is the STALL
        // torque (what the motor delivers at ω = 0) and `max_rotation_speed` the
        // axle no-load speed, both geared down from `lunco:motor:*` through
        // `PowertrainParams`. The curve bounds this wheel's torque authority, so a
        // wheel that breaks traction terminates at its no-load speed rather than
        // accelerating until bearing drag alone balances it.
        //
        // The curve is four-quadrant: above `u·ω_max` it returns negative torque
        // (back-EMF braking), so commanding reverse while still spinning forwards —
        // or rolling downhill past no-load — has real authority rather than a
        // released motor.
        //
        // ── THROTTLE SETS A SPEED, NOT ONLY A TORQUE ──────────────────────────
        //
        // The ceiling below is the torque–speed curve; the TARGET is `u·ω_max`,
        // because that is what a wheel-hub servo does and what the joint-wheel
        // realization of this same motor does (`lunco_hardware::MotorActuator`
        // writes `target_velocity = u·max_omega` with this curve as its
        // `max_torque`). Treating `u` as a pure torque scale instead let a
        // part-throttled wheel free-run all the way to the FULL no-load speed:
        // at `u = 0.4` the raycast wheel accelerated toward 12 rad/s while its
        // joint twin was held to 4.8 and braked back to it.
        //
        // Straight-line driving never saw it — there `u = 1` on every wheel and
        // the two laws coincide exactly. It only appears under STEER, which is
        // the one regime where the sides carry different throttles, and it is
        // half of why `drivetrain_parity`'s heading check was the only one
        // failing: the raycast rover's inner wheels kept pushing where the
        // physical rover's were regenerating, so the two got different yaw
        // moments from the same command.
        //
        // Freewheel at zero demand — not a brake — matching the joint motor,
        // which disables itself rather than commanding a stop.
        let tau_drive = if throttle.abs() <= f64::EPSILON {
            0.0
        } else if wheel.max_rotation_speed > 0.0 {
            // ONE curve, shared as CODE now rather than as a rule: the identical
            // authored law the joint wheel's `MotorActuator` evaluates. `spin_velocity`
            // is already in the demand-positive sense (the servo below targets
            // `throttle · ω_max` in it), which is the sense the curve is written in.
            let ceiling = lunco_hardware::axle_torque(
                wheel.drive_torque_max,
                wheel.max_rotation_speed,
                throttle,
                wheel.spin_velocity,
            )
            .abs();
            // Servo to the commanded speed, held inside the curve — the SAME
            // shape as the brake below, which servos to zero inside its own
            // ceiling. One idiom, two ceilings.
            //
            // WHAT THE TWO REALIZATIONS SHARE IS THE CURVE, NOT THE SERVO. The
            // authored torque–speed law and `ω_max` are single-sourced through
            // `PowertrainParams`, and both kinds clamp to them. How hard a servo
            // pushes toward the target is the actuator's own business: avian
            // drives its joint motor with `lunco:wheel:driveDamping`, this one
            // closes in a step. Neither gain is observable, because the curve
            // saturates first in every regime the vehicle actually uses — so
            // reproducing avian's gain here would copy a solver's internals (and
            // its `dt`-dependence) into this crate for no measurable difference,
            // and would go stale the moment avian changed its motor model.
            let w_target = throttle * wheel.max_rotation_speed;
            (-w_stop_torque(wheel.spin_velocity - w_target, inertia, dt)).clamp(-ceiling, ceiling)
        } else {
            // No authored no-load speed: nothing defines a target to servo to, so
            // the curve degenerates to the plain torque source its stall figure
            // describes — which is exactly what `axle_torque` returns here.
            lunco_hardware::axle_torque(
                wheel.drive_torque_max,
                wheel.max_rotation_speed,
                throttle,
                wheel.spin_velocity,
            )
        };

        // Ground speed at the contact patch, split on the wheel's own axes in the
        // ACTUAL contact plane. Both components are needed here now, not just the
        // longitudinal one: this system is where the tire force is decided.
        let mut v_long = 0.0;
        let mut v_lat = 0.0;
        // The contact basis, kept so the force can be rebuilt in world axes below.
        let mut basis = (DVec3::NEG_Z, DVec3::X);
        // The brake is a VESSEL command, so it is resolved by walking to the
        // vessel rather than read off whatever body this wheel hangs from — see
        // `owning_input_ports`. The chassis fetch below stays the carrier's,
        // because that IS the body whose motion sets the contact-patch velocity.
        let braking = lunco_core::architecture::owning_input_ports(entity, &q_child_of, &q_inputs)
            .map(|c| c.brake_active)
            .unwrap_or(false);
        if let Ok((lin, ang, pos, rot, _inputs, body, motion)) = q_chassis.get(parent.parent()) {
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
            // Reconstruct the hub in the grid-absolute physics frame from the
            // chassis body pose + the wheel's chassis-local transform (the wheel
            // is a `ChildOf` the chassis, so `local_tf` *is* that transform) —
            // never from `global_tf.translation()`, whose render frame drifted
            // the slip lever once the rover drove off origin (CQ-201).
            let (hub_pos, _) = wheel_hub_pose(
                GridPos(pos.0),
                GridRot(rot.0),
                local_tf.translation.as_dvec3(),
                local_tf.rotation.as_dquat(),
            );
            let hub_vel = wheel_hub_velocity(vlin, vang, hub_pos, GridPos(pos.0));
            let wheel_rot = GridRot::from_render_rotation(global_tf.rotation());
            let wheel_forward = wheel_rot.0 * DVec3::NEG_Z;
            let wheel_right = wheel_rot.0 * DVec3::X;
            // Decompose in the CONTACT plane (the ray-hit normal), not a flat
            // wheel basis — the same basis `apply_wheel_drive` applies the force
            // in, so a leaning or side-sloped wheel splits slip correctly.
            let normal = contact.as_ref().map(|h| h.normal).unwrap_or(DVec3::Y);
            basis = crate::contact_plane_basis(wheel_forward, wheel_right, normal);
            v_long = hub_vel.dot(basis.0);
            v_lat = hub_vel.dot(basis.1);
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

        // ONE precedence for both realizations: a braked wheel is not also a
        // driven wheel. The joint motor cannot express "drive plus brake" at all —
        // a velocity motor holds one target — so summing them here would have made
        // throttle-and-brake-together mean different things on the two drivetrains,
        // which is the exact class of divergence the parity scenes exist to catch.
        let tau_drive = if braking { 0.0 } else { tau_drive };

        let on_ground = wheel.last_normal_force >= 1.0 && contact.is_some();
        let mut w = wheel.spin_velocity;
        let mu_n = friction_mu * wheel.last_normal_force;
        // Longitudinal tire force, filled by whichever branch below resolves ω so
        // the axle and the chassis are answered by ONE number. See the block after
        // the solve for why that mattered.
        let mut f_long = 0.0;

        if on_ground {
            // Implicit grip solve assuming traction is unsaturated. Stiff term
            // k_slip is handled implicitly so ω snaps to ~v/r without exploding.
            let denom = inertia / dt + k_slip * r * r + c_bearing;
            let w_grip = (inertia / dt * w + tau_drive + tau_brake + k_slip * r * v_long) / denom;
            let f_slip = k_slip * (w_grip * r - v_long);

            if f_slip.abs() <= mu_n {
                // Tire grips: rolls at ground speed plus a tiny steady slip. The
                // force the ground feels is that steady slip times the stiffness —
                // small at cruise, which is the whole point (see below).
                w = w_grip;
                f_long = f_slip;
            } else {
                // Traction broken: kinetic friction saturates at μ·N and opposes
                // the slip direction. Integrate explicitly — the stiff term is
                // gone, so ω runs away from v/r (wheelspin under drive, or a
                // locked skid when the brake torque wins).
                let slip_sign = (w * r - v_long).signum();
                let tau_traction = slip_sign * mu_n * r;
                w += dt * (tau_drive + tau_brake - tau_traction - c_bearing * w) / inertia;
                f_long = tau_traction / r;
            }
        } else {
            // Airborne: free spin under drive (and any brake) torque vs bearing drag.
            w += dt * (tau_drive + tau_brake - c_bearing * w) / inertia;
        }

        wheel.spin_velocity = w;
        wheel.spin_angle = (wheel.spin_angle + w * dt).rem_euclid(TAU);

        #[cfg(feature = "drive-diag")]
        {
            if let Ok(dbgport) = q_ports.get(wheel.drive_port) {
                if dbgport.value.abs() > f64::EPSILON {
                    let (vlin, vang) = q_chassis
                        .get(parent.parent())
                        .map(|(l, a, _, _, _, _, _)| (l.0, a.0))
                        .unwrap_or((DVec3::ZERO, DVec3::ZERO));
                    bevy::log::info!(
                        "[drive-diag] update_wheel_spin: wheel={:?} port={} w={:.3} tau={:.1} f_long={:.1} muN={:.1} chassis_v=({:.3},{:.3},{:.3}) yaw_rate={:.4}",
                        entity, dbgport.value, w, tau_drive, f_long, mu_n, vlin.x, vlin.y, vlin.z, vang.y
                    );
                }
            }
        }

        // ── THE SAME READBACK THE JOINTED WHEEL PUBLISHES ─────────────────────
        // `lunco_hardware::MotorReadback` on the wheel entity: delivered axle
        // torque (N·m) and axle speed (rad/s), as ports. Both realizations report
        // it under the same names in the same sense, which is what makes a
        // parity plot — or one authored `lunco:telemetry` channel on
        // `components/mobility/wheel.usda` — work for either kind of rover
        // without knowing which one it got.
        //
        // Drive plus brake, because both are the drivetrain acting on the axle;
        // traction and bearing drag are the ground and the bearing, not the
        // machine.
        if let Ok(target) = q_targets.get(entity) {
            if let Ok(mut r) = q_readback.get_mut(target.0) {
                r.torque = tau_drive + tau_brake;
                r.axle_speed = w;
            }
        }

        // ── THE TIRE FORCE — ONE number, for the axle AND the chassis ──────────
        //
        // The chassis force used to be computed independently, in
        // `apply_wheel_drive`, from quantities the axle never saw: a drive term
        // `throttle · N · driveForcePerNormal` and a drag term proportional to the
        // FULL travel speed. Neither is a contact force. A tire transmits torque by
        // SLIPPING — the patch force is `k · (ω·r − v)` — so a model that reads
        // travel speed as slip is claiming a rolling wheel is sliding at road
        // speed, and has to be given a drive coefficient large enough to overcome
        // its own invented drag. That is exactly what the authored pair 2.0 / 50
        // was: two fudges calibrated to cancel.
        //
        // They cancelled ONLY at the parameters they were fitted at. Change μ, or
        // the tire, or the mass, and the cancellation drifts, and every knob
        // interacted with every other.
        //
        // Now both halves read the SAME `f_long` the ω solve just produced, and the
        // cone is spent on real forces:
        //   * at cruise ω ≈ v/r, so `f_long` is a few newtons and nearly the whole
        //     cone is available laterally — a rover holds its line;
        //   * under wheelspin `f_long` saturates at μ·N, and the surplus torque
        //     goes where it physically goes: into ω, as visible spin.
        // Nothing is calibrated against anything else, so μ means μ and a tire
        // swapped at runtime behaves like the tire it is.
        // ── LATERAL: A SLIP ANGLE, NOT A LATERAL VELOCITY ──────────────────────
        //
        // The attribute is `physxVehicleTire:lateralStiffness` and every text —
        // and PhysX — means CORNERING STIFFNESS by that: side force per RADIAN of
        // slip angle. This computed `-k · v_lat`: force per metre-per-second of
        // lateral velocity. Those are not the same model and they do not differ by
        // a constant, because `v_lat = |v| · sin(α)`.
        //
        // The consequence is that grip vanished with speed. At the same slip angle
        // — the same wheel scrubbing equally badly sideways — a wheel at 0.5 m/s
        // got a tenth the side force of one at 5 m/s, so a slow rover slid
        // sideways almost freely. That is why `drivetrain_parity` measured the
        // raycast rover sweeping 53.3° against the jointed rover's 12.2°, and why
        // the fit never converged: the parity handover proved by sweep that NO
        // value of `k` satisfies all five scenes, and `tires/regolith.usda` carries
        // the record of that search. A constant cannot fix a model that has the
        // wrong independent variable.
        //
        // A slip angle is speed-free by construction, so one authored number now
        // means the same grip at every speed.
        //
        // The slip-angle reference is kinematic: the larger of hub travel speed
        // and circumferential wheel speed. A pivot wheel therefore retains a
        // well-defined angle even while its hub's longitudinal speed is near zero.
        // The stiffness is NORMALISED BY LOAD — side force per radian PER NEWTON of
        // normal force — so one authored number describes the tyre rather than the
        // tyre-on-this-particular-rover. That is the same reason PhysX states its
        // `lateralStiffness` per unit gravity, and it is what the old absolute
        // value could not do: `tires/regolith.usda` records a fit that worked on
        // one vehicle and had to be re-fitted for the next.
        //
        // It also puts the saturation point where a tyre spec actually states it.
        // The linear region ends when `C·N·α` reaches the cone `μ·N`, i.e. at
        // `α_peak = μ / C`, independent of load: at C = 10 a μ = 0.8 regolith tyre
        // develops full side force at 4.6° of slip, which is where a real one does.
        // ── THE FRICTION CONE — SCALE THE FORCE, DON'T RE-DERIVE IT ────────────
        //
        // The two components above are the whole tire model: `f_long` from the slip
        // RATIO solve, `f_lat` from the slip ANGLE. When their resultant exceeds the
        // cone `μ·N`, the patch cannot deliver it, so the pair is scaled back onto
        // the cone along its own direction. That is ONE force law with a magnitude
        // bound.
        //
        // It used to re-derive a direction here from `(ω·r − v_long, −v_lat)` — a
        // slip-VELOCITY vector. That reintroduced, in the saturated regime only, the
        // exact lateral variable the slip-angle model was written to replace
        // (`v_lat`, whose grip vanishes with speed), so the tire obeyed one lateral
        // law below the cone and a different one above it. Two laws in one function
        // is the bug, not the numbers: the authored `cornering_stiffness` set the
        // direction the force pointed under grip and had no say in it under
        // saturation.
        //
        // The low-speed definition lives in `tire_patch_force`: actual `omega*r`
        // keeps a rolling/pivoting wheel's angle defined, and `atan2` supplies the
        // static limit when both longitudinal motions vanish. The authored lower
        // validation bound prevents a steady-state test curve being extrapolated
        // below its measured domain; it is evidence metadata, not a fitted switch.
        let (f_long, f_lat) = if on_ground {
            crate::tire_patch_force(
                f_long,
                v_long
                    .abs()
                    .max((wheel.spin_velocity * r).abs())
                    .max(wheel.min_validated_speed),
                v_lat,
                wheel.last_normal_force,
                friction_mu,
                wheel.cornering_stiffness,
            )
        } else {
            (0.0, 0.0)
        };
        wheel.tire_force = basis.0 * f_long + basis.1 * f_lat;

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
    use crate::{Suspension, WheelRaycast};
    use avian3d::prelude::*;
    use bevy::math::DVec3;
    use bevy::prelude::*;
    use bevy::time::{Time, TimePlugin, TimeUpdateStrategy};
    use lunco_core::ActuatorPorts;
    use std::time::Duration;

    /// Put a test app on the SAME clock the product runs on: fixed steps of
    /// exactly `dt`.
    ///
    /// ⚠ It does NOT guarantee one tick per `app.update()` — the accumulator is
    /// filled by the frame that just ran, so the first update runs `FixedUpdate`
    /// zero times. A test that asserts on a specific NUMBER of ticks must drive
    /// the schedule itself (see `run_raycast_spin`); a test that only wants a
    /// steady state can loop updates and ignore the off-by-one.
    ///
    /// `update_wheel_spin` reads `Time<Fixed>` and is registered in `FixedUpdate`,
    /// so a test that pokes the generic clock and registers into `Update` exercises
    /// a path the vehicle never takes. `TimeUpdateStrategy::ManualDuration` pins the
    /// real-clock advance and `Time::<Fixed>` is matched to it, so every step that
    /// runs is exactly `dt` — deterministic, and `dt` is explicit at every call
    /// site. This is how every other physics test in the workspace drives the
    /// fixed schedule (see `lunco_cosim::joint` tests).
    fn app_on_fixed_clock(dt: f64) -> App {
        let mut app = App::new();
        app.add_plugins(TimePlugin);
        app.insert_resource(Time::<Fixed>::from_duration(Duration::from_secs_f64(dt)));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            dt,
        )));
        app
    }

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
        // dt = 0.1 s, chosen so the expected gripped ω below is exact arithmetic
        // (inertia/dt = 10). One fixed tick per `app.update()`.
        let mut app = app_on_fixed_clock(0.1);

        // Entity with no `Port` → throttle reads 0 (free-rolling, so the spin
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
                ActuatorPorts::default(),
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
                visual_entity: Some(visual),
                last_normal_force: 100.0, // ≥1 ⇒ on_ground (with a hit present)
                spin_angle: 0.0,
                spin_velocity: 0.0,
                mass: 8.0,
                moment_of_inertia: 1.0, // overrides ½mr² ⇒ inertia = 1.0 (clean)
                reflected_inertia: 0.0,
                drive_torque_max: 0.0,
                max_rotation_speed: 12.0,
                bearing_damping: 0.0,
                friction_mu: 1.0,
                slip_stiffness: 1000.0,
                cornering_stiffness: 10.0,
                min_validated_speed: 0.0,
                brake_torque_max: 0.0,
                tire_force: DVec3::ZERO,
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
            RayHits(vec![RayHitData {
                entity: chassis,
                distance: 0.5,
                normal: DVec3::Y,
            }]),
            ChildOf(chassis),
        ));

        app.add_systems(FixedUpdate, update_wheel_spin);
        // EXACTLY ONE FIXED TICK, DRIVEN EXPLICITLY — `app.update()` will not do.
        //
        // The expected ω below is one step of the implicit grip solve, so this
        // measurement is only meaningful for a known number of ticks. But the
        // fixed accumulator starts empty and is filled by the frame that just
        // ran, so the FIRST `app.update()` banks `dt` and runs `FixedUpdate`
        // ZERO times — the wheel never integrated and the test read a pristine
        // `spin_velocity: 0.0` as a physics answer. (The sibling
        // no-load-speed test never noticed: it loops 600 updates, where one lost
        // tick is invisible.) Advancing `Time<Fixed>` and running the schedule
        // makes the tick count the thing the test actually controls.
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(Duration::from_secs_f64(0.1));
        app.world_mut().run_schedule(FixedUpdate);

        app.world_mut()
            .query::<&WheelRaycast>()
            .iter(app.world())
            .next()
            .unwrap()
            .spin_velocity
    }

    /// A wheel with nothing to grip must terminate at the MOTOR's no-load speed,
    /// not at whatever bearing drag happens to balance stall torque.
    ///
    /// REGRESSION: `tau_drive` applied the stall torque flat at every ω, so a
    /// wheel that broke traction ran to `τ_stall / c_bearing` — hundreds of rad/s
    /// against a real axle limit of 24. Since the drive torque is several times
    /// the traction limit, that happened on any throttle at all, which is what
    /// "the wheels spin far too fast" was.
    #[test]
    fn a_free_spinning_wheel_stops_at_the_motors_no_load_speed() {
        let max_omega = 24.0;
        // The product's step: 60 Hz fixed, one tick per update.
        let mut app = app_on_fixed_clock(1.0 / 60.0);

        let port = app
            .world_mut()
            .spawn(lunco_core::architecture::Port {
                value: 1.0,
                ..default()
            })
            .id();
        let chassis = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Position(DVec3::ZERO),
                Rotation::default(),
                LinearVelocity(DVec3::ZERO),
                AngularVelocity(DVec3::ZERO),
                ActuatorPorts::default(),
            ))
            .id();
        let visual = app.world_mut().spawn(Transform::default()).id();
        let wheel = app
            .world_mut()
            .spawn((
                WheelRaycast {
                    suspension_port: port,
                    drive_port: port,
                    steer_port: port,
                    steer_axis: DVec3::Y,
                    wheel_radius: 0.4,
                    visual_entity: Some(visual),
                    // AIRBORNE: no normal force, no hit — nothing to push against,
                    // so only the motor curve and bearing drag bound the spin.
                    last_normal_force: 0.0,
                    spin_angle: 0.0,
                    spin_velocity: 0.0,
                    mass: 25.0,
                    moment_of_inertia: 0.0,
                    reflected_inertia: 0.0,
                    drive_torque_max: 255.0,
                    max_rotation_speed: max_omega,
                    bearing_damping: 0.45,
                    friction_mu: 0.8,
                    slip_stiffness: 8000.0,
                    cornering_stiffness: 10.0,
                    min_validated_speed: 0.0,
                    brake_torque_max: 1500.0,
                    tire_force: DVec3::ZERO,
                },
                Suspension {
                    rest_length: 0.7,
                    spring_k: 5000.0,
                    damping_c: 600.0,
                    local_axis: DVec3::Y,
                },
                Transform::default(),
                GlobalTransform::default(),
                RayHits(vec![]),
                ChildOf(chassis),
            ))
            .id();

        app.add_systems(FixedUpdate, update_wheel_spin);
        // Ten seconds of full throttle with nothing to grip.
        for _ in 0..600 {
            app.update();
        }
        let w = app
            .world()
            .get::<WheelRaycast>(wheel)
            .unwrap()
            .spin_velocity;
        assert!(
            w <= max_omega + 1e-6,
            "a free wheel must not pass its motor's no-load speed: {w} > {max_omega}"
        );
        assert!(
            w > max_omega * 0.9,
            "…and should actually reach it under full throttle: {w}"
        );
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
        assert!(
            (near - 1.9231).abs() < 1e-2,
            "expected gripped ω≈1.923, got {near}"
        );
    }
}
