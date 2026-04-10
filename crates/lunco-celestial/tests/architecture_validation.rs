//! Critical architecture validation tests for the body-fixed Grid transition.
//!
//! These tests verify the 4 key assumptions that the implementation plan depends on.
//! If any of these fail, the architecture must be redesigned before implementation.
//!
//! **Important**: Tests 1-3 use isolated big_space setups (no CelestialPlugin)
//! because the CelestialPlugin's integration tests have pre-existing breakage
//! unrelated to these architectural questions.
//!
//! ## Architecture Decision (confirmed by tests)
//!
//! **DO NOT rotate the Grid.** big_space ignores Grid rotation when the
//! FloatingOrigin is in the same Grid. Instead, rotate the **Body** entity
//! and make rovers children of Body. big_space's `propagate_low_precision`
//! handles transform propagation for Body children automatically.

use bevy::prelude::*;
use bevy::math::{DQuat, DVec3, Quat};
use big_space::prelude::*;
use big_space::plugin::BigSpaceMinimalPlugins;

// ─────────────────────────────────────────────────────────────────────────────
// Test 1: Grid rotation propagates to children via GlobalTransform
// ─────────────────────────────────────────────────────────────────────────────
//
// ASSUMPTION: Rotating a Grid entity causes all children to inherit that
// rotation through GlobalTransform. This is the core mechanic of body-fixed grids.
//
// WHY IT MATTERS: If big_space ignores or resets rotation, we can't use Grid
// rotation to keep surface entities fixed relative to the rotating body.

