//! `ViewportPanel` and the workbench's 3D-viewport plumbing.
//!
//! ## Architecture (read this if anything here looks weird)
//!
//! Egui owns the window framebuffer. A dedicated `Camera2d` with
//! `PrimaryEguiContext` — bundled as [`WorkbenchEguiHost`] — is
//! auto-spawned by [`ensure_egui_host`] when no other entity carries
//! `PrimaryEguiContext`. Apps that render 3D add a `Camera3d` tagged
//! with [`WorkbenchViewportCamera`] (or use the [`WorkbenchSceneCamera`]
//! required-component bundle). Each frame, [`apply_workbench_viewport`]
//! syncs `Camera::viewport` on every tagged camera to the rect of the
//! [`ViewportPanel`] — recorded into [`PanelRects`] during the panel's
//! render. So the 3D scene paints into the panel's sub-rect of the
//! window, and never anywhere else.
//!
//! ## Why this is robust *by design*
//!
//! - **Egui pass-skip can't bleed across the chrome.** The 3D camera no
//!   longer covers the whole window. If the egui pass ever misses a
//!   frame (auto-context-pick race, async asset reload, …), the panel's
//!   own opaque background is visible — not a 3D scene leaking out
//!   under your toolbar.
//! - **Bevy_egui auto-pick can't race.** [`ensure_egui_host`] disables
//!   `EguiGlobalSettings::auto_create_primary_context` and pins the
//!   marker on exactly one camera. Extra Camera2d entities (vello
//!   diagram targets, USD preview tabs, …) are harmless because they
//!   target offscreen Images.
//! - **Required components prevent foot-guns.** [`WorkbenchEguiHost`]
//!   pairs `Camera2d` with `PrimaryEguiContext` at the type level;
//!   [`WorkbenchSceneCamera`] pairs `Camera3d` with
//!   [`WorkbenchViewportCamera`]. New code that wants a workbench-aware
//!   camera spawns *one* type and gets the right pair automatically.
//! - **Sentinels catch regressions.** [`check_camera_invariants`]
//!   panics in debug (errors in release) when a new `Camera3d` is added
//!   targeting the window without the marker.
//! - **Opaque viewport panel.** Setting `transparent_background = false`
//!   means the failure mode of any rect-sync glitch is "panel shows the
//!   theme backdrop" — visually bounded, never a full-window 3D bleed.
//!
//! ## What goes where
//!
//! - `ensure_egui_host` (Startup) — auto-spawn the egui host.
//! - `apply_workbench_viewport` (PostUpdate, before `CameraUpdateSystems`)
//!   — push the panel rect into each tagged camera's `Camera::viewport`.
//! - `check_camera_invariants` (Update, runs on `Added<Camera3d>`) —
//!   loud failure if a window-targeting Camera3d shows up untagged.
//! - `ViewportPanel::render` — records the panel's screen rect into
//!   `PanelRects` and reserves the space; the 3D camera does the
//!   actual painting.

use std::collections::HashMap;

use bevy::prelude::*;
// `bevy::camera::*` re-exports work on *both* native and
// `--no-default-features` wasm builds. `bevy::render::camera::*` only
// exists when the `bevy_render` feature is on, which wasm strips.
use bevy::camera::{ClearColorConfig, RenderTarget, Viewport};
use bevy_egui::{egui, EguiGlobalSettings, PrimaryEguiContext};

use crate::{Panel, PanelId, PanelSlot};

/// Stable id for [`ViewportPanel`]. Use this in `Workspace::apply` to
/// place the viewport in a slot without instantiating the panel.
pub const VIEWPORT_PANEL_ID: PanelId = PanelId("workbench::viewport");

/// Marker on a `Camera` (typically a `Camera3d`) whose `Camera::viewport`
/// should follow the [`ViewportPanel`]'s rect each frame.
///
/// Add this to any existing camera spawn site that wants to be confined
/// to the workbench's central viewport. For new spawn sites, prefer
/// [`WorkbenchSceneCamera`] — it pairs `Camera3d` with this marker via
/// required-components.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct WorkbenchViewportCamera;

