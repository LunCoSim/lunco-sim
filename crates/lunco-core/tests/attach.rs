//! Phase 7 invariant tests for `migrate_to_grid`.
//!
//! Atomicity is the contract: `ChildOf`, `CellCoord`, and `Transform`
//! must land in one archetype move so observers fired by re-parenting
//! see a consistent (parent, cell, local_tf) triple.

use bevy::prelude::*;
use big_space::plugin::BigSpaceMinimalPlugins;
use big_space::prelude::*;
use lunco_core::attach::migrate_to_grid;

fn spawn_grid(world: &mut World) -> Entity {
    world
        .spawn((
            Grid::new(1_000.0, 100.0),
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
        ))
        .id()
}

#[test]
fn migrate_to_grid_writes_three_fields_atomically() {
    let mut app = App::new();
    app.add_plugins(BigSpaceMinimalPlugins);

    let grid_a = spawn_grid(app.world_mut());
    let grid_b = spawn_grid(app.world_mut());

    let entity = app
        .world_mut()
        .spawn((
            ChildOf(grid_a),
            CellCoord::new(0, 0, 0),
            Transform::from_xyz(10.0, 0.0, 0.0),
            GlobalTransform::default(),
        ))
        .id();

    let new_cell = CellCoord::new(2, 0, 0);
    let new_tf = Transform::from_xyz(500.0, 0.0, 0.0);
    app.world_mut().commands().queue(move |world: &mut World| {
        let mut commands = world.commands();
        migrate_to_grid(&mut commands, entity, grid_b, new_cell, new_tf);
    });
    app.world_mut().flush();

    assert_eq!(app.world().get::<ChildOf>(entity).unwrap().parent(), grid_b);
    assert_eq!(*app.world().get::<CellCoord>(entity).unwrap(), new_cell);
    assert_eq!(
        app.world().get::<Transform>(entity).unwrap().translation,
        new_tf.translation
    );
}

#[test]
fn migrate_to_grid_overwrites_prior_parent_and_cell() {
    let mut app = App::new();
    app.add_plugins(BigSpaceMinimalPlugins);

    let grid_a = spawn_grid(app.world_mut());
    let grid_b = spawn_grid(app.world_mut());

    let entity = app
        .world_mut()
        .spawn((
            ChildOf(grid_a),
            CellCoord::new(7, 7, 7),
            Transform::from_xyz(1.0, 2.0, 3.0),
            GlobalTransform::default(),
        ))
        .id();

    app.world_mut().commands().queue(move |world: &mut World| {
        let mut commands = world.commands();
        migrate_to_grid(
            &mut commands,
            entity,
            grid_b,
            CellCoord::default(),
            Transform::IDENTITY,
        );
    });
    app.world_mut().flush();

    assert_eq!(app.world().get::<ChildOf>(entity).unwrap().parent(), grid_b);
    assert_eq!(*app.world().get::<CellCoord>(entity).unwrap(), CellCoord::default());
    assert_eq!(
        app.world().get::<Transform>(entity).unwrap().translation,
        Vec3::ZERO
    );
}
