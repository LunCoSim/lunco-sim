//! # lunco-workbench
//!
//! LunCoSim's own workbench shell. Renders the standard engineering-IDE
//! layout documented in [`docs/architecture/11-workbench.md`]:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │ menu bar                                                    │
//! ├─────────────────────────────────────────────────────────────┤
//! │ workspace tabs                                              │
//! ├───┬─────────────────────────────────────────────────────────┤
//! │ A │                                                         │
//! │ c │      egui_dock tree                                     │
//! │ t │      (drag-to-rearrange, split, tabs, float)            │
//! │ . │                                                         │
//! ├───┴─────────────────────────────────────────────────────────┤
//! │ status bar                                                  │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! Powered by [`egui_dock`] under the hood — drag tabs to rearrange,
//! split panels by dragging to the edge, double-click to maximise,
//! float into separate windows. The host app stays decoupled: each
//! panel is just an implementor of [`Panel`].
//!
//! ## What this crate ships today
//!
//! - [`Panel`] trait: minimal render contract (`id`, `title`,
//!   `default_slot`, `render(&mut Ui, &mut World)`)
//! - [`WorkbenchLayout`] resource wrapping `egui_dock::DockState`
//! - Workspace presets (slot-assignment DSL) — see [`Workspace`]
//! - Auto-add of `bevy_egui::EguiPlugin` if the host hasn't
//!
//! ## What's deferred
//!
//! - **Layout persistence** — dock changes reset on launch (egui_dock
//!   has serde support for the tree; wiring it is a follow-up).
//! - **Command palette** — `Ctrl+P` unbound.
//! - **Theming / keybinds** — egui defaults only.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};
use std::collections::HashMap;

mod panel;
mod viewport;
mod workspace;

pub use panel::{Panel, PanelId, PanelSlot};
pub use viewport::{ViewportPanel, WorkbenchViewportCamera, VIEWPORT_PANEL_ID};
pub use workspace::{Workspace, WorkspaceId};

/// Shared backdrop colour for panel bodies, tab headers, and the tab
/// bar — fully opaque so "all opaque" apps (e.g. `modelica_workbench`)
/// get a solid continuous tile. Translucent-panel apps (e.g. the
/// rover sandbox) still get see-through behaviour: they set
/// `Panel::transparent_background = true`, which causes `egui_dock`
/// to skip the body fill AND the workbench renderer keeps the tab
/// bar transparent, so 3D shows through.
pub const PANEL_BACKDROP: bevy_egui::egui::Color32 =
    bevy_egui::egui::Color32::from_rgb(24, 24, 28);

/// Plugin that installs the workbench shell into a Bevy app.
///
/// Auto-adds [`bevy_egui::EguiPlugin`] if the host hasn't (so apps
/// migrating from `bevy_workbench` don't have to remember to add it).
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

/// Workbench state: registered panels + the dock tree they live in.
///
/// Holds an `egui_dock::DockState<PanelId>` plus a registry of `Panel`
/// trait objects keyed by `PanelId`. The tree is mutated directly by
/// the user via egui_dock's drag-and-drop UI; workspaces seed it via
/// the slot-setter DSL ([`set_side_browser`](Self::set_side_browser),
/// [`set_center`](Self::set_center), [`set_right_inspector`](Self::set_right_inspector),
/// [`set_bottom`](Self::set_bottom)).
#[derive(Resource)]
pub struct WorkbenchLayout {
    pub(crate) panels: HashMap<PanelId, Box<dyn Panel>>,
    pub(crate) workspaces: Vec<Box<dyn Workspace>>,
    pub(crate) active_workspace: Option<WorkspaceId>,
    pub(crate) activity_bar: bool,

    // Slot intent — kept so workspaces can rebuild the dock when activated.
    // User drags after that mutate `dock` directly; intent goes stale until
    // the next workspace activation. Each side slot is a Vec so multiple
    // panels can be tabbed in the same dock region.
    pub(crate) side_browser: Vec<PanelId>,
    pub(crate) center: Vec<PanelId>,
    pub(crate) active_center_tab: usize,
    pub(crate) right_inspector: Vec<PanelId>,
    pub(crate) bottom: Vec<PanelId>,

