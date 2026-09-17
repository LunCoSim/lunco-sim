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
//!   `default_slot`, `render(&mut Ui, &mut PanelCtx)`)
//! - [`WorkbenchLayout`] resource wrapping `egui_dock::DockState`
//! - Perspective presets (slot-assignment DSL) — see [`Perspective`]
//! - Auto-add of `bevy_egui::EguiPlugin` if the host hasn't
//!
//! ## What's persisted across restarts
//!
//! - **Window geometry** (size / position / maximized) — global default
//!   in the OS config directory via `lunco-settings`. See
//!   [`lunco_workbench_window::WindowPersistencePlugin`].
//! - **Per-Twin UI state** (active perspective + open-document list) —
//!   `workspace-state/<hash>.json` in the shared LunCoSim config directory,
//!   keyed by Twin path,
//!   VSCode-`workspaceStorage` style. See [`lunco_workbench_state`].
//!
//! ## What's deferred
//!
//! - **Free-form dock-tree fidelity** — restore re-applies the
//!   *perspective* preset, not arbitrary user split rearrangements
//!   (egui_dock's tree isn't serialized; `TabId`/`PanelId` hold
//!   `&'static str`).
//! - **Command palette** — `Ctrl+P` unbound.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use egui_dock::{
    widgets::tab_viewer::OnCloseResponse, DockArea, DockState, NodeIndex, Style, TabViewer,
};
use lunco_core::{on_command, register_commands};
use lunco_settings::{AppSettingsExt, SettingsSection};
use lunco_theme::ColorAlpha;
use lunco_workbench_core::commands::{CloseTab, FocusPanel, OpenTab, OpenTabPreserveFocus};
use lunco_workbench_core::presentation::{HelpAnchors, ViewportPlaceholder};
use lunco_workbench_core::scene::{CurrentSceneName, CurrentScenePath};
use lunco_workbench_core::scene_pick::ScenePickGate;
use lunco_workbench_core::tabs::PendingTabCloses;
use lunco_workbench_core::uri::UriRegistry;
use lunco_workbench_core::viewport::{PanelRects, VIEWPORT_PANEL_ID};
use lunco_workbench_core::WorkbenchPanelAppExt;
use lunco_workbench_core::{
    ApplicationOverlayRenderSet, InstancePanel, MenuCtx, Panel, PanelCtx, PanelId, PanelMenuGroup,
    PanelRenderTarget, PanelScrollPolicy, PanelSlot, PanelSurfaceStyle, Perspective, PerspectiveId,
    TabId, UndoProbeCtx, WorkbenchMenuRegistry, WorkbenchPanelRegistry, WorkbenchRenderSet,
    WorkbenchSnapshot,
};
use lunco_workbench_widgets::{icon_button_sized, text_editor, UiIcon};
use std::collections::HashMap;
use std::sync::Arc;

mod layout;
mod layout_render;
mod perspective;
mod perspective_help;
mod render;
mod session;
mod twin_settings;
mod viewport;

use layout::{WorkbenchLayout, WorkbenchLayoutStateProvider};
pub use render::menu_popup_max_width;
pub(crate) use render::{
    dock_group_rects, find_leaf_matching, first_leaf, measured_menu_row_width,
    measured_titlebar_right_width, menu_item, needs_full_backdrop, new_document_menu_label,
    perspective_help_anchor, perspective_switcher_tabs, publish_panel_anchor, render_custom_menus,
    render_edit_menu, render_help_menu, render_network_menu, render_panel_solo,
    render_settings_menu, render_status_bar_inner, render_time_menu, run_menu_callback,
    scene_camera_is_rendering, top_menu_mode, truncate_title_to_width, PanelTabViewer, TopMenuMode,
};
use render::{
    register_graphics_settings_menu, register_workbench_appearance_settings_menu, render_workbench,
};

pub mod control_status;
pub mod perf_hud;
pub mod perspective_command;
pub mod theme_command;

pub use perspective_help::{
    HelpMouse, HelpPopup, HelpShortcut, LiveHelpSection, LiveHelpSections, PerspectiveHelp,
    PerspectiveHelpPlugin, PerspectiveHelpRegistry,
};
/// Authoritative version and build identity supplied by the host application.
///
/// Workbench is shared by multiple binaries, so its own package metadata is
/// not the product identity users are running. Each host inserts its stamped
/// identity, and Workbench only presents it in shared UI such as Help.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct BuildIdentity {
    /// Release or product version shown to users.
    pub version: String,
    /// Build identifier, normally the short source revision.
    pub build: String,
    /// Canonical GitHub repository containing the source revision.
    pub repository: String,
}

