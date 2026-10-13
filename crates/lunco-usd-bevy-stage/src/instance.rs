//! Generic identity scope for projected USD instance subtrees.
//!
//! Catalog mounts retain hierarchical instance identity. Native reference
//! preparation retains its source revision and admitted composed facts at the
//! headless stage boundary.

use std::sync::Arc;

use bevy::prelude::Component;

use crate::UsdStageProjectionPlan;

/// Immutable canonical reference facts retained by native layer admission.
/// The admission owner fences the actual source revision and live mount before
/// publishing this recipe and composed plan.
#[derive(Debug)]
pub struct UsdReferenceSnapshot {
    pub recipe: Arc<lunco_usd_compose::recipe::StageRecipe>,
    pub plan: Arc<UsdStageProjectionPlan>,
}

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