    pub(crate) status: Option<StatusContent>,

    /// The live dock tree — what egui_dock actually renders.
    pub(crate) dock: DockState<PanelId>,
}

impl Default for WorkbenchLayout {
    fn default() -> Self {
        Self {
            panels: HashMap::new(),
            workspaces: Vec::new(),
            active_workspace: None,
            activity_bar: false,
            side_browser: Vec::new(),
            center: Vec::new(),
            active_center_tab: 0,
            right_inspector: Vec::new(),
            bottom: Vec::new(),
            status: None,
            dock: DockState::new(Vec::new()),
        }
    }
}

impl WorkbenchLayout {
    /// Register a panel and dock it in its default slot.
    pub fn register<P: Panel + 'static>(&mut self, panel: P) {
        let id = panel.id();
        let slot = panel.default_slot();
        match slot {
            PanelSlot::SideBrowser => {
                if !self.side_browser.contains(&id) {
                    self.side_browser.push(id);
                }
            }
            PanelSlot::Center => {
                if !self.center.contains(&id) {
                    self.center.push(id);
                }
            }
            PanelSlot::RightInspector => {
                if !self.right_inspector.contains(&id) {
                    self.right_inspector.push(id);
                }
            }
            PanelSlot::Bottom => {
                if !self.bottom.contains(&id) {
                    self.bottom.push(id);
                }
            }
            PanelSlot::Floating => { /* not yet rendered */ }
        }
        self.panels.insert(id, Box::new(panel));
        self.rebuild_dock();
    }

    /// Toggle visibility of the activity bar on the far left.
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

    /// Rebuild the dock tree from the current slot intent.
    ///
    /// Called by every slot setter and by [`activate_workspace`]. After
    /// rebuild, user drags persist until the next call.
    ///
    /// **Two-mode rendering** — the dock is only used when there are
    /// central tabs (i.e. apps like `modelica_workbench` that have
    /// Code/Diagram in the centre). In 3D apps where the centre is
    /// reserved for the Bevy viewport, the dock is left empty and the
    /// side panels render via plain `egui::SidePanel`/`TopBottomPanel`
    /// instead — see [`render_layout`]. This keeps the central region
    /// transparent so the 3D scene shows through.
    ///
    /// **`egui_dock` fraction quirk** — the docstring says `fraction`
    /// is the OLD node's share, but that's only true for `split_right`
    /// and `split_below`. For `split_left` and `split_above`, `fraction`
    /// is actually the NEW node's share, because the renderer places
    /// the divider at `rect.min + size * fraction` and the new node
    /// sits at `parent.left()` (i.e. the first child). So:
    ///
    /// | function | NEW gets | OLD gets |
    /// |---|---|---|
    /// | `split_left(_, f, new)` | `f` | `1 - f` |
    /// | `split_above(_, f, new)` | `f` | `1 - f` |
    /// | `split_right(_, f, new)` | `1 - f` | `f` |
    /// | `split_below(_, f, new)` | `1 - f` | `f` |
    ///
    /// We always pick the fraction so the panel we just added gets a
    /// small share (20% side, 22% right, 30% bottom).
    pub(crate) fn rebuild_dock(&mut self) {
        // Filter slot intent down to panels actually registered in this
        // app, so workspace presets can optimistically list panels that
        // may only exist in some binaries (e.g. a rover-only Code tab
        // referenced from the shared `BuildWorkspace`).
        let known = |ids: &[PanelId]| -> Vec<PanelId> {
            ids.iter().copied().filter(|id| self.panels.contains_key(id)).collect()
        };
        let side_browser_tabs = known(&self.side_browser);
        let right_inspector_tabs = known(&self.right_inspector);
        let bottom_tabs = known(&self.bottom);
        let center_tabs: Vec<PanelId> = self
            .center
            .iter()
            .copied()
            .filter(|id| self.panels.contains_key(id))
            .collect();

        // 3D apps: no central tabs → don't build a dock tree at all.
        // The renderer will lay out side panels with egui's SidePanels
        // and leave the central area transparent.
        if center_tabs.is_empty() {
            self.dock = DockState::new(Vec::new());
            return;
        }

        // Centre-driven apps: build the standard cross layout in egui_dock.
        // Splits are ordered so right and left span the full window height,
        // and bottom spans the central column's width (sandwiched between
        // them). Each subsequent split at NodeIndex::root() wraps the
        // previous tree, so the outermost splits dominate the layout.
        let mut dock = DockState::new(center_tabs.clone());
        let mut central = NodeIndex::root();

        if !bottom_tabs.is_empty() {
            let main = dock.main_surface_mut();
            let [center_after, _below] = main.split_below(central, 0.7, bottom_tabs);
            central = center_after;
        }

        // Target initial split: 15% side / 65% centre / 20% right.
        // Splits compound: split_right runs first, then split_left wraps
        // the whole tree and shrinks the previous splits proportionally.
        // To land at the target after compounding:
        //   split_right with f_right = 0.765 → right = (1 - 0.765) of pre-left-split = 0.235
        //   split_left  with f_left  = 0.15  → side = 0.15 of total
        //   Right after compounding  = 0.235 × (1 - 0.15) = 0.200 ✓
        //   Centre after compounding = 0.765 × (1 - 0.15) = 0.650 ✓
        if !right_inspector_tabs.is_empty() {
            let main = dock.main_surface_mut();
            let [_old_root, _right] =
                main.split_right(NodeIndex::root(), 0.765, right_inspector_tabs);
        }

        if !side_browser_tabs.is_empty() {
            let main = dock.main_surface_mut();
            // For split_left, fraction is the NEW (left) share — see
            // the table in the doc above.
            let [_old_root, _left] =
                main.split_left(NodeIndex::root(), 0.15, side_browser_tabs);
        }

        let _ = central;
        self.dock = dock;
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

// ─────────────────────────────────────────────────────────────────────
// Renderer
// ─────────────────────────────────────────────────────────────────────

fn render_workbench(world: &mut World) {
    let ctx = {
        let mut state: bevy::ecs::system::SystemState<EguiContexts> =
            bevy::ecs::system::SystemState::new(world);
        let mut contexts = state.get_mut(world);
        match contexts.ctx_mut() {
            Ok(ctx) => ctx.clone(),
            Err(_) => return,
        }
    };

    let Some(mut layout) = world.remove_resource::<WorkbenchLayout>() else {
        return;
    };

    render_layout(&ctx, &mut layout, world);

    world.insert_resource(layout);
}

/// `egui_dock::TabViewer` impl that delegates each tab's render to the
/// `Panel` trait, looking the panel up by id.
struct PanelTabViewer<'a> {
    panels: &'a mut HashMap<PanelId, Box<dyn Panel>>,
    world: &'a mut World,
}

