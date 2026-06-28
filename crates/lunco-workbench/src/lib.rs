//! # lunco-workbench
//!
//! LunCoSim's own workbench shell. Renders the standard engineering-IDE
//! layout documented in `docs/architecture/11-workbench.md`:
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
//! ## What's persisted across restarts
//!
//! - **Window geometry** (size / position / maximized) — global default
//!   in `~/.lunco/settings.json` via `lunco-settings`. See
//!   [`window_persistence`].
//! - **Per-Twin UI state** (active perspective + open-document list) —
//!   `~/.lunco/workspace-state/<hash>.json`, keyed by Twin path,
//!   VSCode-`workspaceStorage` style. See [`workspace_state`].
//!
//! ## What's deferred
//!
//! - **Free-form dock-tree fidelity** — restore re-applies the
//!   *perspective* preset, not arbitrary user split rearrangements
//!   (egui_dock's tree isn't serialized; `TabId`/`PanelId` hold
//!   `&'static str`).
//! - **Document auto-reopen** — open paths are persisted but not yet
//!   replayed on launch (needs per-domain open commands).
//! - **Command palette** — `Ctrl+P` unbound.
//! - **Theming / keybinds** — egui defaults only.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use egui_dock::{
    widgets::tab_viewer::OnCloseResponse, DockArea, DockState, NodeIndex, Style, TabViewer,
};
use lunco_core::{Command, on_command, register_commands};
use lunco_theme::ColorAlpha;
use std::collections::HashMap;

mod panel;
mod perspective;
mod perspective_help;
mod render_robustness;
mod session;
mod viewport;

pub mod file_ops;
pub mod files_panel;
pub mod perf_hud;
pub mod perspective_command;
pub mod picker;
pub mod status_bus;
pub mod theme_command;
pub mod tracked_task;
pub mod twin_browser;
pub mod uri;
pub mod window_command;
pub mod window_persistence;
pub mod window_placement;
pub mod workspace_state;

pub use perspective_help::{
    HelpMouse, HelpPopup, HelpShortcut, HelpTourRequest, PerspectiveHelp, PerspectiveHelpPlugin,
    PerspectiveHelpRegistry,
};
pub use window_command::{merged_titlebar_window, MaximizeWindow, MinimizeWindow, CloseWindow, WindowMaximized};
#[cfg(not(target_arch = "wasm32"))]
pub use window_placement::WindowPlacement;
pub use window_placement::wire_window_placement;
pub use window_persistence::{
    load_window_geometry, restored_window, SkipWindowGeometrySave, WindowGeometry,
    WindowPersistencePlugin, DEFAULT_WINDOW_HEIGHT, DEFAULT_WINDOW_WIDTH,
};
pub use workspace_state::{
    workspace_state_path, AppDocumentSessionExt, DocumentSessionCodec, DocumentSessionRegistry,
    DocumentSnapshot, WorkspaceState, WorkspaceStatePlugin,
};
pub use render_robustness::preferred_wgpu_settings;

pub use panel::{InstancePanel, Panel, PanelCtx, PanelId, PanelSlot, TabId};

/// SystemSet that runs the main workbench egui pass. Use
/// `.after(WorkbenchRenderSet)` for systems that need to read
/// the rects the workbench just published (e.g. help-tour overlays
/// reading [`HelpAnchors`]).
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorkbenchRenderSet;

/// Desired pixel widths for the side / right dock panes. Read each
/// frame by [`WorkbenchLayout::enforce_fixed_widths`] which rewrites
/// the relevant split fractions so the panes stay at a constant
/// absolute size as the window resizes — instead of scaling
/// proportionally, which is egui_dock's default fraction-based
/// behaviour.
///
/// Defaults are sized for "comfortable to read at default zoom" and
/// match common IDE chrome (VS Code's sidebar at 280 px, inspector
/// at 320 px).
#[derive(Resource, Debug, Clone, Copy)]
pub struct DockSizes {
    /// Target width in screen-space pixels for the left-hand side
    /// browser pane.
    pub side_browser_px: f32,
    /// Target width in screen-space pixels for the right-hand
    /// inspector pane.
    pub right_inspector_px: f32,
}

impl Default for DockSizes {
    fn default() -> Self {
        Self {
            side_browser_px: 280.0,
            right_inspector_px: 320.0,
        }
    }
}

/// Saved position of a tab in the dock — opaque to callers.
/// Returned by [`WorkbenchLayout::move_tab_next_to`] and passed to
/// [`WorkbenchLayout::restore_tab_to`] to move a tab back where it
/// was before a demo / programmatic rearrangement.
#[derive(Clone, Copy, Debug)]
pub struct TabLocation {
    surface: egui_dock::SurfaceIndex,
    node: egui_dock::NodeIndex,
    index: egui_dock::TabIndex,
}

/// Screen-space rects of named UI landmarks, refreshed each frame
/// by whoever draws them. Read by feature-tour overlays (e.g. the
/// Modelica help tour) to spotlight a real widget instead of a
/// hand-drawn picture.
///
/// Convention: short stable keys like `"menu.file"`, `"menu.help"`,
/// `"toolbar.run"`. A missing key just means the widget wasn't
/// painted this frame (panel closed, perspective inactive); the
/// overlay falls back to a centred callout.
#[derive(Resource, Default, Debug, Clone)]
pub struct HelpAnchors {
    /// Frame-counter or similar staleness gate is unnecessary —
    /// readers always check the current frame's data after the
    /// writers have run (overlay renders late in the same pass).
    rects: std::collections::HashMap<String, bevy_egui::egui::Rect>,
}

impl HelpAnchors {
    /// Publish a widget's screen rect under `key`. Called from any
    /// UI render fn after laying the widget out (response.rect).
    pub fn set(&mut self, key: impl Into<String>, rect: bevy_egui::egui::Rect) {
        self.rects.insert(key.into(), rect);
    }

    /// Read the most recent rect under `key`, if any.
    pub fn get(&self, key: &str) -> Option<bevy_egui::egui::Rect> {
        self.rects.get(key).copied()
    }

    /// Drop every recorded rect — done once per frame at the start
    /// of the egui pass so stale rects from a closed panel don't
    /// linger as overlay targets.
    pub fn clear(&mut self) {
        self.rects.clear();
    }
}
pub use files_panel::{FilesPanel, FILES_PANEL_ID};
pub use uri::{UriClicked, UriHandler, UriRegistry, UriResolution};
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

/// Bring a registered singleton panel forward in the dock.
///
/// `id` is matched against [`Panel::id`]'s static string (e.g.
/// `"modelica_experiments"`, `"modelica_telemetry"`). No-op when the
/// panel isn't currently in the dock — callers that need to *open*
/// a closed panel should use the View-menu route or fire the
/// existing perspective preset.
///
/// Exposed as a typed command so HTTP automation can deterministically
/// reach a tab before screenshotting / driving it.
#[Command(default)]
pub struct FocusPanel {
    /// The singleton panel's [`PanelId`] string (e.g.
    /// `"modelica_experiments"`).
    pub id: String,
}

#[on_command(FocusPanel)]
fn on_focus_panel(
    trigger: On<FocusPanel>,
    mut layout: ResMut<WorkbenchLayout>,
) {
    let want = trigger.event().id.as_str();
    // PanelId wraps `&'static str`; we can't construct one from a
    // runtime String, so probe each tab in the dock and match by
    // value.
    let mut hit: Option<PanelId> = None;
    for (_, t) in layout.dock.iter_all_tabs() {
        if let TabId::Singleton(pid) = t {
            if pid.0 == want {
                hit = Some(*pid);
                break;
            }
        }
    }
    if let Some(pid) = hit {
        let ok = layout.focus_singleton(pid);
        bevy::log::info!(
            "[FocusPanel] id={:?} focus_singleton -> {}",
            want, ok
        );
    } else {
        // Not in dock yet — look up the registered panel and insert
        // it at its default slot, then focus.
        let registered: Option<(PanelId, PanelSlot)> = layout
            .panels
            .iter()
            .find(|(pid, _)| pid.0 == want)
            .map(|(pid, p)| (*pid, p.default_slot()));
        if let Some((pid, slot)) = registered {
            let inserted = layout.insert_panel_into_dock(pid, slot);
            let focused = layout.focus_singleton(pid);
            bevy::log::info!(
                "[FocusPanel] id={:?} inserted={} focused={}",
                want, inserted, focused
            );
        } else {
            bevy::log::warn!(
                "FocusPanel: no singleton panel registered with id {:?}; available={:?}",
                want,
                layout.panels.keys().map(|p| p.0).collect::<Vec<_>>()
            );
        }
    }
}

