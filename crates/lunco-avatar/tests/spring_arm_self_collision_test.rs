//! The follow camera must never collide with the vehicle it is following.
//!
//! A physical rover is a JOINTED ASSEMBLY, not a subtree: the chassis carries the
//! `RigidBody` the camera follows and each wheel is its own dynamic body held on by
//! joints. Whether a wheel is parented under the chassis or sits as a sibling under
//! the grid is a physics detail that varies by drivetrain — so an exclusion set
//! built from the ECS hierarchy alone let the spring arm's obstacle ray hit the
//! rover's own wheels, collapse the arm length, and drop the camera inside the
//! vehicle.
//!
//! These tests pin the set the arm excludes: the target, everything jointed to it
//! (transitively), and every one of those bodies' descendants — while NOT swallowing
//! unrelated scenery that merely happens to be nearby.

use avian3d::prelude::*;
use bevy::prelude::*;
use lunco_avatar::vessel_collision_exclusions_for_test as exclusions;

/// Build a rover shaped like the real thing: a chassis body, six wheels joined to
/// it (as SIBLINGS under a grid root, the layout that broke the subtree-only
/// version), each wheel owning a child collider prim.
fn spawn_rover(world: &mut World) -> (Entity, Vec<Entity>, Vec<Entity>) {
    let grid = world.spawn(Name::new("Grid")).id();
    let chassis = world
        .spawn((Name::new("Chassis"), RigidBody::Dynamic, ChildOf(grid)))
        .id();

    let mut wheels = Vec::new();
    let mut wheel_colliders = Vec::new();
    for i in 0..6 {
        let wheel = world
            .spawn((
                Name::new(format!("Wheel_{i}")),
                RigidBody::Dynamic,
                ChildOf(grid),
            ))
            .id();
        // The collider geometry hangs off the wheel as its own prim.
        let col = world
            .spawn((Name::new(format!("Tire_{i}")), ChildOf(wheel)))
            .id();
        world.spawn(RevoluteJoint::new(chassis, wheel));
        wheels.push(wheel);
        wheel_colliders.push(col);
    }
    (chassis, wheels, wheel_colliders)
}

#[test]
fn excludes_every_jointed_wheel_and_its_collider_prims() {
    let mut app = App::new();
    let world = app.world_mut();
    let (chassis, wheels, wheel_colliders) = spawn_rover(world);

    let excluded = exclusions(world, chassis);

    assert!(excluded.contains(&chassis), "the followed body itself");
    for w in &wheels {
        assert!(
            excluded.contains(w),
            "a wheel joined to the chassis is part of the vehicle, not an obstacle"
        );
    }
    for c in &wheel_colliders {
        assert!(
            excluded.contains(c),
            "the wheel's own collider prim is what the ray would actually hit"
        );
    }
}

/// Transitive: a payload bolted to a wheel-mounted arm is still the vehicle.
#[test]
fn excludes_transitively_across_a_joint_chain() {
    let mut app = App::new();
    let world = app.world_mut();
    let (chassis, wheels, _) = spawn_rover(world);

    let arm = world.spawn((Name::new("Arm"), RigidBody::Dynamic)).id();
    world.spawn(FixedJoint::new(wheels[0], arm));
    let scoop = world.spawn((Name::new("Scoop"), RigidBody::Dynamic)).id();
    world.spawn(SphericalJoint::new(arm, scoop));

    let excluded = exclusions(world, chassis);
    assert!(
        excluded.contains(&arm),
        "two joints out is still the vehicle"
    );
    assert!(
        excluded.contains(&scoop),
        "three joints out is still the vehicle"
    );
}

/// The camera MUST still collide with the world. An exclusion set that swallowed
/// terrain or a neighbouring rover would put the camera back inside the scenery —
/// the opposite failure, and a silent one.
#[test]
fn does_not_exclude_unrelated_bodies() {
    let mut app = App::new();
    let world = app.world_mut();
    let (chassis, _, _) = spawn_rover(world);

    // A boulder, and a whole second rover parked alongside.
    let boulder = world.spawn((Name::new("Boulder"), RigidBody::Static)).id();
    let (other_chassis, other_wheels, _) = spawn_rover(world);

    let excluded = exclusions(world, chassis);
    assert!(
        !excluded.contains(&boulder),
        "scenery must still block the camera"
    );
    assert!(
        !excluded.contains(&other_chassis),
        "another vessel is an obstacle, not part of this one"
    );
    assert!(
        !excluded.contains(&other_wheels[0]),
        "…including its wheels"
    );
}
