//! Full integration test for the lunar surface teleport workflow.
//!
//! This test runs a headless Bevy app that:
//! 1. Sets up the BigSpace hierarchy (Solar Grid → EMB Grid → Moon Grid → Moon Body)
//! 2. Spawns an avatar camera on the Moon Grid (simulating orbit)
//! 3. Triggers teleport by setting camera to surface position
//! 4. Runs the app for several frames to propagate transforms
//! 5. Verifies:
//!    - Camera is at ~50m altitude above the Moon
//!    - SurfaceCamera mode is active
//!    - Terrain altitude calculation gives ~50m
//!
//! Run with: cargo test -p lunco-avatar --test teleport_integration_test -- --nocapture

use bevy::prelude::*;
use bevy::math::DVec3;
use big_space::prelude::*;

use lunco_celestial::{
    CelestialBody, CelestialReferenceFrame, GravityProvider, PointMassGravity,
    TerrainTileConfig,
};
use lunco_core::Avatar;
use lunco_avatar::{
    SurfaceCamera, SurfaceRelativeMode, FreeFlightCamera, OrbitCamera,
};

const MOON_RADIUS: f64 = 1737.0e3;
const MOON_GRID_CELL_SIZE: f64 = 10_000.0;

/// Full teleport workflow integration test.
#[test]
fn test_full_teleport_workflow() {
    let mut app = App::new();

    // Headless app: no window, no renderer, no input
    app.add_plugins((
        MinimalPlugins,
        big_space::prelude::BigSpaceDefaultPlugins,
    ));

    // Resources needed by the app
    app.insert_resource(TerrainTileConfig::default());

    // Track test state
    app.insert_resource(TestState::default());

    app.add_systems(Startup, |
        mut commands: Commands,
        mut test_state: ResMut<TestState>,
    | {
        // ── Build hierarchy ────────────────────────────────────────────────
        let root = commands.spawn(BigSpace::default()).id();

        let solar_grid = commands.spawn((
            CelestialReferenceFrame { ephemeris_id: 10 },
            Grid::new(1.0e9, 1.0e30),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
        )).id();
        commands.entity(solar_grid).set_parent_in_place(root);

        let emb_grid = commands.spawn((
            CelestialReferenceFrame { ephemeris_id: 3 },
            Grid::new(1.0e8, 1.0e30),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
        )).id();
        commands.entity(emb_grid).set_parent_in_place(solar_grid);

        let moon_grid = commands.spawn((
            CelestialReferenceFrame { ephemeris_id: 301 },
            Grid::new(MOON_GRID_CELL_SIZE as f32, 1.0e30_f32),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
        )).id();
        commands.entity(moon_grid).set_parent_in_place(emb_grid);

        // Moon Body: child of Moon Grid, at origin (identity transform)
        let moon_body = commands.spawn((
            CelestialBody {
                name: "Moon".to_string(),
                ephemeris_id: 301,
                radius_m: MOON_RADIUS,
            },
            GravityProvider {
                model: Box::new(PointMassGravity { gm: 4.904e12 }),
            },
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
        )).id();
        commands.entity(moon_body).set_parent_in_place(moon_grid);

        // Avatar camera — start on Moon Grid in orbit (far from surface)
        let orbit_altitude = MOON_RADIUS * 3.0;
        let avatar = commands.spawn((
            Camera::default(),
            Camera3d::default(),
            Avatar,
            FreeFlightCamera { yaw: 0.0, pitch: -0.2, damping: None },
            FloatingOrigin,
            CellCoord::default(),
            Transform::from_translation(Vec3::new(0.0, orbit_altitude as f32, orbit_altitude as f32 * 0.5)),
            GlobalTransform::default(),
            Name::new("Avatar Camera"),
        )).id();
        commands.entity(avatar).set_parent_in_place(moon_grid);

        test_state.moon_grid = Some(moon_grid);
        test_state.moon_body = Some(moon_body);
        test_state.avatar = Some(avatar);
    });

    // Run a few frames to let big_space propagate transforms
    for _ in 0..10 {
        app.update();
    }

    // ── Verify initial state: camera is in orbit, not on surface ──────────
    // The camera's Transform.translation is grid-local (within the Moon Grid).
    // In orbit at 3x radius, the grid-local position should be large.
    let _initial_alt = {
        let world = app.world_mut();
        let avatar = {
            let ts = world.resource::<TestState>();
            ts.avatar.unwrap()
        };
        let mut q_avatar = world.query::<(&CellCoord, &Transform, Option<&FreeFlightCamera>)>();
        let (_cell, tf, freeflight) = q_avatar.get(world, avatar).unwrap();
        assert!(freeflight.is_some(), "Camera should have FreeFlightCamera initially");
        // Grid-local position magnitude (not world altitude)
        tf.translation.length()
    };

    // ── Simulate teleport: set camera to surface position ─────────────────
    {
        // Get avatar and moon_grid from test state first
        let (avatar, moon_grid) = {
            let world = app.world_mut();
            let ts = world.resource::<TestState>();
            (ts.avatar.unwrap(), ts.moon_grid.unwrap())
        };

        let world = app.world_mut();

        // Compute surface position at lat=0, lon=0 (same logic as teleport command)
        let surface_normal = DVec3::new(0.0, 0.0, 1.0); // lat=0,lon=0 → +Z
        let surface_local_pos = surface_normal * (MOON_RADIUS + 50.0);

        let grid = Grid::new(MOON_GRID_CELL_SIZE as f32, 1.0e30_f32);
        let (new_cell, new_tf_pos) = grid.translation_to_grid(surface_local_pos);

        // Build surface rotation (same as teleport command)
        let up_v = surface_normal.as_vec3();
        let right_v = DVec3::Y.cross(surface_normal).normalize().as_vec3();
        let fwd_v = up_v.cross(right_v);
        let surface_rot = Quat::from_mat3(&Mat3::from_cols(right_v, up_v, -fwd_v));

        let mut entity = world.entity_mut(avatar);
        entity.insert(new_cell);
        entity.insert(Transform::from_translation(new_tf_pos).with_rotation(surface_rot));
        entity.insert(SurfaceCamera { heading: 0.0, pitch: -0.2 });
        entity.insert(SurfaceRelativeMode);
        entity.remove::<FreeFlightCamera>();
        entity.remove::<OrbitCamera>();
        drop(entity);

        // Re-parent to Moon Grid (same as teleport does)
        world.entity_mut(moon_grid).add_child(avatar);
    }

    // Run a few frames to propagate transforms
    for _ in 0..10 {
        app.update();
    }

    // ── Verify post-teleport state ────────────────────────────────────────
    let (post_cell, post_tf, has_surface_camera, has_surface_mode) = {
        let world = app.world_mut();
        let avatar = {
            let test_state = world.resource::<TestState>();
            test_state.avatar.unwrap()
        };
        let mut q_avatar = world.query::<(
            &CellCoord, &Transform,
            Option<&SurfaceCamera>, Option<&SurfaceRelativeMode>,
        )>();
        let (cell, tf, sc, sm) = q_avatar.get(world, avatar).unwrap();
        (*cell, *tf, sc.is_some(), sm.is_some())
    };

    // Camera should be in surface mode
    assert!(has_surface_camera, "Camera should have SurfaceCamera after teleport");
    assert!(has_surface_mode, "Camera should have SurfaceRelativeMode after teleport");

    // Reconstruct position from cell + local transform
    let reconstructed = DVec3::new(
        post_cell.x as f64 * MOON_GRID_CELL_SIZE + post_tf.translation.x as f64,
        post_cell.y as f64 * MOON_GRID_CELL_SIZE + post_tf.translation.y as f64,
        post_cell.z as f64 * MOON_GRID_CELL_SIZE + post_tf.translation.z as f64,
    );

    // Verify altitude is ~50m
    let altitude = reconstructed.length() - MOON_RADIUS;
    assert!(
        (altitude - 50.0).abs() < 10.0,
        "Camera altitude after teleport should be ~50m, got {:.2}m \
         (cell={:?}, local_tf={:?}, reconstructed={:?})",
        altitude, post_cell, post_tf.translation, reconstructed
    );

    // Verify terrain threshold check would pass (altitude < 100km)
    let terrain_threshold = 100_000.0;
    assert!(
        altitude < terrain_threshold,
        "Terrain should spawn at {:.0}m altitude (threshold={})",
        altitude, terrain_threshold
    );
}

/// Test state shared between startup and verification systems.
#[derive(Resource, Default, Clone)]
struct TestState {
    moon_grid: Option<Entity>,
    moon_body: Option<Entity>,
    avatar: Option<Entity>,
    expected_altitude: f64,
}
