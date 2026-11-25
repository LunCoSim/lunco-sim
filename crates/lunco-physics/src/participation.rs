//! Live participation of independent rigid bodies and their attached collision shapes.
use avian3d::prelude::*;
use bevy::prelude::*;
use std::collections::HashSet;

#[derive(Component)]
pub(crate) struct InactiveBody;

#[derive(Component)]
pub(crate) struct InactiveBodyCollider;

/// A validated transition consumed in the same exclusive World command.
pub struct PhysicsParticipation {
    bodies: Vec<(Entity, bool, Vec<Entity>)>,
    colliders: Vec<(Entity, bool)>,
}

pub fn prepare(
    world: &mut World,
    bodies: &[(Entity, bool)],
    colliders: &[(Entity, bool)],
) -> Result<PhysicsParticipation, String> {
    use super::readiness::{RuntimeColliderDisabled, validate_participation_owner};
    let mut targets = HashSet::new();
    let mut body_colliders = HashSet::new();
    let mut plan = PhysicsParticipation {
        bodies: Vec::new(),
        colliders: Vec::new(),
    };
    for &(body, active) in bodies {
        if !targets.insert(body) {
            return Err(format!("Repeated body {body:?}"));
        }
        if world.get::<RigidBody>(body).is_none() {
            return Err(format!("{body:?} has no admitted rigid body"));
        }
        validate_participation_owner(world, body)?;
        let inactive = world.get::<InactiveBody>(body).is_some();
        if world.get::<RigidBodyDisabled>(body).is_some() != inactive {
            return Err(format!("{body:?} is disabled by another physics owner"));
        }
        let colliders: Vec<_> = super::collider_entities(world, body).collect();
        for &collider in &colliders {
            validate_participation_owner(world, collider)?;
            body_colliders.insert(collider);
        }
        if active == inactive {
            plan.bodies.push((body, active, colliders));
        }
    }
    if !targets.is_empty()
        && world
            .query::<&crate::PhysicsJointLink>()
            .iter(world)
            .any(|link| targets.contains(&link.body0) || targets.contains(&link.body1))
    {
        return Err("Joint endpoints cannot change participation independently".into());
    }
    let mut seen = HashSet::new();
    for &(shape, enabled) in colliders {
        if !seen.insert(shape) {
            return Err(format!("Repeated collider {shape:?}"));
        }
        if body_colliders.contains(&shape) {
            return Err(format!(
                "Collider {shape:?} is also controlled by a body in this transition"
            ));
        }
        if world.get::<Collider>(shape).is_none() {
            return Err(format!("{shape:?} has no admitted native collider"));
        }
        validate_participation_owner(world, shape)?;
        let body = world
            .get::<ColliderOf>(shape)
            .map_or(shape, |owner| owner.body);
        if world.get::<InactiveBody>(body).is_some() {
            return Err(format!("{body:?} is inactive"));
        }
        let owned = world.get::<RuntimeColliderDisabled>(shape).is_some();
        if world.get::<ColliderDisabled>(shape).is_some() != owned {
            return Err(format!("{shape:?} is controlled by another physics owner"));
        }
        if enabled == owned {
            plan.colliders.push((shape, enabled));
        }
    }
    Ok(plan)
}

impl PhysicsParticipation {
    pub fn apply(self, world: &mut World) {
        use super::readiness::RuntimeColliderDisabled;
        for (body, active, colliders) in self.bodies {
            if active {
                world
                    .entity_mut(body)
                    .remove::<(InactiveBody, RigidBodyDisabled)>();
            }
            for collider in colliders {
                if !active && world.get::<ColliderDisabled>(collider).is_none() {
                    world
                        .entity_mut(collider)
                        .insert((InactiveBodyCollider, ColliderDisabled));
                } else if active && world.get::<InactiveBodyCollider>(collider).is_some() {
                    world
                        .entity_mut(collider)
                        .remove::<(InactiveBodyCollider, ColliderDisabled)>();
                }
            }
            if !active {
                world
                    .entity_mut(body)
                    .insert((InactiveBody, RigidBodyDisabled));
            }
        }
        for (shape, enabled) in self.colliders {
            if enabled {
                world
                    .entity_mut(shape)
                    .remove::<(RuntimeColliderDisabled, ColliderDisabled)>();
            } else {
                world
                    .entity_mut(shape)
                    .insert((RuntimeColliderDisabled, ColliderDisabled));
            }
        }
    }
}