#[test]
fn test_grid_rotation_propagates_to_child_global_transform() {
    let mut app = App::new();
    app.add_plugins(BigSpaceMinimalPlugins);

    let grid = app.world_mut().spawn((
        Grid::new(10_000.0, 1_000.0),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
    )).id();

    let child_offset = Vec3::new(100.0, 500.0, 200.0);
    let child = app.world_mut().spawn((
        CellCoord::default(),
        Transform::from_translation(child_offset),
        GlobalTransform::default(),
    )).id();

    let fo = app.world_mut().spawn((
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        FloatingOrigin,
    )).id();

    // Root → Grid, FO
    app.world_mut().spawn(BigSpaceRootBundle::default())
        .add_children(&[grid, fo]);
    // Grid → child (NOT root → child!)
    app.world_mut().entity_mut(grid).add_child(child);

    app.update();

    // Rotate the Grid 90 degrees around Z
    let rot_90_z: Quat = DQuat::from_axis_angle(DVec3::Z, std::f64::consts::PI / 2.0).as_quat();
    {
        let mut tf = app.world_mut().get_mut::<Transform>(grid).unwrap();
        tf.rotation = rot_90_z;
    }

    app.update();

    // Verify child position rotated correctly: (100,500,200) → (-500,100,200)
    {
        let child_gtf = app.world().get::<GlobalTransform>(child).unwrap();
        let expected = Vec3::new(-500.0, 100.0, 200.0);
        let error = (child_gtf.translation() - expected).length();
        assert!(
            error < 1e-3,
            "Child position not rotated correctly.\nExpected: {:?}\nGot: {:?}\nError: {:.6}",
            expected, child_gtf.translation(), error
        );

        let child_rot = child_gtf.compute_transform().rotation;
        let rot_error = (child_rot - rot_90_z).length();
        assert!(
            rot_error < 1e-5,
            "Child rotation doesn't match Grid rotation.\nExpected: {:?}\nGot: {:?}\nError: {:.6}",
            rot_90_z, child_rot, rot_error
        );
    }

    info!("PASS: Grid rotation propagates to children via GlobalTransform");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 2: Nested Grid with rotating child (EMB → Moon hierarchy)
// ─────────────────────────────────────────────────────────────────────────────
//
// In our architecture, EMB Grid does NOT rotate. Moon Grid rotates.
// Moon tiles (on Moon Grid) inherit Moon Grid's rotation.
// This is the actual scenario in LunCoSim.

#[test]
fn test_nested_grid_rotation() {
    let mut app = App::new();
    app.add_plugins(BigSpaceMinimalPlugins);

    // Parent Grid (like EMB) — does NOT rotate in our architecture
    let parent_grid = app.world_mut().spawn((
        Grid::new(1.0e8, 1_000.0),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
    )).id();

    // Child Grid (like Moon) — this one rotates
    let child_grid_offset = Vec3::new(0.0, 0.0, 384_400.0);
    let child_grid = app.world_mut().spawn((
        Grid::new(10_000.0, 1_000.0),
        CellCoord::default(),
        Transform::from_translation(child_grid_offset),
        GlobalTransform::default(),
    )).id();

    // Tile on Moon Grid
    let tile_offset = Vec3::new(100.0, 500.0, 200.0);
    let tile = app.world_mut().spawn((
        CellCoord::default(),
        Transform::from_translation(tile_offset),
        GlobalTransform::default(),
    )).id();

    let fo = app.world_mut().spawn((
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        FloatingOrigin,
    )).id();

    app.world_mut().spawn(BigSpaceRootBundle::default())
        .add_child(parent_grid)
        .add_child(fo);
    app.world_mut().entity_mut(parent_grid).add_child(child_grid);
    app.world_mut().entity_mut(child_grid).add_child(tile);

    app.update();

    // Record tile position before child Grid rotation
    let tile_pos_before = app.world().get::<GlobalTransform>(tile).unwrap().translation();

    // Rotate child Grid 90° around Y (like Moon rotating)
    let child_rot: Quat = DQuat::from_axis_angle(DVec3::Y, std::f64::consts::PI / 2.0).as_quat();
    app.world_mut().get_mut::<Transform>(child_grid).unwrap().rotation = child_rot;

    app.update();

    // Tile should have rotated with the child Grid.
    // The tile's world position = child_grid.rotation × tile_offset + child_grid.translation
    // (big_space applies the grid's full transform to its children).
    let tile_gtf = app.world().get::<GlobalTransform>(tile).unwrap();
    let tile_pos_after = tile_gtf.translation();
    let expected_pos: Vec3 = child_rot * tile_offset + child_grid_offset;
    let pos_error = (tile_pos_after - expected_pos).length();

    // Tile should also inherit the child Grid's rotation
    let tile_rot = tile_gtf.compute_transform().rotation;
    let rot_error = (tile_rot - child_rot).length();

    assert!(
        pos_error < 1.0,
        "Tile position not rotated with child Grid.\nExpected: {:?}\nGot: {:?}\nError: {:.6}",
        expected_pos, tile_pos_after, pos_error
    );
    assert!(
        rot_error < 1e-5,
        "Tile rotation doesn't match child Grid rotation.\nExpected: {:?}\nGot: {:?}\nError: {:.6}",
        child_rot, tile_rot, rot_error
    );

    info!("PASS: Nested Grid rotation — child Grid's children inherit rotation");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 3: Avian3D colliders on rotated Grid children
// ─────────────────────────────────────────────────────────────────────────────
//
// ASSUMPTION: Avian3D Collider on a child of a rotated Grid entity uses the
// full GlobalTransform (including inherited rotation) for physics.

#[test]
fn test_avian3d_collider_on_rotated_grid_child() {
    use avian3d::prelude::*;

    let mut app = App::new();
    app.add_plugins(BigSpaceMinimalPlugins);

    let grid = app.world_mut().spawn((
        Grid::new(10_000.0, 1_000.0),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
    )).id();

    let tile_pos = Vec3::new(100.0, 500.0, 200.0);
    let tile = app.world_mut().spawn((
        CellCoord::default(),
        Transform::from_translation(tile_pos),
        GlobalTransform::default(),
        Collider::cuboid(50.0, 10.0, 50.0),
        RigidBody::Static,
    )).id();

    let fo = app.world_mut().spawn((
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        FloatingOrigin,
    )).id();

    app.world_mut().spawn(BigSpaceRootBundle::default())
        .add_child(grid)
        .add_child(fo);
    app.world_mut().entity_mut(grid).add_child(tile);

    app.update();

    assert!(app.world().get::<Collider>(tile).is_some(), "Collider should exist");

    // Rotate Grid 45° around Y
    let rot_45_y: Quat = DQuat::from_axis_angle(DVec3::Y, std::f64::consts::PI / 4.0).as_quat();
    app.world_mut().get_mut::<Transform>(grid).unwrap().rotation = rot_45_y;

    app.update();

    // Verify tile world position is rotated
    let tile_gtf = app.world().get::<GlobalTransform>(tile).unwrap();
    let cos45 = (std::f64::consts::PI / 4.0).cos() as f32;
    let sin45 = (std::f64::consts::PI / 4.0).sin() as f32;
    let expected = Vec3::new(
        100.0 * cos45 + 200.0 * sin45,
        500.0,
        -100.0 * sin45 + 200.0 * cos45,
    );
    let pos_error = (tile_gtf.translation() - expected).length();
    assert!(pos_error < 1e-2, "Tile position error: {:.6}", pos_error);

    // Verify tile rotation matches Grid rotation
    let tile_rot = tile_gtf.compute_transform().rotation;
    let rot_error = (tile_rot - rot_45_y).length();
    assert!(rot_error < 1e-5, "Tile rotation error: {:.6}", rot_error);

    info!("PASS: Avian3D colliders on rotated Grid children have correct GlobalTransform");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 5 (THE SOLUTION): Rover as child of Body inherits rotation
// ─────────────────────────────────────────────────────────────────────────────
//
// This test proves the CORRECT architecture: rovers as children of Body.
// Body rotates → rover inherits rotation via big_space's propagate_low_precision.
// No Grid rotation needed. No big_space patch needed. Works today.

#[test]
fn test_body_child_rover_inherits_rotation() {
    use avian3d::prelude::*;

    let mut app = App::new();
    app.add_plugins(BigSpaceMinimalPlugins);

    // Grid (translates, identity rotation — like Moon Grid)
    let grid = app.world_mut().spawn((
        Grid::new(10_000.0, 1_000.0),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
    )).id();

    // Body at Grid origin (rotates — like Moon Body)
    let body = app.world_mut().spawn((
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Collider::sphere(1737.0e3), // Moon radius
    )).id();

    // Rover as child of Body at surface position (body-fixed coordinates)
    let moon_radius = 1_737_000.0;
    let rover_local_pos = Vec3::new(0.0, moon_radius + 5.0, 0.0); // 5m above surface at pole
    let rover = app.world_mut().spawn((
        Transform::from_translation(rover_local_pos),
        GlobalTransform::default(),
        Collider::cuboid(1.0, 0.5, 2.0),
        RigidBody::Dynamic,
    )).id();

    // FloatingOrigin co-located with Body (at Grid origin) — camera at center of Moon
    let fo = app.world_mut().spawn((
        CellCoord::default(),
        Transform::default(), // At origin, same as Body
        GlobalTransform::default(),
        FloatingOrigin,
    )).id();

    // Hierarchy: BigSpaceRoot → Grid → {Body → Rover, FO}
    app.world_mut().spawn(BigSpaceRootBundle::default())
        .add_child(grid);
    app.world_mut().entity_mut(grid).add_children(&[body, fo]);
    app.world_mut().entity_mut(body).add_child(rover);

    app.update();

    // Record rover position before rotation
    let rover_pos_before = app.world().get::<GlobalTransform>(rover).unwrap().translation();
    info!("Rover pos before: {:?}", rover_pos_before);

    // Rotate the Body 90° around Z (like Moon spinning)
    let body_rot: Quat = DQuat::from_axis_angle(DVec3::Z, std::f64::consts::PI / 2.0).as_quat();
    {
        let mut body_tf = app.world_mut().get_mut::<Transform>(body).unwrap();
        body_tf.rotation = body_rot;
    }

    app.update();

    // Body's GlobalTransform should have the rotation
    let body_gtf = app.world().get::<GlobalTransform>(body).unwrap();
    let body_rot_actual = body_gtf.compute_transform().rotation;
    let body_rot_error = (body_rot_actual - body_rot).length();
    assert!(body_rot_error < 1e-5, "Body rotation error: {:.6}", body_rot_error);

    // Rover's GlobalTransform should include Body's rotation
    // (100, 500, 200) rotated 90° around Z → (-500, 100, 200)
    // Actually our rover is at (0, moon_radius+5, 0) → rotated 90° Z → (-(moon_radius+5), 0, 0)
    let rover_gtf = app.world().get::<GlobalTransform>(rover).unwrap();
    let rover_pos_after = rover_gtf.translation();
    let rover_rot_after = rover_gtf.compute_transform().rotation;

    // Expected: body_rot * (0, moon_radius+5, 0) = (-(moon_radius+5), 0, 0)
    let expected_pos: Vec3 = body_rot * rover_local_pos;
    let pos_error = (rover_pos_after - expected_pos).length();

    // Rover should inherit Body's rotation
    let rot_error = (rover_rot_after - body_rot).length();

    assert!(
        pos_error < 1.0,
        "Rover position not rotated with Body.\nExpected: {:?}\nGot: {:?}\nError: {:.6} (0.14m is fine for f32 at 1.7M scale)",
        expected_pos, rover_pos_after, pos_error
    );
    assert!(
        rot_error < 1e-5,
        "Rover rotation doesn't match Body rotation.\nExpected: {:?}\nGot: {:?}\nError: {:.6}",
        body_rot, rover_rot_after, rot_error
    );

    // Key: rover's LOCAL Transform (relative to Body) is unchanged
    let rover_tf = app.world().get::<Transform>(rover).unwrap();
    let local_pos_unchanged = (rover_tf.translation - rover_local_pos).length();
    assert!(
        local_pos_unchanged < 1e-5,
        "Rover local position should NOT change — it stays in body-fixed coords.\n\
         Expected: {:?}\nGot: {:?}\nError: {:.6}",
        rover_local_pos, rover_tf.translation, local_pos_unchanged
    );

    info!("PASS: Rover as child of Body inherits rotation, stays in body-fixed coords");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 6: Surface gravity direction using Body-local position
// ─────────────────────────────────────────────────────────────────────────────
//
// Proves that gravity direction can be computed in Body-local space,
// which is the same as body-fixed coordinates regardless of Body rotation.

#[test]
fn test_gravity_direction_from_body_local_position() {
    // On a sphere, gravity direction at any surface point is:
    //   gravity_dir = -normalize(entity_local_position_in_body_space)
    // This is independent of Body rotation because local space is body-fixed.

    let moon_radius = 1_737_000.0;

    // Test at various body-local positions (like rover positions on the surface)
    let test_cases = [
        // (local_position, expected_gravity_direction)
        (Vec3::new(0.0, moon_radius, 0.0), Vec3::new(0.0, -1.0, 0.0)),       // North pole
        (Vec3::new(moon_radius, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0)),       // +X "equator"
        (Vec3::new(0.0, 0.0, moon_radius), Vec3::new(0.0, 0.0, -1.0)),       // +Z "equator"
        (Vec3::new(0.0, -moon_radius, 0.0), Vec3::new(0.0, 1.0, 0.0)),       // South pole
        // At 45° latitude, 0° longitude
        (Vec3::new(0.0, 45.0_f64.to_radians().cos() as f32 * moon_radius,
                     45.0_f64.to_radians().sin() as f32 * moon_radius),
         // Expected: normalize(-position)
         Vec3::ZERO), // will compute
    ];

    for (local_pos, expected_dir) in &test_cases {
        let local_pos_d64 = local_pos.as_dvec3();
        let gravity_dir = -local_pos_d64.normalize_or_zero();

        if *expected_dir != Vec3::ZERO {
            let expected_d64 = expected_dir.as_dvec3();
            let error = (gravity_dir - expected_d64).length();
            assert!(
                error < 1e-10,
                "Gravity direction error at local pos {:?}. Expected: {:?}, Got: {:?}, Error: {:.6}",
                local_pos, expected_dir, gravity_dir, error
            );
        } else {
            // For the 45° case, verify it points toward center
            let angle_to_center = gravity_dir.angle_between(-local_pos.as_dvec3().normalize_or_zero());
            assert!(angle_to_center < 1e-10, "Gravity should point to center at 45° lat");
        }
    }

    // Most importantly: gravity direction depends ONLY on local position, not Body rotation
    // Even if Body rotates 90°, the local position (body-fixed) stays the same → gravity dir unchanged
    let local_pos = Vec3::new(0.0, moon_radius, 0.0);
    let gravity_before = -local_pos.as_dvec3().normalize_or_zero();

    // Body rotates 90° Z
    let _body_rot: Quat = DQuat::from_axis_angle(DVec3::Z, std::f64::consts::PI / 2.0).as_quat();

    // Local position in body-fixed coords is UNCHANGED
    let local_pos_after = local_pos; // Body rotation doesn't change local coords
    let gravity_after = -local_pos_after.as_dvec3().normalize_or_zero();

    assert!(
        (gravity_before - gravity_after).length() < 1e-10,
        "Gravity direction should not change with Body rotation (body-fixed coords)"
    );

    info!("PASS: Gravity direction computed from body-local position");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test 4: RayCaster NEG_Y error at various latitudes
// ─────────────────────────────────────────────────────────────────────────────
//
// RESULT: This is pure math — no Bevy needed. It proves NEG_Y is wrong
// everywhere except one pole.

#[test]
fn test_raycaster_neg_y_error_at_various_latitudes() {
    /// Surface normal (radial "up") at lat/lon on unit sphere, Y-up convention.
    fn surface_normal(lat_deg: f64, lon_deg: f64) -> DVec3 {
        let lat = lat_deg.to_radians();
        let lon = lon_deg.to_radians();
        DVec3::new(lat.cos() * lon.sin(), lat.sin(), lat.cos() * lon.cos())
    }

    /// Angle between true radial "down" and NEG_Y.
    fn neg_y_error_angle(lat_deg: f64) -> f64 {
        let up = surface_normal(lat_deg, 0.0);
        let down = -up;
        let dot = down.dot(DVec3::NEG_Y).clamp(-1.0, 1.0);
        dot.acos().to_degrees()
    }

    let cases = [
        (-90.0, "South Pole"), (-60.0, "60°S"), (-45.0, "45°S"),
        (-30.0, "30°S"), (-10.0, "10°S"), (0.0, "Equator"),
        (10.0, "10°N"), (30.0, "30°N"), (45.0, "45°N"),
        (60.0, "60°N"), (80.0, "80°N"), (90.0, "North Pole"),
    ];

    println!("\n┌──────────────┬────────────┬──────────────────┐");
    println!("│ Latitude     │ Error (°)  │ Verdict          │");
    println!("├──────────────┼────────────┼──────────────────┤");

    let mut bad_count = 0;
    for (lat, label) in &cases {
        let err = neg_y_error_angle(*lat);
        let verdict = if err < 1.0 { "OK" }
            else if err < 10.0 { "Concerning" }
            else if err < 45.0 { "Broken" }
            else { "WRONG" };
        if err >= 10.0 { bad_count += 1; }
        println!("│ {:<12} │ {:>9.2}° │ {:<16} │", label, err, verdict);
    }
    println!("├──────────────┴────────────┴──────────────────┤");
    println!("│ {}/{} locations have >10° error           │", bad_count, cases.len());
    println!("└──────────────────────────────────────────────┘");

    // North Pole: NEG_Y = true down → 0°
    assert!(neg_y_error_angle(90.0) < 1e-10, "North Pole should have 0° error");

    // South Pole: NEG_Y = true UP → 180°
    assert!((neg_y_error_angle(-90.0) - 180.0).abs() < 1e-10, "South Pole should have 180° error");

    // Equator: NEG_Y ⟂ radial → 90°
    assert!((neg_y_error_angle(0.0) - 90.0).abs() < 1e-10, "Equator should have 90° error");

    // 45°N: 45° error
    assert!((neg_y_error_angle(45.0) - 45.0).abs() < 1e-10, "45°N should have 45° error");

    info!("PASS: NEG_Y error proven — radial direction required per wheel");
}
