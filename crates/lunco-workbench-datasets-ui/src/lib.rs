//! Optional Twin dataset controls for the workbench browser.
//!
//! The generic browser remains independent from dataset provisioning. Hosts
//! that install [`TwinDatasetsPlugin`] opt into the `lunco-assets` processing
//! stack and get the active Twin's declared resources as a browser section.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use bevy::prelude::*;

use lunco_workbench_browser::{BrowserSectionRegistry, TwinBrowserPlugin};

mod twin_datasets;

pub use twin_datasets::TwinDatasetsSection;

/// Installs the Twin resource browser section.
///
/// The generic browser is installed automatically when needed. Dataset
/// lifecycle and authorization remain owned by `lunco-assets`; this package
/// only renders the registry and emits its typed request/cancel events.
pub struct TwinDatasetsPlugin;

impl Plugin for TwinDatasetsPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<TwinBrowserPlugin>() {
            app.add_plugins(TwinBrowserPlugin);
        }
        app.init_resource::<BrowserSectionRegistry>();
        app.world_mut()
            .resource_mut::<BrowserSectionRegistry>()
            .register(TwinDatasetsSection);
    }
}