register_commands!(on_focus_panel,);
pub use perspective::{Perspective, PerspectiveId};
// The session binding (WorkspaceResource, WorkspacePlugin, add/close events)
// lives in `lunco-workspace` now — consumers import it from there directly.
// `session` here is just the workbench-side recents persistence.
use lunco_workspace::WorkspaceResource;
pub use viewport::{
    PanelRect, PanelRects, ViewportPanel, ViewportPlaceholder, WorkbenchEguiHost,
    WorkbenchSceneCamera, WorkbenchViewportCamera, WorkbenchViewportPlugin, VIEWPORT_PANEL_ID,
};

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
        // Survive transient GPU validation errors (e.g. the Windows
        // window-resize depth/color size mismatch) instead of panicking the
        // render thread. No-op when there's no RenderApp (headless/API-only).
        // The companion backend preference lives in
        // `preferred_wgpu_settings()`, which each binary feeds into its
        // `RenderPlugin` at `DefaultPlugins` build time.
        render_robustness::install_wgpu_error_handler(app);
        if !app.is_plugin_added::<bevy_egui::EguiPlugin>() {
            app.add_plugins(bevy_egui::EguiPlugin::default());
        }
        // Egui host + viewport-rect sync + invariant sentinels.
        // See `viewport.rs` doc-comment for the architecture (why we
        // confine the 3D camera to the panel rect instead of letting it
        // own the full window). Auto-added so hosts don't have to
        // remember to wire it up.
        if !app.is_plugin_added::<viewport::WorkbenchViewportPlugin>() {
            app.add_plugins(viewport::WorkbenchViewportPlugin);
        }
        if !app.is_plugin_added::<lunco_theme::ThemePlugin>() {
            app.add_plugins(lunco_theme::ThemePlugin);
        }
        // Workspace (editor session) resource + event observers. Lives in
        // `lunco-workspace` (bevy ECS substrate, no UI) so headless tests /
        // API-only servers that don't want the full dock shell can install
        // it directly. The workbench adds the recents-persistence sidecar on
        // top (config-dir I/O, which the headless crate deliberately omits).
        if !app.is_plugin_added::<lunco_workspace::WorkspacePlugin>() {
            app.add_plugins(lunco_workspace::WorkspacePlugin);
        }
        app.add_plugins(session::RecentsPlugin);
        // Cross-cutting status bus. Subsystems publish events here;
        // renderers (status bar, console fan-out, diagnostics fan-out)
        // are added separately by their owning plugins.
        if !app.is_plugin_added::<status_bus::StatusBusPlugin>() {
            app.add_plugins(status_bus::StatusBusPlugin);
        }
        // Perf HUD (FPS / frame ms / optional physics ms) wired into
        // the right end of the status bar. Off by default; flip via
        // the `TogglePerfHud` typed command.
        if !app.is_plugin_added::<perf_hud::PerfHudPlugin>() {
            app.add_plugins(perf_hud::PerfHudPlugin);
        }
        if !app.is_plugin_added::<theme_command::ThemeCommandPlugin>() {
            app.add_plugins(theme_command::ThemeCommandPlugin);
        }
        if !app.is_plugin_added::<window_command::WindowCommandPlugin>() {
            app.add_plugins(window_command::WindowCommandPlugin);
        }
        if !app.is_plugin_added::<perspective_command::PerspectiveCommandPlugin>() {
            app.add_plugins(perspective_command::PerspectiveCommandPlugin);
        }
        // Persist & restore primary-window geometry (size / position /
        // maximized) via `lunco-settings`. Native-only; no-op on wasm.
        if !app.is_plugin_added::<window_persistence::WindowPersistencePlugin>() {
            app.add_plugins(window_persistence::WindowPersistencePlugin);
        }
        // Per-Twin (per-project) volatile UI state — active perspective +
        // open-document list — keyed by Twin path, VSCode `workspaceStorage`
        // style. Needs `WorkbenchLayout`, so it lives here, not in the
        // headless `WorkspacePlugin`.
        if !app.is_plugin_added::<workspace_state::WorkspaceStatePlugin>() {
            app.add_plugins(workspace_state::WorkspaceStatePlugin);
        }
        // Plugin-driven registry of `DocumentKind`s. Domain crates
        // (modelica, future julia/usd/sysml/...) register their kinds
        // here; consumers iterate the registry rather than matching
        // a fixed enum. Idempotent — domain plugins can also call
        // `init_resource::<DocumentKindRegistry>()` themselves.
        if !app
            .is_plugin_added::<lunco_twin::DocumentKindRegistryPlugin>()
        {
            app.add_plugins(lunco_twin::DocumentKindRegistryPlugin);
        }
        // Native (rfd) / web (FSA, future) file-picker plumbing.
        // Domain code fires `picker::PickHandle` and observes
        // `picker::PickResolved` without caring which backend is live.
        if !app.is_plugin_added::<picker::PickerPlugin>() {
            app.add_plugins(picker::PickerPlugin);
        }
        // Shell-level file-workflow commands (`OpenFile`, `OpenFolder`,
        // `OpenTwin`, `SaveAll`, `SaveAsTwin`) + the picker→command
        // routing observer. Domain crates contribute their own
        // observers for verbs that need domain-specific handling
        // (e.g. modelica's `on_open_file` reads `.mo` content).
        if !app.is_plugin_added::<file_ops::FileOpsPlugin>() {
            app.add_plugins(file_ops::FileOpsPlugin);
        }
        if !app.is_plugin_added::<perspective_help::PerspectiveHelpPlugin>() {
            app.add_plugins(perspective_help::PerspectiveHelpPlugin);
        }
        app.init_resource::<WorkbenchLayout>()
            .init_resource::<HelpAnchors>()
            .init_resource::<DockSizes>()
            .init_resource::<PendingTabCloses>()
            // Twin Browser plumbing — resources are always present so
            // the panel renders an empty state cleanly when no Twin is
            // open and no domain sections have registered yet. The
            // active Twin is tracked on `WorkspaceResource` (installed
            // by `WorkspacePlugin` above), not a panel-local resource.
            .init_resource::<BrowserSectionRegistry>()
            .init_resource::<BrowserActions>()
            .init_resource::<UnsavedDocs>()
            // Cross-domain URI registry. Starts empty; each domain
            // plugin (lunco-modelica, a future lunco-usd, …) pushes
            // its own handler on build. See `uri.rs` for the trait.
            .init_resource::<UriRegistry>()
            .add_observer(on_open_tab)
            .add_observer(on_close_tab);
        register_all_commands(app);
        app
            .add_systems(
                EguiPrimaryContextPass,
                render_workbench.in_set(WorkbenchRenderSet),
            )
            // Scene picking is handled by bevy_picking (egui occlusion via
            // bevy_egui's picking backend) — no scene-pointer resource, no gate.
            .add_systems(bevy::prelude::Update, maintain_dock_widths);

        // Built-in Files section ships with the workbench so apps get
        // a usable browser even before any domain plugin registers.
        // Registered after init_resource so the registry definitely
        // exists. Domain crates push their sections (Modelica, USD, …)
        // from their own plugin's build, which runs after ours.
        app.world_mut()
            .resource_mut::<BrowserSectionRegistry>()
            .register(FilesSection::default());
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

    /// App-wide Edit menu contributions. Same pattern as
    /// [`settings_menu`](Self::settings_menu) — domain plugins push a
    /// closure via [`WorkbenchLayout::register_edit_menu`] at Startup so
    /// the global Edit menu can host domain-specific verbs (e.g. the
    /// code editor's Cut/Copy/Paste) without each plugin scattering its
    /// own toolbar.
    pub(crate) edit_menu:
        Vec<Box<dyn Fn(&mut bevy_egui::egui::Ui, &mut World) + Send + Sync>>,

    /// App-wide Help menu contributions. Same pattern as
    /// [`settings_menu`](Self::settings_menu) — domain plugins push a
    /// closure via [`WorkbenchLayout::register_help_menu`] at Startup
    /// so the Help drop-down can host tour / docs / about entries
    /// without each domain inventing its own help button.
    pub(crate) help_menu:
        Vec<Box<dyn Fn(&mut bevy_egui::egui::Ui, &mut World, &WorkbenchLayout) + Send + Sync>>,

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

    /// `true` when nothing is queued. Used by close-flow finalizers
    /// to detect whether the per-tab close pipeline has fully drained.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
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
            edit_menu: Vec::new(),
            help_menu: Vec::new(),
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

    /// Move an already-open instance tab to position 0 in its leaf so
    /// it renders as the leftmost tab. No-op if the tab isn't open.
    pub fn move_instance_to_front(&mut self, kind: PanelId, instance: u64) {
        let tab = TabId::Instance { kind, instance };
        let Some((surface, node, tab_idx)) = self.dock.find_tab(&tab) else {
            return;
        };
        if tab_idx.0 == 0 {
            return;
        }
        let surface_ref = self.dock.get_surface_mut(surface).and_then(|s| s.node_tree_mut());
        let Some(tree) = surface_ref else { return };
        if let Some(removed) = tree[node].remove_tab(tab_idx) {
            tree[node].insert_tab(egui_dock::TabIndex(0), removed);
            tree.set_active_tab(node, egui_dock::TabIndex(0));
        }
    }

    /// Opaque handle to a tab's position in the dock — surface,
    /// node, index. Returned by [`Self::move_tab_next_to`] so callers
    /// can restore the tab to its original spot later.
    ///
    /// Wrapper around egui_dock's internal indices; treat as
    /// round-trip only (don't compare across frames where the dock
    /// has been rebuilt).
    /// Move `src` to a fresh split-leaf alongside `sibling_of` so the
    /// two panels render **side-by-side**, not as tabs of the same
    /// strip. Returns the source's original [`TabLocation`] so
    /// callers can restore later, or `None` if either tab isn't in
    /// the dock. No-op when they're already in the same node.
    ///
    /// Splits 50/50 to the right of `sibling_of`'s node. egui_dock
    /// auto-collapses the source leaf if removing the tab leaves it
    /// empty.
    pub fn move_tab_next_to(
        &mut self,
        src: TabId,
        sibling_of: TabId,
    ) -> Option<TabLocation> {
        let src_loc = self.dock.find_tab(&src)?;
        let (t_surface, t_node, _) = self.dock.find_tab(&sibling_of)?;
        if src_loc.0 == t_surface && src_loc.1 == t_node {
            return Some(TabLocation {
                surface: src_loc.0,
                node: src_loc.1,
                index: src_loc.2,
            });
        }
        let saved = TabLocation {
            surface: src_loc.0,
            node: src_loc.1,
            index: src_loc.2,
        };
        self.dock.move_tab(
            src_loc,
            egui_dock::TabDestination::Node(
                t_surface,
                t_node,
                egui_dock::TabInsert::Split(egui_dock::Split::Right),
            ),
        );
        Some(saved)
    }

    /// Move `src` back to a saved [`TabLocation`]. No-op if `src`
    /// isn't in the dock or the destination node no longer exists
    /// (e.g. it was collapsed when a sibling was closed).
    pub fn restore_tab_to(&mut self, src: TabId, loc: TabLocation) {
        let Some(src_loc) = self.dock.find_tab(&src) else {
            return;
        };
        if (src_loc.0, src_loc.1) == (loc.surface, loc.node) {
            return;
        }
        // Validate the destination still exists and is a leaf.
        let dest_ok = self
            .dock
            .get_surface(loc.surface)
            .and_then(|s| s.node_tree())
            .map(|tree| {
                loc.node.0 < tree.len()
                    && tree[loc.node].is_leaf()
            })
            .unwrap_or(false);
        if !dest_ok {
            return;
        }
        let count = self
            .dock
            .get_surface(loc.surface)
            .and_then(|s| s.node_tree())
            .map(|tree| tree[loc.node].tabs_count())
            .unwrap_or(0);
        let idx = egui_dock::TabIndex(loc.index.0.min(count));
        self.dock.move_tab(
            src_loc,
            egui_dock::TabDestination::Node(
                loc.surface,
                loc.node,
                egui_dock::TabInsert::Insert(idx),
            ),
        );
    }

    /// Find the first tab matching the given instance-kind, returning
    /// the typed [`TabId`]. Useful when callers know the kind but not
    /// the instance id (e.g. demo-tour "move the plot tab").
    pub fn find_any_instance(&self, kind: PanelId) -> Option<TabId> {
        for (_, t) in self.dock.iter_all_tabs() {
            if let TabId::Instance { kind: k, instance } = t {
                if *k == kind {
                    return Some(TabId::Instance {
                        kind: *k,
                        instance: *instance,
                    });
                }
            }
        }
        None
    }

    /// Rewrite the side-browser and right-inspector split fractions
    /// so the panes occupy a fixed absolute pixel width regardless
    /// of the current window size. Driven by [`maintain_dock_widths`]
    /// on `WindowResized`.
    ///
    /// Relies on the dock topology that [`rebuild_dock`] produces:
    /// - if `side_browser` non-empty, root is the side-left split.
    /// - if `right_inspector` non-empty, the right-inspector split
    ///   is the previous root, i.e. at `NodeIndex(2)` when wrapped
    ///   by a side-left split, or at `NodeIndex(0)` otherwise.
    pub fn enforce_widths(
        &mut self,
        window_w: f32,
        side_px: f32,
        right_px: f32,
    ) {
        // Reject non-finite inputs up front: `f32::clamp` propagates NaN, so a
        // NaN px width would be written straight into a split fraction and
        // panic egui_dock's separator layout on the next frame.
        if !window_w.is_finite() || !side_px.is_finite() || !right_px.is_finite() {
            return;
        }
        let total_w = window_w.max(100.0);
        let has_side = !self.side_browser.is_empty();
        let has_right = !self.right_inspector.is_empty();
        if !has_side && !has_right {
            return;
        }
        // `main_surface_mut()` returns `&mut Surface<Tab>` which
        // derefs to the underlying `Tree` for indexing.
        let tree = self.dock.main_surface_mut();
        if tree.len() == 0 {
            return;
        }

        // Side-browser split — the outermost (root) when present.
        if has_side {
            let f = (side_px / total_w).clamp(0.05, 0.5);
            if let egui_dock::Node::Horizontal(ref mut s) = tree[NodeIndex(0)] {
                s.fraction = f;
            }
        }

        // Right-inspector split — at NodeIndex(2) if it lives inside
        // the side-left wrap, else at root.
        if has_right {
            let parent_w = if has_side {
                (total_w - side_px).max(100.0)
            } else {
                total_w
            };
            let right_share = (right_px / parent_w).clamp(0.05, 0.5);
            let f = 1.0 - right_share;
            let idx = if has_side {
                NodeIndex(2)
            } else {
                NodeIndex(0)
            };
            if idx.0 < tree.len() {
                if let egui_dock::Node::Horizontal(ref mut s) = tree[idx] {
                    s.fraction = f;
                }
            }
        }
    }

    /// All instance ids of `kind` in left-to-right dock walk order.
    /// Used by VS-Code-style "Close Others / to the Right / All" tab
    /// menus, which need the visual tab sequence (a `HashMap`-backed
    /// domain registry can't supply order).
    pub fn instances_in_order(&self, kind: PanelId) -> Vec<u64> {
        self.dock
            .iter_all_tabs()
            .filter_map(|(_, t)| match t {
                TabId::Instance { kind: k, instance } if *k == kind => {
                    Some(*instance)
                }
                _ => None,
            })
            .collect()
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

    /// Register a closure that contributes entries to the global Edit
    /// menu. Mirrors [`register_settings`](Self::register_settings).
    pub fn register_edit_menu<F>(&mut self, callback: F)
    where
        F: Fn(&mut bevy_egui::egui::Ui, &mut World) + Send + Sync + 'static,
    {
        self.edit_menu.push(Box::new(callback));
    }

    /// Register a closure that contributes entries to the global Help
    /// menu. Mirrors [`register_settings`](Self::register_settings).
    pub fn register_help_menu<F>(&mut self, callback: F)
    where
        F: Fn(&mut bevy_egui::egui::Ui, &mut World, &WorkbenchLayout) + Send + Sync + 'static,
    {
        self.help_menu.push(Box::new(callback));
    }

    /// Register a perspective (named workbench layout). The first one
    /// registered becomes the active default.
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

    /// Reset the dock to a clean state by re-applying the active perspective's
    /// slot preset from scratch (or the first-registered perspective if none is
    /// active). Restores panels a stale persisted layout dropped — most
    /// importantly the 3D `ViewportPanel`, whose absence leaves the centre blank
    /// and the viewport camera inactive. Exposed as the `ResetWorkspaceLayout`
    /// command and the View ▸ "Reset Layout" menu item.
    pub fn reset_to_default_layout(&mut self) {
        let id = self
            .active_perspective
            .or_else(|| self.perspectives.first().map(|p| p.id()));
        if let Some(id) = id {
            self.activate_perspective(id);
        }
    }

    /// The `instance` discriminant of the currently *focused* tab, when
    /// it's a multi-instance tab. Document tabs open with their
    /// `DocumentId.raw()` as the instance (see `open_instance` callers),
    /// so for a focused document this is the active document's id.
    ///
    /// The dock's focused leaf is the source of truth for which tab is
    /// active — `WorkspaceResource.active_document` isn't set on every
    /// open path, so reading it here is what makes hot-exit restore the
    /// *correct* active tab. Returns `None` when the focused tab is a
    /// singleton panel (not a document) or nothing is focused.
    pub fn active_tab_instance(&self) -> Option<u64> {
        let tree = self.dock.main_surface();
        let node = tree.focused_leaf()?;
        if let egui_dock::Node::Leaf(leaf) = &tree[node] {
            if let Some(TabId::Instance { instance, .. }) = leaf.tabs.get(leaf.active.0) {
                return Some(*instance);
            }
        }
        None
    }

    /// Serialize the live dock tree (split sizes, tab arrangement, active
    /// leaf) to JSON for per-Twin hot-exit. `TabId`/`PanelId` carry serde
    /// impls (`panel.rs`); the egui_dock `serde` feature does the rest.
    /// Returns `None` if serialization fails (never expected).
    pub(crate) fn dock_json(&self) -> Option<serde_json::Value> {
        serde_json::to_value(&self.dock).ok()
    }

    /// Cheap structural hash of the dock layout for in-memory change
    /// detection — folds tab arrangement, focus, split fractions and
    /// collapse state straight into a `u64` with no serialization.
    ///
    /// The workspace hot-exit gate runs every frame; it previously
    /// serialized the whole dock to a `serde_json::Value` + `String`
    /// purely to fold into this number — JSON is an I/O-boundary tool, not
    /// the right hammer for an internal change signal (CQ-209). This walks
    /// the live `DockState` nodes with a `Hasher` instead: zero
    /// allocations, and it deliberately ignores node `rect`s — those are
    /// recomputed from the window each layout pass and aren't persisted
    /// intent, so hashing them (as the JSON did) re-fired the save on every
    /// window resize.
    pub(crate) fn dock_layout_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for node in self.dock.iter_nodes() {
            match node {
                egui_dock::Node::Empty => 0u8.hash(&mut h),
                egui_dock::Node::Leaf(leaf) => {
                    1u8.hash(&mut h);
                    leaf.tabs.len().hash(&mut h);
                    for tab in &leaf.tabs {
                        tab.hash(&mut h);
                    }
                    leaf.active.0.hash(&mut h);
                    leaf.collapsed.hash(&mut h);
                }
                egui_dock::Node::Vertical(s) => {
                    2u8.hash(&mut h);
                    s.fraction.to_bits().hash(&mut h);
                    s.fully_collapsed.hash(&mut h);
                }
                egui_dock::Node::Horizontal(s) => {
                    3u8.hash(&mut h);
                    s.fraction.to_bits().hash(&mut h);
                    s.fully_collapsed.hash(&mut h);
                }
            }
        }
        h.finish()
    }

    /// Replace the dock tree from a previously [`dock_json`](Self::dock_json)
    /// snapshot, reconciling it against *this* app's live state:
    ///
    /// - **Singleton** tabs whose `PanelId` isn't registered here are
    ///   dropped (e.g. a `sandbox`-only panel loaded into `lunica`).
    /// - **Instance** tabs are remapped: each carries the *old* session's
    ///   `DocumentId.raw()`; `id_map` translates it to the freshly-restored
    ///   id. A tab whose kind isn't registered, or whose document didn't
    ///   restore (absent from `id_map`), is dropped.
    ///
    /// Empty leaves collapse via egui_dock's `retain_tabs`. Returns `false`
    /// (leaving the current dock untouched) when the JSON won't parse or the
    /// reconciled tree would be empty — the caller then keeps whatever the
    /// codec-driven open path produced.
    pub(crate) fn set_dock_from_json(
        &mut self,
        value: serde_json::Value,
        id_map: &HashMap<(&'static str, u64), u64>,
    ) -> bool {
        use std::collections::HashSet;
        let valid_singletons: HashSet<&'static str> =
            self.panels.keys().map(|p| p.0).collect();
        let valid_kinds: HashSet<&'static str> =
            self.instance_panels.keys().map(|p| p.0).collect();

        let mut new_dock: DockState<TabId> = match serde_json::from_value(value) {
            Ok(d) => d,
            Err(e) => {
                warn!("[WorkspaceState] dock JSON parse failed: {e}; keeping default layout");
                return false;
            }
        };

        // One pass: drop unregistered-kind tabs, remap instance ids that the
        // restore reported a mapping for, and KEEP instances with no mapping
        // as-is. The "keep" case covers stable-instance tabs whose id is a
        // compile-time constant the app re-creates with the same value on
        // every launch (e.g. the default Graphs plot pinned to
        // `DEFAULT_MODELICA_GRAPH`) — dropping those would lose the plot tab.
        // A document tab whose doc failed to restore also lands here; it
        // keeps its stale id and renders empty, which is strictly better than
        // collapsing its leaf and losing the saved split sizes (and the
        // codec's own `OpenTab` re-adds the live tab alongside it).
        new_dock.retain_tabs(|tab| match tab {
            TabId::Singleton(pid) => valid_singletons.contains(pid.0),
            TabId::Instance { kind, instance } => {
                if !valid_kinds.contains(kind.0) {
                    return false;
                }
                if let Some(&new_id) = id_map.get(&(kind.0, *instance)) {
                    *instance = new_id;
                }
                true
            }
        });

        if new_dock.iter_all_tabs().next().is_none() {
            return false; // nothing survived reconciliation — keep current
        }
        // Heal any non-finite split fraction persisted to disk. egui_dock can
        // serialize a NaN fraction (see `sanitize_dock_fractions`), and a NaN
        // reloaded here would panic the dock layout on the very next frame —
        // a permanent boot-crash loop until the workspace cache is wiped.
        sanitize_dock_fractions(&mut new_dock);
        self.dock = new_dock;
        true
    }

    /// Activate a perspective by its raw string id, matching against the
    /// registered set. Returns `true` if a perspective with that id
    /// exists in this app and was activated; `false` (no-op) otherwise.
    ///
    /// The reconciliation seam for persisted state: a `PerspectiveId`
    /// holds a `&'static str` and can't be rebuilt from a runtime
    /// `String`, so restore looks the string up here and drops ids that
    /// aren't registered in the current binary (e.g. a perspective only
    /// `sandbox` ships, loaded into `lunica`).
    pub fn activate_perspective_by_str(&mut self, id: &str) -> bool {
        let found = self
            .perspectives
            .iter()
            .find(|p| p.id().as_str() == id)
            .map(|p| p.id());
        match found {
            Some(pid) => {
                self.activate_perspective(pid);
                true
            }
            None => false,
        }
    }

    /// Rebuild the dock tree from the current slot intent.
    ///
    /// Called by every slot setter and by [`activate_perspective`]. After
    /// rebuild, user drags persist until the next call.
    ///
    /// **Two-mode rendering** — the dock is only used when there are
    /// central tabs (i.e. apps like `lunica` that have
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
    /// Insert a panel into the live dock without rebuilding from
    /// scratch. Used by the View menu's panel checkbox so toggling
    /// one tab doesn't wipe instance tabs (model views, etc.) that
    /// the perspective preset doesn't track. Picks a leaf based on
    /// the panel's default slot; falls back to the focused leaf.
    /// Returns true if the panel was inserted.
    pub(crate) fn insert_panel_into_dock(&mut self, id: PanelId, slot: PanelSlot) -> bool {
        let tab = TabId::Singleton(id);
        // Already there? No-op.
        if self.dock.iter_all_tabs().any(|(_, t)| *t == tab) {
            return false;
        }
        let main = self.dock.main_surface_mut();
        // Find an existing tab in the same slot to drop next to.
        let neighbour: Option<PanelId> = match slot {
            PanelSlot::SideBrowser => self.side_browser.first().copied(),
            PanelSlot::Center => self.center.first().copied(),
            PanelSlot::RightInspector => self.right_inspector.first().copied(),
            PanelSlot::Bottom => self.bottom.first().copied(),
            PanelSlot::Floating => None,
        };
        let target_node: Option<NodeIndex> = neighbour.and_then(|nid| {
            let target_tab = TabId::Singleton(nid);
            // Walk all nodes; egui_dock's NodeIndex is opaque so we
            // probe by index until we find the leaf containing the
            // sibling tab.
            let mut found = None;
            for i in 0..256 {
                let node = NodeIndex(i);
                if let Some(node_ref) = main.iter().nth(i) {
                    if let egui_dock::Node::Leaf(leaf) = node_ref {
                        if leaf.tabs.iter().any(|t| *t == target_tab) {
                            found = Some(node);
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
            found
        });
        if let Some(node) = target_node {
            main.set_focused_node(node);
            main.push_to_focused_leaf(tab);
        } else {
            // Last resort: append to focused leaf (whatever the user
            // had focus on). Better than wiping the dock.
            main.push_to_focused_leaf(tab);
        }
        true
    }

    /// Activate (foreground) a singleton panel tab if it's already
    /// present in the dock. Returns `true` when the panel was found
    /// and focused, `false` when no leaf contains it. Idempotent —
    /// calling on the already-active tab is a no-op success.
    ///
    /// Used by the [`FocusPanel`] typed command so HTTP / scripting
    /// callers can deterministically bring a panel forward (e.g.
    /// activating Experiments before screenshotting it).
    pub fn focus_singleton(&mut self, id: PanelId) -> bool {
        let tab = TabId::Singleton(id);
        if let Some(pos) = self.dock.find_tab(&tab) {
            self.dock.set_focused_node_and_surface((pos.0, pos.1));
            self.dock.set_active_tab(pos);
            true
        } else {
            false
        }
    }

    /// Remove a panel from the live dock without rebuilding from
    /// scratch. Companion to [`insert_panel_into_dock`].
    pub(crate) fn remove_panel_from_dock(&mut self, id: PanelId) -> bool {
        let tab = TabId::Singleton(id);
        let mut removed = false;
        let main = self.dock.main_surface_mut();
        // Collect node indices to mutate.
        let mut hits: Vec<(NodeIndex, usize)> = Vec::new();
        for i in 0..256 {
            let node = NodeIndex(i);
            match main.iter().nth(i) {
                Some(egui_dock::Node::Leaf(leaf)) => {
                    for (idx, t) in leaf.tabs.iter().enumerate() {
                        if *t == tab {
                            hits.push((node, idx));
                        }
                    }
                }
                Some(_) => {}
                None => break,
            }
        }
        for (node, idx) in hits.into_iter().rev() {
            if let Some(egui_dock::Node::Leaf(leaf)) = main.iter_mut().nth(node.0) {
                if idx < leaf.tabs.len() {
                    leaf.tabs.remove(idx);
                    removed = true;
                }
            }
        }
        removed
    }

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

        // Preserve dynamically-opened instance (document/model/viz) tabs
        // across the rebuild. The skeleton below is built purely from the
        // *singleton* slot intent, so without this every instance tab —
        // open model docs, plot instances — would silently vanish on any
        // slot change or perspective switch (and the doc would be left
        // live in its registry with no visible tab). VSCode never closes
        // open editors when you change the layout; neither do we. We walk
        // the current dock, remember each instance tab (in order) and
        // which one was focused, then re-attach them via `open_instance`
        // after the skeleton is rebuilt. Tabs whose kind is no longer
        // registered are dropped.
        let preserved_instances: Vec<(PanelId, u64)> = {
            let main = self.dock.main_surface();
            let mut acc = Vec::new();
            for node in main.iter() {
                if let egui_dock::Node::Leaf(leaf) = node {
                    for tab in &leaf.tabs {
                        if let TabId::Instance { kind, instance } = tab {
                            if self.instance_panels.contains_key(kind) {
                                acc.push((*kind, *instance));
                            }
                        }
                    }
                }
            }
            acc
        };
        let active_instance: Option<(PanelId, u64)> = {
            let tree = self.dock.main_surface();
            tree.focused_leaf().and_then(|node| {
                if let egui_dock::Node::Leaf(leaf) = &tree[node] {
                    match leaf.tabs.get(leaf.active.0) {
                        Some(TabId::Instance { kind, instance })
                            if self.instance_panels.contains_key(kind) =>
                        {
                            Some((*kind, *instance))
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            })
        };

        // Viewport-only perspectives: no central singleton tabs → don't
        // build a side-panel dock tree. The renderer lays out side panels
        // with egui's SidePanels and leaves the central area transparent
        // (it stays in 3D mode — see the `has_dock_tabs` gate in
        // `render_layout` — so a non-empty dock here is *not* shown).
        //
        // A pure 3D app keeps no instance tabs at all, but a hybrid app
        // (the rover sandbox embeds the Modelica workbench) can have
        // document/model tabs open while a viewport-only perspective is
        // active. Park those instance tabs in the dock rather than dropping
        // them — wiping would lose the open documents on every viewport
        // perspective activation. They render nowhere while this
        // perspective is active and re-attach to the centre when the user
        // switches to a centre-driven perspective (which collects them as
        // `preserved_instances` on its own rebuild).
        if center_tabs.is_empty() {
            let parked: Vec<TabId> = preserved_instances
                .iter()
                .map(|(kind, instance)| TabId::Instance {
                    kind: *kind,
                    instance: *instance,
                })
                .collect();
            self.dock = DockState::new(parked);
            return;
        }

        // Centre-driven apps: build the standard cross layout in egui_dock.
        // Splits are ordered so right and left span the full window height,
        // and bottom spans the central column's width (sandwiched between
        // them). Each subsequent split at NodeIndex::root() wraps the
        // previous tree, so the outermost splits dominate the layout.
        let mut dock = DockState::new(center_tabs);
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
            // the table in the doc above. Bumped from 0.15 → 0.22 so
            // the Twin Browser shows full library names ("Modelica
            // Standard Library") without truncation at default zoom.
            let [_old_root, _left] =
                main.split_left(NodeIndex::root(), 0.22, side_browser_tabs);
        }

        let _ = central;
        self.dock = dock;

        // Re-attach the instance tabs we remembered above. `open_instance`
        // resolves each kind's preferred-slot leaf and appends there, so a
        // model doc lands back in the centre and a plot back in the bottom
        // — exactly where they were, even though the skeleton only knows
        // about singleton slots. It focuses each as it goes; we restore the
        // originally-focused instance tab last so the right one stays
        // active.
        for (kind, instance) in &preserved_instances {
            self.open_instance(*kind, *instance);
        }
        if let Some((kind, instance)) = active_instance {
            // Idempotent: the tab is already present, so this just
            // re-focuses it.
            self.open_instance(kind, instance);
        }
    }

    /// Reconcile a freshly-restored dock tree against the active
    /// perspective's declared chrome (side browser / inspectors /
    /// bottom singletons).
    ///
    /// A persisted dock can omit those panels — e.g. it was last saved
    /// while a viewport-only perspective was active (which parks only
    /// instance tabs, no chrome — see [`Self::rebuild_dock`]'s
    /// `center_tabs.is_empty()` branch), or from an older layout. In
    /// dock-mode the renderer draws the dock tree verbatim, so any
    /// missing chrome silently never appears (open documents show, but
    /// the side/right panels are gone).
    ///
    /// When the active perspective is centre-driven (it declares
    /// registered centre singletons) yet the restored dock is missing
    /// any declared chrome, rebuild the full layout from intent.
    /// [`Self::rebuild_dock`] re-attaches the open document/instance
    /// tabs, so only the saved split *sizes* are lost — not the open
    /// documents or the chrome. Viewport-only perspectives (no
    /// registered centre singleton — the sandbox's `View`) are left
    /// untouched: their chrome lives outside the dock by design.
    pub(crate) fn ensure_chrome_present(&mut self) {
        if !self.perspective_chrome_complete() {
            warn!(
                "[WorkspaceState] restored dock missing perspective chrome; \
                 rebuilding layout (open documents preserved, split sizes reset)"
            );
            self.rebuild_dock();
        }
    }

    /// True when the live dock is consistent with the active
    /// perspective's declared chrome. Either the perspective is
    /// viewport-only (declares no *registered* centre singleton — its
    /// side panels render outside the dock, so a chrome-less dock is
    /// correct), or every declared+registered chrome panel
    /// (side/right/bottom/centre singleton) is present in the dock tree.
    ///
    /// Used at both ends of persistence: [`Self::ensure_chrome_present`]
    /// heals a restored dock that fails this, and `build_state` refuses
    /// to persist a dock that fails it (so a transient chrome-less dock —
    /// e.g. mid perspective-switch through the viewport-only
    /// [`Self::rebuild_dock`] branch — never round-trips as a layout with
    /// missing side panels).
    pub(crate) fn perspective_chrome_complete(&self) -> bool {
        let is_centre_driven = self.center.iter().any(|id| self.panels.contains_key(id));
        if !is_centre_driven {
            return true; // viewport-only — chrome renders outside the dock
        }
        let in_dock: std::collections::HashSet<PanelId> = self
            .dock
            .iter_all_tabs()
            .filter_map(|(_, t)| if let TabId::Singleton(id) = t { Some(*id) } else { None })
            .collect();
        self.side_browser
            .iter()
            .chain(self.right_inspector.iter())
            .chain(self.bottom.iter())
            .chain(self.center.iter())
            .filter(|id| self.panels.contains_key(id))
            .all(|id| in_dock.contains(id))
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

    /// Register help content for a perspective.
    fn register_perspective_help(
        &mut self,
        id: PerspectiveId,
        help: PerspectiveHelp,
    ) -> &mut Self;
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

    fn register_perspective_help(
        &mut self,
        id: PerspectiveId,
        help: PerspectiveHelp,
    ) -> &mut Self {
        if !self.world().contains_resource::<PerspectiveHelpRegistry>() {
            self.init_resource::<PerspectiveHelpRegistry>();
        }
        // First registration for this id also contributes the Help-menu
        // item — so a subsystem gets both popup and menu entry from this
        // single call, with no central list to maintain.
        let is_new = self
            .world()
            .resource::<PerspectiveHelpRegistry>()
            .get(id)
            .is_none();
        self.world_mut()
            .resource_mut::<PerspectiveHelpRegistry>()
            .register(id, help);
        if is_new {
            if !self.world().contains_resource::<WorkbenchLayout>() {
                self.init_resource::<WorkbenchLayout>();
            }
            let mut layout = self.world_mut().resource_mut::<WorkbenchLayout>();
            perspective_help::register_help_menu_item(&mut layout, id);
        }
        self
    }
}

// ─────────────────────────────────────────────────────────────────────
// Renderer
// ─────────────────────────────────────────────────────────────────────

/// React to window resize events (and the very first frame) by
/// rewriting the side / right dock fractions so the panes stay at
/// their configured absolute pixel widths. Avoids a per-frame
/// pre-render adjustment — this only runs when the window
/// actually resizes.
fn maintain_dock_widths(
    mut resize_events: bevy::prelude::MessageReader<bevy::window::WindowResized>,
    mut layout: ResMut<WorkbenchLayout>,
    sizes: Res<DockSizes>,
    windows: Query<
        &bevy::window::Window,
        bevy::prelude::With<bevy::window::PrimaryWindow>,
    >,
    mut applied_once: bevy::prelude::Local<bool>,
) {
    // Latest event wins — multiple events in one frame collapse.
    let resized_w = resize_events.read().last().map(|ev| ev.width);
    let initial_w = if !*applied_once {
        windows.single().ok().map(|w| w.width())
    } else {
        None
    };
    let Some(w) = resized_w.or(initial_w) else {
        return;
    };
    layout.enforce_widths(w, sizes.side_browser_px, sizes.right_inspector_px);
    *applied_once = true;
}

/// Clamp every split fraction in `dock` (across **all** surfaces) to a finite
/// value in `(0, 1)`, replacing any non-finite fraction with `0.5`.
///
/// egui's layout asserts on NaN: a pane rect is `min + dim_size * fraction`,
/// so a single non-finite `fraction` anywhere in the tree produces a NaN
/// separator rect and aborts the process in `advance_cursor_after_rect`
/// ("rect is nan", seen on Windows).
///
/// TODO(egui_dock 0.18 — remove the per-frame call in `render_layout` when
/// this is fixed/updated upstream): egui_dock self-poisons the tree from
/// inside `show()`. In `egui_dock-0.18.0/src/widgets/dock_area/show/mod.rs`
/// the separator update runs *every* frame (not just on drag) and computes
/// `split.fraction = (split.fraction + delta / range).clamp(min, max)`. When a
/// pane is squeezed to zero width `range == 0`, so with no drag (`delta == 0`)
/// `delta / range` is `0.0 / 0.0 = NaN`, and `f32::clamp` passes NaN straight
/// through — writing NaN back into the tree. The fix belongs upstream
/// (guard `range > 0`); until then we re-assert this invariant around every
/// `show`. The load-time call in `set_dock_from_json` is independent and stays
/// regardless — it heals a NaN already serialized to disk.
fn sanitize_dock_fractions(dock: &mut DockState<TabId>) {
    for (_surface, node) in dock.iter_all_nodes_mut() {
        if let egui_dock::Node::Horizontal(s) | egui_dock::Node::Vertical(s) = node {
            s.fraction = if s.fraction.is_finite() {
                s.fraction.clamp(0.01, 0.99)
            } else {
                0.5
            };
        }
    }
}

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

    // Clear stale anchor rects at the start of each frame; menu /
    // panel writers refresh them as they render.
    if let Some(mut anchors) = world.get_resource_mut::<HelpAnchors>() {
        anchors.clear();
    }

    let theme = world.resource::<lunco_theme::Theme>().clone();

    // Apply theme to the egui ctx itself (not just per-ui) — the
    // menu bar, status bar, and any other `TopBottomPanel`/`SidePanel`
    // paint their frame from `ctx.style().visuals.panel_fill`
    // *before* running the user closure, so a per-ui
    // `style_mut().visuals = …` assignment lands too late and leaves
    // chrome panels unstyled (dark in Light mode, etc.). Setting
    // visuals on the ctx fixes every chrome panel in one shot.
    ctx.set_visuals(theme.to_visuals());

    render_layout(&ctx, &mut layout, world, &theme);

    world.insert_resource(layout);
    // No scene-pointer gate is computed here: scene picking is bevy_picking-driven
    // and egui occlusion is handled by bevy_egui's picking backend.
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
        // Publish this panel's rect so feature-tour overlays can
        // spotlight it by id (`panel.<panel_id>`). Done before
        // render so even early-returning panels still register an
        // anchor for the current frame.
        let panel_rect = ui.max_rect();
        let panel_key = match *tab {
            TabId::Singleton(id) => Some(format!("panel.{}", id.as_str())),
            TabId::Instance { kind, .. } => Some(format!("panel.{}", kind.as_str())),
        };
        if let (Some(mut a), Some(k)) = (
            self.world.get_resource_mut::<HelpAnchors>(),
            panel_key,
        ) {
            a.set(k, panel_rect);
        }

        match *tab {
            TabId::Singleton(id) => {
                // Take-and-return pattern so the panel can itself borrow
                // other panels' metadata via the layout (future-proof).
                if let Some(mut panel) = self.panels.remove(&id) {
                    // Capability-narrowed context (no raw &mut World).
                    // Mutations the panel emits are queued and applied
                    // after paint (WP-8 structural prevention).
                    let mut ctx = PanelCtx::new(self.world);
                    panel.render(ui, &mut ctx);
                    let deferred = ctx.into_deferred();
                    self.panels.insert(id, panel);
                    for f in deferred {
                        f(self.world);
                    }
                } else {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        format!("Panel `{}` not registered", id.as_str()),
                    );
                }
            }
            TabId::Instance { kind, instance } => {
                if let Some(mut panel) = self.instance_panels.remove(&kind) {
                    let mut ctx = PanelCtx::new(self.world);
                    panel.render(ui, &mut ctx, instance);
                    let deferred = ctx.into_deferred();
                    self.instance_panels.insert(kind, panel);
                    for f in deferred {
                        f(self.world);
                    }
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

    /// Disable egui_dock's per-tab ScrollArea wrapper. Panels that
    /// need scrolling (code editor, docs view, telemetry lists) own
    /// their own ScrollArea internally; the dock-level wrapper would
    /// otherwise pull panel-local toolbars / sticky headers into the
    /// scrollable region and hide them when the body scrolls.
    fn scroll_bars(&self, _tab: &Self::Tab) -> [bool; 2] {
        [false, false]
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

    fn context_menu(
        &mut self,
        ui: &mut egui::Ui,
        tab: &mut Self::Tab,
        _surface: egui_dock::SurfaceIndex,
        _node: egui_dock::NodeIndex,
    ) {
        // Domain hook: dispatch to the registered InstancePanel so it
        // can draw its own menu items (Pin, Open in new view, …).
        // Singletons and unknown-kind instance tabs get no extras —
        // egui_dock still surfaces its built-in "Close" item below.
        if let TabId::Instance { kind, instance } = *tab {
            // Take the panel out so it can mutably borrow `self.world`
            // freely while drawing its menu, then put it back. Mirrors
            // how `tab_ui` swaps panels in/out for render to dodge the
            // self-borrow conflict.
            if let Some(mut panel) = self.instance_panels.remove(&kind) {
                let mut ctx = PanelCtx::new(self.world);
                panel.tab_context_menu(ui, &mut ctx, instance);
                let deferred = ctx.into_deferred();
                self.instance_panels.insert(kind, panel);
                for f in deferred {
                    f(self.world);
                }
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
    // ── Edge resize (custom-decorations only) ───────────────────────
    // Bevy's `decorations: false` strips the WM resize handles too, so
    // we re-implement them: when the pointer hovers an N-pixel border,
    // swap the cursor to the right resize icon and forward press to
    // winit's `start_drag_resize`. Skipped on macOS, where the OS
    // titlebar still owns the window frame.
    #[cfg(not(target_os = "macos"))]
    {
        const RESIZE_BORDER: f32 = 6.0;
        let screen = ctx.content_rect();
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        if let Some(p) = pointer {
            let dx = if p.x < screen.left() + RESIZE_BORDER { -1 }
                else if p.x > screen.right() - RESIZE_BORDER { 1 } else { 0 };
            let dy = if p.y < screen.top() + RESIZE_BORDER { -1 }
                else if p.y > screen.bottom() - RESIZE_BORDER { 1 } else { 0 };
            use bevy::math::CompassOctant;
            let dir = match (dx, dy) {
                (-1, -1) => Some(CompassOctant::NorthWest),
                ( 0, -1) => Some(CompassOctant::North),
                ( 1, -1) => Some(CompassOctant::NorthEast),
                ( 1,  0) => Some(CompassOctant::East),
                ( 1,  1) => Some(CompassOctant::SouthEast),
                ( 0,  1) => Some(CompassOctant::South),
                (-1,  1) => Some(CompassOctant::SouthWest),
                (-1,  0) => Some(CompassOctant::West),
                _ => None,
            };
            if let Some(dir) = dir {
                ctx.set_cursor_icon(match dir {
                    CompassOctant::North => egui::CursorIcon::ResizeNorth,
                    CompassOctant::South => egui::CursorIcon::ResizeSouth,
                    CompassOctant::East => egui::CursorIcon::ResizeEast,
                    CompassOctant::West => egui::CursorIcon::ResizeWest,
                    CompassOctant::NorthEast => egui::CursorIcon::ResizeNorthEast,
                    CompassOctant::NorthWest => egui::CursorIcon::ResizeNorthWest,
                    CompassOctant::SouthEast => egui::CursorIcon::ResizeSouthEast,
                    CompassOctant::SouthWest => egui::CursorIcon::ResizeSouthWest,
                });
                if ctx.input(|i| i.pointer.primary_pressed()) {
                    if let Ok(mut w) = world
                        .query_filtered::<&mut bevy::window::Window, bevy::prelude::With<bevy::window::PrimaryWindow>>()
                        .single_mut(world)
                    {
                        w.start_drag_resize(dir);
                    }
                }
            }
        }
    }

    // ── Opaque-mode backdrop (must run first) ───────────────────────
    // Paint `get_panel_backdrop(theme)` on the background layer BEFORE
    // any panel shapes. egui draws within a layer in shape-issue order,
    // so a rect_filled issued AFTER the menu bar / dock / status bar
    // would paint over them — exactly the "invisible menu" regression
    // the opaque-backdrop change once introduced. Running it first
    // keeps the fill underneath.
    //
    // The trigger is "are there dock tabs?", not "any registered panel
    // transparent?". The latter included transparent side-panels
    // (Inspector, Spawn Palette, …) registered globally but unused in
    // the current perspective, suppressing the backdrop incorrectly
    // and letting the 3D camera bleed through Welcome in
    // modelica_analyze. The dock-tabs check matches the 3D-app vs
    // dock-app branch below — dock mode wants an opaque backdrop;
    // 3D-app mode leaves the centre transparent for Bevy to render
    // through.
    // Backdrop strategy (egui paints over the 3D framebuffer, alpha-
    // blended; only Camera3d's viewport rect is left transparent so
    // 3D shows). Three cases:
    //   - View (empty layout)  → no backdrop. Camera3d paints full
    //     window; chrome (menu/status) overpaints on top.
    //   - Design (no ViewportPanel) → full-window backdrop. Camera3d
    //     is inactive; backdrop fills the framebuffer so no garbage.
    //   - Build (ViewportPanel in layout) → backdrop EVERYWHERE
    //     EXCEPT the ViewportPanel rect. Painted as four strips
    //     around the rect so the dock-leaf gaps (tab-strip header
    //     above the panel, padding below) match theme instead of
    //     showing uncleared framebuffer pixels as a black hole.
    // Only Design (chrome but no ViewportPanel) needs a full-window
    // backdrop to fill the framebuffer — Camera3d is inactive there.
    // View and Build both keep Camera3d running full-window; egui
    // chrome opaquely overlays where panels are and the rest stays
    // transparent so 3D shows through (including dock-leaf gaps).
    // An active placeholder message means the scene is empty — and so the USD
    // avatar `Camera3d` was despawned. View mode (empty layout) normally skips
    // the backdrop because `Camera3d` paints the full window; with no camera
    // that assumption breaks and the *last rendered frame* (stale rovers) would
    // show through. Treat "empty viewport, no camera" like the Design-mode
    // inactive-camera case and fill the framebuffer too. Painted here (before
    // the menu/status panels) so it stays on the background layer *under* the
    // chrome — painting it after the panels would overdraw them.
    let viewport_empty = world
        .get_resource::<viewport::ViewportPlaceholder>()
        .is_some_and(|p| p.message.is_some());
    let needs_full_backdrop = (!viewport::layout_is_empty(layout)
        && !viewport::layout_contains_panel(layout, viewport::VIEWPORT_PANEL_ID))
        || viewport_empty;
    if needs_full_backdrop {
        let painter = ctx.layer_painter(egui::LayerId::background());
        painter.rect_filled(ctx.content_rect(), 0.0, get_panel_backdrop(theme));
    }

    // ── Menu bar ────────────────────────────────────────────────────
    // Doubles as the OS title bar (window chrome is disabled in the
    // binary's `Window` setup — see `lunica.rs`). Bare
    // areas of the row drag the window; double-click toggles maximize;
    // window control buttons (─ ▢ ✕) sit on the far right on
    // Linux/Windows. macOS keeps native traffic lights — we just inset
    // the menu past them.
    egui::TopBottomPanel::top("lunco_workbench_menu_bar")
        // Match the dock tab-bar height so the merged title-bar
        // doesn't read as a thin sliver above thicker rows below.
        // 30px is roughly egui_dock's default tab strip height with
        // our font scale.
        .exact_height(30.0)
        .show(ctx, |ui| {
        ui.style_mut().visuals = theme.to_visuals();

        // Drag region must be registered BEFORE the menu buttons so
        // egui's last-wins hit-testing lets buttons capture clicks
        // over their own area while bare gaps drag the OS window.
        let drag_resp = ui.interact(
            ui.max_rect(),
            ui.id().with("titlebar_drag"),
            egui::Sense::click_and_drag(),
        );
        if drag_resp.drag_started() {
            // start_drag_move() must be called on the live Window
            // component synchronously with the press event — routing
            // through a command would defer it past the press and
            // winit would refuse the drag. Direct mutation is the
            // right call here.
            if let Ok(mut w) = world
                .query_filtered::<&mut bevy::window::Window, bevy::prelude::With<bevy::window::PrimaryWindow>>()
                .single_mut(world)
            {
                w.start_drag_move();
            }
        }
        if drag_resp.double_clicked() {
            world.trigger(window_command::MaximizeWindow { maximized: None });
        }

        // Window title — painted centered behind the menu/control rows
        // (purely visual, doesn't intercept clicks). Read straight off
        // the primary Bevy window so the binary stays the source of
        // truth for what the bar advertises (e.g. listening port).
        let title = world
            .query_filtered::<&bevy::window::Window, bevy::prelude::With<bevy::window::PrimaryWindow>>()
            .single(world)
            .ok()
            .map(|w| w.title.clone())
            .unwrap_or_default();
        if !title.is_empty() {
            ui.painter().text(
                ui.max_rect().center(),
                egui::Align2::CENTER_CENTER,
                &title,
                egui::FontId::proportional(12.0),
                theme.tokens.text_subdued,
            );
        }

        // `ui.horizontal` defaults to top-aligned cross-axis; with the
        // menu bar bumped to 30px the buttons would stick to the top
        // edge. Explicit `Align::Center` keeps them vertically centred
        // in the bar.
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            // Collected screen-rects of the menu buttons + transport
            // controls. Published to `HelpAnchors` after this layout
            // closure finishes so we don't double-borrow `world`
            // while the menu_button closures already hold it.
            let mut anchor_rects: Vec<(&'static str, egui::Rect)> = Vec::new();
            anchor_rects.push(("menu.bar", ui.max_rect()));

            // macOS: leave room for the native traffic lights that
            // float over our content because of `fullsize_content_view`.
            #[cfg(target_os = "macos")]
            ui.add_space(78.0);
            let r_file = ui.menu_button("File", |ui| {
                // Active doc gates Save / Save As / Close — there's
                // nothing to save when no document is focused.
                let active_doc =
                    world.resource::<WorkspaceResource>().active_document;
                let has_active = active_doc.is_some();

                // -- New ----------------------------------------------
                // Submenu populated from `DocumentKindRegistry`. Each
                // entry fires `NewDocument { kind }`; the matching
                // domain observer creates the doc. Ctrl+N fires the
                // default-resolution path through `EditorIntent`.
                ui.menu_button("New", |ui| {
                    let registry = world
                        .resource::<lunco_twin::DocumentKindRegistry>();
                    let mut entries: Vec<(String, String)> = registry
                        .iter()
                        .filter(|(_, m)| m.can_create_new)
                        .map(|(id, m)| (id.as_str().to_string(), m.display_name.clone()))
                        .collect();
                    entries.sort_by(|a, b| a.1.cmp(&b.1));
                    if entries.is_empty() {
                        ui.label(
                            egui::RichText::new("(no document kinds registered)")
                                .weak()
                                .italics(),
                        );
                    } else {
                        for (kind, display) in entries {
                            // Ctrl+N hint shown only on the first
                            // entry — that's the keybind's default
                            // target. egui menus right-align after \t.
                            let label = format!("{display}\tCtrl+N");
                            if ui.button(label).clicked() {
                                world.trigger(file_ops::NewDocument { kind });
                                ui.close();
                            }
                        }
                    }
                });
                ui.separator();

                // -- Open ---------------------------------------------
                if ui.button("Open File…\tCtrl+O").clicked() {
                    world.trigger(file_ops::ShowOpenFilePicker {});
                    ui.close();
                }
                // Open Folder + Recents are native-only for now.
                //
                // TODO(wasm): the browser has no folder picker that
                // hands back a usable path (`webkitdirectory` only
                // exposes loose files, not a writable Twin root), and
                // recents are persisted to `~/.lunco/recents.json` —
                // there is no home dir on wasm and recorded paths
                // can't be re-read (no filesystem, picked content is
                // consumed once). Wasm equivalents need a directory
                // picker via the File-System-Access API and recents
                // backed by localStorage / IndexedDB.
                #[cfg(not(target_arch = "wasm32"))]
                {
                    // Open Folder auto-classifies on the resolved path
                    // — `twin.toml` present routes to Twin mode,
                    // absence gives a plain folder workspace. The
                    // strict-mode `OpenTwin` typed command remains
                    // available to recents/HTTP/scripts that want
                    // explicit Twin semantics, but isn't worth a
                    // separate menu entry.
                    if ui.button("Open Folder/Twin…").clicked() {
                        world.trigger(file_ops::ShowOpenFolderPicker {});
                        ui.close();
                    }

                    // -- Recents ------------------------------------
                    // Twin folders and loose files have separate
                    // lists per VS Code precedent — recently-edited
                    // files within a Twin shouldn't crowd out the
                    // much-shorter list of recently-opened projects.
                    // Persisted to `~/.lunco/recents.json`
                    // (cross-platform) by `WorkspacePlugin`.
                    let (recent_twins, recent_files) = {
                        let ws = world.resource::<WorkspaceResource>();
                        (
                            ws.recents.twin_paths.clone(),
                            ws.recents.loose_paths.clone(),
                        )
                    };
                    ui.add_enabled_ui(!recent_twins.is_empty(), |ui| {
                        ui.menu_button("Open Recent Twin", |ui| {
                            for path in &recent_twins {
                                let label = path
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or_else(|| path.to_str().unwrap_or("(invalid)"));
                                if ui
                                    .button(label)
                                    .on_hover_text(path.display().to_string())
                                    .clicked()
                                {
                                    world.trigger(file_ops::OpenTwin {
                                        path: path.display().to_string(),
                                    });
                                    ui.close();
                                }
                            }
                        });
                    });
                    ui.add_enabled_ui(!recent_files.is_empty(), |ui| {
                        ui.menu_button("Open Recent File", |ui| {
                            for path in &recent_files {
                                let label = path
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or_else(|| path.to_str().unwrap_or("(invalid)"));
                                if ui
                                    .button(label)
                                    .on_hover_text(path.display().to_string())
                                    .clicked()
                                {
                                    world.trigger(file_ops::OpenFile {
                                        path: path.display().to_string(),
                                    });
                                    ui.close();
                                }
                            }
                        });
                    });
                }
                ui.separator();

                // -- Save ---------------------------------------------
                // Save / Save As route through `EditorIntent` so the
                // menu, Ctrl+S, and HTTP API funnel through the same
                // domain resolver.
                if ui
                    .add_enabled(has_active, egui::Button::new("Save\tCtrl+S"))
                    .clicked()
                {
                    world.trigger(lunco_doc_bevy::EditorIntent::Save);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        has_active,
                        egui::Button::new("Save As…\tCtrl+Shift+S"),
                    )
                    .clicked()
                {
                    world.trigger(lunco_doc_bevy::EditorIntent::SaveAs);
                    ui.close();
                }
                if ui.button("Save All").clicked() {
                    world.trigger(file_ops::SaveAll {});
                    ui.close();
                }
                if ui.button("Save as Twin…").clicked() {
                    world.trigger(file_ops::SaveAsTwin {
                        folder: String::new(),
                    });
                    ui.close();
                }
                ui.separator();

                // -- Share --------------------------------------------
                // Copy a link that encodes the active model's source in
                // the URL fragment — opening it elsewhere recreates the
                // model. Behaviour lives in the domain crate
                // (lunco-modelica observes `CopyShareLink`).
                if ui
                    .add_enabled(has_active, egui::Button::new("Copy Share Link"))
                    .on_hover_text(
                        "Copy a URL that encodes this model's source — \
                         anyone who opens it gets the model (nothing is uploaded)",
                    )
                    .on_disabled_hover_text("Copy Share Link — no active document")
                    .clicked()
                {
                    world.trigger(file_ops::CopyShareLink {});
                    ui.close();
                }
                ui.separator();

                // -- Close --------------------------------------------
                if ui
                    .add_enabled(has_active, egui::Button::new("Close\tCtrl+W"))
                    .clicked()
                {
                    world.trigger(lunco_doc_bevy::EditorIntent::Close);
                    ui.close();
                }
            });
            anchor_rects.push(("menu.file", r_file.response.rect));
            let r_edit = ui.menu_button("Edit", |ui| {
                let has_active = world
                    .resource::<WorkspaceResource>()
                    .active_document
                    .is_some();
                if ui
                    .add_enabled(has_active, egui::Button::new("Undo\tCtrl+Z"))
                    .clicked()
                {
                    world.trigger(lunco_doc_bevy::EditorIntent::Undo);
                    ui.close();
                }
                if ui
                    .add_enabled(
                        has_active,
                        egui::Button::new("Redo\tCtrl+Shift+Z"),
                    )
                    .clicked()
                {
                    world.trigger(lunco_doc_bevy::EditorIntent::Redo);
                    ui.close();
                }

                // Domain plugins (e.g. the Modelica code editor)
                // contribute Cut/Copy/Paste/Select-All here via
                // `register_edit_menu`. Same extraction pattern as the
                // Settings menu so callbacks can take `&mut World`.
                let callbacks = std::mem::take(&mut layout.edit_menu);
                if !callbacks.is_empty() {
                    ui.separator();
                    for cb in &callbacks {
                        cb(ui, world);
                    }
                }
                layout.edit_menu = callbacks;
            });
            anchor_rects.push(("menu.edit", r_edit.response.rect));
            let r_view = ui.menu_button("View", |ui| {
                if ui.button("🔄 Reset Layout").clicked() {
                    // Recovery hatch: re-apply the active perspective's preset,
                    // restoring panels (notably the 3D Viewport) a stale
                    // persisted layout dropped.
                    layout.reset_to_default_layout();
                    ui.close();
                }
                ui.separator();
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
                            // Track in the slot list so persistence /
                            // perspective queries see it. Insert into
                            // the *live* dock without a full rebuild
                            // — rebuild_dock would wipe instance tabs
                            // (model views) the user has open.
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
                            layout.insert_panel_into_dock(id, slot);
                        } else if !checked && is_open {
                            // Untrack from slot lists.
                            layout.side_browser.retain(|p| *p != id);
                            layout.center.retain(|p| *p != id);
                            layout.right_inspector.retain(|p| *p != id);
                            layout.bottom.retain(|p| *p != id);
                            // In-place removal — preserves instance
                            // tabs and any user dock customisations.
                            layout.remove_panel_from_dock(id);
                        }
                        ui.close();
                    }
                }
            });
            anchor_rects.push(("menu.view", r_view.response.rect));
            let r_settings = ui.menu_button("Settings", |ui| {
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
            anchor_rects.push(("menu.settings", r_settings.response.rect));
            let r_help = ui.menu_button("Help", |ui| {
                ui.label(format!(
                    "Lunica v{} ({})",
                    env!("CARGO_PKG_VERSION"),
                    env!("LUNCO_GIT_HASH"),
                ));
                let callbacks = std::mem::take(&mut layout.help_menu);
                if !callbacks.is_empty() {
                    ui.separator();
                    for cb in &callbacks {
                        cb(ui, world, layout);
                    }
                }
                layout.help_menu = callbacks;
            });
            anchor_rects.push(("menu.help", r_help.response.rect));

            // Network — Connect / Disconnect. Reads the always-on
            // `lunco_core::NetStatus` and fires the `NetConnectRequest` /
            // `NetDisconnectRequest` bridge events (no lunco-networking dep
            // here, D7); the optional adapter observes them and dials. The
            // menu is always present — in single-player it just offers a
            // "Connect to server" field.
            let r_network = ui.menu_button("Network", |ui| {
                use lunco_core::{
                    NetConnectRequest, NetDisconnectRequest, NetStatus, NetworkRole,
                };
                let status = world
                    .get_resource::<NetStatus>()
                    .cloned()
                    .unwrap_or_default();

                // User Profile Settings Name Input
                let mut profile = world.resource_mut::<lunco_settings::ProfileSettings>();
                let mut name_changed = false;
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    if ui.text_edit_singleline(&mut profile.username).changed() {
                        name_changed = true;
                    }
                });
                if name_changed {
                    let mut p = world.resource_mut::<lunco_settings::ProfileSettings>();
                    p.set_changed();
                }
                ui.separator();

                match status.role {
                    NetworkRole::Host => {
                        ui.label(format!("Hosting · {}", status.endpoint));
                    }
                    NetworkRole::Client => {
                        let state = if status.connected {
                            "Connected"
                        } else {
                            "Connecting…"
                        };
                        ui.label(format!("{state} → {}", status.endpoint));
                        if ui.button("Disconnect").clicked() {
                            world.trigger(NetDisconnectRequest);
                            ui.close();
                        }
                    }
                    NetworkRole::Standalone => {
                        ui.label("Single-player (local)");
                        ui.separator();
                        // Editable address persisted in egui temp memory so it
                        // survives across frames while the menu is open. Seeded
                        // from the adapter's `connect_hint` (page origin / local).
                        let id = ui.make_persistent_id("lunco_network_menu_address");
                        let mut address = ui.data_mut(|d| {
                            d.get_temp::<String>(id).unwrap_or_else(|| {
                                if status.connect_hint.is_empty() {
                                    format!(
                                        "127.0.0.1:{}",
                                        lunco_core::session::DEFAULT_HOST_PORT
                                    )
                                } else {
                                    status.connect_hint.clone()
                                }
                            })
                        });
                        ui.horizontal(|ui| {
                            ui.label("Server:");
                            ui.text_edit_singleline(&mut address);
                        });
                        let enabled = !address.trim().is_empty();
                        if ui
                            .add_enabled(enabled, egui::Button::new("Connect"))
                            .clicked()
                        {
                            world.trigger(NetConnectRequest {
                                address: address.clone(),
                            });
                            ui.close();
                        }
                        ui.data_mut(|d| d.insert_temp(id, address));
                    }
                }
            });
            anchor_rects.push(("menu.network", r_network.response.rect));

            // Pause/Resume simulation. Toggles `Time<Virtual>` so both
            // physics (avian -> Time<Physics> derived from Virtual) and
            // the celestial clock (delta_secs gated) freeze together.
            {
                let mut vtime = world.resource_mut::<bevy::prelude::Time<bevy::prelude::Virtual>>();
                let paused = vtime.is_paused();
                let (glyph, hover) = if paused {
                    ("▶", "Resume simulation")
                } else {
                    ("⏸", "Pause simulation")
                };
                let btn_resp = ui.button(glyph).on_hover_text(hover);
                anchor_rects.push(("toolbar.run", btn_resp.rect));
                if btn_resp.clicked() {
                    if paused { vtime.unpause(); } else { vtime.pause(); }
                }
            }

            // Perspective tabs live in the menu bar (right-aligned).
            // No separate transport bar — saves a row of vertical space.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Window controls — far right on Linux/Windows where
                // the OS chrome is gone. macOS keeps the native traffic
                // lights, so we don't draw our own. On wasm the browser
                // tab owns the chrome, so min/max/close don't apply.
                #[cfg(all(not(target_os = "macos"), not(target_arch = "wasm32")))]
                {
                    let is_max = world
                        .get_resource::<window_command::WindowMaximized>()
                        .map(|s| s.0)
                        .unwrap_or(false);
                    if ui.small_button("✕").on_hover_text("Close").clicked() {
                        world.trigger(window_command::CloseWindow {});
                    }
                    let max_label = if is_max { "🗗" } else { "🗖" };
                    let max_hover = if is_max { "Restore" } else { "Maximize" };
                    if ui.small_button(max_label).on_hover_text(max_hover).clicked() {
                        world.trigger(window_command::MaximizeWindow { maximized: None });
                    }
                    if ui.small_button("─").on_hover_text("Minimize").clicked() {
                        world.trigger(window_command::MinimizeWindow {});
                    }
                    ui.separator();
                }
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
                // TODO: re-enable the perspective switcher once more
                // than one perspective is registered. With a single
                // perspective (Lunica ships only "📐 Design" today) the
                // lone tab is just noise — hide it, but keep the render
                // logic intact for when "Build" / "Simulate" / etc. land.
                if tabs.len() > 1 {
                    for (id, title, is_active) in tabs {
                        let button = egui::Button::new(title.as_str()).selected(is_active);
                        if ui.add(button).clicked() && !is_active {
                            layout.activate_perspective(id);
                        }
                    }
                }
            });

            // Flush collected button rects into `HelpAnchors` now
            // that the menu_button closures have returned and no
            // longer borrow `world`.
            if let Some(mut a) = world.get_resource_mut::<HelpAnchors>() {
                for (k, r) in anchor_rects {
                    a.set(k, r);
                }
            }
        });
    });

    // ── Status bar ──────────────────────────────────────────────────
    // Drives off the cross-cutting `StatusBus` resource. Latest event
    // shows in the strip; click opens a popup with recent history.
    // Falls back to the legacy `layout.status` text when the bus is
    // empty so existing callers keep working during the migration.
    egui::TopBottomPanel::bottom("lunco_workbench_status_bar").show(ctx, |ui| {
        ui.style_mut().visuals = theme.to_visuals();
        render_status_bar_inner(ui, world, layout, theme);
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
    //   1. If the active perspective is centre-driven (non-empty centre
    //      intent, e.g. the modelica workbench's Code/Diagram), render the
    //      full DockArea.
    //   2. Otherwise (viewport-only perspective like the sandbox's `View`),
    //      render the side panels with plain SidePanel / TopBottomPanel and
    //      leave the central area transparent for the 3D viewport.
    //
    // The gate is the centre *intent* (`layout.center`), not merely "does
    // the dock hold any tab". A hybrid app (the rover sandbox embeds the
    // Modelica workbench) can have document/model instance tabs parked in
    // the dock while a viewport-only perspective is active — e.g. restored
    // on boot before the user switches to a doc-capable perspective.
    // Keying off the dock alone would flip the whole workbench into
    // dock-mode and paint tab chrome over the 3D scene; keying off the
    // perspective's centre intent keeps `View` pure-3D and leaves the
    // parked docs hidden until the user switches to a centre-driven
    // perspective (which re-attaches them via `rebuild_dock`).
    let has_dock_tabs =
        !layout.center.is_empty() && layout.dock.iter_all_tabs().next().is_some();

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
        // Tab body fill is set further below alongside the
        // per-state tab colours so the body matches the active tab.
        // Always opaque, in every app. Transparency on the bar made
        // the Modelica workbench look broken, and the sandbox's
        // centre is a transparent `ViewportPanel` anyway — a dark
        // strip above its invisible header just looks like the top
        // edge of the viewport tile, which is fine.
        style.tab_bar.bg_fill = get_panel_backdrop(theme);
        // Drop the hairline under the active tab name too — same
        // visual-noise reason as the tab body stroke.
        style.tab_bar.hline_color = egui::Color32::TRANSPARENT;
        // egui_dock's `Style::from_egui` defaults pull tab colours
        // from `visuals.widgets`, but the result still doesn't track
        // our Light/Dark palette cleanly: inactive tabs come out
        // washed out and active tabs lose contrast against the bar.
        // Bind every interaction state to the theme so tabs read
        // consistently in both modes.
        // Bar = `mantle`. Active tab = `surface0` so it visually
        // joins the body area (which we also paint `surface0` below
        // for the same reason). Inactive tab = `crust` (a step away
        // from the bar) so the strip is legible in both modes —
        // using `mantle` for inactive made every tab vanish into the
        // bar in Light mode where mantle/bar contrast is minimal.
        let palette = &theme.colors;
        style.tab.tab_body.bg_fill = palette.surface0;
        style.tab.active.bg_fill = palette.surface0;
        style.tab.active.text_color = palette.text;
        style.tab.active.outline_color = egui::Color32::TRANSPARENT;
        style.tab.inactive.bg_fill = palette.crust;
        style.tab.inactive.text_color = palette.subtext1;
        style.tab.inactive.outline_color = egui::Color32::TRANSPARENT;
        style.tab.hovered.bg_fill = palette.surface1;
        style.tab.hovered.text_color = palette.text;
        style.tab.hovered.outline_color = egui::Color32::TRANSPARENT;
        style.tab.focused.bg_fill = palette.surface0;
        style.tab.focused.text_color = palette.mauve;
        style.tab.focused.outline_color = egui::Color32::TRANSPARENT;
        style.tab.inactive_with_kb_focus.bg_fill = palette.crust;
        style.tab.inactive_with_kb_focus.text_color = palette.text;
        style.tab.active_with_kb_focus.bg_fill = palette.surface0;
        style.tab.active_with_kb_focus.text_color = palette.mauve;
        style.tab.focused_with_kb_focus.bg_fill = palette.surface0;
        style.tab.focused_with_kb_focus.text_color = palette.mauve;
        // TODO(egui_dock 0.18 bug — remove when fixed/updated upstream):
        // egui_dock writes a NaN split fraction into the tree from inside its
        // own `show()` every frame a pane is squeezed to zero width — see
        // `sanitize_dock_fractions` for the exact `0.0/0.0` site. So we must
        // re-assert the invariant every frame, right before layout, or egui
        // asserts ("rect is nan"). Drop this call once egui_dock guards its
        // `delta / range`; the load-time sanitize stays regardless.
        sanitize_dock_fractions(dock);

        // Guard against degenerate viewport rects. On Windows + Intel
        // Vulkan the swapchain can present a zero/non-finite size for
        // the first frames after the window is mapped; egui_dock 0.18
        // then computes `min + dim_size * fraction` with
        // `Rect::NOTHING`, yielding NaN, and egui asserts in
        // `advance_cursor_after_rect`. Skip the dock for that frame.
        let screen = ctx.content_rect();
        if screen.width().is_finite()
            && screen.height().is_finite()
            && screen.width() > 1.0
            && screen.height() > 1.0
        {
            DockArea::new(dock).style(style).show(ctx, &mut viewer);
            // After the dock has laid itself out, publish the area
            // rect under a generic "panel.center" anchor so the help
            // tour can spotlight the dock content as a whole.
            if let Some(mut a) = world.get_resource_mut::<HelpAnchors>() {
                a.set("panel.center", screen);
            }
        }
    } else {
        // 3D-app mode — explicit side panels, transparent centre.
        // Defaults are percentages of the current window so the layout
        // looks right whether the user runs in 1280×720 or 4K. Targets
        // mirror a 10/80/10 split: side panels 10% of window width each;
        // bottom dock 20% of window height.
        let screen = ctx.content_rect();
        // Defaults are percentages of the current window so the layout
        // looks right whether the user runs in 1280×720 or 4K. Targets
        // mirror a 10/80/10 split: side panels 10% of window width each;
        // bottom dock 20% of window height. egui then owns the live width
        // in its own memory for the session (not persisted — sandbox-style
        // perspectives keep their sizes in the dock tree via 5a instead).
        let side_default = (screen.width() * 0.10).max(140.0);
        let right_default = (screen.width() * 0.10).max(140.0);
        let bottom_default = (screen.height() * 0.20).max(120.0);

        if let Some(id) = layout.side_browser.first().copied() {
            let r = egui::SidePanel::left("lunco_workbench_side_panel_left")
                .resizable(true)
                .default_width(side_default)
                .min_width(120.0)
                .max_width(screen.width() * 0.3)
                .show(ctx, |ui| {
                    ui.style_mut().visuals = theme.to_visuals();
                    render_panel_solo(ui, &id, layout, world);
                });
            if let Some(mut a) = world.get_resource_mut::<HelpAnchors>() {
                a.set("panel.side_browser", r.response.rect);
            }
        }
        if let Some(id) = layout.right_inspector.first().copied() {
            let r = egui::SidePanel::right("lunco_workbench_side_panel_right")
                .resizable(true)
                .default_width(right_default)
                .min_width(140.0)
                .max_width(screen.width() * 0.3)
                .show(ctx, |ui| {
                    ui.style_mut().visuals = theme.to_visuals();
                    render_panel_solo(ui, &id, layout, world);
                });
            if let Some(mut a) = world.get_resource_mut::<HelpAnchors>() {
                a.set("panel.right_inspector", r.response.rect);
            }
        }
        if let Some(id) = layout.bottom.first().copied() {
            let r = egui::TopBottomPanel::bottom("lunco_workbench_bottom_panel")
                .resizable(true)
                .default_height(bottom_default)
                .min_height(60.0)
                .show(ctx, |ui| {
                    ui.style_mut().visuals = theme.to_visuals();
                    render_panel_solo(ui, &id, layout, world);
                });
            if let Some(mut a) = world.get_resource_mut::<HelpAnchors>() {
                a.set("panel.bottom", r.response.rect);
            }
        }
        // Central area: do NOT call CentralPanel — egui's bottom/side
        // panels reserve their space and the remaining region stays
        // free for the 3D scene that Bevy renders to the full window.
        // Scene-vs-chrome picking is handled by bevy_picking (egui occlusion via
        // bevy_egui's picking backend), so there's no pointer gate to compute
        // here anymore.
    }

    // ── Empty-viewport placeholder ──────────────────────────────────
    // Drawn last so it sits on top of the (empty) 3D framebuffer. Only
    // when a domain crate set a message (e.g. lunco-usd: "no scene
    // loaded") AND the viewport is actually on screen — View (empty
    // layout, full-window 3D) or Build (ViewportPanel in the centre).
    // Never in Design mode, where Camera3d is inactive and the centre
    // is chrome. Centered on the window, which is the viewport region
    // in View mode and close enough in Build.
    let placeholder = world
        .get_resource::<viewport::ViewportPlaceholder>()
        .and_then(|p| p.message.clone());
    if let Some(msg) = placeholder {
        let viewport_visible = viewport::layout_is_empty(layout)
            || viewport::layout_contains_panel(layout, viewport::VIEWPORT_PANEL_ID);
        if viewport_visible {
            egui::Area::new(egui::Id::new("lunco_viewport_empty_placeholder"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .interactable(false)
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new(msg)
                            .color(theme.tokens.text_subdued)
                            .italics()
                            .size(16.0),
                    );
                });
        }
    }
}

/// Render a single panel inside its own egui container (side-panel mode).
/// Mirrors PanelTabViewer's lookup-and-take-back pattern.
/// Render the bottom status strip. Reads from [`status_bus::StatusBus`]
/// (cross-cutting; populated by MSL load, compile, sim, etc.) and
/// renders a click-to-expand popup with recent history.
fn render_status_bar_inner(
    ui: &mut egui::Ui,
    world: &mut World,
    layout: &WorkbenchLayout,
    theme: &lunco_theme::Theme,
) {
    use status_bus::{StatusBus, StatusLevel};

    let popup_id = ui.make_persistent_id("lunco_workbench_status_bar_popup");

    // Snapshot what we need from the bus into local owned values so
    // we don't hold a borrow across the popup callback (it also wants
    // to read the bus).
    struct LatestSnapshot {
        source: &'static str,
        message: String,
        level: StatusLevel,
        progress_pct: Option<f64>,
    }
    let (latest, history): (Option<LatestSnapshot>, Vec<status_bus::StatusEvent>) = {
        let bus = world.resource::<StatusBus>();
        let latest = bus.display_latest().map(|e| LatestSnapshot {
            source: e.source,
            message: e.message.clone(),
            level: e.level,
            progress_pct: e.progress_pct(),
        });
        let history: Vec<_> = bus.history().cloned().collect();
        (latest, history)
    };
    let perf_stats = world.resource::<perf_hud::PerfStats>().clone();
    let perf_enabled = world.resource::<perf_hud::PerfHudSettings>().enabled;
    // The networking chip only paints when not standalone; reserve room
    // for it on the right so the clickable status region doesn't overlap.
    let net_active = world
        .get_resource::<lunco_core::NetStatus>()
        .map(|s| !matches!(s.role, lunco_core::NetworkRole::Standalone))
        .unwrap_or(false);

    ui.horizontal(|ui| {
        // The whole strip is one clickable region; the popup anchors
        // off its response so it appears just above the bar.
        let response = ui
            .scope(|ui| {
                ui.set_height(18.0);
                // Whole strip is the click target, not just the dot+text:
                // stretch this region to fill the available width minus the
                // space the right-aligned perf HUD / net chip will claim.
                // The content stays left-aligned; the trailing empty space
                // is still part of the response rect, so a click anywhere on
                // the bar opens the history popup.
                let right_reserve = 8.0
                    + if perf_enabled { 300.0 } else { 0.0 }
                    + if net_active { 220.0 } else { 0.0 };
                ui.set_min_width((ui.available_width() - right_reserve).max(160.0));
                if let Some(l) = latest.as_ref() {
                    let dot_color = match l.level {
                        StatusLevel::Error => theme.tokens.error,
                        StatusLevel::Warn => theme.tokens.warning,
                        StatusLevel::Progress | StatusLevel::Info => theme.tokens.success,
                    };
                    // Painted circle instead of `●` so we don't depend
                    // on a font that ships U+25CF (the wasm build's
                    // egui font fallback chain doesn't, hence "tofu"
                    // boxes for that glyph).
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(10.0, 10.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().circle_filled(rect.center(), 4.0, dot_color);
                    ui.label(egui::RichText::new(l.source).small().strong());
                    ui.label(egui::RichText::new(&l.message).small());
                    if let Some(pct) = l.progress_pct {
                        ui.add(
                            egui::ProgressBar::new((pct as f32) / 100.0)
                                .desired_width(120.0)
                                .desired_height(6.0),
                        );
                    }
                } else {
                    // Bus is empty — fall back to whatever a panel
                    // pushed via the legacy `layout.status_bar(...)`
                    // API so existing call sites keep working.
                    match layout.status.as_ref() {
                        Some(StatusContent::Text(s)) => {
                            ui.label(egui::RichText::new(s).small());
                        }
                        None => {
                            ui.label(egui::RichText::new("ready").small().weak());
                        }
                    }
                }
            })
            .response
            .interact(egui::Sense::click())
            .on_hover_text("Click to view recent status events");

        if response.clicked() {
            egui::Popup::toggle_id(ui.ctx(), popup_id);
        }

        ui.separator();

        // Pinned networking chip — host/client role, endpoint + ports, and a
        // live peer / connection readout. Silent in single-player. Reads
        // `lunco_core::NetStatus` (no lightyear dep here).
        render_net_chip(ui, world, theme);

        // Right-aligned perf segment. Hidden when the HUD is off so
        // we don't show stale zeroes; toggled via `TogglePerfHud` or
        // the Settings menu.
        if perf_enabled {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let phys = perf_stats
                    .physics_ms
                    .map(|ms| format!(" · phys {:>4.1}ms", ms))
                    .unwrap_or_default();
                let p99 = perf_stats
                    .frame_ms_stats()
                    .map(|(_, _, p99)| format!(" · p99 {:>5.1}ms", p99))
                    .unwrap_or_default();
                // Fixed-width fields so the HUD doesn't shift when
                // FPS crosses 99→100 or frame_ms crosses 9→10. Values
                // are right-justified inside their fields by the
                // padding spec; monospace alone isn't enough because
                // the *number of characters* changes.
                ui.label(
                    egui::RichText::new(format!(
                        "FPS {:>5.1} · {:>5.1}ms{}{}",
                        perf_stats.fps, perf_stats.frame_ms, p99, phys,
                    ))
                    .small()
                    .monospace(),
                );
                draw_frame_time_sparkline(ui, &perf_stats, theme);
            });
        }

        // egui::Popup is the post-0.31 API. `open_memory(None)` ties
        // the open state to egui's memory keyed by `popup_id`, so the
        // `toggle_popup` call above flips it.
        egui::Popup::from_response(&response)
            .id(popup_id)
            .align(egui::RectAlign::TOP_START)
            .layout(egui::Layout::top_down_justified(egui::Align::LEFT))
            .open_memory(None)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_min_width(420.0);
                ui.set_max_width(560.0);
                ui.set_max_height(360.0);
                ui.heading("Recent status events");
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if history.is_empty() {
                        ui.label(egui::RichText::new("(no events yet)").weak());
                        return;
                    }
                    // Newest first.
                    for ev in history.iter().rev() {
                        let level_tag = match ev.level {
                            StatusLevel::Info => egui::RichText::new("INFO ")
                                .small()
                                .color(theme.tokens.text_subdued),
                            StatusLevel::Warn => egui::RichText::new("WARN ")
                                .small()
                                .color(theme.tokens.warning),
                            StatusLevel::Error => egui::RichText::new("ERR  ")
                                .small()
                                .color(theme.tokens.error),
                            StatusLevel::Progress => egui::RichText::new("…    ")
                                .small()
                                .color(theme.tokens.text_subdued),
                        };
                        ui.horizontal(|ui| {
                            ui.label(level_tag.monospace());
                            ui.label(
                                egui::RichText::new(format!("[{}]", ev.source))
                                    .small()
                                    .strong(),
                            );
                            ui.label(egui::RichText::new(&ev.message).small());
                        });
                    }
                });
            });
    });
}

