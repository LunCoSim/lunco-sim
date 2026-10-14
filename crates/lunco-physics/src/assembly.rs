//! Mass facts for dynamic rigid bodies connected by live physics joints.
//!
//! An articulated vehicle's translational mass is the sum of the dynamic
//! bodies in its active joint island. This module computes that generic fact
//! from Avian body state and [`PhysicsJointLink`] topology; callers remain
//! responsible for deciding which control or analysis model consumes it.

use std::collections::{HashMap, HashSet};

use avian3d::prelude::{ComputedMass, JointDisabled, RigidBody};
use bevy::prelude::*;

use crate::{
    PhysicsJointLink, PhysicsJointPending, PhysicsJointPrepared, PhysicsJointTopologyPending,
    PhysicsOrderKey,
};

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
#[derive(Resource, Debug)]
pub struct DynamicJointIslandMasses {
    entries: HashMap<Entity, DynamicJointIslandMass>,
    topology_dirty: bool,
    initial_members: HashMap<Entity, std::sync::Arc<[Entity]>>,
}

impl Default for DynamicJointIslandMasses {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            topology_dirty: true,
            initial_members: HashMap::new(),
        }
    }
}

impl DynamicJointIslandMasses {
    /// Read the current island mass for a body.
    pub fn get(&self, body: Entity) -> Option<DynamicJointIslandMass> {
        self.entries.get(&body).copied()
    }

    /// Complete mechanical topology at the prepared initial coordinate.
    /// Mass dependencies remain equation-owned until all members are solved.
    pub fn initial_members(&self, body: Entity) -> Option<&[Entity]> {
        self.initial_members
            .get(&body)
            .map(|members| members.as_ref())
    }
}

/// Retain structural invalidation until the next scheduled mass refresh.
///
/// Lifecycle observers survive a paused simulation and share invalidation
/// across normal and rollback schedules. Mutable mass and link edits are
/// checked at the refresh boundary through native component change detection.
pub struct DynamicJointIslandMassPlugin;

