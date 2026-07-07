//! Entity identity registry — maps stable ULID-based GlobalEntityId to Bevy Entity.

use std::collections::HashMap;
use bevy::prelude::*;
use lunco_core::GlobalEntityId;

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

    pub fn entities(&self) -> Vec<(GlobalEntityId, Entity)> {
        self.api_to_bevy.iter().map(|(&id, &entity)| (id, entity)).collect()
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
        app.init_resource::<ApiEntityRegistry>()
            .add_systems(Update, sync_api_registry);
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
        assert_eq!(registry.entities(), vec![(id, b)]);
    }
}
