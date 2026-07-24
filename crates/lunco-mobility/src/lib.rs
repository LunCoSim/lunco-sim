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

use avian3d::prelude::*;
use bevy::ecs::schedule::common_conditions::any_with_component;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use kernels::{ControlKernelRegistry, DriveMix};
use lunco_core::architecture::Port;
use lunco_core::ports::{PortBackend, PortDirection, PortRef};
use lunco_core::{ActuatorPorts, CommandInputs};

/// they live here rather than in core (see the nothing-into-core rule).
pub mod kernels;
mod sensing;
mod wheel_spin;
use wheel_spin::update_wheel_spin;

pub mod wheel_kinematics;
use wheel_kinematics::{wheel_hub_pose, wheel_hub_velocity};

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
    ($body:block) => {
        $body
    };
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
            // `DriveMix` — the kernel-selected allocation spec that replaced the
            // per-arch `DifferentialDrive`/`AckermannSteer`/`GenericDriveMix`.
            // Registered here with the kernels it selects between; it is a
            // vehicle-domain type and core carries no domain.
            .register_type::<DriveMix>()
            .register_type::<DifferentialCoupling>()
            .register_type::<SuspensionPiston>()
            .register_type::<SuspensionSpring>()
            .register_type::<ProxyWheelMassFolded>()
            // A vehicle's mass must not depend on which `drivetrain` variant
            // realizes its wheels. Ungated: this is a one-shot mass-property
            // correction per chassis, not a force, so it must land even while
            // physics is held — a rover that spawns during a cinematic hold is
            // still the same rover.
            .add_systems(FixedUpdate, fold_proxy_wheel_mass)
            // G5 rocker-bogie differential — separate set: it doesn't read the
            // control ports, only couples two rocker hinges. Idle unless a
            // `DifferentialCoupling` exists, so it's free for every other vehicle.
            .add_systems(
                FixedUpdate,
                differential_coupling_system
                    .run_if(any_with_component::<DifferentialCoupling>)
                    // Applies an equal-and-opposite force pair at the two rocker
                    // anchors, so it accumulates across a physics hold exactly as the
                    // wheel systems do. Same gate, same reason.
                    .run_if(lunco_physics::physics_is_live),
            )
            .add_systems(
                FixedUpdate,
                (
                    suspension_system,
                    apply_wheel_suspension,
                    update_suspension_visuals,
                    // STEER, then SOLVE THE TIRE, then APPLY IT. Steering first so the
                    // contact basis is this tick's heading; the spin solve produces the
                    // patch force; `apply_wheel_drive` only hands it to the body. The
                    // old order (drive → steer → spin) meant the chassis force was
                    // built from last tick's steer angle and from a spin it could not
                    // see, which is what forced the two independent force fudges.
                    apply_wheel_steering,
                    update_wheel_spin,
                    apply_wheel_drive,
                )
                    .chain()
                    // Read the actuator `Port` AFTER wire propagation has carried this
                    // tick's command into it (same fixed tick), so actuation isn't
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
                        //
                        // `physics_is_live`, NOT a bare `Time<Virtual>` check. These systems
                        // write into avian's force accumulator, which only the physics step
                        // clears — and a physics HOLD (a frozen cinematic beat) deliberately
                        // leaves virtual time running, so the old virtual-clock gate was open
                        // for exactly the window that must be closed. Gating on the physics
                        // CLOCK rather than the holds resource also keeps stepped cinematics
                        // drivable: a granted `PhysicsStepRequest` frame unpauses the clock
                        // for exactly the ticks that integrate. It still covers the virtual
                        // pause/speed case it was written for; see `physics_is_live`.
                        lunco_physics::physics_is_live,
                    ),
            );

        // Expose every FSW's logical command ports (a rover's throttle/steer/brake,
        // etc.) through the shared port substrate, so the ONE generic `SetPorts`
        // command (and wires/API/scripts) can drive any controllable by name.
        app.init_resource::<lunco_core::ports::PortRegistry>();
        {
            let mut reg = app
                .world_mut()
                .resource_mut::<lunco_core::ports::PortRegistry>();
            reg.register(COMMAND_INPUT_BACKEND);
        }

        // Own the control-allocation kernel registry here (the plugin that runs
        // `apply_drive_mix`), seeded with the built-in `skid`/`linear` kernels —
        // so any app running the drive systems has it, without depending on the
        // full core plugin. Flight-kernel crates register additively the same way.
        if !app.world().contains_resource::<ControlKernelRegistry>() {
            app.insert_resource(ControlKernelRegistry::with_defaults());
        }

        // Mix the FSW's logical command inputs (written via the port backend) into
        // the actuator command `Port`s BEFORE propagation carries them across the
        // wires (and before the wheel systems, which run
        // `.after(ControlDacSet)`). The
        // command surface is derived from USD `Controls` bindings (never a Rust
        // literal) by `sync_command_surface`, ordered before the mix so a
        // freshly-loaded vessel is drivable the same tick its binding lands.
        app.add_systems(
            FixedUpdate,
            (sync_command_surface, apply_drive_mix)
                .chain()
                .before(lunco_core::ControlDacSet),
        );

        // Keep raycast wheels' physics `Position`/`Rotation` grid-absolute so their
        // suspension rays originate in avian's frame (not the big_space render
        // frame) — the fix for "rover rests but won't drive at an elevated site".
        // Runs in the physics schedule AFTER the step (fresh chassis pose) and
        // BEFORE the spatial query casts the rays. See the fn docs.
        app.add_systems(
            FixedPostUpdate,
            sync_raycast_wheel_physics_pose
                .after(PhysicsSystems::StepSimulation)
                .before(SpatialQuerySystems),
        );

        // ── Rollback replay ──────────────────────────────────────────────────
        // Mirror the FULL actuation chain into `RollbackReplay` with the SAME
        // relative order as `FixedUpdate`, so re-simulating a recorded input
        // reproduces the host's forces exactly. No `Time<Virtual>` pause guard here:
        // a replay step is an instantaneous re-simulation, not a wall-clock tick, so
        // it must run regardless of the pause/speed state of the virtual clock.
        app.add_systems(
            lunco_core::RollbackReplay,
            (sync_command_surface, apply_drive_mix)
                .chain()
                .before(lunco_core::ControlDacSet),
        );
        app.add_systems(
            lunco_core::RollbackReplay,
            (
                suspension_system,
                apply_wheel_suspension,
                update_suspension_visuals,
                // STEER, then SOLVE THE TIRE, then APPLY IT. Steering first so the
                // contact basis is this tick's heading; the spin solve produces the
                // patch force; `apply_wheel_drive` only hands it to the body. The
                // old order (drive → steer → spin) meant the chassis force was
                // built from last tick's steer angle and from a spin it could not
                // see, which is what forced the two independent force fudges.
                apply_wheel_steering,
                update_wheel_spin,
                apply_wheel_drive,
            )
                .chain()
                .after(lunco_core::ControlDacSet),
        );
    }
}

/// Marks a chassis whose proxy wheels' mass has already been folded in, so the
/// fold happens exactly once per vehicle.
#[derive(Component, Debug, Reflect)]
#[reflect(Component)]
pub struct ProxyWheelMassFolded;

