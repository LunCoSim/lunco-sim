//! Registration point for domain-owned live USD edit handling.
//!
//! The composed-stage projector is generic, but a live attribute can belong to
//! a domain-specific in-place refresh instead of ordinary subtree projection.
//! Owners also identify edits whose derived topology crosses a prim subtree and
//! retire that state before a stage-wide reset. The runtime snapshots the small
//! handler table before mutating the world, keeping the generic projector
//! independent of specialized USD domains.

use bevy::asset::AssetId;
use bevy::ecs::entity::Entity;
use bevy::ecs::resource::Resource;
use bevy::ecs::world::World;
use openusd::sdf::Path as SdfPath;

use lunco_usd_bevy_stage::UsdStageAsset;
use lunco_usd_bevy_stage::read::UsdReadObject;

/// One specialized owner of a live USD edit surface.
///
/// The owner decides whether an attribute is handled in place, can invalidate
/// its own one-shot projection marker when a composed schema becomes available,
/// and refreshes the affected prims after a complete native notice batch. It also promotes local
/// subtree resets when its topology crosses that boundary and prepares its
/// derived state before a full stage reset. Reset preparation is synchronous; an
/// error aborts entity replacement. Keeping handlers as function pointers makes
/// the registry cheap to snapshot and keeps the generic live projector
/// independent of domain crates.
#[derive(Clone, Copy)]
pub struct UsdLiveEditOwner {
    id: &'static str,
    claims_edit: Option<fn(&dyn UsdReadObject, &SdfPath, &str) -> bool>,
    invalidates_projection: Option<fn(&mut World, Entity) -> bool>,
    refresh_prims: Option<fn(&mut World, AssetId<UsdStageAsset>, &[SdfPath])>,
    requires_stage_projection_reset: fn(&World, AssetId<UsdStageAsset>, &str) -> bool,
    prepare_stage_projection_reset: fn(&mut World, AssetId<UsdStageAsset>) -> Result<(), String>,
}

impl UsdLiveEditOwner {
    /// Declare one live-edit owner under a stable diagnostic identifier.
    pub const fn new(
        id: &'static str,
        claims_edit: fn(&dyn UsdReadObject, &SdfPath, &str) -> bool,
        invalidates_projection: Option<fn(&mut World, Entity) -> bool>,
        refresh_prims: fn(&mut World, AssetId<UsdStageAsset>, &[SdfPath]),
    ) -> Self {
        Self {
            id,
            claims_edit: Some(claims_edit),
            invalidates_projection,
            refresh_prims: Some(refresh_prims),
            requires_stage_projection_reset: never_requires_stage_projection_reset,
            prepare_stage_projection_reset: no_stage_projection_reset,
        }
    }

    /// Register a domain owner that participates in stage rebuilds without
    /// claiming ordinary in-place live edits.
    pub const fn stage_projection_reset_owner(
        id: &'static str,
        requires_stage_projection_reset: fn(&World, AssetId<UsdStageAsset>, &str) -> bool,
        prepare_stage_projection_reset: fn(
            &mut World,
            AssetId<UsdStageAsset>,
        ) -> Result<(), String>,
    ) -> Self {
        Self {
            id,
            claims_edit: None,
            invalidates_projection: None,
            refresh_prims: None,
            requires_stage_projection_reset,
            prepare_stage_projection_reset,
        }
    }

    /// Stable owner identifier, used to prevent duplicate registration.
    pub const fn id(self) -> &'static str {
        self.id
    }

    /// Test whether this owner handles one composed attribute.
    pub fn claims_edit(self, reader: &dyn UsdReadObject, prim: &SdfPath, attribute: &str) -> bool {
        self.claims_edit
            .is_some_and(|claims| claims(reader, prim, attribute))
    }

    /// Give this owner a chance to invalidate its projection marker.
    pub fn invalidates_projection(self, world: &mut World, entity: Entity) -> bool {
        self.invalidates_projection
            .is_some_and(|invalidate| invalidate(world, entity))
    }

    /// Refresh affected prims once after the complete stage batch is authored.
    /// Paths are unique and ordered by native USD identity.
    pub fn refresh_prims(
        self,
        world: &mut World,
        stage: AssetId<UsdStageAsset>,
        prims: &[SdfPath],
    ) {
        if let Some(refresh) = self.refresh_prims {
            refresh(world, stage, prims);
        }
    }

    /// Whether reprojecting this prim subtree would cut across this owner's
    /// derived stage topology and therefore requires one stage-wide reset.
    pub fn requires_stage_projection_reset(
        self,
        world: &World,
        stage: AssetId<UsdStageAsset>,
        prim_path: &str,
    ) -> bool {
        (self.requires_stage_projection_reset)(world, stage, prim_path)
    }

    /// Retire this owner's stage-derived state before the generic projector
    /// replaces every ECS entity belonging to the stage.
    pub fn prepare_stage_projection_reset(
        self,
        world: &mut World,
        stage: AssetId<UsdStageAsset>,
    ) -> Result<(), String> {
        (self.prepare_stage_projection_reset)(world, stage)
    }
}

fn never_requires_stage_projection_reset(
    _world: &World,
    _stage: AssetId<UsdStageAsset>,
    _prim_path: &str,
) -> bool {
    false
}

fn no_stage_projection_reset(
    _world: &mut World,
    _stage: AssetId<UsdStageAsset>,
) -> Result<(), String> {
    Ok(())
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

    fn refresh(_world: &mut World, _stage: AssetId<UsdStageAsset>, _prims: &[SdfPath]) {}

    fn requires_reset(_world: &World, _stage: AssetId<UsdStageAsset>, _path: &str) -> bool {
        true
    }

    fn reject_reset(_world: &mut World, _stage: AssetId<UsdStageAsset>) -> Result<(), String> {
        Err("reset rejected".to_owned())
    }

    #[test]
    fn registry_rejects_duplicate_owner_ids() {
        let owner = UsdLiveEditOwner::new("test.owner", claims, None, refresh);
        let mut registry = UsdLiveEditRegistry::default();

        assert!(registry.register(owner));
        assert!(!registry.register(owner));
        assert_eq!(registry.snapshot().len(), 1);
    }

    #[test]
    fn stage_reset_owner_exposes_promotion_and_propagates_rejection() {
        let owner = UsdLiveEditOwner::stage_projection_reset_owner(
            "test.stage-reset",
            requires_reset,
            reject_reset,
        );
        let world = World::new();
        let mut mutable_world = World::new();
        let stage = AssetId::<UsdStageAsset>::default();

        assert!(owner.requires_stage_projection_reset(&world, stage, "/Scene/Body"));
        assert_eq!(
            owner.prepare_stage_projection_reset(&mut mutable_world, stage),
            Err("reset rejected".to_owned())
        );
    }
}
