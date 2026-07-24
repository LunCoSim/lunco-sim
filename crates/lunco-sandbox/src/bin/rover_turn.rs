//! Headless rover-STEERING probe (sibling of `rover_jitter`).
//!
//! Builds a joint-based rover **in code** (no USD, no render, no GPU,
//! no FloatingOrigin) that mirrors the real `setup_physical_wheel`
//! architecture, then drives it and measures whether the chassis
//! actually TURNS. Two locomotion modes:
//!
//!   * `skid`      — 4 rigid `RevoluteJoint` wheels, differential torque
//!                   (left = drive+steer, right = drive−steer). Yaw comes
//!                   from the left/right torque split (skid steer).
//!   * `ackermann` — front 2 wheels each carry a steering knuckle
//!                   (chassis →Y-revolute+AngularMotor→ knuckle →X-revolute→
//!                   wheel). AWD: every wheel is torque-driven about its
//!                   (steered) axle. Yaw comes from the castored front tyres.
//!
//! Parameters mirror the shipped assets: friction 0.9, peakTorque 300 N·m,
//! wheel 100 kg, chassis 1000 kg, knuckle 30 kg, steering motor
//! SpringDamper{f=2,ζ=1}, max steer 0.5 rad, SubstepCount 12, lunar g.
//!
//! ```text
//! cargo run -p lunco-sandbox --bin rover_turn -j2 -- --mode=skid --drive=1 --steer=0
//! cargo run -p lunco-sandbox --bin rover_turn -j2 -- --mode=skid --drive=1 --steer=1
//! cargo run -p lunco-sandbox --bin rover_turn -j2 -- --mode=ackermann --drive=1 --steer=1
//! ```
//!
//! Reports net heading change (deg), yaw rate (deg/s) and ground-plane
//! displacement over the sample window — the objective "does it turn" metric.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::app::ScheduleRunnerPlugin;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

// ── Authored rover parameters (mirror assets/.../*rover*.usda + scene) ──
const CHASSIS_MASS: f64 = 1000.0;
const WHEEL_MASS: f64 = 100.0;
const KNUCKLE_MASS: f64 = 30.0;
const WHEEL_RADIUS: f64 = 0.4;
const WHEEL_WIDTH: f64 = 0.3;
const PEAK_TORQUE: f64 = 300.0; // physxVehicleEngine:peakTorque (N·m) — drive motor max_torque
const MAX_STEER: f64 = 0.5; // physxVehicleAckermannSteering:maxSteerAngle (rad)
const STEER_FREQ: f64 = 2.0; // knuckle SpringDamper frequency (Hz)
const STEER_DAMP: f64 = 1.0; // knuckle SpringDamper damping ratio
const MAX_OMEGA: f64 = 12.0; // wheel spin (rad/s) at full throttle ≈ v_max/r
const DRIVE_DAMP: f64 = 30.0; // AccelerationBased velocity-tracking damping (1/s)

// Chassis half-extents (scale 2.0,0.3,3.5 on a unit cube) and wheel layout
// from sandbox_scene.usda (wheels BELOW the body, no overlap).
const CHASSIS_HE: DVec3 = DVec3::new(1.0, 0.15, 1.75);
const WHEEL_X: f64 = 0.9;
const WHEEL_Z: f64 = 1.225;
const WHEEL_DY: f64 = -0.65; // wheel centre relative to chassis centre
const CHASSIS_Y: f64 = 1.05; // chassis centre rest height (wheel just touches y=0)

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Skid,
    Ackermann,
}

#[derive(Resource, Clone)]
struct Config {
    mode: Mode,
    drive: f64,
    steer: f64,
    substeps: u32,
    gravity: f64,
    friction: f64,
    lin_damp: f64,
    ang_damp: f64,
    peak_torque: f64,
    omega: f64,
    steer_freq: f64,
    knuckle_mass: f64,
    knuckle_radius: f64,
    max_steer: f64,
    front_drive: bool,
    settle_ticks: u64,
    sample_ticks: u64,
}

#[derive(Component)]
struct Chassis;

/// A velocity-driven wheel-axle joint. `side` = +1 for the left group, −1 for
/// the right group (used by the skid differential). The drive system writes
/// this joint's `AngularMotor::target_velocity` each tick; the motor applies up
/// to `max_torque` to reach the commanded spin, and the tyre↔ground friction
/// propels the rover — genuine physics-engine drive through the wheel.
#[derive(Component)]
struct DriveJoint {
    side: f64,
}