/// Fold the proxy wheels' authored mass onto the chassis rigid body.
///
/// A ROVER'S MASS IS A PROPERTY OF THE ROVER, NOT OF HOW ITS WHEELS ARE REALIZED.
/// The same `skid_rover.usda` composed with `drivetrain = physical` masses 1100 kg
/// (chassis 1000 + four 25 kg wheel bodies avian integrates in their own right),
/// and with `drivetrain = raycast` massed 1000 kg — the proxy wheels are kinematic,
/// so avian never saw their authored `physics:mass` at all. One variant switch
/// silently changed the vehicle by 10%, which no variant is allowed to do.
///
/// That 10% is directly a speed error: `physxRigidBody:linearDamping` drags `c·m·v`,
/// so terminal speed goes as `F/(c·m)`.
///
/// MASS AND INERTIA MOVE TOGETHER, or the fix is worse than the bug. Folding mass
/// ALONE was measured: the chassis carries an authored `physics:diagonalInertia`
/// under `NoAutoAngularInertia`, so the rover got harder to push and no harder to
/// turn, and `drivetrain_parity`'s heading swung 56.3° → 61.7° against a physical
/// twin at 51° — a 9% gap turned into 29%. Each wheel therefore contributes its
/// parallel-axis term `m·d²` at its authored mount as well as its mass.
///
/// SO DOES THE CENTRE OF MASS. Four 25 kg wheels hanging at `y = −0.65` genuinely
/// pull the vehicle's combined centre of mass down — on the physical rover avian
/// does that arithmetic for free, because those wheels are bodies. Folding only the
/// mass and the tensor left the raycast rover's mass acting at the chassis centre,
/// ~5.9 cm too high, and CoM HEIGHT IS LOAD TRANSFER: it is exactly the quantity a
/// turning comparison is sensitive to. The fold therefore also writes the combined
/// centre of mass — chassis plus the proxy wheels as point masses at their mounts.
///
/// The tensor is taken about that COMBINED centre, not about the body origin: the
/// authored `physics:diagonalInertia` is about the chassis centre, so once the
/// combined centre moves, both the chassis and each wheel contribute a parallel-axis
/// term measured from the NEW centre. (The correction is small but not nothing —
/// ~3.8 kg·m² on a ~1220 kg·m² skid-rover `I_x`, 0.3% — and getting it right costs
/// one subtraction, whereas leaving it wrong is a number nobody could later explain.)
///
/// THE WHEEL'S OWN SPIN INERTIA IS DELIBERATELY NOT FOLDED. `update_wheel_spin`
/// already integrates each wheel's ω against `I = ½·m·r²` ([`WheelRaycast::axle_inertia`]),
/// exactly as the physical wheel's own rigid body does. Adding it to the chassis
/// tensor as well would count one physical quantity twice. What the chassis is
/// missing is only the wheel as a MASS AT A DISTANCE, which is what this adds.
///
/// Inertia is folded only when the body carries [`NoAutoAngularInertia`] — i.e. the
/// tensor is authored. Without it avian recomputes the tensor from colliders every
/// time the mass properties change, and this addition would be silently discarded.
/// The centre of mass is written WITH [`NoAutoCenterOfMass`] for the same reason:
/// avian consults the `CenterOfMass` override only inside `if no_auto_center_of_mass`,
/// so the marker is what makes the write survive the next recompute — and the
/// recompute is what publishes it to `ComputedCenterOfMass`, which is the component
/// the solver integrates against.
pub fn fold_proxy_wheel_mass(
    mut commands: Commands,
    q_chassis: Query<(Entity, &Children), (With<DriveMix>, Without<ProxyWheelMassFolded>)>,
    q_wheels: Query<(&WheelRaycast, &Transform)>,
    mut q_body: Query<(
        &mut Mass,
        Option<&mut AngularInertia>,
        Has<NoAutoAngularInertia>,
        Option<&CenterOfMass>,
        Option<&ComputedCenterOfMass>,
    )>,
) {
    for (chassis, children) in &q_chassis {
        // The wheel's `mass` arrives from `WheelParams::apply_to_raycast`, which may
        // land a frame after the component itself. A wheel still reading zero means
        // the parameters have not been applied yet, so the vehicle is not ready to
        // fold and must be left for a later tick — never folded at half its mass.
        let mut wheels = Vec::new();
        let mut pending = false;
        for child in children.iter() {
            let Ok((wheel, tf)) = q_wheels.get(child) else {
                continue;
            };
            if wheel.mass <= 0.0 {
                pending = true;
                break;
            }
            wheels.push((wheel.mass, tf.translation.as_dvec3()));
        }
        if pending || wheels.is_empty() {
            continue;
        }

        let Ok((mut mass, inertia, inertia_authored, com_override, com_computed)) =
            q_body.get_mut(chassis)
        else {
            continue;
        };

        // The chassis's own centre, BEFORE the wheels are folded in. An authored
        // `physics:centerOfMass` arrives as the override and wins (six_wheel_rover
        // authors one); otherwise avian's collider-derived value is the truth.
        let com_chassis = com_override
            .map(|c| c.0.as_dvec3())
            .or_else(|| com_computed.map(|c| c.0))
            .unwrap_or(DVec3::ZERO);
        let chassis_mass = mass.0 as f64;

        let added: f64 = wheels.iter().map(|(m, _)| *m).sum();
        let total = chassis_mass + added;
        mass.0 += added as f32;

        // Combined centre of mass: chassis at its own centre, each proxy wheel a
        // point mass at its mount. On a symmetric rover the x/z terms cancel and
        // only the drop survives — which is the whole point.
        let com_new = if total > 0.0 {
            let mut moment = com_chassis * chassis_mass;
            for (m, d) in &wheels {
                moment += *d * *m;
            }
            moment / total
        } else {
            com_chassis
        };

        if let (Some(mut inertia), true) = (inertia, inertia_authored) {
            // Parallel-axis about the COMBINED centre: a mass `m` at `d` adds
            // `m·(d_j² + d_k²)` about each axis `i`, with `d` measured from that
            // centre. The chassis contributes too, because its authored tensor is
            // about ITS centre and the centre has just moved.
            let perp = |m: f64, d: DVec3| {
                DVec3::new(
                    m * (d.y * d.y + d.z * d.z),
                    m * (d.x * d.x + d.z * d.z),
                    m * (d.x * d.x + d.y * d.y),
                )
            };
            let mut principal = perp(chassis_mass, com_chassis - com_new);
            for (m, d) in &wheels {
                principal += perp(*m, *d - com_new);
            }
            inertia.principal += principal.as_vec3();
        }

        commands
            .entity(chassis)
            .try_insert((CenterOfMass(com_new.as_vec3()), NoAutoCenterOfMass));
        commands.entity(chassis).try_insert(ProxyWheelMassFolded);
    }
}

/// How far ABOVE the axle a raycast wheel's suspension ray starts — the STRUT TOP.
///
/// THE WHEEL PRIM IS THE AXLE, in both realizations. A raycast strut hangs the hub
/// `rest_length` below its cast origin and the tire holds the hub `wheel_radius`
/// above the ground, so at rest the strut occupies exactly `rest_length −
/// wheel_radius` and its top sits that far above the authored mount. Casting from
/// there puts the hub AT the authored mount at rest, which is where the physical
/// realization's wheel body actually is.
///
/// This used to be baked into the asset instead: `raycast_drivetrain.usda` authored
/// the wheel prims 0.5 m higher than `physical_drivetrain.usda` so the raycast rover
/// would end up at a plausible ride height. One rover then had two mount heights and
/// two centre-of-mass heights depending on a variant switch — and 0.5 m was not even
/// the 0.3 m the authored spring implies. Deriving it here means a suspension swap
/// (`rocker.usda`, `rigid.usda`) moves the strut with it, and nothing needs re-typing.
///
/// A `rigid` mount (`rest_length` 0) returns `−wheel_radius`: the ray starts at the
/// contact patch, which is where a wheel bolted straight to the hull touches down.
pub fn strut_offset(rest_length: f64, wheel_radius: f64) -> f64 {
    rest_length - wheel_radius
}

/// Upper clamp on the suspension force magnitude (N) applied per spring.
/// Bounds the spring+damping sum so a deeply-compressed strut or a numerical
/// velocity spike can't inject an explosive impulse that launches the rover.
const MAX_SUSPENSION_FORCE_N: f64 = 100_000.0;

// ── Pure force laws (unit-tested; the numerically-sensitive bits live here) ─────

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
pub(crate) fn contact_plane_basis(
    wheel_forward: DVec3,
    wheel_right: DVec3,
    normal: DVec3,
) -> (DVec3, DVec3) {
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

/// Suspension normal-force magnitude: spring `k·x` plus damping `c·v`, with the
/// DAMPING bounded to ±spring so the total stays in `[0, 2·spring]` without a
/// `.max(0)` cliff. The cliff (clamping the *total* to ≥0) dropped damping on the
/// rebound half-cycle → an undamped suspension limit-cycle (the forward+turn
/// jitter); unbounded `c·v` also spiked the force on hard hits. Bounding the
/// damping term fixes both. The total is capped at [`MAX_SUSPENSION_FORCE_N`],
/// so a wheel spawned intersecting terrain can't launch the rover.
fn suspension_force_mag(compression: f64, spring_k: f64, relative_vel: f64, damping_c: f64) -> f64 {
    let spring = compression * spring_k;
    let damping = (relative_vel * damping_c).clamp(-spring, spring);
    (spring + damping).clamp(0.0, MAX_SUSPENSION_FORCE_N)
}

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
    /// Radius of the tire (effectively the minimum offset from ground).
    pub wheel_radius: f64,
    /// Entity for the visual mesh to be transformed.
    pub visual_entity: Option<Entity>,
    /// Resultant normal force from the last physics tick, used for friction calculations.
    pub last_normal_force: f64,
    /// Drives the visible spin of the wheel mesh.
    pub spin_angle: f64,
    /// wheelspin/skid, and free-runs (driven by torque vs bearing drag) in the air.
    pub spin_velocity: f64,
    /// `½·m·r²` that resists changes in spin (unless `moment_of_inertia` is set).
    pub mass: f64,
    /// When `> 0` it overrides the mass-derived `½·m·r²`.
    pub moment_of_inertia: f64,
    /// (USD `physxVehicleEngine:peakTorque`, required).
    pub drive_torque_max: f64,
    /// the hub in its own right — never inferred from the drive torque.
    pub bearing_damping: f64,
    /// (joint-motor) realization of the same wheel obeys.
    pub max_rotation_speed: f64,
    /// Caps the traction torque at `μ·N`, above which the tire breaks loose.
    pub friction_mu: f64,
    /// hard the tire grips toward `v/r` before saturating at the friction limit.
    pub slip_stiffness: f64,
    /// one number instead of two independently-fudged ones.
    pub tire_force: DVec3,
    /// like ice.
    pub lateral_grip_stiffness: f64,
    /// traction torque the wheel locks and skids.
    pub brake_torque_max: f64,
    /// Steering rotation axis in the wheel's local frame
    /// (USD `lunco:wheel:steerAxis`, required).
    /// `+Y` (yaw) reproduces a flat-ground car steer; a motorcycle's
    /// raked steering head tilts this (e.g. `(0, cos θ, sin θ)`) so the front
    /// wheel steers about the fork axis, not vertical.
    pub steer_axis: DVec3,
}

