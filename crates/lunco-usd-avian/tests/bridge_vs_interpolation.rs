//! The bridge READ pass and Avian's render interpolation must share one pose
//! contract. Production enables `PhysicsInterpolationPlugin::interpolate_all()`:
//! the fixed bridge writes the solved pose, Avian eases `Transform` after the
//! fixed loop, and Avian restores the solved endpoint in `FixedFirst` before the
//! next bridge READ. The render sample must therefore never become physics
//! truth.
//!
//! A body under constant velocity must travel `v * t`, and its solved
//! `Position` must never be dragged backwards by the render easing.

use avian3d::math::Vector;
use avian3d::physics_transform::Position;
use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use big_space::prelude::{BigSpace, CellCoord, FloatingOrigin, Grid};
use core::time::Duration;
use lunco_core::ActivePhysicsFrame;
use lunco_usd_avian::BigSpacePhysicsBridgePlugin;

const EDGE: f32 = 2000.0;

// This regression preserves the high-resolution numerical reproduction that
// originally exposed the bridge/interpolation ordering bug. It is deliberately
// separate from the production solver-resolution contract.
const INTERPOLATION_REPRODUCTION_SUBSTEPS: u32 = 64;

fn shell(app: &mut App) -> Entity {
    let root = app
        .world_mut()
        .spawn((
            BigSpace::default(),
            Grid::new(EDGE, 100.0),
            GlobalTransform::default(),
        ))
        .id();
    app.world_mut().insert_resource(ActivePhysicsFrame(root));
    let grid = app
        .world_mut()
        .spawn((
            Grid::new(EDGE, 100.0),
            CellCoord::ZERO,
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(root),
        ))
        .id();
    app.world_mut().spawn((
        CellCoord::ZERO,
        Transform::default(),
        GlobalTransform::default(),
        FloatingOrigin,
        ChildOf(grid),
    ));
    grid
}

/// Production shape: `interpolate_all()`, and a RENDER frame that is not a
/// whole multiple of the fixed timestep, so `overstep_fraction` is a varying
/// non-zero value — exactly the case where the eased pose differs from the
/// solved one.
fn make_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()));
    app.init_asset::<Mesh>();
    app.add_plugins((
        big_space::plugin::BigSpaceMinimalPlugins,
        PhysicsPlugins::default().set(PhysicsInterpolationPlugin::interpolate_all()),
        BigSpacePhysicsBridgePlugin,
    ));
    // 10 ms render frame vs the 15.625 ms fixed tick.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
        10_000,
    )));
    app.insert_resource(Gravity(Vector::ZERO));
    app.insert_resource(SubstepCount(INTERPOLATION_REPRODUCTION_SUBSTEPS));
    app.finish();
    app.cleanup();
    app
}

/// The eased `Transform` differs from `Position` between ticks, but
/// `bevy_transform_interpolation::complete_translation_easing` runs in
/// `FixedFirst` and restores `Transform` to the solved endpoint before the
/// bridge's READ pass. This test pins that production arrangement: if the
/// restore is removed, reordered, or the bridge READ moves ahead of it, the
/// solved pose starts tracking the render pose and this fails.
#[test]
fn interpolation_does_not_drag_the_solved_position() {
    let mut app = make_app();
    let grid = shell(&mut app);

    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            LinearVelocity(Vector::new(4.0, 0.0, 0.0)),
            CellCoord::ZERO,
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(grid),
        ))
        .id();

    // The symptom is not a net distance error — a constant lag would be
    // invisible. It is that the lag VARIES with `overstep_fraction`, so the
    // solved pose advances by a different amount every tick. Sample the solved
    // `Position` once per frame and look at the per-frame advance.
    let mut samples = Vec::new();
    for _ in 0..100 {
        app.update();
        samples.push(app.world().get::<Position>(body).expect("Position").x);
    }

    // Per-frame advances, ignoring frames where no fixed tick ran (10 ms render
    // vs 15.625 ms tick means some frames step nothing).
    let steps: Vec<f64> = samples
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|d| d.abs() > 1e-9)
        .collect();
    let tick = 4.0 / 64.0; // v * fixed dt

    let worst = steps
        .iter()
        .map(|d| (d - tick).abs())
        .fold(0.0f64, f64::max);
    let backwards = steps.iter().filter(|d| **d < 0.0).count();

    assert!(
        backwards == 0 && worst < 1e-3,
        "solved Position advance is not uniform — the render easing is being read \
         back as physics truth. {backwards} backward steps; worst deviation from \
         {tick:.4} m/tick = {worst:.4} m; steps = {steps:?}"
    );
}
