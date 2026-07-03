//! # Surface Mobility & Traction Physics
//!
//! This crate implements the core physics models for planetary rovers and 
//! surface exploration vehicles. 
//!
//! ## The "Why": Raycast-Based Ground Interaction
//! Traditional mesh-to-mesh collision for wheels is computationally expensive 
//! and prone to "snagging" on terrain geometry. We use a **Raycast Wheel** 
//! model to provide a stable, high-performance alternative:
//! 1. **Suspension Logic**: An emulated spring-damper system computes normal 
//!    forces based on ray length, preventing high-frequency jitter.
//! 2. **Traction Physics**: Lateral and longitudinal friction are applied 
//!    at the ray's contact point, allowing for complex skid and slip behaviors 
//!    without the overhead of continuous contact manifolds.
//! 3. **Numeric Stability**: By projecting a single ray, we ensure the wheel 
//!    always "floats" at the correct elevation, even on highly irregular 
//!    procedural terrain.
//!
//! ## Control Mixing Models
//! The crate supports hotswappable steering architectures:
//! - **Differential (Skid) Drive**: Common for heavy loaders and excavators; 
//!   turns by varying velocity between left and right tracks.
//! - **Ackermann Steering**: Standard for high-speed mobility; pivots leading 
//!   wheels to maintain a common center of rotation, reducing tire scrub.

use bevy::prelude::*;
use bevy::ecs::schedule::common_conditions::any_with_component;
use bevy::math::{DQuat, DVec3};
use avian3d::prelude::*;
use lunco_core::architecture::DigitalPort;
use lunco_core::ports::{PortBackend, PortDirection, PortRef, PortType};
use lunco_core::kernels::{ControlKernelRegistry, DriveMix};
use lunco_fsw::FlightSoftware;

mod sensing;
mod wheel_spin;
use wheel_spin::update_wheel_spin;

pub mod wheel_kinematics;
use wheel_kinematics::{wheel_hub_pose, wheel_hub_velocity};

/// Drive-actuation chain diagnostic logging — see the `drive-diag` feature in
/// `Cargo.toml`. Expands to `info!` when the feature is on, and to nothing
/// (args not evaluated) when off, so it's zero-cost in normal builds. A single
/// definition keeps the `#[cfg]` out of the physics systems themselves.
#[cfg(feature = "drive-diag")]
macro_rules! drive_diag {
    ($($arg:tt)*) => { bevy::log::info!($($arg)*) };
}
#[cfg(not(feature = "drive-diag"))]
macro_rules! drive_diag {
    ($($arg:tt)*) => {};
}

/// Run `$body` only when the `drive-diag` feature is on. Used where the
/// diagnostic needs extra work (an extra port read + throttle guard) that must
/// also compile out, not just the log call.
#[cfg(feature = "drive-diag")]
macro_rules! drive_diag_block {
    ($body:block) => { $body };
}
#[cfg(not(feature = "drive-diag"))]
macro_rules! drive_diag_block {
    ($body:block) => {};
}

/// Manages the integration of mobility physics and control observers.
pub struct LunCoMobilityPlugin;

impl Plugin for LunCoMobilityPlugin {
    fn build(&self, app: &mut App) {
        // Expose physics-backed spatial queries (Raycast, GroundHeight) so the
        // API / MCP / rhai `query()` can sense geometry without depending on avian.
        sensing::register_physics_queries(app);
        // Bridge avian collision / trigger-volume events onto the telemetry bus
        // so scripts can react via `on_event` instead of polling distance().
        sensing::register_collision_event_bridge(app);

        app.register_type::<Suspension>()
           .register_type::<WheelRaycast>()
           // `DriveMix` (the kernel-selected allocation spec, replacing the old
           // per-arch `DifferentialDrive`/`AckermannSteer`/`GenericDriveMix`) is
           // registered by `lunco-core` alongside the kernel registry.
           .register_type::<DifferentialCoupling>()
           // G5 rocker-bogie differential — separate set: it doesn't read the
           // control ports, only couples two rocker hinges. Idle unless a
           // `DifferentialCoupling` exists, so it's free for every other vehicle.
           .add_systems(
               FixedUpdate,
               differential_coupling_system
                   .run_if(any_with_component::<DifferentialCoupling>)
                   .run_if(|t: Res<Time<Virtual>>| t.relative_speed_f64() > 0.0),
           )
           .add_systems(FixedUpdate, (
               suspension_system,
               apply_wheel_suspension,
               apply_wheel_drive,
               apply_wheel_steering,
               update_wheel_spin,
           ).chain()
           // Read `PhysicalPort` AFTER the DAC has propagated this tick's
           // `DigitalPort` command into it (same fixed tick), so actuation isn't
           // delayed an extra tick. See `lunco_core::ControlDacSet`.
           .after(lunco_core::ControlDacSet)
           .run_if(
               // Run wherever physics is live. On a pure client this used to be
               // skipped entirely (replicated rovers are server-authoritative
               // proxies); predict-own now lets the client locally simulate the
               // ONE rover it possesses. We don't gate by role here — the owned
               // rover is the only `Dynamic` chassis on a client (every other
               // replicated body is pinned `Kinematic` by `force_kinematic_proxies`),
               // and the per-chassis `RigidBody::Kinematic` guard inside each wheel
               // system already skips those. So host/standalone simulate every
               // rover (unchanged) and a client simulates only its owned one.
               |t: Res<Time<Virtual>>| t.relative_speed_f64() > 0.0));

        // Expose every FSW's logical command ports (a rover's throttle/steer/brake,
        // etc.) through the shared port substrate, so the ONE generic `SetPorts`
        // command (and wires/API/scripts) can drive any controllable by name.
        app.init_resource::<lunco_core::ports::PortRegistry>();
        {
            let mut reg = app
                .world_mut()
                .resource_mut::<lunco_core::ports::PortRegistry>();
            reg.register(FSW_COMMAND_BACKEND);
        }

        // Own the control-allocation kernel registry here (the plugin that runs
        // `apply_drive_mix`), seeded with the built-in `skid`/`linear` kernels —
        // so any app running the drive systems has it, without depending on the
        // full core plugin. Flight-kernel crates register additively the same way.
        if !app.world().contains_resource::<ControlKernelRegistry>() {
            app.insert_resource(ControlKernelRegistry::with_defaults());
        }

        // Mix the FSW's logical command inputs (written via the port backend) into
        // the actuator `DigitalPort`s BEFORE the DAC propagates them to `PhysicalPort`
        // (and before the wheel systems, which run `.after(ControlDacSet)`). The
        // command surface is derived from USD `Controls` bindings (never a Rust
        // literal) by `sync_fsw_command_surface`, ordered before the mix so a
        // freshly-loaded vessel is drivable the same tick its binding lands.
        app.add_systems(
            FixedUpdate,
            (sync_fsw_command_surface, apply_drive_mix)
                .chain()
                .before(lunco_core::ControlDacSet),
        );
    }
}

/// Max per-wheel drive force as a multiple of that wheel's normal force
/// (`throttle · N · this`). Caps traction to a fraction of the contact's
/// grip — i.e. how much the tyre can push before the friction cone limits it.
const DEFAULT_DRIVE_FORCE_PER_NORMAL: f64 = 2.0;
/// Default for [`WheelRaycast::contact_grip_stiffness`] (N·s/m) when USD does not
/// author `lunco:contactGripStiffness`.
const DEFAULT_CONTACT_GRIP_STIFFNESS: f64 = 50.0;

/// Upper clamp on the suspension force magnitude (N) applied per spring.
/// Bounds the spring+damping sum so a deeply-compressed strut or a numerical
/// velocity spike can't inject an explosive impulse that launches the rover.
const MAX_SUSPENSION_FORCE_N: f64 = 100_000.0;

/// Full-scale magnitude of a [`DigitalPort`] `raw_value` drive command:
/// `±DIGITAL_PORT_FULL_SCALE` maps to ±100% actuator authority (symmetric i16
/// range, leaving −32768 unused so + and − have equal span).
const DIGITAL_PORT_FULL_SCALE: i16 = 32767;

// ── Pure force laws (unit-tested; the numerically-sensitive bits live here) ─────

/// Per-wheel drive force magnitude from a normalized throttle, clamped to
/// `[-1, 1]`. NEGATIVE throttle drives in **reverse** — the old `clamp(0.0, 1.0)`
/// silently dropped reverse even though the differential mix carried the sign.
fn drive_force_mag(throttle: f64, normal_force: f64, force_per_normal: f64) -> f64 {
    throttle.clamp(-1.0, 1.0) * normal_force * force_per_normal
}

