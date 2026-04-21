//! # lunco-workbench
//!
//! LunCoSim's own workbench shell. Renders the standard engineering-IDE
//! layout documented in [`docs/architecture/11-workbench.md`]:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │ menu bar                                                    │
//! ├─────────────────────────────────────────────────────────────┤
//! │ perspective tabs                                            │
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
//! - Perspective presets (slot-assignment DSL) — see [`Perspective`]
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
use egui_dock::{
    widgets::tab_viewer::OnCloseResponse, DockArea, DockState, NodeIndex, Style, TabViewer,
};
use std::collections::HashMap;

mod panel;
mod perspective;
mod session;
mod viewport;

pub mod twin_browser;

pub use panel::{InstancePanel, Panel, PanelId, PanelSlot, TabId};
pub use twin_browser::{
    BrowserAction, BrowserActions, BrowserCtx, BrowserSection, BrowserSectionRegistry,
    FilesSection, TwinBrowserPanel, UnsavedDocEntry, UnsavedDocs, TWIN_BROWSER_PANEL_ID,
};

// ─────────────────────────────────────────────────────────────────────────────
// Tab-management commands
// ─────────────────────────────────────────────────────────────────────────────

/// Request the workbench open (or focus) a multi-instance tab.
///
/// Fire via `commands.trigger(OpenTab { kind, instance })` from
/// anywhere — a panel's render fn, a system, a domain-crate observer.
/// The workbench installs an observer that handles the event by
/// calling [`WorkbenchLayout::open_instance`] on its own schedule,
/// which avoids the re-entrance trap of touching `WorkbenchLayout`
/// while it's extracted for rendering.
#[derive(Event, Clone, Copy, Debug)]
pub struct OpenTab {
    /// The [`InstancePanel::kind`] to open.
    pub kind: PanelId,
    /// The tab's instance discriminant (typically a raw `DocumentId`).
    pub instance: u64,
}

/// Request the workbench close a multi-instance tab, if open.
#[derive(Event, Clone, Copy, Debug)]
pub struct CloseTab {
    /// The [`InstancePanel::kind`] to close.
    pub kind: PanelId,
    /// The tab's instance discriminant.
    pub instance: u64,
}

fn on_open_tab(trigger: On<OpenTab>, mut layout: ResMut<WorkbenchLayout>) {
    let ev = *trigger.event();
    layout.open_instance(ev.kind, ev.instance);
}

fn on_close_tab(trigger: On<CloseTab>, mut layout: ResMut<WorkbenchLayout>) {
    let ev = *trigger.event();
    layout.close_instance(ev.kind, ev.instance);
}
pub use perspective::{Perspective, PerspectiveId};
pub use session::{
    DocumentClosed, DocumentOpened, RegisterDocument, TwinAdded, TwinClosed,
    UnregisterDocument, WorkspacePlugin, WorkspaceResource,
};
pub use viewport::{ViewportPanel, WorkbenchViewportCamera, VIEWPORT_PANEL_ID};

/// Get the backdrop colour from the active theme.
fn get_panel_backdrop(theme: &lunco_theme::Theme) -> egui::Color32 {
    theme.colors.mantle
}



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
        if !app.is_plugin_added::<lunco_theme::ThemePlugin>() {
            app.add_plugins(lunco_theme::ThemePlugin);
        }
        // Workspace (editor session) resource + event observers. Lives
        // in a sub-plugin so headless tests / API-only servers that
        // don't want the full dock shell can still get the Workspace
        // wiring by adding just `WorkspacePlugin`.
        if !app.is_plugin_added::<session::WorkspacePlugin>() {
            app.add_plugins(session::WorkspacePlugin);
        }
        app.init_resource::<WorkbenchLayout>()
            .init_resource::<PendingTabCloses>()
            // Twin Browser plumbing — resources are always present so
            // the panel renders an empty state cleanly when no Twin is
            // open and no domain sections have registered yet. The
            // active Twin is tracked on `WorkspaceResource` (installed
            // by `WorkspacePlugin` above), not a panel-local resource.
            .init_resource::<BrowserSectionRegistry>()
            .init_resource::<BrowserActions>()
            .init_resource::<UnsavedDocs>()
            .add_observer(on_open_tab)
            .add_observer(on_close_tab)
            .add_systems(EguiPrimaryContextPass, render_workbench);

        // Built-in Files section ships with the workbench so apps get
        // a usable browser even before any domain plugin registers.
        // Registered after init_resource so the registry definitely
        // exists. Domain crates push their sections (Modelica, USD, …)
        // from their own plugin's build, which runs after ours.
        app.world_mut()
            .resource_mut::<BrowserSectionRegistry>()
            .register(FilesSection);
    }
}