impl BuildIdentity {
    /// Create an identity from the host application's stamped values.
    pub fn new(
        version: impl Into<String>,
        build: impl Into<String>,
        repository: impl Into<String>,
    ) -> Self {
        Self {
            version: version.into(),
            build: build.into(),
            repository: repository.into(),
        }
    }

    /// Format the canonical version line shared by Help and Settings.
    pub fn version_label(&self) -> String {
        format!("Version {} ({})", self.version, self.build)
    }

    /// Return the exact source revision URL when the build has a known SHA.
    pub fn source_url(&self) -> Option<String> {
        let revision = self.build.strip_suffix("-dirty").unwrap_or(&self.build);
        if revision.is_empty() || revision == "unknown" {
            return None;
        }
        Some(format!(
            "{}/commit/{}",
            self.repository.trim_end_matches('/'),
            revision
        ))
    }
}

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

/// Name of the binary actually running, for the Help menu's build line.
///
/// This crate is a source library shared by every workbench app (`luncosim`, `lunica`,
/// …), so it cannot know at compile time which one linked it — `CARGO_BIN_NAME`
/// is set for bin targets and would be wrong (or absent) here. The running
/// executable's own file stem is the one answer that is true in every app, so
/// the luncosim stops introducing itself as Lunica.
///
/// Resolved once: the path cannot change while the process lives.
fn running_app_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        #[cfg(not(target_arch = "wasm32"))]
        {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                // A stripped/unreadable `/proc/self/exe` is possible; the crate
                // name is a truthful fallback, unlike a hardcoded app name.
                .unwrap_or_else(|| env!("CARGO_PKG_NAME").to_string())
                .replace("luncosim", "LunCoSim")
        }
        // On the web there is no executable path; the bundle name is the
        // closest true answer and is set by the build script per app.
        #[cfg(target_arch = "wasm32")]
        {
            "LunCoSim".to_string()
        }
    })
}

fn on_open_tab(
    trigger: On<OpenTab>,
    layout: Option<ResMut<WorkbenchLayout>>,
    mut pending: ResMut<PendingTabRequests>,
) {
    let ev = *trigger.event();
    if let Some(mut layout) = layout {
        layout.open_instance(ev.kind, ev.instance);
    } else {
        pending.0.push(TabRequest::Open(ev));
    }
}

fn on_open_tab_preserve_focus(
    trigger: On<OpenTabPreserveFocus>,
    layout: Option<ResMut<WorkbenchLayout>>,
    mut pending: ResMut<PendingTabRequests>,
) {
    let ev = *trigger.event();
    if let Some(mut layout) = layout {
        layout.open_instance_without_focus(ev.kind, ev.instance, ev.restore);
    } else {
        pending.0.push(TabRequest::OpenPreserveFocus(ev));
    }
}

fn on_close_tab(
    trigger: On<CloseTab>,
    layout: Option<ResMut<WorkbenchLayout>>,
    mut pending: ResMut<PendingTabRequests>,
) {
    let ev = *trigger.event();
    if let Some(mut layout) = layout {
        layout.close_instance(ev.kind, ev.instance);
    } else {
        pending.0.push(TabRequest::Close(ev));
    }
}

#[derive(Clone, Copy)]
enum TabRequest {
    Open(OpenTab),
    OpenPreserveFocus(OpenTabPreserveFocus),
    Close(CloseTab),
}

/// Layout mutations raised by egui controls are committed by `Update`, after
/// the complete egui multipass run. Mutating the dock while egui is replaying
/// a layout pass makes the same rectangle contain a different widget on pass
/// two, which is precisely the `Widget rect changed id between passes` warning
/// and can also make a click run twice.
#[derive(Clone)]
enum LayoutRequest {
    Reset,
    SetActivityBar(bool),
    AddSingleton { id: PanelId, slot: PanelSlot },
    RemoveSingleton(PanelId),
    ActivatePerspective(String),
}

#[derive(Resource, Default)]
struct PendingLayoutRequests(Vec<LayoutRequest>);

#[derive(Resource, Default)]
struct PendingTabRequests(Vec<TabRequest>);

