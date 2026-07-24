//! Headless rollback probe on a REAL ARTICULATED ROVER.
//!
//! `rollback_probe` validated deterministic rollback on a single rigid body and got
//! 0.24 mm — then the live client tore the rover apart and launched it tens of metres.
//! The gap: the real vehicle is a JOINTED ASSEMBLY (chassis + 4 wheels + revolute
//! motors), and a rollback that seats the chassis alone violates every joint. The
//! single-body probe could not express that failure, so it could not catch it.
//!
//! This probe closes that gap. It builds a genuine articulated vehicle on the real
//! avian solver — the same `RevoluteJoint` + `AngularMotor` construction the rover uses
//! (`lunco_usd_avian::wheel_revolute_joint`) — and compares three strategies against an
//! authoritative host run:
//!
//!   * NONE          — no reconcile (baseline divergence).
//!   * CHASSIS_ONLY  — rollback the chassis, leave the wheels. THE LIVE BUG (`links=0`).
//!                     Reproduced deliberately: this must FAIL loudly, or the harness
//!                     isn't testing the thing that actually broke.
//!   * FULL_ASSEMBLY — rigid re-frame of chassis + every link, then input replay. THE FIX.
//!
//! Two metrics, because chassis position error alone HID the bug:
//!   * `pos_err`      — chassis distance from the host (does it reconcile?).
//!   * `joint_strain` — how far each wheel has drifted from its rest mount on the chassis
//!                      (does the vehicle stay INTACT?). A torn joint shows up here first,
//!                      before the launch shows up in `pos_err`.
//!
//!   cargo run --bin rollback_rover_probe

use avian3d::prelude::*;
use bevy::app::{
    ScheduleRunnerPlugin, TaskPoolOptions, TaskPoolPlugin, TaskPoolThreadAssignmentPolicy,
};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use std::time::Duration;

const TICKS: usize = 420;
/// Snapshot latency in ticks: the ack for tick T lands at T + ACK_DELAY (≈130 ms).
const ACK_DELAY: usize = 8;
/// Ack cadence: 20 Hz snapshots against a 60 Hz sim.
const SNAPSHOT_INTERVAL: usize = 3;
/// Wheel angular speed (rad/s) at full throttle.
const WHEEL_OMEGA: f64 = 12.0;
/// Wheel mount anchors in chassis-local space (the joint rest offsets).
const MOUNTS: [DVec3; 4] = [
    DVec3::new(-0.7, -0.3, -1.0),
    DVec3::new(0.7, -0.3, -1.0),
    DVec3::new(-0.7, -0.3, 1.0),
    DVec3::new(0.7, -0.3, 1.0),
];

#[derive(Resource, Clone, Copy, Default)]
struct DriveInput {
    throttle: f64,
    steer: f64,
}

/// Public, network-transmissible state — all a snapshot carries, all a restore may touch.
#[derive(Clone, Copy)]
struct RbState {
    pos: DVec3,
    rot: DQuat,
    lv: DVec3,
    av: DVec3,
}

/// The whole vehicle at one tick: chassis + every wheel. The client keeps this LOCALLY —
/// the wire replicates the chassis ONLY (`apply_net_replication` excludes ArticulatedLink).
#[derive(Clone)]
struct Assembly {
    chassis: RbState,
    links: Vec<RbState>,
}

#[derive(Component)]
struct Chassis;
#[derive(Component)]
struct WheelId(usize);

fn compute_pool(threads: usize) -> TaskPoolPlugin {
    TaskPoolPlugin {
        task_pool_options: TaskPoolOptions {
            compute: TaskPoolThreadAssignmentPolicy {
                min_threads: threads,
                max_threads: threads,
                percent: 1.0,
                on_thread_spawn: None,
                on_thread_destroy: None,
            },
            ..default()
        },
    }
}

/// Skid steer: left and right wheels get opposite steer bias — the differential drive the
/// real rover's control-allocation kernel produces from (throttle, steer).
fn drive_wheels(input: Res<DriveInput>, mut q: Query<(&WheelId, &mut RevoluteJoint)>) {
    for (w, mut joint) in q.iter_mut() {
        let left = w.0 % 2 == 0;
        let bias = if left { -input.steer } else { input.steer };
        joint.motor.enabled = true;
        joint.motor.target_velocity = (input.throttle + bias).clamp(-1.5, 1.5) * WHEEL_OMEGA;
    }
}