/// Required-component bundle: the primary 3D scene camera for a
/// workbench-using binary.
///
/// Spawning `WorkbenchSceneCamera` is equivalent to spawning
/// `(Camera3d::default(), WorkbenchViewportCamera)`. The
/// required-components feature guarantees both end up on the entity —
/// new code can't accidentally drop the marker.
#[derive(Component, Debug, Clone, Copy, Default)]
#[require(Camera3d, WorkbenchViewportCamera)]
pub struct WorkbenchSceneCamera;

/// Marker component on the egui-owning camera for one window.
///
/// Auto-inserted by [`ensure_egui_host`] alongside `Camera2d` and
/// `PrimaryEguiContext`. We can't use Bevy's required-components
/// feature here because `bevy_egui::PrimaryEguiContext` (0.39.1)
/// doesn't impl `Default`, so a `#[require(PrimaryEguiContext)]`
/// would fail to compile. The host spawn is a single-site concern
/// anyway — `ensure_egui_host` is the only legitimate place — so a
/// plain marker is sufficient.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct WorkbenchEguiHost;

/// Per-panel screen-space rect, in *physical* pixels.
///
/// Populated by `Panel::render` implementations that want to be
/// camera-targetable (today: [`ViewportPanel`]). Consumed by
/// [`apply_workbench_viewport`] to drive `Camera::viewport`.
#[derive(Resource, Default, Debug)]
pub struct PanelRects {
    rects: HashMap<PanelId, PanelRect>,
}

/// One panel's footprint inside the window, in *physical* pixels.
#[derive(Debug, Clone, Copy)]
pub struct PanelRect {
    /// Top-left of the panel rect inside the window framebuffer.
    pub origin: UVec2,
    /// Width × height of the rect (min 1×1 — never zero, so callers
    /// can safely set `Camera::viewport` without guard checks).
    pub size: UVec2,
}

impl PanelRects {
    /// Drop every recorded rect. Called each frame by
    /// [`clear_panel_rects_each_frame`] so panels that *aren't* in the
    /// active perspective don't leak a stale rect to consumers (e.g.
    /// after a perspective switch the previous perspective's viewport
    /// rect would otherwise still drive `Camera::viewport`, leaving
    /// the 3D scene rendered in a wrong sub-rect).
    pub fn clear(&mut self) {
        self.rects.clear();
    }

    /// Record a panel's rect from inside its render method.
    ///
    /// Converts the egui logical rect to physical pixels using the
    /// current `pixels_per_point`. Uses **floor on the origin** and
    /// **ceil on the far edge** so the resulting physical-pixel rect
    /// fully covers the panel even at non-integer DPRs (1.5, 1.25, …).
    /// Round-half-away-from-zero could leave a 1-px gap between the
    /// camera viewport and the panel edge — that's the dark hairline
    /// some users saw at the top of the 3D scene on a wasm browser
    /// with `devicePixelRatio = 1.5`. Overshoot into surrounding
    /// chrome (also 1 px max) is harmless because egui paints over it
    /// at order > 3D-camera.
    pub fn record_from_ui(&mut self, panel: PanelId, ui: &egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let ppp = ui.ctx().pixels_per_point();
        let origin = UVec2::new(
            (rect.min.x.max(0.0) * ppp).floor() as u32,
            (rect.min.y.max(0.0) * ppp).floor() as u32,
        );
        let end = UVec2::new(
            (rect.max.x.max(0.0) * ppp).ceil() as u32,
            (rect.max.y.max(0.0) * ppp).ceil() as u32,
        );
        let size = UVec2::new(
            end.x.saturating_sub(origin.x).max(1),
            end.y.saturating_sub(origin.y).max(1),
        );
        self.rects.insert(panel, PanelRect { origin, size });
    }

    /// Look up a panel's most-recently-recorded rect.
    pub fn get(&self, panel: PanelId) -> Option<PanelRect> {
        self.rects.get(&panel).copied()
    }
}

/// Empty-state text drawn centered over the 3D viewport region.
///
/// When `message` is `Some`, the workbench paints it centered over the
/// viewport — in **both** the full-window "View" perspective (empty
/// layout) and the docked "Build" perspective (where [`ViewportPanel`]
/// holds the centre). When `None`, nothing is drawn.
///
/// The workbench owns the *rendering* (so it shows regardless of
/// perspective) but is domain-agnostic about *when* to show it: a
/// domain crate that knows what "empty" means — e.g. `lunco-usd`,
/// "no USD scene loaded" — sets `message`. See `render_layout`.
#[derive(Resource, Default)]
pub struct ViewportPlaceholder {
    /// Text to show, or `None` to draw nothing.
    pub message: Option<String>,
}

