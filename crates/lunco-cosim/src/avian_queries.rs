//! Avian port adapter for the physics-owned raw ray observation.
//!
//! The observation component and its query sampler live in
//! [`lunco_physics::raycast`]. This module only maps that generic physics fact
//! into the co-simulation port registry; it does not own a second observation
//! or sampling implementation.

use crate::ports::{AvianGroup, AvianPort};
use bevy::prelude::*;
use lunco_physics::raycast::RaycastObservation;
use lunco_port_core::ports::PortDirection;

/// Raw Avian ray-query ports.  The Modelica sensor wrapper consumes these
/// ports and supplies the semantic names used by a mission model.
pub const RAYCAST_GROUP: AvianGroup = AvianGroup {
    present: |world, entity| world.get::<RaycastObservation>(entity).is_some(),
    entities: |world, out| {
        out.extend(
            world
                .query_filtered::<Entity, With<RaycastObservation>>()
                .iter(world),
        );
    },
    topology_key: |world, entity| u64::from(world.get::<RaycastObservation>(entity).is_some()),
    ports: &[
        AvianPort {
            name: "ray_distance",
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.distance)
            }),
            write: None,
        },
        AvianPort {
            name: "ray_hit_valid",
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| if observation.hit_valid { 1.0 } else { 0.0 })
            }),
            write: None,
        },
        AvianPort {
            name: "ray_hit_position_x",
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.hit_position.x)
            }),
            write: None,
        },
        AvianPort {
            name: "ray_hit_position_y",
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.hit_position.y)
            }),
            write: None,
        },
        AvianPort {
            name: "ray_hit_position_z",
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.hit_position.z)
            }),
            write: None,
        },
        AvianPort {
            name: "ray_hit_normal_x",
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.hit_normal.x)
            }),
            write: None,
        },
        AvianPort {
            name: "ray_hit_normal_y",
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.hit_normal.y)
            }),
            write: None,
        },
        AvianPort {
            name: "ray_hit_normal_z",
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.hit_normal.z)
            }),
            write: None,
        },
        AvianPort {
            name: "ray_sample_time",
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.sample_time_s)
            }),
            write: None,
        },
    ],
    install_topology: register_raycast_topology,
};

fn register_raycast_topology(app: &mut App) {
    app.add_observer(lunco_port_core::ports::bump_port_topology_on_add::<RaycastObservation>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<RaycastObservation>);
}

#[cfg(test)]
mod tests {
    use super::RAYCAST_GROUP;
    use bevy::prelude::*;

    #[test]
    fn raw_group_does_not_publish_altimeter_semantics() {
        let names: Vec<_> = RAYCAST_GROUP.ports.iter().map(|port| port.name).collect();
        assert!(names.contains(&"ray_distance"));
        assert!(names.contains(&"ray_hit_valid"));
        assert!(!names.contains(&"range"));
        assert!(!names.contains(&"altitude"));
        assert!(!names.contains(&"range_rate"));
    }
}