/// **USD is the sole source of a wheel's physical numbers.**
///
/// Every tunable below is zero here on purpose: `Default` exists only as the
/// struct-update base for `WheelParams::to_wheel_raycast`, which immediately
/// overwrites all of them from the composed stage via `apply_to_raycast`. The
/// reader (`lunco_usd_sim::wheel_params`) requires each attribute and reports a
/// collected missing-attribute error, so an unauthored wheel FAILS rather than
/// silently inheriting numbers nobody wrote. A zeroed wheel that ever reaches
/// the world is therefore visibly inert (no drive, no grip) instead of quietly
/// plausible — which is the point.
///
/// `steer_axis` is `+Y` because a zero vector is not a rotation axis at all;
/// `lunco:wheel:steerAxis` is required and overwrites it.
impl Default for WheelRaycast {
    fn default() -> Self {
        Self {
            suspension_port: Entity::PLACEHOLDER,
            drive_port: Entity::PLACEHOLDER,
            steer_port: Entity::PLACEHOLDER,
            wheel_radius: 0.0,
            visual_entity: None,
            last_normal_force: 0.0,
            spin_angle: 0.0,
            spin_velocity: 0.0,
            mass: 0.0,
            moment_of_inertia: 0.0,
            drive_torque_max: 0.0,
            bearing_damping: 0.0,
            max_rotation_speed: 0.0,
            friction_mu: 0.0,
            slip_stiffness: 0.0,
            lateral_grip_stiffness: 0.0,
            tire_force: DVec3::ZERO,
            brake_torque_max: 0.0,
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
/// **Geometry**: the wheel entity transform is the AXLE. The ray starts
/// [`strut_offset`] above it — the strut top — and points straight down, so
/// `hit_distance` is the distance from the strut top to the ground and the spring
/// is compressed when `hit_distance < rest_length`. The wheel visual is positioned
/// at `ground_y + wheel_radius`, which in wheel-local Y is the compression, so an
/// unloaded wheel draws exactly at its authored mount.
fn apply_wheel_suspension(
    mut q_wheels: Query<(
        &mut WheelRaycast,
        &Suspension,
        &RayHits,
        &Transform,
        &ChildOf,
    )>,
    // Force must land only on a body the solver will integrate. A disabled body
    // (frozen while its program compiles, say) never has its accumulators
    // cleared, so force applied to it is stored, not spent, and discharges in
    // full on the step that eventually runs — see `lunco_physics::Integrable`.
    mut q_chassis: Query<(Forces, &RigidBody), (With<DriveMix>, lunco_physics::Integrable)>,
    mut q_visual: Query<&mut Transform, (Without<WheelRaycast>, Without<DriveMix>)>,
) {
    for (mut wheel, susp, hits, wheel_tf, parent) in q_wheels.iter_mut() {
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

            let mut current_distance = susp.rest_length;
            // A hit with a DEGENERATE normal is not a contact. avian casts these rays
            // `solid: true`, so a ray whose origin is INSIDE a collider returns
            // distance 0 with a ZERO normal — and distance 0 always sorts ahead of the
            // real ground a few centimetres below. The old code then computed a
            // saturated `total_force_mag`, applied `zero_normal * mag` (i.e. NO force),
            // and still published that saturated value as `last_normal_force`. Downstream
            // `apply_wheel_drive` gates on `normal_force >= 1.0`, so it ran at full
            // traction authority against a chassis nothing was holding up: the rover
            // tore itself off the ground and reappeared at the grid origin. Report what
            // actually happened — no support — rather than what the spring would have
            // produced had the geometry been real.
            let contact = hits
                .iter_sorted()
                .find(|hit| hit.normal.is_finite() && hit.normal.length_squared() > 1.0e-12);
            if let Some(hit) = contact {
                let distance = hit.distance;
                if distance < susp.rest_length {
                    current_distance = distance;
                    // Suspension is compressed: apply spring-damper force.
                    let compression = susp.rest_length - distance;
                    // Damping calculation based on relative normal velocity.
                    // Positive relative_vel = wheel moving toward ground (compressing).
                    // Negative relative_vel = wheel moving away from ground (extending).
                    let ray_dir_world = forces.rotation().0 * Vec3::NEG_Y.as_dvec3();
                    let lin_vel = forces.linear_velocity();
                    let ang_vel = forces.angular_velocity();
                    let velocity_at_wheel =
                        wheel_hub_velocity(lin_vel, ang_vel, world_pos, forces.position().0);
                    let relative_vel = velocity_at_wheel.dot(ray_dir_world);

                    // Clamped to `MAX_SUSPENSION_FORCE_N`, which this path was
                    // silently missing — the constant existed and was applied only
                    // by the JOINT suspension (`differential_coupling_system`), so
                    // the raycast strut, the one every rover actually uses, had no
                    // ceiling at all. The clamp is what its own doc says it is: a
                    // bound on a deeply-compressed strut or a numerical velocity
                    // spike, so neither can inject an explosive impulse. It is a
                    // backstop, not the fix for the frozen-shot accumulation — that
                    // is `physics_is_live` on the system — but an unbounded force
                    // law is worth closing on its own.
                    let total_force_mag = suspension_force_mag(
                        compression,
                        susp.spring_k,
                        relative_vel,
                        susp.damping_c,
                    )
                    .clamp(0.0, MAX_SUSPENSION_FORCE_N);

                    let force_vec = hit.normal * total_force_mag;
                    if apply_force {
                        forces.apply_force_at_point(force_vec, world_pos);
                    }
                    wheel.last_normal_force = total_force_mag;
                } else {
                    wheel.last_normal_force = 0.0;
                }
            } else {
                wheel.last_normal_force = 0.0;
            }

            // Position the wheel visual on the ground (or fully extended if airborne).
            //
            // The visual is always a CHILD of the wheel entity (see
            // `setup_raycast_wheel` in lunco-usd-sim), so its local Y is relative
            // to the AXLE mount, not the chassis. We want the visual centre at
            // `ground + radius`; the ray starts `strut_offset` above the mount and
            // the ground is `distance` below the ray origin, so in wheel-local
            // space that is `strut_offset + radius - distance`, i.e. the
            // COMPRESSION — zero at rest, positive as the strut packs up.
            if let Some(visual_entity) = wheel.visual_entity {
                if let Ok(mut visual_tf) = q_visual.get_mut(visual_entity) {
                    visual_tf.translation.y = (susp.rest_length - current_distance) as f32;
                }
            }
        }
    }
}

/// Keep each raycast wheel's avian `Position`/`Rotation` in the grid-ABSOLUTE
/// physics frame so its suspension `RayCaster` originates at the true hub — not
/// at the wheel's big_space RENDER-frame `GlobalTransform`.
///
/// avian's `update_ray_caster_positions` derives the ray origin from an entity's
/// own `Position`/`Rotation` when present, falling back to its `GlobalTransform`
/// only when they're absent. A raycast wheel now carries them (spawned in
/// `lunco-usd-sim::setup_raycast_wheel`) but is NOT a physics body, so nothing
/// else maintains them: the big_space bridge disables avian's
/// `transform_to_position` and only syncs `BridgeShadow`-carrying bodies (a bare
/// wheel has neither `RigidBody` nor `Collider`). Without this system the ray
/// would cast from the origin-relative render frame and — at an elevated site
/// (≈ +1945 m grid-absolute vs ≈ −53 m render, a ~2 km gap) — miss the terrain
/// collider entirely, leaving `last_normal_force` at 0 so `apply_wheel_drive`
/// bails on its `normal_force < 1.0` gate. That is the flat-sandbox-works /
/// elevated-moonbase-fails split: near the origin the two frames coincide.
///
/// We compose the chassis' solved grid-absolute pose with the wheel's local
/// transform via `wheel_hub_pose` — exactly how the suspension/drive force point
/// is built — running AFTER the physics step (fresh chassis pose) and BEFORE the
/// spatial query (which reads `Position`), so the cast sees this tick's pose.
fn sync_raycast_wheel_physics_pose(
    mut q_wheels: Query<(&mut Position, &mut Rotation, &Transform, &ChildOf), With<WheelRaycast>>,
    q_chassis: Query<(&Position, &Rotation), (With<DriveMix>, Without<WheelRaycast>)>,
) {
    for (mut wpos, mut wrot, wtf, parent) in q_wheels.iter_mut() {
        if let Ok((cpos, crot)) = q_chassis.get(parent.parent()) {
            let (hub_pos, hub_rot) = wheel_hub_pose(
                cpos.0,
                crot.0,
                wtf.translation.as_dvec3(),
                wtf.rotation.as_dquat(),
            );
            // The wheel's `Position`/`Rotation` IS avian's ray-origin frame: the
            // caster's local origin is `DVec3::ZERO`, so the global origin is
            // `Position + Rotation * ZERO` — and a NaN rotation poisons even that.
            // avian's `raycast` asserts `origin.is_finite()` and takes the whole
            // app down with it, so a solver blow-up upstream must STOP HERE rather
            // than becoming a panic in a system that did nothing wrong. Holding
            // last tick's pose for a frame is strictly better than a crash; if the
            // chassis recovers, the wheel snaps back on the next tick.
            if !hub_pos.is_finite() || !hub_rot.is_finite() {
                continue;
            }
            wpos.0 = hub_pos;
            wrot.0 = hub_rot;
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
    q_wheels: Query<(&WheelRaycast, &Transform, &RayHits, &ChildOf)>,
    q_ports: Query<&Port>,
    // Force must land only on a body the solver will integrate. A disabled body
    // (frozen while its program compiles, say) never has its accumulators
    // cleared, so force applied to it is stored, not spent, and discharges in
    // full on the step that eventually runs — see `lunco_physics::Integrable`.
    mut q_chassis: Query<
        (Forces, &RigidBody, Option<&CommandInputs>),
        (With<DriveMix>, lunco_physics::Integrable),
    >,
) {
    for (wheel, wheel_tf, hits, parent) in q_wheels.iter() {
        let parent_entity = parent.parent();
        if let Ok((mut forces, body, inputs)) = q_chassis.get_mut(parent_entity) {
            // drive-diag: the drive port the wheel reads, the body kind (Dynamic
            // vs Kinematic — the snap-back tell), and ground contact. Throttle-
            // gated so it only fires while driving. Whole block compiles out
            // (incl. the extra port read) without the `drive-diag` feature.
            drive_diag_block!({
                if let Ok(dbgport) = q_ports.get(wheel.drive_port) {
                    if dbgport.value.abs() > f64::EPSILON {
                        info!("[drive-diag] apply_wheel_drive: chassis {:?} body={:?} port.value={} normal_force={} has_contact={}",
                            parent_entity, body, dbgport.value, wheel.last_normal_force, hits.iter().next().is_some());
                    }
                }
            });
            // Skip forces if body is kinematic
            if matches!(body, RigidBody::Kinematic) {
                continue;
            }
            // Braking: the wheel-spin model locks the spin, but the chassis only
            // stops if the contact grips. We make friction saturate (full cone)
            // while braking so a locked wheel actually decelerates the rover.
            let braking = inputs.is_some_and(|c| c.brake_active);

            if let Ok(port) = q_ports.get(wheel.drive_port) {
                // Traction only exists when the ray is hitting the ground. Bind
                // the hit so its surface normal defines the contact plane (needed
                // for leaning single-track wheels).
                if let Some(ground_hit) = hits.iter_sorted().next() {
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

                    // Hub velocity in the contact plane. Needed BEFORE the drive
                    // force: its longitudinal component is the ground speed the
                    // torque–speed rolloff reads.
                    let chassis_vel = forces.linear_velocity();
                    let chassis_ang_vel = forces.angular_velocity();
                    let hub_vel = wheel_hub_velocity(
                        chassis_vel,
                        chassis_ang_vel,
                        hub_pos_world,
                        forces.position().0,
                    );
                    let long_vel = hub_vel.dot(forward); // longitudinal slip
                    let lat_vel = hub_vel.dot(right); // lateral slip

                    // The tire force was already solved this tick, from the real
                    // contact slip `ω·r − v` and the wheel's own lateral slip —
                    // see `update_wheel_spin`. Applying it is all that is left.
                    forces.apply_force_at_point(wheel.tire_force, hub_pos_world);
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
    mut q_wheels: Query<(
        &mut Transform,
        &ChildOf,
        &lunco_hardware::SteeringActuator,
        &WheelRaycast,
    )>,
    q_chassis: Query<&RigidBody, With<DriveMix>>,
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
        let axis = if raw.length_squared() > 1e-12 {
            raw.normalize()
        } else {
            Vec3::Y
        };
        transform.rotation = Quat::from_axis_angle(axis, -steer.output_angle as f32);
    }
}

// The per-arch steering components (`DifferentialDrive`, `AckermannSteer`,
// `GenericDriveMix`) are GONE. A vessel's command→actuator allocation is now the
// data-driven `lunco_core::kernels::DriveMix { kernel, ports, entries }`, whose
// `kernel` names a self-registered `ControlKernel` (`skid` / `linear` / … flight
// allocators later). `apply_drive_mix` looks the kernel up and runs it — no
// per-architecture Rust branch, no component-type taxonomy.

/// `PhysicsDriveAPI` (stiffness/damping/targetPosition). `lunco-usd-sim` reads that
/// into a `PendingDifferential` and `resolve_differential_coupling` attaches this
/// component; inert until present, so existing vehicles are unaffected.
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
    /// Authored `physxGearJoint:gearRatio` — the `r` in `θ_a = r·θ_b`.
    /// `-1` (the default) is the mirror/rocker-bogie case.
    pub ratio: f64,
    /// Target for `θ_a − r·θ_b` (rad). Zero ⇒ symmetric (mirror) rockers at `r = -1`.
    pub rest_offset: f64,
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
            // Mirror rockers — the rocker-bogie case, and what the coupling
            // hardcoded before the authored ratio was threaded through.
            ratio: -1.0,
            rest_offset: 0.0,
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

/// PD multiplier `λ` (N·m about the hinge axis) for the geared constraint
/// `c = θ_a − r·θ_b − rest_offset`, where `r` is the authored
/// `physxGearJoint:gearRatio` — the `r` in `θ_a = r·θ_b`.
///
/// The generalized torque on each body is `τ_i = −λ·∂c/∂θ_i`, so `τ_a = −λ`,
/// `τ_b = +λ·r`, and the chassis takes `−(τ_a + τ_b) = λ·(1 − r)`, which is what
/// conserves angular momentum about the axis for any ratio.
///
/// `rate` is `ċ = (ω_a − r·ω_b − (1 − r)·ω_c) · axis`.
///
/// `r = −1` is the mirror/rocker-bogie case (`c = θ_a + θ_b`) and the default.
///
/// # This is a HOLONOMIC constraint, not a spring you must tune
///
/// The obvious form — `λ = k·c + d·ċ` — is an EXPLICIT penalty, and it made the
/// authored stiffness a stability parameter rather than a physical one. A rocker
/// -bogie differential is a bar or a bevel gearset: it has no compliance, so the
/// only reason `k` was ever finite was that raising it blew the integrator up.
/// That coupling was measured (2026-07-22) to fail on both sides at once: at
/// k = 15 000 it left a 20% residual on a loaded rover AND anything stiffer
/// (30 000, 60 000) reached NaN within 10 s. There was no value that worked, and
/// the vehicle's mass budget was pinned by that fact.
///
/// So evaluate the same spring-damper IMPLICITLY (backward Euler): solve for the
/// torque that will be correct at the END of the step rather than the one implied
/// by the state at its start. With `w` the constraint-space inverse inertia
/// (below), the impulse that a stiffness `k` and damping `d` really deliver over a
/// step `dt` is
///
/// ```text
///     λ = (k·c + d·ċ) / (1 + dt·w·(d + dt·k))
/// ```
///
/// which has two properties the explicit form lacks:
///
/// * **Unconditionally stable.** The denominator grows with `k`, so `λ` can never
///   overshoot; there is no `k` that diverges, and the NaN cliff is gone.
/// * **It converges to the exact constraint.** As `k → ∞`,
///   `λ → (c/dt + ċ)/(w·dt)` — precisely the impulse that drives `c` and `ċ` to
///   zero in one step, i.e. the holonomic gear. Authoring a very large `k` now
///   asks for the ideal joint and GETS it, instead of exploding.
///
/// `w` = `n·I_a⁻¹·n + r²·(n·I_b⁻¹·n) + (1 − r)²·(n·I_c⁻¹·n)` — the constraint's own
/// inverse inertia (`JM⁻¹Jᵀ`), in world space about the hinge axis `n`. Including
/// it is what makes the response mass-INDEPENDENT: a heavier hull raises `w`'s
/// contribution and the solved `λ` rises with it, so the linkage mirrors the same
/// way at 300 kg and at 600 kg. Under the explicit form the same authored `k` got
/// progressively softer as the vehicle grew, which is exactly how a 400 kg hull
/// came to break the mirroring.
///
/// `w ≤ 0` (all three bodies infinitely massive / kinematic) leaves nothing to
/// solve against; the implicit correction degenerates to the explicit one.
fn differential_lambda(
    angle_a: f64,
    angle_b: f64,
    rate: f64,
    rest_offset: f64,
    ratio: f64,
    stiffness: f64,
    damping: f64,
    w: f64,
    dt: f64,
) -> f64 {
    let c = angle_a - ratio * angle_b - rest_offset;
    let explicit = stiffness * c + damping * rate;
    let scale = 1.0 + dt * w * (damping + dt * stiffness);
    if scale > 0.0 {
        explicit / scale
    } else {
        explicit
    }
}

/// The constraint's inverse inertia about `axis_world` — the `w` in
/// [`differential_lambda`], i.e. `J·M⁻¹·Jᵀ` for `c = θ_a − r·θ_b − (1 − r)·θ_c`.
///
/// Each body contributes `(∂c/∂θ_i)² · (n · I_i⁻¹ · n)` with the inertia taken in
/// WORLD space — `I_world⁻¹ = R·I_local⁻¹·Rᵀ`, which is what `rotated()` does — so
/// a rocker that has pitched contributes about the axis it actually has now.
fn differential_inverse_inertia(
    axis_world: DVec3,
    ratio: f64,
    inertias: [(ComputedAngularInertia, DQuat); 3],
) -> f64 {
    let n = axis_world;
    // ∂c/∂θ for (rocker_a, rocker_b, chassis).
    let jacobian = [1.0, -ratio, -(1.0 - ratio)];
    inertias
        .iter()
        .zip(jacobian)
        .map(|((inertia, rotation), j)| {
            let inv_world = inertia.rotated(*rotation).inverse();
            j * j * n.dot(inv_world * n)
        })
        .sum()
}

/// Enforces every [`DifferentialCoupling`] each fixed step. Reads the two
/// rockers' pitch + rate relative to the chassis and applies the PD coupling
/// torque about the hinge axis (equal on each rocker, `−2τ` reaction on the
/// chassis). Idle unless a `DifferentialCoupling` exists.
///
/// **Verified** on an isolated rig (`differential_rig.usda`, 2026-06-30):
/// a fixed base carries a front-heavy rocker A and a balanced rocker B on lateral
/// revolutes. A/B by hinge `angle` ports —
/// - coupling OFF: A free-falls to the pendulum bottom (`+3.06`), B untouched (`+0.06`);
/// - coupling ON:  A held at `+1.72`, B driven to `−1.65` (mirror), `θ_A+θ_B ≈ 0.07`.
///
/// So the coupling correctly enforces `θ_A − r·θ_B → rest_offset` (that rig authors
/// the `r = -1` mirror). NOTE: needs a
/// non-redundant rig to *show* its effect — a passive two-rocker pair each pinned
/// by its own two ground feet already self-levels, leaving nothing for the
/// coupling to do (the original `rocker_bogie.usda` is that redundant case).
/// And keep `stiffness < I/dt²` and damp the rockers, or the explicit penalty
/// rings / diverges.
fn differential_coupling_system(
    q_coupling: Query<&DifferentialCoupling>,
    // Force must land only on a body the solver will integrate. A disabled body
    // (frozen while its program compiles, say) never has its accumulators
    // cleared, so force applied to it is stored, not spent, and discharges in
    // full on the step that eventually runs — see `lunco_physics::Integrable`.
    mut q_bodies: Query<Forces, lunco_physics::Integrable>,
    // `Forces` carries `ComputedAngularInertia` but does not expose it — the field
    // is private and `ReadRigidBodyForces` has no accessor for it. A second,
    // READ-ONLY query is the way in: `Forces` only reads that component too, so
    // the two do not conflict.
    q_inertia: Query<&ComputedAngularInertia>,
    // The implicit solve needs the step it is solving over — see
    // `differential_lambda`. Same `Res<Time>` the wheel systems read in
    // `FixedUpdate`, so it is this tick's fixed step.
    time: Res<Time>,
) {
    let dt = time.delta_secs_f64();
    if dt <= 0.0 {
        return;
    }
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
        // ċ = (ω_a − r·ω_b − (1 − r)·ω_c) · axis_world.
        let r = coupling.ratio;
        let w_c = chassis.angular_velocity();
        let rate =
            (a.angular_velocity() - r * b.angular_velocity() - (1.0 - r) * w_c).dot(axis_world);
        // `J·M⁻¹·Jᵀ` about the hinge axis, from the three bodies' CURRENT world
        // inertias — recomputed every step because a pitched rocker presents a
        // different inertia about the axis than a level one.
        let Ok([inertia_a, inertia_b, inertia_c]) =
            q_inertia.get_many([coupling.rocker_a, coupling.rocker_b, coupling.chassis])
        else {
            continue;
        };
        let w = differential_inverse_inertia(
            axis_world,
            r,
            [
                (*inertia_a, a.rotation().0),
                (*inertia_b, b.rotation().0),
                (*inertia_c, rot_c),
            ],
        );
        let lambda = differential_lambda(
            angle_a,
            angle_b,
            rate,
            coupling.rest_offset,
            r,
            coupling.stiffness,
            coupling.damping,
            w,
            dt,
        );
        if !lambda.is_finite() {
            continue;
        }
        // τ_i = −λ·∂c/∂θ_i: ∂c/∂θ_a = 1, ∂c/∂θ_b = −r.
        a.apply_torque(axis_world * -lambda);
        b.apply_torque(axis_world * (lambda * r));
        // Reaction keeps the system's angular momentum conserved for ANY ratio.
        let mut chassis = chassis;
        chassis.apply_torque(axis_world * (lambda * (1.0 - r)));
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
    // Force must land only on a body the solver will integrate. A disabled body
    // (frozen while its program compiles, say) never has its accumulators
    // cleared, so force applied to it is stored, not spent, and discharges in
    // full on the step that eventually runs — see `lunco_physics::Integrable`.
    mut q_bodies: Query<Forces, lunco_physics::Integrable>,
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
            let total_force_mag: f64 =
                (spring_force_mag + damping_force_mag).clamp(0.0, MAX_SUSPENSION_FORCE_N);

            if !total_force_mag.is_finite() {
                continue;
            }

            let force_vec: DVec3 = world_axis * total_force_mag;

            forces1.apply_force_at_point(force_vec, anchor1_world);
            forces2.apply_force_at_point(-force_vec, anchor2_world);
        }
    }
}

// ── Drive command ports ─────────────────────────────────────────────────────────

/// per-class `DriveCommand` component — command state has one home, this component.
const COMMAND_INPUT_BACKEND: PortBackend = PortBackend {
    list: |w, e, out| {
        if let Some(inputs) = w.get::<CommandInputs>(e) {
            for (name, value) in &inputs.values {
                out.push(PortRef {
                    name: name.clone(),
                    direction: PortDirection::In,
                    value: *value,
                });
            }
        }
    },
    read_output: |_w, _e, _n| None,
    read_input: |w, e, n| {
        w.get::<CommandInputs>(e)
            .and_then(|c| c.values.get(n).copied())
    },
    write_input: |w, e, n, v| {
        if let Some(mut c) = w.get_mut::<CommandInputs>(e) {
            if let Some(slot) = c.values.get_mut(n) {
                *slot = v;
                return true;
            }
        }
        false
    },
    // Map-backed: name-based write is one `get::<CommandInputs>` + a map lookup.
    // A resolve→slot fast path here would need a name interner (the slot can't
    // carry the string) — a documented follow-up if the drive-command write fold
    // shows up in profiling, not needed for correctness.
    resolve_output: None,
    resolve_input: None,
    read_slot: None,
    write_slot: None,
};

/// Derive each controllable's command surface from USD: for any entity that has
/// both a [`CommandInputs`] and a [`lunco_core::ControlBinding`], ensure every port
/// the binding targets exists in `CommandInputs.values` (seeded `0.0`).
///
/// This is what lets the command vocabulary be **data, not a Rust literal**: a
/// vessel's `Controls` profile (→ its `ControlBinding`) declares exactly which
/// command ports it accepts, and the strict command backend then admits writes to
/// those and no others. Additive (never removes keys) and idempotent, so it's safe to
/// run on `Changed<ControlBinding>` regardless of which reader stamped the binding or
/// the surface, and regardless of spawn order.
fn sync_command_surface(
    mut q: Query<
        (&lunco_core::ControlBinding, &mut CommandInputs),
        Changed<lunco_core::ControlBinding>,
    >,
) {
    for (binding, mut inputs) in q.iter_mut() {
        for port in binding.ports() {
            if !inputs.values.contains_key(port) {
                inputs.values.insert(port.to_string(), 0.0);
            }
        }
    }
}

// ── Drive mix ─────────────────────────────────────────────────────────────────

/// System allocating each rover's command inputs (`throttle`/`steer`/`brake`, read
/// from [`CommandInputs::values`]) to its actuator [`Port`]s (indexed by
/// [`ActuatorPorts`]), via the
/// vessel's data-selected [`DriveMix`] kernel (`skid`/`linear`/…, looked up in the
/// [`ControlKernelRegistry`]). No per-architecture branch: the kernel is chosen by
/// USD, and its outputs are saturated to `[-1, 1]` — ±100% actuator authority —
/// before being written to the port. Runs every fixed tick before wire propagation.
fn apply_drive_mix(
    mut q: Query<(Entity, &mut CommandInputs, &ActuatorPorts, &DriveMix)>,
    registry: Res<ControlKernelRegistry>,
    mut q_ports: Query<&mut Port>,
    mut unknown: Local<std::collections::HashSet<String>>,
) {
    for (entity, mut inputs, actuators, mix) in q.iter_mut() {
        // Read this vehicle's logical command inputs off the command surface.
        let throttle = inputs.cmd("throttle");
        let steer = inputs.cmd("steer");
        let brake = inputs.cmd("brake");
        // TWO DIFFERENT `"brake"`s meet here, deliberately in two components:
        //   - the COMMAND `brake` (`inputs.cmd("brake")`) is analog, in [-1,1];
        //   - the ACTUATOR `brake` (`actuators.get("brake")`) is a discretized
        //     1.0/0.0 gate the brake-coefficient ports consume.
        // Merging them into one map would silently feed the analog command straight
        // into the actuator register. Keeping them apart is what makes that impossible.
        //
        // Brake state (old `on_brake_rover`): engaged above half-scale. Locks the
        // wheel-spin/friction cone in the physics systems via `brake_active`.
        inputs.brake_active = brake > 0.5;
        let brake_port_val = if inputs.brake_active { 1.0 } else { 0.0 };
        if let Some(port_b) = actuators.get("brake") {
            if let Ok(mut p) = q_ports.get_mut(port_b) {
                p.value = brake_port_val;
            }
        }

        drive_diag!("[drive-diag] apply_drive_mix: target {:?} kernel={} throttle={} steer={} brake={} ports={:?}", entity, mix.kernel, throttle, steer, inputs.brake_active, actuators.ports);

        // While braking, force throttle/steer to 0 and drive the brake gate (1.0)
        // so brake-coefficient ports engage and drive ports zero out — matching the
        // old per-branch behaviour, now uniform across kernels.
        let drive_inputs = if inputs.brake_active {
            kernels::DriveInputs {
                throttle: 0.0,
                steer: 0.0,
                brake: 1.0,
            }
        } else {
            kernels::DriveInputs {
                throttle,
                steer,
                brake: 0.0,
            }
        };

        // Allocate command → normalized port writes. A built-in registry kernel
        // (`skid`/`linear`/…) wins; otherwise `mix.kernel` names a scripted (rhai)
        // drive kernel — a `lunco_hooks` hook that computes the per-port outputs
        // itself ("control policy in rhai", `lunco:driveKernel`). An unknown name
        // with no matching hook leaves the vessel un-actuated (fail-safe coast).
        let outputs = match registry.get(&mix.kernel) {
            Some(kernel) => kernel(drive_inputs, mix),
            None => {
                // Scripted kernel: hand the hook the vessel's real command surface
                // (`inputs.values`, un-gated — the script owns its brake policy), not the
                // built-in kernels' fixed throttle/steer/brake projection.
                let scripted = scripted_drive_mix(&mix.kernel, &inputs.values);
                if scripted.is_empty() && unknown.insert(mix.kernel.clone()) {
                    warn!("[apply_drive_mix] unknown drive kernel '{}' on {:?} — no built-in and no rhai hook; vessel not actuated", mix.kernel, entity);
                }
                scripted
            }
        };

        for (port, value) in outputs {
            if let Some(port_id) = actuators.get(&port) {
                if let Ok(mut p) = q_ports.get_mut(port_id) {
                    p.value = value.clamp(-1.0, 1.0);
                }
            }
        }
    }
}

/// Invoke a **scripted (rhai) drive kernel** by hook id. Hands the hook the vessel's
/// **actual command surface** — its declared [`CommandInputs::values`] map, keyed by
/// whatever ports that vehicle accepts (a rover's `throttle`/`steer`/`brake`, a
/// lander's `throttle`/`pitch`/`roll`/`yaw`, …) — NOT a fixed Rust key set. The
/// command vocabulary is data, so a scripted kernel reads exactly the ports the
/// vessel exposes and the script owns its own policy (incl. how `brake` gates).
/// Reads back a `port → value` map in `[-1, 1]` (clamped defensively). Empty on an
/// absent or faulted hook: **fail-safe** coast (ports left untouched — the brake
/// port + `brake_active` friction cone are already applied upstream regardless).
/// Host-side; a predicted client needs the identical hook, so the scripted-policy
/// plane (`lunco_networking`) distributes + registers it on every peer.
fn scripted_drive_mix(
    hook_id: &str,
    inputs: &std::collections::HashMap<String, f64>,
) -> Vec<(String, f64)> {
    use lunco_hooks::HookValue;
    let ctx = HookValue::map(
        inputs
            .iter()
            .map(|(k, v)| (k.clone(), HookValue::Float(*v))),
    );
    match lunco_hooks::invoke(hook_id, &[ctx]) {
        Some(Ok(HookValue::Map(entries))) => entries
            .into_iter()
            .filter_map(|(k, v)| v.as_f64().map(|f| (k, f.clamp(-1.0, 1.0))))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod proxy_wheel_mass_tests {
    //! The vehicle must mass the same whichever `drivetrain` variant realizes its
    //! wheels. See [`fold_proxy_wheel_mass`].
    use super::*;

    /// Build a skid-rover-shaped chassis with four proxy wheels at the mounts
    /// `skid_rover.usda` authors — (±1.0, −0.65, ±1.225), the SAME mounts the
    /// `physical` variant's wheel bodies get, because the wheel prim is the axle
    /// in both realizations. Runs the fold; returns (mass, inertia, centre of mass).
    fn fold_a_four_wheel_rover(wheel_mass: f64) -> (f32, Vec3, Vec3) {
        let mut app = App::new();
        app.add_systems(Update, fold_proxy_wheel_mass);

        let chassis = app
            .world_mut()
            .spawn((
                DriveMix::default(),
                Mass(1000.0),
                AngularInertia {
                    principal: Vec3::new(1028.0, 1354.0, 341.0),
                    ..default()
                },
                NoAutoAngularInertia,
            ))
            .id();

        for (x, z) in [(-1.0, -1.225), (1.0, -1.225), (-1.0, 1.225), (1.0, 1.225)] {
            let wheel = app
                .world_mut()
                .spawn((
                    WheelRaycast {
                        mass: wheel_mass,
                        wheel_radius: 0.4,
                        ..default()
                    },
                    Transform::from_translation(Vec3::new(x, -0.65, z)),
                    ChildOf(chassis),
                ))
                .id();
            let _ = wheel;
        }

        app.update();

        let mass = app.world().get::<Mass>(chassis).unwrap().0;
        let inertia = app
            .world()
            .get::<AngularInertia>(chassis)
            .unwrap()
            .principal;
        let com = app
            .world()
            .get::<CenterOfMass>(chassis)
            .map(|c| c.0)
            .unwrap_or(Vec3::ZERO);
        (mass, inertia, com)
    }

    #[test]
    fn a_raycast_rover_masses_the_same_as_its_physical_twin() {
        // The physical twin is chassis 1000 kg + four 25 kg wheel bodies. The
        // raycast rover's wheels are kinematic proxies avian never weighs, so
        // without the fold the same USD file massed 1000 kg — a 10% vehicle
        // change caused by nothing but a variant switch.
        let (mass, _, _) = fold_a_four_wheel_rover(25.0);
        assert!((mass - 1100.0).abs() < 1e-3, "expected 1100 kg, got {mass}");
    }

    #[test]
    fn the_mass_acts_where_the_wheels_hang_it() {
        // A physical rover's four wheel bodies hang at the axle and PULL THE
        // COMBINED CENTRE OF MASS DOWN — avian does that arithmetic for free
        // because they are bodies. The raycast rover's proxies are not, so its
        // mass kept acting at the chassis centre: same total, same tensor, wrong
        // place. CoM height is load transfer, so the two rovers would still have
        // cornered differently with every other number matched.
        //
        //   (1000·0 + 4·25·(−0.65)) / 1100 = −65/1100 = −0.0590909… m
        let (_, _, com) = fold_a_four_wheel_rover(25.0);
        assert!(
            (com.y as f64 + 65.0 / 1100.0).abs() < 1e-6,
            "expected CoM y = -0.0590909, got {}",
            com.y
        );
        // x and z must cancel: the mounts are symmetric (±1.0, ±1.225), and a
        // rover whose mass drifted sideways would pull in a straight line.
        assert!(
            com.x.abs() < 1e-6 && com.z.abs() < 1e-6,
            "symmetric mounts must cancel, got {com:?}"
        );
    }

    #[test]
    fn the_wheels_arrive_at_their_mounts_not_at_the_centre_of_mass() {
        // Mass alone was measured and made the suite WORSE (heading 56.3° → 61.7°
        // against a physical twin at 51°): a heavier rover that was no harder to
        // turn. Each wheel must bring its parallel-axis term `m·d²` at its
        // authored mount, which grows the yaw tensor FASTER than the mass.
        let (mass, inertia, _) = fold_a_four_wheel_rover(25.0);

        // Measured from the COMBINED centre (y = −0.0590909), not from the body
        // origin — the authored tensor is about the chassis centre and that centre
        // has just moved, so the chassis contributes a term of its own.
        //   chassis: 1000·(0.0590909²) = 3.4917 about x and z, 0 about y
        //   wheel at (±1.0, −0.5909091, ±1.225): m·(y²+z²), m·(x²+z²), m·(x²+y²)
        //     = 25·(0.349174 + 1.500625), 25·(1.0 + 1.500625), 25·(1.0 + 0.349174)
        //     = 46.2450, 62.5156, 33.7293   → ×4 → 184.980, 250.063, 134.917
        let expected = Vec3::new(
            1028.0 + 3.4917355 + 184.97986,
            1354.0 + 250.0625,
            341.0 + 3.4917355 + 134.91736,
        );
        assert!(
            (inertia - expected).abs().max_element() < 1e-2,
            "expected {expected:?}, got {inertia:?}"
        );

        // The point of the whole exercise: yaw inertia must rise FASTER than mass,
        // or the rover gets heavier without getting harder to turn.
        let mass_ratio = mass / 1000.0;
        let yaw_ratio = inertia.y / 1354.0;
        assert!(
            yaw_ratio > mass_ratio,
            "yaw inertia grew {yaw_ratio:.4}× but mass grew {mass_ratio:.4}×"
        );
    }

    #[test]
    fn a_wheel_whose_parameters_have_not_landed_yet_defers_the_fold() {
        // `WheelParams::apply_to_raycast` can land a tick after the component. A
        // wheel still reading zero mass means the vehicle is not ready — folding
        // then would permanently pin the rover at a fraction of its real mass.
        let (mass, inertia, com) = fold_a_four_wheel_rover(0.0);
        assert_eq!(mass, 1000.0, "folded before the wheel parameters arrived");
        assert_eq!(inertia, Vec3::new(1028.0, 1354.0, 341.0));
        assert_eq!(
            com,
            Vec3::ZERO,
            "centre of mass moved before the fold was due"
        );
    }

    #[test]
    fn folding_twice_does_not_double_the_rover() {
        let mut app = App::new();
        app.add_systems(Update, fold_proxy_wheel_mass);
        let chassis = app
            .world_mut()
            .spawn((DriveMix::default(), Mass(1000.0)))
            .id();
        app.world_mut().spawn((
            WheelRaycast {
                mass: 25.0,
                ..default()
            },
            Transform::default(),
            ChildOf(chassis),
        ));

        app.update();
        app.update();
        app.update();

        assert!((app.world().get::<Mass>(chassis).unwrap().0 - 1025.0).abs() < 1e-3);
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
        assert!(
            f.dot(n).abs() < 1e-9,
            "forward not in contact plane: {}",
            f.dot(n)
        );
        assert!(
            r.dot(n).abs() < 1e-9,
            "right not in contact plane: {}",
            r.dot(n)
        );
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

    // (drive-mix parse + kernel projection now live in `kernels`.)

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
                let t = args[0]
                    .get("throttle")
                    .and_then(HookValue::as_f64)
                    .unwrap_or(0.0);
                let s = args[0]
                    .get("steer")
                    .and_then(HookValue::as_f64)
                    .unwrap_or(0.0);
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

        let inputs: HashMap<String, f64> = [
            ("throttle".into(), 1.0),
            ("steer".into(), 0.5),
            ("brake".into(), 0.0),
        ]
        .into();
        let mut out = scripted_drive_mix("test.kernel.tank", &inputs);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        // t+s = 1.5 → clamped to 1.0; t-s = 0.5.
        assert_eq!(
            out,
            vec![
                ("drive_left".to_string(), 1.0),
                ("drive_right".to_string(), 0.5)
            ]
        );

        // Absent hook → empty (fail-safe coast; ports left untouched).
        assert!(scripted_drive_mix("test.kernel.absent", &inputs).is_empty());

        lunco_hooks::unregister("test.kernel.tank");
    }

    // ── suspension_force_mag: bounded, never negative, damps both ways ───────
    #[test]
    fn suspension_force_is_nonnegative_and_bounded() {
        let (k, c) = (8000.0, 2800.0);
        let x = 0.05;
        let spring = x * k;
        assert!(
            suspension_force_mag(x, k, -1000.0, c) >= 0.0,
            "ground can't pull"
        );
        assert!(
            suspension_force_mag(x, k, 1000.0, c) <= 2.0 * spring + 1e-9,
            "bounded"
        );
        assert!(
            (suspension_force_mag(x, k, 0.0, c) - spring).abs() < 1e-9,
            "at rest = spring"
        );
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
        if chi > 0.0 {
            k * chi + c * chi_dot
        } else {
            0.0
        }
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
        println!(
            "[oracle] gentle: max|χ_rust−χ_ref| = {dev:.5} m, χ_end = {:.4} (eq {chi_eq:.4})",
            rust.last().unwrap()
        );
        assert!(
            dev < 8.0e-3,
            "fixed law diverges from continuous reference: {dev} m"
        );
        assert!(
            (rust.last().unwrap() - chi_eq).abs() < 2.0e-3,
            "must settle at m·g/k"
        );
    }

    #[test]
    fn fixed_law_settles_no_limit_cycle() {
        // Under-damped config (c small → clear ringing) must still DECAY. The
        // dead-band / `.max(0)` bugs produced a sustained tick-period limit-cycle;
        // assert the late window is quiet relative to the early one.
        let (k, c) = (8000.0, 400.0);
        let (rust, _f) = step_law(fixed(k, c), 0.15, 0.0, 5.0);
        let win = rust.len() / 5;
        let p2p = |s: &[f64]| {
            s.iter().cloned().fold(f64::MIN, f64::max) - s.iter().cloned().fold(f64::MAX, f64::min)
        };
        let early = p2p(&rust[..win]);
        let late = p2p(&rust[rust.len() - win..]);
        println!("[oracle] settle: early p2p {early:.4} m, late p2p {late:.5} m");
        assert!(
            late < 0.15 * early,
            "ringing must decay (limit-cycle guard): {late} vs {early}"
        );
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
        assert!(
            sb > 3.0 * sf,
            "cliff spikes the impact ({sb} N) far past the bounded force ({sf} N)"
        );
        assert!(
            sb > 20_000.0,
            "cliff lets a >20 kN landing transient through"
        );
    }
}

#[cfg(test)]
mod differential_tests {
    use super::*;

    /// A representative constraint inverse inertia and fixed step for the pure
    /// algebra tests below. `w > 0` so the implicit scaling is genuinely exercised
    /// rather than degenerating to the explicit form.
    const W: f64 = 0.05;
    const DT: f64 = 1.0 / 60.0;

    /// A geared pair satisfies `θ_a = r·θ_b + rest_offset` exactly ⇒ no correction.
    #[test]
    fn a_satisfied_gear_needs_no_torque() {
        for (r, a, b) in [
            (-1.0, 0.2, -0.2),
            (1.0, 0.4, 0.4),
            (2.0, 0.6, 0.3),
            (-0.5, 0.25, -0.5),
        ] {
            let lambda = differential_lambda(a, b, 0.0, 0.0, r, 1000.0, 100.0, W, DT);
            assert!(
                lambda.abs() < 1e-12,
                "ratio {r}: satisfied gear pulled {lambda}"
            );
        }
    }

    /// The ratio is what the constraint MEANS: the same pair of angles is an error
    /// for one ratio and satisfied by another. Before the authored
    /// `physxGearJoint:gearRatio` was threaded through, every gear ran as `-1`
    /// regardless of what the scene said, so this distinction did not exist.
    #[test]
    fn the_ratio_decides_what_counts_as_error() {
        let (a, b, k, d) = (0.4, 0.4, 1000.0, 0.0);
        // r = -1 (mirror): c = θ_a + θ_b = 0.8 → in error.
        assert!(differential_lambda(a, b, 0.0, 0.0, -1.0, k, d, W, DT).abs() > 1.0);
        // r = +1 (co-rotating): c = θ_a − θ_b = 0 → satisfied.
        assert!(differential_lambda(a, b, 0.0, 0.0, 1.0, k, d, W, DT).abs() < 1e-9);
    }

    /// `rest_offset` shifts the target: `c = θ_a − r·θ_b − rest_offset`.
    #[test]
    fn rest_offset_moves_the_target() {
        let k = 1000.0;
        // θ_a + θ_b = 0.5, and the gear is authored to want exactly that.
        let at_rest = differential_lambda(0.3, 0.2, 0.0, 0.5, -1.0, k, 0.0, W, DT);
        assert!(
            at_rest.abs() < 1e-12,
            "offset target should be satisfied, got {at_rest}"
        );
    }

    /// Damping opposes constraint-rate even at zero positional error.
    #[test]
    fn damping_opposes_constraint_rate() {
        let lambda = differential_lambda(0.0, 0.0, 0.5, 0.0, -1.0, 1000.0, 100.0, W, DT);
        // τ_a = −λ must oppose a positive rate.
        assert!(-lambda < 0.0, "damping did not oppose the rate");
    }

    /// THE WHOLE POINT OF THE IMPLICIT SOLVE: no stiffness diverges.
    ///
    /// The explicit form `k·c + d·ċ` grows without bound in `k`, so a stiff gear
    /// overshoots and the rig explodes — measured on the real vehicle, where
    /// k = 30 000 and 60 000 both reached NaN inside 10 s. The correction here is
    /// bounded by the constraint's own inertia instead.
    #[test]
    fn stiffness_cannot_diverge() {
        let mut previous = 0.0;
        for k in [1e3, 1e4, 1e5, 1e6, 1e9, 1e15] {
            let lambda = differential_lambda(0.2, 0.2, 0.0, 0.0, -1.0, k, 100.0, W, DT);
            assert!(lambda.is_finite(), "k = {k} produced {lambda}");
            // Monotone in k, and converging rather than running away.
            assert!(lambda >= previous - 1e-9, "k = {k} went backwards");
            previous = lambda;
        }
        // The ceiling is the exact one-step constraint impulse, (c/dt + ċ)/(w·dt).
        let c: f64 = 0.4;
        let ceiling = (c / DT) / (W * DT);
        assert!(
            previous <= ceiling * (1.0 + 1e-6),
            "k → ∞ gave {previous}, above the holonomic impulse {ceiling}"
        );
        assert!(
            previous > ceiling * 0.999,
            "k → ∞ gave {previous}, short of the holonomic impulse {ceiling} — \
             the constraint is not being reached"
        );
    }

    /// A HOLONOMIC gear is mass-independent: the same authored stiffness must
    /// mirror the rockers the same way whatever the vehicle weighs.
    ///
    /// This is the property the explicit penalty lacked, and the reason
    /// `rocker_bogie.usda`'s hull mass was pinned at 300 kg — at 400 kg the same
    /// `k` left a 20% residual. With `w` in the solve, a heavier rig (smaller `w`,
    /// since `w` is an INVERSE inertia) takes proportionally more torque, and at a
    /// stiffness in the constraint regime the resulting angular correction is the
    /// same. Compare the corrective ACCELERATION `λ·w`, which is what actually
    /// moves the constraint.
    #[test]
    fn a_stiff_gear_corrects_the_same_at_any_mass() {
        let k = 1e9; // constraint regime
        let accel = |w: f64| differential_lambda(0.15, 0.15, 0.0, 0.0, -1.0, k, 1500.0, w, DT) * w;
        let light = accel(0.08);
        let heavy = accel(0.02); // 4× the inertia
        assert!(
            (light - heavy).abs() / light < 1e-3,
            "4x inertia changed the correction {light} → {heavy}; the gear is still \
             mass-dependent, so the vehicle's mass budget is still pinned by it"
        );
    }

    /// Angular momentum: the three generalized torques must sum to zero about the
    /// axis for ANY ratio — otherwise the coupling injects spin into the rig.
    #[test]
    fn reaction_conserves_angular_momentum_at_any_ratio() {
        for r in [-2.5, -1.0, -0.5, 0.5, 1.0, 3.0] {
            let lambda = differential_lambda(0.3, -0.1, 0.7, 0.02, r, 5000.0, 100.0, W, DT);
            let tau_a = -lambda;
            let tau_b = lambda * r;
            let chassis = lambda * (1.0 - r);
            assert!(
                (tau_a + tau_b + chassis).abs() < 1e-9,
                "ratio {r}: torques sum to {}, not zero",
                tau_a + tau_b + chassis
            );
        }
    }
}

/// Marker component added to an entity representing the suspension piston visual.
/// Stores the initial Y coordinate so that we can offset it relative to the wheel's
/// visual displacement.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct SuspensionPiston {
    pub initial_y: f32,
}

/// Marker component added to an entity representing the suspension spring visual.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct SuspensionSpring;

/// Animate USD-authored visual suspension components (casing, piston, spring)
/// based on the raycast wheel's dynamic suspension compression.
///
/// The `SuspensionPiston` / `SuspensionSpring` marker components are stamped at
/// LOAD time by `process_usd_sim_prim_read` (lunco-usd-sim) from the prim's
/// authored `lunco:suspensionVisual:role` token — NOT detected here by name and
/// NOT lazy-attached. This system only reads the markers and
/// translates/scales the visuals along the Y-axis relative to their rest
/// positions. See `assets/components/mobility/suspensions/standard.usda`.
fn update_suspension_visuals(
    q_wheels: Query<(&WheelRaycast, &Suspension, &RayHits, Option<&Children>)>,
    mut q_piston: Query<(&mut Transform, &SuspensionPiston), Without<WheelRaycast>>,
    mut q_spring: Query<
        &mut Transform,
        (
            With<SuspensionSpring>,
            Without<WheelRaycast>,
            Without<SuspensionPiston>,
        ),
    >,
) {
    for (wheel, susp, hits, children) in q_wheels.iter() {
        let Some(children) = children else {
            continue;
        };

        let mut current_distance = susp.rest_length;
        if let Some(hit) = hits.iter_sorted().next() {
            if hit.distance < susp.rest_length {
                current_distance = hit.distance;
            }
        }

        // Hub and strut top in WHEEL-LOCAL Y (the wheel prim is the axle): the hub
        // rises by the compression, the top is fixed at `strut_offset`.
        let hub_y = susp.rest_length - current_distance;
        let top_y = strut_offset(susp.rest_length, wheel.wheel_radius);
        let delta_y = susp.rest_length - current_distance;

        for child in children.iter() {
            if Some(child) == wheel.visual_entity {
                continue;
            }

            if let Ok((mut tf, piston)) = q_piston.get_mut(child) {
                tf.translation.y = (piston.initial_y as f64 + delta_y) as f32;
            } else if let Ok(mut tf) = q_spring.get_mut(child) {
                let rest_susp_length = strut_offset(susp.rest_length, wheel.wheel_radius);
                if rest_susp_length > 1e-4 {
                    let current_susp_length = (current_distance - wheel.wheel_radius).max(0.0);
                    let scale_y = (current_susp_length / rest_susp_length) as f32;
                    tf.scale.y = scale_y;
                    // The coil spans hub → strut top, so it sits at their midpoint.
                    tf.translation.y = ((hub_y + top_y) / 2.0) as f32;
                }
            }
        }
    }
}

#[cfg(test)]
mod suspension_visuals_tests {
    use super::*;
    use avian3d::dynamics::integrator::VelocityIntegrationData;
    use avian3d::prelude::forces::AccumulatedLocalAcceleration;

    #[test]
    fn test_suspension_visuals_are_animated() {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(std::time::Duration::from_secs_f64(0.1));
        app.insert_resource(time);

        let chassis = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Position(DVec3::ZERO),
                Rotation::default(),
                LinearVelocity(DVec3::ZERO),
                AngularVelocity(DVec3::ZERO),
                ComputedMass::default(),
                ComputedAngularInertia::default(),
                ComputedCenterOfMass::default(),
                VelocityIntegrationData::default(),
                AccumulatedLocalAcceleration::default(),
                DriveMix::default(),
            ))
            .id();

        let visual = app.world_mut().spawn(Transform::default()).id();

        // Markers are pre-spawned here to test the ANIMATION logic in isolation. In
        // the real app they are stamped at load by `process_usd_sim_prim_read`
        // (lunco-usd-sim) from the prim's `lunco:suspensionVisual:role` token —
        // this test does not exercise that load path.
        // Rest positions as `suspensions/standard.usda` authors them: the wheel prim
        // is the AXLE, so the strut rises from it (restLength 0.7 − radius 0.4 =
        // 0.3 m of strut) and the piston/spring sit ABOVE the prim, not below.
        let piston = app
            .world_mut()
            .spawn((
                SuspensionPiston { initial_y: 0.1 },
                Transform::from_translation(Vec3::new(0.0, 0.1, 0.0)),
            ))
            .id();

        let spring = app
            .world_mut()
            .spawn((
                SuspensionSpring,
                Transform::from_translation(Vec3::new(0.0, 0.15, 0.0)),
            ))
            .id();

        let wheel = app
            .world_mut()
            .spawn((
                WheelRaycast {
                    suspension_port: Entity::PLACEHOLDER,
                    drive_port: Entity::PLACEHOLDER,
                    steer_port: Entity::PLACEHOLDER,
                    wheel_radius: 0.4,
                    visual_entity: Some(visual),
                    ..default()
                },
                Suspension {
                    rest_length: 0.7,
                    spring_k: 1000.0,
                    damping_c: 100.0,
                    local_axis: DVec3::Y,
                },
                Transform::default(),
                RayHits(vec![RayHitData {
                    entity: chassis,
                    distance: 0.5,
                    normal: DVec3::Y,
                }]),
                ChildOf(chassis),
            ))
            .id();

        app.world_mut().entity_mut(wheel).add_child(visual);
        app.world_mut().entity_mut(wheel).add_child(piston);
        app.world_mut().entity_mut(wheel).add_child(spring);

        app.add_systems(
            Update,
            (apply_wheel_suspension, update_suspension_visuals).chain(),
        );
        app.update(); // Frame 1: animates transforms (markers pre-spawned above)

        // 1. The hub rises by the COMPRESSION, because the prim is the axle: the ray
        // starts 0.3 m above it and hit at 0.5, so the strut is packed 0.2 m up.
        // rest_length - distance = 0.7 - 0.5 = 0.2
        let visual_tf = app.world().get::<Transform>(visual).unwrap();
        assert!(
            (visual_tf.translation.y - 0.2f32).abs() < 1e-6,
            "hub at {} , expected the 0.2 m compression",
            visual_tf.translation.y
        );

        // 2. Piston translated to initial_y + delta_y.
        // delta_y = rest_length - distance = 0.7 - 0.5 = 0.2
        // initial_y = 0.1, so current Y = 0.1 + 0.2 = 0.3 — it rides with the hub.
        let piston_tf = app.world().get::<Transform>(piston).unwrap();
        assert!((piston_tf.translation.y - 0.3f32).abs() < 1e-6);

        // 3. Spring scale Y = (distance - radius) / (rest_length - radius)
        // = (0.5 - 0.4) / (0.7 - 0.4) = 0.1 / 0.3 = 0.3333333
        // and it sits midway between the hub (0.2) and the fixed strut top (0.3).
        let spring_tf = app.world().get::<Transform>(spring).unwrap();
        assert!((spring_tf.scale.y - 0.3333333f32).abs() < 1e-5);
        assert!((spring_tf.translation.y - 0.25f32).abs() < 1e-6);
    }
}
