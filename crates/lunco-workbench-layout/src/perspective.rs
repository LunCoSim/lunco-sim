//! Concrete-shell perspective integration.

use crate::layout::WorkbenchLayout;
use bevy::prelude::{Res, ResMut};

impl WorkbenchLayout {
    /// Resolve the active perspective's scene-click owner for the cross-crate
    /// input gate. No active perspective leaves the simulation default intact.
    pub fn active_scene_interaction_mode(&self) -> lunco_interaction_core::SceneInteractionMode {
        self.active_perspective
            .and_then(|active| self.perspectives.iter().find(|p| p.id() == active))
            .map_or(
                lunco_interaction_core::SceneInteractionMode::Simulation,
                |perspective| perspective.scene_interaction_mode(),
            )
    }

    /// Whether the active perspective owns a full-window scene that should
    /// remain behind transient dock chrome even without a viewport panel tab.
    pub fn active_perspective_scene_visible_when_docked(&self) -> bool {
        self.active_perspective
            .and_then(|active| self.perspectives.iter().find(|p| p.id() == active))
            .is_some_and(|perspective| perspective.scene_visible_when_docked())
    }
}

/// Publish the active perspective's scene-click owner to the shared core
/// resource consumed by the selection and possession observers.
pub fn sync_scene_interaction_mode(
    layout: Res<WorkbenchLayout>,
    mut mode: ResMut<lunco_interaction_core::SceneInteractionMode>,
) {
    let next = layout.active_scene_interaction_mode();
    if *mode != next {
        *mode = next;
    }
}
