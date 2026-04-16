//! # lunco-workbench
//!
//! LunCoSim's own workbench shell. Renders the standard engineering-IDE
//! layout documented in [`docs/architecture/11-workbench.md`]:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │ menu bar / command palette                                  │
//! ├─────────────────────────────────────────────────────────────┤
//! │ workspace tabs · transport controls                         │
//! ├───┬─────────────────────────────────────┬───────────────────┤
//! │ A │  side browser       │               │                   │
//! │ c │  (per activity)     │   VIEWPORT    │   Inspector       │
//! │ t │                     │               │  (context-aware)  │
//! │ i │                     │               │                   │
//! │ v │                     ├───────────────┤                   │
//! │   │                     │  bottom dock  │                   │
//! ├───┴─────────────────────┴───────────────┴───────────────────┤
//! │ status bar                                                  │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## What this crate ships today (v0.1)
//!
//! - [`Panel`] trait: minimal render contract for dock contents
//! - [`PanelSlot`] enum: which dock region a panel lives in
//! - [`WorkbenchLayout`] resource: what's docked where, currently open
//! - [`WorkbenchPlugin`]: wires the layout renderer into a Bevy app
//!
//! ## What this crate explicitly does NOT ship yet
//!
//! - **Workspaces.** The design doc defines Build / Simulate / Analyze /
//!   Plan / Observe presets. Not yet — v1 is single-workspace.
//! - **Layout persistence.** Dock sizes reset on every app launch.
//! - **Command palette.** Ctrl+P is unbound.
//! - **Detachable windows.** Multi-viewport integration deferred.
//! - **Theming / keybinds / activity bar icon actions.** Stub UI only.
//!
//! Each of those gets its own commit and shows up here when it does.
//!
//! ## Comparison to `bevy_workbench`
//!
//! We currently use [`bevy_workbench`](https://github.com/LunCoSim/bevy_workbench)
//! (our fork of `Bli-AIk/bevy_workbench`). That crate gave us dock persistence
//! and tabbing for early UI work, but its panel trait mixes world-less `ui` and
//! world-borrowing `ui_world` methods in a way that's awkward for Document-System
//! panels. `lunco-workbench` is the native replacement we'll migrate panels to;
//! `bevy_workbench` will be retired in a single commit once every panel is
//! migrated (see `11-workbench.md` § 13).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use std::collections::HashMap;

mod layout;
mod panel;
mod workspace;

pub use panel::{Panel, PanelId, PanelSlot};
pub use workspace::{Workspace, WorkspaceId};

/// Plugin that installs the workbench shell into a Bevy app.
///
/// Adds an `EguiPrimaryContextPass` system that draws the menu bar,
/// activity bar, docks, and status bar. Auto-adds [`bevy_egui::EguiPlugin`]
/// if the host hasn't already (matching `bevy_workbench`'s behaviour, so
/// migrating apps don't have to remember to add it explicitly).
pub struct WorkbenchPlugin;

impl Plugin for WorkbenchPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<bevy_egui::EguiPlugin>() {
            app.add_plugins(bevy_egui::EguiPlugin::default());
        }
        app.init_resource::<WorkbenchLayout>()
            .add_systems(EguiPrimaryContextPass, render_workbench);
    }
}

/// Which panels are docked in which slot.
///
/// The layout is deliberately flat and simple for v0.1: each slot holds
/// at most one panel. Tabbing, splitting, and drag-to-rearrange come
/// when we migrate the first real panel off `bevy_workbench`.
#[derive(Resource, Default)]
pub struct WorkbenchLayout {
    pub(crate) panels: HashMap<PanelId, Box<dyn Panel>>,
    pub(crate) workspaces: Vec<Box<dyn Workspace>>,
    pub(crate) active_workspace: Option<WorkspaceId>,
    pub(crate) activity_bar: bool,
    pub(crate) side_browser: Option<PanelId>,
    pub(crate) center: Vec<PanelId>,
    pub(crate) active_center_tab: usize,
    pub(crate) right_inspector: Option<PanelId>,
    pub(crate) bottom: Option<PanelId>,
    pub(crate) bottom_visible: bool,
    pub(crate) status: Option<StatusContent>,
}

