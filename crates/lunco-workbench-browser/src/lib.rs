//! Focused Twin files, library and specialist browser views.
//!
//! The concrete workbench shell owns docking, source editing, and viewport
//! presentation. This package owns the reusable browser feature. Dataset
//! controls are provided by the optional `lunco-workbench-datasets-ui`
//! package so hosts that only need browsing do not link the provisioning and
//! processing closure from `lunco-assets`.

use bevy::prelude::*;
use lunco_workbench_core::WorkbenchPanelAppExt;
use lunco_workbench_core::commands::FocusPanel;

pub mod navigation;
pub mod twin_browser;

pub use twin_browser::{
    BrowserAction, BrowserActions, BrowserCtx, BrowserQuery, BrowserSection,
    BrowserSectionRegistry, FilesSection, LuncoLibrarySection, TWIN_BROWSER_PANEL_ID,
    TwinBrowserPanel, UnsavedDocEntry, UnsavedDocs,
};

/// Install the reusable Twin and Files browser feature.
///
/// The plugin provides the generic browser resources, built-in file/library
/// sections, and their panels. Domain UI plugins add their own
/// [`BrowserSection`] implementations after installing this plugin.
pub struct TwinBrowserPlugin;

impl Plugin for TwinBrowserPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BrowserSectionRegistry>()
            .init_resource::<BrowserActions>()
            .init_resource::<BrowserQuery>()
            .register_type::<BrowserQuery>()
            .init_resource::<UnsavedDocs>()
            .add_observer(twin_browser::clear_browser_state_on_twin_closed)
            .add_systems(Update, drain_browser_actions);
        app.init_resource::<twin_browser::files_section::FileTreeView>()
            .add_systems(Update, twin_browser::files_section::prepare_file_tree)
            .add_observer(twin_browser::files_section::retire_file_tree);
        navigation::install(app);

        app.register_panel(TwinBrowserPanel);
        let mut sections = app.world_mut().resource_mut::<BrowserSectionRegistry>();
        sections.register(FilesSection::default());
        sections.register(LuncoLibrarySection::default());
    }
}

/// Dispatch browser-owned actions after the UI render pass.
fn drain_browser_actions(world: &mut World) {
    let actions = {
        let Some(mut outbox) = world.get_resource_mut::<BrowserActions>() else {
            return;
        };
        outbox.take_where(|action| {
            matches!(
                action,
                BrowserAction::OpenPanel { .. } | BrowserAction::OpenTwinSource { .. }
            )
        })
    };

    for action in actions {
        match action {
            BrowserAction::OpenPanel { id } => world.trigger(FocusPanel { id }),
            BrowserAction::OpenTwinSource {
                twin_root,
                relative_path,
                pinned,
            } => lunco_workbench_core::trigger_command_with_origin(
                world,
                lunco_workbench_core::source::OpenTwinSource {
                    twin_root: twin_root.to_string_lossy().into_owned(),
                    relative_path: relative_path.to_string_lossy().into_owned(),
                    pinned,
                    focus: None,
                },
                lunco_core::CommandOrigin::LocalUser {
                    session_id: lunco_command_contracts::SessionId::LOCAL,
                },
            ),
            _ => {}
        }
    }
}
