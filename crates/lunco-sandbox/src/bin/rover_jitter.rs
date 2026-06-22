//! Headless rover-suspension jitter probe.
//!
//! Builds a joint-based rover **in code** (no USD, no render, no GPU,
//! no FloatingOrigin) so the only thing under test is the chassis↔wheel
//! joint physics. Steps Avian deterministically with a fixed timestep,
//! lets the rover settle, then measures residual chassis motion — the
//! objective "jitter" metric.
//!
//! Three suspension modes reproduce / fix the bug:
//!   * `none`      — bare zero-compliance `RevoluteJoint` (current
//!                   `setup_physical_wheel` behaviour). The hypothesis
//!                   says THIS jitters.
//!   * `compliant` — same revolute but with `point_compliance` + more
//!                   substeps (the parameter-only interim).
//!   * `spring`    — chassis →PrismaticJoint(SpringDamper motor)→ hub
//!                   →RevoluteJoint→ wheel (the proper articulated
//!                   suspension reading the authored spring values).
//!
//! ```text
//! cargo run -p lunco-client --bin rover_jitter -j2 -- --suspension=none
//! cargo run -p lunco-client --bin rover_jitter -j2 -- --suspension=spring
//! cargo run -p lunco-client --bin rover_jitter -j2 -- --suspension=none --drive=0.6
//! ```
//!
//! A settled rover has chassis |ω| ≈ 0 and |v| ≈ 0. A jittering one
//! shows persistent non-zero values in the sample window.

use std::time::Duration;

use bevy::prelude::*;
use bevy::app::ScheduleRunnerPlugin;
use bevy::time::TimeUpdateStrategy;
use bevy::math::DVec3;
use avian3d::prelude::*;

// ── Authored rover parameters (mirror assets/.../*rover*.usda) ──
const CHASSIS_MASS: f64 = 1000.0;
const WHEEL_MASS: f64 = 100.0;
// Hub = unsprung mass (upright + bearing). Real cars run ~10-15% of corner
// mass; a too-light hub gives the solver an ill-conditioned mass ratio and
// the chassis→hub→wheel chain diverges. 40 kg keeps it well-behaved.
const HUB_MASS: f64 = 40.0;
const HUB_RADIUS: f64 = 0.15; // hub collider radius — only used for inertia
const WHEEL_RADIUS: f64 = 0.4;
const WHEEL_WIDTH: f64 = 0.3;
const SPRING_K: f64 = 15000.0; // physxVehicleSuspension:springStiffness (N/m)
const SPRING_C: f64 = 3000.0; // physxVehicleSuspension:springDamping  (N·s/m)
const SUSP_TRAVEL: f64 = 0.3; // prismatic limit (m)
const DRIVE_PEAK_TORQUE: f64 = 12000.0; // drive:angular:physics:maxForce (N·m)

// Chassis half-extents and wheel mounting layout.
const CHASSIS_HE: DVec3 = DVec3::new(1.0, 0.5, 1.5);
const WHEEL_X: f64 = 1.0;
const WHEEL_Z: f64 = 1.2;
const CHASSIS_Y: f64 = 1.0; // chassis centre height at spawn
const WHEEL_Y: f64 = WHEEL_RADIUS; // wheel centre so it just touches y=0

#[derive(Clone, Copy, PartialEq)]
enum Suspension {
    None,
    Compliant,
    Spring,
}

#[derive(Resource, Clone)]
struct Config {
    suspension: Suspension,
    drive: f64,
    substeps: u32,
    gravity: f64,
    settle_ticks: u64,
    sample_ticks: u64,
    /// Empirical multiplier on the derived spring frequency. avian's
    /// SpringDamper realizes a k_eff that is a fixed fraction of the
    /// analytic reduced_mass·ω²; this knob calibrates it out.
    spring_scale: f64,
    /// Extra height (m) added to every spawn Y so the rover FALLS onto the
    /// ground — the real scene drops the joint rovers from Y=5. The settle
    /// transient from a drop is far harsher than spawning at rest.
    drop: f64,
    /// Multiplier on the spring damping_ratio. The bounce mode is overdamped
    /// from the authored c, but the chassis pitch/roll mode rings down slowly;
    /// this lets the pitch mode be damped without changing stiffness.
    damp_scale: f64,
    /// Per-wheel coulomb friction coefficient (avian `Friction`). Lower =
    /// easier lateral break-away = faster skid-steer yaw (but less grip).
    friction: f64,
    /// Per-wheel linear velocity damping.
    lin_damp: f64,
    /// Per-wheel angular (spin) velocity damping. Lower = wheel spins up
    /// faster = higher top speed.
    ang_damp: f64,
    /// Drive mode: false = torque-on-wheel (couple → wheelie); true =
    /// forward-force-at-contact (no couple → only real traction pitch).
    force_drive: bool,
}