/// Workbench state: registered panels + the dock tree they live in.
///
/// Holds an `egui_dock::DockState<PanelId>` plus a registry of `Panel`
/// trait objects keyed by `PanelId`. The tree is mutated directly by
/// the user via egui_dock's drag-and-drop UI; perspectives seed it via
/// the slot-setter DSL ([`set_side_browser`](Self::set_side_browser),
/// [`set_center`](Self::set_center), [`set_right_inspector`](Self::set_right_inspector),
/// [`set_bottom`](Self::set_bottom)).
#[derive(Resource)]
pub struct WorkbenchLayout {
    pub(crate) panels: HashMap<PanelId, Box<dyn Panel>>,
    /// Registered multi-instance panel kinds (one entry per
    /// [`InstancePanel::kind`]). Instances share the same renderer;
    /// each tab picks its behaviour via `TabId::Instance { kind, … }`.
    pub(crate) instance_panels: HashMap<PanelId, Box<dyn InstancePanel>>,
    pub(crate) perspectives: Vec<Box<dyn Perspective>>,
    pub(crate) active_perspective: Option<PerspectiveId>,
    pub(crate) activity_bar: bool,

    // Slot intent — kept so perspectives can rebuild the dock when activated.
    // User drags after that mutate `dock` directly; intent goes stale until
    // the next perspective activation. Each side slot is a Vec so multiple
    // panels can be tabbed in the same dock region.
    pub(crate) side_browser: Vec<PanelId>,
    pub(crate) center: Vec<PanelId>,
    pub(crate) active_center_tab: usize,
    pub(crate) right_inspector: Vec<PanelId>,
    pub(crate) bottom: Vec<PanelId>,

    pub(crate) status: Option<StatusContent>,

    /// App-wide Settings menu contributions. Domain plugins push a
    /// closure via [`WorkbenchLayout::register_settings`] at Startup;
    /// the closure is invoked each time the user opens the Settings
    /// drop-down. Keeps editor prefs / theme toggles / etc. in one
    /// discoverable place instead of scattered gear buttons.
    pub(crate) settings_menu:
        Vec<Box<dyn Fn(&mut bevy_egui::egui::Ui, &mut World) + Send + Sync>>,

    /// The live dock tree — what egui_dock actually renders. Stores
    /// [`TabId`]s so both singleton panels and multi-instance tabs
    /// coexist in the same tree.
    pub(crate) dock: DockState<TabId>,
}

/// Queue of tabs whose close-X was clicked but whose close the
/// [`TabViewer`] vetoed so a domain handler can prompt
/// (e.g. unsaved-changes dialog) before the final close.
///
/// Only multi-instance tabs use this pipeline; singleton panels
/// honour [`Panel::closable`] directly. Kept as a standalone resource
/// (not a field on [`WorkbenchLayout`]) because the layout is
/// *extracted* from the world during `render_workbench`, and `on_close`
/// fires from inside that render — so anything it touches has to live
/// on a different resource.
#[derive(Resource, Default)]
pub struct PendingTabCloses {
    pending: Vec<TabId>,
}

impl PendingTabCloses {
    /// Drain queued close requests. Domain-side systems call this
    /// each frame, decide per-tab (clean → confirm & close, dirty →
    /// prompt, then fire [`CloseTab`] on user confirmation).
    pub fn drain(&mut self) -> Vec<TabId> {
        std::mem::take(&mut self.pending)
    }

    /// Push a tab id to the queue. Used by the workbench's own
    /// `on_close` hook; domain crates usually go via
    /// [`drain`](Self::drain) instead.
    pub fn push(&mut self, tab: TabId) {
        self.pending.push(tab);
    }
}