impl<'a> TabViewer for PanelTabViewer<'a> {
    type Tab = PanelId;

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        match self.panels.get(tab) {
            Some(p) => p.title().into(),
            None => format!("?{}?", tab.as_str()).into(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        // Take the panel out, render it, put it back. This avoids keeping
        // a mutable borrow on `self.panels` for the duration of `render`,
        // which would conflict if the panel ever needed to look up sibling
        // panel metadata via the layout (unlikely today, future-proof).
        if let Some(mut panel) = self.panels.remove(tab) {
            panel.render(ui, self.world);
            self.panels.insert(*tab, panel);
        } else {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                format!("Panel `{}` not registered", tab.as_str()),
            );
        }
    }

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(("lunco_workbench_tab", tab.as_str()))
    }

    fn clear_background(&self, tab: &Self::Tab) -> bool {
        // Honour the panel's preference: transparent panels (e.g. the
        // 3D viewport) skip the dock's background fill so the scene
        // behind egui shows through.
        match self.panels.get(tab) {
            Some(panel) => !panel.transparent_background(),
            None => true,
        }
    }

    fn is_closeable(&self, tab: &Self::Tab) -> bool {
        match self.panels.get(tab) {
            Some(panel) => panel.closable(),
            None => true,
        }
    }

    fn tab_style_override(
        &self,
        tab: &Self::Tab,
        global_style: &egui_dock::TabStyle,
    ) -> Option<egui_dock::TabStyle> {
        // The viewport tab's header is dead space (the panel itself
        // renders nothing — the 3D scene shows behind). Make the tab
        // header fully invisible: transparent background, outline, and
        // text. The bar still occupies its 24-px row because
        // egui_dock 0.18 has no per-leaf hide-bar option.
        if *tab == viewport::VIEWPORT_PANEL_ID {
            let mut style = global_style.clone();
            let invisible = egui::Color32::TRANSPARENT;
            for s in [
                &mut style.active,
                &mut style.inactive,
                &mut style.focused,
                &mut style.hovered,
            ] {
                s.bg_fill = invisible;
                s.outline_color = invisible;
                s.text_color = invisible;
            }
            return Some(style);
        }
        // All other tabs: use egui_dock's theme-derived defaults.
        // Earlier attempts to hand-pick colours drifted away from
        // egui's own visuals and looked out-of-place.
        None
    }
}