#[derive(Component)]
struct Chassis;

#[derive(Component)]
struct DriveWheel {
    axle: DVec3,
}

#[derive(Resource, Default)]
struct Stats {
    tick: u64,
    ang_max: f64,
    ang_sumsq: f64,
    lin_max: f64,
    lin_sumsq: f64,
    n: u64,
    height_min: f64,
    height_max: f64,
}

fn parse_args() -> Config {
    let mut suspension = Suspension::None;
    let mut drive = 0.0;
    let mut substeps = 6u32;
    let mut gravity = 1.62; // lunar surface gravity (the real scenario)
    let mut settle = 2.0f64;
    let mut sample = 1.0f64;
    let mut spring_scale = 1.0f64;
    let mut drop = 0.0f64;
    let mut damp_scale = 1.0f64;
    let mut friction = 1.2f64;
    let mut lin_damp = 2.0f64;
    let mut ang_damp = 4.0f64;
    let mut force_drive = false;

    for arg in std::env::args().skip(1) {
        let Some((k, v)) = arg.trim_start_matches("--").split_once('=') else {
            continue;
        };
        match k {
            "suspension" => {
                suspension = match v {
                    "none" => Suspension::None,
                    "compliant" => Suspension::Compliant,
                    "spring" => Suspension::Spring,
                    other => {
                        eprintln!("unknown suspension '{other}', using none");
                        Suspension::None
                    }
                }
            }
            "drive" => drive = v.parse().unwrap_or(0.0),
            "substeps" => substeps = v.parse().unwrap_or(6),
            "gravity" => gravity = v.parse().unwrap_or(1.62),
            "settle" => settle = v.parse().unwrap_or(2.0),
            "sample" => sample = v.parse().unwrap_or(1.0),
            "springscale" => spring_scale = v.parse().unwrap_or(1.0),
            "drop" => drop = v.parse().unwrap_or(0.0),
            "dampscale" => damp_scale = v.parse().unwrap_or(1.0),
            "friction" => friction = v.parse().unwrap_or(1.2),
            "lindamp" => lin_damp = v.parse().unwrap_or(2.0),
            "angdamp" => ang_damp = v.parse().unwrap_or(4.0),
            "drivemode" => force_drive = v == "force",
            _ => {}
        }
    }

    // `compliant` mode also bumps substeps if the user left the default.
    if suspension == Suspension::Compliant && substeps == 6 {
        substeps = 12;
    }

    Config {
        suspension,
        drive,
        substeps,
        gravity,
        settle_ticks: (settle * 60.0).round() as u64,
        sample_ticks: (sample * 60.0).round() as u64,
        spring_scale,
        drop,
        damp_scale,
        friction,
        lin_damp,
        ang_damp,
        force_drive,
    }
}

fn main() {
    let cfg = parse_args();
    let total = cfg.settle_ticks + cfg.sample_ticks;
    let dt = Duration::from_secs_f64(1.0 / 60.0);

    let mut app = App::new();
    app.add_plugins(
        MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::ZERO)),
    )
    .add_plugins(bevy::transform::TransformPlugin)
    // Avian's `collider-from-mesh` cache reads `AssetEvent<Mesh>`, so the
    // Mesh asset must exist even though this headless probe builds all
    // colliders by hand.
    .add_plugins(bevy::asset::AssetPlugin::default())
    .init_asset::<Mesh>()
    .add_plugins(PhysicsPlugins::default())
    .insert_resource(Gravity(DVec3::new(0.0, -cfg.gravity, 0.0)))
    .insert_resource(SubstepCount(cfg.substeps))
    .insert_resource(Time::<Fixed>::from_hz(60.0))
    // Deterministic stepping: each app.update() advances virtual time by
    // exactly one fixed tick, so FixedUpdate fires once per update.
    .insert_resource(TimeUpdateStrategy::ManualDuration(dt))
    .insert_resource(cfg.clone())
    .insert_resource(Stats::default())
    .add_systems(Startup, setup)
    .add_systems(FixedUpdate, apply_drive)
    .add_systems(FixedLast, sample_chassis);

    app.finish();
    app.cleanup();

    for _ in 0..total {
        app.update();
    }

    let s = app.world().resource::<Stats>();
    let n = s.n.max(1) as f64;
    let mode = match cfg.suspension {
        Suspension::None => "none     ",
        Suspension::Compliant => "compliant",
        Suspension::Spring => "spring   ",
    };
    println!(
        "\n=== rover_jitter: suspension={mode} drive={:.2} g={:.2} substeps={} ===",
        cfg.drive, cfg.gravity, cfg.substeps
    );
    println!(
        "settle={:.2}s sample={:.2}s  (sampled {} ticks)",
        cfg.settle_ticks as f64 / 60.0,
        cfg.sample_ticks as f64 / 60.0,
        s.n
    );
    println!(
        "chassis |w|max={:8.4} rad/s   |w|rms={:8.4}   |v|max={:8.4} m/s   |v|rms={:8.4}",
        s.ang_max,
        (s.ang_sumsq / n).sqrt(),
        s.lin_max,
        (s.lin_sumsq / n).sqrt(),
    );
    println!(
        "chassis height range over sample window: {:.4} .. {:.4} m (span {:.4})",
        s.height_min,
        s.height_max,
        s.height_max - s.height_min,
    );
    let verdict = if s.ang_max > 0.25 || s.lin_max > 0.25 {
        "JITTER"
    } else {
        "stable"
    };
    println!("verdict: {verdict}\n");
}