impl Plugin for DynamicJointIslandMassPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DynamicJointIslandMasses>();
        macro_rules! track {
            ($component:ty) => {
                app.add_observer(
                    |_: On<Insert, $component>, mut cache: ResMut<DynamicJointIslandMasses>| {
                        if !cache.topology_dirty {
                            cache.topology_dirty = true;
                        }
                    },
                )
                .add_observer(
                    |_: On<Remove, $component>, mut cache: ResMut<DynamicJointIslandMasses>| {
                        if !cache.topology_dirty {
                            cache.topology_dirty = true;
                        }
                    },
                );
            };
        }
        track!(RigidBody);
        track!(ComputedMass);
        track!(PhysicsJointLink);
        track!(JointDisabled);
        track!(PhysicsJointPending);
        track!(PhysicsJointTopologyPending);
        track!(PhysicsJointPrepared);
        track!(PhysicsOrderKey);
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
/// Install [`DynamicJointIslandMassPlugin`] to retain lifecycle invalidation;
/// refresh stays at the consumer's existing normal/rollback exchange boundary.
pub fn refresh_dynamic_joint_island_masses(
    bodies: Query<(
        Entity,
        &RigidBody,
        Option<&ComputedMass>,
        Option<&PhysicsOrderKey>,
    )>,
    links: Query<
        &PhysicsJointLink,
        (
            Without<JointDisabled>,
            Without<PhysicsJointPending>,
            Without<PhysicsJointTopologyPending>,
        ),
    >,
    changed_bodies: Query<(), (With<RigidBody>, Changed<ComputedMass>)>,
    changed_links: Query<(), Changed<PhysicsJointLink>>,
    all_links: Query<(
        &PhysicsJointLink,
        Has<JointDisabled>,
        Has<PhysicsJointPending>,
        Has<PhysicsJointTopologyPending>,
        Option<&PhysicsJointPrepared>,
    )>,
    mut cache: ResMut<DynamicJointIslandMasses>,
) {
    if !cache.topology_dirty && changed_bodies.is_empty() && changed_links.is_empty() {
        return;
    }
    let _span = bevy::log::info_span!("dynamic_joint_island_mass_rebuild").entered();
    let dynamic_bodies = bodies
        .iter()
        .filter(|(_, mode, _, _)| **mode == RigidBody::Dynamic)
        .map(|(entity, _, computed, _)| (entity, computed.map(|mass| mass.value())))
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
    let mut preparing = HashSet::new();
    let initial_links = all_links
        .iter()
        .filter_map(|(link, disabled, pending, topology_pending, prepared)| {
            if prepared.is_some_and(|prepared| !prepared.activate) {
                return None;
            }
            let prepared = prepared.is_some_and(|prepared| prepared.activate);
            if disabled && !prepared && !pending && !topology_pending {
                return None;
            }
            if (pending || topology_pending) && !prepared {
                preparing.insert(link.body0);
                preparing.insert(link.body1);
            }
            Some((link.body0, link.body1))
        })
        .collect::<Vec<_>>();
    cache.initial_members.clear();
    for mut members in dynamic_joint_islands(dynamic_entities, initial_links) {
        if members.iter().any(|member| {
            preparing.contains(member)
                || !bodies
                    .get(*member)
                    .is_ok_and(|(_, _, _, key)| key.is_some())
        }) {
            continue;
        }
        members.sort_by(|a, b| {
            bodies
                .get(*a)
                .unwrap()
                .3
                .unwrap()
                .0
                .cmp(&bodies.get(*b).unwrap().3.unwrap().0)
        });
        let shared: std::sync::Arc<[Entity]> = members.clone().into();
        cache
            .initial_members
            .extend(members.into_iter().map(|member| (member, shared.clone())));
    }
    cache.topology_dirty = false;
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
        app.add_plugins(DynamicJointIslandMassPlugin)
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
        app.add_plugins(DynamicJointIslandMassPlugin)
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

    #[test]
    fn quiet_refresh_retains_the_resource_until_a_mutable_input_changes() {
        let mut app = App::new();
        app.add_plugins(DynamicJointIslandMassPlugin)
            .add_systems(Update, refresh_dynamic_joint_island_masses)
            .add_systems(
                lunco_core_runtime::RollbackReplay,
                refresh_dynamic_joint_island_masses,
            );
        let body = app
            .world_mut()
            .spawn((RigidBody::Dynamic, ComputedMass::new(3.0)))
            .id();
        app.update();
        app.world_mut()
            .run_schedule(lunco_core_runtime::RollbackReplay);
        let settled = app
            .world()
            .resource_ref::<DynamicJointIslandMasses>()
            .last_changed();
        app.update();
        app.world_mut()
            .run_schedule(lunco_core_runtime::RollbackReplay);
        assert_eq!(
            app.world()
                .resource_ref::<DynamicJointIslandMasses>()
                .last_changed(),
            settled
        );
        app.world_mut()
            .get_mut::<ComputedMass>(body)
            .unwrap()
            .set(7.0);
        app.update();
        assert_eq!(
            app.world()
                .resource::<DynamicJointIslandMasses>()
                .get(body)
                .unwrap()
                .mass_kg,
            Some(7.0)
        );
        app.world_mut()
            .entity_mut(body)
            .insert(RigidBody::Kinematic);
        app.update();
        assert_eq!(
            app.world().resource::<DynamicJointIslandMasses>().get(body),
            None
        );
    }

    #[derive(Resource)]
    struct RefreshEnabled(bool);

    #[test]
    fn lifecycle_invalidation_survives_paused_refresh_and_retires_despawned_inputs() {
        let mut app = App::new();
        app.add_plugins(DynamicJointIslandMassPlugin)
            .insert_resource(RefreshEnabled(true))
            .add_systems(
                Update,
                refresh_dynamic_joint_island_masses
                    .run_if(|enabled: Res<RefreshEnabled>| enabled.0),
            );
        let root = app
            .world_mut()
            .spawn((RigidBody::Dynamic, ComputedMass::new(3.0)))
            .id();
        let part = app
            .world_mut()
            .spawn((RigidBody::Dynamic, ComputedMass::new(7.0)))
            .id();
        let joint = app
            .world_mut()
            .spawn(PhysicsJointLink {
                body0: root,
                body1: part,
            })
            .id();
        app.update();
        app.world_mut().resource_mut::<RefreshEnabled>().0 = false;
        app.world_mut().entity_mut(joint).insert(JointDisabled);
        for _ in 0..5 {
            app.update();
        }
        assert!(
            app.world()
                .resource::<DynamicJointIslandMasses>()
                .topology_dirty
        );
        app.world_mut().resource_mut::<RefreshEnabled>().0 = true;
        app.update();
        assert_eq!(
            app.world()
                .resource::<DynamicJointIslandMasses>()
                .get(root)
                .unwrap()
                .mass_kg,
            Some(3.0)
        );
        app.world_mut().entity_mut(joint).remove::<JointDisabled>();
        app.update();
        assert_eq!(
            app.world()
                .resource::<DynamicJointIslandMasses>()
                .get(root)
                .unwrap()
                .mass_kg,
            Some(10.0)
        );
        app.world_mut().resource_mut::<RefreshEnabled>().0 = false;
        app.world_mut().entity_mut(part).remove::<ComputedMass>();
        for _ in 0..5 {
            app.update();
        }
        app.world_mut().resource_mut::<RefreshEnabled>().0 = true;
        app.update();
        assert_eq!(
            app.world()
                .resource::<DynamicJointIslandMasses>()
                .get(root)
                .unwrap()
                .mass_kg,
            None
        );
        app.world_mut()
            .entity_mut(part)
            .insert(ComputedMass::new(11.0));
        app.update();
        assert_eq!(
            app.world()
                .resource::<DynamicJointIslandMasses>()
                .get(root)
                .unwrap()
                .mass_kg,
            Some(14.0)
        );
        app.world_mut().resource_mut::<RefreshEnabled>().0 = false;
        app.world_mut().despawn(joint);
        app.world_mut().despawn(part);
        for _ in 0..5 {
            app.update();
        }
        app.world_mut().resource_mut::<RefreshEnabled>().0 = true;
        app.update();
        let cache = app.world().resource::<DynamicJointIslandMasses>();
        assert_eq!(cache.get(part), None);
        assert_eq!(cache.get(root).unwrap().mass_kg, Some(3.0));
        app.world_mut().entity_mut(root).remove::<RigidBody>();
        app.update();
        assert!(
            app.world()
                .resource::<DynamicJointIslandMasses>()
                .entries
                .is_empty()
        );
    }

    #[test]
    fn admission_markers_and_mutable_joint_endpoints_invalidate_the_shared_cache() {
        let mut app = App::new();
        app.add_plugins(DynamicJointIslandMassPlugin)
            .add_systems(Update, refresh_dynamic_joint_island_masses);
        let a = app
            .world_mut()
            .spawn((RigidBody::Dynamic, ComputedMass::new(2.0)))
            .id();
        let b = app
            .world_mut()
            .spawn((RigidBody::Dynamic, ComputedMass::new(5.0)))
            .id();
        let c = app
            .world_mut()
            .spawn((RigidBody::Dynamic, ComputedMass::new(11.0)))
            .id();
        let joint = app
            .world_mut()
            .spawn((
                PhysicsJointLink { body0: a, body1: b },
                PhysicsJointPending,
                PhysicsJointTopologyPending,
            ))
            .id();
        app.update();
        app.world_mut()
            .entity_mut(joint)
            .remove::<PhysicsJointPending>();
        app.update();
        assert_eq!(
            app.world()
                .resource::<DynamicJointIslandMasses>()
                .get(a)
                .unwrap()
                .mass_kg,
            Some(2.0)
        );
        app.world_mut()
            .entity_mut(joint)
            .remove::<PhysicsJointTopologyPending>();
        app.update();
        assert_eq!(
            app.world()
                .resource::<DynamicJointIslandMasses>()
                .get(a)
                .unwrap()
                .mass_kg,
            Some(7.0)
        );
        app.world_mut()
            .get_mut::<PhysicsJointLink>(joint)
            .unwrap()
            .body1 = c;
        app.update();
        assert_eq!(
            app.world()
                .resource::<DynamicJointIslandMasses>()
                .get(a)
                .unwrap()
                .mass_kg,
            Some(13.0)
        );
        app.world_mut()
            .entity_mut(joint)
            .insert(PhysicsJointPending);
        app.update();
        assert_eq!(
            app.world()
                .resource::<DynamicJointIslandMasses>()
                .get(a)
                .unwrap()
                .mass_kg,
            Some(2.0)
        );
        app.world_mut()
            .entity_mut(joint)
            .remove::<PhysicsJointPending>();
        app.world_mut()
            .entity_mut(joint)
            .insert(PhysicsJointTopologyPending);
        app.update();
        assert_eq!(
            app.world()
                .resource::<DynamicJointIslandMasses>()
                .get(a)
                .unwrap()
                .mass_kg,
            Some(2.0)
        );
        app.world_mut()
            .entity_mut(joint)
            .remove::<PhysicsJointTopologyPending>();
        app.update();
        assert_eq!(
            app.world()
                .resource::<DynamicJointIslandMasses>()
                .get(a)
                .unwrap()
                .mass_kg,
            Some(13.0)
        );
    }
}
