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
//! - **Command palette** — `Ctrl+P` unbound.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use egui_dock::{
    widgets::tab_viewer::OnCloseResponse, DockArea, DockState, NodeIndex, Style, TabViewer,
};
use lunco_core::{on_command, register_commands, Command};
use lunco_settings::{AppSettingsExt, SettingsSection};
use lunco_theme::ColorAlpha;
use std::collections::HashMap;

pub mod icons;
pub use icons::{icon_button, icon_text_button, paint_icon, UiIcon};

mod editor_tabs;
mod menu;
mod panel;
mod perspective;
mod perspective_help;
mod render_robustness;
mod session;
mod source_viewer;
mod viewport;

pub mod control_status;
pub mod file_ops;
pub mod files_panel;
pub mod input_overlay;
pub mod perf_hud;
pub mod perspective_command;
pub mod picker;
/// Screenshot capture — the render-bound half of `CaptureScreenshot`. Here, and not in
/// `lunco-api`, so that crate cannot link a GPU stack; and not in `lunco-render-bevy`,
/// because lunica screenshots its egui workbench with no 3D renderer.
#[cfg(feature = "api")]
pub mod screenshot;
pub mod status_bus;
pub mod theme_command;
pub mod tracked_task;
pub mod tutorial_overlay;
pub mod twin_browser;
pub mod uri;
pub mod window_command;
pub mod window_persistence;
pub mod window_placement;
pub mod workspace_state;

pub use perspective_help::{
    HelpMouse, HelpPopup, HelpShortcut, HelpTourRequest, LiveHelpSection, LiveHelpSections,
    PerspectiveHelp, PerspectiveHelpPlugin, PerspectiveHelpRegistry,
};
pub use render_robustness::{RenderGaveUp, RenderHealth, RenderHealthHandle, RenderWarning};

/// Register the render-recovery reset at the host application's scene-teardown
/// boundary. The workbench owns GPU state but deliberately does not depend on
/// the USD scene lifecycle crate; the composition root supplies its schedule
/// label when both concerns are present.
pub fn install_render_recovery_teardown<S: bevy::ecs::schedule::ScheduleLabel>(
    app: &mut App,
    schedule: S,
) {
    app.add_systems(schedule, render_robustness::reset_render_recovery);
}
pub use window_command::{
    merged_titlebar_window, CloseWindow, MaximizeWindow, MinimizeWindow, WindowMaximized,
};
pub use window_persistence::{
    load_window_geometry, restored_window, SkipWindowGeometrySave, WindowGeometry,
    WindowPersistencePlugin, DEFAULT_WINDOW_HEIGHT, DEFAULT_WINDOW_WIDTH,
};
pub use window_placement::wire_window_placement;
#[cfg(not(target_arch = "wasm32"))]
pub use window_placement::WindowPlacement;
pub use workspace_state::{
    finalize_revision, revision_term, workspace_state_path, AppDocumentSessionExt,
    DocumentSessionCodec, DocumentSessionRegistry, DocumentSnapshot, WorkspaceState,
    WorkspaceStatePlugin,
};

pub use menu::{MenuCtx, UndoProbeCtx};
pub use panel::{
    InstancePanel, InstancePanelMenuEntry, Panel, PanelCtx, PanelId, PanelMenuGroup,
    PanelScrollPolicy, PanelSlot, TabId,
};

/// SystemSet that runs the main workbench egui pass. Use
/// `.after(WorkbenchRenderSet)` for systems that need to read
/// the rects the workbench just published (e.g. help-tour overlays
/// reading [`HelpAnchors`]).
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorkbenchRenderSet;

/// System set for application-owned transient surfaces that must be rendered
/// after the workbench and authored Bevy UI. The egui order of each surface
/// still controls modal-vs-nonmodal precedence inside this final UI pass.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ApplicationOverlayRenderSet;

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
pub use editor_tabs::{EditorTab, EditorTabId, EditorTabs};
pub use files_panel::{FilesPanel, FILES_PANEL_ID};
pub use twin_browser::{
    BrowserAction, BrowserActions, BrowserCtx, BrowserSection, BrowserSectionRegistry,
    FilesSection, LuncoLibrarySection, TwinBrowserPanel, UnsavedDocEntry, UnsavedDocs,
    TWIN_BROWSER_PANEL_ID,
};
pub use uri::{UriClicked, UriHandler, UriRegistry, UriResolution};

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

/// Request opening a multi-instance tab while preserving the currently
/// focused tab. Used when creating a secondary view from an active editor or
/// graph; ordinary navigation continues to use [`OpenTab`].
#[derive(Event, Clone, Copy, Debug)]
pub struct OpenTabPreserveFocus {
    /// The [`InstancePanel::kind`] to open.
    pub kind: PanelId,
    /// The tab's instance discriminant.
    pub instance: u64,
    /// Explicit tab to restore when the caller has a more precise focus source.
    pub restore: Option<TabId>,
}

/// Request the workbench close a multi-instance tab, if open.
#[derive(Event, Clone, Copy, Debug)]
pub struct CloseTab {
    /// The [`InstancePanel::kind`] to close.
    pub kind: PanelId,
    /// The tab's instance discriminant.
    pub instance: u64,
}

/// Name of the binary actually running, for the Help menu's build line.
///
/// This crate is a LIBRARY shared by every workbench app (`luncosim`, `lunica`,
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

/// Bring a registered singleton panel forward in the dock, mounting it in its
/// authored default slot when it is currently closed.
///
/// `id` is matched against [`Panel::id`]'s static string (e.g.
/// `"modelica_experiments"`, `"modelica_telemetry"`). An unregistered panel
/// is a no-op; a registered closed panel is opened in its authored default
/// slot.
///
/// Exposed as a typed command so HTTP automation can deterministically
/// reach a tab before screenshotting / driving it.
#[Command(default)]
pub struct FocusPanel {
    /// The singleton panel's [`PanelId`] string (e.g.
    /// `"modelica_experiments"`).
    pub id: String,
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
        // A tutorial's named anchor is an actionable request, not a promise
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

/// Drain panel-navigation intents emitted by Twin Browser sections.
///
/// Sections are rendered while `WorkbenchLayout` is temporarily removed from
/// the world, so they cannot focus a panel inline. Keeping this small bridge
/// in the shell gives every domain the same navigation path and leaves panel
/// ownership with the workbench.
fn drain_browser_navigation(world: &mut World) {
    let actions = {
        let Some(mut outbox) = world.get_resource_mut::<BrowserActions>() else {
            return;
        };
        outbox.take_where(|action| matches!(action, BrowserAction::OpenPanel { .. }))
    };
    if actions.is_empty() {
        return;
    }

    let Some(mut layout) = world.get_resource_mut::<WorkbenchLayout>() else {
        bevy::log::warn!("Twin Browser emitted panel navigation before WorkbenchLayout was ready");
        return;
    };
    for action in actions {
        let BrowserAction::OpenPanel { id } = action else {
            unreachable!("browser navigation filter returned a non-navigation action")
        };
        focus_panel_now(&mut layout, &id);
    }
}

register_commands!(on_focus_panel,);

// ─────────────────────────────────────────────────────────────────────────────
// OpenSourceView — open a file's text in the read-only source viewer
// ─────────────────────────────────────────────────────────────────────────────

/// Open a registered asset as read-only text in the source viewer panel.
///
/// Fired by the LunCo Library browser section when any file is clicked —
/// uniformly for every source extension (`.usda`, `.rhai`, `.mo`, `.btxml`,
/// `.wgsl`), because the library is a *browse + read* surface, not a load
/// surface. Distinct from [`OpenFile`](lunco_doc_bevy::OpenFile) on purpose:
/// `OpenFile` is extension-routed (USD and Modelica each claim their own types
/// and open their native editors), so routing the library through it would
/// double-open `.usda`/`.mo` (their observers fire too). `OpenSourceView` has
/// exactly one observer — the workbench source viewer — so there is no conflict.
///
/// The command and its viewer live here because the LunCo Library is a
/// workbench built-in and must behave consistently in every workbench host.
#[Command(default)]
pub struct OpenSourceView {
    /// Registered `AssetFile::asset_path`; arbitrary filesystem paths are not
    /// accepted by this library-only command.
    pub asset_path: String,
}

/// Open an ephemeral generated document in the read-only source viewer.
#[Command(default)]
pub struct OpenEphemeralSource {
    /// URI shown as the document identity.
    pub uri: String,
    /// Complete generated source text.
    pub text: String,
}

/// Open one file belonging to an open Twin in the editable source panel.
#[Command(default)]
pub struct OpenTwinSource {
    /// Absolute root of the already-open Twin.
    pub twin_root: String,
    /// File path relative to that root.
    pub relative_path: String,
    /// Keep the file open when another preview is selected.
    pub pinned: bool,
}

/// Persist the editable source buffer, optionally refreshing its owning domain.
#[Command(default)]
pub struct SaveSourceText {
    /// Absolute root of the already-open Twin.
    pub twin_root: String,
    /// File path relative to that root.
    pub relative_path: String,
    /// Complete UTF-8 source text.
    pub text: String,
    /// Re-dispatch `OpenFile` after writing so the owning domain updates.
    pub update: bool,
}

pub use perspective::{Perspective, PerspectiveId};
// The session binding (WorkspaceResource, WorkspacePlugin, add/close events)
// lives in `lunco-workspace` now — consumers import it from there directly.
// `session` here is just the workbench-side recents persistence.
use lunco_workspace::WorkspaceResource;
pub use viewport::{
    EguiPointerState, PanelRect, PanelRects, ScenePickGate, SceneTarget, ViewportPanel,
    ViewportPlaceholder, WorkbenchEguiHost, WorkbenchViewportPlugin, VIEWPORT_PANEL_ID,
};

/// Get the backdrop colour from the active theme.
fn get_panel_backdrop(theme: &lunco_theme::Theme) -> egui::Color32 {
    theme.colors.mantle
}

/// Persisted workbench appearance preferences.
///
/// This is shell-level presentation state rather than a panel preference: the
/// workbench owns the dock body, while `PanelCtx` exposes the same decision to
/// panel-owned content cards. Keeping the decision here prevents one tab from
/// accidentally becoming transparent while another still paints an opaque
/// rectangle over the scene.
#[derive(
    Resource, serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq, Debug, Default,
)]
pub struct WorkbenchAppearanceSettings {
    /// Let the scene show through every dock/tab content body and its standard
    /// panel surface. The default keeps the themed mantle surface everywhere.
    #[serde(default)]
    pub transparent_tab_content: bool,
}

impl SettingsSection for WorkbenchAppearanceSettings {
    const KEY: &'static str = "workbench_appearance";
}

/// Whether the current panel body should reveal the scene below the workbench.
///
/// The main scene viewport is always transparent because it is the scene host;
/// ordinary panels follow the global appearance setting. The declared panel
/// value remains the fallback for standalone panel harnesses that do not install
/// the workbench settings resource.
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
        .map(|settings| settings.transparent_tab_content)
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
        // Survive transient GPU validation errors (e.g. the Windows
        // window-resize depth/color size mismatch) instead of panicking the
        // render thread. No-op when there's no RenderApp (headless/API-only).
        // The render-health systems install only after the host has selected
        // its explicit adapter/backend settings at `DefaultPlugins` build
        // time.
        render_robustness::install_wgpu_error_handler(app);

        // Screenshot backend. Its ABSENCE (a headless server, which links no workbench) is
        // what makes `CaptureScreenshot` reject cleanly there instead of deferring a
        // response that nothing would ever send.
        #[cfg(feature = "api")]
        app.add_plugins(screenshot::ScreenshotPlugin);
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
        app.add_systems(
            EguiPrimaryContextPass,
            render_robustness::draw_render_recovery_banner.in_set(ApplicationOverlayRenderSet),
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
        // contexts that also add it via `CelestialPlugin` / `UsdBevyPlugin` are
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
        if !app.is_plugin_added::<status_bus::StatusBusPlugin>() {
            app.add_plugins(status_bus::StatusBusPlugin);
        }
        // Perf HUD (FPS / frame ms / optional physics ms) wired into
        // the right end of the status bar. Off by default; flip via
        // the `TogglePerfHud` typed command.
        if !app.is_plugin_added::<tutorial_overlay::TutorialOverlayPlugin>() {
            app.add_plugins(tutorial_overlay::TutorialOverlayPlugin);
        }
        // The blackout badge — "commands are not reaching this vessel". Reads the
        // same `ControlPathRegistry` the authorization gate refuses on, so the
        // indicator and the refusal can never disagree. Draws nothing until a
        // mission declares a blackout.
        if !app.is_plugin_added::<control_status::ControlStatusPlugin>() {
            app.add_plugins(control_status::ControlStatusPlugin);
        }
        // NOTE: guided tours are now driven by rhai scenarios (the coach card is
        // rendered by `tutorial_overlay` and advanced by the running scenario's
        // `on_event`). The old data-driven `tour_driver` (`TourCatalog`/`TourDef`)
        // had zero registrants once lunica moved to rhai tutorials and was removed.
        if !app.is_plugin_added::<perf_hud::PerfHudPlugin>() {
            app.add_plugins(perf_hud::PerfHudPlugin);
        }
        // Input overlay visualizer for video recording & AI observation.
        if !app
            .world()
            .contains_resource::<input_overlay::InputOverlaySettings>()
        {
            input_overlay::build_input_overlay(app);
        } else {
            // A host may install the render-free command substrate before the
            // workbench. Preserve the command surface even when its egui panel
            // is already owned by that host.
            input_overlay::register_input_overlay_commands(app);
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
        // Plugin-driven registry of document kinds. Domain crates
        // (modelica, future julia/usd/sysml/...) register their kinds
        // here; consumers iterate the registry rather than matching
        // a fixed enum. Idempotent — domain plugins can also call
        // `init_resource::<DocumentKindRegistry>()` themselves.
        if !app.is_plugin_added::<lunco_twin::DocumentKindRegistryPlugin>() {
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
            .init_resource::<OfflineRecordingPresentation>()
            .init_resource::<PendingTabRequests>()
            .init_resource::<PendingLayoutRequests>()
            .init_resource::<PendingPanelFocus>()
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
            .init_resource::<EditorTabs<source_viewer::SourceTabState>>()
            .init_resource::<source_viewer::PendingSourceRequests>()
            .init_resource::<source_viewer::PendingSourceReads>()
            .init_resource::<source_viewer::PendingSourceWrites>()
            // Cross-domain URI registry. Starts empty; each domain
            // plugin (lunco-modelica, a future lunco-usd, …) pushes
            // its own handler on build. See `uri.rs` for the trait.
            .init_resource::<UriRegistry>()
            .init_resource::<CurrentSceneName>()
            .init_resource::<CurrentScenePath>()
            .add_observer(on_open_tab)
            .add_observer(on_open_tab_preserve_focus)
            .add_observer(on_close_tab)
            .add_systems(
                Update,
                (drain_pending_tab_requests, drain_pending_layout_requests).chain(),
            )
            .add_systems(
                Update,
                (
                    source_viewer::drain_pending_source_requests,
                    source_viewer::drain_pending_source_reads,
                    source_viewer::drain_pending_source_writes,
                    source_viewer::drain_source_tab_closes,
                )
                    .chain(),
            );
        register_all_commands(app);
        source_viewer::__register_on_open_file_for_text(app);
        source_viewer::__register_on_open_source_view(app);
        source_viewer::__register_on_open_ephemeral_source(app);
        source_viewer::__register_on_open_twin_source(app);
        source_viewer::__register_on_save_source_text(app);
        app.register_instance_panel(source_viewer::SourceEditorPanel);
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
        .add_systems(bevy::prelude::Update, drain_browser_navigation)
        .add_systems(
            Startup,
            (
                register_graphics_settings_menu,
                register_workbench_appearance_settings_menu,
            ),
        );

        // Built-in Files section ships with the workbench so apps get
        // a usable browser even before any domain plugin registers.
        // Registered after init_resource so the registry definitely
        // exists. Domain crates push their sections (Modelica, USD, …)
        // from their own plugin's build, which runs after ours.
        app.world_mut()
            .resource_mut::<BrowserSectionRegistry>()
            .register(FilesSection::default());
        // LunCo Library: the engine's bundled `assets/`, listed above Files
        // (order 150 < 200). Names only; click opens as read-only text via
        // `OpenSourceView`. Registered here, next to FilesSection, so every app
        // gets the reference collection without a per-app hook.
        app.world_mut()
            .resource_mut::<BrowserSectionRegistry>()
            .register(twin_browser::LuncoLibrarySection::default());
    }
}

/// Holds the name of the currently loaded USD scene file to display in the status bar.
#[derive(Resource, Clone, Default, Debug, Reflect)]
#[reflect(Resource, Default)]
pub struct CurrentSceneName(pub String);

/// Holds the canonical path used to load the currently displayed USD scene.
///
/// The status bar owns the display affordance, while the luncosim host updates
/// this resource at the typed `LoadScene` boundary. Keeping the path beside the
/// display name means a click can reveal the exact source without making the UI
/// parse or reconstruct a path from a filename.
#[derive(Resource, Clone, Default, Debug, Reflect)]
#[reflect(Resource, Default)]
pub struct CurrentScenePath(pub String);

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
    /// Presentation-owned perspective required by an active guided flow.
    ///
    /// A tutorial may point at view-local `HelpAnchors`. While that flow is
    /// active, switching to a perspective that cannot publish those anchors
    /// would turn an ordinary user action into a tutorial failure. The
    /// tutorial sets this at its launch boundary and clears it when the flow
    /// ends; every perspective entry point is then constrained in one place.
    required_perspective: Option<String>,
    pub(crate) activity_bar: bool,

    // Slot intent — kept so perspectives can rebuild the dock when activated.
    // User drags after that mutate `dock` directly; intent goes stale until
    // the next perspective activation. Each side slot is a Vec so multiple
    // panels can be tabbed in the same dock region. The secondary vectors
    // describe the optional lower leaf used by the split Build layout.
    pub(crate) side_browser: Vec<PanelId>,
    pub(crate) side_browser_bottom: Vec<PanelId>,
    pub(crate) center: Vec<PanelId>,
    pub(crate) active_center_tab: usize,
    pub(crate) right_inspector: Vec<PanelId>,
    pub(crate) right_inspector_bottom: Vec<PanelId>,
    pub(crate) bottom: Vec<PanelId>,

    /// App-wide Settings menu contributions. Domain plugins push a
    /// closure via [`WorkbenchLayout::register_settings`] at Startup;
    /// the closure is invoked each time the user opens the Settings
    /// drop-down. Keeps editor prefs / theme toggles / etc. in one
    /// discoverable place instead of scattered gear buttons.
    pub(crate) settings_menu:
        Vec<Box<dyn Fn(&mut bevy_egui::egui::Ui, &mut MenuCtx) + Send + Sync>>,

    /// Named, scrollable groups within Settings.  Use this for a coherent
    /// feature area with enough rows that keeping it in the root menu would
    /// obscure unrelated preferences.
    pub(crate) settings_submenus: Vec<(
        String,
        Vec<Box<dyn Fn(&mut bevy_egui::egui::Ui, &mut MenuCtx) + Send + Sync>>,
    )>,

    /// App-wide Edit menu contributions. Same pattern as
    /// [`settings_menu`](Self::settings_menu) — domain plugins push a
    /// closure via [`WorkbenchLayout::register_edit_menu`] at Startup so
    /// the global Edit menu can host domain-specific verbs (e.g. the
    /// code editor's Cut/Copy/Paste) without each plugin scattering its
    /// own toolbar.
    pub(crate) edit_menu: Vec<Box<dyn Fn(&mut bevy_egui::egui::Ui, &mut MenuCtx) + Send + Sync>>,

    /// Undo/redo availability probes. Domain plugins push a closure via
    /// [`WorkbenchLayout::register_undo_probe`] at Startup; each probe
    /// inspects the active document and returns
    /// `Some((can_undo, can_redo))` when its domain owns that document,
    /// `None` otherwise. The Edit menu asks probes in registration order
    /// and the first `Some` wins — the same first-owner-wins contract as
    /// the `EditorIntent` resolvers. With no probe answering, the menu
    /// falls back to "a document is active" so a domain without a probe
    /// keeps working Undo/Redo entries.
    pub(crate) undo_probes: Vec<Box<dyn Fn(&UndoProbeCtx) -> Option<(bool, bool)> + Send + Sync>>,

    /// App-wide Help menu contributions. Same pattern as
    /// [`settings_menu`](Self::settings_menu) — domain plugins push a
    /// closure via [`WorkbenchLayout::register_help_menu`] at Startup
    /// so the Help drop-down can host tour / docs / about entries
    /// without each domain inventing its own help button.
    pub(crate) help_menu: Vec<Box<dyn Fn(&mut bevy_egui::egui::Ui, &mut MenuCtx) + Send + Sync>>,

    /// App-wide File menu contributions. Same pattern as
    /// [`settings_menu`](Self::settings_menu) — domain plugins push a
    /// closure via [`WorkbenchLayout::register_file_menu`] at Startup so
    /// the File menu can host domain-specific verbs (e.g. Load Example)
    /// without hardcoding them in `lunco-workbench`.
    pub(crate) file_menu: Vec<Box<dyn Fn(&mut bevy_egui::egui::Ui, &mut MenuCtx) + Send + Sync>>,

    /// App-wide Time menu contributions. Same pattern as
    /// [`settings_menu`](Self::settings_menu) — domain plugins push a
    /// closure via [`WorkbenchLayout::register_time_menu`] at Startup so
    /// clock-shaped controls (sim rate, the sky clock, epoch readouts)
    /// live under ONE discoverable menu instead of on the toolbar and in
    /// floating overlays. The toolbar keeps pause/resume and nothing else.
    pub(crate) time_menu: Vec<Box<dyn Fn(&mut bevy_egui::egui::Ui, &mut MenuCtx) + Send + Sync>>,

    /// Dynamic top-level menus contributed by domain plugins.
    pub(crate) custom_menus: Vec<(
        &'static str,
        Box<dyn Fn(&mut bevy_egui::egui::Ui, &mut MenuCtx) + Send + Sync>,
    )>,

    /// The live dock tree — what egui_dock actually renders. Stores
    /// [`TabId`]s so both singleton panels and multi-instance tabs
    /// coexist in the same tree.
    pub(crate) dock: DockState<TabId>,

    /// Per-perspective snapshots of the live dock + slot intent, so
    /// switching back to a perspective restores *its own* open tabs and
    /// split layout instead of a fresh preset (or the tabs another
    /// perspective left open). Keyed by [`PerspectiveId`]. A perspective
    /// is snapshotted on the way out (see [`Self::activate_perspective`])
    /// and restored on the way back; a first visit has no entry, so the
    /// perspective's preset is built fresh. This is what keeps Build's
    /// tabs and Design's tabs separate: each lives in its own dock tree.
    pub(crate) dock_cache: HashMap<PerspectiveId, PerspectiveDockSlot>,
}

