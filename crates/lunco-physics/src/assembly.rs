//! Mass facts for dynamic rigid bodies connected by live physics joints.
//!
//! An articulated vehicle's translational mass is the sum of the dynamic
//! bodies in its active joint island. This module computes that generic fact
//! from Avian body state and [`PhysicsJointLink`] topology; callers remain
//! responsible for deciding which control or analysis model consumes it.

use std::collections::{HashMap, HashSet};

use avian3d::prelude::{ComputedMass, JointDisabled, RigidBody};
use bevy::prelude::*;

use crate::{PhysicsJointLink, PhysicsJointPending, PhysicsJointTopologyPending};

/// Total mass of one live dynamic joint island, repeated for each member body.
///
/// `mass_kg` is absent until every dynamic member has a finite, positive mass.
/// Consumers must gate control on [`Self::is_valid`] rather than substituting
/// a point-mass estimate or treating an unavailable measurement as zero.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DynamicJointIslandMass {
    /// Total mass of every dynamic body connected through active joints.
    pub mass_kg: Option<f64>,
    /// Number of dynamic bodies represented by the mass sample.
    pub body_count: usize,
}

impl DynamicJointIslandMass {
    /// Whether this sample contains a valid complete island mass.
    pub fn is_valid(self) -> bool {
        self.mass_kg
            .is_some_and(|mass| mass.is_finite() && mass > 0.0)
    }
}

/// Current mass sample for each dynamic body in the scene.
#[derive(Resource, Debug, Default)]
pub struct DynamicJointIslandMasses {
    entries: HashMap<Entity, DynamicJointIslandMass>,
}

impl DynamicJointIslandMasses {
    /// Read the current island mass for a body.
    pub fn get(&self, body: Entity) -> Option<DynamicJointIslandMass> {
        self.entries.get(&body).copied()
    }
}

/// Compute complete dynamic-joint-island masses from one physics snapshot.
///
/// The body iterator contains dynamic bodies only. Edges whose endpoint is not
/// in that set terminate at the static or kinematic anchor and are ignored.
/// Any dynamic member with missing, nonfinite, or nonpositive mass invalidates
/// its entire island; disconnected islands remain independent.
pub fn aggregate_dynamic_joint_island_masses(
    bodies: impl IntoIterator<Item = (Entity, Option<f64>)>,
    links: impl IntoIterator<Item = (Entity, Entity)>,
) -> HashMap<Entity, DynamicJointIslandMass> {
    let mass_by_body = bodies.into_iter().collect::<HashMap<_, _>>();
    let mut result = HashMap::with_capacity(mass_by_body.len());
    for members in dynamic_joint_islands(mass_by_body.keys().copied(), links) {
        let total_mass = members
            .iter()
            .map(|body| mass_by_body.get(body).copied().flatten())
            .collect::<Option<Vec<_>>>()
            .and_then(|masses| {
                if masses.iter().all(|mass| mass.is_finite() && *mass > 0.0) {
                    let total = masses.into_iter().sum::<f64>();
                    (total.is_finite() && total > 0.0).then_some(total)
                } else {
                    None
                }
            });
        let sample = DynamicJointIslandMass {
            mass_kg: total_mass,
            body_count: members.len(),
        };
        result.extend(members.into_iter().map(|body| (body, sample)));
    }
    result
}

/// Partition dynamic bodies by their active joint links.
///
/// Static or kinematic anchors are not in `dynamic_bodies`, so a link to one
/// stops traversal. Each component and the component list are sorted by stable
/// entity bits for deterministic consumers.
pub fn dynamic_joint_islands(
    dynamic_bodies: impl IntoIterator<Item = Entity>,
    links: impl IntoIterator<Item = (Entity, Entity)>,
) -> Vec<Vec<Entity>> {
    let bodies = dynamic_bodies.into_iter().collect::<HashSet<_>>();
    let mut adjacent = bodies
        .iter()
        .copied()
        .map(|body| (body, Vec::new()))
        .collect::<HashMap<_, _>>();

    for (body0, body1) in links {
        if body0 == body1 || !bodies.contains(&body0) || !bodies.contains(&body1) {
            continue;
        }
        adjacent
            .get_mut(&body0)
            .expect("dynamic body adjacency was initialized")
            .push(body1);
        adjacent
            .get_mut(&body1)
            .expect("dynamic body adjacency was initialized")
            .push(body0);
    }

    let mut remaining = bodies;
    let mut islands = Vec::new();
    while let Some(seed) = remaining.iter().next().copied() {
        let mut members = vec![seed];
        remaining.remove(&seed);
        let mut cursor = 0;
        while cursor < members.len() {
            let body = members[cursor];
            if let Some(neighbors) = adjacent.get(&body) {
                for neighbor in neighbors {
                    if remaining.remove(neighbor) {
                        members.push(*neighbor);
                    }
                }
            }
            cursor += 1;
        }
        members.sort_unstable_by_key(|body| body.to_bits());
        islands.push(members);
    }
    islands.sort_unstable_by_key(|island| island[0].to_bits());
    islands
}