fn drain_pending_tab_requests(
    layout: Option<ResMut<WorkbenchLayout>>,
    mut pending: ResMut<PendingTabRequests>,
) {
    let Some(mut layout) = layout else {
        return;
    };
    for request in std::mem::take(&mut pending.0) {
        match request {
            TabRequest::Open(ev) => layout.open_instance(ev.kind, ev.instance),
            TabRequest::OpenPreserveFocus(ev) => {
                layout.open_instance_without_focus(ev.kind, ev.instance, ev.restore)
            }
            TabRequest::Close(ev) => layout.close_instance(ev.kind, ev.instance),
        }
    }
}

fn drain_pending_layout_requests(
    layout: Option<ResMut<WorkbenchLayout>>,
    mut pending: ResMut<PendingLayoutRequests>,
    mut commands: Commands,
) {
    let Some(mut layout) = layout else {
        return;
    };

    for request in std::mem::take(&mut pending.0) {
        match request {
            LayoutRequest::Reset => layout.reset_to_default_layout(),
            LayoutRequest::SetActivityBar(visible) => layout.activity_bar = visible,
            LayoutRequest::ActivatePerspective(id) => {
                if !layout.activate_perspective_by_str(&id) {
                    perspective_command::report_unknown_perspective(&mut commands, &id);
                }
            }
            LayoutRequest::AddSingleton { id, slot } => {
                if !layout.panels.contains_key(&id) {
                    continue;
                }
                let already_docked = layout
                    .dock
                    .iter_all_tabs()
                    .any(|(_, tab)| matches!(tab, TabId::Singleton(tab_id) if *tab_id == id));
                if already_docked {
                    continue;
                }
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
                    PanelSlot::Hidden => {
                        unreachable!("hidden panels are normalized before queueing")
                    }
                }
                layout.insert_panel_into_dock(id, slot);
            }
            LayoutRequest::RemoveSingleton(id) => {
                layout.side_browser.retain(|panel| *panel != id);
                layout.side_browser_bottom.retain(|panel| *panel != id);
                layout.center.retain(|panel| *panel != id);
                layout.right_inspector.retain(|panel| *panel != id);
                layout.right_inspector_bottom.retain(|panel| *panel != id);
                layout.bottom.retain(|panel| *panel != id);
                layout.remove_panel_from_dock(id);
            }
        }
    }
}

/// Publish the shell's current layout as the renderer-independent read model.
///
/// The dock remains authoritative for concrete rendering and persistence. The
/// snapshot is the only layout fact domain crates should consume, so they do
/// not acquire a dependency on `egui_dock` or the shell's private resource.
pub(crate) fn publish_workbench_snapshot(
    layout: &WorkbenchLayout,
    snapshot: &mut WorkbenchSnapshot,
) {
    let tabs: Vec<TabId> = layout.dock.iter_all_tabs().map(|(_, tab)| *tab).collect();
    let focused_tab = layout.dock.main_surface().focused_leaf().and_then(|node| {
        match &layout.dock.main_surface()[node] {
            egui_dock::Node::Leaf(leaf) => leaf.tabs.get(leaf.active.0).copied(),
            _ => None,
        }
    });
    let visible_panels = layout
        .dock
        .iter_all_nodes()
        .filter_map(|(_, node)| match node {
            egui_dock::Node::Leaf(leaf) => leaf.tabs.get(leaf.active.0),
            _ => None,
        })
        .map(|tab| match tab {
            TabId::Singleton(id) => *id,
            TabId::Instance { kind, .. } => *kind,
        })
        .collect();
    let registered_perspectives = layout
        .perspectives
        .iter()
        .map(|perspective| perspective.id())
        .collect();
    let mut docked_panels = Vec::new();
    for tab in &tabs {
        let id = match tab {
            TabId::Singleton(id) => *id,
            TabId::Instance { kind, .. } => *kind,
        };
        if !docked_panels.contains(&id) {
            docked_panels.push(id);
        }
    }
    snapshot.replace(
        layout.active_perspective,
        focused_tab,
        tabs,
        visible_panels,
        registered_perspectives,
        docked_panels,
    );
}

fn sync_workbench_snapshot(layout: Res<WorkbenchLayout>, mut snapshot: ResMut<WorkbenchSnapshot>) {
    publish_workbench_snapshot(&layout, &mut snapshot);
}