fn make_app(threads: usize) -> App {
    let mut app = App::new();
    app.add_plugins(
        MinimalPlugins
            .set(compute_pool(threads))
            .set(ScheduleRunnerPlugin::run_loop(Duration::ZERO)),
    )
    .add_plugins(bevy::transform::TransformPlugin)
    .add_plugins(bevy::asset::AssetPlugin::default())
    .init_asset::<Mesh>()
    .add_plugins(PhysicsPlugins::default())
    .insert_resource(Gravity(DVec3::new(0.0, -9.81, 0.0)))
    .insert_resource(SubstepCount(12))
    .insert_resource(Time::<Fixed>::from_hz(60.0))
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )))
    .init_resource::<DriveInput>()
    .add_systems(Startup, setup)
    .add_systems(FixedUpdate, drive_wheels);
    app.finish();
    app.cleanup();
    // Settle the vehicle onto its wheels/contacts before anything is recorded.
    for _ in 0..40 {
        app.update();
    }
    app
}

fn setup(mut commands: Commands) {
    commands.spawn((
        RigidBody::Static,
        Collider::cuboid(800.0, 1.0, 800.0),
        Friction::new(1.0),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));

    let chassis = commands
        .spawn((
            Chassis,
            RigidBody::Dynamic,
            Collider::cuboid(1.2, 0.4, 2.2),
            Mass(40.0),
            Transform::from_xyz(0.0, 0.65, 0.0),
        ))
        .id();

    for (i, mount) in MOUNTS.iter().enumerate() {
        let wheel = commands
            .spawn((
                WheelId(i),
                RigidBody::Dynamic,
                Collider::sphere(0.35),
                Mass(5.0),
                Friction::new(1.5),
                Transform::from_xyz(mount.x as f32, 0.65 + mount.y as f32, mount.z as f32),
            ))
            .id();
        // NOTE: wheels are deliberately NOT ECS children of the chassis here. avian's
        // `position_to_transform` writes a WORLD pose into `Transform`; if that Transform
        // belongs to a child, bevy re-composes it with the parent and the collider ends up
        // in the wrong place (the vehicle then can't get traction). The real rover only
        // nests them because the big_space bridge DISABLES avian's position_to_transform
        // and owns the sync itself. The joint — not the hierarchy — is what binds a wheel
        // to the chassis, and the joint is what this probe is testing.

        // A PURE VELOCITY motor: `stiffness: 0.0` (avian: "set to zero for pure velocity
        // control"), so torque = damping · (target_velocity − actual). A SpringDamper model
        // with `target_position: 0.0` would spring-hold each wheel at angle zero and fight
        // its own spin — the "position-hold ... freezes the wheel" failure `lunco-hardware`
        // explicitly warns about, and what kept this rover parked at ~2 m.
        let motor = AngularMotor {
            enabled: true,
            target_velocity: 0.0,
            target_position: 0.0,
            max_torque: 1.0e6,
            motor_model: MotorModel::ForceBased {
                stiffness: 0.0,
                damping: 3000.0,
            },
        };

        commands.spawn((
            WheelId(i),
            RevoluteJoint::new(chassis, wheel)
                .with_local_anchor1(*mount)
                .with_local_anchor2(DVec3::ZERO)
                .with_hinge_axis(DVec3::X)
                .with_motor(motor),
        ));
    }
}

fn step(app: &mut App, input: DriveInput) {
    *app.world_mut().resource_mut::<DriveInput>() = input;
    app.update();
}

fn read_assembly(app: &mut App) -> Assembly {
    let chassis = {
        let mut q = app.world_mut().query_filtered::<
            (&Position, &Rotation, &LinearVelocity, &AngularVelocity),
            With<Chassis>,
        >();
        let (p, r, lv, av) = q.single(app.world()).expect("one chassis");
        RbState {
            pos: p.0,
            rot: r.0,
            lv: lv.0,
            av: av.0,
        }
    };
    let mut links: Vec<(usize, RbState)> = {
        let mut q = app.world_mut().query_filtered::<(
            &WheelId,
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
        ), With<RigidBody>>();
        q.iter(app.world())
            .map(|(w, p, r, lv, av)| {
                (
                    w.0,
                    RbState {
                        pos: p.0,
                        rot: r.0,
                        lv: lv.0,
                        av: av.0,
                    },
                )
            })
            .collect()
    };
    links.sort_by_key(|(i, _)| *i);
    Assembly {
        chassis,
        links: links.into_iter().map(|(_, s)| s).collect(),
    }
}

