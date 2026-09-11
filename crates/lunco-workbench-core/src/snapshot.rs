//! Read-only workbench state published by the concrete shell.

use bevy::prelude::Resource;

use crate::{PanelId, PerspectiveId, TabId};

/// Shell-independent view of the current workbench layout.
///
/// The concrete docking shell remains the source of truth. It publishes this
/// snapshot for domain systems that need layout facts without depending on
/// `egui_dock` or the shell's persistence/rendering implementation.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkbenchSnapshot {
    active_perspective: Option<PerspectiveId>,
    focused_tab: Option<TabId>,
    tabs: Vec<TabId>,
    visible_panels: Vec<PanelId>,
    registered_perspectives: Vec<PerspectiveId>,
    docked_panels: Vec<PanelId>,
}

impl WorkbenchSnapshot {
    /// Record a perspective registered by the concrete shell.
    ///
    /// Registration happens while plugins are being built, before the first
    /// shell update can publish a complete snapshot. Keeping this small
    /// registration seam here lets data-driven callers validate authored
    /// perspective ids at their launch boundary without reading the shell's
    /// dock resource.
    pub fn register_perspective(&mut self, id: PerspectiveId) {
        if !self.registered_perspectives.contains(&id) {
            self.registered_perspectives.push(id);
        }
    }

    /// Publish a new snapshot from the concrete shell.
    pub fn replace(
        &mut self,
        active_perspective: Option<PerspectiveId>,
        focused_tab: Option<TabId>,
        tabs: Vec<TabId>,
        visible_panels: Vec<PanelId>,
        registered_perspectives: Vec<PerspectiveId>,
        docked_panels: Vec<PanelId>,
    ) {
        self.active_perspective = active_perspective;
        self.focused_tab = focused_tab;
        self.tabs = tabs;
        self.visible_panels = visible_panels;
        self.registered_perspectives = registered_perspectives;
        self.docked_panels = docked_panels;
    }

    /// Return the active perspective, if one is selected.
    pub fn active_perspective(&self) -> Option<PerspectiveId> {
        self.active_perspective
    }

    /// Return the currently focused tab, if any.
    pub fn focused_tab(&self) -> Option<TabId> {
        self.focused_tab
    }

    /// Return the focused instance tab, if the focused tab is an instance.
    pub fn active_tab_instance(&self) -> Option<u64> {
        match self.focused_tab {
            Some(TabId::Instance { instance, .. }) => Some(instance),
            _ => None,
        }
    }

    /// Return all open instances of a kind in dock order.
    pub fn instances_in_order(&self, kind: PanelId) -> Vec<u64> {
        self.tabs
            .iter()
            .filter_map(|tab| match tab {
                TabId::Instance {
                    kind: tab_kind,
                    instance,
                } if *tab_kind == kind => Some(*instance),
                _ => None,
            })
            .collect()
    }

    /// Return whether a panel is currently docked.
    pub fn is_panel_docked(&self, id: PanelId) -> bool {
        self.docked_panels.contains(&id)
    }

    /// Return whether a panel is the active tab in any visible dock leaf.
    pub fn is_panel_visible(&self, id: PanelId) -> bool {
        self.visible_panels.contains(&id)
    }

    /// Return whether a perspective is registered with the shell.
    pub fn has_perspective(&self, id: PerspectiveId) -> bool {
        self.registered_perspectives.contains(&id)
    }

    /// Return whether a perspective with the authored raw id is registered.
    pub fn has_perspective_named(&self, id: &str) -> bool {
        self.registered_perspectives
            .iter()
            .any(|registered| registered.as_str() == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_and_layout_facts_are_exposed_without_shell_types() {
        let editor = PerspectiveId("editor");
        let model = PanelId("model");
        let document = TabId::instance(model, 7);
        let mut snapshot = WorkbenchSnapshot::default();

        snapshot.register_perspective(editor);
        snapshot.register_perspective(editor);
        assert!(snapshot.has_perspective(editor));
        assert!(snapshot.has_perspective_named("editor"));

        snapshot.replace(
            Some(editor),
            Some(document),
            vec![TabId::singleton(PanelId("browser")), document],
            vec![model],
            vec![editor],
            vec![model],
        );

        assert_eq!(snapshot.active_perspective(), Some(editor));
        assert_eq!(snapshot.focused_tab(), Some(document));
        assert_eq!(snapshot.active_tab_instance(), Some(7));
        assert_eq!(snapshot.instances_in_order(model), vec![7]);
        assert!(snapshot.is_panel_docked(model));
    }
}