fn setup(mut commands: Commands, cfg: Res<Config>) {
    // Static ground, top face at y = 0.
    commands.spawn((
        Name::new("Ground"),
        RigidBody::Static,
        Collider::cuboid(200.0, 1.0, 200.0),
        Friction::new(1.2),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));

    // Chassis compound body.
    let chassis = commands
        .spawn((
            Name::new("Chassis"),
            Chassis,
            RigidBody::Dynamic,
            Collider::cuboid(CHASSIS_HE.x * 2.0, CHASSIS_HE.y * 2.0, CHASSIS_HE.z * 2.0),
            Mass(CHASSIS_MASS as f32),
            Friction::new(1.2),
            Transform::from_xyz(0.0, (CHASSIS_Y + cfg.drop) as f32, 0.0),
        ))
        .id();

    // Four wheels at the corners.
    let corners = [
        DVec3::new(WHEEL_X, WHEEL_Y, WHEEL_Z),
        DVec3::new(-WHEEL_X, WHEEL_Y, WHEEL_Z),
        DVec3::new(WHEEL_X, WHEEL_Y, -WHEEL_Z),
        DVec3::new(-WHEEL_X, WHEEL_Y, -WHEEL_Z),
    ];

    // Wheel axle is X; Avian's cylinder is Y-native, so rotate Y→X.
    let axle = DVec3::X;
    let wheel_rot = Quat::from_rotation_arc(Vec3::Y, Vec3::X);

    for (i, c) in corners.into_iter().enumerate() {
        // Chassis-local anchor for this wheel/hub (drop-independent: anchors
        // are chassis-local).
        let chassis_anchor = DVec3::new(c.x, c.y - CHASSIS_Y, c.z);
        // World spawn point, raised by the drop height.
        let c_world = c + DVec3::new(0.0, cfg.drop, 0.0);

        let wheel = commands
            .spawn((
                Name::new(format!("Wheel_{i}")),
                DriveWheel { axle },
                RigidBody::Dynamic,
                Collider::cylinder(WHEEL_RADIUS, WHEEL_WIDTH),
                Mass(WHEEL_MASS as f32),
                Friction::new(cfg.friction),
                LinearDamping(cfg.lin_damp),
                AngularDamping(cfg.ang_damp),
                Transform::from_translation(c_world.as_vec3()).with_rotation(wheel_rot),
            ))
            .id();

        match cfg.suspension {
            Suspension::None => {
                // Bare rigid hinge — reproduces current behaviour.
                commands.spawn((
                    RevoluteJoint::new(chassis, wheel)
                        .with_local_anchor1(chassis_anchor)
                        .with_local_anchor2(DVec3::ZERO)
                        .with_hinge_axis(axle),
                    JointCollisionDisabled,
                ));
            }
            Suspension::Compliant => {
                // Soft pin: give the point constraint a little compliance.
                commands.spawn((
                    RevoluteJoint::new(chassis, wheel)
                        .with_local_anchor1(chassis_anchor)
                        .with_local_anchor2(DVec3::ZERO)
                        .with_hinge_axis(axle)
                        .with_point_compliance(1.0e-5),
                    JointCollisionDisabled,
                ));
            }
            Suspension::Spring => {
                // Proper articulated suspension:
                //   chassis →prismatic(ForceBased spring)→ hub →revolute→ wheel
                //
                // The hub carries a small NON-colliding collider purely so
                // Avian computes a real angular-inertia tensor for it. Without
                // it the hub is a mass-only dynamic body with degenerate
                // inertia, and the revolute that feeds drive-torque reaction
                // into the hub makes the solver diverge (NaN). CollisionLayers
                // NONE means it never generates contacts.
                let hub = commands
                    .spawn((
                        Name::new(format!("Hub_{i}")),
                        RigidBody::Dynamic,
                        Collider::sphere(HUB_RADIUS),
                        Mass(HUB_MASS as f32),
                        CollisionLayers::new(LayerMask::NONE, LayerMask::NONE),
                        Transform::from_translation(c_world.as_vec3()),
                    ))
                    .id();

                // Avian's SpringDamper is IMPLICIT (unconditionally stable),
                // but it realizes k_eff = reduced_mass·(2πf)², where the
                // reduced mass of the prismatic's two bodies is dominated by
                // the lighter hub. So the frequency must be derived from the
                // REDUCED mass, not the sprung corner mass, or the spring comes
                // out ~25× too soft and the chassis sinks to the travel limit.
                let reduced = (CHASSIS_MASS * HUB_MASS) / (CHASSIS_MASS + HUB_MASS);
                let frequency = cfg.spring_scale
                    * (SPRING_K / reduced).sqrt()
                    / (2.0 * std::f64::consts::PI);
                let damping_ratio =
                    cfg.damp_scale * SPRING_C / (2.0 * (SPRING_K * reduced).sqrt());

                commands.spawn((
                    PrismaticJoint::new(chassis, hub)
                        .with_local_anchor1(chassis_anchor)
                        .with_local_anchor2(DVec3::ZERO)
                        .with_slider_axis(DVec3::Y)
                        .with_limits(-SUSP_TRAVEL, SUSP_TRAVEL)
                        .with_motor(
                            LinearMotor::new(MotorModel::SpringDamper {
                                frequency,
                                damping_ratio,
                            })
                            .with_target_position(0.0),
                        ),
                    JointCollisionDisabled,
                ));

                commands.spawn((
                    RevoluteJoint::new(hub, wheel)
                        .with_local_anchor1(DVec3::ZERO)
                        .with_local_anchor2(DVec3::ZERO)
                        .with_hinge_axis(axle),
                    JointCollisionDisabled,
                ));
            }
        }
    }
}

