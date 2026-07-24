//! Headless avian determinism probe.
//!
//! The root question for client-side prediction: given identical inputs + initial
//! state, does avian produce the SAME result twice? If not, two peers (host + a
//! predicting client) running the same commands DIVERGE — so reconcile must keep
//! correcting the client, and those corrections fight the live drive = the
//! post-turn wobble (see `project_predict_own_oscillation_cadence`). The
//! net_smoke cadence test already ruled out input-cadence loss (~0.3%); this
//! isolates the remaining suspect — the physics solver itself.
//!
//! Method: settle a dense pile of dynamic boxes (many simultaneous contacts +
//! islands — the worst case for solver/contact ordering) under gravity with
//! DETERMINISTIC time stepping (`TimeUpdateStrategy::ManualDuration`), and report
//! the run-to-run final-position diff for a MULTI-threaded compute pool (like the
//! GUI's `DefaultPlugins`) versus a SINGLE thread. A non-zero multi-thread diff
//! that vanishes single-threaded ⇒ the `parallel` solver is the non-determinism.
//!
//!   cargo run --bin determinism_probe --release

use avian3d::prelude::*;
use bevy::app::{
    ScheduleRunnerPlugin, TaskPoolOptions, TaskPoolPlugin, TaskPoolThreadAssignmentPolicy,
};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use std::time::Duration;

/// Fixed ticks to settle the pile (5 s at 60 Hz).
const TICKS: usize = 300;
/// Number of dynamic boxes in the pile.
const N: usize = 24;

#[derive(Component)]
struct Prop(usize);

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

fn run(threads: usize) -> Vec<DVec3> {
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
    .add_systems(Startup, setup);
    app.finish();
    app.cleanup();
    for _ in 0..TICKS {
        app.update();
    }
    let mut q = app.world_mut().query::<(&Prop, &Position)>();
    let mut out = vec![DVec3::ZERO; N];
    for (p, pos) in q.iter(app.world()) {
        out[p.0] = pos.0;
    }
    out
}

fn setup(mut commands: Commands) {
    commands.spawn((
        Name::new("Ground"),
        RigidBody::Static,
        Collider::cuboid(100.0, 1.0, 100.0),
        Friction::new(0.8),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    // A jittered stack of unit cubes that collapses into a dense, interpenetrating
    // contact pile — the ordering-sensitive case a parallel solver can reorder.
    let mut i = 0;
    'outer: for gy in 0..4 {
        for gx in 0..3 {
            for gz in 0..2 {
                if i >= N {
                    break 'outer;
                }
                let x = gx as f32 * 0.8 - 0.8 + (i as f32) * 0.01;
                let z = gz as f32 * 0.8 - 0.4;
                let y = 1.0 + gy as f32 * 1.05;
                commands.spawn((
                    Prop(i),
                    RigidBody::Dynamic,
                    Collider::cuboid(1.0, 1.0, 1.0),
                    Mass(1.0),
                    Friction::new(0.8),
                    Transform::from_xyz(x, y, z),
                ));
                i += 1;
            }
        }
    }
}

fn total_diff(a: &[DVec3], b: &[DVec3]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (*x - *y).length()).sum()
}

fn main() {
    println!("=== avian determinism probe: {N} boxes, {TICKS} ticks, substeps=12 ===");
    for threads in [8usize, 1usize] {
        let r1 = run(threads);
        let r2 = run(threads);
        let d = total_diff(&r1, &r2);
        // Sanity: the pile must actually MOVE (fall + settle), else 0.0 is trivial.
        let motion: f64 = r1.iter().map(|p| p.length()).sum();
        let tag = if threads == 1 {
            "SINGLE-thread"
        } else {
            "MULTI-thread "
        };
        let verdict = if d < 1e-9 {
            "DETERMINISTIC"
        } else {
            "NON-deterministic"
        };
        println!(
            "{tag} (compute={threads:>2}): run-to-run pos diff = {d:.3e} m  => {verdict}  \
             (settled-pile motion metric = {motion:.2})"
        );
    }
}
