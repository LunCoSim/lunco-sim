//! Headless rollback-reconciliation probe.
//!
//! `determinism_probe` proved avian is deterministic run-to-run. This probe proves
//! the NEXT link in the chain — that deterministic replay actually *reconciles* a
//! diverged client. It compares the two correction strategies on ONE scenario, on
//! the real avian solver:
//!
//!   * BLEND    — the current `reconcile_owned_prediction`: on each ack, push the
//!                acked-seq error into the present pose and half-blend velocity to
//!                authority. A proportional controller — the suspected oscillator.
//!   * ROLLBACK — snap the body to the authoritative state at the acked tick, then
//!                deterministically REPLAY the recorded inputs ack+1..now by stepping
//!                physics. Converges exactly *iff* avian reconverges from a public-
//!                state-only restore (Position/Rotation/Lin/AngVel — all a network
//!                snapshot carries; NO solver warm-start / contact-cache state).
//!
//! The load-bearing risk this isolates: if avian's warm-start impulse caches or
//! contact manifolds make a public-state restore NOT reproduce the reference, the
//! ROLLBACK error will NOT collapse to ~0 — and the live rollback would then need
//! explicit cache invalidation. If it DOES collapse, the live port is safe.
//!
//! Scenario: a friction box on the ground driven like a rover — a body-frame
//! forward force (throttle) + a yaw torque (a steer SWEEP, the changing input that
//! makes BLEND oscillate). The client starts with a SEED ERROR (a stale-seed offset
//! + a small initial-velocity error), receives delayed acks, and corrects.
//!
//!   cargo run --bin rollback_probe --release

use avian3d::prelude::*;
use bevy::app::{
    ScheduleRunnerPlugin, TaskPoolOptions, TaskPoolPlugin, TaskPoolThreadAssignmentPolicy,
};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use std::time::Duration;

/// Ticks per run (10 s at 60 Hz — long enough for several ack cycles + a full sweep).
const TICKS: usize = 600;
/// Snapshot latency: the client's ack for tick T arrives at tick T + ACK_DELAY,
/// i.e. it must predict + reconcile ACK_DELAY unacked ticks (≈ 133 ms at 60 Hz).
const ACK_DELAY: usize = 8;
/// Ticks between snapshot arrivals — the host replicates at 20 Hz while the sim runs
/// at 60 Hz, so an ack (and therefore a correction) lands every 3rd tick. Correcting
/// EVERY tick instead would over-apply the same error ~3× and is not what the live
/// `reconcile_owned_prediction` does (it early-returns unless `ack > last_reconciled`).
const SNAPSHOT_INTERVAL: usize = 3;
/// Forward drive force (body frame, N) from full throttle.
const DRIVE_FORCE: f64 = 60.0;
/// Yaw torque (N·m) from full steer.
const STEER_TORQUE: f64 = 25.0;

/// The per-tick actuation the client also records + replays. `throttle`/`steer` in
/// [-1, 1]; the reference host generates them, the client knows its own copy.
#[derive(Resource, Clone, Copy, Default)]
struct DriveInput {
    throttle: f64,
    steer: f64,
}

/// The public, network-transmissible state of the driven body — exactly what a
/// snapshot carries and all a rollback restore is allowed to touch.
#[derive(Clone, Copy)]
struct BodyState {
    pos: DVec3,
    rot: DQuat,
    lv: DVec3,
    av: DVec3,
}

#[derive(Component)]
struct Vehicle;

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

