//! # Surface Mobility & Traction Physics
//!
//! This crate implements the core physics models for planetary rovers and
//! surface exploration vehicles.
//!
//! ## Ground interaction realizations
//! Traditional mesh-to-mesh collision for wheels is computationally expensive
//! and prone to "snagging" on terrain geometry. The crate supports two
//! realizations behind one authored tire law: raycast wheels use an analytical
//! suspension/contact point, while jointed wheels use Avian for normal contact
//! and the revolute constraint.
//! 1. **Suspension Logic**: An emulated spring-damper system computes normal
//!    forces based on ray length, preventing high-frequency jitter.
//! 2. **Traction Physics**: Both realizations apply the same longitudinal and
//!    lateral tire equations; only the source of the normal load and contact
//!    point differs.
//! 3. **Numeric Stability**: The raycast realization projects a single ray to
//!    avoid wheel snagging on irregular procedural terrain.
//!
//! ## Control Mixing Models
//! The crate supports hotswappable steering architectures:
//! - **Differential (Skid) Drive**: Common for heavy loaders and excavators;
//!   turns by varying velocity between left and right tracks.
//! - **Ackermann Steering**: Standard for high-speed mobility; pivots leading
//!   wheels to maintain a common center of rotation, reducing tire scrub.

use avian3d::dynamics::solver::solver_body::{SolverBody, SolverBodyInertia};
use avian3d::dynamics::solver::xpbd::{solve_xpbd_joint, XpbdSolverSystems};
use avian3d::prelude::*;
use bevy::ecs::schedule::common_conditions::any_with_component;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use kernels::{ControlKernelRegistry, DriveMix};
use lunco_core::architecture::Port;
use lunco_core::coords::{GridPos, GridRot};
use lunco_core::{safe_stop_control_surface, ActuatorPorts, InputPorts};
use std::collections::HashSet;

mod jointed_tire;
/// Control kernels live here rather than in core (see the nothing-into-core
/// rule).
pub mod kernels;
mod sensing;
mod wheel_spin;
pub use jointed_tire::{apply_jointed_tire_forces, JointedWheelTire};
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

