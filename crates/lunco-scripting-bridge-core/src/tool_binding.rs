//! Explicit callback endpoints with an ECS lifetime and scene ownership.

use bevy::prelude::*;

/// The owner of a callback lease after it has been selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolHookBindingRetention {
    /// The publishing surface retains the lease until replacement or teardown.
    Surface,
    /// Selection transfers the lease to one queued invocation, which retires it.
    Invocation,
}

/// A callback lease retired by its surface or selected invocation. Queued tool
/// work retains the exact entity and endpoint, never a name-only lookup.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct ToolHookBinding {
    /// Whether the publishing surface or the selected invocation owns retirement.
    pub retention: ToolHookBindingRetention,
    /// Registered tool namespace.
    pub tool: String,
    /// Registered one-argument hook.
    pub hook: String,
    /// Twin admitted by the UI owner, when Twin-scoped.
    pub owner_twin_id: Option<u64>,
    /// Publication lifetime owned by the installing UI surface.
    pub lifetime: lunco_core::InteractionLifetime,
    /// Registration owner pinned at binding creation.
    pub tool_scope: lunco_tools::ToolScope,
}

impl ToolHookBinding {
    /// Bind a registered endpoint to one Twin/scene lifetime.
    pub fn for_scene(
        tool: String,
        hook: String,
        owner_twin_id: Option<u64>,
        retention: ToolHookBindingRetention,
        scene: Option<&lunco_core::SceneTransitionCoordinator>,
    ) -> Result<Self, String> {
        let binding = Self::create(
            tool,
            hook,
            owner_twin_id,
            retention,
            lunco_core::InteractionLifetime::Scene(scene.and_then(|s| s.lifecycle_generation())),
        )?;
        binding.validate_lifetime(scene)?;
        Ok(binding)
    }

    /// Bind an owner-scoped endpoint independently of scene readiness.
    pub fn for_owner(
        tool: String,
        hook: String,
        owner_twin_id: Option<u64>,
        retention: ToolHookBindingRetention,
    ) -> Result<Self, String> {
        Self::create(
            tool,
            hook,
            owner_twin_id,
            retention,
            lunco_core::InteractionLifetime::Owner,
        )
    }

    fn create(
        tool: String,
        hook: String,
        owner_twin_id: Option<u64>,
        retention: ToolHookBindingRetention,
        lifetime: lunco_core::InteractionLifetime,
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
        Ok(Self {
            retention,
            tool,
            hook,
            owner_twin_id,
            lifetime,
            tool_scope,
        })
    }

    /// Transfer a retained owner endpoint to one invocation.
    /// The host parents this lease to the installed endpoint so replacement
    /// also revokes work that has not reached the execution queue yet.
    pub fn select_for_owner(&self, owner_twin_id: Option<u64>) -> Result<Self, String> {
        if self.lifetime != lunco_core::InteractionLifetime::Owner
            || self.retention != ToolHookBindingRetention::Surface
        {
            return Err("only retained owner endpoints create selected invocations".into());
        }
        if owner_twin_id == Some(0)
            || self
                .owner_twin_id
                .is_some_and(|owner| Some(owner) != owner_twin_id)
        {
            return Err("menu selection does not belong to its publishing Twin".into());
        }
        Ok(Self {
            retention: ToolHookBindingRetention::Invocation,
            lifetime: lunco_core::InteractionLifetime::Owner,
            owner_twin_id,
            ..self.clone()
        })
    }

    /// Release an exact one-shot lease on every terminal admission/queue path.
    pub fn retire_invocation(&self, entity: Entity, world: &mut World) {
        if self.retention == ToolHookBindingRetention::Invocation
            && world.get::<Self>(entity) == Some(self)
        {
            world.despawn(entity);
        }
    }

    /// Validate target lifetime without requiring simulation readiness.
    pub fn validate_lifetime(
        &self,
        scene: Option<&lunco_core::SceneTransitionCoordinator>,
    ) -> Result<(), String> {
        self.lifetime.validate(scene)
    }
}
