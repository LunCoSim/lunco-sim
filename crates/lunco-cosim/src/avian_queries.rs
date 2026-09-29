//! Avian port adapter for the physics-owned raw ray observation.
//!
//! The observation component and its query sampler live in
//! [`lunco_physics::raycast`]. This module only maps that generic physics fact
//! into the co-simulation port registry; it does not own a second observation
//! or sampling implementation.

use crate::ports::{AvianGroup, AvianPort, AvianPortContract};
use bevy::prelude::*;
use lunco_physics::raycast::RaycastObservation;
use lunco_port_core::ports::PortDirection;

/// Raw Avian ray-query ports. The Modelica sensor wrapper consumes these
/// ports and supplies the semantic names used by a mission model. The body-local
/// origin/direction ports expose the effective mounted geometry already used by
/// the raw query, including every transform between the sensor and rigid body.
pub const RAYCAST_GROUP: AvianGroup = AvianGroup {
    source: "Avian ray query",
    present: |world, entity| world.get::<RaycastObservation>(entity).is_some(),
    entities: |world, out| {
        out.extend(
            world
                .query_filtered::<Entity, With<RaycastObservation>>()
                .iter(world),
        );
    },
    topology_key: |world, entity| {
        world
            .get::<RaycastObservation>(entity)
            .map(|observation| raycast_frame_topology_key(observation.body_global_id))
            .unwrap_or_default()
    },
    ports: &[
        AvianPort {
            name: "ray_distance",
            contract: AvianPortContract::RAY_DISTANCE,
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
            contract: AvianPortContract::DIMENSIONLESS,
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
            contract: AvianPortContract::RAY_POSITION,
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
            contract: AvianPortContract::RAY_POSITION,
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
            contract: AvianPortContract::RAY_POSITION,
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
            contract: AvianPortContract::RAY_NORMAL,
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
            contract: AvianPortContract::RAY_NORMAL,
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
            contract: AvianPortContract::RAY_NORMAL,
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
            contract: AvianPortContract::RAY_TIME,
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.sample_time_s)
            }),
            write: None,
        },
        AvianPort {
            name: "ray_origin_body_local_x",
            contract: AvianPortContract::RAY_ORIGIN,
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.ray_origin_body_local.x)
            }),
            write: None,
        },
        AvianPort {
            name: "ray_origin_body_local_y",
            contract: AvianPortContract::RAY_ORIGIN,
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.ray_origin_body_local.y)
            }),
            write: None,
        },
        AvianPort {
            name: "ray_origin_body_local_z",
            contract: AvianPortContract::RAY_ORIGIN,
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.ray_origin_body_local.z)
            }),
            write: None,
        },
        AvianPort {
            name: "ray_direction_body_local_x",
            contract: AvianPortContract::RAY_DIRECTION,
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.direction_body_local.x)
            }),
            write: None,
        },
        AvianPort {
            name: "ray_direction_body_local_y",
            contract: AvianPortContract::RAY_DIRECTION,
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.direction_body_local.y)
            }),
            write: None,
        },
        AvianPort {
            name: "ray_direction_body_local_z",
            contract: AvianPortContract::RAY_DIRECTION,
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get::<RaycastObservation>(entity)
                    .map(|observation| observation.direction_body_local.z)
            }),
            write: None,
        },
    ],
    install_topology: register_raycast_topology,
};

fn register_raycast_topology(app: &mut App) {
    app.add_observer(lunco_port_core::ports::bump_port_topology_on_add::<RaycastObservation>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<RaycastObservation>)
        .add_systems(PostUpdate, check_raycast_structure);
}

fn raycast_frame_topology_key(body_global_id: Option<u64>) -> u64 {
    body_global_id.map_or(0, |body_id| body_id.rotate_left(1) | 1)
}

fn check_raycast_structure(
    changed: Query<(Entity, &RaycastObservation), Changed<RaycastObservation>>,
    mut state: ResMut<lunco_port_core::ports::PortTopologyState>,
    mut revision: ResMut<lunco_port_core::ports::PortTopologyRevision>,
) {
    for (entity, observation) in &changed {
        let key = raycast_frame_topology_key(observation.body_global_id);
        if state.changed::<RaycastObservation>(entity, key) {
            revision.bump();
        }
    }
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
        assert!(names.contains(&"ray_origin_body_local_y"));
        assert!(names.contains(&"ray_direction_body_local_y"));
        assert!(!names.contains(&"range"));
        assert!(!names.contains(&"altitude"));
        assert!(!names.contains(&"range_rate"));
    }
}
