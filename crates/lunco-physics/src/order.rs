//! Stable identities for floating-point work that mutates shared physics state.

use std::time::Duration;

use avian3d::{prelude::Physics, schedule::PhysicsTime};
use bevy::prelude::{Component, Entity, Reflect, ReflectComponent, Time, error};

use crate::PhysicsHolds;

/// Authored identity used to order physics operations that can share a body.
#[derive(Component, Clone, Debug, PartialEq, Eq, Reflect)]
#[reflect(Component)]
pub struct PhysicsOrderKey(pub String);

/// Missing or duplicate identity in an order-sensitive physics operation.
#[derive(Debug)]
pub struct PhysicsOrderError {
    /// Entity whose identity is missing or duplicates an earlier entry.
    pub entity: Entity,
    /// Owner of the ordered operation.
    pub scope: &'static str,
    /// Human-readable invariant failure.
    pub detail: String,
}

/// Validate and sort entities by their authored physics identity.
pub fn ordered_physics_entities<'a>(
    candidates: impl Iterator<Item = (Entity, Option<&'a PhysicsOrderKey>)>,
    scope: &'static str,
) -> Result<Vec<Entity>, PhysicsOrderError> {
    let mut candidates = candidates.collect::<Vec<_>>();
    if let Some((entity, _)) = candidates.iter().find(|(_, key)| key.is_none()) {
        return Err(PhysicsOrderError {
            entity: *entity,
            scope,
            detail: "operation is missing its stable order key".to_owned(),
        });
    }
    if let Some((entity, _)) = candidates
        .iter()
        .find(|(_, key)| key.is_some_and(|key| key.0.is_empty()))
    {
        return Err(PhysicsOrderError {
            entity: *entity,
            scope,
            detail: "operation has an empty stable order key".to_owned(),
        });
    }

    candidates.sort_by(|left, right| {
        left.1
            .expect("order key validated")
            .0
            .cmp(&right.1.expect("order key validated").0)
    });
    for pair in candidates.windows(2) {
        let (left_entity, left_key) = pair[0];
        let (right_entity, right_key) = pair[1];
        let left_key = &left_key.expect("order key validated").0;
        let right_key = &right_key.expect("order key validated").0;
        if left_key == right_key {
            return Err(PhysicsOrderError {
                entity: right_entity,
                scope,
                detail: format!(
                    "stable order key {left_key:?} is shared by entities {left_entity:?} and {right_entity:?}"
                ),
            });
        }
    }

    Ok(candidates.into_iter().map(|(entity, _)| entity).collect())
}

/// Stop physics after an owner finds an invalid execution order.
pub fn report_invalid_physics_order(
    holds: Option<&mut PhysicsHolds>,
    faults: Option<&mut lunco_core::RuntimeFaults>,
    physics_time: Option<&mut Time<Physics>>,
    fault_code: &'static str,
    invalid: &PhysicsOrderError,
) {
    if let Some(holds) = holds {
        holds.set(PhysicsHolds::SAFETY_FAILURE, true);
    }
    if let Some(physics_time) = physics_time {
        physics_time.pause();
        physics_time.advance_by(Duration::ZERO);
    }
    let has_fault_resource = faults.is_some();
    let won = faults.is_some_and(|faults| {
        faults.raise(
            fault_code,
            Some(invalid.entity),
            invalid.scope,
            invalid.detail.clone(),
        )
    });
    if won || !has_fault_resource {
        error!(
            "[physics] refusing to step with invalid {} order: {}",
            invalid.scope, invalid.detail
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_keys_define_total_order_and_reject_invalid_identity() {
        let first = Entity::from_raw_u32(1).expect("valid test entity");
        let second = Entity::from_raw_u32(2).expect("valid test entity");
        let first_key = PhysicsOrderKey("/Rover/A".to_owned());
        let second_key = PhysicsOrderKey("/Rover/B".to_owned());

        let ordered = ordered_physics_entities(
            [(second, Some(&second_key)), (first, Some(&first_key))].into_iter(),
            "test wheel force",
        )
        .expect("unique authored identities have a stable order");
        assert_eq!(ordered, [first, second]);

        let duplicate = ordered_physics_entities(
            [(first, Some(&first_key)), (second, Some(&first_key))].into_iter(),
            "test wheel force",
        )
        .expect_err("duplicate authored identities cannot order shared-body work");
        assert_eq!(duplicate.entity, second);

        let missing = ordered_physics_entities([(first, None)].into_iter(), "test wheel force")
            .expect_err("missing identity cannot fall back to ECS order");
        assert_eq!(missing.entity, first);
    }

    #[test]
    fn invalid_order_fault_holds_and_pauses_physics() {
        let entity = Entity::from_raw_u32(1).expect("valid test entity");
        let invalid = PhysicsOrderError {
            entity,
            scope: "test wheel force",
            detail: "missing stable order key".to_owned(),
        };
        let mut holds = PhysicsHolds::default();
        let mut faults = lunco_core::RuntimeFaults::default();
        let mut physics_time = Time::<Physics>::default();

        report_invalid_physics_order(
            Some(&mut holds),
            Some(&mut faults),
            Some(&mut physics_time),
            "physics-wheel-order-invalid",
            &invalid,
        );

        assert!(holds.holds(PhysicsHolds::SAFETY_FAILURE));
        assert!(physics_time.is_paused());
        assert!(faults.active());
    }
}