/// Cached snapshot of one perspective's dock tree + slot intent — the
/// unit [`WorkbenchLayout::dock_cache`] stores per perspective so a
/// return visit restores the exact layout (tabs + splits + which centre
/// tab was active) the user left, rather than the preset.
#[derive(Clone)]
pub(crate) struct PerspectiveDockSlot {
    pub(crate) dock: DockState<TabId>,
    pub(crate) side_browser: Vec<PanelId>,
    pub(crate) side_browser_bottom: Vec<PanelId>,
    pub(crate) center: Vec<PanelId>,
    pub(crate) active_center_tab: usize,
    pub(crate) right_inspector: Vec<PanelId>,
    pub(crate) right_inspector_bottom: Vec<PanelId>,
    pub(crate) bottom: Vec<PanelId>,
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
            required_perspective: None,
            activity_bar: false,
            side_browser: Vec::new(),
            side_browser_bottom: Vec::new(),
            center: Vec::new(),
            active_center_tab: 0,
            right_inspector: Vec::new(),
            right_inspector_bottom: Vec::new(),
            bottom: Vec::new(),
            settings_menu: Vec::new(),
            settings_submenus: Vec::new(),
            edit_menu: Vec::new(),
            undo_probes: Vec::new(),
            help_menu: Vec::new(),
            file_menu: Vec::new(),
            time_menu: Vec::new(),
            custom_menus: Vec::new(),
            dock: DockState::new(Vec::new()),
            dock_cache: HashMap::new(),
        }
    }
}

impl WorkbenchLayout {
    /// Register a panel and dock it in its default slot.
    pub fn register<P: Panel + 'static>(&mut self, panel: P) {
        let id = panel.id();
        let slot = panel.default_slot();
        // A perspective may declare a panel before the domain plugin registers
        // its renderer. In that case the declared slot is authoritative: do
        // not append the panel to its authored default as well, or a stacked
        // Build preset silently turns back into one tab strip.
        let declared = self.side_browser.contains(&id)
            || self.side_browser_bottom.contains(&id)
            || self.center.contains(&id)
            || self.right_inspector.contains(&id)
            || self.right_inspector_bottom.contains(&id)
            || self.bottom.contains(&id);
        if !declared {
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
                PanelSlot::Hidden => { /* registered, intentionally not docked */ }
            }
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
        if let Some(path) = self.dock.find_tab(&tab) {
            self.dock.set_focused_node_and_surface(path.node_path());
            if let Err(e) = self.dock.set_active_tab(path) {
                bevy::log::warn!(
                    "open_instance: could not foreground tab {kind:?}#{instance} \
                     at {path:?}: {e:?}"
                );
            }
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
        let preferred_slot = self.instance_panels.get(&kind).map(|p| p.default_slot());
        // Build the set of singleton PanelIds occupying each slot so
        // we can find a leaf hosting any of them.
        let slot_ids: std::collections::HashSet<PanelId> = match preferred_slot {
            Some(PanelSlot::Center) => self.center.iter().copied().collect(),
            Some(PanelSlot::Bottom) => self.bottom.iter().copied().collect(),
            Some(PanelSlot::SideBrowser) => self.side_browser.iter().copied().collect(),
            Some(PanelSlot::RightInspector) => self.right_inspector.iter().copied().collect(),
            _ => std::collections::HashSet::new(),
        };
        let center_ids: std::collections::HashSet<PanelId> = self.center.iter().copied().collect();
        // The full-window 3D scene's leaf is EXCLUSIVE: an instance tab must never
        // be appended into it. That leaf's `ViewportPanel` renders nothing (the 3D
        // camera paints full-window behind it) but its render is what records the
        // scene-vs-chrome pick leaf; a co-tenant tab foregrounded there blanks the
        // viewport and swallows every click (the "opening a Graph kills the Build
        // controls" bug). We exclude it below and, if it's the only candidate,
        // split a fresh leaf beneath it instead.
        let vp_leaf = self.scene_viewport_leaf();
        let target_leaf = {
            let main = self.dock.main_surface_mut();
            // Priority 1: leaf already hosting another instance of
            // this kind — keeps families together.
            find_leaf_matching(
                main,
                |t| matches!(*t, TabId::Instance { kind: k, .. } if k == kind),
            )
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
            // Priority 4: any leaf at all, except that a kind whose
            // authoritative default is the Bottom dock must create a bottom
            // split when no bottom singleton has reserved one. Otherwise the
            // first leaf can be the left browser, making a default graph tab
            // appear in the wrong surface.
            .or_else(|| {
                if preferred_slot != Some(PanelSlot::Bottom) {
                    first_leaf(main)
                } else {
                    None
                }
            })
            // …but never the exclusive scene-viewport leaf.
            .filter(|n| Some(*n) != vp_leaf)
        };

        if let Some(leaf) = target_leaf {
            let main = self.dock.main_surface_mut();
            main[leaf].append_tab(tab);
            // Focus the just-appended tab.
            if let Some(count) = main[leaf].tabs_count().checked_sub(1) {
                if let Err(e) = main.set_active_tab(leaf, count) {
                    bevy::log::warn!(
                        "open_instance: appended {kind:?}#{instance} to leaf \
                         {leaf:?} but could not foreground it: {e:?}"
                    );
                }
            }
            // Focus the leaf/surface too so egui_dock foregrounds it.
            self.dock.set_focused_node_and_surface(egui_dock::NodePath {
                surface: egui_dock::SurfaceIndex::main(),
                node: leaf,
            });
        } else if let Some(vp) = vp_leaf {
            // Only the scene-viewport leaf is available (e.g. Build, whose Bottom
            // slot is empty). Split a fresh leaf BELOW the viewport (~30% tall) and
            // drop the tab there so the viewport keeps its own exclusive leaf.
            self.dock.main_surface_mut().split_below(vp, 0.7, vec![tab]);
            if let Some(path) = self.dock.find_tab(&tab) {
                self.dock.set_focused_node_and_surface(path.node_path());
                let _ = self.dock.set_active_tab(path);
            }
        } else {
            // Empty dock (e.g. 3D app with no center tabs). Seed a
            // single leaf with this tab so at least something shows.
            self.dock = DockState::new(vec![tab]);
        }
    }

    /// Open an instance tab without changing the user's current tab.
    pub fn open_instance_without_focus(
        &mut self,
        kind: PanelId,
        instance: u64,
        restore: Option<TabId>,
    ) {
        let previous = restore.or_else(|| {
            self.dock.main_surface().focused_leaf().and_then(|node| {
                match &self.dock.main_surface()[node] {
                    egui_dock::Node::Leaf(leaf) => leaf.tabs.get(leaf.active.0).cloned(),
                    _ => None,
                }
            })
        });
        self.open_instance(kind, instance);
        if let Some(previous) = previous {
            if let Some(path) = self.dock.find_tab(&previous) {
                self.dock.set_focused_node_and_surface(path.node_path());
                let _ = self.dock.set_active_tab(path);
            }
        }
    }

