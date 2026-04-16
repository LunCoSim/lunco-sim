//! UI for the sandbox editing tools.
//!
//! All UI lives here. Panels are pure presentation — they query state
//! and emit commands. They never mutate domain state directly (except for
//! UI-local state like SpawnState and SelectedEntity).

use bevy::prelude::*;
use lunco_workbench::{PanelId, Workspace, WorkspaceId, WorkbenchAppExt, WorkbenchLayout};

pub mod spawn_palette;
pub mod inspector;
pub mod entity_list;

/// Plugin that registers all sandbox editing UI panels and the default
/// Build workspace preset.
pub struct SandboxEditUiPlugin;

impl Plugin for SandboxEditUiPlugin {
    fn build(&self, app: &mut App) {
        app.register_panel(spawn_palette::SpawnPalette)
            .register_panel(inspector::Inspector)
            .register_panel(entity_list::EntityList)
            .register_workspace(BuildWorkspace);
    }
}

/// Rover sandbox's default workspace preset.
///
/// Mirrors the "Build" layout from the workbench design doc
/// ([`docs/architecture/11-workbench.md`] § 4):
/// entity list on the left, inspector on the right, spawn palette in
/// the bottom dock, and **no Center panel** — the 3D world shows
/// through the central region.
pub struct BuildWorkspace;

impl Workspace for BuildWorkspace {
    fn id(&self) -> WorkspaceId { WorkspaceId("rover_build") }
    fn title(&self) -> String { "🏗 Build".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        layout.set_side_browser(Some(PanelId("entity_list")));
        layout.set_center(vec![]); // empty Center → 3D viewport shows through
        layout.set_right_inspector(Some(PanelId("sandbox_inspector")));
        layout.set_bottom(Some(PanelId("spawn_palette")));
    }
}