/// Body-frame drive: forward force along the body's local -Z (`ConstantLocalForce`,
/// so its world direction rotates WITH the body) + a yaw torque. Overwritten from
/// `DriveInput` every tick, so actuation is a pure function of (input, current
/// pose) — the state-dependence that makes a rover's trajectory diverge under a
/// heading/position error, exactly what reconciliation has to undo.
fn drive_body(
    input: Res<DriveInput>,
    mut q: Query<(&mut ConstantLocalForce, &mut ConstantTorque), With<Vehicle>>,
) {
    for (mut force, mut torque) in q.iter_mut() {
        force.0 = DVec3::new(0.0, 0.0, -1.0) * (DRIVE_FORCE * input.throttle);
        torque.0 = DVec3::new(0.0, STEER_TORQUE * input.steer, 0.0);
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
    // Actuation must run BEFORE physics writeback each tick; FixedUpdate is before
    // FixedPostUpdate (where avian steps), matching the live pipeline order.
    .add_systems(FixedUpdate, drive_body);
    app.finish();
    app.cleanup();
    app
}

fn setup(mut commands: Commands) {
    commands.spawn((
        Name::new("Ground"),
        RigidBody::Static,
        Collider::cuboid(400.0, 1.0, 400.0),
        Friction::new(0.9),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    commands.spawn((
        Vehicle,
        RigidBody::Dynamic,
        Collider::cuboid(1.2, 0.6, 2.0),
        Mass(1.0),
        Friction::new(0.9),
        ConstantLocalForce(DVec3::ZERO),
        ConstantTorque(DVec3::ZERO),
        Transform::from_xyz(0.0, 0.31, 0.0),
    ));
}

/// One deterministic tick = one fixed update = one physics step (ManualDuration +
/// Fixed@60 ⇒ exactly one step per `update`, the same primitive `determinism_probe`
/// relies on). Set the input first so `drive_body` actuates from it this tick.
fn step(app: &mut App, input: DriveInput) {
    *app.world_mut().resource_mut::<DriveInput>() = input;
    app.update();
}

fn read_state(app: &mut App) -> BodyState {
    let mut q = app
        .world_mut()
        .query_filtered::<(&Position, &Rotation, &LinearVelocity, &AngularVelocity), With<Vehicle>>(
        );
    let (p, r, lv, av) = q.single(app.world()).expect("one vehicle");
    BodyState {
        pos: p.0,
        rot: r.0,
        lv: lv.0,
        av: av.0,
    }
}

/// Restore ONLY the public snapshot state — the deliberate constraint: a real
/// client can seat nothing else (no solver warm-start / contact caches). If avian
/// needs those to reproduce the reference, ROLLBACK error won't collapse and we've
/// learned the live port must invalidate them.
fn restore_state(app: &mut App, s: BodyState) {
    let mut q = app.world_mut().query_filtered::<(
        &mut Position,
        &mut Rotation,
        &mut LinearVelocity,
        &mut AngularVelocity,
    ), With<Vehicle>>();
    let world = app.world_mut();
    let (mut p, mut r, mut lv, mut av) = q.single_mut(world).expect("one vehicle");
    p.0 = s.pos;
    r.0 = s.rot;
    lv.0 = s.lv;
    av.0 = s.av;
}

/// The steer-sweep + throttle stream both peers apply. Constant throttle + a slow
/// sinusoidal steer: the CHANGING input that exposes a proportional corrector's lag.
fn gen_input(t: usize) -> DriveInput {
    let phase = t as f64 / 45.0; // ~0.75 s period-ish sweep
    DriveInput {
        throttle: 1.0,
        steer: 0.8 * phase.sin(),
    }
}

/// Run the authoritative host: record per-tick input + resulting state.
fn run_host(threads: usize) -> (Vec<DriveInput>, Vec<BodyState>) {
    let mut app = make_app(threads);
    let mut inputs = Vec::with_capacity(TICKS);
    let mut states = Vec::with_capacity(TICKS);
    for t in 0..TICKS {
        let inp = gen_input(t);
        step(&mut app, inp);
        inputs.push(inp);
        states.push(read_state(&mut app));
    }
    (inputs, states)
}

/// Correction strategy under test.
#[derive(Clone, Copy, PartialEq)]
enum Strategy {
    None,
    Blend,
    Rollback,
}

/// Run the client with a seed error + delayed acks, correcting per `strat`. Returns
/// the per-tick position error vs the host at the SAME tick.
fn run_client(
    strat: Strategy,
    host_inputs: &[DriveInput],
    host_states: &[BodyState],
    threads: usize,
) -> Vec<f64> {
    let mut app = make_app(threads);
    // SEED ERROR: start offset + with a small velocity error, the stale-seed
    // condition a freshly-promoted predicted body suffers.
    let seed = BodyState {
        pos: host_states.get(0).map_or(DVec3::ZERO, |s| s.pos) + DVec3::new(0.6, 0.0, 0.4),
        rot: DQuat::from_rotation_y(0.08),
        lv: DVec3::new(0.3, 0.0, 0.0),
        av: DVec3::ZERO,
    };
    // Prime one tick, then seat the seed error so the body actually diverges.
    step(&mut app, host_inputs[0]);
    restore_state(&mut app, seed);

    // Client's own recorded prediction history: state AFTER applying input[t].
    let mut predicted: Vec<BodyState> = Vec::with_capacity(TICKS);
    let mut errors = vec![0.0f64; TICKS];

    for t in 0..TICKS {
        // Local prediction: apply our own input this tick (identical to host_inputs
        // because the client knows what it pressed).
        step(&mut app, host_inputs[t]);
        predicted.push(read_state(&mut app));

        // Reconcile ONLY when a snapshot actually arrives (20 Hz), mirroring the live
        // `ack > last_reconciled` gate — a fair baseline for BLEND, and a realistic
        // (longer) replay window for ROLLBACK.
        if strat != Strategy::None && t >= ACK_DELAY && t % SNAPSHOT_INTERVAL == 0 {
            let ack = t - ACK_DELAY;
            match strat {
                Strategy::None => {}
                Strategy::Blend => {
                    // Proportional: push acked-seq error into the present + half-
                    // blend velocity to authority (mirrors reconcile_owned_prediction).
                    let auth = host_states[ack];
                    let pred = predicted[ack];
                    let dpos = auth.pos - pred.pos;
                    let cur = read_state(&mut app);
                    restore_state(
                        &mut app,
                        BodyState {
                            pos: cur.pos + dpos,
                            rot: (auth.rot * pred.rot.inverse()).normalize() * cur.rot,
                            lv: (cur.lv + auth.lv) * 0.5,
                            av: (cur.av + auth.av) * 0.5,
                        },
                    );
                }
                Strategy::Rollback => {
                    // Snap to authority at the acked tick, then deterministically
                    // replay our recorded inputs ack+1..=t back to the present.
                    restore_state(&mut app, host_states[ack]);
                    for r in (ack + 1)..=t {
                        step(&mut app, host_inputs[r]);
                    }
                    // Overwrite the now-stale predicted history for the replayed
                    // window so the next ack compares against the corrected trajectory.
                    // (Cheap: just re-read the present; intermediate seqs are pruned
                    // in the live system — here only predicted[ack] matters, already past.)
                }
            }
        }
        errors[t] = (read_state(&mut app).pos - host_states[t].pos).length();
    }
    errors
}

fn summarize(errors: &[f64]) -> (f64, f64, f64) {
    // Report on the SETTLED tail (after acks have had time to act): mean, max, and
    // the tail-half mean (steady-state — where oscillation vs convergence shows).
    let tail = &errors[errors.len() / 2..];
    let mean = errors.iter().sum::<f64>() / errors.len() as f64;
    let max = errors.iter().cloned().fold(0.0, f64::max);
    let tail_mean = tail.iter().sum::<f64>() / tail.len() as f64;
    (mean, max, tail_mean)
}

fn main() {
    let threads = 8usize;
    println!("=== rollback reconciliation probe: {TICKS} ticks, ack delay {ACK_DELAY}, compute={threads} ===");
    let (inputs, host) = run_host(threads);
    let host_motion: f64 = host.last().map_or(0.0, |s| s.pos.length());
    println!("host drove {host_motion:.2} m from origin (scenario is non-trivial)\n");

    for (name, strat) in [
        ("NONE     (no reconcile) ", Strategy::None),
        ("BLEND    (current recon) ", Strategy::Blend),
        ("ROLLBACK (replay resim)  ", Strategy::Rollback),
    ] {
        let errs = run_client(strat, &inputs, &host, threads);
        let (mean, max, tail) = summarize(&errs);
        println!("{name}: mean_err={mean:.4} m  max_err={max:.4} m  steady_tail_err={tail:.5} m");
    }
    println!(
        "\nExpect: NONE stays diverged; BLEND reduces but leaves a steady/oscillating tail;\n\
         ROLLBACK tail ~0 (public-state restore reproduces the reference) => live port is safe."
    );
}
