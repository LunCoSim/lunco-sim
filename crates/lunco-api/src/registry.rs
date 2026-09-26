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

/// System that synchronizes [GlobalEntityId] components into the [ApiEntityRegistry].
pub fn sync_api_registry(
    mut registry: ResMut<ApiEntityRegistry>,
    q_added: Query<(Entity, &GlobalEntityId), Added<GlobalEntityId>>,
    mut q_removed: RemovedComponents<GlobalEntityId>,
) {
    // Removes FIRST: a scene reload despawns a prim's entity and spawns its
    // replacement with the SAME deterministic id in the same frame. Processing
    // the add first and the remove second handed `remove` a stale reverse
    // entry for the reused id (guarded in `remove` too, belt and braces).
    for entity in q_removed.read() {
        registry.remove(entity);
    }
    for (entity, id) in q_added.iter() {
        registry.assign(entity, *id);
    }
}

/// Plugin that registers the entity registry and synchronization system.
pub struct ApiEntityRegistryPlugin;

impl Plugin for ApiEntityRegistryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ApiEntityRegistry>().add_systems(
            PreUpdate,
            sync_api_registry.in_set(lunco_core::RuntimeCycleSet::EntityIndex),
        );
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
