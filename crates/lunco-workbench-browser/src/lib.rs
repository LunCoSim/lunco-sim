//! Twin and Files browser panels for the LunCoSim workbench.
//!
//! The concrete workbench shell owns docking, source editing, and viewport
//! presentation. This package owns the reusable browser feature. Dataset
//! controls are provided by the optional `lunco-workbench-datasets-ui`
//! package so hosts that only need browsing do not link the provisioning and
//! processing closure from `lunco-assets`.


use bevy::prelude::*;
use lunco_workbench_core::commands::FocusPanel;
use lunco_workbench_core::WorkbenchPanelAppExt;

mod files_panel;
pub mod twin_browser;

pub use files_panel::{FilesPanel, FILES_PANEL_ID};
pub use twin_browser::{
    BrowserAction, BrowserActions, BrowserCtx, BrowserQuery, BrowserScope, BrowserSection,
    BrowserSectionRegistry, FilesSection, LuncoLibrarySection, TwinBrowserPanel, UnsavedDocEntry,
    UnsavedDocs, TWIN_BROWSER_PANEL_ID,
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
            .init_resource::<UnsavedDocs>()
            .add_observer(twin_browser::clear_browser_state_on_twin_closed)
            .add_systems(Update, drain_browser_navigation);

        app.register_panel(TwinBrowserPanel)
            .register_panel(FilesPanel);
        let mut sections = app.world_mut().resource_mut::<BrowserSectionRegistry>();
        sections.register(FilesSection::default());
        sections.register(LuncoLibrarySection::default());
    }
}

/// Turn browser panel-navigation actions into the workbench's typed focus
/// command. The browser never reaches into the shell's private dock state.
fn drain_browser_navigation(world: &mut World) {
    let actions = {
        let Some(mut outbox) = world.get_resource_mut::<BrowserActions>() else {
            return;
        };
        outbox.take_where(|action| matches!(action, BrowserAction::OpenPanel { .. }))
    };

    for action in actions {
        let BrowserAction::OpenPanel { id } = action else {
            continue;
        };
        world.trigger(FocusPanel { id });
    }
}