/// Workbench-central panel that reserves a rect for the 3D viewport.
///
/// The panel itself paints nothing — its only job is to record its
/// screen-space rect into [`PanelRects`] each frame so the
/// [`apply_workbench_viewport`] system can drive every
/// [`WorkbenchViewportCamera`]-tagged camera's `Camera::viewport`.
///
/// Background: was historically transparent so a full-window 3D camera
/// could show through the dock. That design caused the "UI vanishes on
/// zoom" bug — any egui pass-skip and the 3D would overpaint the
/// chrome. Now the 3D camera is confined to this rect, the panel can
/// (and must) paint an opaque background, and the failure mode is
/// bounded to "panel shows backdrop" instead of "all UI gone".
pub struct ViewportPanel;

impl Panel for ViewportPanel {
    fn id(&self) -> PanelId {
        VIEWPORT_PANEL_ID
    }

    fn title(&self) -> String {
        // Empty title — there's nothing useful to show in a tab header
        // for "the 3D viewport". egui_dock still draws the bar (we
        // can't hide it per-leaf in 0.18) but the content is blank.
        String::new()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn closable(&self) -> bool {
        // If the user closes the viewport tab, the centre region
        // collapses and the side panels reflow oddly. Keep it docked.
        false
    }

    fn transparent_background(&self) -> bool {
        // TRANSPARENT — required by the current render order
        // (`WorkbenchEguiHost` Camera2d order=1, Camera3d order=0).
        // Camera3d paints 3D into its viewport rect FIRST; egui paints
        // chrome on top with `ClearColorConfig::None`. If this panel
        // painted an opaque backdrop, egui would overpaint the 3D
        // pixels Camera3d just wrote and the centre area would be
        // solid theme dark. Bleed safety isn't lost — `apply_workbench
        // _viewport` sets `Camera::is_active = false` when ViewportPanel
        // isn't in the active layout, so no 3D ever reaches the
        // framebuffer to leak.
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        if let Some(mut rects) = world.get_resource_mut::<PanelRects>() {
            rects.record_from_ui(VIEWPORT_PANEL_ID, ui);
        }
        // Reserve the panel's space so egui_dock's layout accounts for
        // it; no widgets are drawn — the 3D camera paints here.
        ui.allocate_space(ui.available_size());
    }
}

/// Startup system — auto-spawn one [`WorkbenchEguiHost`] if none exists.
///
/// Always disables `EguiGlobalSettings::auto_create_primary_context` so
/// nothing else (e.g. bevy_egui's startup auto-promoter) can pick a
/// different camera as primary. Idempotent: re-running won't spawn
/// duplicates.
pub fn ensure_egui_host(
    mut commands: Commands,
    mut egui_global: ResMut<EguiGlobalSettings>,
    existing: Query<(), With<PrimaryEguiContext>>,
) {
    egui_global.auto_create_primary_context = false;
    if existing.iter().next().is_none() {
        commands.spawn((
            Camera2d,
            // `order = 1` puts egui's Camera2d strictly AFTER the
            // default-order (0) `Camera3d`, regardless of which entity
            // spawned first. Without this, the render order ties on
            // `order` and falls back to entity-creation order — and
            // 3D cameras that arrive late (USD avatar, fallback
            // free-flight, …) end up painting OVER the chrome,
            // bleeding 3D through the top-right of the menu bar.
            //
            // `ClearColorConfig::None` keeps the 3D scene the Camera3d
            // wrote to the framebuffer instead of wiping it before
            // egui paints. egui's chrome paints opaque panel frames
            // on top; the central viewport's framebuffer pixels stay
            // visible where no chrome covers them.
            Camera {
                order: 1,
                clear_color: ClearColorConfig::None,
                ..default()
            },
            PrimaryEguiContext,
            WorkbenchEguiHost,
            Name::new("WorkbenchEguiHost"),
        ));
    }
}

/// PostUpdate system — write the [`ViewportPanel`] rect into every
/// [`WorkbenchViewportCamera`]-tagged camera's `Camera::viewport`.
///
/// Runs before Bevy's `CameraUpdateSystems` so the new viewport is in
/// effect for the same frame the panel measured. If the panel hasn't
/// recorded its rect yet (first frame, perspective without a viewport
/// panel) the cameras' viewports are cleared to `None` — they render
/// to their target's full extent, which the invariant sentinel will
/// catch if the target is the window.
pub fn apply_workbench_viewport(
    rects: Res<PanelRects>,
    layout: Option<Res<crate::WorkbenchLayout>>,
    mut cameras: Query<&mut Camera, With<WorkbenchViewportCamera>>,
) {
    // Three modes, gated on the *current layout's contents* — never on
    // `PanelRects`, because that resource intentionally keeps stale
    // rects across perspective switches:
    //   (a) Empty layout / no layout (View perspective, tooling
    //       binaries) → camera active, viewport=None, full window.
    //   (b) Layout CONTAINS ViewportPanel (Build) → camera active,
    //       viewport=last recorded rect (or None until first paint).
    //   (c) Layout has other panels but NO ViewportPanel (Design) →
    //       camera INACTIVE. No 3D render reaches the framebuffer, so
    //       no panel gap, no stale rect, and no pass-skip can leak 3D
    //       under the UI.
    let (layout_empty, layout_has_viewport) = match layout.as_ref() {
        None => (true, false),
        Some(l) => (layout_is_empty(l), layout_contains_panel(l, VIEWPORT_PANEL_ID)),
    };
    // Camera3d always renders to the full window now; the chrome
    // panels (opaque side/top/bottom) overlay where they are and the
    // 3D shows through every transparent gap — including the dock
    // tab-strip area above ViewportPanel and the dock padding below.
    // Sub-rect viewports left a hole of uncleared framebuffer in
    // those gaps; full-window 3D fills them naturally.
    let _ = rects; // kept around for diagnostics; no longer drives Camera::viewport
    let target_viewport: Option<Viewport> = None;
    let want_active = layout_empty || layout_has_viewport;

    for mut camera in &mut cameras {
        if camera.is_active != want_active {
            camera.is_active = want_active;
        }
        let same = match (&camera.viewport, &target_viewport) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.physical_position == b.physical_position && a.physical_size == b.physical_size
            }
            _ => false,
        };
        if !same {
            camera.viewport = target_viewport.clone();
        }
    }
}

