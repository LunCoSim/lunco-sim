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
use bevy::camera::{ClearColorConfig, RenderTarget};
use bevy_egui::{egui, EguiGlobalSettings, PrimaryEguiContext};

use crate::{Panel, PanelCtx, PanelId, PanelSlot};

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
    /// egui-authoritative "the pointer is over the live 3D scene this frame".
    ///
    /// OR-folded from each scene-hosting panel's [`scene_pointer_from_ui`] during
    /// render — egui's own occlusion-aware hit test, NOT a point-in-rect
    /// reconstruction — and reset to `false` at the top of `render_workbench`
    /// each frame (see [`reset_pointer_over_scene`](Self::reset_pointer_over_scene)).
    /// This is the single scene-vs-chrome signal for the picking backend
    /// ([`egui_viewport_aware_picking`]) and [`track_egui_focus`]. It supersedes
    /// the old geometric `any_contains(cursor)` test, which reconstructed the rect
    /// in physical pixels (floor/ceil at DPR), couldn't see egui occlusion, and
    /// excluded the leaf tab-bar strip — the source of the dropped-edge-click and
    /// fractional-DPR corner cases.
    ///
    /// NOTE: during render this holds the RAW fold (pointer over a scene panel's
    /// leaf); [`resolve_scene_pointer`] then overwrites it in PostUpdate with the
    /// mode-resolved value (folding in egui's `is_pointer_over_egui`). Consumers
    /// read the resolved value.
    pointer_over_scene: bool,
    /// Per docked *chrome* panel this frame: `(body, card)` in egui points, where
    /// `body` is the leaf content area the panel was given and `card` is the rect
    /// it actually painted (`ui.min_rect()`). Chrome panels are transparent leaves
    /// that paint a content-sized card, so `body − card` is a transparent gap where
    /// the full-window 3D shows through and should be clickable. Recorded by the
    /// dock dispatch for every non-`is_scene_viewport` panel (so it works in EVERY
    /// docked perspective); reset each frame. Consumed by [`resolve_scene_pointer`].
    chrome_cards: Vec<(egui::Rect, egui::Rect)>,
    /// The DockArea's own rect this frame (egui points), if a dock rendered. The
    /// dock does NOT always fill the window — anything OUTSIDE it is bare
    /// full-window 3D (no egui), which must read as scene, not chrome. The
    /// chrome blanket is bounded to this rect. `None` in full-window/View mode.
    dock_rect: Option<egui::Rect>,
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
        let rect = Self::panel_rect_from_ui(ui);
        self.record(panel, rect);
    }

    /// Compute a [`PanelRect`] (physical pixels) from an egui `Ui`,
    /// without touching the world. Lets a panel measure its rect during
    /// the read-only paint and stash it via [`record`](Self::record)
    /// inside a `PanelCtx::defer` closure (no `&mut World` in render).
    /// Uses the same floor-origin / ceil-far-edge rounding as the old
    /// inline `record_from_ui` body.
    pub fn panel_rect_from_ui(ui: &egui::Ui) -> PanelRect {
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
        PanelRect { origin, size }
    }

    /// Record a precomputed panel rect. Pairs with
    /// [`panel_rect_from_ui`](Self::panel_rect_from_ui) for the
    /// measure-then-defer pattern.
    pub fn record(&mut self, panel: PanelId, rect: PanelRect) {
        self.rects.insert(panel, rect);
    }

    /// Look up a panel's most-recently-recorded rect.
    pub fn get(&self, panel: PanelId) -> Option<PanelRect> {
        self.rects.get(&panel).copied()
    }

    /// egui's occlusion-aware "is the pointer over this scene panel's content?"
    /// hit test, measured during the panel's render. Uses `rect_contains_pointer`
    /// — egui's own layer-ordered test — so a floating egui window over the
    /// viewport reads as chrome, and there is no physical-pixel rounding. Each
    /// scene-hosting panel ([`ViewportPanel`] and the USD viewport) folds the
    /// result via [`record_scene_panel`](Self::record_scene_panel).
    pub fn scene_pointer_from_ui(ui: &egui::Ui) -> bool {
        ui.rect_contains_pointer(ui.available_rect_before_wrap())
    }

    /// Record that a scene-viewport panel rendered this frame, OR-folding whether
    /// the pointer is over its leaf (so multiple scene panels in one layout
    /// combine). Pair with [`reset_pointer_over_scene`](Self::reset_pointer_over_scene)
    /// at frame start.
    pub fn record_scene_panel(&mut self, over: bool) {
        self.pointer_over_scene |= over;
    }

    /// Overwrite the resolved "pointer over scene" — called only by
    /// [`resolve_scene_pointer`] after mode resolution.
    pub fn set_pointer_over_scene(&mut self, over: bool) {
        self.pointer_over_scene = over;
    }

    /// Record a docked chrome panel's `(body, card)` this frame (egui points).
    /// `body` = the leaf content area it was given; `card` = what it painted
    /// (`ui.min_rect()`). Only for non-`is_scene_viewport` panels.
    pub fn record_chrome_panel(&mut self, body: egui::Rect, card: egui::Rect) {
        self.chrome_cards.push((body, card));
    }

    /// Record the DockArea's rect this frame (egui points). Called by
    /// `render_layout` after `DockArea::show_inside`.
    pub fn set_dock_rect(&mut self, rect: egui::Rect) {
        self.dock_rect = Some(rect);
    }

    /// The DockArea rect this frame, or `None` in full-window/View mode.
    pub fn dock_rect(&self) -> Option<egui::Rect> {
        self.dock_rect
    }

    /// True if `pos` (egui points) is over some chrome panel's painted card.
    pub fn pointer_over_card(&self, pos: egui::Pos2) -> bool {
        self.chrome_cards.iter().any(|(_, card)| card.contains(pos))
    }

    /// True if `pos` is inside a chrome panel's body but OUTSIDE its painted card
    /// — a transparent gap where the full-window 3D shows through (clickable
    /// scene). Tab bars / separators are outside every panel body, so they are
    /// NOT gaps and stay chrome.
    pub fn pointer_in_transparent_gap(&self, pos: egui::Pos2) -> bool {
        self.chrome_cards
            .iter()
            .any(|(body, card)| body.contains(pos) && !card.contains(pos))
    }

    /// Clear the per-frame scene-pointer signals. Called once at the top of
    /// `render_workbench`, before any panel renders, so a frame where no scene
    /// panel is in the layout (or the pointer left the viewport) reads `false`.
    /// The rect map is deliberately NOT cleared here — it persists for camera
    /// confinement; only these per-frame bools reset.
    pub fn reset_pointer_over_scene(&mut self) {
        self.pointer_over_scene = false;
        self.chrome_cards.clear();
        self.dock_rect = None;
    }

    /// True when the pointer is over the live 3D scene (not chrome, not occluded)
    /// this frame — the authoritative scene-vs-chrome gate. See
    /// [`pointer_over_scene`](Self::pointer_over_scene) (the field) for how it's
    /// produced.
    pub fn pointer_over_scene(&self) -> bool {
        self.pointer_over_scene
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

    fn is_scene_viewport(&self) -> bool {
        // This IS the scene — exempt from the pick gate's chrome-card recording.
        true
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

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        // Record the live viewport rect so `apply_workbench_viewport` can confine
        // the 3D camera to it in DockArea mode. Scene-vs-chrome picking is handled
        // by bevy_picking (egui occlusion via bevy_egui's picking backend), so
        // there's no pointer gate to compute here anymore.
        //
        // Measure the rect now (needs `ui`), then write it into `PanelRects`
        // after the paint via `defer` — render has no `&mut World`.
        let rect = PanelRects::panel_rect_from_ui(ui);
        // Authoritative scene-vs-chrome signal — egui's own occlusion-aware hit
        // test, measured now (needs `ui`), folded after the paint.
        let over_scene = PanelRects::scene_pointer_from_ui(ui);
        ctx.defer(move |world| {
            if let Some(mut rects) = world.get_resource_mut::<PanelRects>() {
                rects.record(VIEWPORT_PANEL_ID, rect);
                rects.record_scene_panel(over_scene);
            }
        });
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

/// Push the workbench's layout state — is the 3D scene shown, and (future) in
/// what rect — into [`SceneViewport`](lunco_core::SceneViewport). The
/// viewport-camera reconciler in `lunco-usd-bevy` actuates it onto the actual
/// cameras; this system deliberately does NOT touch `Camera::is_active` so the
/// workbench and the camera switch stop fighting over it.
pub fn apply_workbench_viewport(
    layout: Option<Res<crate::WorkbenchLayout>>,
    vp: Option<ResMut<lunco_core::SceneViewport>>,
) {
    // The workbench contributes VISIBILITY (and a future rect) to the scene
    // viewport; it never writes `Camera::is_active` — the viewport-camera
    // reconciler in `lunco-usd-bevy` is the single authority. Gated on the
    // *current layout's contents* (never on stale `PanelRects`):
    //   (a) Empty layout / no layout (View perspective, tooling) → visible.
    //   (b) Layout CONTAINS ViewportPanel (Build)                 → visible.
    //   (c) Other panels but NO ViewportPanel (Design)            → hidden, so
    //       no 3D reaches the framebuffer and no pass-skip leaks it under the UI.
    let (layout_empty, layout_has_viewport) = match layout.as_ref() {
        None => (true, false),
        Some(l) => (layout_is_empty(l), layout_contains_panel(l, VIEWPORT_PANEL_ID)),
    };
    let Some(mut vp) = vp else { return };
    // 3D renders full-window: the chrome panels (opaque side/top/bottom)
    // overlay it and the scene shows through every transparent gap. `rect =
    // None` = full window; a future sub-rect would derive it from the
    // ViewportPanel's recorded rect.
    vp.visible = layout_empty || layout_has_viewport;
    vp.rect = None;
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

/// Opt-in system — tag freshly-added window-targeting `Camera3d`
/// entities with [`WorkbenchViewportCamera`] so [`apply_workbench_viewport`]
/// manages their `is_active`/viewport and they stop tripping
/// [`check_camera_invariants`].
///
/// This is **opt-in** rather than part of [`WorkbenchViewportPlugin`]:
/// tooling binaries (`model_viewer`, `joint_minimal`) deliberately want a
/// bare full-window 3D camera and never register a [`ViewportPanel`], so
/// auto-tagging is the host app's choice. Workbench apps that DO show the
/// 3D scene inside a [`ViewportPanel`] (sandbox, luncosim) add this in
/// `Update` so avatar-/USD-spawned cameras — which land async, long after
/// `Startup` — get the marker the moment they appear.
///
/// RTT cameras (`RenderTarget::Image`) are skipped: they paint into their
/// own offscreen Image (USD preview, vello diagrams) and must not have a
/// window-scoped viewport written to them.
pub fn auto_tag_workbench_3d_cameras(
    mut commands: Commands,
    new_cams: Query<
        (Entity, Option<&RenderTarget>),
        (Added<Camera3d>, Without<WorkbenchViewportCamera>),
    >,
) {
    for (entity, target) in &new_cams {
        let targets_window = matches!(target, None | Some(RenderTarget::Window(_)));
        if targets_window {
            commands.entity(entity).insert(WorkbenchViewportCamera);
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

/// Turn OFF bevy_egui's built-in pointer-capture backend on the primary egui
/// context. Its `capture_pointer_input_system` emits a top-priority
/// `bevy_picking` hit over the WHOLE egui context viewport whenever egui
/// `wants_pointer_input()` — and in egui-dock "Build" mode the central
/// `ViewportPanel` leaf makes egui want the pointer over the 3D region, so that
/// blanket capture suppressed every scene pick (clicks never reached the 3D).
/// We replace it with [`egui_viewport_aware_picking`], which captures only over
/// real chrome and never over the live viewport rect. Idempotent; the change
/// guard keeps it from dirtying the component every frame.
pub fn disable_egui_pointer_capture(
    mut q: Query<&mut bevy_egui::EguiContextSettings, With<PrimaryEguiContext>>,
) {
    for mut s in q.iter_mut() {
        if s.capture_pointer_input {
            s.capture_pointer_input = false;
        }
    }
}

/// Collapse egui's own geometry + the per-frame dock/panel rects into the single
/// resolved `pointer_over_scene` signal that the pick gate ([`track_egui_focus`] →
/// `EguiFocus.wants_pointer`) and [`egui_viewport_aware_picking`] read. Runs in
/// PostUpdate after the egui pass, ordered BEFORE both consumers.
///
/// The pointer is over CHROME (scene picks stand down) when any of:
///  • `is_pointer_over_egui()` — over a reserved egui panel / menu / status bar /
///    floating window / popup. This is the authoritative signal for full-window
///    ("View") mode and for any real (non-dock-leaf) egui surface, in EVERY
///    perspective. Crucially it is pure geometry + occlusion (`layer_id_at`), NOT
///    masked by mouse-button-down — unlike `egui_wants_pointer_input()`, whose
///    `&& !any_down()` clause returned false at the exact frame of a press over a
///    panel background/label (the original chrome-click leak). **This is the fix
///    that matters and is confirmed working.**
///  • `pointer_over_card()` — over a docked chrome panel's painted card. egui_dock
///    leaves all share the Background layer, so `is_pointer_over_egui` can't see
///    them; the per-panel card rect (recorded in `tab_ui`) does.
///  • inside the dock rect, off the transparent viewport leaf, and not in an
///    in-leaf transparent gap (keeps tab bars / separators blocked).
///
/// TODO(pick-gate): the "click the bare 3D BELOW the whole dock" case does NOT work
/// yet. Intent: the dock often does not fill the window, so anything OUTSIDE
/// `dock_rect` is full-window 3D and should be scene. But `dock_rect` is taken from
/// `viewport_ui.min_rect()` after `DockArea::show_inside` (see `render_layout`), and
/// that appears to report the full available area rather than the dock's true drawn
/// extent — so `in_dock` stays true below the dock and the blanket keeps blocking
/// it. Fix by measuring the dock's real bounds (e.g. the union of the recorded leaf
/// body rects, or an egui_dock API for the laid-out tree rect) instead of
/// `min_rect()`. The in-leaf gap path (`in_gap`) and the card/`is_pointer_over_egui`
/// chrome paths are correct; only the below-dock region is unresolved.
pub fn resolve_scene_pointer(
    mut panel_rects: ResMut<PanelRects>,
    mut q: Query<&mut bevy_egui::EguiContext, With<PrimaryEguiContext>>,
) {
    let mut over_egui = false;
    let mut pointer: Option<egui::Pos2> = None;
    for mut ctx in q.iter_mut() {
        let c = ctx.get_mut();
        over_egui |= c.is_pointer_over_egui();
        pointer = pointer.or_else(|| c.pointer_interact_pos());
    }
    let on_leaf = panel_rects.pointer_over_scene(); // raw fold from the viewport leaf
    // Over a chrome panel's painted card, in an in-leaf transparent gap, and/or
    // inside the dock rect at all? (See the TODO above re: the dock_rect extent.)
    let (over_card, in_gap, in_dock) = match pointer {
        Some(p) => (
            panel_rects.pointer_over_card(p),
            panel_rects.pointer_in_transparent_gap(p),
            panel_rects.dock_rect().is_some_and(|r| r.contains(p)),
        ),
        None => (false, false, false),
    };
    let over_chrome = over_egui || over_card || (in_dock && !on_leaf && !in_gap);
    panel_rects.set_pointer_over_scene(!over_chrome);
}

/// Scene-vs-chrome-aware egui picking backend — the replacement for bevy_egui's
/// blanket capture (disabled by [`disable_egui_pointer_capture`]).
///
/// Emits a high-order `bevy_picking` hit for the egui context entity ONLY when the
/// pointer is over chrome, i.e. NOT over the scene. It reads the single resolved
/// `PanelRects::pointer_over_scene()` computed by [`resolve_scene_pointer`] (which
/// runs just before this), so:
///   • over chrome → egui hit wins → the 3D pick is suppressed;
///   • over the scene (viewport leaf, full-window View centre) → no egui hit →
///     bevy_picking's mesh hit reaches the scene observers.
/// Mirrors bevy_egui's own `capture_pointer_input_system` (PostUpdate, hit with no
/// world position so consumers can tell chrome from a real mesh pick). Unlike the
/// stock capture it does NOT gate on `egui_wants_pointer_input()` (button-masked —
/// see [`resolve_scene_pointer`]); the resolved geometric signal is unconditional.
pub fn egui_viewport_aware_picking(
    pointers: Query<(
        &bevy::picking::pointer::PointerId,
        &bevy::picking::pointer::PointerLocation,
    )>,
    egui_q: Query<(Entity, &Camera), With<PrimaryEguiContext>>,
    panel_rects: Res<PanelRects>,
    picking_order: Option<Res<bevy_egui::EguiPickingOrder>>,
    mut out: MessageWriter<bevy::picking::backend::PointerHits>,
) {
    use bevy::camera::NormalizedRenderTarget;
    use bevy::picking::backend::{HitData, PointerHits};

    let extra = picking_order.map(|o| o.0).unwrap_or(0.6);
    for (id, loc) in pointers
        .iter()
        .filter_map(|(i, p)| p.location.as_ref().map(|l| (i, l)))
    {
        let NormalizedRenderTarget::Window(_) = loc.target else {
            continue;
        };
        for (entity, camera) in egui_q.iter() {
            // Pointer must be inside the egui camera's viewport (full window).
            let Some(vp) = camera.physical_viewport_rect() else {
                continue;
            };
            if !vp.as_rect().contains(loc.position) {
                continue;
            }
            // `resolve_scene_pointer` (runs just before this) has already collapsed
            // scene-vs-chrome into `pointer_over_scene`. Over the scene → let the
            // mesh pick win (no capture). Over chrome → emit a high-order capture
            // hit UNCONDITIONALLY. We must NOT gate on `egui_wants_pointer_input()`
            // here: its `&& !any_down()` clause returns false at the instant of a
            // press over a panel background/label, which let the gizmo/hover pick
            // leak through chrome on click. `pointer_over_scene` is the pure
            // geometric+occlusion signal, unaffected by button state.
            if panel_rects.pointer_over_scene() {
                continue;
            }
            out.write(PointerHits::new(
                *id,
                vec![(entity, HitData::new(entity, 0.0, None, None))],
                camera.order as f32 + extra,
            ));
        }
    }
}

/// Relay egui's input-capture flags into the ECS as [`lunco_core::EguiFocus`].
///
/// egui reads its own copy of the winit events and never removes anything from
/// Bevy's `ButtonInput`, so raw scene-input systems (keyboard driving, camera
/// orbit, scroll-zoom) would otherwise fire even while an egui text field is
/// focused or the pointer is over a panel. This publishes `wants_keyboard` (from
/// egui's `wants_keyboard_input()`) and `wants_pointer` (the negation of the
/// resolved `PanelRects::pointer_over_scene()` — see [`resolve_scene_pointer`],
/// NOT the button-masked `egui_wants_pointer_input()`) so those systems can gate
/// on `EguiFocus` without depending on `bevy_egui`.
///
/// Runs in `PostUpdate` after `EguiPostUpdateSet::ProcessOutput` (same slot as
/// the picking backend) so the flags are this-frame-fresh; consumers in `Update`
/// read them one frame later, which is imperceptible for held input.
pub fn track_egui_focus(
    mut focus: ResMut<lunco_core::EguiFocus>,
    mut q: Query<&mut bevy_egui::EguiContext, With<PrimaryEguiContext>>,
    panel_rects: Res<PanelRects>,
) {
    // Pointer gate: egui owns the pointer whenever it is NOT over the live 3D
    // scene. `pointer_over_scene` has already been resolved by
    // `resolve_scene_pointer` (this-frame, mode-aware, occlusion-aware). We do NOT
    // use `egui_wants_pointer_input()` here: its `&& !any_down()` clause drops to
    // false at the instant of a press over a panel background/label, which leaked
    // scene picks / camera-orbit through chrome on click (and made wheel-zoom only
    // work with a button held). The resolved geometric signal has no such quirk.
    let ptr = !panel_rects.pointer_over_scene();

    // Keyboard still comes straight from egui focus (a focused text field).
    let mut kb = false;
    for mut ctx in q.iter_mut() {
        kb |= ctx.get_mut().egui_wants_keyboard_input();
    }

    // Change-guarded so the resource isn't dirtied every frame.
    if focus.wants_keyboard != kb || focus.wants_pointer != ptr {
        focus.wants_keyboard = kb;
        focus.wants_pointer = ptr;
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
            .init_resource::<lunco_core::EguiFocus>()
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
            // Keep bevy_egui's blanket pointer-capture OFF; we provide our own
            // viewport-aware backend instead (so the egui-dock ViewportPanel leaf
            // doesn't suppress 3D picking over the scene).
            .add_systems(Update, disable_egui_pointer_capture)
            // Resolve the per-frame scene-vs-chrome signal from the render fold +
            // egui geometry. PostUpdate after the egui pass, BEFORE the two
            // consumers below (which read the resolved `pointer_over_scene`).
            .add_systems(
                PostUpdate,
                resolve_scene_pointer
                    .after(bevy_egui::EguiPostUpdateSet::ProcessOutput)
                    .before(egui_viewport_aware_picking)
                    .before(track_egui_focus),
            )
            // Custom egui picking backend. PostUpdate after the egui pass so the
            // resolved scene-vs-chrome signal is this-frame fresh (mirrors
            // bevy_egui's own capture system's schedule).
            .add_systems(
                PostUpdate,
                egui_viewport_aware_picking
                    .after(bevy_egui::EguiPostUpdateSet::ProcessOutput),
            )
            // Publish the pointer/keyboard gate into `EguiFocus` (same post-egui
            // slot) so raw scene-input systems can gate on it. See `track_egui_focus`.
            .add_systems(
                PostUpdate,
                track_egui_focus.after(bevy_egui::EguiPostUpdateSet::ProcessOutput),
            )
            .add_systems(
                PostUpdate,
                apply_workbench_viewport
                    .before(bevy::camera::CameraUpdateSystems),
            );
    }
}