fn mark_wheel_ports_causal(
    trigger: On<Add, WheelRaycast>,
    query: Query<&WheelRaycast>,
    mut commands: Commands,
) {
    let Ok(wheel) = query.get(trigger.entity) else {
        return;
    };
    for port in [wheel.drive_port, wheel.steer_port] {
        if port != Entity::PLACEHOLDER {
            commands
                .entity(port)
                .try_insert(lunco_core::CausalStateSink);
        }
    }
}

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
            .register_type::<JointedWheelTire>()
            // `DriveMix` is the kernel-selected allocation spec. Registered
            // here with the kernels it selects between; it is a vehicle-domain
            // type and core carries no domain.
            .register_type::<DriveMix>()
            .register_type::<DifferentialCoupling>()
            .register_type::<SteerBaseRotation>()
            .register_type::<SuspensionPiston>()
            .register_type::<SuspensionSpring>()
            .register_type::<ProxyWheelMassFolded>()
            .add_observer(mark_wheel_ports_causal)
            // A vehicle's mass must not depend on which `drivetrain` variant
            // realizes its wheels. Ungated: this is a one-shot mass-property
            // correction per chassis, not a force, so it must land even while
            // physics is held — a rover that spawns during a cinematic hold is
            // still the same rover.
            .add_systems(FixedUpdate, fold_proxy_wheel_mass)
            // A raycast suspension is a physics model without Avian colliders.
            // Publish its support geometry through the physics contract once the
            // USD-to-mobility projection has created the wheel entities. Terrain
            // consumes only that contract and never inspects mobility types.
            .add_systems(Update, publish_raycast_support_footprints)
            // G5 rocker-bogie differential — separate set: it doesn't read the
            // control ports, only couples two rocker hinges. Idle unless a
            // `DifferentialCoupling` exists, so it's free for every other vehicle.
            .add_systems(
                // The differential is a substep constraint. It runs before the
                // native revolute pass so native hinge alignment and limits
                // close the same XPBD iteration.
                SubstepSchedule,
                solve_differential_gear
                    .in_set(XpbdSolverSystems::SolveConstraints)
                    .after(solve_xpbd_joint::<FixedJoint>)
                    .before(solve_xpbd_joint::<RevoluteJoint>)
                    .run_if(any_with_component::<DifferentialCoupling>)
                    // Same live-physics gate as the wheel systems: a frozen scene
                    // must not have its linkage projected while it is mounting.
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
                    apply_jointed_tire_forces,
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
        // Own the control-allocation kernel registry here (the plugin that runs
        // `apply_drive_mix`), seeded with the built-in `skid`/`linear` kernels —
        // so any app running the drive systems has it, without depending on the
        // full core plugin. Flight-kernel crates register additively the same way.
        if !app.world().contains_resource::<ControlKernelRegistry>() {
            app.insert_resource(ControlKernelRegistry::with_defaults());
        }

        // Mix the FSW's logical input ports (written via the shared port backend) into
        // the actuator command `Port`s BEFORE propagation carries them across the
        // wires (and before the wheel systems, which run
        // `.after(ControlDacSet)`). The
        // input surface is derived from USD `Controls` bindings (never a Rust
        // literal) by `sync_input_ports`, ordered before the mix so a
        // freshly-loaded vessel is drivable the same tick its binding lands.
        app.add_systems(
            FixedUpdate,
            (sync_input_ports, apply_vehicle_brake, apply_drive_mix)
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
            (sync_input_ports, apply_vehicle_brake, apply_drive_mix)
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

/// Resolve a wheel's authored carrier chain to the vehicle's actuator owner.
///
/// A wheel can be a direct child of the vessel root or hang from an articulated
/// link such as a rocker or bogie.  The mobility systems that apply forces use
/// the immediate rigid-body carrier; mass and initial-support publication need
/// the inverse mapping so their body-local offsets can be expressed in the
/// vessel owner's frame.  The authored `ChildOf` chain is the topology source;
/// no vehicle-specific path spelling belongs here.
fn actuator_root_and_local_transform(
    wheel: Entity,
    roots: &Query<Entity, With<ActuatorPorts>>,
    parents: &Query<&ChildOf>,
    transforms: &Query<&Transform>,
) -> Option<(Entity, Transform)> {
    let mut current = wheel;
    let mut chain = Vec::new();
    let mut visited = HashSet::new();

    loop {
        if roots.get(current).is_ok() {
            let mut local = Transform::IDENTITY;
            for transform in chain.iter().rev() {
                local = local.mul_transform(*transform);
            }
            return Some((current, local));
        }
        if !visited.insert(current) {
            return None;
        }
        chain.push(transforms.get(current).copied().unwrap_or_default());
        current = parents.get(current).ok()?.parent();
    }
}

/// Publish the support envelope of the raycast physics realization.
///
/// This is intentionally owned by mobility: it knows the contact model and its
/// wheel parameters. The published component is owned by `lunco-physics`, so
/// terrain, readiness, and future support consumers do not depend on rover
/// components. Collider-backed bodies need no publisher; Avian supplies their
/// support geometry through runtime collider AABBs.
fn publish_raycast_support_footprints(
    mut commands: Commands,
    roots: Query<
        Entity,
        (
            With<ActuatorPorts>,
            Without<lunco_physics::PhysicsSupportFootprint>,
        ),
    >,
    actuator_roots: Query<Entity, With<ActuatorPorts>>,
    wheels: Query<(Entity, &WheelRaycast, &Suspension)>,
    parents: Query<&ChildOf>,
    transforms: Query<&Transform>,
) {
    for root in roots.iter() {
        let contacts = wheels
            .iter()
            .filter_map(|(wheel_entity, wheel, suspension)| {
                let (owner, local) = actuator_root_and_local_transform(
                    wheel_entity,
                    &actuator_roots,
                    &parents,
                    &transforms,
                )?;
                if owner != root || !wheel.wheel_radius.is_finite() || wheel.wheel_radius <= 0.0 {
                    return None;
                }
                Some(lunco_physics::PhysicsSupportContact {
                    local_offset: local.translation.as_dvec3(),
                    radius: wheel.wheel_radius,
                    probe_origin: local.translation.as_dvec3()
                        + local.rotation.as_dquat()
                            * DVec3::Y
                            * strut_offset(suspension.rest_length, wheel.wheel_radius),
                    probe_direction: local.rotation.as_dquat() * DVec3::NEG_Y,
                    probe_length: suspension.rest_length,
                })
            })
            .collect::<Vec<_>>();
        if !contacts.is_empty() {
            commands.entity(root).try_insert((
                lunco_physics::PhysicsSupportFootprint(contacts),
                // A probe-only support model has no low rigid body for the
                // generic activation path to place. Request the shared
                // one-shot initial placement; this is not part of per-frame
                // ring selection and is consumed by terrain after readiness.
                lunco_core::NeedsGroundSettle,
            ));
        }
    }
}

/// Fold the proxy wheels' authored mass onto the chassis rigid body.
///
/// A ROVER'S MASS IS A PROPERTY OF THE ROVER, NOT OF HOW ITS WHEELS ARE REALIZED.
/// The same `skid_rover.usda` composed with `drivetrain = physical` masses 1100 kg
/// (chassis 1000 + four 25 kg wheel bodies avian integrates in their own right),
/// and with `drivetrain = raycast` massed 1000 kg — the proxy wheels are kinematic,
/// so avian never saw their authored `physxVehicleWheel:mass` at all. One variant switch
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
    q_chassis: Query<Entity, (With<ActuatorPorts>, Without<ProxyWheelMassFolded>)>,
    q_roots: Query<Entity, With<ActuatorPorts>>,
    q_wheels: Query<(Entity, &WheelRaycast)>,
    q_parents: Query<&ChildOf>,
    q_transforms: Query<&Transform>,
    mut q_body: Query<(
        &mut Mass,
        Option<&mut AngularInertia>,
        Has<NoAutoAngularInertia>,
        Option<&CenterOfMass>,
        Option<&ComputedCenterOfMass>,
    )>,
) {
    let mut wheels_by_chassis = std::collections::HashMap::<Entity, Vec<(f64, DVec3)>>::new();
    for (wheel_entity, wheel) in q_wheels.iter() {
        let Some((chassis, local)) =
            actuator_root_and_local_transform(wheel_entity, &q_roots, &q_parents, &q_transforms)
        else {
            continue;
        };
        wheels_by_chassis
            .entry(chassis)
            .or_default()
            .push((wheel.mass, local.translation.as_dvec3()));
    }

    for chassis in q_chassis.iter() {
        // The wheel's `mass` arrives from `WheelParams::apply_to_raycast`, which may
        // land a frame after the component itself. A wheel still reading zero means
        // the vehicle is not ready to fold and must be left for a later tick — never
        // folded at half its mass. Descendant wheels are grouped by their actuator
        // owner, so articulated rocker/bogie wheels receive the same mass treatment
        // as direct-child wheels.
        let Some(wheels) = wheels_by_chassis.get(&chassis) else {
            continue;
        };
        let pending = wheels.iter().any(|(mass, _)| *mass <= 0.0);
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
            for (m, d) in wheels {
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
            for (m, d) in wheels {
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
pub fn contact_plane_basis(
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

/// Resolve a longitudinal tire demand and lateral slip into one Coulomb-limited
/// contact-patch force.
///
/// `cornering_stiffness` is the normalized PhysX value (side force per radian,
/// per newton of normal load). `rolling_reference_speed` is derived by the wheel
/// realization as `max(|v_hub,long|, |omega*r|)`: the actual longitudinal motion
/// available to define a slip angle, including a counter-rotating pivot wheel.
/// There is no fitted low-speed threshold. If translation and rotation are both
/// zero while the patch moves sideways, the angle tends to 90 degrees and the
/// ordinary Coulomb limit supplies static lateral resistance.
///
/// The final pair is scaled onto the single Coulomb cone without changing its
/// direction. Both wheel realizations call this function: raycast wheels apply
/// its result to the chassis, while jointed wheels apply it to the wheel body at
/// Avian's saved contact point. The Avian bridge retains the normal constraint
/// but disables its generic tangent impulse for jointed tire contacts, so this
/// remains the only tire model.
pub fn tire_patch_force(
    longitudinal_force: f64,
    rolling_reference_speed: f64,
    lateral_speed: f64,
    normal_force: f64,
    friction_mu: f64,
    cornering_stiffness: f64,
) -> (f64, f64) {
    if normal_force <= 0.0 {
        return (0.0, 0.0);
    }

    let lateral_force = if cornering_stiffness > 0.0 {
        let slip_angle = (-lateral_speed).atan2(rolling_reference_speed.abs());
        cornering_stiffness * normal_force * slip_angle
    } else {
        0.0
    };

    let cone = friction_mu.max(0.0) * normal_force;
    let magnitude = longitudinal_force.hypot(lateral_force);
    if magnitude > cone && magnitude > f64::EPSILON {
        let scale = cone / magnitude;
        (longitudinal_force * scale, lateral_force * scale)
    } else {
        (longitudinal_force, lateral_force)
    }
}

/// Static friction demanded by a locked wheel at one contact patch.
///
/// `normal_force` is the suspension load for this wheel, so the gravity share
/// is independent of wheel count.  Clamping to the authored Coulomb cone keeps
/// an over-steep slope physically free to slide instead of turning the parking
/// brake into an invisible constraint.
fn parking_brake_force(
    gravity: DVec3,
    contact_normal: DVec3,
    normal_force: f64,
    friction_mu: f64,
) -> DVec3 {
    if normal_force <= 0.0 || friction_mu <= 0.0 || !gravity.is_finite() {
        return DVec3::ZERO;
    }
    let normal = contact_normal.normalize_or_zero();
    if normal == DVec3::ZERO {
        return DVec3::ZERO;
    }
    let tangent_gravity = gravity - normal * gravity.dot(normal);
    let tangent_len = tangent_gravity.length();
    // At static equilibrium N = m * |g_normal|, therefore this contact's
    // supported mass share is N / |g_normal|. Using |g| here underestimates the
    // required hold force by cos(slope), guaranteeing slow downhill creep on
    // every non-flat surface even with the parking brake fully engaged.
    let normal_accel = -gravity.dot(normal);
    if normal_accel <= f64::EPSILON || tangent_len <= f64::EPSILON {
        return DVec3::ZERO;
    }
    let requested = normal_force * tangent_len / normal_accel;
    let available = friction_mu.max(0.0) * normal_force;
    -tangent_gravity / tangent_len * requested.min(available)
}

/// Advance the analytic raycast longitudinal tire/axle solve by one fixed step.
///
/// The return value is `(new_axle_speed, longitudinal_patch_force)`.  Both the
/// raycast wheel's spin and patch-force calculations use this exact relationship.
/// A jointed wheel uses Avian only for normal contact and the revolute
/// constraint; its tangential force and axle update call the same equations as
/// the raycast wheel. The solver's accumulated normal impulse supplies the
/// load, so no second lateral/friction law or physical-only coefficient exists.
pub fn longitudinal_tire_step(
    axle_speed: f64,
    hub_speed: f64,
    radius: f64,
    inertia: f64,
    slip_stiffness: f64,
    bearing_damping: f64,
    drive_torque: f64,
    brake_torque: f64,
    normal_force: f64,
    friction_mu: f64,
    dt: f64,
) -> (f64, f64) {
    if dt <= 0.0 || inertia <= 0.0 || radius <= 0.0 {
        return (axle_speed, 0.0);
    }
    if normal_force < 1.0 {
        let speed = axle_speed
            + dt * (drive_torque + brake_torque - bearing_damping * axle_speed) / inertia;
        return (speed, 0.0);
    }

    let denom = inertia / dt + slip_stiffness * radius * radius + bearing_damping;
    let w_grip = (inertia / dt * axle_speed
        + drive_torque
        + brake_torque
        + slip_stiffness * radius * hub_speed)
        / denom.max(1.0e-12);
    let f_slip = slip_stiffness * (w_grip * radius - hub_speed);
    let mu_n = friction_mu.max(0.0) * normal_force;
    if f_slip.abs() <= mu_n {
        (w_grip, f_slip)
    } else {
        // At exact standstill the previous-step slip is zero even when the
        // candidate grip force is non-zero (for example, a fresh drive torque).
        // Use the candidate slip direction in that case; otherwise the first
        // saturated tick incorrectly applies zero patch force.
        let previous_slip = axle_speed * radius - hub_speed;
        let slip_sign = if previous_slip.abs() > f64::EPSILON {
            previous_slip.signum()
        } else {
            f_slip.signum()
        };
        let traction_torque = slip_sign * mu_n * radius;
        let speed = axle_speed
            + dt * (drive_torque + brake_torque - traction_torque - bearing_damping * axle_speed)
                / inertia;
        (speed, traction_torque / radius)
    }
}

#[cfg(test)]
mod tire_patch_tests {
    use super::{parking_brake_force, tire_patch_force};
    use bevy::math::DVec3;

    #[test]
    fn rolling_speed_defines_pivot_slip_angle_without_a_threshold() {
        let (_, lateral) = tire_patch_force(0.0, 2.0, 1.0, 400.0, 10.0, 2.9);
        let expected = 2.9 * 400.0 * (-0.5_f64).atan();
        assert!((lateral - expected).abs() < 1.0e-9);
    }

    #[test]
    fn combined_force_preserves_direction_at_coulomb_limit() {
        let (long, lateral) = tire_patch_force(600.0, 2.0, -1.0, 200.0, 1.5, 2.9);
        assert!((long.hypot(lateral) - 300.0).abs() < 1.0e-9);
        assert!(long > 0.0 && lateral > 0.0);
    }

    #[test]
    fn zero_load_has_no_patch_force() {
        assert_eq!(tire_patch_force(10.0, 1.0, 1.0, 0.0, 1.0, 2.9), (0.0, 0.0));
    }

    #[test]
    fn parking_brake_cancels_gravity_on_a_holdable_slope() {
        let gravity = DVec3::new(0.0, -1.62, -0.8);
        let normal_force = 400.0;
        let force = parking_brake_force(gravity, DVec3::Y, normal_force, 1.5);
        assert!(force.z > 0.0);
        assert!((force.y).abs() < 1.0e-12);
        assert!(force.length() <= 1.5 * normal_force + 1.0e-9);
        let supported_mass = normal_force / 1.62;
        assert!(
            (force.z + supported_mass * gravity.z).abs() < 1.0e-9,
            "parking force must exactly balance this contact's tangential gravity share"
        );
    }
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
    /// Rotor inertia reflected through the gearbox to the axle, kg·m²
    /// (`J·ratio²`). Added on top of the tire's own inertia in
    /// [`Self::axle_inertia`] — at high reductions it dominates ½·m·r², which
    /// is why a geared rover spins up slowly instead of snapping to speed.
    /// `0` = undriven wheel (castor) or no drivetrain authored.
    pub reflected_inertia: f64,
    /// (derived from the composed motor's `lunco:motor:stallTorque` and optional
    /// gearbox, required for a driven wheel).
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
    /// Tire CORNERING stiffness: side force per radian of slip angle (N/rad).
    ///
    /// NOT force per m/s of lateral velocity — that was the old model, and the
    /// rename is deliberate: the units changed, so a value carried across without
    /// thought fails to compile instead of being silently wrong by ~100×.
    ///
    /// The force is clamped with the load-dependent Coulomb cone after the
    /// cornering law is evaluated.
    pub cornering_stiffness: f64,
    /// Lowest speed at which the authored steady-state cornering curve was
    /// validated. This is an evidence boundary, not a fitted transition.
    pub min_validated_speed: f64,
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
            reflected_inertia: 0.0,
            drive_torque_max: 0.0,
            bearing_damping: 0.0,
            max_rotation_speed: 0.0,
            friction_mu: 0.0,
            slip_stiffness: 0.0,
            cornering_stiffness: 0.0,
            min_validated_speed: 0.0,
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

    /// Rotational inertia about the axle in kg·m²: the tire's own inertia
    /// (USD-authored `physxVehicleWheel:moi` when set, else the solid-disk
    /// estimate `½·m·r²` from mass and radius) plus the drivetrain's
    /// [`reflected rotor inertia`](Self::reflected_inertia). Returns `None`
    /// when the runtime projection does not contain a finite, positive
    /// physical input or when the combined inertia is not usable.
    #[inline]
    pub fn axle_inertia(&self) -> Option<f64> {
        let tire = if self.moment_of_inertia.is_finite() && self.moment_of_inertia > 0.0 {
            self.moment_of_inertia
        } else {
            if !(self.wheel_radius.is_finite()
                && self.wheel_radius > 0.0
                && self.mass.is_finite()
                && self.mass > 0.0)
            {
                return None;
            }
            0.5 * self.mass * self.wheel_radius * self.wheel_radius
        };
        let inertia = tire + self.reflected_inertia;
        (inertia.is_finite() && inertia > 0.0).then_some(inertia)
    }
}

/// Bodies whose independent mobility force sources are held by a fixed weld to
/// another dynamic body. A raycast wheel is not an Avian body, so its analytical
/// suspension must not become a second support path while the chassis is being
/// carried by that weld. The set is rebuilt from the live joint graph each tick;
/// detaching the authored joint therefore releases the wheels without a
/// scene-specific callback or stale latch.
fn dynamically_fixed_bodies(
    fixed_joints: &Query<&FixedJoint>,
    bodies: &Query<&RigidBody>,
) -> HashSet<Entity> {
    let mut held = HashSet::new();
    for joint in fixed_joints.iter() {
        if matches!(bodies.get(joint.body1), Ok(RigidBody::Dynamic))
            && matches!(bodies.get(joint.body2), Ok(RigidBody::Dynamic))
        {
            held.insert(joint.body1);
            held.insert(joint.body2);
        }
    }
    held
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
    mut q_chassis: Query<(Forces, &RigidBody), lunco_physics::Integrable>,
    fixed_joints: Query<&FixedJoint>,
    q_bodies: Query<&RigidBody>,
    mut q_visual: Query<&mut Transform, (Without<WheelRaycast>, Without<ActuatorPorts>)>,
) {
    let fixed_dynamic_bodies = dynamically_fixed_bodies(&fixed_joints, &q_bodies);

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
            let apply_force = !matches!(body, RigidBody::Kinematic)
                && !fixed_dynamic_bodies.contains(&parent_entity);
            let (world_pos, _) = wheel_hub_pose(
                GridPos(forces.position().0),
                GridRot(forces.rotation().0),
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
                    // Damping is measured along the CONTACT NORMAL — the same
                    // axis the spring force is applied on below. Projecting on
                    // chassis-frame down instead put damper and spring on
                    // different axes on any slope, under-damping exactly the
                    // tilted contacts that ring hardest.
                    // Positive relative_vel = wheel moving toward ground (compressing).
                    // Negative relative_vel = wheel moving away from ground (extending).
                    let lin_vel = forces.linear_velocity();
                    let ang_vel = forces.angular_velocity();
                    let velocity_at_wheel = wheel_hub_velocity(
                        lin_vel,
                        ang_vel,
                        world_pos,
                        GridPos(forces.position().0),
                    );
                    let relative_vel = -velocity_at_wheel.dot(hit.normal);

                    let total_force_mag = suspension_force_mag(
                        compression,
                        susp.spring_k,
                        relative_vel,
                        susp.damping_c,
                    );

                    let force_vec = hit.normal * total_force_mag;
                    if apply_force {
                        forces.apply_force_at_point(force_vec, world_pos.0);
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
    mut q_wheels: Query<
        (Entity, &mut Position, &mut Rotation, &Transform, &ChildOf),
        With<WheelRaycast>,
    >,
    q_chassis: Query<(&Position, &Rotation), (With<RigidBody>, Without<WheelRaycast>)>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
) {
    for (wheel, mut wpos, mut wrot, wtf, parent) in q_wheels.iter_mut() {
        if let Ok((cpos, crot)) = q_chassis.get(parent.parent()) {
            let (hub_pos, hub_rot) = wheel_hub_pose(
                GridPos(cpos.0),
                GridRot(crot.0),
                wtf.translation.as_dvec3(),
                wtf.rotation.as_dquat(),
            );
            // The wheel's `Position`/`Rotation` IS avian's ray-origin frame: the
            // caster's local origin is `DVec3::ZERO`, so the global origin is
            // `Position + Rotation * ZERO` — and a NaN rotation poisons even that.
            // avian's `raycast` asserts `origin.is_finite()` and takes the whole
            // app down with it. Do not leave the wheel on an old pose and let the
            // next system interpret that pose as current physics. Record the
            // invalid source as a terminal scene fault; the fault gate pauses
            // force production and the scene lifecycle owns the explicit reset.
            if !hub_pos.0.is_finite() || !hub_rot.0.is_finite() {
                if let Some(holds) = holds.as_deref_mut() {
                    holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
                }
                if let Some(faults) = faults.as_deref_mut() {
                    if faults.raise(
                        "mobility-nonfinite-wheel-pose",
                        Some(wheel),
                        "raycast wheel",
                        format!(
                            "hub_position={:?}, hub_rotation={:?}, chassis={:?}",
                            hub_pos.0,
                            hub_rot.0,
                            parent.parent(),
                        ),
                    ) {
                        error!(
                            "[mobility] terminal runtime failure: non-finite raycast wheel pose on {wheel:?}"
                        );
                    }
                }
                continue;
            }
            // Compare-gate like the bridge's writeback: the hub pose is a
            // deterministic function of the chassis pose and the wheel's local
            // transform, so an idle chassis recomputes bit-identical values —
            // exact compare, no epsilon. Writing unconditionally dirtied every
            // wheel's `Position`/`Rotation` change ticks per tick even parked.
            if wpos.0 != hub_pos.0 {
                wpos.0 = hub_pos.0;
            }
            if wrot.0 != hub_rot.0 {
                wrot.0 = hub_rot.0;
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
    q_wheels: Query<(&WheelRaycast, &Suspension, &Transform, &RayHits, &ChildOf)>,
    q_ports: Query<&Port>,
    // Force must land only on a body the solver will integrate. A disabled body
    // (frozen while its program compiles, say) never has its accumulators
    // cleared, so force applied to it is stored, not spent, and discharges in
    // full on the step that eventually runs — see `lunco_physics::Integrable`.
    mut q_chassis: Query<
        (
            Forces,
            &RigidBody,
            Option<&InputPorts>,
            Option<&lunco_environment::LocalGravity>,
        ),
        lunco_physics::Integrable,
    >,
    fixed_joints: Query<&FixedJoint>,
    q_bodies: Query<&RigidBody>,
) {
    let fixed_dynamic_bodies = dynamically_fixed_bodies(&fixed_joints, &q_bodies);

    for (wheel, susp, wheel_tf, hits, parent) in q_wheels.iter() {
        let parent_entity = parent.parent();
        if let Ok((mut forces, body, inputs, gravity)) = q_chassis.get_mut(parent_entity) {
            if fixed_dynamic_bodies.contains(&parent_entity) {
                continue;
            }
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
            if q_ports.get(wheel.drive_port).is_ok() {
                // Traction only exists when the ray is hitting the ground.
                let Some(hit) = hits
                    .iter_sorted()
                    .find(|hit| hit.normal.is_finite() && hit.normal.length_squared() > 1.0e-12)
                else {
                    continue;
                };
                {
                    let normal_force = wheel.last_normal_force;
                    if normal_force < 1.0 {
                        // Not enough contact to transmit meaningful force
                        continue;
                    }

                    // Reconstruct the wheel's world pose in the grid-absolute physics
                    // frame from the chassis Position/Rotation + the wheel's LOCAL
                    // transform (exactly as `apply_wheel_suspension` does); the
                    // `GridPos` signature keeps the render-frame `GlobalTransform`
                    // out (CQ-201). `wheel_tf.rotation` carries the steer angle (set
                    // in `apply_wheel_steering`); roll-spin lives on the child
                    // visual, so the drive direction stays correct.
                    let (hub_pos_world, hub_rot_world) = wheel_hub_pose(
                        GridPos(forces.position().0),
                        GridRot(forces.rotation().0),
                        wheel_tf.translation.as_dvec3(),
                        wheel_tf.rotation.as_dquat(),
                    );
                    // The tyre force belongs at the ray's contact point, not at
                    // the hub. Applying it at the hub erased the wheel-radius
                    // moment and was only equivalent on a flat, upright contact.
                    // On a DEM slope that missing moment changes load transfer and
                    // feeds the next suspension sample, producing the observed
                    // forward/back contact limit cycle. The ray starts at the
                    // authored strut top, so reconstruct its hit point in the
                    // same grid-absolute frame as the chassis and ray query.
                    let ray_origin = hub_pos_world.0
                        + hub_rot_world.0
                            * DVec3::Y
                            * strut_offset(susp.rest_length, wheel.wheel_radius);
                    let ray_direction = hub_rot_world.0 * DVec3::NEG_Y;
                    let contact_point = ray_origin + ray_direction * hit.distance;
                    // The tire force was already solved this tick, from the real
                    // contact slip `ω·r − v` and the wheel's own lateral slip —
                    // see `update_wheel_spin`. Applying it is all that is left.
                    // A wheel brake must provide static friction as well as
                    // spin-down torque.  The old path only damped axle spin,
                    // so a rover with zero wheel speed still slid down a
                    // slope: the tire solver produced a few newtons while
                    // gravity supplied the remaining acceleration.  Project
                    // gravity onto this contact plane and distribute the
                    // required support by the measured normal load.  The
                    // authored tire friction coefficient remains the hard
                    // limit; a slope steeper than the tire can hold still
                    // slides naturally.
                    let parking_force = if inputs.is_some_and(|i| i.brake_active) {
                        gravity
                            .map(|g| {
                                parking_brake_force(
                                    g.0,
                                    hit.normal,
                                    normal_force,
                                    wheel.friction_mu,
                                )
                            })
                            .unwrap_or(DVec3::ZERO)
                    } else {
                        DVec3::ZERO
                    };
                    forces.apply_force_at_point(wheel.tire_force + parking_force, contact_point);
                    drive_diag_block!({
                        if wheel.tire_force.length() > 1.0 {
                            let arm = contact_point - forces.position().0;
                            let moment = arm.cross(wheel.tire_force);
                            info!(
                                "[drive-diag] apply_wheel_drive: chassis {:?} tire_force=({:.1},{:.1},{:.1}) at=({:.2},{:.2},{:.2}) body_pos=({:.2},{:.2},{:.2}) computed_moment_y={:.1}",
                                parent_entity,
                                wheel.tire_force.x,
                                wheel.tire_force.y,
                                wheel.tire_force.z,
                                contact_point.x,
                                contact_point.y,
                                contact_point.z,
                                forces.position().0.x,
                                forces.position().0.y,
                                forces.position().0.z,
                                moment.y
                            );
                        }
                    });
                }
            }
        }
    }
}

/// A steered wheel's base (authored mount) local rotation, captured before the
/// first steer write. `apply_wheel_steering` composes the steer quat on top of
/// this instead of assigning `Transform::rotation` wholesale — the wholesale
/// write erased any authored camber/toe/rake every tick, so a mount authored
/// with a lean snapped upright the moment steering ran.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct SteerBaseRotation(pub Quat);

/// Applies the steered angle to a raycast front wheel's transform. The angle
/// itself (rate-limited servo slew + Ackermann inner/outer geometry) is computed
/// by the SHARED [`lunco_hardware::SteeringActuator`] system — the exact same
/// model the physical joint wheel uses — so steering is identical across wheel
/// kinds and the logic lives in one place (DRY). This system only reads the
/// computed `output_angle` and rotates the wheel about its steer axis, composed
/// onto the captured [`SteerBaseRotation`]; the visual mesh rotation
/// (steer + roll spin) is composed in `update_wheel_spin`.
fn apply_wheel_steering(
    mut commands: Commands,
    mut q_wheels: Query<(
        Entity,
        &mut Transform,
        &ChildOf,
        &lunco_hardware::SteeringActuator,
        &WheelRaycast,
        Option<&SteerBaseRotation>,
    )>,
    q_chassis: Query<&RigidBody, With<RigidBody>>,
) {
    for (entity, mut transform, parent, steer, wheel, base) in q_wheels.iter_mut() {
        // Predict-own: this chain runs on a client too. Skip wheels of a
        // `Kinematic` chassis (replicated rovers this peer does NOT own), whose
        // local steer ports are stale and would point the wheels wrong.
        if let Ok(body) = q_chassis.get(parent.parent()) {
            if matches!(body, RigidBody::Kinematic) {
                continue;
            }
        }
        // The mount's authored rotation, captured on first run — before this
        // system has ever written the transform, so it IS the authored value.
        let base_rotation = match base {
            Some(b) => b.0,
            None => {
                let b = transform.rotation;
                commands.entity(entity).try_insert(SteerBaseRotation(b));
                b
            }
        };
        // Steer about the wheel's steer axis, in the MOUNT frame — so a raked
        // motorcycle fork tilts the axis with the mount. Default `+Y` reproduces
        // the flat yaw steer.
        let raw = wheel.steer_axis.as_vec3();
        let axis = if raw.length_squared() > 1e-12 {
            raw.normalize()
        } else {
            Vec3::Y
        };
        transform.rotation =
            base_rotation * Quat::from_axis_angle(axis, -steer.output_angle as f32);
    }
}

// A vessel's command-to-actuator allocation is the data-driven
// `lunco_core::kernels::DriveMix { kernel, ports, entries }`, whose `kernel`
// names a self-registered `ControlKernel` (`skid` / `linear` / future flight
// allocators). `apply_drive_mix` resolves and runs the selected kernel without
// a per-architecture component taxonomy.

/// Drive interpretation for an authored `PhysicsDriveAPI` relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect)]
pub enum DifferentialDriveType {
    /// Coefficients describe a generalized force/torque.
    Force,
    /// Coefficients describe a generalized acceleration.
    Acceleration,
}

/// Authored `PhysicsDriveAPI` parameters for a gear relation. `lunco-usd-sim`
/// reads these fields into a `PendingDifferential` and
/// `resolve_differential_coupling` attaches this component.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct DifferentialCoupling {
    /// The chassis body the two rockers pivot against (reaction torque target).
    pub chassis: Entity,
    /// Left rocker body.
    pub rocker_a: Entity,
    /// Right rocker body.
    pub rocker_b: Entity,
    /// Authored `physxGearJoint:gearRatio` — the `r` in `θ_a = r·θ_b`.
    pub ratio: f64,
    /// Target for `θ_a − r·θ_b` (rad).
    pub rest_offset: f64,
    /// Target relation velocity (rad/s).
    pub target_velocity: f64,
    /// Coupling stiffness (N·m per rad of relation error in force mode).
    pub stiffness: f64,
    /// Coupling damping (N·m·s per rad/s of relation velocity in force mode).
    pub damping: f64,
    /// Maximum generalized force/torque. Infinity is the standard USD fallback.
    pub max_force: f64,
    /// Whether the authored coefficients are acceleration-based.
    pub drive_type: DifferentialDriveType,
}

/// Signed rotation angle (rad) of a relative quaternion about `axis` — the twist
/// component of a swing-twist decomposition. For a pure rotation `θ` about a unit
/// `axis`, `q = (cos θ/2, sin θ/2 · axis)` and this returns `θ`, wrapped to
/// `(-π, π]`. Used to read each rocker's pitch in the chassis frame.
#[cfg(test)]
fn angle_about_axis(rel: DQuat, axis: DVec3) -> f64 {
    let a = axis.normalize_or_zero();
    if a == DVec3::ZERO {
        return 0.0;
    }

    // Remove swing before measuring the hinge twist.  Using the raw relative
    // quaternion's `atan2(projected_vector, w)` treats the swing component as
    // part of the scalar half-angle; once a rover is tilted, that produces a
    // different angle from the native revolute joint and the two constraints
    // fight.  This is the same swing-twist decomposition used by the joint
    // telemetry backend, kept here in f64 for the solver.
    let vector = DVec3::new(rel.x, rel.y, rel.z);
    let projected = a * vector.dot(a);
    let twist = DQuat::from_xyzw(projected.x, projected.y, projected.z, rel.w);
    let norm_sq = twist.length_squared();
    if norm_sq <= 1e-24 {
        return 0.0;
    }
    let twist = twist.normalize();
    let mut angle = 2.0 * twist.w.clamp(-1.0, 1.0).acos();
    if vector.dot(a) < 0.0 {
        angle = -angle;
    }
    while angle > std::f64::consts::PI {
        angle -= std::f64::consts::TAU;
    }
    while angle <= -std::f64::consts::PI {
        angle += std::f64::consts::TAU;
    }
    angle
}

/// Read the angular state of a native Avian revolute joint using the same
/// joint-frame construction as its XPBD solver. `Rotation` is the frame-start
/// body orientation and `delta_rotation` is the accumulated substep correction.
fn native_revolute_state(
    joint: &RevoluteJoint,
    body1_rotation: DQuat,
    body2_rotation: DQuat,
    body1_delta: DQuat,
    body2_delta: DQuat,
) -> Option<(f64, DVec3)> {
    let basis1 = joint.local_basis1()?;
    let basis2 = joint.local_basis2()?;
    let axis = joint.hinge_axis;
    let axis1 = body1_rotation * basis1 * axis;
    let b1 = body1_rotation * basis1 * axis.any_orthonormal_vector();
    let b2 = body2_rotation * basis2 * axis.any_orthonormal_vector();
    let corrected_axis = (body1_delta * axis1).normalize_or_zero();
    if corrected_axis == DVec3::ZERO {
        return None;
    }
    let corrected_b1 = body1_delta * b1;
    let corrected_b2 = body2_delta * b2;
    let angle = corrected_b1
        .cross(corrected_b2)
        .dot(corrected_axis)
        .atan2(corrected_b1.dot(corrected_b2));
    Some((angle, corrected_axis))
}

/// The gear's relation `c = θ_a − r·θ_b − rest_offset`, on rocker pitches
/// measured in the CHASSIS frame. Zero ⇒ the drive is at its target.
fn gear_error(angle_a: f64, angle_b: f64, ratio: f64, rest_offset: f64) -> f64 {
    angle_a - ratio * angle_b - rest_offset
}

/// Generalized impulse for one authored USD drive update.
fn gear_drive_impulse(
    position_error: f64,
    relation_velocity: f64,
    target_velocity: f64,
    stiffness: f64,
    damping: f64,
    max_force: f64,
    inverse_mass: f64,
    dt: f64,
    drive_type: DifferentialDriveType,
) -> f64 {
    if inverse_mass <= f64::EPSILON || !dt.is_finite() || dt <= 0.0 {
        return 0.0;
    }
    let raw = -stiffness * position_error + damping * (target_velocity - relation_velocity);
    if !raw.is_finite() {
        return 0.0;
    }
    let limited = if max_force.is_finite() {
        raw.clamp(-max_force, max_force)
    } else {
        raw
    };
    if !limited.is_finite() {
        return 0.0;
    }
    match drive_type {
        DifferentialDriveType::Force => limited * dt,
        DifferentialDriveType::Acceleration => limited / inverse_mass * dt,
    }
}

/// The rocker-bogie differential is an authored `PhysicsDriveAPI:angular`
/// relation inside Avian's substep solver. Its stiffness, damping, target
/// velocity, drive type, and max-force limit are all projected from USD.
///
/// Runs per substep before the native revolute pass. The current world rotation
/// of each body is `delta_rotation · Rotation` (Avian only writes `Rotation` back
/// at the end of the step), matching Avian's own joint solver.
fn solve_differential_gear(
    q_coupling: Query<&DifferentialCoupling>,
    q_revolute: Query<&RevoluteJoint>,
    mut q_solver: Query<
        (&mut SolverBody, &SolverBodyInertia, &Rotation),
        Without<RigidBodyDisabled>,
    >,
    time: Res<Time>,
) {
    let dt = time.delta_secs_f64();
    for coupling in q_coupling.iter() {
        let Ok([(mut sa, ia, ra), (mut sb, ib, rb), (mut sc, ic, rc)]) =
            q_solver.get_many_mut([coupling.rocker_a, coupling.rocker_b, coupling.chassis])
        else {
            continue;
        };

        let Some(joint_a) = q_revolute
            .iter()
            .find(|joint| joint.body1 == coupling.chassis && joint.body2 == coupling.rocker_a)
        else {
            continue;
        };
        let Some(joint_b) = q_revolute
            .iter()
            .find(|joint| joint.body1 == coupling.chassis && joint.body2 == coupling.rocker_b)
        else {
            continue;
        };

        // Current world rotations inside the substep.
        let Some((angle_a, axis_a)) = native_revolute_state(
            joint_a,
            rc.0,
            ra.0,
            sc.delta_rotation.0,
            sa.delta_rotation.0,
        ) else {
            continue;
        };
        let Some((angle_b, axis_b)) = native_revolute_state(
            joint_b,
            rc.0,
            rb.0,
            sc.delta_rotation.0,
            sb.delta_rotation.0,
        ) else {
            continue;
        };
        let r = coupling.ratio;
        let error = gear_error(angle_a, angle_b, r, coupling.rest_offset);
        let chassis_gradient = -axis_a + r * axis_b;
        let gradients = [axis_a, -r * axis_b, chassis_gradient];

        let mut inv_inertias = [
            ia.effective_inv_angular_inertia(),
            ib.effective_inv_angular_inertia(),
            ic.effective_inv_angular_inertia(),
        ];
        // Match Avian's pairwise dominance rule for a multi-body constraint.
        // A kinematic body retains its authored inertia tensor, but its higher
        // dominance makes it the immovable side of every native joint. The
        // gear must apply the same rule or it can rotate bodies while the scene
        // is still being held for collider admission.
        let dominances = [ia.dominance(), ib.dominance(), ic.dominance()];
        let least_dominance = dominances.into_iter().min().unwrap_or_default();
        for (inverse_inertia, dominance) in inv_inertias.iter_mut().zip(dominances) {
            if dominance > least_dominance {
                *inverse_inertia *= 0.0;
            }
        }
        let inverse_mass: f64 = gradients
            .iter()
            .zip(inv_inertias.iter())
            .map(|(gradient, inverse_inertia)| gradient.dot(*inverse_inertia * *gradient))
            .sum();
        let relation_velocity = gradients[0].dot(sa.angular_velocity)
            + gradients[1].dot(sb.angular_velocity)
            + gradients[2].dot(sc.angular_velocity);
        let impulse = gear_drive_impulse(
            error,
            relation_velocity,
            coupling.target_velocity,
            coupling.stiffness,
            coupling.damping,
            coupling.max_force,
            inverse_mass,
            dt,
            coupling.drive_type,
        );
        if impulse == 0.0 || !impulse.is_finite() {
            continue;
        }
        for (body, (gradient, inv)) in [&mut sa, &mut sb, &mut sc]
            .into_iter()
            .zip(gradients.iter().zip(inv_inertias.iter()))
        {
            body.angular_velocity += *inv * (*gradient * impulse);
        }
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
/// **Model**: the ONE suspension force law, [`suspension_force_mag`], applied
/// along the prismatic joint's slider axis — the same bounded-damping law the
/// raycast strut uses. This path used to clamp the TOTAL `(spring + damping)`
/// to `[0, MAX]`, which dropped all damping on the rebound half-cycle — the
/// exact `.max(0)` cliff the shared law was written to remove. The force is
/// applied as an equal and opposite pair on the two connected bodies.
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
            let total_force_mag: f64 =
                suspension_force_mag(compression, susp.spring_k, rel_vel, susp.damping_c);

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

/// Derive each endpoint's input surface from its intent binding: for any entity
/// that has both [`InputPorts`] and a [`lunco_core::ControlBinding`], ensure every
/// bound port exists in `InputPorts.values` (seeded `0.0`).
///
/// This is what lets the command vocabulary be **data, not a Rust literal**: a
/// vessel's `Controls` profile (→ its `ControlBinding`) declares exactly which
/// command ports it accepts, and the strict command backend then admits writes to
/// those and no others. Additive (never removes keys) and idempotent, so it's safe to
/// run on `Changed<ControlBinding>` regardless of which reader stamped the binding or
/// the surface, and regardless of spawn order.
fn sync_input_ports(
    mut q: Query<
        (&lunco_core::ControlBinding, &mut InputPorts),
        Or<(Changed<lunco_core::ControlBinding>, Added<InputPorts>)>,
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

// ── Vehicle brake and drive allocation ───────────────────────────────────────

/// Apply the vessel-wide brake command independently of drive allocation.
///
/// Every vehicle control surface has an [`ActuatorPorts`] index, whether its
/// drive outputs are allocated by an imperative [`DriveMix`] or by authored
/// co-simulation wiring. Braking is a mechanism shared by both cases: it owns
/// [`InputPorts::brake_active`] (used by the tire solve) and the discrete
/// `brake` actuator port. Tying either fact to `DriveMix` would incorrectly
/// disable brakes—and the mobility chassis itself—when Modelica owns drive
/// allocation.
fn apply_vehicle_brake(
    mut q: Query<(&mut InputPorts, &ActuatorPorts)>,
    mut q_ports: Query<&mut Port>,
) {
    for (mut inputs, actuators) in &mut q {
        inputs.brake_active = inputs.cmd("brake") > 0.5;
        if let Some(port_b) = actuators.get("brake") {
            if let Ok(mut port) = q_ports.get_mut(port_b) {
                port.value = if inputs.brake_active { 1.0 } else { 0.0 };
            }
        }
    }
}

/// System allocating each rover's input ports (`throttle`/`steer`/`brake`, read
/// from [`InputPorts::values`]) to its actuator [`Port`]s (indexed by
/// [`ActuatorPorts`]), via the
/// vessel's data-selected [`DriveMix`] kernel (`skid`/`linear`/…, looked up in the
/// [`ControlKernelRegistry`]). No per-architecture branch: the kernel is chosen by
/// USD, and its outputs are saturated to `[-1, 1]` — ±100% actuator authority —
/// before being written to the port. Runs every fixed tick before wire propagation.
fn apply_drive_mix(
    mut q: Query<(Entity, &mut InputPorts, &ActuatorPorts, &DriveMix)>,
    registry: Res<ControlKernelRegistry>,
    mut q_ports: Query<&mut Port>,
    mut unknown: Local<std::collections::HashSet<String>>,
) {
    for (entity, mut inputs, actuators, mix) in q.iter_mut() {
        // Read this vehicle's logical command inputs off the command surface.
        let throttle = inputs.cmd("throttle");
        let steer = inputs.cmd("steer");

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
        // with no matching hook leaves the vessel explicitly stopped and braked.
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

        if outputs.is_empty() {
            // A failed scripted kernel must not leave the previous actuator
            // registers latched.  Neutralise the complete surface and engage
            // the brake gate before the next propagation tick.
            safe_stop_control_surface(Some(&mut *inputs), Some(actuators), &mut q_ports);
            continue;
        }

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
/// **actual input surface** — its declared [`InputPorts::values`] map, keyed by
/// whatever ports that vehicle accepts (a rover's `throttle`/`steer`/`brake`, a
/// lander's `throttle`/`pitch`/`roll`/`yaw`, …) — NOT a fixed Rust key set. The
/// command vocabulary is data, so a scripted kernel reads exactly the ports the
/// vessel exposes and the script owns its own policy (incl. how `brake` gates).
/// Reads back a `port → value` map in `[-1, 1]` (clamped defensively). Empty on an
/// absent or faulted hook: the complete actuator surface is explicitly neutralised
/// and braked by [`safe_stop_control_surface`].
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
                ActuatorPorts::default(),
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
            .spawn((ActuatorPorts::default(), Mass(1000.0)))
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

    #[test]
    fn articulated_wheel_mass_is_found_through_its_carrier() {
        let mut app = App::new();
        app.add_systems(Update, fold_proxy_wheel_mass);
        let chassis = app
            .world_mut()
            .spawn((ActuatorPorts::default(), Mass(1000.0)))
            .id();
        let rocker = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(0.0, -0.2, 0.5)),
                ChildOf(chassis),
            ))
            .id();
        app.world_mut().spawn((
            WheelRaycast {
                mass: 25.0,
                ..default()
            },
            Transform::from_translation(Vec3::new(0.0, -0.65, -1.2)),
            ChildOf(rocker),
        ));

        app.update();

        let mass = app.world().get::<Mass>(chassis).unwrap().0;
        let com = app
            .world()
            .get::<CenterOfMass>(chassis)
            .map(|c| c.0)
            .unwrap_or(Vec3::ZERO);
        assert!((mass - 1025.0).abs() < 1e-3);
        assert!((com.y + 0.85 * 25.0 / 1025.0).abs() < 1e-6);
        assert!((com.z + 0.7 * 25.0 / 1025.0).abs() < 1e-6);
    }
}

#[cfg(test)]
mod force_law_tests {
    //! Regression guards for the numerically-sensitive wheel force laws. Each
    //! test pins a property whose violation previously caused a jitter or a
    //! broken control (the comments name the bug).
    use super::*;
    use bevy::math::{DQuat, DVec3};
    use lunco_core::coords::VehicleFrame;

    #[test]
    fn authored_allocator_vehicle_keeps_the_shared_brake_without_drive_mix() {
        let mut app = App::new();
        app.add_systems(Update, apply_vehicle_brake);

        let brake_port = app.world_mut().spawn(Port::default()).id();
        let mut inputs = InputPorts::new(&["throttle", "steer", "brake"]);
        inputs.values.insert("brake".to_string(), 1.0);
        let vehicle = app
            .world_mut()
            .spawn((
                inputs,
                ActuatorPorts::new(std::collections::HashMap::from([(
                    "brake".to_string(),
                    brake_port,
                )])),
            ))
            .id();

        app.update();

        assert!(app.world().get::<InputPorts>(vehicle).unwrap().brake_active);
        assert_eq!(app.world().get::<Port>(brake_port).unwrap().value, 1.0);
        assert!(app.world().get::<DriveMix>(vehicle).is_none());
    }

    // ── Single-track lean: contact-plane traction basis ─────────────────────
    #[test]
    fn contact_basis_upright_matches_flat_wheel() {
        // Upright wheel: contact normal = world up. Basis must equal the raw
        // wheel forward/right (so existing rovers are unchanged).
        let (f, r) = contact_plane_basis(
            VehicleFrame::FORWARD_LOCAL,
            VehicleFrame::RIGHT_LOCAL,
            DVec3::Y,
        );
        assert!(
            (f - VehicleFrame::FORWARD_LOCAL).length() < 1e-9,
            "forward changed: {f:?}"
        );
        assert!(
            (r - VehicleFrame::RIGHT_LOCAL).length() < 1e-9,
            "right changed: {r:?}"
        );
    }

    #[test]
    fn contact_basis_leaned_lies_in_contact_plane() {
        // Cambered contact: normal tilted 22° off vertical. Both basis vectors
        // must lie in the plane ⟂ to the normal, stay unit, and be orthogonal.
        let n = DVec3::new(0.0, 1.0, 0.4).normalize();
        let (f, r) = contact_plane_basis(VehicleFrame::FORWARD_LOCAL, VehicleFrame::RIGHT_LOCAL, n);
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

        // Swing must not change the hinge angle.  This is the case the rover
        // reaches while its chassis is tilted on a step; the differential and
        // the native revolute joint must read the same DOF there.
        let swung = DQuat::from_axis_angle(DVec3::Z, 0.7) * DQuat::from_axis_angle(DVec3::X, 0.3);
        assert!((angle_about_axis(swung, axis) - 0.3).abs() < 1e-9);
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

    #[test]
    fn force_drive_impulse_is_finite_and_limited() {
        for error in [0.4, -0.15, 0.6, 0.2] {
            let impulse = gear_drive_impulse(
                error,
                0.0,
                0.0,
                8_000.0,
                1_200.0,
                100.0,
                0.11,
                1.0 / 64.0,
                DifferentialDriveType::Force,
            );
            assert!(impulse.is_finite());
            assert!(impulse.abs() <= 100.0 / 64.0);
        }
    }

    #[test]
    fn satisfied_drive_at_target_needs_no_impulse() {
        assert_eq!(
            gear_drive_impulse(
                0.0,
                0.0,
                0.0,
                8_000.0,
                1_200.0,
                f64::INFINITY,
                0.11,
                1.0 / 64.0,
                DifferentialDriveType::Force,
            ),
            0.0
        );
    }

    #[test]
    fn the_ratio_defines_the_error() {
        assert_eq!(gear_error(0.4, 0.4, -1.0, 0.0), 0.8);
        assert_eq!(gear_error(0.4, 0.4, 1.0, 0.0), 0.0);
    }

    #[test]
    fn rest_offset_moves_the_target() {
        let c = gear_error(0.3, 0.2, -1.0, 0.5);
        assert!(
            c.abs() < 1e-12,
            "offset target should be satisfied, got {c}"
        );
    }

    #[test]
    fn an_immovable_rig_is_left_alone() {
        assert_eq!(
            gear_drive_impulse(
                0.5,
                0.0,
                0.0,
                8_000.0,
                1_200.0,
                f64::INFINITY,
                0.0,
                1.0 / 64.0,
                DifferentialDriveType::Force,
            ),
            0.0
        );
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
                ActuatorPorts::default(),
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

        app.world_mut().entity_mut(visual).insert(ChildOf(wheel));
        app.world_mut().entity_mut(piston).insert(ChildOf(wheel));
        app.world_mut().entity_mut(spring).insert(ChildOf(wheel));

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