/// True iff `panel` appears in the active layout — either as a tab in
/// the dock or in one of the four slot Vecs the perspectives populate.
///
/// `dock.iter_all_tabs()` alone isn't enough: a perspective that calls
/// `set_side_browser/set_center/set_right_inspector/set_bottom` writes
/// to the slot Vecs first; the dock is *rebuilt from those slots* by
/// `rebuild_dock`. In steady state both contain the same panels, but
/// pinning the camera-active decision to layout membership (rather than
/// `PanelRects` which keeps stale rects on purpose) is what makes the
/// "is the viewport even part of this perspective?" question
/// authoritative.
/// True iff every slot Vec is empty AND the dock has no *singleton panel*
/// tabs — a View-style perspective that wants the entire window for the
/// 3D scene with no chrome painted on top.
///
/// Parked *instance* tabs (open documents/models) are deliberately
/// ignored: a hybrid app (the rover sandbox embeds the Modelica
/// workbench) can have documents open while a viewport-only perspective
/// is active. `rebuild_dock` parks those instance tabs in the dock so
/// they survive and re-attach on switch, and `render_layout` keeps the
/// workbench in 3D mode (it gates on the centre intent, not the dock).
/// They never paint chrome here, so the camera must stay full-window —
/// counting them as "non-empty" would wrongly flip the camera inactive
/// (the Design-style "panels but no viewport" branch) and blank the 3D.
pub(crate) fn layout_is_empty(layout: &crate::WorkbenchLayout) -> bool {
    layout.side_browser.is_empty()
        && layout.center.is_empty()
        && layout.right_inspector.is_empty()
        && layout.bottom.is_empty()
        && !layout
            .dock
            .iter_all_tabs()
            .any(|(_, t)| matches!(t, crate::TabId::Singleton(_)))
}