impl Default for WorkbenchLayout {
    fn default() -> Self {
        Self {
            panels: HashMap::new(),
            instance_panels: HashMap::new(),
            perspectives: Vec::new(),
            active_perspective: None,
            activity_bar: false,
            side_browser: Vec::new(),
            center: Vec::new(),
            active_center_tab: 0,
            right_inspector: Vec::new(),
            bottom: Vec::new(),
            status: None,
            settings_menu: Vec::new(),
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

    /// Register a multi-instance panel *kind*. Tabs of this kind are
    /// opened via [`open_instance`](Self::open_instance) and dispatched
    /// to this [`InstancePanel`] by the workbench's tab viewer.
    ///
    /// A given kind should only be registered once per App; re-registering
    /// replaces the previous renderer.
    pub fn register_instance_panel<P: InstancePanel + 'static>(&mut self, panel: P) {
        self.instance_panels.insert(panel.kind(), Box::new(panel));
    }

    /// Open (or focus, if already open) a multi-instance tab of `kind`
    /// with the given `instance` discriminant. Slot comes from the
    /// kind's [`InstancePanel::default_slot`] on first open.
    ///
    /// The workbench scans the dock for an existing tab matching the
    /// id and focuses it if found; otherwise adds a new tab to the
    /// **center** leaf — identified by matching any singleton tab
    /// currently in the `center` slot intent.
    pub fn open_instance(&mut self, kind: PanelId, instance: u64) {
        if !self.instance_panels.contains_key(&kind) {
            bevy::log::warn!(
                "open_instance: no InstancePanel registered for kind {:?}",
                kind
            );
            return;
        }
        let tab = TabId::Instance { kind, instance };
        // Already open? Focus it.
        if let Some((surface, node, tab_idx)) = self.dock.find_tab(&tab) {
            self.dock.set_focused_node_and_surface((surface, node));
            self.dock
                .set_active_tab((surface, node, tab_idx));
            return;
        }

        // Find the center leaf. We identify it as the one containing
        // any tab whose `PanelId` is in our `center` slot intent —
        // or, failing that, any existing `TabId::Instance` of this
        // same `kind` (because instance tabs of a kind belong in its
        // `default_slot`, which for model views is Center).
        //
        // Falling back to "first leaf" was wrong: after split_left /
        // split_right / split_below, the tree's first leaf in walk
        // order is the left side panel, and new tabs landed inside
        // the Package Browser instead of the center.
        // Resolve the kind's preferred slot. New instance tabs should
        // land in the same dock area as their kind's defaults — e.g.
        // a `VizPanel` (Bottom) opened next to the singleton `Graphs`
        // tab, NOT in the Center alongside the model view.
        let preferred_slot = self
            .instance_panels
            .get(&kind)
            .map(|p| p.default_slot());
        // Build the set of singleton PanelIds occupying each slot so
        // we can find a leaf hosting any of them.
        let slot_ids: std::collections::HashSet<PanelId> = match preferred_slot {
            Some(PanelSlot::Center) => self.center.iter().copied().collect(),
            Some(PanelSlot::Bottom) => self.bottom.iter().copied().collect(),
            Some(PanelSlot::SideBrowser) => self.side_browser.iter().copied().collect(),
            Some(PanelSlot::RightInspector) => self.right_inspector.iter().copied().collect(),
            _ => std::collections::HashSet::new(),
        };
        let center_ids: std::collections::HashSet<PanelId> =
            self.center.iter().copied().collect();
        let target_leaf = {
            let main = self.dock.main_surface_mut();
            // Priority 1: leaf already hosting another instance of
            // this kind — keeps families together.
            find_leaf_matching(main, |t| matches!(*t, TabId::Instance { kind: k, .. } if k == kind))
                // Priority 2: leaf hosting any singleton in the
                // kind's preferred slot.
                .or_else(|| {
                    find_leaf_matching(main, |t| match *t {
                        TabId::Singleton(id) => slot_ids.contains(&id),
                        _ => false,
                    })
                })
                // Priority 3: leaf hosting any Center singleton (the
                // historical fallback, kept so kinds with no
                // preferred slot still land somewhere visible).
                .or_else(|| {
                    find_leaf_matching(main, |t| match *t {
                        TabId::Singleton(id) => center_ids.contains(&id),
                        _ => false,
                    })
                })
                // Priority 4: any leaf at all.
                .or_else(|| first_leaf(main))
        };

        if let Some(leaf) = target_leaf {
            let main = self.dock.main_surface_mut();
            main[leaf].append_tab(tab);
            // Focus the just-appended tab.
            if let Some(count) = main[leaf].tabs_count().checked_sub(1) {
                main.set_active_tab(leaf, count);
            }
            // Focus the leaf/surface too so egui_dock foregrounds it.
            self.dock
                .set_focused_node_and_surface((egui_dock::SurfaceIndex::main(), leaf));
        } else {
            // Empty dock (e.g. 3D app with no center tabs). Seed a
            // single leaf with this tab so at least something shows.
            self.dock = DockState::new(vec![tab]);
        }
    }

    /// Close a multi-instance tab if present. Idempotent.
    pub fn close_instance(&mut self, kind: PanelId, instance: u64) {
        let tab = TabId::Instance { kind, instance };
        if let Some(pos) = self.dock.find_tab(&tab) {
            self.dock.remove_tab(pos);
        }
    }

    /// Toggle visibility of the activity bar on the far left.
    pub fn toggle_activity_bar(&mut self) {
        self.activity_bar = !self.activity_bar;
    }

    /// Set a single-line string rendered in the status bar.
    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some(StatusContent::Text(text.into()));
    }

