use avian3d::prelude::*;
use bevy::prelude::*;

/// Append every descendant of `root` to `out`.
fn collect_subtree(root: Entity, q_children: &Query<&Children>, out: &mut Vec<Entity>) {
    if let Ok(children) = q_children.get(root) {
        for &child in children {
            out.push(child);
            collect_subtree(child, q_children, out);
        }
    }
}

/// Every Avian joint type as one connectivity view for the spring arm.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct VesselJoints<'w, 's> {
    revolute: Query<'w, 's, &'static RevoluteJoint>,
    fixed: Query<'w, 's, &'static FixedJoint>,
    prismatic: Query<'w, 's, &'static PrismaticJoint>,
    spherical: Query<'w, 's, &'static SphericalJoint>,
    distance: Query<'w, 's, &'static DistanceJoint>,
}

impl VesselJoints<'_, '_> {
    fn adjacency(&self) -> bevy::platform::collections::HashMap<Entity, Vec<Entity>> {
        let mut adjacency: bevy::platform::collections::HashMap<Entity, Vec<Entity>> =
            bevy::platform::collections::HashMap::default();
        let mut link = |a: Entity, b: Entity| {
            adjacency.entry(a).or_default().push(b);
            adjacency.entry(b).or_default().push(a);
        };
        self.revolute
            .iter()
            .for_each(|joint| link(joint.body1, joint.body2));
        self.fixed
            .iter()
            .for_each(|joint| link(joint.body1, joint.body2));
        self.prismatic
            .iter()
            .for_each(|joint| link(joint.body1, joint.body2));
        self.spherical
            .iter()
            .for_each(|joint| link(joint.body1, joint.body2));
        self.distance
            .iter()
            .for_each(|joint| link(joint.body1, joint.body2));
        adjacency
    }
}