fn render_layout(ctx: &egui::Context, layout: &mut WorkbenchLayout, world: &mut World) {
    // ── Menu bar ────────────────────────────────────────────────────
    egui::TopBottomPanel::top("lunco_workbench_menu_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.menu_button("File", |ui| {
                ui.label("(File menu — todo)");
            });
            ui.menu_button("Edit", |ui| {
                ui.label("(Edit menu — todo)");
            });
            ui.menu_button("View", |ui| {
                if ui.button("Toggle Activity Bar").clicked() {
                    layout.toggle_activity_bar();
                    ui.close();
                }
                ui.separator();
                ui.label(egui::RichText::new("Panels").weak().small());
                // List every registered panel with a checkbox showing
                // whether it's currently in the dock. Clicking a closed
                // panel re-docks it in its default slot.
                let panels_meta: Vec<(PanelId, String, PanelSlot, bool)> = {
                    let docked: std::collections::HashSet<PanelId> =
                        layout.dock.iter_all_tabs().map(|(_, id)| *id).collect();
                    let mut sorted: Vec<(PanelId, String, PanelSlot, bool)> = layout
                        .panels
                        .values()
                        .map(|p| {
                            let id = p.id();
                            (id, p.title(), p.default_slot(), docked.contains(&id))
                        })
                        .collect();
                    sorted.sort_by(|a, b| a.1.cmp(&b.1));
                    sorted
                };
                for (id, title, slot, is_open) in panels_meta {
                    let mut checked = is_open;
                    if ui.checkbox(&mut checked, title).clicked() {
                        if checked && !is_open {
                            // Re-dock into the panel's default slot.
                            match slot {
                                PanelSlot::SideBrowser => {
                                    if !layout.side_browser.contains(&id) {
                                        layout.side_browser.push(id);
                                    }
                                }
                                PanelSlot::Center => {
                                    if !layout.center.contains(&id) {
                                        layout.center.push(id);
                                    }
                                }
                                PanelSlot::RightInspector => {
                                    if !layout.right_inspector.contains(&id) {
                                        layout.right_inspector.push(id);
                                    }
                                }
                                PanelSlot::Bottom => {
                                    if !layout.bottom.contains(&id) {
                                        layout.bottom.push(id);
                                    }
                                }
                                PanelSlot::Floating => {}
                            }
                            layout.rebuild_dock();
                        } else if !checked && is_open {
                            // Closing via this checkbox: remove from
                            // every slot AND from the live dock tree.
                            layout.side_browser.retain(|p| *p != id);
                            layout.center.retain(|p| *p != id);
                            layout.right_inspector.retain(|p| *p != id);
                            layout.bottom.retain(|p| *p != id);
                            layout.rebuild_dock();
                        }
                        ui.close();
                    }
                }
            });
            ui.menu_button("Help", |ui| {
                ui.label("LunCoSim workbench v0.2 (egui_dock)");
            });
            // Workspace tabs live in the menu bar (right-aligned).
            // No separate transport bar — saves a row of vertical space.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let active = layout.active_workspace;
                let tabs: Vec<(WorkspaceId, String, bool)> = layout
                    .workspaces
                    .iter()
                    .map(|w| {
                        let id = w.id();
                        (id, w.title(), active == Some(id))
                    })
                    // Iterate in reverse so right-to-left layout still puts
                    // them in registration order from left to right.
                    .rev()
                    .collect();
                for (id, title, is_active) in tabs {
                    let button = egui::Button::new(title.as_str()).selected(is_active);
                    if ui.add(button).clicked() && !is_active {
                        layout.activate_workspace(id);
                    }
                }
            });
        });
    });

    // ── Status bar ──────────────────────────────────────────────────
    egui::TopBottomPanel::bottom("lunco_workbench_status_bar").show(ctx, |ui| {
        ui.horizontal(|ui| match layout.status.as_ref() {
            Some(StatusContent::Text(s)) => {
                ui.label(egui::RichText::new(s).small());
            }
            None => {
                ui.label(egui::RichText::new("ready").small().weak());
            }
        });
    });

    // ── Activity bar ────────────────────────────────────────────────
    if layout.activity_bar {
        egui::SidePanel::left("lunco_workbench_activity_bar")
            .resizable(false)
            .exact_width(40.0)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(4.0);
                    for icon in ["📁", "🧩", "📦", "🔎", "⚙"] {
                        ui.label(icon);
                        ui.add_space(8.0);
                    }
                });
            });
    }

    // ── Dock area / side panels ─────────────────────────────────────
    // Two-mode rendering:
    //   1. If the dock has tabs (centre-driven app like modelica
    //      workbench), render the full DockArea.
    //   2. Otherwise (3D app like rover_sandbox_usd), render the side
    //      panels with plain SidePanel / TopBottomPanel and leave the
    //      central area transparent for the 3D viewport.
    let has_dock_tabs = layout.dock.iter_all_tabs().next().is_some();

    if has_dock_tabs {
        // Compute this BEFORE we split-borrow `panels` mutably below.
        let any_transparent = layout
            .panels
            .values()
            .any(|p| p.transparent_background());

        // In opaque-panels mode (no 3D viewport), paint a solid backdrop
        // on the lowest egui layer so that any gap in the dock — the
        // separator handles, the hairline around leaves, the margin
        // around the central frame — renders as PANEL_BACKDROP rather
        // than bleeding the window's clear colour through. This is
        // what makes the tab bar actually look contained rather than
        // floating on a transparent strip.
        if !any_transparent {
            let painter = ctx.layer_painter(egui::LayerId::background());
            painter.rect_filled(ctx.screen_rect(), 0.0, PANEL_BACKDROP);
        }

        let WorkbenchLayout { panels, dock, .. } = &mut *layout;
        let mut viewer = PanelTabViewer { panels, world };
        let mut style = Style::from_egui(ctx.style().as_ref());
        // Drop the outer dock border — it shows up as a thin line along
        // the inside edge of the side panels and looks like dead pixels
        // when the dock is otherwise transparent.
        style.main_surface_border_stroke = egui::Stroke::NONE;
        // Drop the resize separator's idle colour — that's the 1px line
        // between docked panels. Hover/drag colours stay so the user
        // can still find and grab the divider.
        style.separator.color_idle = egui::Color32::TRANSPARENT;
        // Drop the per-tab body border (the rectangle around every
        // panel content area). This is the "border when unfolded".
        style.tab.tab_body.stroke = egui::Stroke::NONE;
        // Opaque body fill matching the tab-header colour, so that
        // panels with `transparent_background = false` (e.g. the
        // Modelica workbench's panels) render on a solid tile rather
        // than inheriting a theme-dependent colour that can look
        // washed out over an empty central region. Panels that opt
        // into `transparent_background = true` (e.g. the rover's
        // side panels) cause `clear_background` to return false and
        // this fill is skipped for them — 3D still shows through.
        style.tab.tab_body.bg_fill = PANEL_BACKDROP;
        // Always opaque, in every app. Transparency on the bar made
        // the Modelica workbench look broken, and the rover sandbox's
        // centre is a transparent `ViewportPanel` anyway — a dark
        // strip above its invisible header just looks like the top
        // edge of the viewport tile, which is fine.
        style.tab_bar.bg_fill = PANEL_BACKDROP;
        // Drop the hairline under the active tab name too — same
        // visual-noise reason as the tab body stroke.
        style.tab_bar.hline_color = egui::Color32::TRANSPARENT;
        DockArea::new(dock).style(style).show(ctx, &mut viewer);
    } else {
        // 3D-app mode — explicit side panels, transparent centre.
        // Defaults are percentages of the current window so the layout
        // looks right whether the user runs in 1280×720 or 4K. Targets
        // mirror the centre-driven 15/70/15 split: side panels 15% of
        // window width each; bottom dock 20% of window height.
        let screen = ctx.content_rect();
        let side_default = (screen.width() * 0.15).max(140.0);
        let bottom_default = (screen.height() * 0.20).max(120.0);

        if let Some(id) = layout.side_browser.first().copied() {
            egui::SidePanel::left("lunco_workbench_side_panel_left")
                .resizable(true)
                .default_width(side_default)
                .min_width(120.0)
                .show(ctx, |ui| render_panel_solo(ui, &id, layout, world));
        }
        if let Some(id) = layout.right_inspector.first().copied() {
            egui::SidePanel::right("lunco_workbench_side_panel_right")
                .resizable(true)
                .default_width(side_default)
                .min_width(140.0)
                .show(ctx, |ui| render_panel_solo(ui, &id, layout, world));
        }
        if let Some(id) = layout.bottom.first().copied() {
            egui::TopBottomPanel::bottom("lunco_workbench_bottom_panel")
                .resizable(true)
                .default_height(bottom_default)
                .min_height(60.0)
                .show(ctx, |ui| render_panel_solo(ui, &id, layout, world));
        }
        // Central area: do NOT call CentralPanel — egui's bottom/side
        // panels reserve their space and the remaining region stays
        // free for the 3D scene that Bevy renders to the full window.
    }
}

/// Render a single panel inside its own egui container (side-panel mode).
/// Mirrors PanelTabViewer's lookup-and-take-back pattern.
fn render_panel_solo(
    ui: &mut egui::Ui,
    id: &PanelId,
    layout: &mut WorkbenchLayout,
    world: &mut World,
) {
    if let Some(panel) = layout.panels.get(id) {
        ui.label(egui::RichText::new(panel.title()).strong());
        ui.separator();
    }
    if let Some(mut panel) = layout.panels.remove(id) {
        panel.render(ui, world);
        layout.panels.insert(*id, panel);
    } else {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            format!("Panel `{}` not registered", id.as_str()),
        );
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
            layout.set_center(vec![]);
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
        assert_eq!(layout.side_browser, vec![PanelId("panel_a")]);
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

        assert_eq!(layout.active_workspace(), Some(WorkspaceId("a")));
        assert_eq!(layout.side_browser, vec![PanelId("panel_a")]);
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
        assert_eq!(layout.side_browser, vec![PanelId("panel_b")]);
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
        assert_eq!(layout.active_workspace(), Some(WorkspaceId("a")));
        assert_eq!(layout.side_browser, vec![PanelId("panel_a")]);
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
}
