//! Commands whose payloads address the workbench contract.
//!
//! The concrete shell owns the observers that apply these commands to its
//! layout. Keeping the payloads here lets headless adapters and network
//! synchronization carry the same typed contract without depending on the
//! egui docking implementation.

use bevy::ecs::reflect::ReflectEvent;
use bevy::prelude::Event;
use bevy::reflect::std_traits::ReflectDefault;
use lunco_core::Command;

use crate::{PanelId, TabId};

/// Request the workbench open (or focus) a multi-instance tab.
///
/// The concrete shell owns the observer that applies this event to its dock.
/// Domain UI crates can therefore open a tab without depending on egui-dock.
#[derive(Event, Clone, Copy, Debug)]
pub struct OpenTab {
    /// The [`crate::InstancePanel::kind`] to open.
    pub kind: PanelId,
    /// The tab's instance discriminant, typically a document id.
    pub instance: u64,
}

/// Request opening a multi-instance tab while preserving the currently
/// focused tab.
#[derive(Event, Clone, Copy, Debug)]
pub struct OpenTabPreserveFocus {
    /// The [`crate::InstancePanel::kind`] to open.
    pub kind: PanelId,
    /// The tab's instance discriminant.
    pub instance: u64,
    /// Explicit tab to restore when the caller has a more precise focus source.
    pub restore: Option<TabId>,
}

/// Request the workbench close a multi-instance tab, if open.
#[derive(Event, Clone, Copy, Debug)]
pub struct CloseTab {
    /// The [`crate::InstancePanel::kind`] to close.
    pub kind: PanelId,
    /// The tab's instance discriminant, typically a document id.
    pub instance: u64,
}

/// Bring a registered singleton panel forward in the concrete shell.
#[Command(default)]
pub struct FocusPanel {
    /// The singleton panel's stable id.
    pub id: String,
}

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
