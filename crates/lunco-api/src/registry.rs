//! Entity identity registry — maps stable GlobalEntityId values to Bevy entities.

use bevy::prelude::*;
use lunco_core::GlobalEntityId;
use std::collections::HashMap;

/// Bidirectional mapping between API entity IDs and Bevy entities.
#[derive(Resource, Default)]
pub struct ApiEntityRegistry {
    api_to_bevy: HashMap<GlobalEntityId, Entity>,
    bevy_to_api: HashMap<Entity, GlobalEntityId>,
}

impl ApiEntityRegistry {
    pub fn assign(&mut self, entity: Entity, id: GlobalEntityId) {
        if let Some(previous_id) = self.bevy_to_api.remove(&entity)
            && previous_id != id
            && self.api_to_bevy.get(&previous_id) == Some(&entity)
        {
            self.api_to_bevy.remove(&previous_id);
        }
        // An id is DETERMINISTIC (prim path / provenance), so a scene reload
        // moves it from the despawned entity to its replacement. Drop the old
        // reverse entry or it dangles and later poisons a `remove`.
        if let Some(old) = self.api_to_bevy.insert(id, entity) {
            if old != entity {
                self.bevy_to_api.remove(&old);
            }
        }
        self.bevy_to_api.insert(entity, id);
    }

    pub fn remove(&mut self, entity: Entity) {
        if let Some(id) = self.bevy_to_api.remove(&entity) {
            // Only drop the forward mapping if it still points at THIS entity.
            // On a reload the id has already been re-assigned to the
            // replacement entity — removing it here made every re-projected
            // prim (the doc-backed twin's rovers!) vanish from the API: not
            // listable, not resolvable, not possessable ("the physical rover
            // doesn't work"), while the entity itself lived on in the scene.
            if self.api_to_bevy.get(&id) == Some(&entity) {
                self.api_to_bevy.remove(&id);
            }
        }
    }

    pub fn resolve(&self, id: &GlobalEntityId) -> Option<Entity> {
        self.api_to_bevy.get(id).copied()
    }

    pub fn api_id_for(&self, entity: Entity) -> Option<GlobalEntityId> {
        self.bevy_to_api.get(&entity).copied()
    }

    /// Snapshot registered identities without imposing a stable iteration
    /// order. Callers that expose a batch or choose a first match must sort by
    /// `GlobalEntityId` or use [`Self::entities_by_identity`].
    pub fn entities_unordered(&self) -> Vec<(GlobalEntityId, Entity)> {
        self.api_to_bevy
            .iter()
            .map(|(&id, &entity)| (id, entity))
            .collect()
    }

    /// Snapshot registered identities in stable ascending `GlobalEntityId`
    /// order for observable batches and deterministic first-match selection.
    pub fn entities_by_identity(&self) -> Vec<(GlobalEntityId, Entity)> {
        let mut entities = self.entities_unordered();
        entities.sort_unstable_by_key(|(id, _)| id.get());
        entities
    }
}

fn index_global_entity_id(
    trigger: On<Insert, GlobalEntityId>,
    ids: Query<&GlobalEntityId>,
    mut registry: ResMut<ApiEntityRegistry>,
) {
    if let Ok(id) = ids.get(trigger.entity) {
        registry.assign(trigger.entity, *id);
    }
}

fn index_existing_global_entity_ids(
    ids: Query<(Entity, &GlobalEntityId)>,
    mut registry: ResMut<ApiEntityRegistry>,
) {
    for (entity, id) in &ids {
        registry.assign(entity, *id);
    }
}

fn unindex_global_entity_id(
    trigger: On<Remove, GlobalEntityId>,
    mut registry: ResMut<ApiEntityRegistry>,
) {
    registry.remove(trigger.entity);
}

/// Plugin that registers the entity registry and its component lifecycle observers.
pub struct ApiEntityRegistryPlugin;

impl Plugin for ApiEntityRegistryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ApiEntityRegistry>()
            .add_systems(PreStartup, index_existing_global_entity_ids)
            .add_observer(index_global_entity_id)
            .add_observer(unindex_global_entity_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assign_and_resolve() {
        let mut registry = ApiEntityRegistry::default();
        let entity = Entity::PLACEHOLDER;
        let id = GlobalEntityId::from_raw(42);
        registry.assign(entity, id);
        assert_eq!(registry.resolve(&id), Some(entity));
    }

    #[test]
    fn test_remove() {
        let mut registry = ApiEntityRegistry::default();
        let entity = Entity::PLACEHOLDER;
        let id = GlobalEntityId::from_raw(43);
        registry.assign(entity, id);
        registry.remove(entity);
        assert_eq!(registry.resolve(&id), None);
    }

    #[test]
    fn reload_reassigns_id_and_late_remove_of_old_entity_keeps_new_mapping() {
        // Scene reload: the deterministic id moves from despawned entity A to
        // replacement B; A's removal event may be processed AFTER the assign.
        // The late remove must not clobber B's mapping (the bug that made
        // every re-projected rover vanish from the API).
        let mut registry = ApiEntityRegistry::default();
        let a = Entity::from_raw_u32(1).unwrap();
        let b = Entity::from_raw_u32(2).unwrap();
        let id = GlobalEntityId::from_raw(44);
        registry.assign(a, id);
        registry.assign(b, id);
        registry.remove(a);
        assert_eq!(registry.resolve(&id), Some(b));
        assert_eq!(registry.api_id_for(b), Some(id));
        assert_eq!(registry.api_id_for(a), None);
        assert_eq!(registry.entities_by_identity(), vec![(id, b)]);
    }

    #[test]
    fn component_insert_replace_and_remove_update_registry_immediately() {
        let mut app = App::new();
        app.add_plugins(ApiEntityRegistryPlugin);

        let entity = app.world_mut().spawn(GlobalEntityId::from_raw(51)).id();
        assert_eq!(
            app.world()
                .resource::<ApiEntityRegistry>()
                .resolve(&GlobalEntityId::from_raw(51)),
            Some(entity),
        );

        app.world_mut()
            .entity_mut(entity)
            .insert(GlobalEntityId::from_raw(52));
        let registry = app.world().resource::<ApiEntityRegistry>();
        assert_eq!(registry.resolve(&GlobalEntityId::from_raw(51)), None);
        assert_eq!(
            registry.resolve(&GlobalEntityId::from_raw(52)),
            Some(entity)
        );

        app.world_mut().despawn(entity);
        let registry = app.world().resource::<ApiEntityRegistry>();
        assert_eq!(registry.resolve(&GlobalEntityId::from_raw(52)), None);
    }

    #[test]
    fn stable_entity_snapshot_uses_identity_order_not_assignment_order() {
        let low = GlobalEntityId::from_raw(11);
        let middle = GlobalEntityId::from_raw(22);
        let high = GlobalEntityId::from_raw(33);
        let entries = [
            (high, Entity::from_raw_u32(3).unwrap()),
            (low, Entity::from_raw_u32(1).unwrap()),
            (middle, Entity::from_raw_u32(2).unwrap()),
        ];
        let mut registry = ApiEntityRegistry::default();
        for (id, entity) in entries {
            registry.assign(entity, id);
        }

        assert_eq!(
            registry.entities_by_identity(),
            vec![
                (low, Entity::from_raw_u32(1).unwrap()),
                (middle, Entity::from_raw_u32(2).unwrap()),
                (high, Entity::from_raw_u32(3).unwrap())
            ]
        );
    }
}
