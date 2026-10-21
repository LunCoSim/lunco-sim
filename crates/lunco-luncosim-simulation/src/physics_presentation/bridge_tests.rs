//! The bridge READ pass and Avian's render interpolation must share one pose
//! contract. Production enables `PhysicsInterpolationPlugin::interpolate_all()`:
//! the fixed bridge writes the solved pose, Avian eases `Transform` after the
//! fixed loop, and `PreUpdate` runs before `FixedFirst` restores the solved
//! endpoint. Admission therefore seeds only unseen bodies; reads of established
//! bodies happen at the fixed physics boundary. The render sample must never
//! become physics truth.
//!
//! A body under constant velocity must travel `v * t`, and its solved
//! `Position` must never be dragged backwards by the render easing.

use avian3d::physics_transform::Position;
use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use big_space::prelude::{BigSpace, CellCoord, FloatingOrigin, Grid};
use core::time::Duration;
use lunco_spatial::ActivePhysicsFrame;
use lunco_usd_avian_core::BigSpacePhysicsBridgePlugin;

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
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        bevy::diagnostic::DiagnosticsPlugin,
    ));
    app.init_asset::<Mesh>();
    app.insert_resource(lunco_time::MissionClock::anchored(2451545.0, 0))
        .init_resource::<lunco_physics::PhysicsHolds>()
        .init_resource::<lunco_core::RuntimeFaults>();
    app.add_plugins((
        big_space::plugin::BigSpaceMinimalPlugins,
        crate::luncosim_physics_plugins(),
        BigSpacePhysicsBridgePlugin,
    ));
    // 10 ms render frame vs the 15.625 ms fixed tick.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
        10_000,
    )));
    app.insert_resource(Gravity(Vec3::ZERO));
    app.insert_resource(SubstepCount(INTERPOLATION_REPRODUCTION_SUBSTEPS));
    app.finish();
    app.cleanup();
    app
}

/// The eased `Transform` differs from `Position` between ticks.
/// `bevy_transform_interpolation::complete_translation_easing` restores it in
/// `FixedFirst`, after the `PreUpdate` scene-admission pass. This test pins the
/// bridge's admission and fixed-step ownership: an established body's render
/// sample must not feed back into its solved position.
#[test]
fn interpolation_does_not_drag_the_solved_position() {
    let mut app = make_app();
    let grid = shell(&mut app);

    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            LinearVelocity(Vec3::new(4.0, 0.0, 0.0)),
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

#[test]
fn queued_fixed_cycles_do_not_extrapolate_the_render_pose() {
    let mut app = make_app();
    let grid = shell(&mut app);
    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            LinearVelocity(Vec3::X),
            Transform::default(),
            ChildOf(grid),
        ))
        .id();
    let mut rocks = Vec::new();
    for i in 0..1440 {
        rocks.push(
            app.world_mut()
                .spawn((
                    RigidBody::Static,
                    Transform::from_rotation(Quat::from_rotation_y(
                        i as f32 * std::f32::consts::TAU / 1440.0,
                    )),
                    ChildOf(grid),
                ))
                .id(),
        );
    }
    app.add_systems(
        RunFixedMainLoop,
        (|mut time: ResMut<Time<Fixed>>| {
            time.accumulate_overstep(Duration::from_secs(5));
        })
        .after(bevy::app::RunFixedMainLoopSystems::FixedMainLoop)
        .before(avian3d::interpolation::TransformEasingSystems::Ease),
    );
    app.update();
    app.update();
    let pending = app.world().resource::<Time<Fixed>>().overstep();
    assert!(pending >= Duration::from_secs(5));
    let solved = app.world().get::<Position>(body).unwrap().x;
    let rendered = app.world().get::<Transform>(body).unwrap().translation.x as f64;
    assert!(
        rendered <= solved + 1e-6,
        "rendered {rendered}, solved {solved}, pending {pending:?}"
    );
    for rock in rocks {
        let rotation = app.world().get::<Transform>(rock).unwrap().rotation;
        assert!(
            rotation.is_normalized(),
            "{rotation:?}, pending {pending:?}"
        );
    }
}