impl WorkbenchLayout {
    /// Register a panel and dock it in its default slot.
    pub fn register<P: Panel + 'static>(&mut self, panel: P) {
        let id = panel.id();
        let slot = panel.default_slot();
        match slot {
            PanelSlot::SideBrowser => {
                if self.side_browser.is_none() {
                    self.side_browser = Some(id);
                }
            }
            PanelSlot::Center => {
                if !self.center.contains(&id) {
                    self.center.push(id);
                }
            }
            PanelSlot::RightInspector => {
                if self.right_inspector.is_none() {
                    self.right_inspector = Some(id);
                }
            }
            PanelSlot::Bottom => {
                if self.bottom.is_none() {
                    self.bottom = Some(id);
                    self.bottom_visible = true;
                }
            }
            PanelSlot::Floating => { /* not yet rendered */ }
        }
        self.panels.insert(id, Box::new(panel));
    }

    /// Toggle visibility of the bottom dock.
    pub fn toggle_bottom(&mut self) {
        self.bottom_visible = !self.bottom_visible;
    }

    /// Toggle the activity bar on the far left.
    pub fn toggle_activity_bar(&mut self) {
        self.activity_bar = !self.activity_bar;
    }

    /// Set a single-line string rendered in the status bar.
    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some(StatusContent::Text(text.into()));
    }

    /// Register a workspace and store it in the switcher. If this is the
    /// first workspace added, it also becomes active and its `apply`
    /// runs immediately to seed the initial layout.
    pub fn register_workspace<W: Workspace + 'static>(&mut self, workspace: W) {
        let id = workspace.id();
        let first = self.workspaces.is_empty();
        self.workspaces.push(Box::new(workspace));
        if first {
            self.activate_workspace(id);
        }
    }

    /// Switch to the named workspace, re-applying its slot preset.
    /// No-op if the id isn't registered.
    pub fn activate_workspace(&mut self, id: WorkspaceId) {
        // Temporarily take the workspaces vec out so we can call
        // `apply` with `&mut self`. The workspace itself doesn't need
        // to be mutated, so cloning into and out of the field is fine.
        let workspaces = std::mem::take(&mut self.workspaces);
        if let Some(ws) = workspaces.iter().find(|w| w.id() == id) {
            ws.apply(self);
            self.active_workspace = Some(id);
        }
        self.workspaces = workspaces;
    }

    /// Which workspace is currently active, if any.
    pub fn active_workspace(&self) -> Option<WorkspaceId> {
        self.active_workspace
    }
}

/// Content options for the status bar.
pub enum StatusContent {
    /// A simple single-line string.
    Text(String),
}

/// Extension trait on [`App`] for ergonomic panel + workspace registration.
pub trait WorkbenchAppExt {
    /// Register a panel with the default workbench layout.
    fn register_panel<P: Panel + 'static>(&mut self, panel: P) -> &mut Self;

    /// Register a workspace. The first workspace registered becomes
    /// active and its `apply` seeds the initial slot assignments.
    fn register_workspace<W: Workspace + 'static>(&mut self, workspace: W) -> &mut Self;
}