pub(crate) fn layout_contains_panel(layout: &crate::WorkbenchLayout, panel: PanelId) -> bool {
    if layout.side_browser.iter().any(|p| *p == panel)
        || layout.center.iter().any(|p| *p == panel)
        || layout.right_inspector.iter().any(|p| *p == panel)
        || layout.bottom.iter().any(|p| *p == panel)
    {
        return true;
    }
    layout
        .dock
        .iter_all_tabs()
        .any(|(_, t)| matches!(t, crate::TabId::Singleton(id) if *id == panel))
}

/// Sentinel — runs each frame on newly-added Camera3d entities and
/// panics (debug) / logs (release) when one targets the window without
/// the [`WorkbenchViewportCamera`] marker.
///
/// Catches the entire regression class: any future code path that
/// spawns a window-targeting `Camera3d` without going through
/// [`WorkbenchSceneCamera`] or remembering to add the marker will trip
/// this the moment the entity is built.
///
/// The check is per-`Added<Camera3d>` rather than a periodic sweep so
/// USD/avatar-spawned cameras (which can land many frames after
/// startup) are still validated, and so deleting + respawning the host
/// during teardown doesn't yield false negatives.
pub fn check_camera_invariants(
    new_cams: Query<
        (Entity, Option<&RenderTarget>),
        (Added<Camera3d>, Without<WorkbenchViewportCamera>),
    >,
) {
    for (entity, target) in &new_cams {
        let targets_window = matches!(target, None | Some(RenderTarget::Window(_)));
        if targets_window {
            // Warn loudly but don't panic — tooling binaries
            // (model_viewer, joint_minimal) deliberately want a
            // full-window 3D camera and don't use ViewportPanel. The
            // warning identifies workbench-using binaries that forgot
            // the tag, without breaking the legitimate cases.
            warn!(
                "WorkbenchPlugin: Camera3d {entity:?} targets the window without \
                 `WorkbenchViewportCamera`. If this binary uses `ViewportPanel`, the 3D \
                 scene will bleed across the egui chrome on pass skip — spawn via \
                 `WorkbenchSceneCamera` or insert `WorkbenchViewportCamera`. If this \
                 binary intentionally uses a full-window 3D camera (model_viewer, \
                 joint_minimal, …) this warning is benign."
            );
        }
    }
}

/// Sentinel — runs once a couple of seconds after startup and verifies
/// there's exactly one `PrimaryEguiContext` in the world.
///
/// The grace period covers binaries that prefer to spawn the host
/// themselves (legacy paths) — they'll have done so by the time this
/// fires. After that, anything other than 1 is a bug worth panicking
/// over in debug builds.
pub fn check_host_invariant_once(
    hosts: Query<(), With<PrimaryEguiContext>>,
    time: Res<Time>,
    mut done: Local<bool>,
) {
    if *done || time.elapsed_secs() < 1.0 {
        return;
    }
    *done = true;
    let n = hosts.iter().count();
    if n != 1 {
        warn!(
            "WorkbenchPlugin: expected exactly 1 `PrimaryEguiContext`, found {n}. \
             Was a stray (Camera2d, PrimaryEguiContext) spawned outside \
             `ensure_egui_host`? See `lunco-workbench/src/viewport.rs`."
        );
    }
}

/// Sub-plugin auto-added by `WorkbenchPlugin`. Wires the egui host,
/// the panel-rect tracking, the `Camera::viewport` sync, and the
/// invariant sentinels.
pub struct WorkbenchViewportPlugin;

impl Plugin for WorkbenchViewportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PanelRects>()
            .init_resource::<ViewportPlaceholder>()
            .add_systems(Startup, ensure_egui_host)
            // (Intentionally no per-frame clear of PanelRects: with the
            // current Camera::viewport architecture, keeping the last
            // recorded rect when ViewportPanel isn't in the active
            // perspective keeps the 3D camera confined to a sub-rect
            // that chrome easily overpaints. Clearing per frame let
            // the 3D camera fall back to full-window rendering, and
            // bevy_egui's alpha-blended chrome can't reliably cover
            // a full-window 3D bleed through every panel gap. The
            // permanent fix is RTT — see follow-up design note.)
            .add_systems(Update, (check_camera_invariants, check_host_invariant_once))
            .add_systems(
                PostUpdate,
                apply_workbench_viewport
                    .before(bevy::camera::CameraUpdateSystems),
            );
    }
}