/// Render the always-visible networking chip in the status bar.
/// Reads `lunco_core::NetStatus` (always present; populated by the
/// optional `lunco-networking` adapter when it's wired). Silent (zero pixels)
/// in single-player (`Standalone`), so non-networked apps show nothing.
///
/// - **Host**: green dot, `HOST :PORT · N peers` (this window's listen port).
/// - **Client (connected)**: green dot, `CLIENT → host:port`.
/// - **Client (connecting)**: amber dot, `connecting → host:port`.
fn render_net_chip(
    ui: &mut egui::Ui,
    world: &mut World,
    theme: &lunco_theme::Theme,
) {
    use lunco_core::{NetStatus, NetworkRole};
    let Some(status) = world.get_resource::<NetStatus>().cloned() else {
        return;
    };
    let (dot, label) = match status.role {
        // Single-player — the wire is inert, so show nothing.
        NetworkRole::Standalone => return,
        NetworkRole::Host => {
            let s = if status.peers == 1 { "" } else { "s" };
            (
                theme.tokens.success,
                format!("HOST {} · {} peer{s}", status.endpoint, status.peers),
            )
        }
        NetworkRole::Client if status.connected => {
            (theme.tokens.success, format!("CLIENT → {}", status.endpoint))
        }
        NetworkRole::Client => (
            theme.tokens.warning,
            format!("connecting → {}", status.endpoint),
        ),
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, dot);
    ui.label(egui::RichText::new(label).small())
        .on_hover_text("LunCoSim networking");
    ui.separator();
}