/// Marks a front wheel-axle joint that steers by rotating its body1 (chassis)
/// frame about Y. `apply_steer` yaws the wheel into the steered heading through
/// this one constraint — no floating knuckle body (which is solver-unstable in
/// avian 0.6.1).
#[derive(Component)]
struct SteerFrame;

#[derive(Resource, Default)]
struct Stats {
    tick: u64,
    started: bool,
    // sample-window start pose
    yaw0: f64,
    x0: f64,
    z0: f64,
    // latest pose
    yaw: f64,
    x: f64,
    z: f64,
    speed_max: f64,
}

/// skid differential mix, identical to `lunco_mobility::skid_mix` but in f64
/// throttle space (−1..=1) rather than i16.
fn skid_mix(forward: f64, steer: f64) -> (f64, f64) {
    let steer = steer.clamp(-1.0, 1.0);
    let drive = forward.clamp(-1.0, 1.0) * (1.0 - 0.5 * steer.abs());
    let l = drive + steer;
    let r = drive - steer;
    let m = l.abs().max(r.abs()).max(1.0);
    (l / m, r / m)
}

/// Heading (yaw) of a rotation: angle of the forward (−Z) vector in the XZ
/// ground plane. Unwrapped continuity is handled by the caller.
fn yaw_of(rot: bevy::math::DQuat) -> f64 {
    let fwd = rot * DVec3::NEG_Z;
    fwd.x.atan2(-fwd.z) // 0 when facing −Z, +ve as it turns toward +X
}

fn parse_args() -> Config {
    let mut mode = Mode::Skid;
    let mut drive = 1.0;
    let mut steer = 1.0;
    let mut substeps = 12u32;
    let mut gravity = 1.62;
    let mut friction = 0.9;
    let mut lin_damp = 0.1;
    let mut ang_damp = 0.3;
    let mut peak_torque = PEAK_TORQUE;
    let mut omega = MAX_OMEGA;
    let mut steer_freq = STEER_FREQ;
    let mut knuckle_mass = KNUCKLE_MASS;
    let mut knuckle_radius = 0.1f64;
    let mut max_steer = MAX_STEER;
    let mut front_drive = true;
    let mut settle = 1.0f64;
    let mut sample = 3.0f64;

    for arg in std::env::args().skip(1) {
        let Some((k, v)) = arg.trim_start_matches("--").split_once('=') else {
            continue;
        };
        match k {
            "mode" => {
                mode = match v {
                    "skid" => Mode::Skid,
                    "ackermann" | "ack" => Mode::Ackermann,
                    other => {
                        eprintln!("unknown mode '{other}', using skid");
                        Mode::Skid
                    }
                }
            }
            "drive" => drive = v.parse().unwrap_or(1.0),
            "steer" => steer = v.parse().unwrap_or(1.0),
            "substeps" => substeps = v.parse().unwrap_or(12),
            "gravity" => gravity = v.parse().unwrap_or(1.62),
            "friction" => friction = v.parse().unwrap_or(0.9),
            "lindamp" => lin_damp = v.parse().unwrap_or(0.1),
            "angdamp" => ang_damp = v.parse().unwrap_or(0.3),
            "torque" => peak_torque = v.parse().unwrap_or(PEAK_TORQUE),
            "omega" => omega = v.parse().unwrap_or(MAX_OMEGA),
            "steerfreq" => steer_freq = v.parse().unwrap_or(STEER_FREQ),
            "knucklemass" => knuckle_mass = v.parse().unwrap_or(KNUCKLE_MASS),
            "knuckleradius" => knuckle_radius = v.parse().unwrap_or(0.1),
            "frontdrive" => front_drive = v != "0",
            "maxsteer" => max_steer = v.parse().unwrap_or(MAX_STEER),
            "settle" => settle = v.parse().unwrap_or(1.0),
            "sample" => sample = v.parse().unwrap_or(3.0),
            _ => {}
        }
    }

    Config {
        mode,
        drive,
        steer,
        substeps,
        gravity,
        friction,
        lin_damp,
        ang_damp,
        peak_torque,
        omega,
        steer_freq,
        knuckle_mass,
        knuckle_radius,
        max_steer,
        front_drive,
        settle_ticks: (settle * 60.0).round() as u64,
        sample_ticks: (sample * 60.0).round() as u64,
    }
}