/// Focus requests emitted while the dock layout is scoped out during egui
/// rendering. Drained on the next `Update`, when `WorkbenchLayout` is present.
#[derive(Resource, Default)]
struct PendingPanelFocus(Vec<String>);

#[on_command(FocusPanel)]
fn on_focus_panel(
    trigger: On<FocusPanel>,
    layout: Option<ResMut<WorkbenchLayout>>,
    pending: Option<ResMut<PendingPanelFocus>>,
) {
    // `FocusPanel` is safe to fire at any time (e.g. an asset-browser click
    // before the workbench has finished setting up, or in a host config that
    // doesn't add the full workbench). `WorkbenchLayout` is only present once
    // `WorkbenchPlugin` has run; treat its absence as a no-op rather than
    // panicking — there is no dock to focus into.
    let Some(mut layout) = layout else {
        if let Some(mut pending) = pending {
            let id = trigger.event().id.clone();
            if !pending.0.contains(&id) {
                pending.0.push(id);
            }
        }
        bevy::log::debug!(
            "[FocusPanel] id={:?} queued — WorkbenchLayout temporarily unavailable",
            trigger.event().id
        );
        return;
    };
    focus_panel_now(&mut layout, &trigger.event().id);
}

fn focus_panel_now(layout: &mut WorkbenchLayout, want: &str) {
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
        bevy::log::info!("[FocusPanel] id={:?} focus_singleton -> {}", want, ok);
    } else {
        // A guided's named anchor is an actionable request, not a promise
        // that the user already opened the panel. Mount the registered panel
        // in its authored default slot, then foreground it. Panels omitted
        // from presets use the side browser when explicitly opened.
        let Some((pid, authored_slot)) = layout
            .panels
            .iter()
            .find(|(pid, _)| pid.0 == want)
            .map(|(pid, panel)| (*pid, panel.default_slot()))
        else {
            bevy::log::warn!("[FocusPanel] id={:?} is not registered", want);
            return;
        };
        let slot = match authored_slot {
            PanelSlot::Hidden => PanelSlot::SideBrowser,
            slot => slot,
        };
        match slot {
            PanelSlot::SideBrowser if !layout.side_browser.contains(&pid) => {
                layout.side_browser.push(pid)
            }
            PanelSlot::Center if !layout.center.contains(&pid) => layout.center.push(pid),
            PanelSlot::RightInspector if !layout.right_inspector.contains(&pid) => {
                layout.right_inspector.push(pid)
            }
            PanelSlot::Bottom if !layout.bottom.contains(&pid) => layout.bottom.push(pid),
            PanelSlot::Hidden => unreachable!("hidden panels are normalized above"),
            _ => {}
        }
        let inserted = layout.insert_panel_into_dock(pid, slot);
        let focused = layout.focus_singleton(pid);
        bevy::log::info!(
            "[FocusPanel] id={:?} opened (inserted={inserted}) and focused={focused}",
            want
        );
    }
}

fn drain_pending_panel_focus(
    mut pending: ResMut<PendingPanelFocus>,
    mut layout: ResMut<WorkbenchLayout>,
) {
    for id in std::mem::take(&mut pending.0) {
        focus_panel_now(&mut layout, &id);
    }
}

register_commands!(on_focus_panel,);

// The session binding (WorkspaceResource, WorkspacePlugin, add/close events)
// lives in `lunco-workspace` now — consumers import it from there directly.
// `session` here is just the workbench-side recents persistence.
use lunco_workspace::WorkspaceResource;
pub use viewport::{ViewportPanel, WorkbenchEguiHost, WorkbenchViewportPlugin};

/// Get the backdrop colour from the active theme.
fn get_panel_backdrop(theme: &lunco_theme::Theme) -> egui::Color32 {
    theme.colors.mantle
}

/// Resolve the one themed surface used behind ordinary workbench panels.
fn panel_surface_fill(theme: &lunco_theme::Theme, translucent_tab_content: bool) -> egui::Color32 {
    if translucent_tab_content {
        theme.tokens.overlay_backdrop
    } else {
        theme.colors.mantle
    }
}