    /// Register a perspective and store it in the switcher. If this is the
    /// first perspective added, it also becomes active and its `apply`
    /// runs immediately to seed the initial layout.
    /// Register a closure that contributes rows to the app-wide
    /// Settings drop-down in the menu bar. Called once per open of the
    /// menu; the closure may read/write Bevy resources via `world`.
    ///
    /// Intended for domain plugins to expose editor / theme / pane
    /// preferences without each plugin inventing its own gear button.
    pub fn register_settings<F>(&mut self, callback: F)
    where
        F: Fn(&mut bevy_egui::egui::Ui, &mut World) + Send + Sync + 'static,
    {
        self.settings_menu.push(Box::new(callback));
    }

    pub fn register_perspective<W: Perspective + 'static>(&mut self, perspective: W) {
        let id = perspective.id();
        let first = self.perspectives.is_empty();
        self.perspectives.push(Box::new(perspective));
        if first {
            self.activate_perspective(id);
        }
    }

    /// Switch to the named perspective, re-applying its slot preset.
    /// No-op if the id isn't registered.
    pub fn activate_perspective(&mut self, id: PerspectiveId) {
        let perspectives = std::mem::take(&mut self.perspectives);
        if let Some(ws) = perspectives.iter().find(|w| w.id() == id) {
            ws.apply(self);
            self.active_perspective = Some(id);
        }
        self.perspectives = perspectives;
    }

    /// Which perspective is currently active, if any.
    pub fn active_perspective(&self) -> Option<PerspectiveId> {
        self.active_perspective
    }

    /// Rebuild the dock tree from the current slot intent.
    ///
    /// Called by every slot setter and by [`activate_perspective`]. After
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
        // app, so perspective presets can optimistically list panels that
        // may only exist in some binaries (e.g. a rover-only Code tab
        // referenced from the shared `BuildPerspective`).
        //
        // Perspective slot-setters still use `PanelId` — slot presets
        // describe singleton-panel layouts. Instance-panel tabs are
        // opened dynamically at runtime (e.g. Package Browser opens a
        // model tab) and don't come from the perspective preset.
        let known = |ids: &[PanelId]| -> Vec<TabId> {
            ids.iter()
                .copied()
                .filter(|id| self.panels.contains_key(id))
                .map(TabId::Singleton)
                .collect()
        };
        let side_browser_tabs = known(&self.side_browser);
        let right_inspector_tabs = known(&self.right_inspector);
        let bottom_tabs = known(&self.bottom);
        let center_tabs: Vec<TabId> = self
            .center
            .iter()
            .copied()
            .filter(|id| self.panels.contains_key(id))
            .map(TabId::Singleton)
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

/// Extension trait on [`App`] for ergonomic panel + perspective registration.
pub trait WorkbenchAppExt {
    /// Register a panel with the default workbench layout.
    fn register_panel<P: Panel + 'static>(&mut self, panel: P) -> &mut Self;

    /// Register a multi-instance panel kind (e.g. model tabs).
    /// Instances are opened at runtime via
    /// [`WorkbenchLayout::open_instance`].
    fn register_instance_panel<P: InstancePanel + 'static>(&mut self, panel: P) -> &mut Self;

