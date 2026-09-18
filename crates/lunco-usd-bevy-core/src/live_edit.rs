//! Registration point for domain-owned live USD edit handling.
//!
//! The composed-stage projector is generic, but a live attribute can belong to
//! a domain-specific in-place refresh instead of ordinary subtree projection.
//! Owners register that decision here. The runtime snapshots the small handler
//! table before mutating the world, so handlers can safely perform their own
//! refresh without creating a dependency from the generic projector to every
//! specialized USD domain.

use bevy::asset::AssetId;
use bevy::ecs::entity::Entity;
use bevy::ecs::resource::Resource;
use bevy::ecs::world::World;
use openusd::sdf::Path as SdfPath;

use lunco_usd_bevy_stage::read::UsdReadObject;
use lunco_usd_bevy_stage::UsdStageAsset;

/// One specialized owner of a live USD edit surface.
///
/// The owner decides whether an attribute is handled in place, can invalidate
/// its own one-shot projection marker when a composed schema becomes available,
/// and refreshes the affected stage after a claimed edit. Keeping these as
/// function pointers makes the registry cheap to snapshot and keeps the
/// generic live projector independent of domain crates.
#[derive(Clone, Copy)]
pub struct UsdLiveEditOwner {
    id: &'static str,
    claims_edit: fn(&dyn UsdReadObject, &SdfPath, &str) -> bool,
    invalidates_projection: fn(&mut World, Entity) -> bool,
    refresh_stage: fn(&mut World, AssetId<UsdStageAsset>),
}

impl UsdLiveEditOwner {
    /// Declare one live-edit owner under a stable diagnostic identifier.
    pub const fn new(
        id: &'static str,
        claims_edit: fn(&dyn UsdReadObject, &SdfPath, &str) -> bool,
        invalidates_projection: fn(&mut World, Entity) -> bool,
        refresh_stage: fn(&mut World, AssetId<UsdStageAsset>),
    ) -> Self {
        Self {
            id,
            claims_edit,
            invalidates_projection,
            refresh_stage,
        }
    }

    /// Stable owner identifier, used to prevent duplicate registration.
    pub const fn id(self) -> &'static str {
        self.id
    }

    /// Test whether this owner handles one composed attribute.
    pub fn claims_edit(self, reader: &dyn UsdReadObject, prim: &SdfPath, attribute: &str) -> bool {
        (self.claims_edit)(reader, prim, attribute)
    }

    /// Give this owner a chance to invalidate its projection marker.
    pub fn invalidates_projection(self, world: &mut World, entity: Entity) -> bool {
        (self.invalidates_projection)(world, entity)
    }

    /// Refresh the stage after one or more edits claimed by this owner.
    pub fn refresh_stage(self, world: &mut World, stage: AssetId<UsdStageAsset>) {
        (self.refresh_stage)(world, stage);
    }
}

/// Domain handlers for incremental live USD projection.
#[derive(Resource, Default)]
pub struct UsdLiveEditRegistry {
    owners: Vec<UsdLiveEditOwner>,
}

impl UsdLiveEditRegistry {
    /// Register an owner once. A duplicate stable id is rejected so two
    /// plugins cannot silently compete for the same live-edit surface.
    pub fn register(&mut self, owner: UsdLiveEditOwner) -> bool {
        if self
            .owners
            .iter()
            .any(|registered| registered.id() == owner.id())
        {
            return false;
        }
        self.owners.push(owner);
        true
    }

    /// Snapshot the registered owners before a caller mutates the [`World`].
    pub fn snapshot(&self) -> Vec<UsdLiveEditOwner> {
        self.owners.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(_reader: &dyn UsdReadObject, _prim: &SdfPath, _attribute: &str) -> bool {
        true
    }

    fn never_invalidates(_world: &mut World, _entity: Entity) -> bool {
        false
    }

    fn refresh(_world: &mut World, _stage: AssetId<UsdStageAsset>) {}

    #[test]
    fn registry_rejects_duplicate_owner_ids() {
        let owner = UsdLiveEditOwner::new("test.owner", claims, never_invalidates, refresh);
        let mut registry = UsdLiveEditRegistry::default();

        assert!(registry.register(owner));
        assert!(!registry.register(owner));
        assert_eq!(registry.snapshot().len(), 1);
    }
}