/// Structural inputs for the cached spring-arm self-collision filter.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct VesselCollisionTopology<'w, 's> {
    children: Query<'w, 's, (), Or<(Added<Children>, Changed<Children>)>>,
    parents: Query<'w, 's, (), Or<(Added<ChildOf>, Changed<ChildOf>)>>,
    revolute: Query<
        'w,
        's,
        (Entity, &'static RevoluteJoint),
        Or<(Added<RevoluteJoint>, Changed<RevoluteJoint>)>,
    >,
    fixed:
        Query<'w, 's, (Entity, &'static FixedJoint), Or<(Added<FixedJoint>, Changed<FixedJoint>)>>,
    prismatic: Query<
        'w,
        's,
        (Entity, &'static PrismaticJoint),
        Or<(Added<PrismaticJoint>, Changed<PrismaticJoint>)>,
    >,
    spherical: Query<
        'w,
        's,
        (Entity, &'static SphericalJoint),
        Or<(Added<SphericalJoint>, Changed<SphericalJoint>)>,
    >,
    distance: Query<
        'w,
        's,
        (Entity, &'static DistanceJoint),
        Or<(Added<DistanceJoint>, Changed<DistanceJoint>)>,
    >,
    removed_children: RemovedComponents<'w, 's, Children>,
    removed_parents: RemovedComponents<'w, 's, ChildOf>,
    removed_revolute: RemovedComponents<'w, 's, RevoluteJoint>,
    removed_fixed: RemovedComponents<'w, 's, FixedJoint>,
    removed_prismatic: RemovedComponents<'w, 's, PrismaticJoint>,
    removed_spherical: RemovedComponents<'w, 's, SphericalJoint>,
    removed_distance: RemovedComponents<'w, 's, DistanceJoint>,
}

/// Render-rate cache for spring-arm self-collision filters.
#[derive(Default)]
pub(crate) struct VesselCollisionFilterCache {
    adjacency: bevy::platform::collections::HashMap<Entity, Vec<Entity>>,
    joint_bodies: bevy::ecs::entity::EntityHashMap<[Entity; 2]>,
    filters: bevy::ecs::entity::EntityHashMap<SpatialQueryFilter>,
    initialized: bool,
}

impl VesselCollisionFilterCache {
    fn observe_joint(&mut self, entity: Entity, bodies: [Entity; 2]) -> bool {
        let changed = self.joint_bodies.get(&entity) != Some(&bodies);
        self.joint_bodies.insert(entity, bodies);
        changed
    }

    fn remove_joint(&mut self, entity: Entity) -> bool {
        self.joint_bodies.remove(&entity).is_some()
    }

    pub(crate) fn refresh(
        &mut self,
        joints: &VesselJoints,
        topology: &mut VesselCollisionTopology,
    ) {
        let mut dirty = !self.initialized;
        dirty |= !topology.children.is_empty() || !topology.parents.is_empty();
        dirty |= topology.removed_children.read().next().is_some();
        dirty |= topology.removed_parents.read().next().is_some();

        for (entity, joint) in &topology.revolute {
            dirty |= self.observe_joint(entity, [joint.body1, joint.body2]);
        }
        for (entity, joint) in &topology.fixed {
            dirty |= self.observe_joint(entity, [joint.body1, joint.body2]);
        }
        for (entity, joint) in &topology.prismatic {
            dirty |= self.observe_joint(entity, [joint.body1, joint.body2]);
        }
        for (entity, joint) in &topology.spherical {
            dirty |= self.observe_joint(entity, [joint.body1, joint.body2]);
        }
        for (entity, joint) in &topology.distance {
            dirty |= self.observe_joint(entity, [joint.body1, joint.body2]);
        }

        for entity in topology.removed_revolute.read() {
            self.remove_joint(entity);
            dirty = true;
        }
        for entity in topology.removed_fixed.read() {
            self.remove_joint(entity);
            dirty = true;
        }
        for entity in topology.removed_prismatic.read() {
            self.remove_joint(entity);
            dirty = true;
        }
        for entity in topology.removed_spherical.read() {
            self.remove_joint(entity);
            dirty = true;
        }
        for entity in topology.removed_distance.read() {
            self.remove_joint(entity);
            dirty = true;
        }

        if !dirty {
            return;
        }
        self.adjacency = joints.adjacency();
        self.filters.clear();
        self.initialized = true;
    }

    pub(crate) fn filter_for(
        &mut self,
        target: Entity,
        q_children: &Query<&Children>,
    ) -> &SpatialQueryFilter {
        if !self.filters.contains_key(&target) {
            let excluded =
                vessel_collision_exclusions_from_adjacency(target, q_children, &self.adjacency);
            let mut filter = SpatialQueryFilter::from_excluded_entities(excluded);
            filter.mask = LayerMask(!lunco_core::NON_PHYSICAL_QUERY_LAYERS);
            self.filters.insert(target, filter);
        }
        self.filters
            .get(&target)
            .expect("spring-arm filter inserted above")
    }
}

#[cfg(test)]
fn vessel_collision_exclusions(
    target: Entity,
    q_children: &Query<&Children>,
    joints: &VesselJoints,
) -> Vec<Entity> {
    let adjacency = joints.adjacency();
    vessel_collision_exclusions_from_adjacency(target, q_children, &adjacency)
}

fn vessel_collision_exclusions_from_adjacency(
    target: Entity,
    q_children: &Query<&Children>,
    adjacency: &bevy::platform::collections::HashMap<Entity, Vec<Entity>>,
) -> Vec<Entity> {
    let mut members = vec![target];
    let mut seen = bevy::platform::collections::HashSet::from([target]);
    let mut queue = std::collections::VecDeque::from([target]);
    while let Some(entity) = queue.pop_front() {
        for &neighbor in adjacency.get(&entity).into_iter().flatten() {
            if seen.insert(neighbor) {
                members.push(neighbor);
                queue.push_back(neighbor);
            }
        }
    }

    let mut excluded = members.clone();
    for member in members {
        collect_subtree(member, q_children, &mut excluded);
    }
    excluded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exclusions(world: &mut World, target: Entity) -> Vec<Entity> {
        let mut system = bevy::ecs::system::IntoSystem::into_system(
            move |q_children: Query<&Children>, joints: VesselJoints| {
                vessel_collision_exclusions(target, &q_children, &joints)
            },
        );
        system.initialize(world);
        system
            .run((), world)
            .expect("exclusion query cannot fail because it only reads")
    }

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
            let collider = world
                .spawn((Name::new(format!("Tire_{i}")), ChildOf(wheel)))
                .id();
            world.spawn(RevoluteJoint::new(chassis, wheel));
            wheels.push(wheel);
            wheel_colliders.push(collider);
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
        for wheel in &wheels {
            assert!(
                excluded.contains(wheel),
                "a wheel joined to the chassis is part of the vehicle, not an obstacle"
            );
        }
        for collider in &wheel_colliders {
            assert!(
                excluded.contains(collider),
                "the wheel's own collider prim is what the ray would actually hit"
            );
        }
    }

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

    #[test]
    fn does_not_exclude_unrelated_bodies() {
        let mut app = App::new();
        let world = app.world_mut();
        let (chassis, _, _) = spawn_rover(world);

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
        assert!(!excluded.contains(&other_wheels[0]), "including its wheels");
    }
}
