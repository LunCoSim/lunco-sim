//! # Tire Spin Integrator
//!
//! Raycast wheels are kinematic proxies — the chassis dynamics live on the
//! parent rigid body, so the wheel mesh carries no angular velocity of its own.
//! This module gives each tire a real rotational state so the spin you *see*
//! matches the physics the rover is actually experiencing.

use avian3d::prelude::*;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_core::coords::{GridPos, GridRot, VehicleFrame};
use lunco_core::InputPorts;

use crate::wheel_kinematics::{wheel_heading, wheel_hub_pose, wheel_hub_velocity};
use crate::{WheelBodyMount, WheelRaycast};

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
/// is read from the USD wheel component (mass, friction, and tire) — see
/// `setup_raycast_wheel` — so the spin you see is grounded in the authored data:
/// - `I = ½·m·r²` — solid-disk inertia from USD `physxVehicleWheel:mass` and radius.
/// - `τ_drive` is the solved physical torque published by the authored
///   mechanical network; there is no Rust-side motor curve or command clamp.
/// - **Grounded**: the contact slip `(ω·r − v)` is resisted by tire grip with a
///   stiff longitudinal stiffness, capped by the Coulomb limit `μ·N`. Below the
///   limit the wheel grips (`ω → v/r`, solved implicitly for unconditional
///   stability); above it the tire breaks loose and `ω` runs away from `v/r`
///   (visible wheelspin or lock-up skid). This is the standard slip-ratio model.
/// - **Airborne**: no contact → no traction; `ω` spins up under `τ_drive` and
///   bleeds off through bearing drag.
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
        &RayHits,
        &WheelBodyMount,
    )>,
    mut q_ports: ParamSet<(
        Query<&lunco_core::architecture::Port>,
        Query<&mut lunco_core::architecture::Port>,
    )>,
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
        // The wheel body owner is resolved from authored topology and carried by
        // `WheelBodyMount`. A raycast wheel may be nested under a visual or
        // suspension carrier, so its ECS parent is not a physics ownership edge.
        With<RigidBody>,
    >,
    mut q_visual: Query<&mut Transform, Without<WheelRaycast>>,
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

    for (entity, mut wheel, local_tf, hits, mount) in q_wheels.iter_mut() {
        // A ray can report a zero-normal hit when its origin is inside a
        // collider. Suspension rejects that as non-contact; the spin solver
        // must use the same contact selection or it will solve grip against a
        // fabricated flat normal for one tick at the terrain transition.
        let contact = hits
            .iter_sorted()
            .find(|hit| hit.normal.is_finite() && hit.normal.length_squared() > 1.0e-12);

        // All dynamics coefficients are USD-derived (stored on the component).
        let Some(r) = (wheel.wheel_radius.is_finite() && wheel.wheel_radius > 0.0)
            .then_some(wheel.wheel_radius)
        else {
            continue;
        };
        let Some(inertia) = wheel.axle_inertia() else {
            continue;
        };
        let k_slip = wheel.slip_stiffness;
        let c_bearing = wheel.bearing_damping;
        let friction_mu = wheel.friction_mu;

        let Some(tau_drive) = ({
            let q_read_ports = q_ports.p0();
            q_read_ports
                .get(wheel.drive_port)
                .ok()
                .map(|port| port.value)
        }) else {
            continue;
        };

        // Ground speed at the contact patch, split on the wheel's own axes in the
        // ACTUAL contact plane. Both components are needed here now, not just the
        // longitudinal one: this system is where the tire force is decided.
        let mut v_long = 0.0;
        let mut v_lat = 0.0;
        // The contact basis, kept so the force can be rebuilt in world axes below.
        let mut basis = (VehicleFrame::FORWARD_LOCAL, VehicleFrame::RIGHT_LOCAL);
        // The brake is a VESSEL command, so it is resolved by walking to the
        // vessel rather than read off whatever body this wheel hangs from — see
        // `owning_input_ports`. The chassis fetch below stays the carrier's,
        // because that IS the body whose motion sets the contact-patch velocity.
        let braking = lunco_core::architecture::owning_input_ports(entity, &q_child_of, &q_inputs)
            .map(|c| c.brake_active)
            .unwrap_or(false);
        if let Ok((lin, ang, pos, rot, _inputs, body, motion)) = q_chassis.get(mount.body) {
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
            // resolved body pose and authored body-local mount —
            // never from `global_tf.translation()`, whose render frame drifted
            // the slip lever once the rover drove off origin (CQ-201).
            let wheel_local_rotation =
                mount.local.rotation.as_dquat() * local_tf.rotation.as_dquat();
            let (hub_pos, _) = wheel_hub_pose(
                GridPos(pos.0),
                GridRot(rot.0),
                mount.local.translation.as_dvec3(),
                wheel_local_rotation,
            );
            let hub_vel = wheel_hub_velocity(vlin, vang, hub_pos, GridPos(pos.0));
            let (wheel_forward, wheel_right) = wheel_heading(GridRot(rot.0), wheel_local_rotation);
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
        let on_ground = wheel.last_normal_force >= 1.0 && contact.is_some();
        // This is the shared analytic tire solve. The physical realization gets
        // its normal load and contact point from Avian, then calls this same
        // longitudinal/lateral law; Avian's generic tangent friction is disabled
        // for those marked wheel contacts so it cannot create a second model.
        let (w, f_long) = crate::longitudinal_tire_step(
            wheel.spin_velocity,
            v_long,
            r,
            inertia,
            k_slip,
            c_bearing,
            tau_drive,
            tau_brake,
            if on_ground {
                wheel.last_normal_force
            } else {
                0.0
            },
            friction_mu,
            dt,
        );

        wheel.spin_velocity = w;
        wheel.spin_angle = (wheel.spin_angle + w * dt).rem_euclid(TAU);
        if let Ok(mut speed_port) = q_ports.p1().get_mut(wheel.speed_port) {
            speed_port.value = w;
        }

        // ── THE TIRE FORCE — ONE number, for the axle AND the chassis ──────────
        //
        // The chassis force used to be computed independently, in
        // `apply_wheel_drive`, from quantities the axle never saw: a drive term
        // a throttle-scaled normal force and a drag term proportional to the FULL
        // travel speed. Neither is a contact force. A tire transmits torque by
        // SLIPPING — the patch force is `k · (ω·r − v)` — so a model that reads
        // travel speed as slip is claiming a rolling wheel is sliding at road
        // speed, and has to be given a drive coefficient large enough to overcome
        // its own invented drag. That was a pair of calibration fudges, not a tire
        // property.
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
        // ── LATERAL: THE STANDARD PHYSX LOAD GRAPH ─────────────────────────────
        //
        // The tire's lateral stiffness is authored as
        // `physxVehicleTire:lateralStiffnessGraph`, not as a LunCo scalar. The
        // shared patch law evaluates that graph at this contact's normal load
        // and then applies the resulting force to the slip angle.
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
        // The graph's reference load makes the same tire setup usable across
        // contact loads while retaining the standard load-dependent response.
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
        // is the bug, not the numbers: a former scalar stiffness set the direction
        // the force pointed under grip and had no say in it under saturation.
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
                wheel.lateral_stiffness_graph,
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
    use crate::{Suspension, WheelBodyMount, WheelRaycast};
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

    #[test]
    fn wheel_inertia_rejects_unprojected_values_instead_of_using_a_floor() {
        assert_eq!(WheelRaycast::default().axle_inertia(), None);

        let wheel = WheelRaycast {
            wheel_radius: 0.5,
            mass: 8.0,
            moment_of_inertia: 0.0,
            ..default()
        };
        assert_eq!(wheel.axle_inertia(), Some(1.0));
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

        // A real solved drive endpoint is required even for zero torque. The
        // projection never substitutes a missing endpoint with a Rust default.
        let port = app
            .world_mut()
            .spawn(lunco_core::architecture::Port { value: 0.0 })
            .id();
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
                speed_port: port,
                steer_port: port,
                steer_axis: DVec3::Y,
                wheel_radius: 0.5,
                visual_entity: Some(visual),
                last_normal_force: 100.0, // ≥1 ⇒ on_ground (with a hit present)
                spin_angle: 0.0,
                spin_velocity: 0.0,
                mass: 8.0,
                moment_of_inertia: 1.0, // overrides ½mr² ⇒ inertia = 1.0 (clean)
                bearing_damping: 0.0,
                friction_mu: 1.0,
                slip_stiffness: 1000.0,
                lateral_stiffness_graph: crate::TireLateralStiffnessGraph {
                    minimum_normalized_load: 1.0,
                    max_stiffness: 4_000.0,
                    rest_load: 400.0,
                },
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
            WheelBodyMount {
                body: chassis,
                local: Transform::from_translation(wheel_local),
            },
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

    /// A free wheel follows the physical solved torque and bearing loss. Its
    /// speed is not capped by a duplicated Rust no-load-speed rule.
    #[test]
    fn a_free_spinning_wheel_follows_solved_torque_and_bearing_loss() {
        // The product's step: 60 Hz fixed, one tick per update.
        let mut app = app_on_fixed_clock(1.0 / 60.0);

        let port = app
            .world_mut()
            .spawn(lunco_core::architecture::Port { value: 1.0 })
            .id();
        let speed_port = app
            .world_mut()
            .spawn(lunco_core::architecture::Port { value: 0.0 })
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
                    speed_port,
                    steer_port: port,
                    steer_axis: DVec3::Y,
                    wheel_radius: 0.4,
                    visual_entity: Some(visual),
                    // AIRBORNE: no normal force, no hit — the solved shaft torque
                    // and authored bearing loss are the only rotational terms.
                    last_normal_force: 0.0,
                    spin_angle: 0.0,
                    spin_velocity: 0.0,
                    mass: 25.0,
                    moment_of_inertia: 0.0,
                    bearing_damping: 0.45,
                    friction_mu: 0.8,
                    slip_stiffness: 8000.0,
                    lateral_stiffness_graph: crate::TireLateralStiffnessGraph {
                        minimum_normalized_load: 1.0,
                        max_stiffness: 4_000.0,
                        rest_load: 400.0,
                    },
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
                WheelBodyMount {
                    body: chassis,
                    local: Transform::IDENTITY,
                },
                GlobalTransform::default(),
                RayHits(vec![]),
                ChildOf(chassis),
            ))
            .id();

        app.add_systems(FixedUpdate, update_wheel_spin);
        // Ten seconds of full throttle with nothing to grip. Drive the fixed
        // schedule explicitly so this test owns the exact number of integration
        // steps rather than depending on the app's outer time accumulator.
        for _ in 0..600 {
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .advance_by(Duration::from_secs_f64(1.0 / 60.0));
            app.world_mut().run_schedule(FixedUpdate);
        }
        let w = app
            .world()
            .get::<WheelRaycast>(wheel)
            .unwrap()
            .spin_velocity;
        let dt: f64 = 1.0 / 60.0;
        let inertia: f64 = 0.5 * 25.0 * 0.4 * 0.4;
        let equilibrium: f64 = 1.0 / 0.45;
        let expected = equilibrium * (1.0 - (1.0 - 0.45 * dt / inertia).powi(600));
        assert!(
            (w - expected).abs() < 0.05,
            "free wheel must follow the torque/bearing integration law: {w} vs {expected}"
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