fn main() {
    let cfg = parse_args();
    let total = cfg.settle_ticks + cfg.sample_ticks;
    let dt = Duration::from_secs_f64(1.0 / 60.0);

    let mut app = App::new();
    app.add_plugins(MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::ZERO)))
        .add_plugins(bevy::transform::TransformPlugin)
        .add_plugins(bevy::asset::AssetPlugin::default())
        .init_asset::<Mesh>()
        .add_plugins(PhysicsPlugins::default())
        .insert_resource(Gravity(DVec3::new(0.0, -cfg.gravity, 0.0)))
        .insert_resource(SubstepCount(cfg.substeps))
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(TimeUpdateStrategy::ManualDuration(dt))
        .insert_resource(cfg.clone())
        .insert_resource(Stats::default())
        .add_systems(Startup, setup)
        .add_systems(FixedUpdate, (apply_steer, apply_drive).chain())
        .add_systems(FixedLast, sample_chassis);

    app.finish();
    app.cleanup();

    for _ in 0..total {
        app.update();
    }

    let s = app.world().resource::<Stats>();
    let mode = match cfg.mode {
        Mode::Skid => "skid     ",
        Mode::Ackermann => "ackermann",
    };
    let dyaw = (s.yaw - s.yaw0).to_degrees();
    let dx = s.x - s.x0;
    let dz = s.z - s.z0;
    let dist = (dx * dx + dz * dz).sqrt();
    let secs = cfg.sample_ticks as f64 / 60.0;
    println!(
        "\n=== rover_turn: mode={mode} drive={:.2} steer={:.2} g={:.2} \
         friction={:.2} torque={:.0} substeps={} ===",
        cfg.drive, cfg.steer, cfg.gravity, cfg.friction, cfg.peak_torque, cfg.substeps
    );
    println!(
        "sample={:.2}s  heading change = {:+8.2} deg   ({:+7.2} deg/s)",
        secs,
        dyaw,
        dyaw / secs
    );
    println!(
        "ground displacement = {:6.3} m   (dx={:+.3}, dz={:+.3})   max speed = {:.3} m/s",
        dist, dx, dz, s.speed_max
    );
    let verdict = if dyaw.abs() < 5.0 {
        "NO TURN"
    } else if dyaw.abs() < 20.0 {
        "weak turn"
    } else {
        "TURNS"
    };
    println!("verdict: {verdict}\n");
}

fn setup(mut commands: Commands, cfg: Res<Config>) {
    // Static ground, top face at y = 0.
    commands.spawn((
        Name::new("Ground"),
        RigidBody::Static,
        Collider::cuboid(400.0, 1.0, 400.0),
        Friction::new(cfg.friction),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));

    let chassis = commands
        .spawn((
            Name::new("Chassis"),
            Chassis,
            RigidBody::Dynamic,
            Collider::cuboid(CHASSIS_HE.x * 2.0, CHASSIS_HE.y * 2.0, CHASSIS_HE.z * 2.0),
            Mass(CHASSIS_MASS as f32),
            Friction::new(cfg.friction),
            Transform::from_xyz(0.0, CHASSIS_Y as f32, 0.0),
        ))
        .id();

    // Wheel layout: index 0=FL,1=FR,2=RL,3=RR. Front = |z|>0 toward −Z, but
    // the sign only matters for which pair steers; we steer the two front
    // (z = +WHEEL_Z here) wheels. side = +1 left (x>0), −1 right (x<0).
    let corners = [
        (DVec3::new(WHEEL_X, CHASSIS_Y + WHEEL_DY, WHEEL_Z), true), // FL front-left
        (DVec3::new(-WHEEL_X, CHASSIS_Y + WHEEL_DY, WHEEL_Z), true), // FR front-right
        (DVec3::new(WHEEL_X, CHASSIS_Y + WHEEL_DY, -WHEEL_Z), false), // RL
        (DVec3::new(-WHEEL_X, CHASSIS_Y + WHEEL_DY, -WHEEL_Z), false), // RR
    ];

    // Avian's cylinder is Y-native; rotate Y→X so the axle lies along X.
    let wheel_rot = Quat::from_rotation_arc(Vec3::Y, Vec3::X);
    let axle = DVec3::X; // hinge axis in world/parent frame at spawn

    // Drive motor: pure velocity control (stiffness 0), mass-auto-scaled,
    // clamped to peak_torque. Built per joint so target_velocity is set live.
    let drive_motor = || {
        AngularMotor::new(MotorModel::AccelerationBased {
            stiffness: 0.0,
            damping: DRIVE_DAMP,
        })
        .with_max_torque(cfg.peak_torque)
    };

    for (i, (c, is_front)) in corners.into_iter().enumerate() {
        let side = if c.x > 0.0 { 1.0 } else { -1.0 };
        let chassis_anchor = DVec3::new(c.x, c.y - CHASSIS_Y, c.z);

        let wheel = commands
            .spawn((
                Name::new(format!("Wheel_{i}")),
                RigidBody::Dynamic,
                Collider::cylinder(WHEEL_RADIUS, WHEEL_WIDTH),
                Mass(WHEEL_MASS as f32),
                Friction::new(cfg.friction),
                LinearDamping(cfg.lin_damp),
                AngularDamping(cfg.ang_damp),
                Transform::from_translation(c.as_vec3()).with_rotation(wheel_rot),
            ))
            .id();

        let steered = cfg.mode == Mode::Ackermann && is_front;
        let drive_this = !steered || cfg.front_drive;

        // Every wheel: a SINGLE revolute straight to the chassis (stable, like
        // the skid wheels). Front wheels additionally get a SteerFrame marker;
        // `apply_steer` rotates this joint's chassis-side frame about Y so the
        // wheel physically yaws into the steered heading — geometric Ackermann
        // through one constraint, no floating knuckle.
        let mut joint = RevoluteJoint::new(chassis, wheel)
            .with_local_anchor1(chassis_anchor)
            .with_local_anchor2(DVec3::ZERO)
            .with_hinge_axis(axle);
        if drive_this {
            joint = joint.with_motor(drive_motor());
        }
        let mut e = commands.spawn((joint, JointCollisionDisabled));
        if drive_this {
            e.try_insert(DriveJoint { side });
        }
        if steered {
            e.try_insert(SteerFrame);
        }
    }
}