fn write_assembly(app: &mut App, a: &Assembly) {
    {
        let mut q = app.world_mut().query_filtered::<(
            &mut Position,
            &mut Rotation,
            &mut LinearVelocity,
            &mut AngularVelocity,
        ), With<Chassis>>();
        let world = app.world_mut();
        let (mut p, mut r, mut lv, mut av) = q.single_mut(world).expect("one chassis");
        p.0 = a.chassis.pos;
        r.0 = a.chassis.rot;
        lv.0 = a.chassis.lv;
        av.0 = a.chassis.av;
    }
    if a.links.is_empty() {
        return; // CHASSIS_ONLY: deliberately leave the wheels behind — the live bug.
    }
    let mut q = app.world_mut().query_filtered::<(
        &WheelId,
        &mut Position,
        &mut Rotation,
        &mut LinearVelocity,
        &mut AngularVelocity,
    ), With<RigidBody>>();
    let world = app.world_mut();
    for (w, mut p, mut r, mut lv, mut av) in q.iter_mut(world) {
        let Some(s) = a.links.get(w.0) else { continue };
        p.0 = s.pos;
        r.0 = s.rot;
        lv.0 = s.lv;
        av.0 = s.av;
    }
}

/// Rigidly re-frame a recorded assembly onto an authoritative chassis state: the chassis
/// lands exactly on authority and every link is carried with it, so joint offsets, wheel
/// spin and contact config stay internally consistent. THIS IS THE FIX UNDER TEST.
fn reframe(pred: &Assembly, auth: RbState) -> Assembly {
    let d_rot = auth.rot * pred.chassis.rot.inverse();
    let links = pred
        .links
        .iter()
        .map(|l| RbState {
            pos: auth.pos + d_rot * (l.pos - pred.chassis.pos),
            rot: d_rot * l.rot,
            lv: auth.lv + d_rot * (l.lv - pred.chassis.lv),
            av: auth.av + d_rot * (l.av - pred.chassis.av),
        })
        .collect();
    Assembly {
        chassis: auth,
        links,
    }
}

/// Worst wheel drift from its rest mount on the chassis. ~0 ⇒ vehicle intact; large ⇒
/// joints torn (the live failure, which chassis-position error alone did not reveal).
fn joint_strain(a: &Assembly) -> f64 {
    let mut worst: f64 = 0.0;
    for (i, l) in a.links.iter().enumerate() {
        let expected = a.chassis.pos + a.chassis.rot * MOUNTS[i];
        worst = worst.max((l.pos - expected).length());
    }
    worst
}

fn gen_input(t: usize) -> DriveInput {
    DriveInput {
        throttle: 1.0,
        steer: 0.5 * (t as f64 / 40.0).sin(),
    }
}

fn run_host(threads: usize) -> (Vec<DriveInput>, Vec<Assembly>) {
    let mut app = make_app(threads);
    let mut inputs = Vec::with_capacity(TICKS);
    let mut states = Vec::with_capacity(TICKS);
    for t in 0..TICKS {
        let i = gen_input(t);
        step(&mut app, i);
        inputs.push(i);
        states.push(read_assembly(&mut app));
    }
    (inputs, states)
}

#[derive(Clone, Copy, PartialEq)]
enum Strategy {
    None,
    ChassisOnly,
    FullAssembly,
    /// Rollback onto a COMPLETE authoritative state — chassis *and* every wheel (spin,
    /// contact, suspension). This is only possible if the wire REPLICATES the links, which
    /// today it does not (`apply_net_replication` excludes `ArticulatedLink`).
    ///
    /// It is the control experiment for the central question: is rollback on this rover
    /// limited by the ALGORITHM, or by MISSING INFORMATION? If this converges while
    /// `FullAssembly` does not, then no amount of tuning will fix chassis-only rollback —
    /// the wheels must be replicated. If it ALSO fails to converge, the problem is deeper
    /// and replicating wheels would be wasted bandwidth.
    FullAuthority,
}