/// Contact friction opposing the slip *velocity vector*. Continuous through zero
/// (no dead-band) so a near-stationary wheel is still damped — a slip dead-band
/// left sub-threshold motion undamped and produced a stiction limit-cycle (the
/// steering jitter). Linear `-k·slip` below the Coulomb cone, saturating at it.
/// While `braking`, a locked wheel grips at the FULL cone (opposing all sliding)
/// so it actually decelerates the chassis.
/// Wheel traction basis projected into the contact plane defined by `normal`.
///
/// Returns `(forward, right)`: orthonormal vectors spanning the plane ⟂ to the
/// contact `normal`, with `forward` the wheel heading projected into that plane
/// and `right = forward × normal`. For an upright wheel (where `normal` is the
/// wheel's own up, so the heading already lies in the plane) this reproduces the
/// raw `(wheel_forward, wheel_right)` — existing rovers are byte-for-byte
/// unchanged. For a **leaning single-track vehicle** the contact normal tilts
/// with the lean, so decomposing slip/drive in this basis gives the correct
/// longitudinal/lateral split instead of assuming a flat patch. Falls back to
/// the raw vectors if the heading is parallel to the normal (degenerate).
fn contact_plane_basis(wheel_forward: DVec3, wheel_right: DVec3, normal: DVec3) -> (DVec3, DVec3) {
    let n = normal.normalize_or_zero();
    if n == DVec3::ZERO {
        return (wheel_forward, wheel_right);
    }
    let forward = (wheel_forward - n * wheel_forward.dot(n))
        .try_normalize()
        .unwrap_or(wheel_forward);
    let right = forward.cross(n).try_normalize().unwrap_or(wheel_right);
    (forward, right)
}

fn contact_friction(
    slip_vec: DVec3,
    grip_stiffness: f64,
    max_friction: f64,
    braking: bool,
) -> DVec3 {
    let slip_speed = slip_vec.length();
    if braking && slip_speed > 1e-6 {
        -slip_vec * (max_friction / slip_speed)
    } else if slip_speed * grip_stiffness <= max_friction {
        // Linear grip; `-k·slip` → 0 continuously as slip → 0 (no division).
        -slip_vec * grip_stiffness
    } else {
        // Saturated at the cone; slip_speed > 0 here.
        -slip_vec * (max_friction / slip_speed)
    }
}

/// Suspension normal-force magnitude: spring `k·x` plus damping `c·v`, with the
/// DAMPING bounded to ±spring so the total stays in `[0, 2·spring]` without a
/// `.max(0)` cliff. The cliff (clamping the *total* to ≥0) dropped damping on the
/// rebound half-cycle → an undamped suspension limit-cycle (the forward+turn
/// jitter); unbounded `c·v` also spiked the force on hard hits. Bounding the
/// damping term fixes both.
fn suspension_force_mag(compression: f64, spring_k: f64, relative_vel: f64, damping_c: f64) -> f64 {
    let spring = compression * spring_k;
    let damping = (relative_vel * damping_c).clamp(-spring, spring);
    spring + damping
}

/// A high-performance wheel model using emulated suspension rays.
///
/// **Theory**: Instead of a physical collider, this component projects a ray
/// downwards. The resulting distance is used to solve the spring-damper
/// equation, simulating the behavior of a physical tire and strut.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct WheelRaycast {
    /// Port mapping for suspension telemetry.
    pub suspension_port: Entity,
    /// Port mapping for drive torque actuation.
    pub drive_port: Entity,
    /// Port mapping for steering angle actuation.
    pub steer_port: Entity,
    /// Length of the suspension at rest in meters.
    pub rest_length: f64,
    /// Hooke's Law spring constant (Stiffness in N/m).
    pub spring_k: f64,
    /// Damping coefficient to suppress oscillations (Ns/m).
    pub damping_c: f64,
    /// Radius of the tire (effectively the minimum offset from ground).
    pub wheel_radius: f64,
    /// Y-offset of the ray origin relative to the wheel transform (meters).
    /// Used for visual wheel positioning.
    pub ray_origin_y: f64,
    /// Entity for the visual mesh to be transformed.
    pub visual_entity: Option<Entity>,
    /// Resultant normal force from the last physics tick, used for friction calculations.
    pub last_normal_force: f64,
    /// Accumulated rolling angle of the tire about its axle (radians, wrapped to 0..2π).
    /// Drives the visible spin of the wheel mesh.
    pub spin_angle: f64,
    /// Angular velocity of the tire about its axle (rad/s). State for the spin
    /// integrator — couples to ground speed when rolling, diverges under
    /// wheelspin/skid, and free-runs (driven by torque vs bearing drag) in the air.
    pub spin_velocity: f64,
    /// Tire mass in kg (USD `physics:mass`). Sets the rotational inertia
    /// `½·m·r²` that resists changes in spin (unless `moment_of_inertia` is set).
    pub mass: f64,
    /// Explicit axle moment of inertia in kg·m² (USD `physxVehicleWheel:moi`).
    /// When `> 0` it overrides the mass-derived `½·m·r²`.
    pub moment_of_inertia: f64,
    /// Peak motor drive torque about the axle in N·m. Derived from the USD
    /// motor curve (`motorPower / ω_noLoad`).
    pub drive_torque_max: f64,
    /// Axle bearing/rolling drag in N·m·s. Derived so the free (airborne) spin
    /// terminates at the motor's no-load speed (`drive_torque_max / ω_noLoad`).
    pub bearing_damping: f64,
    /// Tire-ground friction coefficient (USD `lunco:frictionCoefficient`).
    /// Caps the traction torque at `μ·N`, above which the tire breaks loose.
    pub friction_mu: f64,
    /// Longitudinal slip stiffness in N per m/s of contact slip. Governs how
    /// hard the tire grips toward `v/r` before saturating at the friction limit.
    pub slip_stiffness: f64,
    /// Chassis-contact grip stiffness in N·s/m: the slope of the contact
    /// friction force vs slip velocity in `apply_wheel_drive`, before it
    /// saturates at the Coulomb cone `μ·N`. Distinct from `slip_stiffness`
    /// (which is the *axle* spin model in `update_wheel_spin`). USD:
    /// `lunco:contactGripStiffness`.
    pub contact_grip_stiffness: f64,
    /// Peak brake torque about the axle in N·m. When it exceeds the available
    /// traction torque the wheel locks and skids.
    pub brake_torque_max: f64,
    /// Max per-wheel drive force as a multiple of that wheel's normal force
    /// (`throttle · N · this`). Caps traction to a fraction of contact grip.
    /// USD: `lunco:driveForcePerNormal` (default [`DEFAULT_DRIVE_FORCE_PER_NORMAL`]).
    pub drive_force_per_normal: f64,
    /// Steering rotation axis in the wheel's local frame (USD `lunco:steerAxis`).
    /// Default `+Y` (yaw) reproduces a flat-ground car steer; a motorcycle's
    /// raked steering head tilts this (e.g. `(0, cos θ, sin θ)`) so the front
    /// wheel steers about the fork axis, not vertical.
    pub steer_axis: DVec3,
}

impl Default for WheelRaycast {
    fn default() -> Self {
        Self {
            suspension_port: Entity::PLACEHOLDER,
            drive_port: Entity::PLACEHOLDER,
            steer_port: Entity::PLACEHOLDER,
            rest_length: 0.4,
            spring_k: 8000.0,
            damping_c: 2800.0,
            wheel_radius: 0.4,
            ray_origin_y: 0.0,
            visual_entity: None,
            last_normal_force: 0.0,
            spin_angle: 0.0,
            spin_velocity: 0.0,
            mass: 25.0,
            moment_of_inertia: 0.0,
            drive_torque_max: 220.0,
            bearing_damping: 2.5,
            friction_mu: 1.0,
            slip_stiffness: 8000.0,
            contact_grip_stiffness: DEFAULT_CONTACT_GRIP_STIFFNESS,
            brake_torque_max: 600.0,
            drive_force_per_normal: DEFAULT_DRIVE_FORCE_PER_NORMAL,
            steer_axis: DVec3::Y,
        }
    }
}

impl WheelRaycast {
    /// The tire's local roll about its axle as a quaternion.
    ///
    /// This is the single source of truth for the wheel's rotation — the visual
    /// mesh is rebuilt from it each tick, and any other system (telemetry,
    /// odometry, networking, a drivetrain model) can read the same orientation
    /// without inspecting the render transform. Built fresh from the wrapped
    /// `spin_angle`, so it never accumulates floating-point drift and is
    /// continuous across the 2π wrap (a 2π quaternion is identity).
    #[inline]
    pub fn spin_quat(&self) -> Quat {
        Quat::from_rotation_x(-(self.spin_angle as f32))
    }

    /// The tire's angular velocity about its axle in rad/s (signed: positive is
    /// forward roll). Real physical state — e.g. wheel-encoder odometry can
    /// integrate ground distance as `spin_velocity * wheel_radius`.
    #[inline]
    pub fn axle_angular_velocity(&self) -> f64 {
        self.spin_velocity
    }

    /// Surface (contact-patch) speed implied by the current spin, `ω · r` in m/s.
    /// Compare against chassis ground speed to recover the slip ratio.
    #[inline]
    pub fn surface_speed(&self) -> f64 {
        self.spin_velocity * self.wheel_radius
    }

    /// Rotational inertia of the tire about its axle in kg·m². Uses the
    /// USD-authored `physxVehicleWheel:moi` when set, else the solid-disk
    /// estimate `½·m·r²` from mass and radius.
    #[inline]
    pub fn axle_inertia(&self) -> f64 {
        if self.moment_of_inertia > 0.0 {
            return self.moment_of_inertia.max(1e-4);
        }
        let r = self.wheel_radius.max(1e-3);
        (0.5 * self.mass * r * r).max(1e-4)
    }
}