    /// Move an already-open instance tab to position 0 in its leaf so
    /// it renders as the leftmost tab. No-op if the tab isn't open.
    pub fn move_instance_to_front(&mut self, kind: PanelId, instance: u64) {
        let tab = TabId::Instance { kind, instance };
        let Some(path) = self.dock.find_tab(&tab) else {
            return;
        };
        if path.tab.0 == 0 {
            return;
        }
        let surface_ref = self
            .dock
            .get_surface_mut(path.surface)
            .and_then(|s| s.node_tree_mut());
        let Some(tree) = surface_ref else { return };
        if let Some(removed) = tree[path.node].remove_tab(path.tab) {
            tree[path.node].insert_tab(egui_dock::TabIndex(0), removed);
            let _ = tree.set_active_tab(path.node, egui_dock::TabIndex(0));
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
    pub fn move_tab_next_to(&mut self, src: TabId, sibling_of: TabId) -> Option<TabLocation> {
        let src_loc = self.dock.find_tab(&src)?;
        let sib = self.dock.find_tab(&sibling_of)?;
        if src_loc.surface == sib.surface && src_loc.node == sib.node {
            return Some(TabLocation {
                surface: src_loc.surface,
                node: src_loc.node,
                index: src_loc.tab,
            });
        }
        let saved = TabLocation {
            surface: src_loc.surface,
            node: src_loc.node,
            index: src_loc.tab,
        };
        self.dock.move_tab(
            src_loc,
            egui_dock::TabDestination::Node(
                sib.node_path(),
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
        if (src_loc.surface, src_loc.node) == (loc.surface, loc.node) {
            return;
        }
        // Validate the destination still exists and is a leaf.
        let dest_ok = self
            .dock
            .get_surface(loc.surface)
            .and_then(|s| s.node_tree())
            .map(|tree| loc.node.0 < tree.len() && tree[loc.node].is_leaf())
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
                egui_dock::NodePath {
                    surface: loc.surface,
                    node: loc.node,
                },
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
    pub fn enforce_widths(&mut self, window_w: f32, side_px: f32, right_px: f32) {
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
        if tree.is_empty() {
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
            let idx = if has_side { NodeIndex(2) } else { NodeIndex(0) };
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
                TabId::Instance { kind: k, instance } if *k == kind => Some(*instance),
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

    /// Register a perspective and store it in the switcher. If this is the
    /// first perspective added, it also becomes active and its `apply`
    /// runs immediately to seed the initial layout.
    /// Register a closure that contributes rows to the app-wide
    /// Settings drop-down in the menu bar. Called once per open of the
    /// menu; the closure reads through [`MenuCtx`] and queues typed intent for
    /// application after painting.
    ///
    /// Intended for domain plugins to expose editor / theme / pane
    /// preferences without each plugin inventing its own gear button. Callbacks
    /// may emit typed events or replace an existing resource through `MenuCtx`;
    /// they cannot receive a raw `World` mutation closure.
    pub fn register_settings<F>(&mut self, callback: F)
    where
        F: Fn(&mut bevy_egui::egui::Ui, &mut MenuCtx) + Send + Sync + 'static,
    {
        self.settings_menu.push(Box::new(callback));
    }

    /// Register rows under one named, scrollable Settings submenu. Multiple
    /// plugins can contribute to the same submenu without coupling to one
    /// another; the label is the single grouping key.
    pub fn register_settings_submenu<F>(&mut self, label: impl Into<String>, callback: F)
    where
        F: Fn(&mut bevy_egui::egui::Ui, &mut MenuCtx) + Send + Sync + 'static,
    {
        let label = label.into();
        if let Some((_, callbacks)) = self
            .settings_submenus
            .iter_mut()
            .find(|(existing, _)| existing == &label)
        {
            callbacks.push(Box::new(callback));
        } else {
            self.settings_submenus
                .push((label, vec![Box::new(callback)]));
        }
    }

    /// Register a closure that contributes entries to the global Edit
    /// menu. Mirrors [`register_settings`](Self::register_settings).
    pub fn register_edit_menu<F>(&mut self, callback: F)
    where
        F: Fn(&mut bevy_egui::egui::Ui, &mut MenuCtx) + Send + Sync + 'static,
    {
        self.edit_menu.push(Box::new(callback));
    }

    /// Register an undo/redo availability probe for the global Edit menu.
    ///
    /// The probe returns `Some((can_undo, can_redo))` for documents its
    /// domain owns (read the domain registry off [`UndoProbeCtx`]), `None` for
    /// anything else. First registered probe to answer wins — mirror of
    /// the `EditorIntent` resolver contract, so register exactly one per
    /// domain, next to [`register_edit_menu`](Self::register_edit_menu).
    pub fn register_undo_probe<F>(&mut self, probe: F)
    where
        F: Fn(&UndoProbeCtx) -> Option<(bool, bool)> + Send + Sync + 'static,
    {
        self.undo_probes.push(Box::new(probe));
    }

    /// Register a closure that contributes entries to the global Help
    /// menu. Mirrors [`register_settings`](Self::register_settings).
    pub fn register_help_menu<F>(&mut self, callback: F)
    where
        F: Fn(&mut bevy_egui::egui::Ui, &mut MenuCtx) + Send + Sync + 'static,
    {
        self.help_menu.push(Box::new(callback));
    }

    /// Register a closure that contributes entries to the global File
    /// menu. Mirrors [`register_settings`](Self::register_settings).
    pub fn register_file_menu<F>(&mut self, callback: F)
    where
        F: Fn(&mut bevy_egui::egui::Ui, &mut MenuCtx) + Send + Sync + 'static,
    {
        self.file_menu.push(Box::new(callback));
    }

    /// Register a closure that contributes entries to the global Time
    /// menu. Mirrors [`register_settings`](Self::register_settings).
    ///
    /// This is where a clock control belongs. The toolbar carries
    /// pause/resume alone, so anything that sets a rate, retargets a clock
    /// or shows an epoch goes here rather than into a floating overlay the
    /// user cannot turn off.
    pub fn register_time_menu<F>(&mut self, callback: F)
    where
        F: Fn(&mut bevy_egui::egui::Ui, &mut MenuCtx) + Send + Sync + 'static,
    {
        self.time_menu.push(Box::new(callback));
    }

    /// Register a custom top-level menu button.
    pub fn register_custom_menu<F>(&mut self, name: &'static str, callback: F)
    where
        F: Fn(&mut bevy_egui::egui::Ui, &mut MenuCtx) + Send + Sync + 'static,
    {
        self.custom_menus.push((name, Box::new(callback)));
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

    /// Switch to the named perspective. No-op if the id isn't registered.
    ///
    /// Each perspective keeps its **own** open tabs and split layout: on
    /// the way out the live dock (+ slot intent) is snapshotted into
    /// [`Self::dock_cache`], and on the way back a previously-snapshotted
    /// perspective is restored verbatim instead of being rebuilt from its
    /// preset. A first visit has no snapshot, so the preset is built fresh
    /// — and the live dock is cleared first so the new perspective doesn't
    /// inherit the outgoing one's tabs (the old "VSCode never closes
    /// editors" merge is what made Build show Design's tabs).
    pub fn activate_perspective(&mut self, id: PerspectiveId) {
        // Guided presentations own their authored chrome for the duration of
        // the flow. This applies equally to the title-bar switcher, the typed
        // API command, and internal callers because they all converge here.
        // An unknown/stale requirement is ignored so it cannot make ordinary
        // perspective switching unusable after a provider disappears.
        let id = self
            .required_perspective
            .as_deref()
            .and_then(|required| {
                self.perspectives
                    .iter()
                    .find(|perspective| perspective.id().as_str() == required)
                    .map(|perspective| perspective.id())
            })
            .unwrap_or(id);
        // Validate the target before mutating — an unknown id is a no-op
        // and must not snapshot/restore anything.
        if !self.perspectives.iter().any(|w| w.id() == id) {
            return;
        }
        let restores_cached_layout = self
            .perspectives
            .iter()
            .find(|w| w.id() == id)
            .is_some_and(|w| w.restores_cached_layout());
        let prev = self.active_perspective;
        let switching = prev != Some(id);

        // Snapshot the outgoing perspective's live layout on a real switch.
        if switching {
            if let Some(prev_id) = prev {
                self.snapshot_perspective(prev_id);
            }
            // Restore a visited perspective's cached layout verbatim — its
            // tabs and splits come back exactly as left, no preset rebuild.
            if restores_cached_layout {
                if let Some(slot) = self.dock_cache.remove(&id) {
                    self.restore_perspective(slot);
                    self.active_perspective = Some(id);
                    return;
                }
            } else {
                // A presentation workspace must never revive stale document tabs.
                self.dock_cache.remove(&id);
            }
            // First visit: drop the live dock so the incoming preset's
            // rebuild seeds an empty skeleton instead of merging the
            // outgoing perspective's instance tabs into it.
            self.dock = DockState::new(Vec::new());
        }

        // `ws.apply(self)` borrows `self` mutably while we hold a borrowed
        // `ws`, so take the registry out for the call (same dance the
        // original did).
        let perspectives = std::mem::take(&mut self.perspectives);
        if let Some(ws) = perspectives.iter().find(|w| w.id() == id) {
            ws.apply(self);
            self.active_perspective = Some(id);
        }
        self.perspectives = perspectives;
    }

    /// Snapshot the current live dock + slot intent under `id` so a later
    /// return to that perspective restores it. No-op as a storage detail —
    /// the caller has already decided to switch away.
    fn snapshot_perspective(&mut self, id: PerspectiveId) {
        self.dock_cache.insert(
            id,
            PerspectiveDockSlot {
                dock: self.dock.clone(),
                side_browser: self.side_browser.clone(),
                side_browser_bottom: self.side_browser_bottom.clone(),
                center: self.center.clone(),
                active_center_tab: self.active_center_tab,
                right_inspector: self.right_inspector.clone(),
                right_inspector_bottom: self.right_inspector_bottom.clone(),
                bottom: self.bottom.clone(),
            },
        );
    }

    /// Restore a previously-snapshotted perspective's dock + slot intent
    /// into the live fields. The cached tree already carries its own
    /// chrome and tabs, so no rebuild is needed (and none must run, or it
    /// would wipe the restored tabs).
    fn restore_perspective(&mut self, slot: PerspectiveDockSlot) {
        let PerspectiveDockSlot {
            dock,
            side_browser,
            side_browser_bottom,
            center,
            active_center_tab,
            right_inspector,
            right_inspector_bottom,
            bottom,
        } = slot;
        self.dock = dock;
        self.side_browser = side_browser;
        self.side_browser_bottom = side_browser_bottom;
        self.center = center;
        self.active_center_tab = active_center_tab;
        self.right_inspector = right_inspector;
        self.right_inspector_bottom = right_inspector_bottom;
        self.bottom = bottom;
    }

    /// Which perspective is currently active, if any.
    pub fn active_perspective(&self) -> Option<PerspectiveId> {
        self.active_perspective
    }

    /// Require a registered perspective while a presentation owns the
    /// workbench. Passing `None` returns perspective selection to the user.
    ///
    /// The value is intentionally a runtime string: curriculum providers
    /// author perspective ids, while [`PerspectiveId`] is a static registry
    /// key. The requirement is resolved against the registered perspectives
    /// at activation time and never persisted as workspace state.
    pub fn set_required_perspective(&mut self, id: Option<&str>) {
        self.required_perspective = id.map(str::to_owned);
    }

    /// Whether the host registered a perspective with this authored id.
    ///
    /// This is a read-only validation seam for data-driven callers. They can
    /// reject an unknown id before resetting or changing the active layout.
    pub fn has_perspective(&self, id: &str) -> bool {
        self.perspectives
            .iter()
            .any(|perspective| perspective.id().as_str() == id)
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
            // Drop any cached layout for this perspective and clear the live
            // dock so the re-activation rebuilds a *fresh* preset — without
            // this, `activate_perspective` would either restore the cached
            // (still-dirty) layout or preserve the current tabs.
            self.dock_cache.remove(&id);
            self.dock = DockState::new(Vec::new());
            self.activate_perspective(id);
        }
    }

    /// Reset the entire workbench presentation to its first-registered
    /// perspective and its authored slot preset. This is stronger than
    /// [`Self::reset_to_default_layout`]: opening a guided tutorial must not
    /// inherit the user's current perspective or any cached per-perspective
    /// tabs and splits.
    pub fn reset_to_default_perspective(&mut self) {
        let Some(id) = self
            .required_perspective
            .as_deref()
            .and_then(|required| {
                self.perspectives
                    .iter()
                    .find(|perspective| perspective.id().as_str() == required)
                    .map(|perspective| perspective.id())
            })
            .or_else(|| self.perspectives.first().map(|p| p.id()))
        else {
            return;
        };
        self.dock_cache.clear();
        self.dock = DockState::new(Vec::new());
        self.active_perspective = None;
        self.activate_perspective(id);
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
        for (path, node) in self.dock.iter_all_nodes() {
            path.surface.0.hash(&mut h);
            path.node.0.hash(&mut h);
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

    /// Reconcile a serialized dock tree against *this* app's live state,
    /// returning a fresh [`DockState<TabId>`] without touching the live
    /// dock:
    ///
    /// - **Singleton** tabs whose `PanelId` isn't registered here are
    ///   dropped (e.g. a `luncosim`-only panel loaded into `lunica`).
    /// - **Instance** tabs are remapped: each carries the *old* session's
    ///   instance id; `id_map` translates it to the freshly-restored id.
    ///   A tab whose kind isn't registered is dropped; one whose document
    ///   didn't restore (absent from `id_map`) is kept as-is (stable-id
    ///   tabs like the default plot, or a stale doc tab the codec re-opens).
    ///
    /// Empty leaves collapse via egui_dock's `retain_tabs`; non-finite
    /// split fractions are healed ([`sanitize_dock_fractions`]). Returns
    /// `None` when the JSON won't parse or nothing survived — the caller
    /// then keeps its current dock. Shared by the live restore path
    /// ([`set_dock_from_json`]) and the per-perspective cache seeding
    /// ([`seed_perspective_docks`]) so cached trees go through the same
    /// cross-app reconciliation as the active one.
    pub(crate) fn reconcile_dock(
        &self,
        value: serde_json::Value,
        id_map: &HashMap<(&'static str, u64), u64>,
    ) -> Option<DockState<TabId>> {
        use std::collections::HashSet;
        let valid_singletons: HashSet<&'static str> = self.panels.keys().map(|p| p.0).collect();
        let valid_kinds: HashSet<&'static str> = self.instance_panels.keys().map(|p| p.0).collect();

        // A NaN fraction (see `sanitize_dock_fractions`) serializes to JSON
        // `null`, which won't deserialize back into `f32` — so the load-time
        // sanitize below is unreachable unless we heal the `Value` first.
        let mut value = value;
        heal_non_finite_nulls(&mut value);

        let mut new_dock: DockState<TabId> = match serde_json::from_value(value) {
            Ok(d) => d,
            Err(e) => {
                warn!("[WorkspaceState] dock JSON parse failed: {e}; keeping default layout");
                return None;
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
        // A singleton panel is one renderer with one egui identity. A stale
        // layout may contain it in more than one leaf (older Build layouts put
        // Telemetry in both side and bottom), which makes egui render the same
        // widget tree twice and report ID collisions. Keep the first occurrence
        // in dock order; the current perspective then supplies its canonical
        // position on the next normal layout rebuild.
        let mut seen_singletons = HashSet::new();
        new_dock.retain_tabs(|tab| match tab {
            TabId::Singleton(pid) => {
                valid_singletons.contains(pid.0) && seen_singletons.insert(*pid)
            }
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

        new_dock.iter_all_tabs().next()?;
        // Heal any non-finite split fraction persisted to disk. egui_dock can
        // serialize a NaN fraction (see `sanitize_dock_fractions`), and a NaN
        // reloaded here would panic the dock layout on the very next frame —
        // a permanent boot-crash loop until the workspace cache is wiped.
        sanitize_dock_fractions(&mut new_dock);
        // Heal a blanked 3D viewport. The full-window scene `ViewportPanel`
        // (`scene_target() == MainViewport`) renders nothing itself — the 3D camera
        // paints full-window behind it — but its `render` is what records the
        // scene-vs-chrome pick leaf. A persisted layout (or a stray drag) can leave
        // another tab (e.g. a Modelica plot) tabbed onto the viewport's leaf;
        // foregrounded there it blanks the viewport, which then never records its
        // leaf, and every click in the still-visible 3D is swallowed as chrome (the
        // "3D visible but the middle of Build is dead" regression). Evict any such
        // co-tenant into its own leaf so the viewport is alone and live again.
        if let Some(vp_id) = self.scene_viewport_panel_id() {
            Self::evict_scene_viewport_cotenants(&mut new_dock, TabId::Singleton(vp_id));
        }
        Some(new_dock)
    }

    /// The registered singleton panel that IS the full-window 3D scene
    /// (`Panel::scene_target() == Some(SceneTarget::MainViewport)`), if any.
    /// App-agnostic — every workbench app that hosts a 3D scene registers exactly
    /// one such panel (the luncosim's `ViewportPanel`); tooling apps register none.
    /// Used to keep that panel foregrounded so a co-tenant tab can never blank the
    /// viewport controls (see [`reconcile_dock`](Self::reconcile_dock)).
    fn scene_viewport_panel_id(&self) -> Option<PanelId> {
        self.panels
            .iter()
            .find(|(_, p)| p.scene_target() == Some(SceneTarget::MainViewport))
            .map(|(id, _)| *id)
    }

    /// The dock leaf currently hosting the scene [`ViewportPanel`], if it's in the
    /// tree. That leaf is kept EXCLUSIVE (see [`open_instance`](Self::open_instance)
    /// and [`evict_scene_viewport_cotenants`](Self::evict_scene_viewport_cotenants))
    /// so no other tab can blank the 3D.
    fn scene_viewport_leaf(&self) -> Option<NodeIndex> {
        let vp = self.scene_viewport_panel_id()?;
        self.dock.find_tab(&TabId::Singleton(vp)).map(|p| p.node)
    }

    /// Move any tab sharing the scene viewport's leaf out into a fresh leaf split
    /// below it, so the viewport ends up alone. A persisted layout (or a stray
    /// drag) can leave e.g. a Modelica plot tabbed onto the viewport; foregrounded,
    /// it blanks the 3D and eats clicks. Operates on `dock` in place; no-op when the
    /// viewport isn't present or already has its leaf to itself.
    fn evict_scene_viewport_cotenants(dock: &mut DockState<TabId>, vp_tab: TabId) {
        let Some(pos) = dock.find_tab(&vp_tab) else {
            return;
        };
        let vp_node = pos.node;
        let cotenants: Vec<TabId> = match &dock.main_surface()[vp_node] {
            egui_dock::Node::Leaf(leaf) => {
                leaf.tabs.iter().copied().filter(|t| *t != vp_tab).collect()
            }
            _ => Vec::new(),
        };
        if cotenants.is_empty() {
            // Viewport already alone — just make sure it's the active tab.
            let _ = dock.set_active_tab(pos);
            return;
        }
        let main = dock.main_surface_mut();
        if let egui_dock::Node::Leaf(leaf) = &mut main[vp_node] {
            leaf.tabs.retain(|t| *t == vp_tab);
            leaf.active = egui_dock::TabIndex(0);
        }
        // Fresh leaf beneath the viewport (~30% tall) holds the evicted tabs.
        main.split_below(vp_node, 0.7, cotenants);
    }

    /// Replace the live dock from a previously [`dock_json`](Self::dock_json)
    /// snapshot (reconciled via [`reconcile_dock`]). Returns `false`
    /// (leaving the current dock untouched) when the JSON won't parse or the
    /// reconciled tree would be empty — the caller then keeps whatever the
    /// codec-driven open path produced.
    pub(crate) fn set_dock_from_json(
        &mut self,
        value: serde_json::Value,
        id_map: &HashMap<(&'static str, u64), u64>,
    ) -> bool {
        match self.reconcile_dock(value, id_map) {
            Some(d) => {
                self.dock = d;
                true
            }
            None => false,
        }
    }

    /// Capture every perspective's dock tree (+ slot intent) for hot-exit:
    /// each cached perspective's dock ([`dock_cache`]) plus the active
    /// perspective's **live** dock. Chrome-incomplete trees (a transient
    /// state mid-switch) are skipped so they don't round-trip as a layout
    /// with missing panels. Keyed by [`PerspectiveId`] string — the inverse
    /// of [`seed_perspective_docks`]. The active perspective is captured
    /// last so an id that's somehow both live and cached resolves to the
    /// live tree.
    pub(crate) fn capture_perspective_docks(
        &self,
    ) -> std::collections::HashMap<String, crate::workspace_state::PerspectiveDockSnapshot> {
        use crate::workspace_state::PerspectiveDockSnapshot;
        let mut out: std::collections::HashMap<String, PerspectiveDockSnapshot> =
            std::collections::HashMap::new();
        for (id, slot) in &self.dock_cache {
            if !self.chrome_complete(
                &slot.dock,
                &slot.side_browser,
                &slot.side_browser_bottom,
                &slot.center,
                &slot.right_inspector,
                &slot.right_inspector_bottom,
                &slot.bottom,
            ) {
                continue;
            }
            let Some(dock) = serde_json::to_value(&slot.dock).ok() else {
                continue;
            };
            out.insert(
                id.as_str().to_string(),
                PerspectiveDockSnapshot {
                    layout_revision: self.perspective_layout_revision(*id),
                    dock,
                    side_browser: slot.side_browser.clone(),
                    side_browser_bottom: slot.side_browser_bottom.clone(),
                    center: slot.center.clone(),
                    active_center_tab: slot.active_center_tab,
                    right_inspector: slot.right_inspector.clone(),
                    right_inspector_bottom: slot.right_inspector_bottom.clone(),
                    bottom: slot.bottom.clone(),
                },
            );
        }
        if let Some(id) = self.active_perspective() {
            if self.perspective_chrome_complete() {
                if let Some(dock) = self.dock_json() {
                    out.insert(
                        id.as_str().to_string(),
                        PerspectiveDockSnapshot {
                            layout_revision: self.perspective_layout_revision(id),
                            dock,
                            side_browser: self.side_browser.clone(),
                            side_browser_bottom: self.side_browser_bottom.clone(),
                            center: self.center.clone(),
                            active_center_tab: self.active_center_tab,
                            right_inspector: self.right_inspector.clone(),
                            right_inspector_bottom: self.right_inspector_bottom.clone(),
                            bottom: self.bottom.clone(),
                        },
                    );
                }
            }
        }
        out
    }

    /// Restore every saved perspective's dock tree after a launch / Twin
    /// switch — the inverse of [`capture_perspective_docks`]. The **active**
    /// perspective's tree is reconciled into the live dock
    /// ([`set_dock_from_json`] + [`ensure_chrome_present`]); every other
    /// saved perspective is reconciled into a [`PerspectiveDockSlot`] and
    /// stashed in [`dock_cache`], so switching to it later restores its own
    /// tabs instead of a fresh preset. `id_map` remaps saved instance ids
    /// onto live tabs across ALL trees (instance ids are global — the doc
    /// set is opened once). Perspectives not registered in this app are
    /// skipped (a `luncosim`-only perspective loaded into `lunica`).
    pub(crate) fn seed_perspective_docks(
        &mut self,
        docks: &std::collections::HashMap<String, crate::workspace_state::PerspectiveDockSnapshot>,
        id_map: &HashMap<(&'static str, u64), u64>,
    ) {
        let active_str = self.active_perspective().map(|p| p.as_str().to_string());

        // Active perspective: reconcile into the LIVE dock + heal chrome.
        if let Some(active) = active_str.as_deref() {
            if let Some(snap) = docks.get(active).filter(|snap| {
                snap.layout_revision == self.perspective_layout_revision_by_str(active)
            }) {
                if self.set_dock_from_json(snap.dock.clone(), id_map) {
                    self.ensure_chrome_present();
                }
            }
        }

        // Non-active perspectives: reconcile each into a cached slot. Collect
        // first (reconcile borrows &self), insert after (borrows &mut self).
        let mut seeded: Vec<(PerspectiveId, PerspectiveDockSlot)> = Vec::new();
        for (id_str, snap) in docks {
            if Some(id_str.as_str()) == active_str.as_deref() {
                continue;
            }
            let Some(pid) = self
                .perspectives
                .iter()
                .find(|p| p.id().as_str() == id_str.as_str())
                .map(|p| p.id())
            else {
                continue; // not registered in this app — skip
            };
            if snap.layout_revision != self.perspective_layout_revision(pid) {
                continue;
            }
            let Some(slot) = self.reconcile_dock_slot(snap, id_map) else {
                continue;
            };
            seeded.push((pid, slot));
        }
        for (pid, slot) in seeded {
            self.dock_cache.insert(pid, slot);
        }
    }

    /// Return the registered preset revision for a perspective.
    fn perspective_layout_revision(&self, id: PerspectiveId) -> u32 {
        self.perspectives
            .iter()
            .find(|perspective| perspective.id() == id)
            .map_or(0, |perspective| perspective.layout_revision())
    }

    /// String-keyed companion used while restoring persisted state.
    fn perspective_layout_revision_by_str(&self, id: &str) -> u32 {
        self.perspectives
            .iter()
            .find(|perspective| perspective.id().as_str() == id)
            .map_or(0, |perspective| perspective.layout_revision())
    }

    /// Reconcile a [`PerspectiveDockSnapshot`] into a live
    /// [`PerspectiveDockSlot`] for the cache (reconciled tree + the slot
    /// intent carried alongside it). `None` when the dock won't reconcile —
    /// the caller skips caching that perspective (it builds from preset on
    /// first visit instead).
    fn reconcile_dock_slot(
        &self,
        snap: &crate::workspace_state::PerspectiveDockSnapshot,
        id_map: &HashMap<(&'static str, u64), u64>,
    ) -> Option<PerspectiveDockSlot> {
        let dock = self.reconcile_dock(snap.dock.clone(), id_map)?;
        Some(PerspectiveDockSlot {
            dock,
            side_browser: snap.side_browser.clone(),
            side_browser_bottom: snap.side_browser_bottom.clone(),
            center: snap.center.clone(),
            active_center_tab: snap.active_center_tab,
            right_inspector: snap.right_inspector.clone(),
            right_inspector_bottom: snap.right_inspector_bottom.clone(),
            bottom: snap.bottom.clone(),
        })
    }

    /// Activate a perspective by its raw string id, matching against the
    /// registered set. Returns `true` if a perspective with that id
    /// exists in this app and was activated; `false` (no-op) otherwise.
    ///
    /// The reconciliation seam for persisted state: a `PerspectiveId`
    /// holds a `&'static str` and can't be rebuilt from a runtime
    /// `String`, so restore looks the string up here and drops ids that
    /// aren't registered in the current binary (e.g. a perspective only
    /// `luncosim` ships, loaded into `lunica`).
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
            PanelSlot::Hidden => None,
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
                        if leaf.tabs.contains(&target_tab) {
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
            self.dock.set_focused_node_and_surface(pos.node_path());
            // A stale path here means the tab is present but unreachable — report
            // failure to the caller (the `FocusPanel` command) instead of silently
            // claiming success while nothing foregrounds.
            if let Err(e) = self.dock.set_active_tab(pos) {
                bevy::log::warn!("focus_singleton: could not foreground {id:?} at {pos:?}: {e:?}");
                return false;
            }
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
        let side_browser_bottom_tabs = known(&self.side_browser_bottom);
        let right_inspector_tabs = known(&self.right_inspector);
        let right_inspector_bottom_tabs = known(&self.right_inspector_bottom);
        let bottom_tabs = known(&self.bottom);
        let center_tabs: Vec<TabId> = self
            .center
            .iter()
            .copied()
            .filter(|id| self.panels.contains_key(id))
            .map(TabId::Singleton)
            .collect();

        // Preserve dynamically-opened instance (document/model/viz) tabs
        // across a *same-perspective* slot rebuild (e.g. `add_to_center`,
        // a panel re-registering, or `ResetWorkspaceLayout` after it
        // cleared the dock). The skeleton below is built purely from the
        // *singleton* slot intent, so without this every instance tab —
        // open model docs, plot instances — would silently vanish when a
        // slot intent changes within one perspective. VSCode never closes
        // open editors when you change the layout; neither do we. We walk
        // the current dock, remember each instance tab (in order) and
        // which one was focused, then re-attach them via `open_instance`
        // after the skeleton is rebuilt. Tabs whose kind is no longer
        // registered are dropped.
        //
        // Cross-perspective switches do NOT reach here with the outgoing
        // tabs still live: `activate_perspective` clears `self.dock`
        // before rebuilding the incoming perspective's preset (or restores
        // a cached dock without rebuilding at all), so each perspective's
        // tabs stay isolated in its own dock cache entry.
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
        // (the rover luncosim embeds the Modelica workbench) can have
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
            let [_old_root, right] =
                main.split_right(NodeIndex::root(), 0.765, right_inspector_tabs);
            if !right_inspector_bottom_tabs.is_empty() {
                let [_top, _bottom] = main.split_below(right, 0.5, right_inspector_bottom_tabs);
            }
        }

        if !side_browser_tabs.is_empty() {
            let main = dock.main_surface_mut();
            // For split_left, fraction is the NEW (left) share — see
            // the table in the doc above. Bumped from 0.15 → 0.22 so
            // the Twin Browser shows full library names ("Modelica
            // Standard Library") without truncation at default zoom.
            let [_old_root, left] = main.split_left(NodeIndex::root(), 0.22, side_browser_tabs);
            if !side_browser_bottom_tabs.is_empty() {
                let [_top, _bottom] = main.split_below(left, 0.5, side_browser_bottom_tabs);
            }
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
    /// registered centre singleton — the luncosim's `View`) are left
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
        self.chrome_complete(
            &self.dock,
            &self.side_browser,
            &self.side_browser_bottom,
            &self.center,
            &self.right_inspector,
            &self.right_inspector_bottom,
            &self.bottom,
        )
    }

    /// True when `dock` is consistent with the given slot intent's declared
    /// chrome. Either the intent is viewport-only (declares no *registered*
    /// centre singleton — its chrome renders outside the dock, so a
    /// chrome-less dock is correct), or every declared+registered chrome
    /// panel (side/right/bottom/centre singleton) is present in `dock`.
    ///
    /// Parameterised over the dock + intent so it can judge a CACHED
    /// perspective's dock by its OWN intent (in
    /// [`capture_perspective_docks`]), not the active perspective's. The
    /// live variant is [`perspective_chrome_complete`].
    fn chrome_complete(
        &self,
        dock: &DockState<TabId>,
        side_browser: &[PanelId],
        side_browser_bottom: &[PanelId],
        center: &[PanelId],
        right_inspector: &[PanelId],
        right_inspector_bottom: &[PanelId],
        bottom: &[PanelId],
    ) -> bool {
        let is_centre_driven = center.iter().any(|id| self.panels.contains_key(id));
        if !is_centre_driven {
            return true; // viewport-only — chrome renders outside the dock
        }
        let in_dock: std::collections::HashSet<PanelId> = dock
            .iter_all_tabs()
            .filter_map(|(_, t)| {
                if let TabId::Singleton(id) = t {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();
        side_browser
            .iter()
            .chain(side_browser_bottom.iter())
            .chain(right_inspector.iter())
            .chain(right_inspector_bottom.iter())
            .chain(bottom.iter())
            .chain(center.iter())
            .filter(|id| self.panels.contains_key(id))
            .all(|id| in_dock.contains(id))
    }
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
    fn register_perspective_help(&mut self, id: PerspectiveId, help: PerspectiveHelp) -> &mut Self;
}

impl WorkbenchAppExt for App {
    fn register_panel<P: Panel + 'static>(&mut self, panel: P) -> &mut Self {
        if !self.world().contains_resource::<WorkbenchLayout>() {
            self.init_resource::<WorkbenchLayout>();
        }
        self.world_mut()
            .resource_mut::<WorkbenchLayout>()
            .register(panel);
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
fn heal_non_finite_nulls(value: &mut serde_json::Value) {
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
        let Ok(mut contexts) = state.get_mut(world) else {
            return;
        };
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

    // Drop last pass's panel rects; the panels in the active layout refill them as
    // they paint. A panel that left the layout (closed tab, perspective switch)
    // must not keep driving its consumer (`resize_viewport_image`) from a stale
    // rect. Cleared HERE rather than in `First` because the consumers run in
    // `Update` — i.e. before this pass — and would otherwise always see an empty
    // map.
    if let Some(mut rects) = world.get_resource_mut::<viewport::PanelRects>() {
        rects.clear();
    }

    // Tell the pick gate its per-frame inputs are real this frame. (The inputs
    // themselves were cleared in `First` by `reset_scene_pick_gate`, which — unlike
    // this pass — is guaranteed to run every frame. See `ScenePickGate`.)
    if let Some(mut gate) = world.get_resource_mut::<viewport::ScenePickGate>() {
        gate.mark_rendered();
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
fn first_leaf(surface: &mut egui_dock::Tree<TabId>) -> Option<NodeIndex> {
    for (index, node) in surface.iter_mut().enumerate() {
        if node.is_leaf() {
            return Some(NodeIndex(index));
        }
    }
    None
}

/// Return the screen rect occupied by the dock leaves containing any panel in
/// `ids`. Generic tutorial anchors describe the authored workbench slot, not a
/// particular tab, so a stacked slot is represented by the union of its
/// leaves.
fn dock_group_rect(dock: &egui_dock::DockState<TabId>, ids: &[PanelId]) -> Option<egui::Rect> {
    if ids.is_empty() {
        return None;
    }

    let mut rect = None;
    for node in dock.main_surface().iter() {
        let egui_dock::Node::Leaf(leaf) = node else {
            continue;
        };
        let contains_group_tab = leaf.tabs.iter().any(|tab| match tab {
            TabId::Singleton(id) => ids.contains(id),
            TabId::Instance { kind, .. } => ids.contains(kind),
        });
        if contains_group_tab {
            rect = Some(rect.map_or(leaf.rect, |current: egui::Rect| current.union(leaf.rect)));
        }
    }
    rect
}

/// First leaf containing any tab for which `pred` returns `true`.
/// Used by [`WorkbenchLayout::open_instance`] to find the center
/// tabset after perspective splits have moved it around.
fn find_leaf_matching<F>(surface: &mut egui_dock::Tree<TabId>, pred: F) -> Option<NodeIndex>
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

/// Record a docked panel's blocked region into the scene-pick gate.
///
/// `body` is the whole leaf content area the panel was given. What it *blocks*
/// depends on its background:
/// - **Transparent** leaf → egui_dock paints nothing, so only the card the panel
///   actually drew (`ui.min_rect()`) blocks; `body − card` is see-through and the
///   full-window 3D behind it must stay clickable.
/// - **Opaque** leaf (the default: egui_dock fills it with `tab_body.bg_fill`) →
///   the WHOLE body blocks. Recording `min_rect()` here was the bug: any panel
///   whose content is shorter than its leaf turned its own painted background into
///   a "transparent gap", so clicking the empty lower half of a Modelica panel
///   picked in the hidden 3D scene behind it. (Worse for a panel that early-returns
///   without allocating: `min_rect()` is then a zero-size rect at the leaf's
///   top-left and the entire body read as gap.)
fn record_chrome(world: &mut World, ui: &egui::Ui, body: egui::Rect, transparent: bool) {
    let card = if transparent { ui.min_rect() } else { body };
    if let Some(mut gate) = world.get_resource_mut::<viewport::ScenePickGate>() {
        gate.record_chrome_panel(body, card);
    }
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
        let measured_panel_rect = viewport::PanelRects::panel_rect_from_ui(ui);
        let panel_key = match *tab {
            TabId::Singleton(id) => Some(format!("panel.{}", id.as_str())),
            TabId::Instance { kind, .. } => Some(format!("panel.{}", kind.as_str())),
        };
        if let (Some(mut a), Some(k)) = (self.world.get_resource_mut::<HelpAnchors>(), panel_key) {
            a.set(k, panel_rect);
        }

        // Publish the active tab's authoritative screen rect before rendering
        // its contents. Camera/image panels and runtime-authored surfaces both
        // consume this same geometry; neither needs to infer dock positions.
        let panel_id = match *tab {
            TabId::Singleton(id) => id,
            TabId::Instance { kind, .. } => kind,
        };
        if let Some(mut rects) = self.world.get_resource_mut::<viewport::PanelRects>() {
            rects.record(panel_id, measured_panel_rect);
        }

        match *tab {
            TabId::Singleton(id) => {
                // Take-and-return pattern so the panel can itself borrow
                // other panels' metadata via the layout (future-proof).
                if let Some(mut panel) = self.panels.remove(&id) {
                    // Capability-narrowed context (no raw &mut World).
                    // Mutations the panel emits are queued and applied
                    // after paint (WP-8 structural prevention).
                    let is_main_scene =
                        panel.scene_target() == Some(viewport::SceneTarget::MainViewport);
                    let transparent = panel_body_is_transparent(
                        self.world,
                        panel.transparent_background(),
                        is_main_scene,
                    );
                    // Full leaf body (egui_dock clips the tab-content ui to the
                    // whole leaf area, below the tab bar) — NOT `max_rect()`, which
                    // only spans the growable content and misses the transparent
                    // area below a short card.
                    let body = ui.clip_rect();
                    let scroll_policy = panel.scroll_policy();
                    let mut ctx = PanelCtx::new(self.world);
                    match scroll_policy {
                        PanelScrollPolicy::Vertical => {
                            egui::ScrollArea::vertical()
                                .id_salt(("workbench_panel_body", id.as_str()))
                                .auto_shrink([false; 2])
                                .show(ui, |ui| panel.render(ui, &mut ctx));
                        }
                        PanelScrollPolicy::SelfManaged => panel.render(ui, &mut ctx),
                    }
                    let intents = ctx.into_intents();
                    self.panels.insert(id, panel);
                    for intent in intents {
                        intent.apply(self.world);
                    }
                    if !is_main_scene {
                        record_chrome(self.world, ui, body, transparent);
                    }
                } else {
                    let error_color = self
                        .world
                        .get_resource::<lunco_theme::Theme>()
                        .map(|t| t.tokens.error)
                        .unwrap_or(egui::Color32::LIGHT_RED);
                    ui.colored_label(
                        error_color,
                        format!("Panel `{}` not registered", id.as_str()),
                    );
                }
            }
            TabId::Instance { kind, instance } => {
                if let Some(mut panel) = self.instance_panels.remove(&kind) {
                    // Instance tabs are always chrome — no `InstancePanel` hosts a
                    // live scene (the scene viewport and the USD preview are both
                    // singleton `Panel`s).
                    let transparent = panel_body_is_transparent(
                        self.world,
                        panel.transparent_background(),
                        false,
                    );
                    let body = ui.clip_rect();
                    let scroll_policy = panel.scroll_policy();
                    let mut ctx = PanelCtx::new(self.world);
                    match scroll_policy {
                        PanelScrollPolicy::Vertical => {
                            egui::ScrollArea::vertical()
                                .id_salt(("workbench_instance_panel_body", kind.as_str(), instance))
                                .auto_shrink([false; 2])
                                .show(ui, |ui| panel.render(ui, &mut ctx, instance));
                        }
                        PanelScrollPolicy::SelfManaged => panel.render(ui, &mut ctx, instance),
                    }
                    let intents = ctx.into_intents();
                    self.instance_panels.insert(kind, panel);
                    for intent in intents {
                        intent.apply(self.world);
                    }
                    record_chrome(self.world, ui, body, transparent);
                } else {
                    let error_color = self
                        .world
                        .get_resource::<lunco_theme::Theme>()
                        .map(|t| t.tokens.error)
                        .unwrap_or(egui::Color32::LIGHT_RED);
                    ui.colored_label(
                        error_color,
                        format!("InstancePanel kind `{}` not registered", kind.as_str()),
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
            TabId::Singleton(id) => {
                let Some(panel) = self.panels.get(&id) else {
                    return true;
                };
                let is_main_scene =
                    panel.scene_target() == Some(viewport::SceneTarget::MainViewport);
                !panel_body_is_transparent(
                    self.world,
                    panel.transparent_background(),
                    is_main_scene,
                )
            }
            TabId::Instance { kind, .. } => {
                let Some(panel) = self.instance_panels.get(&kind) else {
                    return true;
                };
                !panel_body_is_transparent(self.world, panel.transparent_background(), false)
            }
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
                self.world.resource_mut::<PendingTabCloses>().push(*tab);
                OnCloseResponse::Ignore
            }
        }
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab, _path: egui_dock::NodePath) {
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
                let intents = ctx.into_intents();
                self.instance_panels.insert(kind, panel);
                for intent in intents {
                    intent.apply(self.world);
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

/// One menu row in the top-bar drop-downs: label + optional shortcut,
/// greys out when `enabled` is false and then explains itself via
/// `disabled_hint` ("No document open", "Nothing to undo", …).
///
/// Extracted because the `add_enabled(…, Button::new("Label\tShortcut"))`
/// pattern was copy-pasted across Save / Copy Share Link / Close /
/// Undo / Redo and every copy forgot the disabled hint — with the hint a
/// required parameter it can't be forgotten on the next menu item.
/// Returns the [`egui::Response`] so callers can still chain
/// `.on_hover_text(…)` and `.clicked()`.
fn menu_item(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    shortcut: &str,
    disabled_hint: &str,
) -> egui::Response {
    let text = if shortcut.is_empty() {
        label.to_owned()
    } else {
        format!("{label}\t{shortcut}")
    };
    ui.add_enabled(enabled, egui::Button::new(text))
        .on_disabled_hover_text(disabled_hint)
}

/// Run one contributed menu callback behind the capability-limited
/// [`MenuCtx`], then apply its typed intent while the workbench layout is
/// still temporarily removed from the world.
fn run_menu_callback(
    ui: &mut egui::Ui,
    world: &mut World,
    callback: &(dyn Fn(&mut egui::Ui, &mut MenuCtx) + Send + Sync),
) {
    let mut menu = MenuCtx::new(world);
    callback(ui, &mut menu);
    for intent in menu.into_intents() {
        intent.apply(world);
    }
}

fn render_layout(
    ctx: &egui::Context,
    layout: &mut WorkbenchLayout,
    world: &mut World,
    theme: &lunco_theme::Theme,
) {
    // ── Clean capture ───────────────────────────────────────────────
    // A frame the offline recorder is capturing is a FILM frame: the whole
    // workbench chrome — menu/title bar, status strip (and its FPS readout),
    // activity bar, dock — must not be burnt into the footage. Skipping this
    // pass while a recording is active leaves the 3D view full-bleed; the
    // deliberate overlays (input HUD, vessel telemetry, notifications) are
    // drawn by their own systems and survive. Scoped to `active`, so the
    // editor comes back the instant the recorder stops — and between shots,
    // where no frame is captured, any flicker is invisible in the footage.
    // The offline recorder is driven over the command surface, so it — and with it
    // the question "is a frame being captured?" — exists only under `api`.
    #[cfg(feature = "api")]
    if world
        .get_resource::<screenshot::OfflineRecordingState>()
        .is_some_and(|r| r.active)
        && !world
            .get_resource::<OfflineRecordingPresentation>()
            .is_some_and(|p| p.retain_workbench_chrome)
    {
        return;
    }

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
            let dx = if p.x < screen.left() + RESIZE_BORDER {
                -1
            } else if p.x > screen.right() - RESIZE_BORDER {
                1
            } else {
                0
            };
            let dy = if p.y < screen.top() + RESIZE_BORDER {
                -1
            } else if p.y > screen.bottom() - RESIZE_BORDER {
                1
            } else {
                0
            };
            use bevy::math::CompassOctant;
            let dir = match (dx, dy) {
                (-1, -1) => Some(CompassOctant::NorthWest),
                (0, -1) => Some(CompassOctant::North),
                (1, -1) => Some(CompassOctant::NorthEast),
                (1, 0) => Some(CompassOctant::East),
                (1, 1) => Some(CompassOctant::SouthEast),
                (0, 1) => Some(CompassOctant::South),
                (-1, 1) => Some(CompassOctant::SouthWest),
                (-1, 0) => Some(CompassOctant::West),
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
    // egui 0.35 unified the panel API: panels, the central area, and the dock
    // now render *inside* a `Ui` rather than directly onto the `Context`. Build
    // one root Ui spanning the whole viewport; every panel below shows into it,
    // consuming edges in call order, and the dock/centre takes the remainder.
    let mut viewport_ui = egui::Ui::new(
        ctx.clone(),
        "lunco_workbench_viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );

    egui::Panel::top("lunco_workbench_menu_bar")
        // Match the dock tab-bar height so the merged title-bar
        // doesn't read as a thin sliver above thicker rows below.
        // 30px is roughly egui_dock's default tab strip height with
        // our font scale.
        .exact_size(30.0)
        .show(&mut viewport_ui, |ui| {
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
        // MenuBar creates its own compact horizontal child. Put that child
        // inside the full-height title-bar layout so the row is centred in
        // the 30px bar rather than starting at its top edge.
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            egui::MenuBar::new()
                .config(
                    egui::menu::MenuConfig::new()
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
                )
                .ui(ui, |ui| {
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
                                world.trigger(lunco_doc_bevy::NewDocument { kind });
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
                                    world.trigger(lunco_workspace::open::OpenTwin {
                                        path: path.display().to_string(),
                                    });
                                    ui.close();
                                }
                            }
                        });
                    })
                    .response
                    .on_disabled_hover_text("No Twin folders opened yet");
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
                                    world.trigger(lunco_doc_bevy::OpenFile {
                                        path: path.display().to_string(),
                                    });
                                    ui.close();
                                }
                            }
                        });
                    })
                    .response
                    .on_disabled_hover_text("No files opened yet");
                }
                ui.separator();

                // -- Save ---------------------------------------------
                // Save / Save As route through `EditorIntent` so the
                // menu, Ctrl+S, and HTTP API funnel through the same
                // domain resolver.
                if menu_item(ui, has_active, "Save", "Ctrl+S", "No document open")
                    .clicked()
                {
                    world.trigger(lunco_doc_bevy::EditorIntent::Save);
                    ui.close();
                }
                if menu_item(
                    ui,
                    has_active,
                    "Save As…",
                    "Ctrl+Shift+S",
                    "No document open",
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
                if menu_item(ui, has_active, "Copy Share Link", "", "No document open")
                    .on_hover_text(
                        "Copy a URL that encodes this model's source — \
                         anyone who opens it gets the model (nothing is uploaded)",
                    )
                    .clicked()
                {
                    world.trigger(file_ops::CopyShareLink {});
                    ui.close();
                }
                ui.separator();

                let callbacks = std::mem::take(&mut layout.file_menu);
                if !callbacks.is_empty() {
                    for cb in &callbacks {
                        run_menu_callback(ui, world, cb.as_ref());
                    }
                    ui.separator();
                }
                layout.file_menu = callbacks;

                // -- Close --------------------------------------------
                if menu_item(ui, has_active, "Close", "Ctrl+W", "No document open")
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
                // Ask the domain probes whether the active document's
                // undo/redo stacks are actually non-empty; first probe to
                // recognise the document wins (same contract as the
                // `EditorIntent` resolvers). No probe answering falls
                // back to plain "a document is active" so a domain that
                // registered no probe keeps working entries.
                let (can_undo, can_redo) = layout
                    .undo_probes
                    .iter()
                    .find_map(|probe| probe(&UndoProbeCtx::new(world)))
                    .unwrap_or((has_active, has_active));
                let undo_hint = if has_active {
                    "Nothing to undo"
                } else {
                    "No document open"
                };
                let redo_hint = if has_active {
                    "Nothing to redo"
                } else {
                    "No document open"
                };
                if menu_item(ui, can_undo, "Undo", "Ctrl+Z", undo_hint).clicked() {
                    world.trigger(lunco_doc_bevy::EditorIntent::Undo);
                    ui.close();
                }
                if menu_item(ui, can_redo, "Redo", "Ctrl+Shift+Z", redo_hint)
                    .clicked()
                {
                    world.trigger(lunco_doc_bevy::EditorIntent::Redo);
                    ui.close();
                }

                // Domain plugins (e.g. the Modelica code editor)
                // contribute Cut/Copy/Paste/Select-All here via
                // `register_edit_menu`. Same extraction pattern as the
                // Settings menu so callbacks receive the capability-limited
                // `MenuCtx`.
                let callbacks = std::mem::take(&mut layout.edit_menu);
                if !callbacks.is_empty() {
                    ui.separator();
                    for cb in &callbacks {
                        run_menu_callback(ui, world, cb.as_ref());
                    }
                }
                layout.edit_menu = callbacks;
            });
            anchor_rects.push(("menu.edit", r_edit.response.rect));
            let r_view = ui.menu_button("View", |ui| {
                if ui.button("Reset Layout").clicked() {
                    // Recovery hatch: re-apply the active perspective's preset,
                    // restoring panels (notably the 3D Viewport) a stale
                    // persisted layout dropped.
                    world
                        .resource_mut::<PendingLayoutRequests>()
                        .0
                        .push(LayoutRequest::Reset);
                    ui.close();
                }
                ui.separator();
                if ui.button("Toggle Activity Bar").clicked() {
                    world
                        .resource_mut::<PendingLayoutRequests>()
                        .0
                        .push(LayoutRequest::SetActivityBar(!layout.activity_bar));
                    ui.close();
                }
                ui.separator();
                // Panels, grouped by the workflow each one declares
                // (`Panel::menu_group`) instead of one flat alphabetical dump.
                // Each row is a checkbox showing whether the panel is currently
                // in the dock; clicking a closed one re-docks it in its default
                // slot. `Hidden` panels never appear (fixtures like the
                // viewport, layout-only entries, instance-tab facets).
                struct ViewPanelEntry {
                    group: PanelMenuGroup,
                    title: String,
                    slot: PanelSlot,
                    open: bool,
                    singleton: Option<PanelId>,
                    instance: Option<(PanelId, u64)>,
                }
                let panels_meta: Vec<ViewPanelEntry> = {
                    let docked: std::collections::HashSet<PanelId> = layout
                        .dock
                        .iter_all_tabs()
                        .filter_map(|(_, id)| match id {
                            TabId::Singleton(pid) => Some(*pid),
                            TabId::Instance { .. } => None,
                        })
                        .collect();
                    let mut sorted: Vec<ViewPanelEntry> =
                        layout
                            .panels
                            .values()
                            .filter(|p| p.menu_group() != PanelMenuGroup::Hidden)
                            .map(|p| {
                                let id = p.id();
                                ViewPanelEntry {
                                    group: p.menu_group(),
                                    title: p.title(),
                                    slot: p.default_slot(),
                                    open: docked.contains(&id),
                                    singleton: Some(id),
                                    instance: None,
                                }
                            })
                            .collect();
                    // Instance panels normally have no global menu entry.
                    // Include only the canonical instance explicitly exposed
                    // by the panel, so document tabs remain discoverable by
                    // their owning workflow rather than becoming noise here.
                    for (kind, panel) in &layout.instance_panels {
                        let Some(entry) = panel.menu_entry() else {
                            continue;
                        };
                        let tab = TabId::Instance {
                            kind: *kind,
                            instance: entry.instance,
                        };
                        sorted.push(ViewPanelEntry {
                            group: entry.group,
                            title: entry.title.to_owned(),
                            slot: panel.default_slot(),
                            open: layout.dock.find_tab(&tab).is_some(),
                            singleton: None,
                            instance: Some((*kind, entry.instance)),
                        });
                    }
                    // Group first, then title — but sort titles on their FIRST
                    // ALPHANUMERIC char: sorting on the raw string put every
                    // emoji-prefixed title ("🛠 Tools") in a block after every
                    // plain one, which is why related panels never sat together.
                    let sort_key = |t: &str| -> String {
                        t.chars()
                            .skip_while(|c| !c.is_alphanumeric())
                            .collect::<String>()
                            .to_lowercase()
                    };
                    sorted.sort_by(|a, b| {
                        a.group
                            .cmp(&b.group)
                            .then_with(|| sort_key(&a.title).cmp(&sort_key(&b.title)))
                    });
                    sorted
                };
                let mut last_group: Option<PanelMenuGroup> = None;
                for entry in panels_meta {
                    let ViewPanelEntry {
                        group,
                        title,
                        slot,
                        open: is_open,
                        singleton,
                        instance,
                    } = entry;
                    if last_group != Some(group) {
                        if last_group.is_some() {
                            ui.separator();
                        }
                        let heading = match group {
                            PanelMenuGroup::Scene => "Scene",
                            PanelMenuGroup::Design => "Design",
                            PanelMenuGroup::Tools => "Tools",
                            PanelMenuGroup::Other => "Other",
                            PanelMenuGroup::Hidden => unreachable!("filtered above"),
                        };
                        ui.label(egui::RichText::new(heading).weak().small());
                        last_group = Some(group);
                    }
                    let mut checked = is_open;
                    if ui.checkbox(&mut checked, title).clicked() {
                        if checked && !is_open {
                            if let Some((kind, instance)) = instance {
                                world
                                    .resource_mut::<PendingTabRequests>()
                                    .0
                                    .push(TabRequest::Open(OpenTab { kind, instance }));
                                ui.close();
                                continue;
                            }
                            let Some(id) = singleton else {
                                ui.close();
                                continue;
                            };
                            // Track in the slot list so persistence /
                            // perspective queries see it. Insert into
                            // the *live* dock without a full rebuild
                            // — rebuild_dock would wipe instance tabs
                            // (model views) the user has open.
                            //
                            // A hidden default slot has no preset dock region;
                            // opening it explicitly gives it a stable side-browser
                            // home until Reset Layout.
                            let slot = match slot {
                                PanelSlot::Hidden => PanelSlot::SideBrowser,
                                other => other,
                            };
                            world
                                .resource_mut::<PendingLayoutRequests>()
                                .0
                                .push(LayoutRequest::AddSingleton { id, slot });
                        } else if !checked && is_open {
                            if let Some((kind, instance)) = instance {
                                world
                                    .resource_mut::<PendingTabRequests>()
                                    .0
                                    .push(TabRequest::Close(CloseTab { kind, instance }));
                                ui.close();
                                continue;
                            }
                            let Some(id) = singleton else {
                                ui.close();
                                continue;
                            };
                            // Untrack from slot lists.
                            world
                                .resource_mut::<PendingLayoutRequests>()
                                .0
                                .push(LayoutRequest::RemoveSingleton(id));
                        }
                        ui.close();
                    }
                }
            });
            anchor_rects.push(("menu.view", r_view.response.rect));

            // Custom top-level menus
            let custom_menus = std::mem::take(&mut layout.custom_menus);

            for (name, cb) in &custom_menus {
                let r_custom = ui.menu_button(*name, |ui| {
                    run_menu_callback(ui, world, cb.as_ref());
                });

                anchor_rects.push((*name, r_custom.response.rect));
            }
            layout.custom_menus = custom_menus;

            let r_settings = ui.menu_button("Settings", |ui| {
                ui.label(egui::RichText::new("Theme").weak().small());
                let mut theme = world.resource_mut::<lunco_theme::Theme>();
                let mode = theme.mode;

                let label = match mode {
                    lunco_theme::ThemeMode::Dark => "Dark",
                    lunco_theme::ThemeMode::Light => "Light",
                };

                if ui.button(label).clicked() {
                    theme.toggle_mode();
                }
                ui.separator();

                // Take the callbacks out while the layout is still extracted;
                // each callback receives `MenuCtx` and is restored below.
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
                        run_menu_callback(ui, world, cb.as_ref());
                    }
                }
                layout.settings_menu = callbacks;

                // Feature areas with many rows stay discoverable without
                // forcing the root Settings menu to fill the viewport.
                let submenus = std::mem::take(&mut layout.settings_submenus);
                for (label, callbacks) in &submenus {
                    ui.menu_button(label, |ui| {
                        let max_height = ui.spacing().interact_size.y * 24.0;
                        egui::ScrollArea::vertical()
                            .max_height(max_height)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for (i, callback) in callbacks.iter().enumerate() {
                                    if i > 0 {
                                        ui.separator();
                                    }
                                    run_menu_callback(ui, world, callback.as_ref());
                                }
                            });
                    });
                }
                layout.settings_submenus = submenus;
            });
            anchor_rects.push(("menu.settings", r_settings.response.rect));
            let r_help = ui.menu_button("Help", |ui| {
                if let Some(identity) = world.get_resource::<BuildIdentity>() {
                    ui.label(format!(
                        "{} · {}",
                        running_app_name(),
                        identity.version_label()
                    ));
                    if let Some(source_url) = identity.source_url() {
                        ui.hyperlink_to("View source commit on GitHub", source_url);
                    } else {
                        ui.label(
                            egui::RichText::new("Source commit unavailable")
                                .weak()
                                .italics(),
                        );
                    }
                }
                let callbacks = std::mem::take(&mut layout.help_menu);
                if !callbacks.is_empty() {
                    ui.separator();
                    for cb in &callbacks {
                        run_menu_callback(ui, world, cb.as_ref());
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
                        ui.separator();
                        // Copy invite link. The address a guest should dial isn't
                        // knowable from the host side (which interface?), so it's
                        // editable — prefilled with the best-guess LAN IP:port the
                        // adapter detected (`invite_hint`). The link carries the
                        // self-signed cert digest in its `#fragment` so a browser
                        // guest can pin it. Built inline (workbench keeps no
                        // networking dep, D7); the canonical format lives in
                        // `lunco_networking::connect_link`.
                        let addr_id =
                            ui.make_persistent_id("lunco_network_invite_address");
                        let mut invite_addr = ui.data_mut(|d| {
                            d.get_temp::<String>(addr_id)
                                .unwrap_or_else(|| status.invite_hint.clone())
                        });
                        ui.horizontal(|ui| {
                            ui.label("Guest dials:");
                            ui.text_edit_singleline(&mut invite_addr);
                        });
                        let digest = status.invite_digest.trim();
                        let frag = if digest.is_empty() {
                            String::new()
                        } else {
                            format!("#{digest}")
                        };
                        let a = invite_addr.trim();
                        let enabled = !a.is_empty();
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(enabled, egui::Button::new("Copy web link"))
                                .on_hover_text("https://lunica.lunco.space/?connect=… — opens in a browser")
                                .on_disabled_hover_text(
                                    "Enter the address guests should dial first",
                                )
                                .clicked()
                            {
                                let link = format!(
                                    "https://lunica.lunco.space/?connect={a}{frag}"
                                );
                                ui.ctx().copy_text(link);
                            }
                            let app_q = if digest.is_empty() {
                                String::new()
                            } else {
                                format!("&digest={digest}")
                            };
                            if ui
                                .add_enabled(enabled, egui::Button::new("Copy app link"))
                                .on_hover_text("luncosim://connect?… — opens the desktop app")
                                .on_disabled_hover_text(
                                    "Enter the address guests should dial first",
                                )
                                .clicked()
                            {
                                let link =
                                    format!("luncosim://connect?address={a}{app_q}");
                                ui.ctx().copy_text(link);
                            }
                        });
                        ui.data_mut(|d| d.insert_temp(addr_id, invite_addr));
                    }
                    NetworkRole::Client => {
                        let state = if status.connected {
                            "Connected"
                        } else {
                            "Connecting…"
                        };
                        ui.label(format!("{state} -> {}", status.endpoint));
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
                        // Optional self-signed cert digest to pin. A browser
                        // joining a self-signed LAN/dev host by IP needs this
                        // (it can't skip TLS validation); paste the digest the
                        // host prints (`🔐 WebTransport cert digest: …`). Leave
                        // blank for a CA-cert host or a native bare-IP dial.
                        let digest_id =
                            ui.make_persistent_id("lunco_network_menu_digest");
                        let mut digest = ui
                            .data_mut(|d| d.get_temp::<String>(digest_id))
                            .unwrap_or_default();
                        ui.horizontal(|ui| {
                            ui.label("Cert digest:");
                            ui.add(
                                egui::TextEdit::singleline(&mut digest)
                                    .hint_text("optional — self-signed host"),
                            );
                        });
                        let enabled = !address.trim().is_empty();
                        if ui
                            .add_enabled(enabled, egui::Button::new("Connect"))
                            .on_disabled_hover_text("Enter a server address first")
                            .clicked()
                        {
                            world.trigger(NetConnectRequest {
                                address: address.clone(),
                                digest: digest.clone(),
                            });
                            ui.close();
                        }
                        ui.data_mut(|d| d.insert_temp(id, address));
                        ui.data_mut(|d| d.insert_temp(digest_id, digest));
                    }
                }
            });
            anchor_rects.push(("menu.network", r_network.response.rect));

            // Time — every clock control that is not pause/resume.
            //
            // The sim RATE used to sit on the toolbar beside the pause button and
            // the sky clock floated permanently over the viewport. Both are time
            // controls, neither is needed on every frame of every session, and
            // between them they made "what time is it" a two-place question. One
            // menu answers it; the toolbar keeps only the verb you actually reach
            // for mid-drive.
            //
            // Domain plugins contribute rows via
            // `WorkbenchLayout::register_time_menu` (the celestial sky clock, the
            // >8x warp band), so nothing about the sky is hardcoded here.
            let r_time = ui.menu_button("Time", |ui| {
                ui.label(egui::RichText::new("Simulation rate").weak().small());
                let (paused, rate) = world
                    .get_resource::<lunco_time::TimeTransport>()
                    .map(|t| {
                        (
                            matches!(t.mode, lunco_time::TransportMode::Paused),
                            t.rate,
                        )
                    })
                    .unwrap_or((false, 1.0));

                // ONLY the physics-real band is offered here. At or below
                // `MAX_REALTIME_RATE` the rate multiplies the NUMBER of fixed steps
                // per frame, so bodies genuinely integrate faster — a rover really
                // does drive 4x faster, with identical solver fidelity. Past that
                // ceiling `advance_clock` selects `TimeRegime::KinematicWarp` and
                // returns relative_speed 0: the tick FREEZES and only the epoch
                // moves. That is a sky-viewing tool, not a fast-forward, so it stays
                // in the celestial/mission-control panels.
                ui.horizontal(|ui| {
                    for &m in lunco_time::REALTIME_RATE_OPTIONS {
                        let on = !paused && (rate - m).abs() < f64::EPSILON;
                        if ui
                            .selectable_label(on, format!("{m:.0}x"))
                            .on_hover_text("Run the simulation (physics included) at this rate")
                            .clicked()
                        {
                            world.trigger(lunco_time::SetTimeTransport {
                                playing: Some(true),
                                rate: Some(m),
                            });
                        }
                    }
                });
                if rate > lunco_time::MAX_REALTIME_RATE {
                    // `Res<Theme>`, NOT `lunco_theme::active(ctx)`: the latter reads
                    // a per-frame copy that only the Modelica canvas ever publishes,
                    // so everywhere else it silently returns `Theme::dark()`.
                    let warn = world
                        .get_resource::<lunco_theme::Theme>()
                        .map(|t| t.tokens.warning)
                        .unwrap_or(egui::Color32::YELLOW);
                    ui.label(
                        egui::RichText::new(format!("{rate:.0}x sky — tick frozen")).color(warn),
                    )
                    .on_hover_text(
                        "Kinematic warp: the sim tick is frozen. Bodies do not move; \
                         only the epoch advances.",
                    );
                }

                let callbacks = std::mem::take(&mut layout.time_menu);
                if !callbacks.is_empty() {
                    ui.separator();
                    for cb in &callbacks {
                        run_menu_callback(ui, world, cb.as_ref());
                    }
                }
                layout.time_menu = callbacks;
            });
            anchor_rects.push(("menu.time", r_time.response.rect));

            // Pause/Resume simulation via the single transport authority
            // (`TimeTransport.mode`, doc 19). The spine maps `Paused` onto
            // `relative_speed = 0`, freezing tick + avian (`Time<Physics>` derives
            // from Virtual) + epoch together — and now stays in sync with the
            // avatar pause hotkey and the mission-control / celestial panels, which
            // write the same resource.
            {
                let paused = world
                    .get_resource::<lunco_time::TimeTransport>()
                    .is_some_and(|t| matches!(t.mode, lunco_time::TransportMode::Paused));
                let (icon, hover) = if paused {
                    (UiIcon::Play, "Resume simulation")
                } else {
                    (UiIcon::Pause, "Pause simulation")
                };
                let btn_resp = icon_button(ui, icon, hover);
                anchor_rects.push(("toolbar.run", btn_resp.rect));
                if btn_resp.clicked() {
                    world.trigger(lunco_time::SetTimeTransport {
                        playing: Some(paused),
                        ..default()
                    });
                }

                // PAUSE/RESUME AND NOTHING ELSE. The rate selector used to sit here
                // too; it moved to the Time menu above. The toolbar is the place for
                // the one verb you reach for mid-drive, not for the whole clock.
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
                    if icon_button(ui, UiIcon::Close, "Close").clicked() {
                        world.trigger(window_command::CloseWindow {});
                    }
                    let max_icon = if is_max {
                        UiIcon::Restore
                    } else {
                        UiIcon::Maximize
                    };
                    let max_hover = if is_max { "Restore" } else { "Maximize" };
                    if icon_button(ui, max_icon, max_hover).clicked() {
                        world.trigger(window_command::MaximizeWindow { maximized: None });
                    }
                    if icon_button(ui, UiIcon::Minimize, "Minimize").clicked() {
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
                // perspective (Lunica ships only "⚙ Lunica" today) the
                // lone tab is just noise — hide it, but keep the render
                // logic intact for when "Build" / "Simulate" / etc. land.
                if tabs.len() > 1 {
                    for (id, title, is_active) in tabs {
                        let button = egui::Button::new(title.as_str()).selected(is_active);
                        if ui.add(button).clicked() && !is_active {
                            world
                                .resource_mut::<PendingLayoutRequests>()
                                .0
                                .push(LayoutRequest::ActivatePerspective(id.0.to_owned()));
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
    });
    });

    // ── Status bar ──────────────────────────────────────────────────
    // Drives off the cross-cutting `StatusBus` resource. Latest event
    // shows in the strip; click opens a popup with recent history.
    egui::Panel::bottom("lunco_workbench_status_bar").show(&mut viewport_ui, |ui| {
        ui.style_mut().visuals = theme.to_visuals();
        render_status_bar_inner(ui, world, theme);
    });

    // ── Activity bar ────────────────────────────────────────────────
    if layout.activity_bar {
        egui::Panel::left("lunco_workbench_activity_bar")
            .resizable(false)
            .exact_size(40.0)
            .show(&mut viewport_ui, |ui| {
                ui.style_mut().visuals = theme.to_visuals();
                ui.vertical_centered(|ui| {
                    ui.add_space(4.0);
                    for label in ["Files", "Parts", "Assets", "Find", "Settings"] {
                        ui.label(label);
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
    //   2. Otherwise (viewport-only perspective like the luncosim's `View`),
    //      render the side panels with plain SidePanel / TopBottomPanel and
    //      leave the central area transparent for the 3D viewport.
    //
    // The gate is the centre *intent* (`layout.center`), not merely "does
    // the dock hold any tab". A hybrid app (the rover luncosim embeds the
    // Modelica workbench) can have document/model instance tabs parked in
    // the dock while a viewport-only perspective is active — e.g. restored
    // on boot before the user switches to a doc-capable perspective.
    // Keying off the dock alone would flip the whole workbench into
    // dock-mode and paint tab chrome over the 3D scene; keying off the
    // perspective's centre intent keeps `View` pure-3D and leaves the
    // parked docs hidden until the user switches to a centre-driven
    // perspective (which re-attaches them via `rebuild_dock`).
    let has_dock_tabs = !layout.center.is_empty() && layout.dock.iter_all_tabs().next().is_some();

    if has_dock_tabs {
        let transparent_tab_content = world
            .resource::<WorkbenchAppearanceSettings>()
            .transparent_tab_content;
        let WorkbenchLayout {
            panels,
            instance_panels,
            dock,
            side_browser,
            right_inspector,
            bottom,
            ..
        } = &mut *layout;
        let mut viewer = PanelTabViewer {
            panels,
            instance_panels,
            world,
        };
        let mut style = Style::from_egui(viewport_ui.style().as_ref());
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
        // The tab strip is distinct chrome, so it uses the theme's crust while
        // ordinary tab bodies use mantle. The dedicated ViewportPanel remains
        // transparent because it hosts the full-window scene camera.
        style.tab_bar.bg_fill = theme.colors.crust;
        // Drop the hairline under the active tab name too — same
        // visual-noise reason as the tab body stroke.
        style.tab_bar.hline_color = egui::Color32::TRANSPARENT;
        // egui_dock's `Style::from_egui` defaults pull tab colours
        // from `visuals.widgets`, but the result still doesn't track
        // our Light/Dark palette cleanly: inactive tabs come out
        // washed out and active tabs lose contrast against the bar.
        // Bind every interaction state to the theme so tabs read
        // consistently in both modes.
        // The tab labels themselves remain transparent: the selected state is
        // communicated by text colour, not by a black rectangle.
        let palette = &theme.colors;
        style.tab.tab_body.bg_fill = if transparent_tab_content {
            egui::Color32::TRANSPARENT
        } else {
            palette.mantle
        };
        style.tab.active.bg_fill = egui::Color32::TRANSPARENT;
        style.tab.active.text_color = palette.text;
        style.tab.active.outline_color = egui::Color32::TRANSPARENT;
        style.tab.inactive.bg_fill = egui::Color32::TRANSPARENT;
        style.tab.inactive.text_color = palette.subtext1;
        style.tab.inactive.outline_color = egui::Color32::TRANSPARENT;
        style.tab.hovered.bg_fill = egui::Color32::TRANSPARENT;
        style.tab.hovered.text_color = palette.text;
        style.tab.hovered.outline_color = egui::Color32::TRANSPARENT;
        style.tab.focused.bg_fill = egui::Color32::TRANSPARENT;
        style.tab.focused.text_color = palette.mauve;
        style.tab.focused.outline_color = egui::Color32::TRANSPARENT;
        style.tab.inactive_with_kb_focus.bg_fill = egui::Color32::TRANSPARENT;
        style.tab.inactive_with_kb_focus.text_color = palette.text;
        style.tab.active_with_kb_focus.bg_fill = egui::Color32::TRANSPARENT;
        style.tab.active_with_kb_focus.text_color = palette.mauve;
        style.tab.focused_with_kb_focus.bg_fill = egui::Color32::TRANSPARENT;
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
            // The dock's true extent for the scene-pick gate: the rect we are
            // ABOUT to hand it — i.e. what's left of the root Ui after the menu /
            // status / activity bars consumed their edges. Anything outside it is
            // bare full-window 3D that must read as scene, not chrome.
            //
            // Must be measured BEFORE `show_inside`. The old code took
            // `viewport_ui.min_rect()` AFTER it, but `viewport_ui` is the ROOT
            // background Ui spanning the whole window — the menu bar and status bar
            // are drawn into it too — so `min_rect()` came back as ≈ the entire
            // window. `in_dock` was then true everywhere and the chrome blanket
            // swallowed every click on bare 3D outside a dock leaf.
            let dock_rect = viewport_ui.available_rect_before_wrap();
            DockArea::new(dock)
                .style(style)
                .show_inside(&mut viewport_ui, &mut viewer);
            // The scene viewport LEAF's rect, straight from egui_dock's post-layout
            // tree. `LeafNode::rect` persists even when the leaf is COLLAPSED or the
            // viewport sits behind another tab — cases where `ViewportPanel::render`
            // doesn't run and so can't record the scene pick leaf itself. Feeding
            // this to the gate keeps the full-window 3D clickable through
            // collapse / fold / background (else a collapsed centre goes dead to
            // clicks). `panels`/`dock` are usable again here — `viewer`'s reborrows
            // ended at `show_inside`.
            let scene_vp_tab = panels
                .iter()
                .find(|(_, p)| p.scene_target() == Some(SceneTarget::MainViewport))
                .map(|(id, _)| TabId::Singleton(*id));
            let scene_vp_rect = scene_vp_tab.and_then(|vp_tab| {
                dock.main_surface().iter().find_map(|node| match node {
                    egui_dock::Node::Leaf(leaf) if leaf.tabs.contains(&vp_tab) => Some(leaf.rect),
                    _ => None,
                })
            });

            // Publish generic slot anchors from the laid-out dock tree. The
            // alternate explicit-panel renderer below publishes these from
            // egui::Panel responses; docked perspectives need the same
            // contract or a tutorial would fail merely because its authored
            // perspective uses egui_dock.
            if let Some(mut a) = world.get_resource_mut::<HelpAnchors>() {
                a.set("panel.center", screen);
                if let Some(rect) = dock_group_rect(dock, side_browser) {
                    a.set("panel.side_browser", rect);
                }
                if let Some(rect) = dock_group_rect(dock, right_inspector) {
                    a.set("panel.right_inspector", rect);
                }
                if let Some(rect) = dock_group_rect(dock, bottom) {
                    a.set("panel.bottom", rect);
                }
            }
            if let Some(mut g) = world.get_resource_mut::<viewport::ScenePickGate>() {
                g.set_dock_rect(dock_rect);
                g.set_scene_viewport_rect(scene_vp_rect);
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
        // in its own memory for the session (not persisted — luncosim-style
        // perspectives keep their sizes in the dock tree via 5a instead).
        let side_default = (screen.width() * 0.10).max(140.0);
        let right_default = (screen.width() * 0.10).max(140.0);
        let bottom_default = (screen.height() * 0.20).max(120.0);

        let side_panel_fill = if world
            .resource::<WorkbenchAppearanceSettings>()
            .transparent_tab_content
        {
            egui::Color32::TRANSPARENT
        } else {
            theme.colors.mantle
        };

        if let Some(id) = layout.side_browser.first().copied() {
            let r = egui::Panel::left("lunco_workbench_side_panel_left")
                .resizable(true)
                .default_size(side_default)
                .min_size(120.0)
                .max_size(screen.width() * 0.3)
                .frame(
                    egui::Frame::side_top_panel(viewport_ui.style().as_ref()).fill(side_panel_fill),
                )
                .show(&mut viewport_ui, |ui| {
                    ui.style_mut().visuals = theme.to_visuals();
                    render_panel_solo(ui, &id, layout, world);
                });
            if let Some(mut a) = world.get_resource_mut::<HelpAnchors>() {
                a.set("panel.side_browser", r.response.rect);
            }
        }
        if let Some(id) = layout.right_inspector.first().copied() {
            let r = egui::Panel::right("lunco_workbench_side_panel_right")
                .resizable(true)
                .default_size(right_default)
                .min_size(140.0)
                .max_size(screen.width() * 0.3)
                .frame(
                    egui::Frame::side_top_panel(viewport_ui.style().as_ref()).fill(side_panel_fill),
                )
                .show(&mut viewport_ui, |ui| {
                    ui.style_mut().visuals = theme.to_visuals();
                    render_panel_solo(ui, &id, layout, world);
                });
            if let Some(mut a) = world.get_resource_mut::<HelpAnchors>() {
                a.set("panel.right_inspector", r.response.rect);
            }
        }
        if let Some(id) = layout.bottom.first().copied() {
            let r = egui::Panel::bottom("lunco_workbench_bottom_panel")
                .resizable(true)
                .default_size(bottom_default)
                .min_size(60.0)
                .frame(
                    egui::Frame::side_top_panel(viewport_ui.style().as_ref()).fill(side_panel_fill),
                )
                .show(&mut viewport_ui, |ui| {
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
fn render_status_bar_inner(ui: &mut egui::Ui, world: &mut World, theme: &lunco_theme::Theme) {
    use status_bus::{StatusBarAction, StatusBus, StatusLevel};

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
    // Raw frame times straight out of Bevy's own `Diagnostic` ring buffer — `PerfStats`
    // no longer shadows it with a second `VecDeque` holding the same values.
    let frame_history: Vec<f32> = world
        .get_resource::<bevy::diagnostic::DiagnosticsStore>()
        .map(perf_hud::frame_history)
        .unwrap_or_default();
    let perf_enabled = world.resource::<perf_hud::PerfHudSettings>().enabled;
    // The networking chip only paints when not standalone; reserve room
    // for it on the right so the clickable status region doesn't overlap.
    let net_active = world
        .get_resource::<lunco_core::NetStatus>()
        .map(|s| !matches!(s.role, lunco_core::NetworkRole::Standalone))
        .unwrap_or(false);
    let scene_name = world
        .get_resource::<CurrentSceneName>()
        .map(|s| s.0.clone())
        .unwrap_or_default();
    let tutorial_title = world
        .get_resource::<crate::tutorial_overlay::TutorialHud>()
        .map(|hud| hud.title.clone())
        .unwrap_or_default();
    let scene_path = world
        .get_resource::<CurrentScenePath>()
        .map(|s| s.0.clone())
        .unwrap_or_default();
    let scene_popup_id = ui.make_persistent_id("lunco_workbench_loaded_scene_popup");

    ui.horizontal(|ui| {
        // Calculate the reserved width for all elements to the right of the status scope
        let right_reserve = 16.0
            + if perf_enabled { 300.0 } else { 0.0 }
            + if net_active { 220.0 } else { 0.0 }
            + if !tutorial_title.is_empty() {
                190.0
            } else {
                0.0
            }
            + if !scene_name.is_empty() { 150.0 } else { 0.0 };

        let status_width = (ui.available_width() - right_reserve).max(160.0);

        // The status message scope on the left
        let latest_attention = latest
            .as_ref()
            .is_some_and(|event| event.level == StatusLevel::Attention);
        let mut attention_clicked = false;
        let response = ui
            .allocate_ui_with_layout(
                egui::vec2(status_width, 18.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    if let Some(l) = latest.as_ref() {
                        let dot_color = match l.level {
                            StatusLevel::Error => theme.tokens.error,
                            StatusLevel::Attention => theme.tokens.error,
                            StatusLevel::Warn => theme.tokens.warning,
                            StatusLevel::Progress | StatusLevel::Info => theme.tokens.success,
                        };
                        let attention = l.level == StatusLevel::Attention;
                        if attention {
                            attention_clicked = ui
                                .add_sized(
                                    [ui.available_width(), 18.0],
                                    egui::Button::new(
                                        egui::RichText::new(&l.message)
                                            .small()
                                            .strong()
                                            .color(theme.tokens.error),
                                    ),
                                )
                                .on_hover_text("Click to continue")
                                .clicked();
                        } else {
                            // Painted circle instead of `●` so we don't depend
                            // on a font that ships U+25CF (the wasm build's
                            // egui font fallback chain doesn't, hence "tofu"
                            // boxes for that glyph).
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                            ui.painter().circle_filled(rect.center(), 4.0, dot_color);
                            ui.label(egui::RichText::new(l.source).small().strong());
                            let text = egui::RichText::new(&l.message).small();
                            ui.add(egui::Label::new(text).truncate());
                            if l.level == StatusLevel::Progress {
                                if let Some(pct) = l.progress_pct {
                                    ui.add(
                                        egui::ProgressBar::new((pct as f32) / 100.0)
                                            .desired_width(120.0)
                                            .desired_height(6.0),
                                    );
                                } else {
                                    ui.spinner();
                                }
                            }
                        }
                    } else {
                        ui.label(egui::RichText::new("ready").small().weak());
                    }
                },
            )
            .response;

        if attention_clicked {
            if let Some(source) = latest
                .as_ref()
                .filter(|event| event.level == StatusLevel::Attention)
                .map(|event| event.source)
            {
                world.trigger(StatusBarAction { source });
            } else {
                unreachable!("attention status button rendered without an attention event");
            }
        } else if !latest_attention
            && response
                .interact(egui::Sense::click())
                .on_hover_text("Click to view recent status events")
                .clicked()
        {
            egui::Popup::toggle_id(ui.ctx(), popup_id);
        }

        if !tutorial_title.is_empty() {
            ui.separator();
            ui.label(
                egui::RichText::new(format!("Tutorial: {tutorial_title}"))
                    .small()
                    .strong(),
            );
        }

        if !scene_name.is_empty() {
            ui.separator();
            let scene_response = ui
                .add(
                    egui::Label::new(egui::RichText::new(format!("Scene: {}", scene_name)).small())
                        .sense(egui::Sense::click()),
                )
                .on_hover_text("Click to show the full path of the loaded USD file");
            if scene_response.clicked() {
                egui::Popup::toggle_id(ui.ctx(), scene_popup_id);
            }
            if !scene_path.is_empty() {
                egui::Popup::from_response(&scene_response)
                    .id(scene_popup_id)
                    .align(egui::RectAlign::BOTTOM_START)
                    .open_memory(None)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                        ui.set_min_width(520.0);
                        ui.heading("Loaded USD file");
                        ui.separator();
                        ui.add(
                            egui::Label::new(egui::RichText::new(&scene_path).monospace()).wrap(),
                        );
                    });
            }
        }

        ui.separator();

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
                let p99 = perf_hud::frame_ms_stats(&frame_history)
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
                draw_frame_time_sparkline(ui, &frame_history, theme);
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
                            StatusLevel::Attention => egui::RichText::new("ACTN ")
                                .small()
                                .color(theme.tokens.error),
                            StatusLevel::Progress => egui::RichText::new("…    ")
                                .small()
                                .color(theme.tokens.text_subdued),
                        };
                        ui.horizontal(|ui| {
                            // Reserve the whole row so the message gets the
                            // remaining width and wraps within the popup,
                            // rather than making the row only as wide as its
                            // contents.
                            ui.set_width(ui.available_width());
                            ui.label(level_tag.monospace());
                            ui.label(
                                egui::RichText::new(format!("[{}]", ev.source))
                                    .small()
                                    .strong(),
                            );
                            ui.add_sized(
                                [ui.available_width(), 0.0],
                                egui::Label::new(egui::RichText::new(&ev.message).small()).wrap(),
                            );
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
fn render_net_chip(ui: &mut egui::Ui, world: &mut World, theme: &lunco_theme::Theme) {
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
        NetworkRole::Client if status.connected => (
            theme.tokens.success,
            format!("CLIENT → {}", status.endpoint),
        ),
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
fn draw_frame_time_sparkline(ui: &mut egui::Ui, frame_history: &[f32], theme: &lunco_theme::Theme) {
    if frame_history.is_empty() {
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
    let max_ms: f32 = frame_history
        .iter()
        .copied()
        .fold(0.0_f32, f32::max)
        .max(25.0_f32);

    // 16.67 ms (60 FPS) reference line — pulls from `text_subdued`
    // and softens with alpha so it doesn't compete with the trace.
    let muted = theme.tokens.text_subdued;
    let muted_soft = muted.alpha(80);
    let ref_y = rect.bottom() - rect.height() * (16.67 / max_ms).min(1.0);
    painter.line_segment(
        [
            egui::pos2(rect.left(), ref_y),
            egui::pos2(rect.right(), ref_y),
        ],
        egui::Stroke::new(0.5, muted_soft),
    );

    let n = frame_history.len();
    let step = rect.width() / (perf_hud::FRAME_HISTORY_LEN - 1).max(1) as f32;
    let mut prev: Option<egui::Pos2> = None;
    for (i, ms) in frame_history.iter().enumerate() {
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
        let scroll_policy = panel.scroll_policy();
        let mut ctx = PanelCtx::new(world);
        match scroll_policy {
            PanelScrollPolicy::Vertical => {
                egui::ScrollArea::vertical()
                    .id_salt(("workbench_solo_panel_body", id.as_str()))
                    .auto_shrink([false; 2])
                    .show(ui, |ui| panel.render(ui, &mut ctx));
            }
            PanelScrollPolicy::SelfManaged => panel.render(ui, &mut ctx),
        }
        let intents = ctx.into_intents();
        layout.panels.insert(*id, panel);
        for intent in intents {
            intent.apply(world);
        }
    } else {
        let error_color = world
            .get_resource::<lunco_theme::Theme>()
            .map(|t| t.tokens.error)
            .unwrap_or(egui::Color32::LIGHT_RED);
        ui.colored_label(
            error_color,
            format!("Panel `{}` not registered", id.as_str()),
        );
    }
}

fn register_workbench_appearance_settings_menu(world: &mut World) {
    use bevy_egui::egui;
    let Some(mut layout) = world.get_resource_mut::<crate::WorkbenchLayout>() else {
        return;
    };
    layout.register_settings_submenu("Appearance", |ui, ctx| {
        let Some(mut settings) = ctx
            .resource::<WorkbenchAppearanceSettings>()
            .copied()
        else {
            return;
        };
        let original = settings;
        ui.checkbox(
            &mut settings.transparent_tab_content,
            "Transparent tab content",
        )
        .on_hover_text(
            "Show the 3D scene through every tab body and its standard panel surface. "
                .to_string(),
        );
        ui.label(
            egui::RichText::new(
                "Off uses the same themed background in every tab; on reveals the scene behind all tabs.",
            )
            .weak()
            .small(),
        );
        if settings != original {
            ctx.set_resource(settings);
        }
    });
}

fn register_graphics_settings_menu(world: &mut World) {
    use bevy_egui::egui;
    let Some(mut layout) = world.get_resource_mut::<crate::WorkbenchLayout>() else {
        return;
    };
    layout.register_settings_submenu("Graphics", |ui, ctx| {
        ui.label(egui::RichText::new("Rendering").weak().small());
        if let Some(current) = ctx.resource::<lunco_render::RenderingQualitySettings>() {
            let mut settings = *current;
            let current_preset = settings.preset();
            let mut selected_preset = current_preset.unwrap_or(lunco_render::RenderingQuality::Balanced);
            let mut preset_changed = false;
            egui::ComboBox::from_id_salt("graphics.rendering_quality")
                .selected_text(current_preset.map_or("Custom", |preset| preset.label()))
                .show_ui(ui, |ui| {
                    for quality in lunco_render::RenderingQuality::all() {
                        preset_changed |= ui
                            .selectable_value(&mut selected_preset, quality, quality.label())
                            .changed();
                    }
                });
            if preset_changed {
                settings.apply_preset(selected_preset);
            }
            ui.label(
                egui::RichText::new(
                    "Presets only suggest values. The fields below are authoritative and are never silently downgraded to another preset.",
                )
                .weak()
                .small(),
            );
            ui.collapsing("Shadow allocation", |ui| {
                ui.add(
                    egui::DragValue::new(&mut settings.directional_shadow_map_size)
                        .speed(128.0)
                        .prefix("Directional map: ")
                        .suffix(" px"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.point_shadow_map_size)
                        .speed(128.0)
                        .prefix("Point map: ")
                        .suffix(" px"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.directional_cascades)
                        .speed(1.0)
                        .range(1..=bevy::pbr::MAX_CASCADES_PER_LIGHT)
                        .prefix("Directional cascades: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.max_directional_shadow_casters)
                        .speed(1.0)
                        .range(0..=bevy::pbr::MAX_DIRECTIONAL_LIGHTS)
                        .prefix("Directional shadow casters: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.max_point_shadow_casters)
                        .speed(1.0)
                        .prefix("Point shadow casters: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.max_spot_shadow_casters)
                        .speed(1.0)
                        .prefix("Spot shadow casters: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.shadow_depth_bias)
                        .speed(0.005)
                        .prefix("Shadow depth bias: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.shadow_normal_bias)
                        .speed(0.1)
                        .prefix("Shadow normal bias: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.shadow_budget_bytes)
                        .speed(1024.0 * 1024.0)
                        .range(1..=u64::MAX)
                        .prefix("Logical shadow byte ceiling: ")
                        .suffix(" bytes"),
                );
                ui.label(
                    egui::RichText::new(
                        "This explicit Depth32 shadow-storage ceiling must cover the configured caster limits. It never changes map sizes, cascades, or caster limits automatically; adapter limits are reported separately.",
                    )
                    .weak()
                    .small(),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.shadow_minimum_distance)
                        .speed(0.1)
                        .prefix("Shadow minimum distance: ")
                        .suffix(" m"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.shadow_first_cascade_far_bound)
                        .speed(1.0)
                        .prefix("First cascade far bound: ")
                        .suffix(" m"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.shadow_maximum_distance)
                        .speed(10.0)
                        .prefix("Maximum shadow distance: ")
                        .suffix(" m"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.shadow_cascade_overlap)
                        .speed(0.01)
                        .prefix("Cascade overlap: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.local_light_default_range)
                        .speed(1.0)
                        .prefix("Local-light default range: ")
                        .suffix(" m"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.local_shadow_map_near_z)
                        .speed(0.01)
                        .prefix("Local shadow near Z: ")
                        .suffix(" m"),
                );
            });
            ui.collapsing("Horizon terrain shadows", |ui| {
                ui.checkbox(
                    &mut settings.horizon_shadow_cache_enabled,
                    "Use pre-baked horizon shadow cache",
                );
                ui.add(
                    egui::DragValue::new(&mut settings.horizon_shadow_cache_sun_threshold_deg)
                        .speed(0.01)
                        .range(0.001..=179.0)
                        .prefix("Cache refresh angle: ")
                        .suffix("°"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.horizon_march_steps)
                        .speed(1.0)
                        .range(1..=4096)
                        .prefix("Live march steps: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.horizon_cache_samples_per_axis)
                        .speed(1.0)
                        .range(1..=8)
                        .prefix("Cache samples per axis: "),
                );
                ui.label(
                    egui::RichText::new(
                        "These are explicit terrain-shadow quality controls. Cache use and bake sampling are never changed automatically by the platform or memory budget.",
                    )
                    .weak()
                    .small(),
                );
            });
            ui.collapsing("Light defaults", |ui| {
                ui.label(
                    egui::RichText::new(
                        "These values apply only when a USD light omits its intensity; authored USD intensity remains authoritative.",
                    )
                    .weak()
                    .small(),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.distant_light_default_illuminance)
                        .speed(1_000.0)
                        .prefix("Distant-light default: ")
                        .suffix(" lx"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.local_light_default_intensity)
                        .speed(100.0)
                        .prefix("Sphere-light default: ")
                        .suffix(" lm"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.rect_light_default_intensity)
                        .speed(100.0)
                        .prefix("Rect-light default: ")
                        .suffix(" lm"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.dome_default_intensity)
                        .speed(100.0)
                        .prefix("Textured-dome default: ")
                        .suffix(" cd/m²"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.dome_cubemap_face_size)
                        .speed(128.0)
                        .range(1..=4096)
                        .prefix("Dome cubemap face size: ")
                        .suffix(" px (power of two)"),
                );
            });
            ui.collapsing("Parametric surfaces", |ui| {
                ui.label(
                    egui::RichText::new(
                        "NURBS tessellation controls mesh detail only; USD control nets, orders, and authored trim data remain authoritative.",
                    )
                    .weak()
                    .small(),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.nurbs_surface_samples_per_control_span)
                        .speed(1.0)
                        .range(1..=64)
                        .prefix("Samples per control span: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.nurbs_surface_minimum_subdivisions)
                        .speed(1.0)
                        .range(1..=4096)
                        .prefix("Surface minimum subdivisions: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.nurbs_surface_maximum_subdivisions)
                        .speed(1.0)
                        .range(1..=4096)
                        .prefix("Surface maximum subdivisions: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.nurbs_trim_curve_samples)
                        .speed(1.0)
                        .range(1..=4096)
                        .prefix("Trim-curve samples: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.nurbs_trim_minimum_subdivisions)
                        .speed(1.0)
                        .range(1..=4096)
                        .prefix("Trim minimum subdivisions: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.nurbs_trim_maximum_subdivisions)
                        .speed(1.0)
                        .range(1..=4096)
                        .prefix("Trim maximum subdivisions: "),
                );
            });
            ui.collapsing("Primitive meshes", |ui| {
                ui.label(
                    egui::RichText::new(
                        "These settings control viewer tessellation for USD spheres, cylinders, cones, and capsules; USD dimensions remain authoritative.",
                    )
                    .weak()
                    .small(),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.primitive_sphere_longitudes)
                        .speed(1.0)
                        .range(3..=4096)
                        .prefix("Sphere longitudes: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.primitive_sphere_latitudes)
                        .speed(1.0)
                        .range(2..=4096)
                        .prefix("Sphere latitudes: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.primitive_radial_segments)
                        .speed(1.0)
                        .range(3..=4096)
                        .prefix("Cylinder/cone radial segments: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.primitive_capsule_longitudes)
                        .speed(1.0)
                        .range(3..=4096)
                        .prefix("Capsule longitudes: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.primitive_capsule_latitudes)
                        .speed(1.0)
                        .range(2..=4096)
                        .prefix("Capsule latitudes: "),
                );
            });
            ui.collapsing("Curve tubes", |ui| {
                ui.label(
                    egui::RichText::new(
                        "These settings control only the viewer tessellation of USD curve tubes; curve points, widths, and topology remain authored USD data.",
                    )
                    .weak()
                    .small(),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.curve_samples_per_segment)
                        .speed(1.0)
                        .range(1..=4096)
                        .prefix("Samples per curve segment: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.curve_radial_segments)
                        .speed(1.0)
                        .range(3..=4096)
                        .prefix("Tube radial segments: "),
                );
            });
            ui.collapsing("Camera look", |ui| {
                ui.label(
                    egui::RichText::new(
                        "These settings apply to scene cameras when USD does not author an environment bloom override.",
                    )
                    .weak()
                    .small(),
                );
                egui::ComboBox::from_id_salt("graphics.camera_tone_map")
                    .selected_text(match settings.camera_tone_map {
                        lunco_render::ToneMap::None => "None",
                        lunco_render::ToneMap::TonyMcMapface => "TonyMcMapface",
                        lunco_render::ToneMap::AgX => "AgX",
                        lunco_render::ToneMap::AcesFitted => "ACES fitted",
                        lunco_render::ToneMap::Reinhard => "Reinhard",
                    })
                    .show_ui(ui, |ui| {
                        for tone_map in [
                            lunco_render::ToneMap::None,
                            lunco_render::ToneMap::TonyMcMapface,
                            lunco_render::ToneMap::AgX,
                            lunco_render::ToneMap::AcesFitted,
                            lunco_render::ToneMap::Reinhard,
                        ] {
                            let label = match tone_map {
                                lunco_render::ToneMap::None => "None",
                                lunco_render::ToneMap::TonyMcMapface => "TonyMcMapface",
                                lunco_render::ToneMap::AgX => "AgX",
                                lunco_render::ToneMap::AcesFitted => "ACES fitted",
                                lunco_render::ToneMap::Reinhard => "Reinhard",
                            };
                            ui.selectable_value(&mut settings.camera_tone_map, tone_map, label);
                        }
                    });
                egui::ComboBox::from_id_salt("graphics.camera_msaa")
                    .selected_text(match settings.camera_msaa {
                        lunco_render::MsaaLevel::Off => "Off",
                        lunco_render::MsaaLevel::X2 => "2x",
                        lunco_render::MsaaLevel::X4 => "4x",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut settings.camera_msaa,
                            lunco_render::MsaaLevel::Off,
                            "Off",
                        );
                        ui.selectable_value(
                            &mut settings.camera_msaa,
                            lunco_render::MsaaLevel::X2,
                            "2x",
                        );
                        ui.selectable_value(
                            &mut settings.camera_msaa,
                            lunco_render::MsaaLevel::X4,
                            "4x",
                        );
                    });
                ui.add(
                    egui::DragValue::new(&mut settings.camera_exposure_ev100)
                        .speed(0.1)
                        .prefix("Unauthored camera exposure (EV100): "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.camera_bloom_intensity)
                        .speed(0.01)
                        .prefix("Bloom intensity: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.camera_bloom_low_frequency_boost)
                        .speed(0.01)
                        .prefix("Bloom low-frequency boost: "),
                );
                ui.label(
                    egui::RichText::new(
                        "A positive bloom intensity enables HDR. An authored USD environment value wins over this default; no automatic quality downgrade is applied.",
                    )
                    .weak()
                    .small(),
                );
            });
            ui.collapsing("Presentation recovery", |ui| {
                ui.label(
                    egui::RichText::new(
                        "These are safety timings for render failures, not quality fallbacks. The renderer never changes quality automatically.",
                    )
                    .weak()
                    .small(),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.render_failure_quiet_period_secs)
                        .speed(0.1)
                        .range(0.01..=settings.render_failure_give_up_after_secs)
                        .prefix("Failure quiet period: ")
                        .suffix(" s"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.render_failure_give_up_after_secs)
                        .speed(0.5)
                        .range(settings.render_failure_quiet_period_secs..=3600.0)
                        .prefix("Stop presentation after: ")
                        .suffix(" s"),
                );
            });
            ui.collapsing("Terrain mesh cache", |ui| {
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_mesh_cache_bytes)
                        .speed(16.0 * 1024.0 * 1024.0)
                        .range(1..=u64::MAX)
                        .prefix("Mesh-cache byte ceiling: ")
                        .suffix(" bytes"),
                );
                ui.label(
                    egui::RichText::new(
                        "The cache evicts least-recently-used meshes at this explicit ceiling; terrain detail is not silently downgraded.",
                    )
                    .weak()
                    .small(),
                );
            });
            ui.collapsing("Terrain derived maps", |ui| {
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_derived_map_resolution)
                        .speed(128.0)
                        .range(1..=4096)
                        .prefix("Map resolution: ")
                        .suffix(" px/side (power of two)"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_derived_ao_directions)
                        .speed(1.0)
                        .range(1..=64)
                        .prefix("AO directions: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_derived_ao_steps)
                        .speed(1.0)
                        .range(1..=64)
                        .prefix("AO steps per direction: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_derived_ao_radius_fraction)
                        .speed(0.01)
                        .range(0.01..=1.0)
                        .prefix("AO radius fraction: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_derived_roughness_base)
                        .speed(0.01)
                        .range(0.0..=1.0)
                        .prefix("Flat-ground roughness: "),
                );
                ui.add(
                    egui::DragValue::new(
                        &mut settings.terrain_derived_roughness_saturation_radians,
                    )
                    .speed(0.01)
                    .range(0.01..=std::f32::consts::FRAC_PI_2)
                    .prefix("Roughness saturation slope: ")
                    .suffix(" rad"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_derived_texture_anisotropy)
                        .speed(1.0)
                        .range(1..=16)
                        .prefix("Derived-texture anisotropy: "),
                );
                ui.label(
                    egui::RichText::new(
                        "These settings control the baked terrain roughness, ambient-occlusion, normal textures, and filtering. Changes rebake off-thread and keep the previous maps visible until ready.",
                    )
                    .weak()
                    .small(),
                );
            });
            ui.collapsing("Terrain rocks", |ui| {
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_rock_max_instances)
                        .speed(128.0)
                        .range(1..=1_000_000)
                        .prefix("Maximum rock instances: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_rock_mesh_buckets)
                        .speed(1.0)
                        .range(2..=64)
                        .prefix("Rock size buckets: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_rock_mesh_cube_count)
                        .speed(1.0)
                        .range(1..=64)
                        .prefix("Boxes per rock mesh: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_rock_lod_start_distance)
                        .speed(10.0)
                        .range(0.0..=100_000.0)
                        .prefix("Rock LOD start: ")
                        .suffix(" m"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_rock_lod_fade_distance)
                        .speed(10.0)
                        .range(0.1..=100_000.0)
                        .prefix("Rock LOD fade: ")
                        .suffix(" m"),
                );
                ui.label(
                    egui::RichText::new(
                        "The instance limit is explicit: authored density is never silently reduced by a hidden renderer cap. Mesh detail and native visibility distances are Graphics settings.",
                    )
                    .weak()
                    .small(),
                );
            });
            ui.collapsing("Terrain LOD", |ui| {
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_tile_resolution)
                        .speed(2.0)
                        .range(3..=4097)
                        .prefix("Streamed tile resolution: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_cinematic_resolution)
                        .speed(2.0)
                        .range(3..=4097)
                        .prefix("Cinematic tile resolution: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_pixel_error)
                        .speed(0.1)
                        .range(0.1..=32.0)
                        .prefix("Screen error: ")
                        .suffix(" px"),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_max_depth)
                        .speed(1.0)
                        .range(1..=20)
                        .prefix("Max depth: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_probe_resolution)
                        .speed(2.0)
                        .range(3..=257)
                        .prefix("Error probe resolution: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_bakes_per_frame)
                        .speed(1.0)
                        .range(1..=256)
                        .prefix("Bakes per frame: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_max_inflight_bakes)
                        .speed(1.0)
                        .range(1..=512)
                        .prefix("In-flight bakes: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_tile_budget)
                        .speed(16.0)
                        .range(1..=8192)
                        .prefix("Selected tile budget: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_cover_edits_per_frame)
                        .speed(4.0)
                        .range(1..=4096)
                        .prefix("Cover edits per frame: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_hysteresis_ratio)
                        .speed(0.01)
                        .range(1.01..=4.0)
                        .prefix("LOD hysteresis ratio: "),
                );
                ui.add(
                    egui::DragValue::new(&mut settings.terrain_lod_morph_start_ratio)
                        .speed(0.01)
                        .range(0.0..=0.99)
                        .prefix("Geomorph start ratio: "),
                );
                ui.label(
                    egui::RichText::new(
                        "These are explicit terrain rendering controls. A custom value is applied as authored; the renderer does not silently choose a lower preset.",
                    )
                    .weak()
                    .small(),
                );
            });
            if let Err(reason) = settings.validate() {
                let error_color = ctx
                    .resource::<lunco_theme::Theme>()
                    .map(|theme| theme.tokens.error)
                    .unwrap_or(egui::Color32::LIGHT_RED);
                ui.colored_label(
                    error_color,
                    format!("Graphics settings rejected: {reason}"),
                );
            }
            // Keep invalid edits in memory so dependent fields can be corrected
            // over multiple UI interactions. Runtime consumers validate at their
            // own boundaries and preserve the last applied quality; the settings
            // persister likewise refuses to replace the last valid disk value.
            if settings != *current {
                ctx.set_resource(settings);
            }
        }

        ui.separator();
        if let Some(current) = ctx.resource::<lunco_render::CommunicationLineSettings>() {
            let mut settings = *current;
            ui.checkbox(&mut settings.show, "Show communication lines")
                .on_hover_text(
                    "Display runtime connectivity beams between communication endpoints. \
                     Off by default; the setting affects only this viewer.",
                );
            if settings != *current {
                ctx.set_resource(settings);
            }
        }

        ui.separator();
        ui.label(egui::RichText::new("Terrain").weak().small());
        let Some(mut settings) = ctx.resource::<lunco_settings::TerrainSettings>().cloned() else {
            return;
        };
        let original = settings.clone();
        ui.checkbox(
            &mut settings.enable_shaders,
            "Enable high-quality procedural shaders",
        )
        .on_hover_text(
            "Enable dynamic micro-relief normal mapping and albedo mottle. \
                 Turning this off improves WebAssembly/browser frame rate. \
                 Persisted to ~/.lunco/settings.json.",
        );
        ui.add(
            egui::Slider::new(&mut settings.visual_detail_radius_m, 5.0..=200.0)
                .text("Camera detail radius (m)"),
        )
        .on_hover_text(
            "Distance around each active terrain camera that requests the finest \
             available terrain geometry. Persisted to ~/.lunco/settings.json.",
        );
        ui.add(
            egui::Slider::new(&mut settings.visual_detail_hysteresis_m, 0.0..=200.0)
                .text("Camera detail retention (m)"),
        )
        .on_hover_text(
            "Extra distance that keeps already-refined tiles resident while the \
             camera moves away, preventing fine-to-coarse-to-fine flicker. \
             Persisted to ~/.lunco/settings.json.",
        );
        if settings != original {
            ctx.set_resource(settings);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_content_setting_has_uniform_default_and_transparent_override() {
        let mut world = World::new();

        // Standalone panel harnesses retain the panel declaration when the
        // workbench appearance resource is not installed.
        assert!(panel_body_is_transparent(&world, true, false));

        world.insert_resource(WorkbenchAppearanceSettings::default());
        // The application default is one opaque mantle surface for every tab,
        // including panels that historically opted into transparency.
        assert!(!panel_body_is_transparent(&world, true, false));

        world
            .resource_mut::<WorkbenchAppearanceSettings>()
            .transparent_tab_content = true;
        assert!(panel_body_is_transparent(&world, false, false));
        // The scene host is always transparent, independent of the preference.
        assert!(panel_body_is_transparent(&world, false, true));
    }

    #[test]
    fn tab_content_setting_deserializes_older_empty_section() {
        let settings: WorkbenchAppearanceSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, WorkbenchAppearanceSettings::default());
    }

    #[test]
    fn build_identity_formats_the_shared_version_label() {
        let identity = BuildIdentity::new(
            "0.6.0-nightly.37.1",
            "abc12345-dirty",
            "https://github.com/LunCoSim/lunco-sim",
        );

        assert_eq!(
            identity.version_label(),
            "Version 0.6.0-nightly.37.1 (abc12345-dirty)"
        );
        assert_eq!(
            identity.source_url().as_deref(),
            Some("https://github.com/LunCoSim/lunco-sim/commit/abc12345")
        );
    }

    /// `FocusPanel` can arrive from another UI/domain plugin before (or without)
    /// `WorkbenchPlugin`; an absent dock is a valid no-op state, not a fatal
    /// observer-parameter error.
    #[test]
    fn focus_panel_without_workbench_layout_does_not_panic() {
        let mut app = App::new();
        app.add_observer(on_focus_panel);
        app.world_mut().trigger(FocusPanel {
            id: "not_mounted".into(),
        });
    }

    #[test]
    fn focus_panel_is_queued_while_layout_is_scoped_out() {
        let mut app = App::new();
        app.init_resource::<PendingPanelFocus>();
        app.add_observer(on_focus_panel);
        app.world_mut().trigger(FocusPanel {
            id: "source_viewer".into(),
        });
        assert_eq!(
            app.world().resource::<PendingPanelFocus>().0,
            ["source_viewer"]
        );
    }

    struct FocusPanelFixture;

    impl Panel for FocusPanelFixture {
        fn id(&self) -> PanelId {
            PanelId("focus_fixture")
        }

        fn title(&self) -> String {
            "Focus fixture".into()
        }

        fn default_slot(&self) -> PanelSlot {
            PanelSlot::SideBrowser
        }

        fn render(&mut self, _ui: &mut egui::Ui, _ctx: &mut PanelCtx) {}
    }

    #[test]
    fn focus_panel_mounts_a_registered_closed_panel_in_its_authored_slot() {
        let mut layout = WorkbenchLayout::default();
        layout.register(FocusPanelFixture);
        // Simulate the viewport-only perspective: the panel remains registered
        // globally, but its tab is not part of this perspective's dock.
        layout.set_side_browser(None);
        assert!(!layout
            .dock
            .iter_all_tabs()
            .any(|(_, tab)| *tab == TabId::Singleton(PanelId("focus_fixture"))));

        focus_panel_now(&mut layout, "focus_fixture");

        assert!(layout
            .dock
            .iter_all_tabs()
            .any(|(_, tab)| *tab == TabId::Singleton(PanelId("focus_fixture"))));
        assert_eq!(layout.side_browser, [PanelId("focus_fixture")]);
    }

    struct DockPanel(PanelId);

    impl Panel for DockPanel {
        fn id(&self) -> PanelId {
            self.0
        }

        fn title(&self) -> String {
            self.0 .0.to_string()
        }

        fn default_slot(&self) -> PanelSlot {
            PanelSlot::Center
        }

        fn render(&mut self, _ui: &mut egui::Ui, _ctx: &mut PanelCtx) {}
    }

    #[test]
    fn stacked_side_slots_build_independent_top_and_bottom_leaves() {
        let mut layout = WorkbenchLayout::default();
        for id in ["entities", "telemetry", "viewport", "inspector", "spawn"] {
            layout.register(DockPanel(PanelId(id)));
        }

        layout.set_side_browser_stacked(vec![PanelId("entities")], vec![PanelId("telemetry")]);
        layout.set_center(vec![PanelId("viewport")]);
        layout.set_right_inspector_stacked(vec![PanelId("inspector")], vec![PanelId("spawn")]);

        let leaves: Vec<Vec<TabId>> = layout
            .dock
            .main_surface()
            .iter()
            .filter_map(|node| match node {
                egui_dock::Node::Leaf(leaf) => Some(leaf.tabs.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(leaves.len(), 5, "center plus two stacked side regions");
        for id in ["entities", "telemetry", "viewport", "inspector", "spawn"] {
            assert!(
                leaves
                    .iter()
                    .any(|tabs| tabs.contains(&TabId::Singleton(PanelId(id)))),
                "missing dock panel {id}"
            );
        }
    }

    #[test]
    fn registering_a_predeclared_stacked_panel_keeps_its_declared_slot() {
        let mut layout = WorkbenchLayout::default();
        layout.set_side_browser_stacked(vec![PanelId("entities")], vec![PanelId("telemetry")]);
        layout.set_center(vec![PanelId("viewport")]);
        layout.set_right_inspector_stacked(vec![PanelId("inspector")], vec![PanelId("spawn")]);

        layout.register(DockPanel(PanelId("entities")));
        layout.register(DockPanel(PanelId("telemetry")));
        layout.register(DockPanel(PanelId("viewport")));
        layout.register(DockPanel(PanelId("inspector")));
        layout.register(DockPanel(PanelId("spawn")));

        assert_eq!(layout.side_browser, [PanelId("entities")]);
        assert_eq!(layout.side_browser_bottom, [PanelId("telemetry")]);
        assert_eq!(layout.right_inspector, [PanelId("inspector")]);
        assert_eq!(layout.right_inspector_bottom, [PanelId("spawn")]);
        assert!(layout.bottom.is_empty());
    }

    struct TestPerspective {
        id: PerspectiveId,
        title: &'static str,
        marker: PanelId,
    }

    impl Perspective for TestPerspective {
        fn id(&self) -> PerspectiveId {
            self.id
        }
        fn title(&self) -> String {
            self.title.to_string()
        }
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
    fn perspectives_keep_separate_docks() {
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
        // A is active (first-registered). Simulate a tab open only in A.
        let only_a = TabId::Singleton(PanelId("only_in_a"));
        layout.dock = DockState::new(vec![only_a]);

        // A → B: B must NOT inherit A's tab.
        layout.activate_perspective(PerspectiveId("b"));
        assert_eq!(layout.active_perspective(), Some(PerspectiveId("b")));
        assert!(
            !layout.dock.iter_all_tabs().any(|(_, t)| *t == only_a),
            "B inherited A's tab — each perspective must keep its own dock"
        );

        // B → A: A's tab must come back from the per-perspective cache.
        layout.activate_perspective(PerspectiveId("a"));
        assert_eq!(layout.active_perspective(), Some(PerspectiveId("a")));
        assert!(
            layout.dock.iter_all_tabs().any(|(_, t)| *t == only_a),
            "A's tab was not restored on return"
        );
    }

    #[test]
    fn reset_to_default_layout_drops_cached_tabs() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("a"),
            title: "A",
            marker: PanelId("panel_a"),
        });
        let only_a = TabId::Singleton(PanelId("only_in_a"));
        layout.dock = DockState::new(vec![only_a]);
        // Round-trip through another perspective so A's tab is cached, then
        // a reset of A must produce a clean preset, not the cached tab.
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("b"),
            title: "B",
            marker: PanelId("panel_b"),
        });
        layout.activate_perspective(PerspectiveId("b"));
        layout.activate_perspective(PerspectiveId("a"));
        assert!(layout.dock.iter_all_tabs().any(|(_, t)| *t == only_a));

        layout.reset_to_default_layout();
        assert!(
            !layout.dock.iter_all_tabs().any(|(_, t)| *t == only_a),
            "reset restored the cached tab instead of a clean preset"
        );
        assert_eq!(layout.side_browser, vec![PanelId("panel_a")]);
    }

    #[test]
    fn reset_to_default_perspective_discards_current_view_and_all_caches() {
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
        layout.dock = DockState::new(vec![TabId::Singleton(PanelId("stale"))]);
        layout.activate_perspective(PerspectiveId("a"));
        assert_eq!(layout.active_perspective(), Some(PerspectiveId("a")));
        assert!(layout.dock_cache.contains_key(&PerspectiveId("b")));

        layout.reset_to_default_perspective();

        assert_eq!(layout.active_perspective(), Some(PerspectiveId("a")));
        assert!(layout.dock_cache.is_empty());
        assert!(!layout
            .dock
            .iter_all_tabs()
            .any(|(_, tab)| *tab == TabId::Singleton(PanelId("stale"))));
        assert_eq!(layout.side_browser, vec![PanelId("panel_a")]);
    }

    #[test]
    fn required_perspective_keeps_guided_presentations_in_their_authored_layout() {
        let mut layout = WorkbenchLayout::default();
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("view"),
            title: "View",
            marker: PanelId("view_panel"),
        });
        layout.register_perspective(TestPerspective {
            id: PerspectiveId("build"),
            title: "Build",
            marker: PanelId("build_panel"),
        });

        layout.set_required_perspective(Some("build"));
        layout.activate_perspective(PerspectiveId("view"));

        assert_eq!(layout.active_perspective(), Some(PerspectiveId("build")));
        assert_eq!(layout.side_browser, [PanelId("build_panel")]);

        layout.set_required_perspective(None);
        layout.activate_perspective(PerspectiveId("view"));
        assert_eq!(layout.active_perspective(), Some(PerspectiveId("view")));
    }

    /// A NaN split fraction (egui_dock poisons the tree from inside `show`)
    /// serializes to JSON `null`, which used to fail `from_value` outright —
    /// so the user's whole dock silently reset on every launch. Build a real
    /// split, poison it, and round-trip through serde rather than asserting on
    /// a hand-written JSON literal, so this stays honest if egui_dock changes
    /// its wire format.
    #[test]
    fn nan_split_fraction_survives_a_dock_json_round_trip() {
        let mut dock: DockState<TabId> = DockState::new(vec![TabId::Singleton(PanelId("a"))]);
        dock.main_surface_mut().split_right(
            egui_dock::NodeIndex::root(),
            0.5,
            vec![TabId::Singleton(PanelId("b"))],
        );

        // Reproduce the upstream `0.0 / 0.0` poisoning.
        for (_surface, node) in dock.iter_all_nodes_mut() {
            if let egui_dock::Node::Horizontal(s) | egui_dock::Node::Vertical(s) = node {
                s.fraction = f32::NAN;
            }
        }

        let mut value = serde_json::to_value(&dock).expect("dock serializes");
        assert!(
            serde_json::from_value::<DockState<TabId>>(value.clone()).is_err(),
            "expected non-finite floats to serialize as null and fail to parse \
             — if this now succeeds, serde_json changed and the heal is dead code"
        );

        heal_non_finite_nulls(&mut value);
        let restored: DockState<TabId> =
            serde_json::from_value(value).expect("healed dock must parse");

        let fractions: Vec<f32> = restored
            .iter_all_nodes()
            .filter_map(|(_surface, node)| match node {
                egui_dock::Node::Horizontal(s) | egui_dock::Node::Vertical(s) => Some(s.fraction),
                _ => None,
            })
            .collect();
        assert!(
            !fractions.is_empty(),
            "split node did not survive the round trip"
        );
        assert!(
            fractions.iter().all(|f| (*f - 0.5).abs() < f32::EPSILON),
            "healed fractions should default to 0.5, got {fractions:?}"
        );
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
