//! Generic identity scope for projected USD instance subtrees.
//!
//! Runtime-spawned references reuse the source asset's composed prim paths.
//! These markers and the prepared plan therefore belong to the headless stage
//! boundary; visual, physics, and simulation projections can all consume them.

use std::sync::Arc;

use bevy::prelude::Component;

use crate::UsdStageProjectionPlan;

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
#[derive(Component, Debug, Clone)]
pub struct UsdInstanceProjection {
    /// The runtime instance root whose identity scopes this projection.
    pub root: Option<bevy::ecs::entity::Entity>,
    /// The source asset's prepared plan, remapped to the instance namespace.
    pub plan: Arc<UsdStageProjectionPlan>,
}