/// System solving the vertical suspension dynamics.
///
/// **Logic**: Performs a ray-world intersection check. If a hit is detected
/// within the suspension travel range, it applies an upward force to the
/// parent chassis based on the compression distance and relative velocity.
///
/// **Suspension model**: Spring-damper using Hooke's law:
/// `F = k * compression + c * relative_velocity`
/// Damping is bidirectional — it resists both compression and extension
/// to prevent oscillation. Force is only applied upward (along hit normal)
/// to avoid pulling the chassis into the ground.
///
/// **Geometry**: The ray origin is at the wheel entity transform (the
/// suspension mount point on the chassis). The ray points straight down.
/// `hit_distance` = distance from mount to ground. When `hit_distance <
/// rest_length` the spring is compressed. The wheel visual is positioned at
/// `ground_y + wheel_radius` so the tire rests on the terrain surface.
fn apply_wheel_suspension(
    mut q_wheels: Query<(
        &mut WheelRaycast,
        &RayHits,
        &Transform,
        &ChildOf,
    )>,
    mut q_chassis: Query<(Forces, &RigidBody), With<FlightSoftware>>,
    mut q_visual: Query<&mut Transform, (Without<WheelRaycast>, Without<FlightSoftware>)>,
) {
    for (mut wheel, hits, wheel_tf, parent) in q_wheels.iter_mut() {
        let parent_entity = parent.parent();
        if let Ok((mut forces, body)) = q_chassis.get_mut(parent_entity) {
            // A Kinematic chassis (a client's replicated proxy rover, or a body
            // mid gizmo-drag) must NOT receive the suspension spring force — its
            // pose is authoritative (snapshot-driven) and a local force would fight
            // it. But the wheel GROUND PLACEMENT + normal force below are pure
            // animation derived from the downward raycast, so they STILL run: that's
            // what lets a proxy's wheels rest on the terrain and report `on_ground`
            // to the spin model instead of floating at their authored rest offset.
            let apply_force = !matches!(body, RigidBody::Kinematic);
            let (world_pos, _) = wheel_hub_pose(
                forces.position().0,
                forces.rotation().0,
                wheel_tf.translation.as_dvec3(),
                wheel_tf.rotation.as_dquat(),
            );

            if let Some(hit) = hits.iter_sorted().next() {
                let distance = hit.distance;
                if distance < wheel.rest_length {
                    // Suspension is compressed: apply spring-damper force.
                    let compression = wheel.rest_length - distance;
                    // Damping calculation based on relative normal velocity.
                    // Positive relative_vel = wheel moving toward ground (compressing).
                    // Negative relative_vel = wheel moving away from ground (extending).
                    let ray_dir_world = forces.rotation().0 * Vec3::NEG_Y.as_dvec3();
                    let lin_vel = forces.linear_velocity();
                    let ang_vel = forces.angular_velocity();
                    let velocity_at_wheel =
                        wheel_hub_velocity(lin_vel, ang_vel, world_pos, forces.position().0);
                    let relative_vel = velocity_at_wheel.dot(ray_dir_world);

                    let total_force_mag = suspension_force_mag(
                        compression,
                        wheel.spring_k,
                        relative_vel,
                        wheel.damping_c,
                    );

                    let force_vec = hit.normal * total_force_mag;
                    if apply_force {
                        forces.apply_force_at_point(force_vec, world_pos);
                    }
                    wheel.last_normal_force = total_force_mag;

                    // Position the wheel visual on the ground.
                    //
                    // The visual is now always a CHILD of the wheel entity
                    // (see `setup_raycast_wheel` in lunco-usd-sim) — its
                    // local Y is relative to the wheel mount point, not the
                    // chassis. We want the visual centre at `ground + radius`
                    // in world space; in wheel-local space that's
                    // `wheel_radius - distance` (the suspension extension
                    // below the mount, lifted by the wheel radius).
                    if let Some(visual_entity) = wheel.visual_entity {
                        if let Ok(mut visual_tf) = q_visual.get_mut(visual_entity) {
                            visual_tf.translation.y = (wheel.wheel_radius - distance) as f32;
                        }
                    }
                } else {
                    wheel.last_normal_force = 0.0;
                }
            } else {
                wheel.last_normal_force = 0.0;
            }
        }
    }
}

/// System applying longitudinal drive torque and lateral friction.
///
/// **Theory**: Drive force is applied along the wheel's forward vector at the
/// world-space contact point. Both longitudinal (forward/back) and lateral
/// (side-to-side) friction are computed using a Coulomb friction model where
/// the maximum friction force is `mu * normal_force`. This prevents the rover
/// from sliding like it's on ice and limits drive force to what the tire can
/// actually grip.
fn apply_wheel_drive(
    q_wheels: Query<(
        &WheelRaycast,
        &Transform,
        &RayHits,
        &ChildOf,
    )>,
    q_ports: Query<&lunco_core::architecture::PhysicalPort>,
    mut q_chassis: Query<(Forces, &RigidBody, Option<&FlightSoftware>), With<FlightSoftware>>,
) {
    for (wheel, wheel_tf, hits, parent) in q_wheels.iter() {
        let parent_entity = parent.parent();
        if let Ok((mut forces, body, fsw)) = q_chassis.get_mut(parent_entity) {
            // drive-diag: the drive port the wheel reads, the body kind (Dynamic
            // vs Kinematic — the snap-back tell), and ground contact. Throttle-
            // gated so it only fires while driving. Whole block compiles out
            // (incl. the extra port read) without the `drive-diag` feature.
            drive_diag_block!({
                if let Ok(dbgport) = q_ports.get(wheel.drive_port) {
                    if dbgport.value.abs() > f32::EPSILON {
                        info!("[drive-diag] apply_wheel_drive: chassis {:?} body={:?} port.value={} normal_force={} has_contact={}",
                            parent_entity, body, dbgport.value, wheel.last_normal_force, hits.iter().next().is_some());
                    }
                }
            });
            // Skip forces if body is kinematic
            if matches!(body, RigidBody::Kinematic) { continue; }
            // Braking: the wheel-spin model locks the spin, but the chassis only
            // stops if the contact grips. We make friction saturate (full cone)
            // while braking so a locked wheel actually decelerates the rover.
            let braking = fsw.is_some_and(|f| f.brake_active);

            if let Ok(port) = q_ports.get(wheel.drive_port) {
                // Traction only exists when the ray is hitting the ground. Bind
                // the hit so its surface normal defines the contact plane (needed
                // for leaning single-track wheels).
                if let Some(ground_hit) = hits.iter().next() {
                    let normal_force = wheel.last_normal_force;
                    if normal_force < 1.0 {
                        // Not enough contact to transmit meaningful force
                        continue;
                    }

                    // Reconstruct the wheel's world pose in the AVIAN physics frame
                    // from the chassis Position/Rotation + the wheel's LOCAL transform
                    // (exactly as `apply_wheel_suspension` does). Using `GlobalTransform`
                    // here mixed the big_space floating-origin/render frame into avian's
                    // cell-local frame: `forces.apply_force_at_point` and the lever arm
                    // `hub - forces.position()` then used a point offset by the whole
                    // origin-rebasing distance, producing spurious torque/slip once the
                    // rover drove away from the floating origin (masked near it).
                    // `wheel_tf.rotation` carries the steer angle (set in
                    // `apply_wheel_steering`); roll-spin lives on the child visual, so the
                    // drive direction stays correct.
                    let (hub_pos_world, wheel_world_rot) = wheel_hub_pose(
                        forces.position().0,
                        forces.rotation().0,
                        wheel_tf.translation.as_dvec3(),
                        wheel_tf.rotation.as_dquat(),
                    );
                    // Single-track / lean support: build the traction basis in
                    // the ACTUAL contact plane (the ray hit normal), not a flat
                    // wheel basis. For an upright wheel (normal ≈ wheel up) this
                    // reproduces the old forward/right exactly, so existing rovers
                    // are unchanged; a leaning bike gets its drive + lateral grip
                    // in the tilted contact plane instead of fighting a phantom
                    // flat patch.
                    let wheel_forward: DVec3 = wheel_world_rot * DVec3::NEG_Z;
                    let wheel_right: DVec3 = wheel_world_rot * DVec3::X;
                    let (forward, right) =
                        contact_plane_basis(wheel_forward, wheel_right, ground_hit.normal);

                    // --- Drive force ---
                    // `port.value` is the wire-scaled throttle; `drive_force_mag`
                    // clamps it to [-1, 1] (negative = reverse).
                    let drive_force_vec = forward
                        * drive_force_mag(port.value as f64, normal_force, wheel.drive_force_per_normal);
                    forces.apply_force_at_point(drive_force_vec, hub_pos_world);

                    // --- Friction (longitudinal + lateral) ---
                    let max_friction = wheel.friction_mu * normal_force;
                    let chassis_vel = forces.linear_velocity();
                    let chassis_ang_vel = forces.angular_velocity();
                    let hub_vel =
                        wheel_hub_velocity(chassis_vel, chassis_ang_vel, hub_pos_world, forces.position().0);
                    let long_vel = hub_vel.dot(forward); // longitudinal slip
                    let lat_vel = hub_vel.dot(right); // lateral slip
                    let slip_vec = long_vel * forward + lat_vel * right;
                    let friction_force = contact_friction(
                        slip_vec,
                        wheel.contact_grip_stiffness,
                        max_friction,
                        braking,
                    );
                    forces.apply_force_at_point(friction_force, hub_pos_world);
                }
            }
        }
    }
}