/// Drive every wheel. Two modes:
///   - torque (default): a couple on the wheel about its axle. The rigid
///     revolute transmits the reaction couple straight into the chassis as a
///     nose-up pitch → wheelie/launch at speed.
///   - force (`--drivemode=force`): a forward force applied at the wheel's
///     ground-contact point. Its moment about the (free) axle spins the wheel;
///     the linear part propels the chassis. NO reaction couple → only the real
///     traction pitch remains. This is what the raycast wheel does.
fn apply_drive(cfg: Res<Config>, mut q: Query<(&DriveWheel, Forces)>) {
    if cfg.drive == 0.0 {
        return;
    }
    for (w, mut forces) in q.iter_mut() {
        if cfg.force_drive {
            // Equivalent traction for the authored torque: F = τ / r.
            let force_mag = cfg.drive * DRIVE_PEAK_TORQUE / WHEEL_RADIUS;
            // Forward = horizontal, perpendicular to the axle (up × axle).
            let forward = DVec3::Y.cross(w.axle).normalize();
            // Contact point = directly below the wheel centre.
            let contact = forces.position().0 - DVec3::Y * WHEEL_RADIUS;
            forces.apply_force_at_point(forward * force_mag, contact);
        } else {
            forces.apply_torque(w.axle * (cfg.drive * DRIVE_PEAK_TORQUE));
        }
    }
}

/// Accumulate chassis jitter stats during the sample window.
fn sample_chassis(
    cfg: Res<Config>,
    mut stats: ResMut<Stats>,
    q: Query<(&AngularVelocity, &LinearVelocity, &Position), With<Chassis>>,
) {
    stats.tick += 1;
    if stats.tick <= cfg.settle_ticks {
        return;
    }
    let Ok((ang, lin, pos)) = q.single() else {
        return;
    };
    let a = ang.0.length();
    let v = lin.0.length();
    if stats.n == 0 {
        stats.height_min = pos.0.y;
        stats.height_max = pos.0.y;
    }
    stats.ang_max = stats.ang_max.max(a);
    stats.lin_max = stats.lin_max.max(v);
    stats.ang_sumsq += a * a;
    stats.lin_sumsq += v * v;
    stats.height_min = stats.height_min.min(pos.0.y);
    stats.height_max = stats.height_max.max(pos.0.y);
    stats.n += 1;
}
