//! Commands whose payloads address the workbench contract.
//!
//! The concrete shell owns the observers that apply these commands to its
//! layout. Keeping the payloads here lets headless adapters and network
//! synchronization carry the same typed contract without depending on the
//! egui docking implementation.

use bevy::ecs::reflect::ReflectEvent;
use bevy::reflect::std_traits::ReflectDefault;
use lunco_core::Command;

/// Activate a registered perspective by its stable identifier.
#[Command(default)]
pub struct ActivatePerspective {
    /// The identifier of the perspective to activate.
    pub id: String,
}

/// Reset the concrete workbench layout to the active perspective preset.
#[Command(default)]
pub struct ResetWorkspaceLayout {}

/// Constrain the shell to an authored perspective, or release the constraint.
#[Command(default)]
pub struct SetRequiredPerspective {
    /// Raw perspective identifier, or `None` to release the constraint.
    pub id: Option<String>,
}

/// Reset the shell to its required or first registered perspective.
#[Command(default)]
pub struct ResetToDefaultPerspective {}