/// Applies the steered angle to a raycast front wheel's transform. The angle
/// itself (rate-limited servo slew + Ackermann inner/outer geometry) is computed
/// by the SHARED [`lunco_hardware::SteeringActuator`] system — the exact same
/// model the physical joint wheel uses — so steering is identical across wheel
/// kinds and the logic lives in one place (DRY). This system only reads the
/// computed `output_angle` and rotates the wheel about local Y; the visual mesh
/// rotation (steer + roll spin) is composed in `update_wheel_spin`.
fn apply_wheel_steering(
    mut q_wheels: Query<(&mut Transform, &ChildOf, &lunco_hardware::SteeringActuator, &WheelRaycast)>,
    q_chassis: Query<&RigidBody, With<FlightSoftware>>,
) {
    for (mut transform, parent, steer, wheel) in q_wheels.iter_mut() {
        // Predict-own: this chain runs on a client too. Skip wheels of a
        // `Kinematic` chassis (replicated rovers this peer does NOT own), whose
        // local steer ports are stale and would point the wheels wrong.
        if let Ok(body) = q_chassis.get(parent.parent()) {
            if matches!(body, RigidBody::Kinematic) {
                continue;
            }
        }
        // Steer about the wheel's steer axis. Default `+Y` reproduces the flat
        // yaw steer; a raked motorcycle fork tilts the axis so the front wheel
        // turns about the steering head, not vertical.
        let raw = wheel.steer_axis.as_vec3();
        let axis = if raw.length_squared() > 1e-12 { raw.normalize() } else { Vec3::Y };
        transform.rotation = Quat::from_axis_angle(axis, -steer.output_angle as f32);
    }
}

// The per-arch steering components (`DifferentialDrive`, `AckermannSteer`,
// `GenericDriveMix`) are GONE. A vessel's command→actuator allocation is now the
// data-driven `lunco_core::kernels::DriveMix { kernel, ports, entries }`, whose
// `kernel` names a self-registered `ControlKernel` (`skid` / `linear` / … flight
// allocators later). `apply_drive_mix` looks the kernel up and runs it — no
// per-architecture Rust branch, no component-type taxonomy.

/// G5 — Rocker-bogie **differential** coupling primitive.
///
/// A rocker-bogie chassis hangs between two rockers (one per side), each joined
/// to the body by a lateral-axis revolute. A real rover links the two rockers
/// through a *differential* — a transverse bar or gear — so that when the left
/// rocker pitches up, the right pitches down by the same amount and the body
/// rides at their **average** pitch (keeping the payload level over rough
/// ground). Avian has no gear/differential joint, so this is a soft holonomic
/// coupling: a PD law that drives the constraint `θ_a + θ_b → rest_sum` with
/// equal/opposite corrective torques about the hinge axis (reaction on the
/// chassis). Everything *else* in a rocker-bogie (the rocker/bogie links) is
/// already buildable with today's authored `PhysicsRevoluteJoint`s — this fills
/// the one missing piece. Stiff but compliant: the body still conforms, it just
/// can't simply fold one rocker flat while the other stays put.
///
/// Author from USD on the chassis prim (e.g. `PhysxVehicleDifferentialAPI` /
/// `lunco:differential*` → this component); inert until present, so existing
/// vehicles are unaffected.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct DifferentialCoupling {
    /// The chassis body the two rockers pivot against (reaction torque target).
    pub chassis: Entity,
    /// Left rocker body (hinged to the chassis about `axis`).
    pub rocker_a: Entity,
    /// Right rocker body.
    pub rocker_b: Entity,
    /// Hinge axis in the **chassis local** frame (typically lateral, ±X).
    pub axis: DVec3,
    /// Target for `θ_a + θ_b` (rad). Zero ⇒ symmetric (mirror) rockers.
    pub rest_sum: f64,
    /// Coupling stiffness (N·m per rad of constraint error).
    pub stiffness: f64,
    /// Coupling damping (N·m per rad/s of constraint-error rate).
    pub damping: f64,
}

impl Default for DifferentialCoupling {
    fn default() -> Self {
        Self {
            chassis: Entity::PLACEHOLDER,
            rocker_a: Entity::PLACEHOLDER,
            rocker_b: Entity::PLACEHOLDER,
            axis: DVec3::X,
            rest_sum: 0.0,
            stiffness: 200_000.0,
            damping: 20_000.0,
        }
    }
}

/// Signed rotation angle (rad) of a relative quaternion about `axis` — the twist
/// component of a swing-twist decomposition. For a pure rotation `θ` about a unit
/// `axis`, `q = (cos θ/2, sin θ/2 · axis)` and this returns `θ`, wrapped to
/// `(-π, π]`. Used to read each rocker's pitch in the chassis frame.
fn angle_about_axis(rel: DQuat, axis: DVec3) -> f64 {
    let a = axis.normalize_or_zero();
    let proj = DVec3::new(rel.x, rel.y, rel.z).dot(a);
    let mut angle = 2.0 * proj.atan2(rel.w);
    // 2·atan2 lands in (−2π, 2π); fold to (−π, π].
    if angle > std::f64::consts::PI {
        angle -= std::f64::consts::TAU;
    } else if angle <= -std::f64::consts::PI {
        angle += std::f64::consts::TAU;
    }
    angle
}

/// PD corrective torque (N·m, about the hinge axis) for the differential
/// constraint `c = θ_a + θ_b − rest_sum`. Applied **identically** to each rocker
/// (∂c/∂θ each = 1); the chassis takes `−2·τ` as reaction. `rate_sum` is
/// `ċ = (ω_a + ω_b − 2·ω_c)·axis`.
fn differential_torque(
    angle_a: f64,
    angle_b: f64,
    rate_sum: f64,
    rest_sum: f64,
    stiffness: f64,
    damping: f64,
) -> f64 {
    let c = angle_a + angle_b - rest_sum;
    -(stiffness * c + damping * rate_sum)
}

/// Enforces every [`DifferentialCoupling`] each fixed step. Reads the two
/// rockers' pitch + rate relative to the chassis and applies the PD coupling
/// torque about the hinge axis (equal on each rocker, `−2τ` reaction on the
/// chassis). Idle unless a `DifferentialCoupling` exists.
///
/// **Verified** on an isolated rig (`differential_rig_test.usda`, 2026-06-30):
/// a fixed base carries a front-heavy rocker A and a balanced rocker B on lateral
/// revolutes. A/B by hinge `angle` ports —
/// - coupling OFF: A free-falls to the pendulum bottom (`+3.06`), B untouched (`+0.06`);
/// - coupling ON:  A held at `+1.72`, B driven to `−1.65` (mirror), `θ_A+θ_B ≈ 0.07`.
///
/// So the coupling correctly enforces `θ_A + θ_B → rest_sum`. NOTE: needs a
/// non-redundant rig to *show* its effect — a passive two-rocker pair each pinned
/// by its own two ground feet already self-levels, leaving nothing for the
/// coupling to do (the original `rocker_bogie_test.usda` is that redundant case).
/// And keep `stiffness < I/dt²` and damp the rockers, or the explicit penalty
/// rings / diverges.
fn differential_coupling_system(
    q_coupling: Query<&DifferentialCoupling>,
    mut q_bodies: Query<Forces>,
) {
    for coupling in q_coupling.iter() {
        let Ok([chassis, mut a, mut b]) =
            q_bodies.get_many_mut([coupling.chassis, coupling.rocker_a, coupling.rocker_b])
        else {
            continue;
        };
        let rot_c = chassis.rotation().0;
        let axis_world = (rot_c * coupling.axis).normalize_or_zero();
        // Rocker pitch in the chassis frame (twist about the hinge axis).
        let angle_a = angle_about_axis(rot_c.inverse() * a.rotation().0, coupling.axis);
        let angle_b = angle_about_axis(rot_c.inverse() * b.rotation().0, coupling.axis);
        // ċ = (ω_a + ω_b − 2·ω_c) · axis_world.
        let w_c = chassis.angular_velocity();
        let rate_sum = (a.angular_velocity() + b.angular_velocity() - 2.0 * w_c).dot(axis_world);
        let tau = differential_torque(
            angle_a,
            angle_b,
            rate_sum,
            coupling.rest_sum,
            coupling.stiffness,
            coupling.damping,
        );
        if !tau.is_finite() {
            continue;
        }
        let torque = axis_world * tau;
        a.apply_torque(torque);
        b.apply_torque(torque);
        // Reaction keeps the system's angular momentum conserved.
        let mut chassis = chassis;
        chassis.apply_torque(-2.0 * torque);
    }
}

/// Suspension configuration for joint-based (non-raycast) chassis.
///
/// **Why**: Some vehicles use physical collision wheels for higher fidelity,
/// but still require emulated spring-damper logic for PrismaticJoints.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct Suspension {
    /// target static length of the strut.
    pub rest_length: f64,
    /// Stiffness (N/m).
    pub spring_k: f64,
    /// Dampening (Ns/m).
    pub damping_c: f64,
    /// Direction of extension.
    pub local_axis: DVec3,
}

impl Default for Suspension {
    fn default() -> Self {
        Self {
            rest_length: 0.4,
            spring_k: 50000.0,
            damping_c: 2000.0,
            local_axis: DVec3::Y,
        }
    }
}