/// Build the shell-supplied appearance contract for panel-owned content.
///
/// The dock body and standalone side-panel frame may use a translucent
/// backdrop, but a panel's standard content frame remains transparent in that
/// mode. This preserves the shell presentation while keeping the style
/// decision outside the renderer-independent panel crate.
fn panel_content_surface_style(
    theme: &lunco_theme::Theme,
    translucent_tab_content: bool,
) -> PanelSurfaceStyle {
    PanelSurfaceStyle {
        fill: if translucent_tab_content {
            egui::Color32::TRANSPARENT
        } else {
            theme.colors.mantle
        },
        inner_margin: theme.spacing.window_padding.into(),
        corner_radius: theme.rounding.window.into(),
    }
}

/// Persisted workbench appearance preferences.
///
/// This is shell-level presentation state rather than a panel preference: the
/// workbench owns the dock body, while `PanelCtx` exposes the same decision to
/// panel-owned content cards. Keeping the decision here prevents one tab from
/// accidentally becoming transparent while another still paints an opaque
/// rectangle over the scene.
#[derive(Resource, serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct WorkbenchAppearanceSettings {
    /// Use the theme's translucent surface behind every dock/tab content body.
    /// The default keeps panel chrome readable while allowing the scene to
    /// remain visible behind it.
    #[serde(default = "default_translucent_tab_content")]
    pub translucent_tab_content: bool,
}

fn default_translucent_tab_content() -> bool {
    true
}

impl Default for WorkbenchAppearanceSettings {
    fn default() -> Self {
        Self {
            translucent_tab_content: default_translucent_tab_content(),
        }
    }
}

impl SettingsSection for WorkbenchAppearanceSettings {
    const KEY: &'static str = "workbench_appearance";
}

/// Whether the current panel body leaves the scene unobscured by egui chrome.
///
/// The main scene viewport is always transparent because it is the scene host;
/// ordinary panels use the theme's translucent surface or opaque mantle. A
/// declared panel transparency remains available for standalone panel
/// harnesses that do not install the workbench settings resource.
fn panel_body_is_transparent(
    world: &World,
    declared_transparent: bool,
    is_main_scene: bool,
) -> bool {
    if is_main_scene {
        return true;
    }
    world
        .get_resource::<WorkbenchAppearanceSettings>()
        .map(|_| false)
        .unwrap_or(declared_transparent)
}

/// Plugin that installs the workbench shell into a Bevy app.
///
/// Auto-adds [`bevy_egui::EguiPlugin`] if the host hasn't (so apps
/// migrating from `bevy_workbench` don't have to remember to add it).
pub struct WorkbenchPlugin;

/// Presentation policy for the offline recorder.
///
/// Headless/offscreen film capture leaves the workbench chrome out of the
/// render target. A native `--windowed-ui` capture explicitly opts into the
/// composed application surface so authored schema and telemetry shots record
/// the same panels a user sees.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct OfflineRecordingPresentation {
    /// Keep the title bar, dock, and workbench panels in a native UI capture.
    pub retain_workbench_chrome: bool,
}

