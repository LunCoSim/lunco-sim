//! Entity identity registry — maps stable ULID-based API IDs to Bevy Entity.

use std::collections::HashMap;
use bevy::prelude::*;
use ulid::Ulid;
use crate::schema::ApiEntityId;

/// Marker component assigned by the registry to entities with API identity.
#[derive(Component)]
pub struct ApiIdentity { pub id: ApiEntityId }

/// Bidirectional mapping between API entity IDs and Bevy entities.
#[derive(Resource, Default)]
pub struct ApiEntityRegistry {
    api_to_bevy: HashMap<ApiEntityId, Entity>,
    bevy_to_api: HashMap<Entity, ApiEntityId>,
}

impl ApiEntityRegistry {
    fn next_ulid(&mut self) -> Ulid {
        Ulid::new()
    }

    pub fn assign(&mut self, entity: Entity) -> ApiEntityId {
        if let Some(&id) = self.bevy_to_api.get(&entity) { return id; }
        let id = ApiEntityId(self.next_ulid());
        self.api_to_bevy.insert(id, entity);
        self.bevy_to_api.insert(entity, id);
        id
    }

    pub fn remove(&mut self, entity: Entity) {
        if let Some(id) = self.bevy_to_api.remove(&entity) {
            self.api_to_bevy.remove(&id);
        }
    }

    pub fn resolve(&self, id: &ApiEntityId) -> Option<Entity> {
        self.api_to_bevy.get(id).copied()
    }

    pub fn api_id_for(&self, entity: Entity) -> Option<ApiEntityId> {
        self.bevy_to_api.get(&entity).copied()
    }

    pub fn entities(&self) -> impl Iterator<Item = (ApiEntityId, Entity)> + '_ {
        self.api_to_bevy.iter().map(|(&id, &entity)| (id, entity))
    }
}

/// System that assigns API identities to entities.
///
/// Only assigns to root entities (no ChildOf) to avoid assigning to children
/// like wheels, visuals, etc.
pub fn assign_api_id(
    mut commands: Commands,
    mut registry: ResMut<ApiEntityRegistry>,
    q_new: Query<Entity, (Without<ApiIdentity>, Without<ChildOf>)>,
) {
    let count = q_new.iter().count();
    if count > 0 {
        eprintln!("[lunco-api] assign_api_id: found {} new root entities", count);
    }
    for entity in q_new.iter() {
        let id = registry.assign(entity);
        commands.entity(entity).insert(ApiIdentity { id });
    }
}

/// System that removes API identities from despawned entities.
pub fn cleanup_api_id(
    mut registry: ResMut<ApiEntityRegistry>,
    mut q_removed: RemovedComponents<ApiIdentity>,
) {
    for entity in q_removed.read() {
        registry.remove(entity);
    }
}

/// Plugin that registers the entity registry and lifecycle systems.
pub struct ApiEntityRegistryPlugin;

impl Plugin for ApiEntityRegistryPlugin {
    fn build(&self, app: &mut App) {
        eprintln!("[lunco-api] Registering ApiEntityRegistryPlugin");
        app.init_resource::<ApiEntityRegistry>()
            .add_systems(Update, (assign_api_id, cleanup_api_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assign_and_resolve() {
        let mut registry = ApiEntityRegistry::default();
        let entity = Entity::PLACEHOLDER;
        let id = registry.assign(entity);
        assert_eq!(registry.resolve(&id), Some(entity));
    }

    #[test]
    fn test_remove() {
        let mut registry = ApiEntityRegistry::default();
        let entity = Entity::PLACEHOLDER;
        let id = registry.assign(entity);
        registry.remove(entity);
        assert_eq!(registry.resolve(&id), None);
    }

    #[test]
    fn test_idempotent_assign() {
        let mut registry = ApiEntityRegistry::default();
        let entity = Entity::PLACEHOLDER;
        let id1 = registry.assign(entity);
        let id2 = registry.assign(entity);
        assert_eq!(id1, id2);
    }
}
