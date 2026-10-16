//! Explicit callback endpoints with an ECS lifetime and scene ownership.

use bevy::prelude::*;

/// A callback lease. Its owner despawns it on replacement or teardown; queued
/// tool invocations retain the exact entity and endpoint, never a name-only lookup.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct ToolHookBinding {
    /// Registered tool namespace.
    pub tool: String,
    /// Registered one-argument hook.
    pub hook: String,
    /// Twin admitted by the UI owner, when Twin-scoped.
    pub owner_twin_id: Option<u64>,
    /// Scene transaction admitted when this binding was created.
    pub scene_generation: Option<u64>,
    /// Registration owner pinned at binding creation.
    pub tool_scope: lunco_tools::ToolScope,
}

impl ToolHookBinding {
    /// Bind a registered endpoint to one Twin/scene lifetime.
    pub fn new(
        tool: String,
        hook: String,
        owner_twin_id: Option<u64>,
        scene: Option<&lunco_core::SceneTransitionCoordinator>,
    ) -> Result<Self, String> {
        if owner_twin_id == Some(0) {
            return Err("UI callback Twin owner must be non-zero".into());
        }
        let tool_scope = lunco_tools::resolve_hook(&tool, &hook)?;
        if let lunco_tools::ToolScope::Twin(id) = &tool_scope {
            if id.parse::<u64>().ok() != owner_twin_id {
                return Err("UI callback tool does not belong to the publishing Twin".into());
            }
        }
        let binding = Self {
            tool,
            hook,
            owner_twin_id,
            scene_generation: scene.and_then(|s| s.completed_generation()),
            tool_scope,
        };
        binding.validate_scene(scene)?;
        Ok(binding)
    }

    /// Reject callbacks while replacing the scene or after its generation changes.
    pub fn validate_scene(
        &self,
        scene: Option<&lunco_core::SceneTransitionCoordinator>,
    ) -> Result<(), String> {
        lunco_core::scene::validate_interaction_scene(self.scene_generation, scene)
    }
}