impl Plugin for WorkbenchPlugin {
    fn build(&self, app: &mut App) {
        // Source browsing can read browser-served assets, so the shared
        // transport policy must exist even when the workbench is composed
        // without the dataset or updater plugins.
        lunco_settings::ensure_download_settings(app);
        // Survive transient GPU validation errors (e.g. the Windows
        // window-resize depth/color size mismatch) instead of panicking the
        // render thread. No-op when there's no RenderApp (headless/API-only).
        // The render-health systems install only after the host has selected
        // its explicit adapter/backend settings at `DefaultPlugins` build
        // time.
        lunco_render_recovery::install_wgpu_error_handler(app);

        // Screenshot backend. Its ABSENCE (a headless server, which links no workbench) is
        // what makes `CaptureScreenshot` reject cleanly there instead of deferring a
        // response that nothing would ever send.
        #[cfg(feature = "api")]
        {
            app.add_plugins(lunco_capture::screenshot::ScreenshotPlugin);
            if !app.is_plugin_added::<lunco_workspace_api::WorkspaceApiQueriesPlugin>() {
                app.add_plugins(lunco_workspace_api::WorkspaceApiQueriesPlugin);
            }
        }
        if !app.is_plugin_added::<bevy_egui::EguiPlugin>() {
            app.add_plugins(bevy_egui::EguiPlugin {
                // egui owns the workbench chrome and its transient surfaces:
                // menus, status history, dialogs, and tooltips. Runtime-authored
                // HUI is the scene HUD and must remain underneath those surfaces;
                // otherwise a HUD can paint over an egui popup even when the
                // popup is in egui's Foreground layer. The scene viewport remains
                // below both UI systems, so this changes only UI-vs-UI ordering.
                ui_render_order: bevy_egui::UiRenderOrder::EguiAboveBevyUi,
                ..Default::default()
            });
        }
        // `bevy_egui` may auto-create its primary context in `PreStartup`,
        // before the viewport plugin's `Startup` host setup runs. Disable that
        // creation now so the workbench owns the one primary camera and can
        // install its shared scene-target composition contract.
        app.world_mut()
            .resource_mut::<bevy_egui::EguiGlobalSettings>()
            .auto_create_primary_context = false;
        app.add_systems(
            EguiPrimaryContextPass,
            lunco_render_recovery::draw_render_recovery_banner.in_set(ApplicationOverlayRenderSet),
        );
        app.configure_sets(
            EguiPrimaryContextPass,
            ApplicationOverlayRenderSet.after(WorkbenchRenderSet),
        );
        // Egui host + viewport-geometry sync + invariant sentinels.
        // See `viewport.rs` doc-comment for the layered full-window scene
        // architecture. Auto-added so hosts don't have to
        // remember to wire it up.
        if !app.is_plugin_added::<viewport::WorkbenchViewportPlugin>() {
            app.add_plugins(viewport::WorkbenchViewportPlugin);
        }
        if !app.is_plugin_added::<lunco_theme::ThemePlugin>() {
            app.add_plugins(lunco_theme::ThemePlugin);
        }
        app.register_settings_section::<WorkbenchAppearanceSettings>();
        app.register_settings_section::<lunco_render::CommunicationLineSettings>();
        // The mission-time spine (doc 19): `TimeTransport` is the single
        // play/pause + rate authority and `WorldTime` the derived view. Guarded so
        // contexts that also add it via `CelestialPlugin` / `UsdAnimationPlugin` are
        // fine. Adding it on the workbench shell makes the transport present
        // wherever the toolbar Pause button lives — including modelica-only
        // `lunica`, which has no celestial/USD plugins — so the button drives the
        // same authority as the avatar hotkey and mission-control panel.
        if !app.is_plugin_added::<lunco_time::TimePlugin>() {
            app.add_plugins(lunco_time::TimePlugin);
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
        if !app.is_plugin_added::<lunco_status_core::status_bus::StatusBusPlugin>() {
            app.add_plugins(lunco_status_core::status_bus::StatusBusPlugin);
        }
        // Perf HUD (FPS / frame ms / optional physics ms) wired into
        // the right end of the status bar. Off by default; flip via
        // the `TogglePerfHud` typed command.
        // The blackout badge — "commands are not reaching this vessel". Reads the
        // same `ControlPathRegistry` the authorization gate refuses on, so the
        // indicator and the refusal can never disagree. Draws nothing until a
        // mission declares a blackout.
        if !app.is_plugin_added::<control_status::ControlStatusPlugin>() {
            app.add_plugins(control_status::ControlStatusPlugin);
        }
        // Guided presentation is an optional host-level plugin. The workbench
        // only publishes the generic render-set and anchor contracts it uses.
        if !app.is_plugin_added::<perf_hud::PerfHudPlugin>() {
            app.add_plugins(perf_hud::PerfHudPlugin);
        }
        // Input overlay visualizer for video recording & AI observation.
        if !app
            .world()
            .contains_resource::<lunco_input_ui::InputOverlaySettings>()
        {
            lunco_input_ui::build_input_overlay(app);
        } else {
            // A host may install the render-free command substrate before the
            // workbench. Preserve the command surface even when its egui panel
            // is already owned by that host.
            lunco_input_ui::register_input_overlay_commands(app);
        }
        if !app.is_plugin_added::<theme_command::ThemeCommandPlugin>() {
            app.add_plugins(theme_command::ThemeCommandPlugin);
        }
        if !app.is_plugin_added::<lunco_workbench_window::WindowCommandPlugin>() {
            app.add_plugins(lunco_workbench_window::WindowCommandPlugin);
        }
        if !app.is_plugin_added::<perspective_command::PerspectiveCommandPlugin>() {
            app.add_plugins(perspective_command::PerspectiveCommandPlugin);
        }
        // Persist & restore primary-window geometry (size / position /
        // maximized) via `lunco-settings`. Native-only; no-op on wasm.
        if !app.is_plugin_added::<lunco_workbench_window::WindowPersistencePlugin>() {
            app.add_plugins(lunco_workbench_window::WindowPersistencePlugin);
        }
        // Per-Twin (per-project) volatile UI state — active perspective +
        // open-document list — keyed by Twin path, VSCode `workspaceStorage`
        // style. Persistence is a reusable capability; this adapter is the
        // only concrete-shell bridge for dock capture and restore.
        if !app.is_plugin_added::<lunco_workbench_state::WorkspaceStatePlugin>() {
            app.add_plugins(lunco_workbench_state::WorkspaceStatePlugin::new(
                WorkbenchLayoutStateProvider,
            ));
        }
        // Plugin-driven registry of document kinds. Domain crates
        // (modelica, future julia/usd/sysml/...) register their kinds
        // here; consumers iterate the registry rather than matching
        // a fixed enum. Idempotent — domain plugins can also call
        // `init_resource::<DocumentKindRegistry>()` themselves.
        if !app.is_plugin_added::<lunco_twin::DocumentKindRegistryPlugin>() {
            app.add_plugins(lunco_twin::DocumentKindRegistryPlugin);
        }
        // Shell-level picker/file-workflow commands (`ShowOpenFilePicker`,
        // `OpenFolder`, `OpenTwin`, `SaveAll`, `SaveAsTwin`) + the
        // picker→command routing observer. `OpenFile` is the shared document
        // command; domain crates contribute their own
        // observers for verbs that need domain-specific handling
        // (e.g. modelica's `on_open_file` reads `.mo` content).
        if !app.is_plugin_added::<lunco_workbench_file_ops::FileOpsPlugin>() {
            app.add_plugins(lunco_workbench_file_ops::FileOpsPlugin);
        }
        if !app.is_plugin_added::<lunco_workbench_text_editor::TextEditorPlugin>() {
            app.add_plugins(lunco_workbench_text_editor::TextEditorPlugin);
        }
        if !app.is_plugin_added::<perspective_help::PerspectiveHelpPlugin>() {
            app.add_plugins(perspective_help::PerspectiveHelpPlugin);
        }
        app.init_resource::<WorkbenchLayout>()
            .init_resource::<WorkbenchMenuRegistry>()
            .init_resource::<WorkbenchSnapshot>()
            .init_resource::<lunco_interaction_core::SceneInteractionMode>()
            .init_resource::<OfflineRecordingPresentation>()
            .init_resource::<PendingTabRequests>()
            .init_resource::<PendingLayoutRequests>()
            .init_resource::<PendingPanelFocus>()
            .init_resource::<HelpAnchors>()
            .init_resource::<DockSizes>()
            .init_resource::<PendingTabCloses>()
            .init_resource::<twin_settings::TwinSettingsView>()
            // Cross-domain URI registry. Starts empty; each domain
            // plugin (lunco-modelica-core, a future USD command domain, …) pushes
            // its own handler on build. See `uri.rs` for the trait.
            .init_resource::<UriRegistry>()
            .init_resource::<CurrentSceneName>()
            .init_resource::<CurrentScenePath>()
            .add_observer(on_open_tab)
            .add_observer(on_open_tab_preserve_focus)
            .add_observer(on_close_tab)
            .add_observer(twin_settings::clear_on_twin_closed)
            .add_systems(
                Update,
                twin_settings::refresh_view.run_if(resource_changed::<WorkspaceResource>),
            )
            .add_systems(
                Update,
                // A navigation gesture may request both a perspective and a
                // tab while the layout is extracted for egui rendering. Apply
                // the layout first so the tab lands in the requested
                // perspective, not the outgoing one.
                (
                    drain_registered_panels,
                    drain_pending_layout_requests,
                    drain_pending_tab_requests,
                    sync_workbench_snapshot,
                )
                    .chain(),
            )
            .add_systems(First, perspective::sync_scene_interaction_mode);
        register_all_commands(app);
        app.register_panel(twin_settings::TwinSettingsPanel::default());
        drain_registered_panels(app.world_mut());
        app.add_systems(
            EguiPrimaryContextPass,
            render_workbench.in_set(WorkbenchRenderSet),
        )
        // Scene picking is handled by bevy_picking (egui occlusion via
        // bevy_egui's picking backend) — no scene-pointer resource, no gate.
        .add_systems(
            bevy::prelude::Update,
            (maintain_dock_widths, drain_pending_panel_focus),
        )
        .add_systems(
            Startup,
            (
                register_graphics_settings_menu,
                register_workbench_appearance_settings_menu,
            ),
        );
    }
}

/// Transfer renderer-neutral registrations into the concrete shell once its
/// layout exists. Registrations made before the shell are retained by the
/// contract registry until this boundary; registrations made after the shell
/// are picked up on the next update.
fn drain_registered_panels(world: &mut World) {
    if !world.contains_resource::<WorkbenchLayout>() {
        return;
    }
    let (panels, instance_panels) = {
        let Some(mut registry) = world.get_resource_mut::<WorkbenchPanelRegistry>() else {
            return;
        };
        (registry.take_panels(), registry.take_instance_panels())
    };
    let mut layout = world.resource_mut::<WorkbenchLayout>();
    for panel in panels {
        layout.register_boxed(panel);
    }
    for panel in instance_panels {
        layout.register_instance_panel_boxed(panel);
    }
}

/// Extension trait on [`App`] for ergonomic panel + perspective registration.
pub trait WorkbenchAppExt {
    /// Register a perspective. The first perspective registered becomes
    /// active and its layout plan seeds the initial slot assignments.
    fn register_perspective<W: Perspective + 'static>(&mut self, perspective: W) -> &mut Self;

