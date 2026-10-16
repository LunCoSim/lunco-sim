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

/// Publication lifetime, independent of the scene captured by a selected call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolHookBindingLifetime {
    /// HUDs and context popups belong to one committed scene transaction.
    Scene(Option<u64>),
    /// Installed menu contributions live until provider replacement or Twin close.
    Provider,
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
    pub lifetime: ToolHookBindingLifetime,
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
            ToolHookBindingLifetime::Scene(scene.and_then(|s| s.completed_generation())),
        )?;
        binding.validate_scene(scene)?;
        Ok(binding)
    }

    /// Bind an installed provider independently of scene resets. Each selected
    /// invocation still captures and validates its own scene transaction.
    pub fn for_provider(
        tool: String,
        hook: String,
        owner_twin_id: Option<u64>,
    ) -> Result<Self, String> {
        Self::create(
            tool,
            hook,
            owner_twin_id,
            ToolHookBindingRetention::Surface,
            ToolHookBindingLifetime::Provider,
        )
    }

    fn create(
        tool: String,
        hook: String,
        owner_twin_id: Option<u64>,
        retention: ToolHookBindingRetention,
        lifetime: ToolHookBindingLifetime,
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

    /// Transfer a selected provider endpoint to a scene-bound invocation.
    /// The host parents this lease to the installed endpoint so replacement
    /// also revokes work that has not reached the execution queue yet.
    pub fn select_for_scene(
        &self,
        owner_twin_id: Option<u64>,
        generation: Option<u64>,
    ) -> Result<Self, String> {
        if self.lifetime != ToolHookBindingLifetime::Provider {
            return Err(
                "only installed provider endpoints create selected scene invocations".into(),
            );
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
            lifetime: ToolHookBindingLifetime::Scene(generation),
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

    /// Reject callbacks while replacing the scene or after its generation changes.
    pub fn validate_scene(
        &self,
        scene: Option<&lunco_core::SceneTransitionCoordinator>,
    ) -> Result<(), String> {
        match self.lifetime {
            ToolHookBindingLifetime::Scene(generation) => {
                lunco_core::scene::validate_interaction_scene(generation, scene)
            }
            ToolHookBindingLifetime::Provider => Ok(()),
        }
    }
}