/// Steer the front wheels by rotating each steered joint's chassis-side frame
/// about Y. The revolute's alignment constraint then yaws the wheel to match,
/// so the wheel physically points into the steered heading and its rolling +
/// lateral grip redirect the rover — real geometric Ackermann, one constraint.
fn apply_steer(cfg: Res<Config>, mut q: Query<&mut RevoluteJoint, With<SteerFrame>>) {
    let angle = cfg.steer.clamp(-1.0, 1.0) * cfg.max_steer;
    let basis = JointBasis::Local(DQuat::from_rotation_y(angle).into());
    for mut joint in q.iter_mut() {
        // Only the basis (orientation) changes; the anchor (pivot) is preserved.
        joint.frame1.basis = basis;
    }
}

/// Set each drive joint's target spin from the command — mirrors the
/// velocity-motor `motor_actuator_system`. Skid uses a left/right differential;
/// Ackermann drives all four equally (AWD) and turns via the knuckles.
/// Forward (−Z roll) is a NEGATIVE relative ω about the +X axle.
fn apply_drive(cfg: Res<Config>, mut q: Query<(&DriveJoint, &mut RevoluteJoint)>) {
    let (l_mix, r_mix) = skid_mix(cfg.drive, cfg.steer);
    for (dj, mut joint) in q.iter_mut() {
        let throttle = match cfg.mode {
            Mode::Skid => {
                if dj.side > 0.0 {
                    l_mix
                } else {
                    r_mix
                }
            }
            // AWD non-differential: every wheel gets the same forward throttle;
            // the front knuckles do the steering.
            Mode::Ackermann => cfg.drive.clamp(-1.0, 1.0),
        };
        joint.motor.target_velocity = -throttle * cfg.omega;
    }
}

fn sample_chassis(
    cfg: Res<Config>,
    mut stats: ResMut<Stats>,
    q: Query<(&Rotation, &Position, &LinearVelocity), With<Chassis>>,
) {
    stats.tick += 1;
    let Ok((rot, pos, lin)) = q.single() else {
        return;
    };
    let yaw = yaw_of(rot.0);
    if stats.tick <= cfg.settle_ticks {
        return;
    }
    if !stats.started {
        stats.started = true;
        stats.yaw0 = yaw;
        stats.yaw = yaw;
        stats.x0 = pos.0.x;
        stats.z0 = pos.0.z;
        stats.x = pos.0.x;
        stats.z = pos.0.z;
        return;
    }
    // Unwrap yaw continuity relative to the previous sample.
    let prev = stats.yaw;
    let mut y = yaw;
    while y - prev > std::f64::consts::PI {
        y -= std::f64::consts::TAU;
    }
    while y - prev < -std::f64::consts::PI {
        y += std::f64::consts::TAU;
    }
    stats.yaw = y;
    stats.x = pos.0.x;
    stats.z = pos.0.z;
    let speed = (lin.0.x * lin.0.x + lin.0.z * lin.0.z).sqrt();
    stats.speed_max = stats.speed_max.max(speed);
}