/// Draws a small frame-time sparkline in the status bar so spikes
/// the smoothed `FPS` number hides become visible. Y axis auto-
/// scales to whatever the worst recent sample was; a faint reference
/// line at 16.67 ms (60 FPS) anchors the eye.
fn draw_frame_time_sparkline(
    ui: &mut egui::Ui,
    stats: &perf_hud::PerfStats,
    theme: &lunco_theme::Theme,
) {
    if stats.frame_history.is_empty() {
        return;
    }
    // Plot dimensions chosen to fit the 18 px-tall status bar with
    // a few px of breathing room.
    let size = egui::vec2(120.0, 14.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter().with_clip_rect(rect);

    // Auto-scale: top of the plot is the worst recent sample, but
    // never below ~25 ms so a calm 60 FPS run doesn't make 1 ms
    // jitter look like a spike.
    let max_ms: f32 = stats
        .frame_history
        .iter()
        .copied()
        .fold(0.0_f32, f32::max)
        .max(25.0);

    // 16.67 ms (60 FPS) reference line — pulls from `text_subdued`
    // and softens with alpha so it doesn't compete with the trace.
    let muted = theme.tokens.text_subdued;
    let muted_soft = muted.alpha(80);
    let ref_y = rect.bottom() - rect.height() * (16.67 / max_ms).min(1.0);
    painter.line_segment(
        [egui::pos2(rect.left(), ref_y), egui::pos2(rect.right(), ref_y)],
        egui::Stroke::new(0.5, muted_soft),
    );

    let n = stats.frame_history.len();
    let step = rect.width() / (perf_hud::FRAME_HISTORY_LEN - 1).max(1) as f32;
    let mut prev: Option<egui::Pos2> = None;
    for (i, ms) in stats.frame_history.iter().enumerate() {
        let x = rect.left() + i as f32 * step;
        let y = rect.bottom() - rect.height() * (*ms / max_ms).clamp(0.0, 1.0);
        let here = egui::pos2(x, y);
        // Per-sample colour: success ≤16.67 ms, warning ≤33 ms, error above.
        let colour = if *ms <= 16.67 {
            theme.tokens.success
        } else if *ms <= 33.34 {
            theme.tokens.warning
        } else {
            theme.tokens.error
        };
        if let Some(p) = prev {
            painter.line_segment([p, here], egui::Stroke::new(1.0, colour));
        }
        prev = Some(here);
    }
    // Outline so the plot reads as a chart, not random pixels.
    let outline = muted.alpha(100);
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(0.5, outline),
        egui::StrokeKind::Inside,
    );
    let _ = n;
}

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
        let mut ctx = PanelCtx::new(world);
        panel.render(ui, &mut ctx);
        let deferred = ctx.into_deferred();
        layout.panels.insert(*id, panel);
        for f in deferred {
            f(world);
        }
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