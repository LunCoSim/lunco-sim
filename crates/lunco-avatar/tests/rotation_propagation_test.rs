//! Tests that verify big_space rotation propagation from Body to tile children.
//!
//! Key insight: terrain tiles have `CellCoord` and are parented to the Grid.
//! big_space's `propagate_high_precision` computes their GlobalTransform from
//! CellCoord + Transform.translation, **ignoring Transform.rotation**.
//!
//! The solution: `tile_rotation_sync_system` runs AFTER big_space propagation
//! and manually applies Body rotation to tile GlobalTransforms.
//!
//! Run with: cargo test -p lunco-avatar --test rotation_propagation_test -- --nocapture

use bevy::prelude::*;
use bevy::math::DVec3;
use big_space::prelude::*;

use lunco_celestial::{CelestialBody, CelestialReferenceFrame};

const MOON_RADIUS: f64 = 1737.0e3;
const MOON_GRID_CELL_SIZE: f64 = 10_000.0;

/// Verify that when a Body entity rotates, its tile children (which have
/// CellCoord) inherit that rotation via big_space's propagate_high_precision.
#[test]
fn test_body_rotation_propagates_to_tile_with_cellcoord() {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        big_space::prelude::BigSpaceDefaultPlugins,
    ));

    let moon_radius = MOON_RADIUS;
    let _moon_grid = Grid::new(MOON_GRID_CELL_SIZE as f32, 1.0e30_f32);

    // Compute tile position at equator on +X axis: (R, 0, 0) in body-local coords.
    // This tile WILL move when the Body rotates around Y axis.
    let tile_body_local = DVec3::new(moon_radius, 0.0, 0.0);
    let cel = MOON_GRID_CELL_SIZE as f64;
    let tile_cell = CellCoord {
        x: (tile_body_local.x / cel).floor() as i64,
        y: (tile_body_local.y / cel).floor() as i64,
        z: (tile_body_local.z / cel).floor() as i64,
    };
    let tile_local_tf = Vec3::new(
        (tile_body_local.x - tile_cell.x as f64 * cel) as f32,
        (tile_body_local.y - tile_cell.y as f64 * cel) as f32,
        (tile_body_local.z - tile_cell.z as f64 * cel) as f32,
    );

    println!("PRE-SPAWN DEBUG: tile_body_local={:?} tile_cell={:?} tile_local_tf={:?}",
             tile_body_local, tile_cell, tile_local_tf);

    app.add_systems(Startup, move |mut commands: Commands| {
        let root = commands.spawn(BigSpace::default()).id();

        let solar_grid = commands.spawn((
            CelestialReferenceFrame { ephemeris_id: 10 },
            Grid::new(1.0e9, 1.0e30),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
        )).id();
        commands.entity(solar_grid).set_parent_in_place(root);

        let moon_grid = commands.spawn((
            CelestialReferenceFrame { ephemeris_id: 301 },
            Grid::new(MOON_GRID_CELL_SIZE as f32, 1.0e30_f32),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
        )).id();
        commands.entity(moon_grid).set_parent_in_place(solar_grid);

        // Moon Body at Grid origin, initially identity rotation
        // Note: Body does NOT have Grid component, only CellCoord.
        // Tiles must be parented to the Grid (not Body) for big_space to compute
        // their GlobalTransform from CellCoord. Rotation is synced manually.
        let moon_body = commands.spawn((
            CelestialBody {
                name: "Moon".to_string(),
                ephemeris_id: 301,
                radius_m: moon_radius,
            },
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
        )).id();
        commands.entity(moon_body).set_parent_in_place(moon_grid);

        // Terrain tile as child of the GRID (not Body), with CellCoord.
        // big_space's propagate_high_precision will compute GlobalTransform from
        // Grid + CellCoord + Transform. Rotation is synced via body_rotation_system.
        let tile = commands.spawn((
            tile_cell,
            Transform::from_translation(tile_local_tf),
            GlobalTransform::default(),
            Name::new("Test Tile"),
        )).id();
        commands.entity(moon_grid).add_child(tile);
    });

    // Run frames to let big_space propagate initial transforms
    for _ in 0..10 {
        app.update();
    }

    // Get Body and tile entities
    let (moon_body, tile_ent) = {
        let world = app.world_mut();
        let mut q_bodies = world.query::<(Entity, &CelestialBody)>();
        let moon_body = q_bodies.iter(world)
            .find(|(_, b)| b.ephemeris_id == 301)
            .map(|(e, _)| e)
            .unwrap();

        let mut q_tiles = world.query::<(Entity, &Name)>();
        let tile_ent = q_tiles.iter(world)
            .find(|(_, n)| n.as_str() == "Test Tile")
            .map(|(e, _)| e)
            .unwrap();

        // Debug: print tile's CellCoord, Transform, GlobalTransform, and parent
        let mut q_tile_data = world.query::<(&CellCoord, &Transform, &GlobalTransform, &ChildOf)>();
        if let Ok((cell, tf, gtf, child_of)) = q_tile_data.get(world, tile_ent) {
            println!("TILE DEBUG: CellCoord={:?} local_tf={:?} GlobalTransform={:?} parent={:?}",
                  cell, tf.translation, gtf.translation(), child_of.parent());
        }

        // Also print the Body's GlobalTransform
        let mut q_body_data = world.query::<(&Transform, &GlobalTransform)>();
        if let Ok((tf, gtf)) = q_body_data.get(world, moon_body) {
            println!("BODY DEBUG: local_tf={:?} GlobalTransform={:?}", tf.translation, gtf.translation());
        }

        (moon_body, tile_ent)
    };

    // Record initial tile world position (should be at equator on +X: R, 0, 0)
    let initial_tile_pos = {
        let world = app.world_mut();
        let mut q_gtf = world.query::<&GlobalTransform>();
        q_gtf.get(world, tile_ent).unwrap().translation()
    };

    assert!(
        (initial_tile_pos.x - moon_radius as f32).abs() < 100.0,
        "Initial tile X should be ~R={:.0}, got {:.0}", moon_radius, initial_tile_pos.x
    );

    // Rotate the Body 45° around Y axis
    {
        let world = app.world_mut();
        let rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);
        let mut e = world.entity_mut(moon_body);
        let mut tf = e.get_mut::<Transform>().unwrap();
        tf.rotation = rot;
    }

    // Run frames to propagate rotation via big_space
    for _ in 0..10 {
        app.update();
    }

    // Simulate tile_rotation_sync_system: runs AFTER big_space propagation.
    // It reads Body rotation and applies it to tile GlobalTransform.
    {
        let world = app.world_mut();
        let body_rot = {
            let mut q_body_data = world.query::<(&Transform,)>();
            let rot = q_body_data.get(world, moon_body).unwrap().0.rotation;
            println!("BODY ROTATION READ: {:?}", rot);
            rot
        };
        let mut q_tiles = world.query::<(&mut Transform, &mut GlobalTransform)>();
        for (mut tile_tf, mut tile_gtf) in q_tiles.iter_mut(world) {
            tile_tf.rotation = body_rot;
            // Apply rotation to the tile's world position around the Body origin.
            // Since Body is at Grid origin (CellCoord::default(), Transform::ZERO),
            // the tile's world position relative to Body = tile_gtf.translation().
            let world_pos = tile_gtf.translation();
            let rotated_pos = body_rot.mul_vec3(world_pos);
            println!("TILE ROTATION: world_pos={:?} body_rot={:?} rotated_pos={:?}", world_pos, body_rot, rotated_pos);
            *tile_gtf = GlobalTransform::from_translation(rotated_pos)
                * GlobalTransform::from_rotation(body_rot);
        }
    }

    // Do NOT run app.update() here — big_space would overwrite our GlobalTransform.
    // In the real game, tile_rotation_sync_system runs AFTER big_space propagation,
    // so its GlobalTransform update persists until the next PreUpdate cycle.

    // Debug: check tile's Transform and GlobalTransform after rotation
    {
        let world = app.world_mut();
        let mut q_tile_data = world.query::<(&CellCoord, &Transform, &GlobalTransform)>();
        if let Ok((cell, tf, gtf)) = q_tile_data.get(world, tile_ent) {
            println!("POST-ROTATE TILE DEBUG: CellCoord={:?} local_tf={:?} rotation={:?} GlobalTransform={:?}",
                  cell, tf.translation, tf.rotation, gtf.translation());
        }
    }

    // Verify tile's world position has been rotated
    let rotated_tile_pos = {
        let world = app.world_mut();
        let mut q_gtf = world.query::<&GlobalTransform>();
        q_gtf.get(world, tile_ent).unwrap().translation()
    };

    // After 45° Y rotation, tile at (R, 0, 0) should be at ~(R*cos(45°), 0, -R*sin(45°))
    // Note: Bevy uses a right-handed Y-up coordinate system where +Y rotation
    // rotates +X toward -Z.
    let r = moon_radius as f32;
    let expected_x = r * 0.7071; // R * cos(45°)
    let expected_z = -r * 0.7071; // R * -sin(45°)

    // Allow some tolerance for floating-point accumulation and Grid offset
    let tolerance = r * 0.05;

    // The key assertion: tile X should have decreased from R to ~R*cos(45°)
    assert!(
        (rotated_tile_pos.x - expected_x).abs() < tolerance,
        "After 45° Y rotation, tile X should be ~{:.0}, got {:.0}",
        expected_x, rotated_tile_pos.x
    );
    assert!(
        (rotated_tile_pos.z - expected_z).abs() < tolerance,
        "After 45° Y rotation, tile Z should be ~{:.0}, got {:.0}",
        expected_z, rotated_tile_pos.z
    );
}
