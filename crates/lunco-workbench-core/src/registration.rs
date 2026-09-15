//! Renderer-neutral panel registration for workbench hosts.

use bevy::app::App;
use bevy::prelude::Resource;

use crate::{InstancePanel, Panel};

/// Registration queue shared by panel packages and a concrete workbench host.
///
/// Panel packages add their renderers during plugin construction. The concrete
/// shell drains this queue into its layout when it is present. Keeping the
/// queue here means a reusable panel package does not need a dependency on the
/// shell merely to make its panel available.
#[derive(Resource, Default)]
pub struct WorkbenchPanelRegistry {
    panels: Vec<Box<dyn Panel>>,
    instance_panels: Vec<Box<dyn InstancePanel>>,
}

impl WorkbenchPanelRegistry {
    /// Register or replace a singleton panel while preserving registration
    /// order for new panel ids.
    pub fn register_panel<P: Panel + 'static>(&mut self, panel: P) {
        let id = panel.id();
        if let Some(existing) = self.panels.iter_mut().find(|existing| existing.id() == id) {
            *existing = Box::new(panel);
        } else {
            self.panels.push(Box::new(panel));
        }
    }

    /// Register or replace a multi-instance panel kind while preserving
    /// registration order for new kinds.
    pub fn register_instance_panel<P: InstancePanel + 'static>(&mut self, panel: P) {
        let kind = panel.kind();
        if let Some(existing) = self
            .instance_panels
            .iter_mut()
            .find(|existing| existing.kind() == kind)
        {
            *existing = Box::new(panel);
        } else {
            self.instance_panels.push(Box::new(panel));
        }
    }

    /// Remove all queued singleton panels in registration order.
    pub fn take_panels(&mut self) -> Vec<Box<dyn Panel>> {
        std::mem::take(&mut self.panels)
    }

    /// Remove all queued multi-instance panel kinds in registration order.
    pub fn take_instance_panels(&mut self) -> Vec<Box<dyn InstancePanel>> {
        std::mem::take(&mut self.instance_panels)
    }
}

/// Extension methods for registering panels without depending on the concrete
/// egui docking shell.
pub trait WorkbenchPanelAppExt {
    /// Queue a singleton panel for the active workbench host.
    fn register_panel<P: Panel + 'static>(&mut self, panel: P) -> &mut Self;

    /// Queue a multi-instance panel kind for the active workbench host.
    fn register_instance_panel<P: InstancePanel + 'static>(&mut self, panel: P) -> &mut Self;
}

impl WorkbenchPanelAppExt for App {
    fn register_panel<P: Panel + 'static>(&mut self, panel: P) -> &mut Self {
        self.init_resource::<WorkbenchPanelRegistry>();
        self.world_mut()
            .resource_mut::<WorkbenchPanelRegistry>()
            .register_panel(panel);
        self
    }

    fn register_instance_panel<P: InstancePanel + 'static>(&mut self, panel: P) -> &mut Self {
        self.init_resource::<WorkbenchPanelRegistry>();
        self.world_mut()
            .resource_mut::<WorkbenchPanelRegistry>()
            .register_instance_panel(panel);
        self
    }
}