/// Solves linear suspension equations for entities linked by joints.
///
/// **Model**: Spring-damper using Hooke's law applied along the prismatic
/// joint's slider axis. Damping is bidirectional — it resists both compression
/// and extension to prevent oscillation. The force is applied as an equal and
/// opposite pair on the two connected bodies.
fn suspension_system(
    q_joints: Query<(&PrismaticJoint, &Suspension)>,
    mut q_bodies: Query<Forces>,
) {
    for (joint, susp) in q_joints.iter() {
        let e1 = joint.body1;
        let e2 = joint.body2;

        if let Ok([mut forces1, mut forces2]) = q_bodies.get_many_mut([e1, e2]) {
            let pos1 = forces1.position().0;
            let rot1 = forces1.rotation().0;
            let pos2 = forces2.position().0;
            let rot2 = forces2.rotation().0;

            let world_axis: DVec3 = rot1 * susp.local_axis;

            let anchor1_world: DVec3 = pos1 + rot1 * joint.local_anchor1().unwrap_or_default();
            let anchor2_world: DVec3 = pos2 + rot2 * joint.local_anchor2().unwrap_or_default();

            let diff_world: DVec3 = anchor2_world - anchor1_world;
            let current_length: f64 = -diff_world.dot(world_axis);
            let vel1 = forces1.velocity_at_point(anchor1_world);
            let vel2 = forces2.velocity_at_point(anchor2_world);
            let rel_vel: f64 = (vel2 - vel1).dot(world_axis);

            let compression: f64 = (susp.rest_length - current_length).max(0.0);
            let spring_force_mag: f64 = compression * susp.spring_k;

            // Damping opposes relative motion: positive when compressing (adds
            // force), negative when extending (reduces force). Clamp total to
            // zero minimum so we never pull bodies together.
            let damping_force_mag: f64 = rel_vel * susp.damping_c;
            let total_force_mag: f64 = (spring_force_mag + damping_force_mag).clamp(0.0, MAX_SUSPENSION_FORCE_N);

            if !total_force_mag.is_finite() { continue; }

            let force_vec: DVec3 = world_axis * total_force_mag;

            forces1.apply_force_at_point(force_vec, anchor1_world);
            forces2.apply_force_at_point(-force_vec, anchor2_world);
        }
    }
}

// ── Drive command ports ─────────────────────────────────────────────────────────

/// Port backend exposing a [`FlightSoftware`]'s `inputs` map as writable **input**
/// ports on any entity carrying the component — the single command sink for every
/// controllable (rover `throttle`/`steer`/`brake`, avatar `forward`/`side`/`up`, a
/// lander's `throttle`/`pitch`/`roll`/`yaw`, …). Reachable by `SetPorts`/wires/API/
/// scripts; the vehicle's actuator ([`apply_drive_mix`], `apply_fly`, a Modelica
/// bridge) reads its own vocabulary back out.
///
/// Strict: only keys the vehicle *seeded* into `inputs` (its declared command
/// surface — see [`FlightSoftware::new`]) are writable, so an undeclared command
/// name is rejected and still surfaces as a dangling wire. Replaces the old
/// per-class `DriveCommand` component — command state now has one home on the FSW.
const FSW_COMMAND_BACKEND: PortBackend = PortBackend {
    list: |w, e, out| {
        if let Some(fsw) = w.get::<FlightSoftware>(e) {
            for (name, value) in &fsw.inputs {
                out.push(PortRef {
                    name: name.clone(),
                    direction: PortDirection::In,
                    port_type: PortType::Signal,
                    value: *value,
                });
            }
        }
    },
    read_output: |_w, _e, _n| None,
    read_input: |w, e, n| w.get::<FlightSoftware>(e).and_then(|f| f.inputs.get(n).copied()),
    write_input: |w, e, n, v| {
        if let Some(mut f) = w.get_mut::<FlightSoftware>(e) {
            if let Some(slot) = f.inputs.get_mut(n) {
                *slot = v;
                return true;
            }
        }
        false
    },
    // Map-backed: name-based write is one `get::<FlightSoftware>` + a map lookup.
    // A resolve→slot fast path here would need a name interner (the slot can't
    // carry the string) — a documented follow-up if the drive-command write fold
    // shows up in profiling, not needed for correctness.
    resolve_output: None,
    resolve_input: None,
    read_slot: None,
    write_slot: None,
};

/// Derive each controllable's FSW command surface from USD: for any entity that has
/// both a [`FlightSoftware`] and a [`lunco_core::ControlBinding`], ensure every port
/// the binding targets exists in `FlightSoftware.inputs` (seeded `0.0`).
///
/// This is what lets the command vocabulary be **data, not a Rust literal**: a
/// vessel's `Controls` profile (→ its `ControlBinding`) declares exactly which
/// command ports it accepts, and the strict FSW backend then admits writes to those
/// and no others. Additive (never removes keys) and idempotent, so it's safe to run
/// on `Changed<ControlBinding>` regardless of which reader stamped the binding or
/// the FSW, and regardless of spawn order.
fn sync_fsw_command_surface(
    mut q: Query<(&lunco_core::ControlBinding, &mut FlightSoftware), Changed<lunco_core::ControlBinding>>,
) {
    for (binding, mut fsw) in q.iter_mut() {
        for port in binding.ports() {
            if !fsw.inputs.contains_key(port) {
                fsw.inputs.insert(port.to_string(), 0.0);
            }
        }
    }
}

// ── Drive mix ─────────────────────────────────────────────────────────────────

/// System allocating each rover's FSW command inputs (`throttle`/`steer`/`brake`,
/// read from [`FlightSoftware::inputs`]) to its actuator [`DigitalPort`]s, via the
/// vessel's data-selected [`DriveMix`] kernel (`skid`/`linear`/…, looked up in the
/// [`ControlKernelRegistry`]). No per-architecture branch: the kernel is chosen by
/// USD, its normalized `[-1,1]` outputs are scaled to the i16 port range here. Runs
/// every fixed tick before the DAC.
fn apply_drive_mix(
    mut q: Query<(Entity, &mut FlightSoftware, &DriveMix)>,
    registry: Res<ControlKernelRegistry>,
    mut q_ports: Query<&mut DigitalPort>,
    mut unknown: Local<std::collections::HashSet<String>>,
) {
    let full = DIGITAL_PORT_FULL_SCALE as f64;
    for (entity, mut fsw, mix) in q.iter_mut() {
        // Read this vehicle's logical command inputs off the FSW command surface.
        let throttle = fsw.cmd("throttle");
        let steer = fsw.cmd("steer");
        let brake = fsw.cmd("brake");
        // Brake state (old `on_brake_rover`): engaged above half-scale. Locks the
        // wheel-spin/friction cone in the physics systems via `fsw.brake_active`.
        fsw.brake_active = brake > 0.5;
        let brake_port_val = if fsw.brake_active { DIGITAL_PORT_FULL_SCALE } else { 0 };
        if let Some(&port_b) = fsw.port_map.get("brake") {
            if let Ok(mut p) = q_ports.get_mut(port_b) { p.raw_value = brake_port_val; }
        }

        drive_diag!("[drive-diag] apply_drive_mix: target {:?} kernel={} throttle={} steer={} brake={} ports={:?}", entity, mix.kernel, throttle, steer, fsw.brake_active, fsw.port_map);

        // While braking, force throttle/steer to 0 and drive the brake gate (1.0)
        // so brake-coefficient ports engage and drive ports zero out — matching the
        // old per-branch behaviour, now uniform across kernels.
        let inputs = if fsw.brake_active {
            lunco_core::kernels::DriveInputs { throttle: 0.0, steer: 0.0, brake: 1.0 }
        } else {
            lunco_core::kernels::DriveInputs { throttle, steer, brake: 0.0 }
        };

        // Allocate command → normalized port writes. A built-in registry kernel
        // (`skid`/`linear`/…) wins; otherwise `mix.kernel` names a scripted (rhai)
        // drive kernel — a `lunco_hooks` hook that computes the per-port outputs
        // itself ("control policy in rhai", `lunco:driveKernel`). An unknown name
        // with no matching hook leaves the vessel un-actuated (fail-safe coast).
        let outputs = match registry.get(&mix.kernel) {
            Some(kernel) => kernel(inputs, mix),
            None => {
                // Scripted kernel: hand the hook the vessel's real command surface
                // (`fsw.inputs`, un-gated — the script owns its brake policy), not the
                // built-in kernels' fixed throttle/steer/brake projection.
                let scripted = scripted_drive_mix(&mix.kernel, &fsw.inputs);
                if scripted.is_empty() && unknown.insert(mix.kernel.clone()) {
                    warn!("[apply_drive_mix] unknown drive kernel '{}' on {:?} — no built-in and no rhai hook; vessel not actuated", mix.kernel, entity);
                }
                scripted
            }
        };

        for (port, value) in outputs {
            if let Some(&port_id) = fsw.port_map.get(&port) {
                if let Ok(mut p) = q_ports.get_mut(port_id) {
                    p.raw_value = (value * full).round().clamp(-full, full) as i16;
                }
            }
        }
    }
}

