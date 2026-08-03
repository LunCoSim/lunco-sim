//! Generic observations made by Avian's physics query API.
//!
//! This module deliberately does not know the words *IMU*, *altimeter*, or
//! *touchdown*.  It owns one reusable primitive: an authored ray mounted in a
//! USD hierarchy.  Avian samples the ray after physics writeback and publishes
//! the raw result.  Modelica components decide whether that result is useful
//! as a range measurement, terrain estimator, optical probe, or something else.
//!
//! Rigid-body state and collider contacts are already exposed directly by
//! [`crate::avian::RIGID_BODY_GROUP`] and
//! [`crate::avian::COLLIDER_CONTACT_GROUP`].  Keeping those facts in their
//! native Avian groups means adding a new flight computer does not require a
//! new Rust sensor implementation.

use crate::connection::PortDirection;
use crate::ports::{AvianGroup, AvianPort};
use avian3d::prelude::{Physics, Position, RigidBody, Rotation, SpatialQueryFilter};
use bevy::math::{DVec3, Dir3};
use bevy::prelude::*;

/// A raw, single-ray observation authored on a mounted USD prim.
///
/// Configuration (`offset`, `axis`, and `max_distance`) is structural input.
/// The result contains only what Avian's query returned: validity, distance,
/// hit point, hit normal, and the physics sample timestamp.  A miss is invalid
/// and has no invented distance or terrain altitude.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct RaycastObservation {
    /// Additional offset in the ray prim's local frame, in metres.
    pub offset: DVec3,
    /// Ray direction in the ray prim's local frame.
    pub axis: DVec3,
    /// Maximum distance passed to Avian's query, in metres.
    pub max_distance: f64,
    /// Distance returned by Avian for the last valid hit, in metres.
    pub distance: f64,
    /// World-grid position returned by the query, in metres.
    pub hit_position: DVec3,
    /// World-grid surface normal returned by the query.
    pub hit_normal: DVec3,
    /// Whether the last query hit a physical collider.
    pub hit_valid: bool,
    /// Physics-clock timestamp associated with the result.
    pub sample_time_s: f64,
}

impl Default for RaycastObservation {
    fn default() -> Self {
        Self {
            offset: DVec3::ZERO,
            axis: DVec3::NEG_Y,
            max_distance: 100.0,
            distance: 0.0,
            hit_position: DVec3::ZERO,
            hit_normal: DVec3::ZERO,
            hit_valid: false,
            sample_time_s: 0.0,
        }
    }
}

/// Raw Avian ray-query ports.  The Modelica sensor wrapper consumes these
/// ports and supplies the semantic names used by a mission model.
pub const RAYCAST_GROUP: AvianGroup = AvianGroup {
    present: |world, entity| world.get::<RaycastObservation>(entity).is_some(),
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
                world.get::<RaycastObservation>(entity).map(|observation| {
                    if observation.hit_valid {
                        1.0
                    } else {
                        0.0
                    }
                })
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
};

/// Sample every mounted ray after Avian has written back the completed physics
/// state.  The next co-simulation propagation consumes this observation; there
/// is no same-tick feedback from a query that has not yet seen the solver.
pub fn sample_raycast_observations(
    grid: lunco_physics::GridSpatialQuery,
    time: Res<Time<Physics>>,
    parents: Query<&ChildOf>,
    transforms: Query<&Transform>,
    bodies: Query<(&Position, &Rotation), With<RigidBody>>,
    mut observations: Query<(Entity, &mut RaycastObservation)>,
) {
    for (entity, mut observation) in &mut observations {
        let mut cursor = entity;
        let mut mount = Transform::IDENTITY;
        let Some((body_position, body_rotation, excluded_entities)) = (0..64).find_map(|_| {
            if let Ok((position, rotation)) = bodies.get(cursor) {
                let mut excluded = Vec::new();
                let mut ancestor = entity;
                while ancestor != cursor {
                    excluded.push(ancestor);
                    ancestor = parents.get(ancestor).ok()?.0;
                }
                excluded.push(cursor);
                return Some((position, rotation, excluded));
            }
            if let Ok(local) = transforms.get(cursor) {
                mount = local.mul_transform(mount);
            }
            cursor = parents.get(cursor).ok()?.0;
            None
        }) else {
            continue;
        };

        let body_rotation = body_rotation.0;
        let mount_offset =
            mount.translation.as_dvec3() + mount.rotation.as_dquat() * observation.offset;
        let origin = body_position.0 + body_rotation * mount_offset;
        let direction = body_rotation * (mount.rotation.as_dquat() * observation.axis);
        let Ok(direction) = Dir3::new(direction.as_vec3()) else {
            continue;
        };

        let mut filter = SpatialQueryFilter::from_mask(avian3d::prelude::LayerMask(
            !lunco_core::NON_PHYSICAL_QUERY_LAYERS,
        ));
        for excluded in excluded_entities {
            filter.excluded_entities.insert(excluded);
        }

        let Some(hit) = grid.cast_ray_grid(
            lunco_core::coords::GridPos(origin),
            direction,
            observation.max_distance,
            true,
            &filter,
        ) else {
            observation.distance = 0.0;
            observation.hit_position = DVec3::ZERO;
            observation.hit_normal = DVec3::ZERO;
            observation.hit_valid = false;
            observation.sample_time_s = time.elapsed_secs_f64();
            continue;
        };

        observation.distance = hit.distance;
        observation.hit_position = origin + direction.as_vec3().as_dvec3() * hit.distance;
        observation.hit_normal = hit.normal;
        observation.hit_valid = true;
        observation.sample_time_s = time.elapsed_secs_f64();
    }
}

#[cfg(test)]
mod tests {
    use super::{RaycastObservation, RAYCAST_GROUP};
    use bevy::prelude::*;

    #[test]
    fn a_miss_is_an_invalid_raw_observation_without_a_fallback_distance() {
        let observation = RaycastObservation::default();
        assert!(!observation.hit_valid);
        assert_eq!(observation.distance, 0.0);
    }

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