fn run_client(
    strat: Strategy,
    inputs: &[DriveInput],
    host: &[Assembly],
    threads: usize,
) -> (Vec<f64>, Vec<f64>) {
    let mut app = make_app(threads);

    // SEED ERROR: a freshly-promoted predicted body starts off-pose (the stale seed).
    // Applied as a rigid re-frame so the vehicle starts INTACT — we're testing
    // reconciliation, not spawning a pre-broken rover.
    let mut off = host[0].chassis;
    off.pos += DVec3::new(0.5, 0.0, 0.35);
    off.rot = DQuat::from_rotation_y(0.06) * off.rot;
    let seed = reframe(&host[0], off);
    write_assembly(&mut app, &seed);

    let mut predicted: Vec<Assembly> = Vec::with_capacity(TICKS);
    let mut errs = vec![0.0; TICKS];
    let mut strains = vec![0.0; TICKS];

    for t in 0..TICKS {
        // A torn assembly diverges without bound and eventually feeds NaN into avian,
        // which panics inside the solver (`update_solver_body_aabbs`) and takes the whole
        // process down before the other strategies can run. Bail out the moment a run has
        // demonstrably blown up: record it as blown up (which IS the verdict) rather than
        // stepping a NaN world. This is what a torn joint does on a MOVING rover — exactly
        // the live failure — so it must be observable, not fatal.
        let cur = read_assembly(&mut app);
        let blown = !cur.chassis.pos.is_finite() || cur.chassis.pos.length() > 1.0e4;
        if blown {
            for k in t..TICKS {
                errs[k] = f64::INFINITY;
                strains[k] = f64::INFINITY;
            }
            break;
        }

        step(&mut app, inputs[t]);
        predicted.push(read_assembly(&mut app));

        if strat != Strategy::None && t >= ACK_DELAY && t % SNAPSHOT_INTERVAL == 0 {
            let ack = t - ACK_DELAY;
            let auth = host[ack].chassis; // the wire carries the CHASSIS ONLY
            let pred = &predicted[ack];

            let target = match strat {
                Strategy::None => unreachable!(),
                // THE BUG: seat the chassis on authority, leave the wheels behind.
                Strategy::ChassisOnly => Assembly {
                    chassis: auth,
                    links: Vec::new(),
                },
                // THE FIX (as shipped): rigid re-frame — authoritative chassis, but the
                // wheels are our own PREDICTED ones carried along. Internally consistent
                // (joints intact) yet only partly authoritative.
                Strategy::FullAssembly => reframe(pred, auth),
                // CONTROL: complete authority, wheels included (requires replicating links).
                Strategy::FullAuthority => host[ack].clone(),
            };
            write_assembly(&mut app, &target);

            // Deterministically replay every unacked input back to the present.
            for r in (ack + 1)..=t {
                step(&mut app, inputs[r]);
            }
        }

        let now = read_assembly(&mut app);
        errs[t] = (now.chassis.pos - host[t].chassis.pos).length();
        strains[t] = joint_strain(&now);
    }
    (errs, strains)
}

fn tail_mean(v: &[f64]) -> f64 {
    let t = &v[v.len() / 2..];
    t.iter().sum::<f64>() / t.len() as f64
}

fn main() {
    let threads = 8usize;
    println!("=== rollback probe: ARTICULATED ROVER (chassis + 4 jointed wheels) ===");
    println!(
        "    {TICKS} ticks | ack delay {ACK_DELAY} | snapshots every {SNAPSHOT_INTERVAL} ticks\n"
    );

    let (inputs, host) = run_host(threads);
    let drove = (host.last().unwrap().chassis.pos - host[0].chassis.pos).length();
    let base_strain = tail_mean(&host.iter().map(joint_strain).collect::<Vec<_>>());
    println!(
        "host drove {drove:.2} m | joint_strain={base_strain:.4} m  (baseline: intact vehicle)\n"
    );

    // A rover that barely moves makes every strategy look good — reconciling a parked
    // vehicle is trivial and would hide exactly the turning dynamics that broke live.
    // Refuse to report a verdict on a scenario that isn't actually driving.
    if drove < 5.0 {
        println!(
            "!! INVALID SCENARIO: host only drove {drove:.2} m — the rover is not being driven, \n\
             so these results prove NOTHING about a moving vehicle. Fix the drive before trusting them."
        );
    }

    for (name, strat) in [
        ("NONE           (no reconcile)      ", Strategy::None),
        ("CHASSIS_ONLY   (THE LIVE BUG)      ", Strategy::ChassisOnly),
        (
            "FULL_ASSEMBLY  (chassis authority) ",
            Strategy::FullAssembly,
        ),
        (
            "FULL_AUTHORITY (+ wheels on wire)  ",
            Strategy::FullAuthority,
        ),
    ] {
        let (errs, strains) = run_client(strat, &inputs, &host, threads);
        let blew_up = errs.iter().any(|e| !e.is_finite());
        if blew_up {
            println!(
                "{name}: *** BLEW UP *** (assembly torn -> unbounded divergence -> NaN; \
                 avian's solver cannot survive this)"
            );
            continue;
        }
        let pos = tail_mean(&errs);
        let max = errs.iter().cloned().fold(0.0, f64::max);
        let strain = tail_mean(&strains);
        let verdict = if strain > base_strain + 0.5 {
            "TORN APART"
        } else if pos < 0.25 {
            "CONVERGED"
        } else {
            "diverged"
        };
        println!(
            "{name}: pos_err(tail)={pos:10.4} m  max={max:11.3} m  joint_strain={strain:8.4} m  => {verdict}"
        );
    }
    println!(
        "\nCHASSIS_ONLY must reproduce the live failure (wheels left behind => joints torn => launch).\n\
         FULL_ASSEMBLY must CONVERGE with joint_strain at the host baseline."
    );
}
