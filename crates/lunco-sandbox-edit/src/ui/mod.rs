//! UI for the sandbox editing tools.
//!
//! All UI lives here. Panels are pure presentation — they query state
//! and emit commands. They never mutate domain state directly (except for
//! UI-local state like SpawnState and SelectedEntity).

use bevy::prelude::*;
use bevy_workbench::WorkbenchApp;

pub mod spawn_palette;
pub mod inspector;
pub mod entity_list;

/// Plugin that registers all sandbox editing UI panels.
pub struct SandboxEditUiPlugin;

impl Plugin for SandboxEditUiPlugin {
    fn build(&self, app: &mut App) {
        app.register_panel(spawn_palette::SpawnPalette);
        app.register_panel(inspector::Inspector);
        app.register_panel(entity_list::EntityList);
    }
}