/// Refresh the island-mass cache from the current effective Avian body masses.
///
/// Only fully admitted, enabled physics joints are considered active topology.
/// `ComputedMass` is the effective value the solver uses. An island is invalid
/// until every dynamic member has that solver-owned mass sample.
pub fn refresh_dynamic_joint_island_masses(
    bodies: Query<(Entity, &RigidBody, Option<&ComputedMass>)>,
    links: Query<
        &PhysicsJointLink,
        (
            Without<JointDisabled>,
            Without<PhysicsJointPending>,
            Without<PhysicsJointTopologyPending>,
        ),
    >,
    mut cache: ResMut<DynamicJointIslandMasses>,
) {
    let dynamic_bodies = bodies
        .iter()
        .filter(|(_, mode, _)| **mode == RigidBody::Dynamic)
        .map(|(entity, _, computed)| (entity, computed.map(|mass| mass.value())))
        .collect::<Vec<_>>();
    let dynamic_entities = dynamic_bodies
        .iter()
        .map(|(entity, _)| *entity)
        .collect::<HashSet<_>>();
    let active_links = links
        .iter()
        .filter(|link| {
            dynamic_entities.contains(&link.body0) && dynamic_entities.contains(&link.body1)
        })
        .map(|link| (link.body0, link.body1))
        .collect::<Vec<_>>();
    cache.entries = aggregate_dynamic_joint_island_masses(dynamic_bodies, active_links);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(raw: u32) -> Entity {
        Entity::from_raw_u32(raw).expect("test entity id is valid")
    }

    #[test]
    fn sums_every_dynamic_body_in_a_joint_island_and_keeps_islands_separate() {
        let lander = entity(1);
        let leg = entity(2);
        let payload = entity(3);
        let independent_body = entity(4);
        let masses = aggregate_dynamic_joint_island_masses(
            [
                (lander, Some(2_000.0)),
                (leg, Some(180.0)),
                (payload, Some(450.0)),
                (independent_body, Some(75.0)),
            ],
            [(lander, leg), (lander, payload)],
        );

        for body in [lander, leg, payload] {
            assert_eq!(masses[&body].mass_kg, Some(2_630.0));
            assert_eq!(masses[&body].body_count, 3);
        }
        assert_eq!(masses[&independent_body].mass_kg, Some(75.0));
        assert_eq!(masses[&independent_body].body_count, 1);
    }

    #[test]
    fn missing_or_invalid_member_mass_invalidates_its_whole_island() {
        let lander = entity(11);
        let ramp = entity(12);
        let independent_body = entity(13);
        let masses = aggregate_dynamic_joint_island_masses(
            [
                (lander, Some(2_000.0)),
                (ramp, None),
                (independent_body, Some(75.0)),
            ],
            [(lander, ramp)],
        );

        assert_eq!(masses[&lander].mass_kg, None);
        assert_eq!(masses[&ramp].mass_kg, None);
        assert!(!masses[&lander].is_valid());
        assert_eq!(masses[&independent_body].mass_kg, Some(75.0));
    }

    #[test]
    fn ignores_links_to_non_dynamic_anchors_and_validates_numeric_totals() {
        let dynamic = entity(21);
        let kinematic_anchor = entity(22);
        let unlinked = entity(23);
        let masses = aggregate_dynamic_joint_island_masses(
            [(dynamic, Some(f64::MAX)), (unlinked, Some(1.0))],
            [(dynamic, kinematic_anchor)],
        );

        assert_eq!(masses[&dynamic].mass_kg, Some(f64::MAX));
        assert_eq!(masses[&unlinked].mass_kg, Some(1.0));

        let overflow = aggregate_dynamic_joint_island_masses(
            [(dynamic, Some(f64::MAX)), (unlinked, Some(f64::MAX))],
            [(dynamic, unlinked)],
        );
        assert_eq!(overflow[&dynamic].mass_kg, None);
    }

    #[test]
    fn refresh_uses_effective_dynamic_mass_and_excludes_disabled_or_pending_joints() {
        let mut app = App::new();
        app.init_resource::<DynamicJointIslandMasses>()
            .add_systems(Update, refresh_dynamic_joint_island_masses);

        let lander = app
            .world_mut()
            .spawn((RigidBody::Dynamic, ComputedMass::new(2_000.0)))
            .id();
        let leg = app
            .world_mut()
            .spawn((RigidBody::Dynamic, ComputedMass::new(180.0)))
            .id();
        let ramp = app
            .world_mut()
            .spawn((RigidBody::Dynamic, ComputedMass::new(32.0)))
            .id();
        let detached = app
            .world_mut()
            .spawn((RigidBody::Dynamic, ComputedMass::new(450.0)))
            .id();
        app.world_mut().spawn(PhysicsJointLink {
            body0: lander,
            body1: leg,
        });
        app.world_mut().spawn((
            PhysicsJointLink {
                body0: lander,
                body1: ramp,
            },
            JointDisabled,
        ));
        app.world_mut().spawn((
            PhysicsJointLink {
                body0: lander,
                body1: detached,
            },
            PhysicsJointPending,
        ));

        app.update();

        let cache = app.world().resource::<DynamicJointIslandMasses>();
        assert_eq!(cache.get(lander).unwrap().mass_kg, Some(2_180.0));
        assert_eq!(cache.get(leg).unwrap().mass_kg, Some(2_180.0));
        assert_eq!(cache.get(ramp).unwrap().mass_kg, Some(32.0));
        assert_eq!(cache.get(detached).unwrap().mass_kg, Some(450.0));
    }

    #[test]
    fn refresh_invalidates_the_full_island_if_one_solver_mass_is_missing() {
        let mut app = App::new();
        app.init_resource::<DynamicJointIslandMasses>()
            .add_systems(Update, refresh_dynamic_joint_island_masses);
        let root = app
            .world_mut()
            .spawn((RigidBody::Dynamic, ComputedMass::new(2_000.0)))
            .id();
        let missing_mass = app.world_mut().spawn(RigidBody::Dynamic).id();
        app.world_mut().spawn(PhysicsJointLink {
            body0: root,
            body1: missing_mass,
        });

        app.update();

        let cache = app.world().resource::<DynamicJointIslandMasses>();
        assert_eq!(cache.get(root).unwrap().mass_kg, None);
        assert_eq!(cache.get(missing_mass).unwrap().body_count, 2);
    }
}
