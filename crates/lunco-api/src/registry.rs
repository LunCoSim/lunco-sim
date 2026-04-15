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
        self.api_to_bevy.insert(id, entity);
        self.bevy_to_api.insert(entity, id);
    }

    pub fn remove(&mut self, entity: Entity) {
        if let Some(id) = self.bevy_to_api.remove(&entity) {
            self.api_to_bevy.remove(&id);
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
    for (entity, id) in q_added.iter() {
        registry.assign(entity, *id);
    }
    for entity in q_removed.read() {
        registry.remove(entity);
    }
}

/// Plugin that registers the entity registry and synchronization system.
pub struct ApiEntityRegistryPlugin;

impl Plugin for ApiEntityRegistryPlugin {
    fn build(&self, app: &mut App) {
        eprintln!("[lunco-api] Registering ApiEntityRegistryPlugin");
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
        let id = GlobalEntityId::new();
        registry.assign(entity, id);
        assert_eq!(registry.resolve(&id), Some(entity));
    }

    #[test]
    fn test_remove() {
        let mut registry = ApiEntityRegistry::default();
        let entity = Entity::PLACEHOLDER;
        let id = GlobalEntityId::new();
        registry.assign(entity, id);
        registry.remove(entity);
        assert_eq!(registry.resolve(&id), None);
    }
}