/// Invoke a **scripted (rhai) drive kernel** by hook id. Hands the hook the vessel's
/// **actual command surface** — its declared [`FlightSoftware::inputs`] map, keyed by
/// whatever ports that vehicle accepts (a rover's `throttle`/`steer`/`brake`, a
/// lander's `throttle`/`pitch`/`roll`/`yaw`, …) — NOT a fixed Rust key set. The
/// command vocabulary is data, so a scripted kernel reads exactly the ports the
/// vessel exposes and the script owns its own policy (incl. how `brake` gates).
/// Reads back a `port → value` map in `[-1, 1]` (clamped defensively). Empty on an
/// absent or faulted hook: **fail-safe** coast (ports left untouched — the brake
/// port + `brake_active` friction cone are already applied upstream regardless).
/// Host-side; a predicted client needs the identical hook, so the scripted-policy
/// plane (`lunco_networking`) distributes + registers it on every peer.
fn scripted_drive_mix(hook_id: &str, inputs: &std::collections::HashMap<String, f64>) -> Vec<(String, f64)> {
    use lunco_hooks::HookValue;
    let ctx = HookValue::map(inputs.iter().map(|(k, v)| (k.clone(), HookValue::Float(*v))));
    match lunco_hooks::invoke(hook_id, &[ctx]) {
        Some(Ok(HookValue::Map(entries))) => entries
            .into_iter()
            .filter_map(|(k, v)| v.as_f64().map(|f| (k, f.clamp(-1.0, 1.0))))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod force_law_tests {
    //! Regression guards for the numerically-sensitive wheel force laws. Each
    //! test pins a property whose violation previously caused a jitter or a
    //! broken control (the comments name the bug).
    use super::*;
    use bevy::math::{DQuat, DVec3};

    // ── Single-track lean: contact-plane traction basis ─────────────────────
    #[test]
    fn contact_basis_upright_matches_flat_wheel() {
        // Upright wheel: contact normal = world up. Basis must equal the raw
        // wheel forward/right (so existing rovers are unchanged).
        let (f, r) = contact_plane_basis(DVec3::NEG_Z, DVec3::X, DVec3::Y);
        assert!((f - DVec3::NEG_Z).length() < 1e-9, "forward changed: {f:?}");
        assert!((r - DVec3::X).length() < 1e-9, "right changed: {r:?}");
    }

    #[test]
    fn contact_basis_leaned_lies_in_contact_plane() {
        // Cambered contact: normal tilted 22° off vertical. Both basis vectors
        // must lie in the plane ⟂ to the normal, stay unit, and be orthogonal.
        let n = DVec3::new(0.0, 1.0, 0.4).normalize();
        let (f, r) = contact_plane_basis(DVec3::NEG_Z, DVec3::X, n);
        assert!(f.dot(n).abs() < 1e-9, "forward not in contact plane: {}", f.dot(n));
        assert!(r.dot(n).abs() < 1e-9, "right not in contact plane: {}", r.dot(n));
        assert!((f.length() - 1.0).abs() < 1e-9 && (r.length() - 1.0).abs() < 1e-9);
        assert!(f.dot(r).abs() < 1e-9, "forward/right not orthogonal");
    }

    // ── G5 differential coupling: twist angle + PD law ──────────────────────
    #[test]
    fn angle_about_axis_reads_pure_twist() {
        let axis = DVec3::X;
        // A +0.3 rad rotation about X reads back as +0.3.
        let q = DQuat::from_axis_angle(DVec3::X, 0.3);
        assert!((angle_about_axis(q, axis) - 0.3).abs() < 1e-9);
        // Sign flips with rotation direction.
        let q_neg = DQuat::from_axis_angle(DVec3::X, -0.3);
        assert!((angle_about_axis(q_neg, axis) + 0.3).abs() < 1e-9);
        // Identity ⇒ zero pitch.
        assert!(angle_about_axis(DQuat::IDENTITY, axis).abs() < 1e-12);
    }

    #[test]
    fn differential_restores_and_opposes_motion() {
        let (k, d) = (1000.0, 100.0);
        // Symmetric rockers at rest_sum=0, no motion ⇒ no torque.
        assert!(differential_torque(0.0, 0.0, 0.0, 0.0, k, d).abs() < 1e-12);
        // Both rockers pitched the SAME way (sum > 0) ⇒ restoring torque is
        // NEGATIVE (drives the sum back toward zero / the average pitch).
        assert!(differential_torque(0.2, 0.2, 0.0, 0.0, k, d) < 0.0);
        // MIRRORED rockers (a up, b down) satisfy the constraint ⇒ no torque,
        // which is exactly the differential letting the body conform.
        assert!(differential_torque(0.2, -0.2, 0.0, 0.0, k, d).abs() < 1e-12);
        // Damping opposes a positive constraint-rate even at zero error.
        assert!(differential_torque(0.0, 0.0, 0.5, 0.0, k, d) < 0.0);
    }

    // (drive-mix parse + kernel projection now live in `lunco_core::kernels`.)

    // ── scripted (rhai) drive kernel: hook-driven mixing, by DriveMix.kernel id ──
    #[test]
    fn scripted_drive_mix_maps_command_to_ports() {
        use lunco_hooks::{HookResult, HookValue, RegisteredHook, ScriptHook};
        use std::collections::HashMap;
        use std::sync::Arc;

        // A native stand-in for a rhai kernel: tank mix over the vessel's OWN command
        // ports (a rover exposes `throttle`/`steer`) — left=t+s, right=t-s.
        struct TankKernel;
        impl ScriptHook for TankKernel {
            fn invoke(&self, args: &[HookValue]) -> HookResult {
                let t = args[0].get("throttle").and_then(HookValue::as_f64).unwrap_or(0.0);
                let s = args[0].get("steer").and_then(HookValue::as_f64).unwrap_or(0.0);
                Ok(HookValue::map([
                    ("drive_left", HookValue::Float((t + s).clamp(-1.0, 1.0))),
                    ("drive_right", HookValue::Float((t - s).clamp(-1.0, 1.0))),
                ]))
            }
        }
        lunco_hooks::register(RegisteredHook {
            id: "test.kernel.tank".into(),
            backend: "rust".into(),
            deterministic: true,
            hook: Arc::new(TankKernel),
        });

        let inputs: HashMap<String, f64> =
            [("throttle".into(), 1.0), ("steer".into(), 0.5), ("brake".into(), 0.0)].into();
        let mut out = scripted_drive_mix("test.kernel.tank", &inputs);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        // t+s = 1.5 → clamped to 1.0; t-s = 0.5.
        assert_eq!(
            out,
            vec![("drive_left".to_string(), 1.0), ("drive_right".to_string(), 0.5)]
        );

        // Absent hook → empty (fail-safe coast; ports left untouched).
        assert!(scripted_drive_mix("test.kernel.absent", &inputs).is_empty());

        lunco_hooks::unregister("test.kernel.tank");
    }

    // ── drive_force_mag: reverse must work ──────────────────────────────────
    #[test]
    fn drive_supports_forward_and_reverse() {
        let n = 1000.0;
        let f = DEFAULT_DRIVE_FORCE_PER_NORMAL;
        assert!(drive_force_mag(0.5, n, f) > 0.0, "forward drives forward");
        // REGRESSION: reverse used to be clamped to 0 (`clamp(0.0, 1.0)`).
        assert!(drive_force_mag(-0.5, n, f) < 0.0, "negative throttle = reverse");
        assert!((drive_force_mag(0.5, n, f) + drive_force_mag(-0.5, n, f)).abs() < 1e-9);
        // throttle clamps to [-1, 1]
        assert_eq!(drive_force_mag(5.0, n, f), drive_force_mag(1.0, n, f));
        assert_eq!(drive_force_mag(-5.0, n, f), drive_force_mag(-1.0, n, f));
        assert_eq!(drive_force_mag(0.0, n, f), 0.0);
    }

    // ── contact_friction: continuous through zero, saturating, brake grips ───
    #[test]
    fn friction_zero_at_rest_and_continuous_through_zero() {
        // REGRESSION: a slip dead-band left sub-threshold motion undamped → a
        // stiction limit-cycle (the steering jitter). Friction must be exactly
        // zero at zero slip AND shrink smoothly toward it (no cliff).
        let (k, max) = (50.0, 1e9); // huge cone → always linear
        assert_eq!(contact_friction(DVec3::ZERO, k, max, false), DVec3::ZERO);
        let big = contact_friction(DVec3::new(1e-3, 0.0, 0.0), k, max, false);
        let small = contact_friction(DVec3::new(1e-4, 0.0, 0.0), k, max, false);
        assert!((big - DVec3::new(1e-3, 0.0, 0.0) * -k).length() < 1e-12);
        assert!(small.length() < big.length(), "shrinks continuously toward 0");
    }

    #[test]
    fn friction_opposes_slip_saturates_and_is_continuous_at_boundary() {
        let (k, max) = (50.0, 100.0);
        let slip = DVec3::new(10.0, 0.0, 0.0); // k*slip = 500 > 100 → saturated
        let f = contact_friction(slip, k, max, false);
        assert!((f.length() - max).abs() < 1e-9, "saturates at the cone");
        assert!(f.dot(slip) < 0.0, "opposes slip");
        // both branches agree exactly at the linear/saturation boundary
        let boundary = DVec3::new(max / k, 0.0, 0.0);
        let fb = contact_friction(boundary, k, max, false);
        assert!((fb - boundary * -k).length() < 1e-9);
        assert!((fb.length() - max).abs() < 1e-9);
    }

    #[test]
    fn braking_grips_at_full_cone() {
        // REGRESSION: braking only zeroed the drive ports; the chassis coasted.
        // A locked wheel must grip at the full cone even when normal grip is weak.
        let (k, max) = (50.0, 100.0);
        let slip = DVec3::new(0.1, 0.0, 0.0); // k*slip = 5 ≪ 100 → linear when coasting
        let coast = contact_friction(slip, k, max, false);
        let brake = contact_friction(slip, k, max, true);
        assert!(brake.length() > coast.length(), "brake grips harder");
        assert!((brake.length() - max).abs() < 1e-9, "full cone");
    }

    // ── suspension_force_mag: bounded, never negative, damps both ways ───────
    #[test]
    fn suspension_force_is_nonnegative_and_bounded() {
        let (k, c) = (8000.0, 2800.0);
        let x = 0.05;
        let spring = x * k;
        assert!(suspension_force_mag(x, k, -1000.0, c) >= 0.0, "ground can't pull");
        assert!(suspension_force_mag(x, k, 1000.0, c) <= 2.0 * spring + 1e-9, "bounded");
        assert!((suspension_force_mag(x, k, 0.0, c) - spring).abs() < 1e-9, "at rest = spring");
    }

    #[test]
    fn suspension_damps_the_rebound_half_cycle() {
        // REGRESSION: `(spring + c·v).max(0)` dropped ALL damping on fast rebound
        // → an undamped suspension limit-cycle (the forward+turn jitter). A
        // moderate rebound must still be damped (force below spring), not clamped.
        let (k, c) = (8000.0, 2800.0);
        let x = 0.1;
        let spring = x * k; // 800
        let f = suspension_force_mag(x, k, -0.1, c); // c·v = -280, within ±spring
        assert!(f < spring, "rebound is damped");
        assert!(f > 0.0, "not clamped to zero");
        assert!((f - (spring - 280.0)).abs() < 1e-9);
    }
}

/// Step-2 **oracle**: validate the production suspension force law against a
/// continuous, proper-solver reference — a quarter-car (one sprung mass on one
/// spring-damper strut over ground). The continuous physics is stated
/// declaratively in `assets/models/QuarterCar.mo`; an adaptive Modelica solver
/// integrates it as ground truth. Here we integrate the *same* equations with a
/// fine RK4 step (≈ the Modelica answer to many digits for this non-stiff system)
/// and compare against the real `suspension_force_mag`, stepped with the
/// production scheme (semi-implicit Euler at dt = 1/60). See
/// `docs/architecture/28-modelica-realtime-physics.md` §8 (Step 2).
///
/// What it establishes:
/// 1. **Physics + integration agree** in the gentle regime (the clamp inactive) —
///    the Rust law tracks the continuous reference to sub-cm.
/// 2. **The fixed law settles** — no sustained limit-cycle (the dead-band / `.max(0)`
///    bugs would ring forever).
/// 3. **The bound is the fix** — on a hard landing the production law caps the
///    force at `2·k·χ`, while the old `.max(0)` cliff lets it spike (the 27 kN-class
///    transient the jitter work removed). The oracle is sensitive to that exact
///    regression.
#[cfg(test)]
mod oracle {
    use super::*;

    // Quarter-car: m·χ̈ = m·g − F(χ, χ̇). χ = compression (m), χ̇ = compression rate.
    const M: f64 = 250.0; // sprung mass per wheel — quarter of a 1000 kg chassis
    const G: f64 = 9.81;
    const DT_SIM: f64 = 1.0 / 60.0; // the real FixedUpdate step
    const DT_FINE: f64 = 1.0e-4; // RK4 reference step (≈ the adaptive-solver answer)

    /// The continuous physics the Rust law approximates (QuarterCar.mo): ideal
    /// linear spring-damper, no clamp. Zero force when out of contact (χ ≤ 0).
    fn reference_force(chi: f64, chi_dot: f64, k: f64, c: f64) -> f64 {
        if chi > 0.0 { k * chi + c * chi_dot } else { 0.0 }
    }

    /// Integrate the reference with RK4 at a fine step → ground truth trajectory of
    /// compression χ. State = (χ, v=χ̇); χ′ = v, v′ = g − F/m.
    fn reference_chi(k: f64, c: f64, chi0: f64, v0: f64, secs: f64) -> Vec<f64> {
        let n = (secs / DT_FINE) as usize;
        let d = |chi: f64, v: f64| (v, G - reference_force(chi, v, k, c) / M);
        let (mut chi, mut v) = (chi0, v0);
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let (a1, b1) = d(chi, v);
            let (a2, b2) = d(chi + 0.5 * DT_FINE * a1, v + 0.5 * DT_FINE * b1);
            let (a3, b3) = d(chi + 0.5 * DT_FINE * a2, v + 0.5 * DT_FINE * b2);
            let (a4, b4) = d(chi + DT_FINE * a3, v + DT_FINE * b3);
            chi += DT_FINE / 6.0 * (a1 + 2.0 * a2 + 2.0 * a3 + a4);
            v += DT_FINE / 6.0 * (b1 + 2.0 * b2 + 2.0 * b3 + b4);
            out.push(chi);
        }
        out
    }

    /// Integrate a force law with the PRODUCTION scheme — semi-implicit Euler at
    /// dt = 1/60, exactly as `apply_wheel_suspension` runs. Returns the per-step
    /// compression and the applied force, so callers can probe both trajectory and
    /// force transients.
    fn step_law<F: Fn(f64, f64) -> f64>(
        force: F,
        chi0: f64,
        v0: f64,
        secs: f64,
    ) -> (Vec<f64>, Vec<f64>) {
        let n = (secs / DT_SIM) as usize;
        let (mut chi, mut v) = (chi0, v0);
        let (mut chis, mut forces) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for _ in 0..n {
            let f = if chi > 0.0 { force(chi, v) } else { 0.0 };
            v += DT_SIM * (G - f / M); // semi-implicit: velocity first…
            chi += DT_SIM * v; //          …then position with the new velocity
            chis.push(chi);
            forces.push(f);
        }
        (chis, forces)
    }

    // The production law and the OLD buggy `.max(0)` cliff it replaced.
    fn fixed(k: f64, c: f64) -> impl Fn(f64, f64) -> f64 {
        move |chi, v| suspension_force_mag(chi, k, v, c)
    }
    fn buggy(k: f64, c: f64) -> impl Fn(f64, f64) -> f64 {
        move |chi, v| (chi * k + v * c).max(0.0) // clamps the TOTAL, not the damping term
    }

    fn max_abs_dev(a: &[f64], b_fine: &[f64]) -> f64 {
        // a is sampled at DT_SIM, b_fine at DT_FINE; compare at matching times.
        let ratio = (DT_SIM / DT_FINE).round() as usize;
        a.iter()
            .enumerate()
            .map(|(i, &x)| {
                let j = ((i + 1) * ratio - 1).min(b_fine.len() - 1);
                (x - b_fine[j]).abs()
            })
            .fold(0.0_f64, f64::max)
    }

    #[test]
    fn fixed_law_tracks_the_continuous_reference_in_the_gentle_regime() {
        // Production params, a soft settle from below equilibrium (χ_eq = 0.3066 m).
        // The clamp never engages, so the Rust law IS the continuous physics and the
        // only gap is fixed-step integration error — must stay sub-cm.
        let (k, c) = (8000.0, 2800.0);
        let (rust, _f) = step_law(fixed(k, c), 0.20, 0.0, 3.0);
        let reference = reference_chi(k, c, 0.20, 0.0, 3.0);
        let dev = max_abs_dev(&rust, &reference);
        let chi_eq = M * G / k;
        println!("[oracle] gentle: max|χ_rust−χ_ref| = {dev:.5} m, χ_end = {:.4} (eq {chi_eq:.4})",
            rust.last().unwrap());
        assert!(dev < 8.0e-3, "fixed law diverges from continuous reference: {dev} m");
        assert!((rust.last().unwrap() - chi_eq).abs() < 2.0e-3, "must settle at m·g/k");
    }

    #[test]
    fn fixed_law_settles_no_limit_cycle() {
        // Under-damped config (c small → clear ringing) must still DECAY. The
        // dead-band / `.max(0)` bugs produced a sustained tick-period limit-cycle;
        // assert the late window is quiet relative to the early one.
        let (k, c) = (8000.0, 400.0);
        let (rust, _f) = step_law(fixed(k, c), 0.15, 0.0, 5.0);
        let win = rust.len() / 5;
        let p2p = |s: &[f64]| s.iter().cloned().fold(f64::MIN, f64::max)
            - s.iter().cloned().fold(f64::MAX, f64::min);
        let early = p2p(&rust[..win]);
        let late = p2p(&rust[rust.len() - win..]);
        println!("[oracle] settle: early p2p {early:.4} m, late p2p {late:.5} m");
        assert!(late < 0.15 * early, "ringing must decay (limit-cycle guard): {late} vs {early}");
    }

    #[test]
    fn bounded_law_caps_the_landing_spike_the_cliff_let_through() {
        // Hard landing: χ starts at 0 with a fast downward (compressing) velocity.
        // The continuous force AND the old `.max(0)` law spike to ≈ c·v at impact
        // (the 27 kN-class transient); the production law bounds it to 2·k·χ. This
        // is the design trade — fidelity for fixed-step stability — and the property
        // the oracle guards.
        let (k, c) = (8000.0, 2800.0);
        let v_impact = 12.0;
        let (chi_fixed, f_fixed) = step_law(fixed(k, c), 0.0, v_impact, 0.5);
        let (_chi_buggy, f_buggy) = step_law(buggy(k, c), 0.0, v_impact, 0.5);
        // The impact tick: step 0 applies zero force for both laws (χ starts at 0),
        // so by step 1 both see the SAME state (χ = chi_fixed[0], same fast v). The
        // force difference there is purely the law — the cleanest contrast.
        let (chi_at_impact, sf, sb) = (chi_fixed[0], f_fixed[1], f_buggy[1]);
        let bound = 2.0 * k * chi_at_impact;
        println!("[oracle] impact tick (χ = {chi_at_impact:.3} m): fixed {sf:.0} N (≤ 2·k·χ = {bound:.0}), cliff {sb:.0} N");
        // The production law obeys its bound; the cliff passes the full c·v spike.
        assert!(sf <= bound + 1.0, "fixed law must stay within 2·k·χ");
        assert!(sb > 3.0 * sf, "cliff spikes the impact ({sb} N) far past the bounded force ({sf} N)");
        assert!(sb > 20_000.0, "cliff lets a >20 kN landing transient through");
    }

    // ── Longitudinal 1-DOF: m·v̇ = F_long(v) — friction + drive validation ──────
    //
    // A block / chassis on flat ground. Friction (`contact_friction`) opposes its
    // velocity; drive (`drive_force_mag`) pushes it. The continuous reference and
    // the production law share the SAME force law, so the gap is integration only —
    // except for the dead-band contrast, which is the old stiction bug.
    use bevy::math::DVec3;

    const N_NORMAL: f64 = M * G; // contact normal force = weight (2452.5 N)

    /// RK4 fine-step reference for `m·v̇ = net(v)` → ground-truth velocity trace.
    fn long_reference<F: Fn(f64) -> f64>(net: F, v0: f64, secs: f64) -> Vec<f64> {
        let n = (secs / DT_FINE) as usize;
        let d = |v: f64| net(v) / M;
        let mut v = v0;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let (k1, k2, k3, k4);
            k1 = d(v);
            k2 = d(v + 0.5 * DT_FINE * k1);
            k3 = d(v + 0.5 * DT_FINE * k2);
            k4 = d(v + DT_FINE * k3);
            v += DT_FINE / 6.0 * (k1 + 2.0 * k2 + 2.0 * k3 + k4);
            out.push(v);
        }
        out
    }

    /// Production scheme: explicit velocity step at dt = 1/60 (how the chassis
    /// velocity advances under the per-tick contact force).
    fn long_step<F: Fn(f64) -> f64>(net: F, v0: f64, secs: f64) -> Vec<f64> {
        let n = (secs / DT_SIM) as usize;
        let mut v = v0;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            v += DT_SIM * net(v) / M;
            out.push(v);
        }
        out
    }

    fn sign_flips(s: &[f64]) -> usize {
        s.windows(2).filter(|w| w[0] * w[1] < 0.0).count()
    }

    /// Longitudinal component of the production contact friction at speed `v`.
    fn fric_x(k: f64, mu_n: f64, braking: bool) -> impl Fn(f64) -> f64 {
        move |v| contact_friction(DVec3::new(v, 0.0, 0.0), k, mu_n, braking).x
    }
    /// The OLD dead-band friction: a constant Coulomb force outside a slip
    /// dead-band, nothing inside it — the stiction limit-cycle (steering jitter).
    fn fric_deadband(mu_n: f64) -> impl Fn(f64) -> f64 {
        move |v| if v.abs() > 1e-3 { -mu_n * v.signum() } else { 0.0 }
    }
    /// Drive minus contact friction — the longitudinal net force under throttle.
    fn drive_minus_friction(throttle: f64, k: f64, mu_n: f64) -> impl Fn(f64) -> f64 {
        move |v| drive_force_mag(throttle, N_NORMAL, DEFAULT_DRIVE_FORCE_PER_NORMAL) + contact_friction(DVec3::new(v, 0.0, 0.0), k, mu_n, false).x
    }

    #[test]
    fn friction_brings_a_sliding_block_to_rest_without_chatter() {
        // Knee μN/k ≈ 4 m/s, so v0 = 15 starts in the Coulomb (saturated) regime,
        // decelerates linearly, crosses into the viscous knee, then asymptotes to 0.
        let (k, mu_n) = (600.0, N_NORMAL);
        let rust = long_step(fric_x(k, mu_n, false), 15.0, 4.0);
        let reference = long_reference(fric_x(k, mu_n, false), 15.0, 4.0);
        let dev = max_abs_dev(&rust, &reference);
        println!("[oracle] friction: max|v−v_ref| = {dev:.4} m/s, v_end = {:.4}", rust.last().unwrap());
        assert!(dev < 0.05, "regularized friction diverges from the continuous reference: {dev}");
        assert!(rust.last().unwrap().abs() < 0.05, "block must come to rest");
        // REGRESSION: continuous-through-zero friction approaches rest asymptotically,
        // never overshooting through zero → no stiction chatter.
        assert_eq!(sign_flips(&rust), 0, "regularized friction must not chatter through zero");
    }

    #[test]
    fn deadband_friction_chatters_the_regression_the_fix_removed() {
        // REGRESSION (the steering jitter): a slip dead-band let sub-threshold motion
        // overshoot through zero → a stiction limit-cycle. The continuous law settles;
        // the dead-band law sign-flips forever near rest. The oracle catches exactly this.
        let mu_n = N_NORMAL;
        let smooth = long_step(fric_x(600.0, mu_n, false), 15.0, 4.0);
        let deadband = long_step(fric_deadband(mu_n), 15.0, 4.0);
        let (fs, fd) = (sign_flips(&smooth), sign_flips(&deadband));
        println!("[oracle] chatter: regularized {fs} sign-flips, dead-band {fd}");
        assert_eq!(fs, 0, "regularized law settles");
        assert!(fd >= 5, "dead-band law limit-cycles near zero (got {fd} flips)");
    }

    #[test]
    fn braking_stops_the_block_sooner_than_coasting() {
        // Dynamic form of the unit test: with weak grip, coasting friction is linear
        // and slow; full-cone braking decelerates hard, so the block stops far sooner.
        let (k, mu_n) = (50.0, N_NORMAL);
        let coast = long_step(fric_x(k, mu_n, false), 10.0, 3.0);
        let brake = long_step(fric_x(k, mu_n, true), 10.0, 3.0);
        println!("[oracle] brake: coast v_end {:.3}, brake v_end {:.3}", coast.last().unwrap(), brake.last().unwrap());
        assert!(brake.last().unwrap().abs() < 0.3, "braking grips the full cone → quick stop");
        assert!(coast.last().unwrap().abs() > 3.0, "weak coasting grip is still rolling");
    }

    #[test]
    fn drive_accelerates_to_a_balanced_terminal_velocity() {
        // Moderate throttle: drive < μN, so contact grip balances it at v_term = drive/k
        // (viscous regime). Validates drive magnitude (throttle·N·2) and the
        // drive/friction balance against the continuous reference.
        let (throttle, k, mu_n) = (0.2, 50.0, N_NORMAL);
        let drive = drive_force_mag(throttle, N_NORMAL, DEFAULT_DRIVE_FORCE_PER_NORMAL);
        let v_term = drive / k;
        assert!(drive < mu_n, "scenario must stay in the sub-cone (balanced) regime");
        let rust = long_step(drive_minus_friction(throttle, k, mu_n), 0.0, 30.0);
        let reference = long_reference(drive_minus_friction(throttle, k, mu_n), 0.0, 30.0);
        let dev = max_abs_dev(&rust, &reference);
        println!("[oracle] drive: v_term {:.3} (expected {v_term:.3}), max dev {dev:.4}", rust.last().unwrap());
        assert!(dev < 0.05, "drive+friction diverges from the continuous reference: {dev}");
        assert!((rust.last().unwrap() - v_term).abs() < 0.3, "must settle at drive/k");
    }

    #[test]
    fn reverse_throttle_mirrors_forward() {
        // REGRESSION: reverse used to be clamped away (`clamp(0.0, 1.0)`). Negative
        // throttle must produce the mirror-image terminal velocity.
        let (k, mu_n) = (50.0, N_NORMAL);
        let fwd = long_step(drive_minus_friction(0.2, k, mu_n), 0.0, 30.0);
        let rev = long_step(drive_minus_friction(-0.2, k, mu_n), 0.0, 30.0);
        println!("[oracle] reverse: fwd v {:.3}, rev v {:.3}", fwd.last().unwrap(), rev.last().unwrap());
        assert!((fwd.last().unwrap() + rev.last().unwrap()).abs() < 1e-6, "reverse mirrors forward");
    }

    #[test]
    fn excess_throttle_breaks_traction_past_the_friction_cone() {
        // High throttle: drive > μN, so contact grip can NEVER balance it — the chassis
        // accelerates past the cone knee (wheelspin), net accel → (drive−μN)/m.
        let (throttle, k, mu_n) = (0.8, 50.0, N_NORMAL);
        let drive = drive_force_mag(throttle, N_NORMAL, DEFAULT_DRIVE_FORCE_PER_NORMAL);
        assert!(drive > mu_n, "this scenario must exceed the cone");
        let rust = long_step(drive_minus_friction(throttle, k, mu_n), 0.0, 10.0);
        let n = rust.len();
        let late_accel = (rust[n - 1] - rust[n - 2]) / DT_SIM;
        let expect = (drive - mu_n) / M;
        println!("[oracle] wheelspin: v {:.1} m/s climbing, late accel {late_accel:.3} (≈ (drive−μN)/m = {expect:.3})", rust.last().unwrap());
        assert!(*rust.last().unwrap() > 50.0, "breaks traction and keeps accelerating");
        assert!((late_accel - expect).abs() < 0.2, "saturated: net accel = (drive−μN)/m");
    }
}