    /// Register help content for a perspective.
    fn register_perspective_help(&mut self, id: PerspectiveId, help: PerspectiveHelp) -> &mut Self;
}

impl WorkbenchAppExt for App {
    fn register_perspective<W: Perspective + 'static>(&mut self, perspective: W) -> &mut Self {
        if !self.world().contains_resource::<WorkbenchLayout>() {
            self.init_resource::<WorkbenchLayout>();
        }
        drain_registered_panels(self.world_mut());
        let id = perspective.id();
        self.world_mut()
            .resource_mut::<WorkbenchLayout>()
            .register_perspective(perspective);
        if !self.world().contains_resource::<WorkbenchSnapshot>() {
            self.init_resource::<WorkbenchSnapshot>();
        }
        self.world_mut()
            .resource_mut::<WorkbenchSnapshot>()
            .register_perspective(id);
        self
    }

    fn register_perspective_help(&mut self, id: PerspectiveId, help: PerspectiveHelp) -> &mut Self {
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
            let title = self
                .world()
                .resource::<WorkbenchLayout>()
                .perspectives
                .iter()
                .find(|perspective| perspective.id() == id && perspective.show_in_switcher())
                .map(|perspective| perspective.title());
            if let Some(title) = title {
                if !self.world().contains_resource::<WorkbenchMenuRegistry>() {
                    self.init_resource::<WorkbenchMenuRegistry>();
                }
                let mut menus = self.world_mut().resource_mut::<WorkbenchMenuRegistry>();
                perspective_help::register_help_menu_item(&mut menus, id, title);
            }
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
    windows: Query<&bevy::window::Window, bevy::prelude::With<bevy::window::PrimaryWindow>>,
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
/// Replace the `null`s a serialized dock tree uses for non-finite `f32`s.
///
/// JSON has no NaN/Inf, so `serde_json` writes any non-finite `f32` as `null`
/// — which then refuses to deserialize back into `f32`, failing the *entire*
/// layout parse. Two independent sources produce them:
///
/// - `"fraction": null` — a split poisoned by the egui_dock `0.0 / 0.0` bug
///   (see [`sanitize_dock_fractions`]). Healed to `0.5`.
/// - `rect` / `viewport` coordinates — `egui::Rect::NOTHING` is `±infinity`,
///   so any node egui hasn't laid out yet serializes as `null`. Healed to
///   `0.0`; egui recomputes every rect on the next `show`, so the value is
///   irrelevant as long as it parses.
///
/// Without this pre-pass the user silently loses their dock on every launch,
/// and `sanitize_dock_fractions` never gets to run — there is no `DockState`
/// to sanitize yet.
pub(crate) fn heal_non_finite_nulls(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                match (key.as_str(), v.is_null()) {
                    ("fraction", true) => *v = serde_json::json!(0.5),
                    ("x" | "y", true) => *v = serde_json::json!(0.0),
                    _ => heal_non_finite_nulls(v),
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(heal_non_finite_nulls),
        _ => {}
    }
}

pub(crate) fn sanitize_dock_fractions(dock: &mut DockState<TabId>) {
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