impl WorkbenchAppExt for App {
    fn register_panel<P: Panel + 'static>(&mut self, panel: P) -> &mut Self {
        // Registration happens at app build time, not in a Startup system.
        // This keeps ordering deterministic (call order == registration
        // order) and avoids the "did I order Startup systems correctly?"
        // trap that would otherwise bite workspace presets that reference
        // panels by id.
        if !self.world().contains_resource::<WorkbenchLayout>() {
            self.init_resource::<WorkbenchLayout>();
        }
        self.world_mut().resource_mut::<WorkbenchLayout>().register(panel);
        self
    }

    fn register_workspace<W: Workspace + 'static>(&mut self, workspace: W) -> &mut Self {
        if !self.world().contains_resource::<WorkbenchLayout>() {
            self.init_resource::<WorkbenchLayout>();
        }
        self.world_mut()
            .resource_mut::<WorkbenchLayout>()
            .register_workspace(workspace);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestWorkspace {
        id: WorkspaceId,
        title: &'static str,
        marker: PanelId,
    }

    impl Workspace for TestWorkspace {
        fn id(&self) -> WorkspaceId { self.id }
        fn title(&self) -> String { self.title.to_string() }
        fn apply(&self, layout: &mut WorkbenchLayout) {
            layout.set_side_browser(Some(self.marker));
            layout.set_right_inspector(None);
            layout.set_bottom(None);
        }
    }

    #[test]
    fn first_registered_workspace_auto_activates() {
        let mut layout = WorkbenchLayout::default();
        assert!(layout.active_workspace().is_none());

        layout.register_workspace(TestWorkspace {
            id: WorkspaceId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });

        assert_eq!(layout.active_workspace(), Some(WorkspaceId("a")));
        assert_eq!(layout.side_browser, Some(PanelId("panel_a")));
    }

    #[test]
    fn second_workspace_does_not_override_active() {
        let mut layout = WorkbenchLayout::default();
        layout.register_workspace(TestWorkspace {
            id: WorkspaceId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });
        layout.register_workspace(TestWorkspace {
            id: WorkspaceId("b"),
            title: "B",
            marker: PanelId("panel_b"),
        });

        // Still on A — registering B shouldn't steal focus.
        assert_eq!(layout.active_workspace(), Some(WorkspaceId("a")));
        assert_eq!(layout.side_browser, Some(PanelId("panel_a")));
    }

    #[test]
    fn activate_workspace_applies_preset() {
        let mut layout = WorkbenchLayout::default();
        layout.register_workspace(TestWorkspace {
            id: WorkspaceId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });
        layout.register_workspace(TestWorkspace {
            id: WorkspaceId("b"),
            title: "B",
            marker: PanelId("panel_b"),
        });

        layout.activate_workspace(WorkspaceId("b"));
        assert_eq!(layout.active_workspace(), Some(WorkspaceId("b")));
        assert_eq!(layout.side_browser, Some(PanelId("panel_b")));
    }

    #[test]
    fn activate_unknown_workspace_is_noop() {
        let mut layout = WorkbenchLayout::default();
        layout.register_workspace(TestWorkspace {
            id: WorkspaceId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });

        layout.activate_workspace(WorkspaceId("ghost"));
        // Still on A; the side_browser from A's apply is still set.
        assert_eq!(layout.active_workspace(), Some(WorkspaceId("a")));
        assert_eq!(layout.side_browser, Some(PanelId("panel_a")));
    }

    #[test]
    fn center_tabs_stack_in_order() {
        let mut layout = WorkbenchLayout::default();
        layout.add_to_center(PanelId("a"));
        layout.add_to_center(PanelId("b"));
        layout.add_to_center(PanelId("a")); // duplicate — no-op
        assert_eq!(layout.center, vec![PanelId("a"), PanelId("b")]);
    }

    #[test]
    fn set_active_center_panel_selects_by_id() {
        let mut layout = WorkbenchLayout::default();
        layout.set_center(vec![PanelId("code"), PanelId("diagram")]);
        layout.set_active_center_panel(PanelId("diagram"));
        assert_eq!(layout.active_center_tab, 1);
    }

    #[test]
    fn set_center_clamps_active_tab() {
        let mut layout = WorkbenchLayout::default();
        layout.set_center(vec![PanelId("a"), PanelId("b"), PanelId("c")]);
        layout.set_active_center_tab(2);
        layout.set_center(vec![PanelId("x")]); // shrink
        assert_eq!(layout.active_center_tab, 0);
    }

    #[test]
    fn set_bottom_none_hides_bottom() {
        let mut layout = WorkbenchLayout::default();
        layout.set_bottom(Some(PanelId("console")));
        assert!(layout.bottom_visible);
        assert_eq!(layout.bottom, Some(PanelId("console")));

        layout.set_bottom(None);
        assert!(!layout.bottom_visible);
        assert_eq!(layout.bottom, None);
    }
}

fn render_workbench(world: &mut World) {
    // egui contexts are themselves a SystemParam, but we need exclusive
    // World access to hand panels a `&mut World`. Fetch the primary
    // context via the EguiContexts SystemState pattern.
    let ctx = {
        let mut state: bevy::ecs::system::SystemState<EguiContexts> = bevy::ecs::system::SystemState::new(world);
        let mut contexts = state.get_mut(world);
        match contexts.ctx_mut() {
            Ok(ctx) => ctx.clone(),
            Err(_) => return,
        }
    };

    // Move the layout resource out of the world so panels can take
    // `&mut World` during their render without borrow conflicts.
    let Some(mut layout) = world.remove_resource::<WorkbenchLayout>() else {
        return;
    };

    layout::render(&ctx, &mut layout, world);

    world.insert_resource(layout);
}