    /// Register a perspective. The first perspective registered becomes
    /// active and its `apply` seeds the initial slot assignments.
    fn register_perspective<W: Perspective + 'static>(&mut self, perspective: W) -> &mut Self;
}

impl WorkbenchAppExt for App {
    fn register_panel<P: Panel + 'static>(&mut self, panel: P) -> &mut Self {
        if !self.world().contains_resource::<WorkbenchLayout>() {
            self.init_resource::<WorkbenchLayout>();
        }
        self.world_mut().resource_mut::<WorkbenchLayout>().register(panel);
        self
    }

    fn register_instance_panel<P: InstancePanel + 'static>(&mut self, panel: P) -> &mut Self {
        if !self.world().contains_resource::<WorkbenchLayout>() {
            self.init_resource::<WorkbenchLayout>();
        }
        self.world_mut()
            .resource_mut::<WorkbenchLayout>()
            .register_instance_panel(panel);
        self
    }

    fn register_perspective<W: Perspective + 'static>(&mut self, perspective: W) -> &mut Self {
        if !self.world().contains_resource::<WorkbenchLayout>() {
            self.init_resource::<WorkbenchLayout>();
        }
        self.world_mut()
            .resource_mut::<WorkbenchLayout>()
            .register_perspective(perspective);
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

    let theme = world.resource::<lunco_theme::Theme>().clone();

    render_layout(&ctx, &mut layout, world, &theme);

    world.insert_resource(layout);
}

/// First leaf node (in walk order) in a `Surface`'s tree, if any.
/// Used as a last-resort fallback when no more specific target leaf
/// can be identified.
fn first_leaf(
    surface: &mut egui_dock::Tree<TabId>,
) -> Option<NodeIndex> {
    for (index, node) in surface.iter_mut().enumerate() {
        if node.is_leaf() {
            return Some(NodeIndex(index));
        }
    }
    None
}

/// First leaf containing any tab for which `pred` returns `true`.
/// Used by [`WorkbenchLayout::open_instance`] to find the center
/// tabset after perspective splits have moved it around.
fn find_leaf_matching<F>(
    surface: &mut egui_dock::Tree<TabId>,
    pred: F,
) -> Option<NodeIndex>
where
    F: Fn(&TabId) -> bool,
{
    for (index, node) in surface.iter_mut().enumerate() {
        if node.is_leaf() {
            if let Some(tabs) = node.tabs() {
                if tabs.iter().any(&pred) {
                    return Some(NodeIndex(index));
                }
            }
        }
    }
    None
}

/// `egui_dock::TabViewer` impl that delegates each tab's render to
/// the matching `Panel` (for singletons) or `InstancePanel` (for
/// multi-instance tabs), looking them up by the tab's [`TabId`].
struct PanelTabViewer<'a> {
    panels: &'a mut HashMap<PanelId, Box<dyn Panel>>,
    instance_panels: &'a mut HashMap<PanelId, Box<dyn InstancePanel>>,
    world: &'a mut World,
}

