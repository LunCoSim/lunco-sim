//! Generic identity scope for projected USD instance subtrees.
//!
//! Runtime-spawned references reuse the source asset's composed prim paths.
//! These markers and the prepared plan therefore belong to the headless stage
//! boundary; visual, physics, and simulation projections can all consume them.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::asset::Handle;
use bevy::prelude::Component;

use crate::{UsdStageAsset, UsdStageProjectionPlan};

/// Seed marker for a runtime-spawned USD instance root.
#[derive(Component, Debug, Clone, Copy)]
pub struct UsdInstanceRoot;

/// Propagated marker identifying a descendant of a runtime-spawned instance.
#[derive(Component, Debug, Clone)]
pub struct UsdInstanceMember {
    /// The instance-root entity this member descends from.
    pub root: bevy::ecs::entity::Entity,
    /// The instance root's composed prim path.
    pub root_path: String,
}

/// Prepared composed read data and identity scope for a referenced instance.
/// Spawn and deletion use this plan directly; the first later authored edit
/// composes the reference into the live stage and switches every clone to it.
#[derive(Component, Debug, Clone)]
pub struct UsdInstanceProjection {
    /// The runtime instance root whose identity scopes this projection.
    pub root: Option<bevy::ecs::entity::Entity>,
    /// The prepared source asset retained for as long as this instance is live.
    /// Sibling instances can then reuse its loaded recipe and immutable plan.
    pub source_asset: Handle<UsdStageAsset>,
    /// Exact admitted composition identity and closure retained with the live
    /// instance. The transport asset may use a different USD root identity.
    pub reference_recipe: Arc<lunco_usd_compose::recipe::StageRecipe>,
    /// An instance-scoped view over the source asset's shared prepared plan.
    pub plan: Arc<UsdStageProjectionPlan>,
    /// Exact authored asset identity, retained for promotion into the live
    /// canonical stage when the instance first receives a live edit.
    pub asset_path: String,
    /// Explicit prim target from the authored reference arc, when present.
    pub reference_prim_path: Option<String>,
    /// The instance root's authored composed type name.
    pub type_name: Option<String>,
    /// Shared state keeps every descendant on the same prepared/live read
    /// boundary without walking the instance subtree during promotion.
    promoted: Arc<AtomicBool>,
}

impl UsdInstanceProjection {
    /// Build one prepared projection for a referenced instance.
    pub fn new(
        source_asset: Handle<UsdStageAsset>,
        reference_recipe: Arc<lunco_usd_compose::recipe::StageRecipe>,
        plan: Arc<UsdStageProjectionPlan>,
        asset_path: impl Into<String>,
        reference_prim_path: Option<String>,
        type_name: Option<String>,
    ) -> Self {
        Self {
            root: None,
            source_asset,
            reference_recipe,
            plan,
            asset_path: asset_path.into(),
            reference_prim_path,
            type_name,
            promoted: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether the instance has been authored into its scene's live stage.
    pub fn is_promoted(&self) -> bool {
        self.promoted.load(Ordering::Acquire)
    }

    /// Mark this reference instance as owned by the live canonical stage.
    pub fn mark_promoted(&self) {
        self.promoted.store(true, Ordering::Release);
    }
}