impl<'a> TabViewer for PanelTabViewer<'a> {
    type Tab = TabId;

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        match *tab {
            TabId::Singleton(id) => match self.panels.get(&id) {
                Some(p) => p.dynamic_title(self.world).into(),
                None => format!("?{}?", id.as_str()).into(),
            },
            TabId::Instance { kind, instance } => match self.instance_panels.get(&kind) {
                Some(p) => p.title(self.world, instance).into(),
                None => format!("?{}#{}?", kind.as_str(), instance).into(),
            },
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        match *tab {
            TabId::Singleton(id) => {
                // Take-and-return pattern so the panel can itself borrow
                // other panels' metadata via the layout (future-proof).
                if let Some(mut panel) = self.panels.remove(&id) {
                    panel.render(ui, self.world);
                    self.panels.insert(id, panel);
                } else {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        format!("Panel `{}` not registered", id.as_str()),
                    );
                }
            }
            TabId::Instance { kind, instance } => {
                if let Some(mut panel) = self.instance_panels.remove(&kind) {
                    panel.render(ui, self.world, instance);
                    self.instance_panels.insert(kind, panel);
                } else {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        format!(
                            "InstancePanel kind `{}` not registered",
                            kind.as_str()
                        ),
                    );
                }
            }
        }
    }

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(("lunco_workbench_tab", tab.debug_id()))
    }

    fn clear_background(&self, tab: &Self::Tab) -> bool {
        match *tab {
            TabId::Singleton(id) => match self.panels.get(&id) {
                Some(panel) => !panel.transparent_background(),
                None => true,
            },
            TabId::Instance { kind, .. } => match self.instance_panels.get(&kind) {
                Some(panel) => !panel.transparent_background(),
                None => true,
            },
        }
    }

    fn is_closeable(&self, tab: &Self::Tab) -> bool {
        match *tab {
            TabId::Singleton(id) => match self.panels.get(&id) {
                Some(panel) => panel.closable(),
                None => true,
            },
            TabId::Instance { kind, .. } => match self.instance_panels.get(&kind) {
                Some(panel) => panel.closable(),
                None => true,
            },
        }
    }

    /// Called when the user clicks the tab's × button. Returning
    /// [`OnCloseResponse::Ignore`] cancels the close; the tab stays.
    /// For multi-instance tabs we queue the id and cancel, so
    /// domain crates can confirm-on-unsaved-changes before the tab
    /// actually goes away. Singleton panels close immediately.
    fn on_close(&mut self, tab: &mut Self::Tab) -> OnCloseResponse {
        match *tab {
            TabId::Singleton(_) => OnCloseResponse::Close,
            TabId::Instance { .. } => {
                // `WorkbenchLayout` is extracted during render, so we
                // use the standalone `PendingTabCloses` resource. A
                // domain-side system drains it each frame, prompts
                // if needed, and fires `CloseTab` on user confirmation.
                self.world
                    .resource_mut::<PendingTabCloses>()
                    .push(*tab);
                OnCloseResponse::Ignore
            }
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
        if *tab == TabId::Singleton(viewport::VIEWPORT_PANEL_ID) {
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
        None
    }
}

fn render_layout(ctx: &egui::Context, layout: &mut WorkbenchLayout, world: &mut World, theme: &lunco_theme::Theme) {
    // ── Opaque-mode backdrop (must run first) ───────────────────────
    // In apps where every panel is opaque (no 3D viewport showing
    // through), paint `get_panel_backdrop(theme)` on the background layer BEFORE
    // registering any panel shapes. egui draws within a layer in the
    // order shapes are issued, so a rect_filled issued AFTER the menu
    // bar / dock / status bar would paint over them — exactly the
    // "invisible menu" regression the opaque-backdrop change
    // introduced. Running it first keeps the fill underneath.
    let any_transparent = layout
        .panels
        .values()
        .any(|p| p.transparent_background());
    if !any_transparent {
        let painter = ctx.layer_painter(egui::LayerId::background());
        painter.rect_filled(ctx.screen_rect(), 0.0, get_panel_backdrop(theme));
    }

    // ── Menu bar ────────────────────────────────────────────────────
    egui::TopBottomPanel::top("lunco_workbench_menu_bar").show(ctx, |ui| {
        ui.style_mut().visuals = theme.to_visuals();
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
                    // Only track singleton tabs for the "Panels" menu —
                    // instance tabs (one per open doc) aren't a menu
                    // concept.
                    let docked: std::collections::HashSet<PanelId> = layout
                        .dock
                        .iter_all_tabs()
                        .filter_map(|(_, id)| match id {
                            TabId::Singleton(pid) => Some(*pid),
                            TabId::Instance { .. } => None,
                        })
                        .collect();
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
            ui.menu_button("Settings", |ui| {
                ui.label(egui::RichText::new("Theme").weak().small());
                let mut theme = world.resource_mut::<lunco_theme::Theme>();
                let mode = theme.mode;
                
                let label = match mode {
                    lunco_theme::ThemeMode::Dark => "🌙 Dark",
                    lunco_theme::ThemeMode::Light => "☀ Light",
                };

                if ui.button(label).clicked() {
                    theme.toggle_mode();
                }
                ui.separator();

                // Take the callbacks out so we can pass &mut World into
                // them while the layout is still extracted. Restored
                // at the end of the block.
                let callbacks = std::mem::take(&mut layout.settings_menu);
                if callbacks.is_empty() {
                    ui.label(
                        egui::RichText::new("(no settings registered)")
                            .weak()
                            .italics(),
                    );
                } else {
                    for (i, cb) in callbacks.iter().enumerate() {
                        if i > 0 {
                            ui.separator();
                        }
                        cb(ui, world);
                    }
                }
                layout.settings_menu = callbacks;
            });
            ui.menu_button("Help", |ui| {
                ui.label("LunCoSim workbench v0.2 (egui_dock)");
            });
            // Perspective tabs live in the menu bar (right-aligned).
            // No separate transport bar — saves a row of vertical space.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let active = layout.active_perspective;
                let tabs: Vec<(PerspectiveId, String, bool)> = layout
                    .perspectives
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
                        layout.activate_perspective(id);
                    }
                }
            });
        });
    });

    // ── Status bar ──────────────────────────────────────────────────
    egui::TopBottomPanel::bottom("lunco_workbench_status_bar").show(ctx, |ui| {
        ui.style_mut().visuals = theme.to_visuals();
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
                ui.style_mut().visuals = theme.to_visuals();
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
        let WorkbenchLayout {
            panels,
            instance_panels,
            dock,
            ..
        } = &mut *layout;
        let mut viewer = PanelTabViewer {
            panels,
            instance_panels,
            world,
        };
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
        style.tab.tab_body.bg_fill = get_panel_backdrop(theme);
        // Always opaque, in every app. Transparency on the bar made
        // the Modelica workbench look broken, and the rover sandbox's
        // centre is a transparent `ViewportPanel` anyway — a dark
        // strip above its invisible header just looks like the top
        // edge of the viewport tile, which is fine.
        style.tab_bar.bg_fill = get_panel_backdrop(theme);
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
                .show(ctx, |ui| {
                    ui.style_mut().visuals = theme.to_visuals();
                    render_panel_solo(ui, &id, layout, world);
                });
        }
        if let Some(id) = layout.right_inspector.first().copied() {
            egui::SidePanel::right("lunco_workbench_side_panel_right")
                .resizable(true)
                .default_width(side_default)
                .min_width(140.0)
                .show(ctx, |ui| {
                    ui.style_mut().visuals = theme.to_visuals();
                    render_panel_solo(ui, &id, layout, world);
                });
        }
        if let Some(id) = layout.bottom.first().copied() {
            egui::TopBottomPanel::bottom("lunco_workbench_bottom_panel")
                .resizable(true)
                .default_height(bottom_default)
                .min_height(60.0)
                .show(ctx, |ui| {
                    ui.style_mut().visuals = theme.to_visuals();
                    render_panel_solo(ui, &id, layout, world);
                });
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

    struct TestPerspective {
        id: PerspectiveId,
        title: &'static str,
        marker: PanelId,
    }

    impl Perspective for TestPerspective {
        fn id(&self) -> PerspectiveId { self.id }
        fn title(&self) -> String { self.title.to_string() }
        fn apply(&self, layout: &mut WorkbenchLayout) {
            layout.set_side_browser(Some(self.marker));
            layout.set_right_inspector(None);
            layout.set_bottom(None);
            layout.set_center(vec![]);
        }
    }

    #[test]
    fn first_registered_perspective_auto_activates() {
        let mut layout = WorkbenchLayout::default();
        assert!(layout.active_perspective().is_none());

        layout.register_perspective(TestPerspective {
            id: PerspectiveId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });

        assert_eq!(layout.active_perspective(), Some(PerspectiveId("a")));
        assert_eq!(layout.side_browser, vec![PanelId("panel_a")]);
    }

    #[test]
    fn second_perspective_does_not_override_active() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("b"),
            title: "B",
            marker: PanelId("panel_b"),
        });

        assert_eq!(layout.active_perspective(), Some(PerspectiveId("a")));
        assert_eq!(layout.side_browser, vec![PanelId("panel_a")]);
    }

    #[test]
    fn activate_perspective_applies_preset() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("b"),
            title: "B",
            marker: PanelId("panel_b"),
        });

        layout.activate_perspective(PerspectiveId("b"));
        assert_eq!(layout.active_perspective(), Some(PerspectiveId("b")));
        assert_eq!(layout.side_browser, vec![PanelId("panel_b")]);
    }

    #[test]
    fn activate_unknown_perspective_is_noop() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });

        layout.activate_perspective(PerspectiveId("ghost"));
        assert_eq!(layout.active_perspective(), Some(PerspectiveId("a")));
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
