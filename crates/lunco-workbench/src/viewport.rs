//! `ViewportPanel` and the workbench's 3D-viewport plumbing.
//!
//! ## Architecture (read this if anything here looks weird)
//!
//! Two cameras share the window, and they are **layered**, not tiled:
//!
//! 1. The scene `Camera3d` (order 0), tagged [`WorkbenchViewportCamera`] (or
//!    spawned as [`WorkbenchSceneCamera`]). It renders the 3D **full-window** —
//!    [`apply_workbench_viewport`] publishes visibility into
//!    `lunco_core::SceneViewport` with `rect = None`, and the single-authority
//!    reconciler in `lunco-usd-bevy` actuates it. It **clears** the window's
//!    main texture each frame.
//! 2. The egui host `Camera2d` (order 1) — [`WorkbenchEguiHost`], carrying
//!    `PrimaryEguiContext`, auto-spawned by [`ensure_egui_host`]. bevy_egui
//!    paints the chrome into *this camera's* `ViewTarget`, on top, with
//!    `ClearColorConfig::None` so it does not wipe the 3D the `Camera3d` just
//!    wrote. Chrome is opaque where panels are; everywhere else the 3D shows
//!    through.
//!
//! The host exists as its own camera because scene cameras are transient — USD
//! scenes spawn them, `camera_switch` swaps them, avatars despawn them — while
//! the egui context must be stable for the life of the app.
//!
//! ## The framebuffer contract (the load-bearing invariant)
//!
//! **The two cameras must share ONE main texture.** Bevy keys a target's main
//! textures by `(target, usages, format, msaa)` — `MainTextureKey` in
//! `bevy_render::view::prepare_view_targets`. Same key ⇒ one shared texture:
//! the `Camera3d` clears it, egui paints over it, correct. Different key ⇒ the
//! host silently gets a *private* texture out of the `TextureCache`, and
//! because its clear config is `None` (`LoadOp::Load` forever — see
//! `ColorAttachment::get_attachment`) **nothing ever clears it**. It degenerates
//! into an accumulation buffer: chrome that stops being drawn (a panel dropped
//! by a perspective switch, a status bar orphaned by a resize) stays baked in
//! and keeps compositing over the live 3D, frozen at the frame it was painted.
//! Only a window resize clears it, by changing the texture descriptor.
//!
//! That is a real bug this crate shipped: `SceneCamera` defaults to MSAA ×2
//! while a bare `Camera2d` defaults to ×4, so the keys diverged and Build's
//! panels ghosted over the View perspective. [`sync_egui_host_msaa`] is what
//! holds the invariant — it is correctness plumbing, not image quality (egui
//! gains nothing from MSAA). If you ever give the host its own MSAA, format, or
//! HDR setting, you break this and the ghosts come back.
//!
//! The alternative architecture — an independent, alpha-composited egui layer
//! (`clear_color: Custom(NONE)` + `CameraOutputMode::Write { blend_state:
//! Some(PREMULTIPLIED_ALPHA_BLENDING), .. }`) — would decouple the two cameras
//! at the cost of an extra full-window texture and blend blit per frame. We
//! deliberately keep the shared-texture model: it is what `bevy_ui` itself does
//! for UI-over-3D, and it is cheaper.
//!
//! ## Why this is robust *by design*
//!
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
//! - **The texture-sharing key is synced, not assumed.**
//!   [`sync_egui_host_msaa`] re-establishes it whenever a scene camera's MSAA
//!   moves or a camera is newly tagged, so a change to the setting (it is
//!   user-facing, and `Off` on wasm) cannot quietly split the textures again.
//!
//! ## When NO 3D camera renders the window
//!
//! Design-style perspectives and the Modelica workbench have no active window
//! `Camera3d`, so nothing clears the target. `render_layout` covers exactly
//! that case by painting a full-window backdrop on egui's background layer
//! (`needs_full_backdrop`). That is why chrome-only apps are not affected by
//! the invariant above.
//!
//! ## What goes where
//!
//! - `ensure_egui_host` (Startup) — auto-spawn the egui host.
//! - `sync_egui_host_msaa` (Update) — hold the shared-main-texture invariant.
//! - `apply_workbench_viewport` (PostUpdate, before `CameraUpdateSystems`)
//!   — publish viewport *visibility* into `SceneViewport` (the rect stays
//!   `None`: the 3D renders full-window and the chrome overlays it).
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
use bevy::camera::{ClearColorConfig, Hdr, RenderTarget};
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

/// Which live 3D scene a pointer position belongs to.
///
/// The pick gate's answer to *"the pointer is over WHICH scene?"* — the question
/// that a single global `pointer_over_scene: bool` could not represent. Only
/// [`SceneTarget::MainViewport`] is the window-wide Bevy scene that the avatar
/// camera and `bevy_picking` drive; every other variant is a panel-owned scene
/// that renders to its own offscreen image and handles its own input, so the
/// main scene's consumers must treat it as chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneTarget {
    /// The full-window 3D scene: the transparent [`ViewportPanel`] leaf in dock
    /// mode, or the bare window centre in full-window ("View") mode.
    MainViewport,
    /// A panel that renders a scene to an offscreen image and shows it as an
    /// `egui::Image` (e.g. the USD preview). It owns its own drag/scroll input;
    /// the main scene must not also react to it.
    Offscreen(PanelId),
}

/// Per-panel screen-space rect, in *physical* pixels.
///
/// Populated by `Panel::render` implementations that want to be
/// camera-targetable ([`ViewportPanel`], the USD preview). Consumed by
/// `resize_viewport_image` in `lunco-usd`.
///
/// Cleared at the top of every egui pass (`render_workbench`) and refilled by the
/// panels that actually render, so a panel that left the layout (perspective
/// switch, closed tab) does NOT leak a stale rect to its consumers. Consumers run
/// in `Update` and therefore read the *previous* egui pass's rects — one frame of
/// lag, which is what the offscreen-image resize already tolerates.
///
/// This resource holds *only* persistent physical-pixel rects. The per-frame
/// pick-gate inputs live in [`ScenePickGate`], which has a different reset
/// lifetime and a different coordinate space (egui points).
#[derive(Resource, Default, Debug)]
pub struct PanelRects {
    rects: HashMap<PanelId, PanelRect>,
}

/// Per-frame inputs and resolved output of the scene-vs-chrome pick gate.
///
/// **Lifetime contract** (one reset, one writer, one resolver):
/// 1. [`reset_scene_pick_gate`] (`First`, runs unconditionally EVERY frame) clears
///    the per-frame inputs and lowers `rendered`.
/// 2. The egui pass (`render_workbench` → panels → dock dispatch) raises
///    `rendered` and records the inputs: the scene leaf under the pointer, each
///    chrome panel's `(body, card)`, and the dock's extent.
/// 3. [`resolve_scene_pointer`] (`PostUpdate`, after the egui pass) folds those
///    with egui's own geometry into `resolved`, latching ownership on press.
///    If the egui pass was skipped this frame (`rendered == false`) it holds the
///    previous answer rather than resolving against empty/stale inputs.
///
/// All rects here are **egui points** (not physical pixels — that's [`PanelRects`]).
#[derive(Resource, Default, Debug)]
pub struct ScenePickGate {
    /// Raised by `render_workbench`; lowered by [`reset_scene_pick_gate`]. False
    /// means "the egui pass did not run this frame" (window occluded/minimized,
    /// or a host that doesn't call `render_workbench`).
    rendered: bool,
    /// The scene-hosting leaf the pointer is inside this frame, if any — from
    /// egui's own occlusion-aware [`scene_pointer_from_ui`] hit test, NOT a
    /// point-in-rect reconstruction. Raw render-time input; never read by
    /// consumers.
    scene_leaf: Option<SceneTarget>,
    /// Per docked *chrome* panel this frame: `(body, card)`, where `body` is the
    /// leaf content area the panel was given and `card` is the region it actually
    /// blocks. For a transparent panel `card = ui.min_rect()` (what it painted),
    /// so `body − card` is a see-through gap the full-window 3D shows through and
    /// which must stay clickable. For an **opaque** panel (the default —
    /// egui-dock fills the whole leaf with `tab_body.bg_fill`) `card = body`:
    /// there is no gap, and the empty lower half of a short panel is still chrome.
    chrome_cards: Vec<(egui::Rect, egui::Rect)>,
    /// The DockArea's extent this frame, if a dock rendered — the rect handed to
    /// `DockArea::show_inside`, measured BEFORE it lays out (so it is the dock's
    /// true area, not the whole window). Anything outside it is bare full-window
    /// 3D and must read as scene. `None` in full-window/View mode.
    dock_rect: Option<egui::Rect>,
    /// The scene `ViewportPanel`'s dock LEAF rect this frame, read straight from
    /// egui_dock's post-layout tree (`LeafNode::rect`) — NOT from the panel's
    /// render. That distinction is the whole point: when the leaf is COLLAPSED or
    /// the viewport is a background tab, `ViewportPanel::render` (which records
    /// [`scene_leaf`](Self::scene_leaf)) doesn't run, yet the full-window 3D still
    /// shows in this rect and must stay clickable. `None` outside dock mode / when
    /// the viewport isn't in the tree.
    scene_viewport_rect: Option<egui::Rect>,
    /// Resolved scene under the pointer — the single source of truth read by
    /// [`egui_viewport_aware_picking`] and [`track_egui_focus`]. `None` = chrome.
    resolved: Option<SceneTarget>,
    /// True while a pointer button has been held since the press that produced
    /// `resolved`. Drives the press-latch (see [`PressLatch`]).
    latched: bool,
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
    /// Drop every recorded rect. Called at the top of each egui pass
    /// (`render_workbench`), before any panel renders; the panels in the active
    /// layout refill it as they paint. Without this a panel that left the layout
    /// (closed tab, perspective switch) would leak its last rect forever and keep
    /// driving its consumer — e.g. `lunco-usd`'s `resize_viewport_image` sizing
    /// the offscreen image to a panel that is no longer shown.
    pub fn clear(&mut self) {
        self.rects.clear();
    }

    /// Compute a [`PanelRect`] (physical pixels) from an egui `Ui`,
    /// without touching the world. Lets a panel measure its rect during
    /// the read-only paint and stash it via [`record`](Self::record)
    /// inside a `PanelCtx::defer` closure (no `&mut World` in render).
    ///
    /// Uses **floor on the origin** and **ceil on the far edge** so the
    /// physical-pixel rect fully covers the panel even at non-integer DPRs
    /// (1.5, 1.25, …); round-half-away-from-zero could leave a 1-px gap between
    /// the camera viewport and the panel edge.
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
}

/// The egui-geometry half of the gate's per-frame inputs — everything that has to
/// be read out of the egui `Context` in `PostUpdate`. Split out from
/// [`resolve_scene_pointer`] so the decision logic ([`resolve_scene_target`]) is a
/// pure function and unit-testable without a window.
#[derive(Debug, Clone, Copy, Default)]
pub struct EguiPointerState {
    /// `Context::is_pointer_over_egui()` — over a reserved panel / menu / status
    /// bar / floating window / popup. Pure geometry + occlusion (`layer_id_at`),
    /// NOT masked by button-down (unlike `egui_wants_pointer_input()`, whose
    /// `&& !any_down()` clause dropped to false on the exact frame of a press over
    /// a panel background — the original chrome-click leak).
    pub over_egui: bool,
    /// `Context::is_using_pointer()` — egui is actively dragging one of its own
    /// widgets (slider, dock separator, scrollbar). Must own the pointer for the
    /// whole drag no matter where the cursor travels.
    pub using_pointer: bool,
    /// `Context::pointer_hover_pos()` — `None` once the pointer leaves the window.
    /// Deliberately NOT `pointer_interact_pos()`, which egui keeps alive after
    /// `PointerGone` and would leave the gate resolving against a phantom cursor.
    pub hover_pos: Option<egui::Pos2>,
    /// Any pointer button currently held.
    pub any_down: bool,
}

impl ScenePickGate {
    /// egui's occlusion-aware "is the pointer over this scene panel's content?"
    /// hit test, measured during the panel's render. Uses `rect_contains_pointer`
    /// — egui's own layer-ordered test — so a floating egui window over the
    /// viewport reads as chrome, and there is no physical-pixel rounding.
    pub fn scene_pointer_from_ui(ui: &egui::Ui) -> bool {
        ui.rect_contains_pointer(ui.available_rect_before_wrap())
    }

    /// Record that the pointer is inside `target`'s scene leaf this frame. Called
    /// from each scene-hosting panel's render (via `PanelCtx::defer`). Leaves are
    /// disjoint, so at most one call per frame passes `true`.
    pub fn record_scene_leaf(&mut self, target: SceneTarget, over: bool) {
        if over {
            self.scene_leaf = Some(target);
        }
    }

    /// Record a docked chrome panel's blocked region this frame (egui points).
    /// `body` = the leaf content area it was given; `card` = the region it blocks
    /// (`ui.min_rect()` for a transparent panel, `body` for an opaque one — see
    /// [`chrome_cards`](Self::chrome_cards)).
    pub fn record_chrome_panel(&mut self, body: egui::Rect, card: egui::Rect) {
        self.chrome_cards.push((body, card));
    }

    /// Record the DockArea's extent this frame (egui points). Called by
    /// `render_layout` with the rect it hands to `DockArea::show_inside`.
    pub fn set_dock_rect(&mut self, rect: egui::Rect) {
        self.dock_rect = Some(rect);
    }

    /// Record the scene viewport leaf's rect (egui points) from egui_dock's
    /// post-layout tree. Called by `render_layout` after `show_inside`. See
    /// [`scene_viewport_rect`](Self::scene_viewport_rect).
    pub fn set_scene_viewport_rect(&mut self, rect: Option<egui::Rect>) {
        self.scene_viewport_rect = rect;
    }

    /// Mark that the egui pass ran this frame (so the gate's inputs are real).
    pub fn mark_rendered(&mut self) {
        self.rendered = true;
    }

    /// Clear the per-frame inputs. Runs unconditionally every frame in `First`.
    pub fn begin_frame(&mut self) {
        self.rendered = false;
        self.scene_leaf = None;
        self.chrome_cards.clear();
        self.dock_rect = None;
        self.scene_viewport_rect = None;
    }

    /// The scene the pointer is over right now, or `None` for chrome. The single
    /// source of truth; written only by [`resolve_scene_pointer`].
    pub fn resolved(&self) -> Option<SceneTarget> {
        self.resolved
    }

    /// True when the pointer owns the **main full-window 3D scene** — the gate for
    /// `bevy_picking` and the avatar camera. An offscreen preview panel resolves to
    /// its own [`SceneTarget`], so it reads `false` here and cannot double-drive
    /// the main camera.
    pub fn over_main_scene(&self) -> bool {
        self.resolved == Some(SceneTarget::MainViewport)
    }

    /// Fold this frame's inputs + egui's geometry into the resolved target,
    /// applying the press-latch. The whole decision, in one testable place.
    pub fn resolve(&mut self, egui_state: EguiPointerState) {
        if !self.rendered {
            // The egui pass was skipped this frame: the inputs below are empty,
            // not merely stale. Resolving against them would answer with garbage
            // (and the old code closed a feedback loop by reading its own previous
            // output back in as an input). Hold the last real answer instead.
            return;
        }
        let candidate = resolve_scene_target(
            egui_state,
            self.scene_leaf,
            &self.chrome_cards,
            self.dock_rect,
            self.scene_viewport_rect,
        );
        let latch = PressLatch {
            held: self.latched,
            owner: self.resolved,
        };
        let next = latch.update(egui_state.any_down, candidate);
        self.resolved = next.owner;
        self.latched = next.held;
    }
}

/// Press-latch state machine: whoever owns the pointer at the instant of the press
/// keeps it for the whole drag.
///
/// Without this, ownership is pure geometry re-evaluated every frame, so dragging a
/// slider over the viewport hands the pointer to the scene camera mid-drag, and
/// orbiting the scene into the inspector freezes the camera mid-orbit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PressLatch {
    /// A button has been held since the press that decided `owner`.
    pub held: bool,
    /// The owner decided at that press (or, when idle, the live geometric answer).
    pub owner: Option<SceneTarget>,
}

impl PressLatch {
    /// Advance one frame. `candidate` is the live geometric answer.
    ///
    /// - No button down → follow the geometry, drop the latch.
    /// - Button goes down this frame (`!held`) → adopt the geometry AT THE PRESS
    ///   and latch it.
    /// - Button still down (`held`) → hold the latched owner, ignore geometry.
    #[must_use]
    pub fn update(self, any_down: bool, candidate: Option<SceneTarget>) -> Self {
        match (any_down, self.held) {
            (false, _) => Self { held: false, owner: candidate },
            (true, false) => Self { held: true, owner: candidate },
            (true, true) => Self { held: true, owner: self.owner },
        }
    }
}

/// Pure scene-vs-chrome decision — no ECS, no egui `Context`, no window.
///
/// The pointer is over CHROME (`None`) when:
///  • egui is dragging one of its own widgets (`using_pointer`), or
///  • `is_pointer_over_egui()` — a reserved egui panel / menu / status bar /
///    floating window / popup, or
///  • the pointer has left the window (`hover_pos == None`), or
///  • it is over a docked chrome panel's blocked region (egui_dock leaves all share
///    the Background layer, so `is_pointer_over_egui` cannot see them — the
///    per-panel card rect recorded during the dock dispatch does), or
///  • it is inside the dock's extent but on no scene leaf and in no transparent gap
///    (tab bars, separators, dock padding).
///
/// Otherwise it is over a scene:
///  • the scene leaf it is actually inside (egui's own occlusion-aware hit test),
///  • a transparent gap in a chrome leaf — the full-window 3D shows through,
///  • or anywhere OUTSIDE the dock's extent: that is bare full-window 3D with no
///    egui over it at all. (This is the case the old `dock_rect =
///    viewport_ui.min_rect()` broke: `min_rect()` after `DockArea::show_inside` is
///    the whole window — menu bar and status bar are drawn into the same root Ui —
///    so `in_dock` was true everywhere and the chrome blanket swallowed every bare
///    3D click.)
pub fn resolve_scene_target(
    egui_state: EguiPointerState,
    scene_leaf: Option<SceneTarget>,
    chrome_cards: &[(egui::Rect, egui::Rect)],
    dock_rect: Option<egui::Rect>,
    scene_viewport_rect: Option<egui::Rect>,
) -> Option<SceneTarget> {
    if egui_state.using_pointer || egui_state.over_egui {
        return None;
    }
    let pos = egui_state.hover_pos?;
    // A scene-hosting leaf that RENDERED under the pointer wins: an offscreen
    // preview records BOTH its own target and an opaque chrome card (it must block
    // the MAIN scene), and we want the precise target, not just "not the main
    // scene". The main viewport records its target here too when its panel renders.
    if let Some(target) = scene_leaf {
        return Some(target);
    }
    let over_card = chrome_cards.iter().any(|(_, card)| card.contains(pos));
    if over_card {
        return None;
    }
    // The scene viewport leaf that did NOT render this frame — COLLAPSED, or the
    // viewport sitting behind another tab — so it recorded no `scene_leaf` above.
    // Its dock-layout `rect` still covers the full-window 3D showing there, so the
    // click must reach the scene. Checked AFTER the cards so a genuine opaque panel
    // still wins. This is what keeps a collapsed/folded/backgrounded viewport
    // clickable instead of dead.
    if scene_viewport_rect.is_some_and(|r| r.contains(pos)) {
        return Some(SceneTarget::MainViewport);
    }
    let in_gap = chrome_cards
        .iter()
        .any(|(body, card)| body.contains(pos) && !card.contains(pos));
    if in_gap {
        return Some(SceneTarget::MainViewport);
    }
    if dock_rect.is_some_and(|r| r.contains(pos)) {
        return None;
    }
    Some(SceneTarget::MainViewport)
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

    fn scene_target(&self) -> Option<SceneTarget> {
        // This IS the full-window scene — exempt from the pick gate's chrome-card
        // recording; the 3D camera paints through this transparent leaf.
        Some(SceneTarget::MainViewport)
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
        let over_scene = ScenePickGate::scene_pointer_from_ui(ui);
        ctx.defer(move |world| {
            if let Some(mut rects) = world.get_resource_mut::<PanelRects>() {
                rects.record(VIEWPORT_PANEL_ID, rect);
            }
            if let Some(mut gate) = world.get_resource_mut::<ScenePickGate>() {
                gate.record_scene_leaf(SceneTarget::MainViewport, over_scene);
            }
        });
        // Reserve the panel's space so egui_dock's layout accounts for
        // it; no widgets are drawn — the 3D camera paints here.
        ui.allocate_space(ui.available_size());
    }
}

/// Keep the egui host's MSAA equal to the scene camera's.
///
/// **This is what makes `ClearColorConfig::None` on the host sound**, and it is
/// load-bearing for correctness, not image quality — egui gains nothing from
/// MSAA.
///
/// Bevy keys a render target's main textures by
/// `(target, usages, format, msaa)` (`bevy_render::view::prepare_view_targets`,
/// `MainTextureKey`). Two window cameras with the SAME key share one main
/// texture; with different keys they each get their own out of the
/// `TextureCache`. bevy_egui draws the chrome into the *egui host camera's*
/// `ViewTarget` (`EguiViewTarget`), so:
///
/// - Same MSAA → host shares the 3D camera's texture. `Camera3d` (order 0)
///   clears it every frame, egui paints chrome on top: correct.
/// - Different MSAA → the host gets a PRIVATE texture that nothing clears,
///   because its own clear config is `None` (= `LoadOp::Load` forever, see
///   `ColorAttachment::get_attachment`). It silently becomes an accumulation
///   buffer: chrome that stops being drawn — panels dropped by a perspective
///   switch, a status bar left behind by a window resize — stays baked in it
///   and keeps compositing over the live 3D, frozen at the frame it was
///   painted. Only a resize clears it, by changing the texture descriptor.
///
/// That is exactly the bug this system prevents: the scene camera defaults to
/// MSAA ×2 (`SceneCamera`, and `Off` on wasm) while a bare `Camera2d` defaults
/// to ×4, so the keys diverged and the chrome ghosted.
///
/// **Change-driven.** MSAA practically never moves on its own, so the system
/// early-outs on quiet frames. The trigger set must cover EVERY way the pairing
/// (active window scene-camera → host) can change — the earlier set missed two, which
/// is how the ghost came back:
/// - `Changed<Msaa>` — the user flipped the setting (insertion counts as a change);
/// - `Added<WorkbenchViewportCamera>` — a camera newly became a scene camera
///   (`auto_tag_workbench_3d_cameras` tags USD/avatar cameras long after startup);
/// - `Changed<Camera>` — a DIFFERENT camera became active (`is_active` lives on
///   `Camera`), AND a window **resize** (bevy's camera driver rewrites `Camera.computed`
///   with the new target size). Both must re-pick/re-sync;
/// - **removal** (`RemovedComponents`) — a scene swap (Open-Twin) DESPAWNS the old
///   active camera and spawns the twin's. Without watching removal, the host stayed
///   synced to the despawned camera's MSAA; the new active camera then rendered into a
///   host texture keyed to the WRONG MSAA (a private, never-cleared one), and stale
///   chrome accumulated over the live 3D. This is the regression.
///
/// It is still change-driven: on a frame where none of these fired it does two empty
/// archetype checks and returns.
pub fn sync_egui_host_msaa(
    // `Without<WorkbenchEguiHost>` on the scene-camera queries, and
    // `Without<WorkbenchViewportCamera>` on the host's `&mut Msaa`, keep the read and
    // write sets disjoint — Bevy rejects the system otherwise (B0001), because a
    // `Changed<T>` filter is an access to `T` just like reading it.
    dirty: Query<
        (),
        (
            With<WorkbenchViewportCamera>,
            Without<WorkbenchEguiHost>,
            Or<(Changed<Msaa>, Changed<Camera>, Added<WorkbenchViewportCamera>)>,
        ),
    >,
    mut removed: RemovedComponents<WorkbenchViewportCamera>,
    host_added: Query<(), Added<WorkbenchEguiHost>>,
    scene_cams: Query<
        (&Camera, &Msaa, &RenderTarget, Has<Hdr>),
        (With<WorkbenchViewportCamera>, Without<WorkbenchEguiHost>),
    >,
    // The host must match the scene camera's `Hdr` too, not just MSAA. Both feed bevy's
    // main-texture key `(target, usages, format, msaa)` — the `Hdr` marker selects the
    // `Rgba16Float` format. A scene camera with bloom/AgX is HDR, so a non-HDR egui host
    // gets a DIFFERENT-format private texture that never shares the (cleared) 3D texture,
    // and stale chrome bakes into it — the ghost that returns on a perspective switch,
    // where only panels change and no camera event fires.
    mut host: Query<(Entity, &mut Msaa, Has<Hdr>), (With<WorkbenchEguiHost>, Without<WorkbenchViewportCamera>)>,
    mut commands: Commands,
) {
    // A removed scene camera (scene swap) must re-run the sync even though no live
    // entity carries the change flag — drain the reader so it doesn't re-fire forever.
    let had_removal = removed.read().count() > 0;
    if dirty.is_empty() && host_added.is_empty() && !had_removal {
        return;
    }
    // Prefer the camera that is actually drawing the window; fall back to any
    // window-targeting scene camera so the host is already correct on the
    // frames before one is activated. All scene cameras take their MSAA from
    // the same setting, so which one we read only matters in a world that is
    // already inconsistent.
    let is_window = |t: &RenderTarget| matches!(t, RenderTarget::Window(_));
    let want = scene_cams
        .iter()
        .find(|(c, _, t, _)| c.is_active && is_window(t))
        .or_else(|| scene_cams.iter().find(|(_, _, t, _)| is_window(t)))
        .map(|(_, msaa, _, hdr)| (*msaa, hdr));
    let Some((want_msaa, want_hdr)) = want else {
        // No 3D camera on the window (Design mode, the Modelica workbench).
        // Nothing renders the scene, so nothing clears the target — but
        // `render_layout` paints a full-window backdrop in exactly that case,
        // which covers the framebuffer. Leave the host's MSAA alone.
        return;
    };
    for (entity, mut msaa, host_hdr) in &mut host {
        // Guarded writes: only touch state (and trip change-detection) when it actually
        // differs, so the host doesn't churn every frame.
        if *msaa != want_msaa {
            *msaa = want_msaa;
        }
        if want_hdr != host_hdr {
            if want_hdr {
                commands.entity(entity).try_insert(Hdr);
            } else {
                commands.entity(entity).remove::<Hdr>();
            }
        }
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
            //
            // This is only sound while this camera SHARES the window's main
            // texture with the scene `Camera3d` — see [`sync_egui_host_msaa`],
            // which is what keeps that true.
            Camera {
                order: 1,
                clear_color: ClearColorConfig::None,
                ..default()
            },
            // MSAA is not spelled out here on purpose: it is not a look choice
            // for this camera, it is the texture-sharing key.
            // [`sync_egui_host_msaa`] copies the scene camera's live value onto
            // this entity every frame — including when the user changes the MSAA
            // setting, which a spawn-time constant could not follow.
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
            commands.entity(entity).try_insert(WorkbenchViewportCamera);
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

/// Clear the pick gate's per-frame inputs. Runs in `First`, **unconditionally**,
/// so the reset happens on every frame — including frames where the egui pass is
/// skipped (window occluded / minimized, or a host that doesn't call
/// `render_workbench`). That is what makes the gate's input lifetime honest:
/// `rendered == false` then tells [`resolve_scene_pointer`] to hold its previous
/// answer rather than resolve against empty inputs.
pub fn reset_scene_pick_gate(mut gate: ResMut<ScenePickGate>) {
    gate.begin_frame();
}

/// Collapse egui's own geometry + this frame's dock/panel rects into the single
/// resolved [`SceneTarget`] that [`track_egui_focus`] (→ `EguiFocus.wants_pointer`)
/// and [`egui_viewport_aware_picking`] read. Runs in PostUpdate after the egui pass,
/// ordered BEFORE both consumers.
///
/// The decision itself is [`resolve_scene_target`] (pure, unit-tested); the
/// press-latch is [`PressLatch`] (pure, unit-tested). This system's only job is to
/// read egui's `Context` and hand those two the inputs.
pub fn resolve_scene_pointer(
    mut gate: ResMut<ScenePickGate>,
    mut q: Query<&mut bevy_egui::EguiContext, With<PrimaryEguiContext>>,
) {
    let mut state = EguiPointerState::default();
    for mut ctx in q.iter_mut() {
        let c = ctx.get_mut();
        state.over_egui |= c.is_pointer_over_egui();
        state.using_pointer |= c.egui_is_using_pointer();
        // `pointer_hover_pos()` — NOT `pointer_interact_pos()`, which egui keeps
        // alive after `PointerGone` and would have the gate resolving against a
        // cursor that has left the window entirely.
        state.hover_pos = state.hover_pos.or_else(|| c.pointer_hover_pos());
        state.any_down |= c.input(|i| i.pointer.any_down());
    }
    gate.resolve(state);
}

/// Scene-vs-chrome-aware egui picking backend — the replacement for bevy_egui's
/// blanket capture (disabled by [`disable_egui_pointer_capture`]).
///
/// Emits a high-order `bevy_picking` hit for the egui context entity ONLY when the
/// pointer is over chrome, i.e. NOT over the main 3D scene. It reads the single
/// resolved [`ScenePickGate::over_main_scene`] computed by [`resolve_scene_pointer`]
/// (which runs just before this), so:
///   • over chrome → egui hit wins → the 3D pick is suppressed;
///   • over an offscreen preview panel (USD) → still a capture hit: that panel owns
///     its own input and mesh hits must not fire *behind* its image;
///   • over the main scene (viewport leaf, full-window View centre) → no egui hit →
///     bevy_picking's mesh hit reaches the scene observers.
/// Mirrors bevy_egui's own `capture_pointer_input_system` (PostUpdate, hit with no
/// world position so consumers can tell chrome from a real mesh pick). Unlike the
/// stock capture it does NOT gate on `egui_wants_pointer_input()` (button-masked —
/// see [`resolve_scene_pointer`]); the resolved signal is unconditional.
pub fn egui_viewport_aware_picking(
    pointers: Query<(
        &bevy::picking::pointer::PointerId,
        &bevy::picking::pointer::PointerLocation,
    )>,
    egui_q: Query<(Entity, &Camera), With<PrimaryEguiContext>>,
    gate: Res<ScenePickGate>,
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
            // scene-vs-chrome into one `SceneTarget`. Over the main scene → let the
            // mesh pick win (no capture). Otherwise → emit a high-order capture hit
            // UNCONDITIONALLY. We must NOT gate on `egui_wants_pointer_input()`
            // here: its `&& !any_down()` clause returns false at the instant of a
            // press over a panel background/label, which let the gizmo/hover pick
            // leak through chrome on click.
            if gate.over_main_scene() {
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
/// resolved [`ScenePickGate::over_main_scene`] — see [`resolve_scene_pointer`],
/// NOT the button-masked `egui_wants_pointer_input()`) so those systems can gate
/// on `EguiFocus` without depending on `bevy_egui`.
///
/// Runs in `PostUpdate` after `EguiPostUpdateSet::ProcessOutput` (same slot as
/// the picking backend) so the flags are this-frame-fresh; consumers in `Update`
/// read them one frame later, which is imperceptible for held input.
pub fn track_egui_focus(
    mut focus: ResMut<lunco_core::EguiFocus>,
    mut q: Query<&mut bevy_egui::EguiContext, With<PrimaryEguiContext>>,
    gate: Res<ScenePickGate>,
) {
    // Pointer gate: egui owns the pointer whenever it is NOT over the main 3D
    // scene. The gate has already resolved that (this-frame, mode-aware,
    // occlusion-aware) AND latched ownership on press, so a drag that started on an
    // egui widget keeps the pointer for its whole duration even as the cursor
    // travels over the viewport — and vice versa. We do NOT use
    // `egui_wants_pointer_input()`: its `&& !any_down()` clause drops to false at
    // the instant of a press over a panel background/label, which leaked scene picks
    // / camera-orbit through chrome on click.
    let ptr = !gate.over_main_scene();

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
            .init_resource::<ScenePickGate>()
            .init_resource::<ViewportPlaceholder>()
            .init_resource::<lunco_core::EguiFocus>()
            .add_systems(Startup, ensure_egui_host)
            // Clear the pick gate's per-frame inputs. `First` — NOT the egui pass —
            // because this must happen even on frames where the egui pass is
            // skipped; that's what lets `resolve_scene_pointer` tell "no inputs"
            // from "stale inputs" and hold instead of resolving against garbage.
            // (`PanelRects` is deliberately NOT cleared here: its consumers run in
            // `Update`, before the egui pass writes it. It is cleared at the top of
            // `render_workbench` instead.)
            .add_systems(First, reset_scene_pick_gate)
            .add_systems(
                Update,
                (
                    check_camera_invariants,
                    check_host_invariant_once,
                    // Load-bearing for the no-ghost-chrome invariant — see
                    // `sync_egui_host_msaa`. Change-driven: early-outs unless a
                    // scene camera's MSAA moved or a camera/host was just added.
                    sync_egui_host_msaa,
                ),
            )
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, Rect};

    const USD_PREVIEW: PanelId = PanelId("usd::viewport");

    fn rect(min: (f32, f32), max: (f32, f32)) -> Rect {
        Rect::from_min_max(pos2(min.0, min.1), pos2(max.0, max.1))
    }

    /// Pointer at `p`, idle, over no real egui surface.
    fn hovering(p: (f32, f32)) -> EguiPointerState {
        EguiPointerState {
            over_egui: false,
            using_pointer: false,
            hover_pos: Some(pos2(p.0, p.1)),
            any_down: false,
        }
    }

    // ── resolve_scene_target: rect math (6a, 6c, 6g) ────────────────────────

    /// 6a — bare full-window 3D OUTSIDE the dock's extent is scene, not chrome.
    /// The old `dock_rect = viewport_ui.min_rect()` made this the whole window, so
    /// the chrome blanket swallowed the click.
    #[test]
    fn below_the_dock_is_scene() {
        let dock = rect((0.0, 30.0), (800.0, 400.0));
        let cards = [(rect((0.0, 30.0), (200.0, 400.0)), rect((0.0, 30.0), (200.0, 400.0)))];
        // Below the dock (y = 500) — bare 3D.
        let out = resolve_scene_target(hovering((400.0, 500.0)), None, &cards, Some(dock), None);
        assert_eq!(out, Some(SceneTarget::MainViewport));
    }

    /// Inside the dock but on no leaf and in no gap (a tab bar / separator) → chrome.
    #[test]
    fn dock_furniture_is_chrome() {
        let dock = rect((0.0, 30.0), (800.0, 400.0));
        let cards = [(rect((0.0, 60.0), (200.0, 400.0)), rect((0.0, 60.0), (200.0, 400.0)))];
        // y = 40 is the leaf's tab-bar strip: inside the dock, outside every body.
        let out = resolve_scene_target(hovering((100.0, 40.0)), None, &cards, Some(dock), None);
        assert_eq!(out, None);
    }

    /// With no dock at all (full-window "View" mode), everything not over an egui
    /// surface is the main scene.
    #[test]
    fn full_window_mode_is_all_scene() {
        let out = resolve_scene_target(hovering((400.0, 300.0)), None, &[], None, None);
        assert_eq!(out, Some(SceneTarget::MainViewport));
    }

    /// 6c — an OPAQUE panel records `card == body`, so its empty lower half is
    /// chrome, not a see-through gap into the hidden 3D scene.
    #[test]
    fn opaque_panel_body_has_no_gap() {
        let body = rect((0.0, 30.0), (200.0, 400.0));
        let cards = [(body, body)]; // opaque: card == body
        let out = resolve_scene_target(hovering((100.0, 380.0)), None, &cards, Some(body), None);
        assert_eq!(out, None);
    }

    /// …while a TRANSPARENT panel's unpainted remainder still falls through to the
    /// full-window 3D behind it.
    #[test]
    fn transparent_panel_gap_is_scene() {
        let body = rect((0.0, 30.0), (200.0, 400.0));
        let card = rect((0.0, 30.0), (200.0, 120.0)); // painted a short card
        let cards = [(body, card)];
        let on_card = resolve_scene_target(hovering((100.0, 60.0)), None, &cards, Some(body), None);
        let in_gap = resolve_scene_target(hovering((100.0, 300.0)), None, &cards, Some(body), None);
        assert_eq!(on_card, None);
        assert_eq!(in_gap, Some(SceneTarget::MainViewport));
    }

    /// A real egui surface (menu bar, floating window, popup) always wins.
    #[test]
    fn over_egui_is_chrome() {
        let mut st = hovering((400.0, 300.0));
        st.over_egui = true;
        assert_eq!(resolve_scene_target(st, None, &[], None, None), None);
    }

    /// 6g — pointer gone from the window ⇒ nothing is hovered. (The old code used
    /// `pointer_interact_pos()`, which egui keeps alive after `PointerGone`.)
    #[test]
    fn pointer_off_window_owns_nothing() {
        let mut st = hovering((400.0, 300.0));
        st.hover_pos = None;
        assert_eq!(resolve_scene_target(st, None, &[], None, None), None);
    }

    /// The scene leaf the pointer is actually inside wins over the dock blanket.
    #[test]
    fn scene_leaf_wins() {
        let dock = rect((0.0, 30.0), (800.0, 400.0));
        let out = resolve_scene_target(
            hovering((400.0, 200.0)),
            Some(SceneTarget::MainViewport),
            &[],
            Some(dock),
            None,
        );
        assert_eq!(out, Some(SceneTarget::MainViewport));
    }

    /// 6d — an offscreen preview resolves to ITS OWN target, never `MainViewport`,
    /// so it cannot double-drive the main avatar camera.
    #[test]
    fn offscreen_preview_is_not_the_main_scene() {
        let dock = rect((0.0, 30.0), (800.0, 400.0));
        let body = rect((200.0, 60.0), (800.0, 400.0));
        // The preview records both its target AND an opaque chrome card.
        let cards = [(body, body)];
        let out = resolve_scene_target(
            hovering((400.0, 200.0)),
            Some(SceneTarget::Offscreen(USD_PREVIEW)),
            &cards,
            Some(dock),
            None,
        );
        assert_eq!(out, Some(SceneTarget::Offscreen(USD_PREVIEW)));
        assert_ne!(out, Some(SceneTarget::MainViewport));
    }

    /// A COLLAPSED (or background-tab) viewport leaf records no `scene_leaf` — its
    /// `ViewportPanel::render` didn't run — but its dock-layout `rect` still covers
    /// the full-window 3D. The click MUST reach the scene, not die as chrome. This
    /// is the collapse/fold/background fix.
    #[test]
    fn collapsed_viewport_leaf_rect_is_scene() {
        let dock = rect((0.0, 30.0), (800.0, 400.0));
        let vp = rect((200.0, 30.0), (800.0, 400.0)); // the viewport leaf's rect
        // No scene_leaf (panel didn't render), no chrome card over the centre.
        let out = resolve_scene_target(hovering((400.0, 200.0)), None, &[], Some(dock), Some(vp));
        assert_eq!(out, Some(SceneTarget::MainViewport));
    }

    /// …but a genuine OPAQUE panel drawn over that rect still wins — chrome beats
    /// the collapsed-viewport fallback (it's checked after the cards).
    #[test]
    fn opaque_panel_over_collapsed_viewport_is_chrome() {
        let dock = rect((0.0, 30.0), (800.0, 400.0));
        let vp = rect((0.0, 30.0), (800.0, 400.0));
        let body = rect((0.0, 30.0), (200.0, 400.0));
        let cards = [(body, body)]; // opaque panel on the left, over the vp rect
        let out = resolve_scene_target(hovering((100.0, 200.0)), None, &cards, Some(dock), Some(vp));
        assert_eq!(out, None);
    }

    // ── PressLatch: the drag-ownership state machine (6b) ───────────────────

    /// Idle → ownership follows the geometry every frame.
    #[test]
    fn latch_idle_follows_geometry() {
        let l = PressLatch::default();
        let l = l.update(false, Some(SceneTarget::MainViewport));
        assert_eq!(l, PressLatch { held: false, owner: Some(SceneTarget::MainViewport) });
        let l = l.update(false, None);
        assert_eq!(l, PressLatch { held: false, owner: None });
    }

    /// 6b — press an egui widget, then drag OVER the viewport: egui keeps the
    /// pointer for the whole drag. (Regression: the scene camera used to start
    /// orbiting mid-drag.)
    #[test]
    fn latch_widget_drag_over_viewport_keeps_chrome() {
        let mut l = PressLatch::default();
        l = l.update(false, None); // hovering the slider
        l = l.update(true, None); // press on the slider → latch chrome
        assert!(l.held);
        // Cursor now travels over the viewport; geometry says "scene".
        l = l.update(true, Some(SceneTarget::MainViewport));
        assert_eq!(l.owner, None, "chrome must keep the pointer for the whole drag");
        l = l.update(true, Some(SceneTarget::MainViewport));
        assert_eq!(l.owner, None);
        // Release → geometry takes over again.
        l = l.update(false, Some(SceneTarget::MainViewport));
        assert_eq!(l, PressLatch { held: false, owner: Some(SceneTarget::MainViewport) });
    }

    /// 6b mirror case — press in the 3D scene to orbit, drag into the inspector:
    /// the scene keeps the pointer. (Regression: the camera used to freeze
    /// mid-orbit.)
    #[test]
    fn latch_scene_drag_into_chrome_keeps_scene() {
        let mut l = PressLatch::default();
        l = l.update(false, Some(SceneTarget::MainViewport));
        l = l.update(true, Some(SceneTarget::MainViewport)); // press in the scene
        l = l.update(true, None); // cursor drags into the inspector
        assert_eq!(l.owner, Some(SceneTarget::MainViewport));
        l = l.update(true, None);
        assert_eq!(l.owner, Some(SceneTarget::MainViewport));
        l = l.update(false, None); // release over the inspector
        assert_eq!(l, PressLatch { held: false, owner: None });
    }

    /// A press adopts the geometry AT THE PRESS — not the value latched by a
    /// previous drag.
    #[test]
    fn latch_press_adopts_geometry_at_press() {
        let mut l = PressLatch { held: false, owner: None };
        l = l.update(true, Some(SceneTarget::MainViewport));
        assert_eq!(l, PressLatch { held: true, owner: Some(SceneTarget::MainViewport) });
    }

    // ── ScenePickGate lifecycle (6e) ────────────────────────────────────────

    /// A frame whose egui pass was skipped must HOLD the previous answer, not
    /// resolve against the (correctly) empty inputs — and must never feed its own
    /// output back in as an input.
    #[test]
    fn skipped_egui_frame_holds_last_answer() {
        let mut gate = ScenePickGate::default();
        // Frame 1: egui ran, pointer over the viewport leaf.
        gate.begin_frame();
        gate.mark_rendered();
        gate.record_scene_leaf(SceneTarget::MainViewport, true);
        gate.resolve(hovering((400.0, 300.0)));
        assert!(gate.over_main_scene());

        // Frame 2: egui pass skipped. Inputs are empty; the gate must hold.
        gate.begin_frame();
        gate.resolve(EguiPointerState::default());
        assert!(gate.over_main_scene(), "must hold, not re-resolve against empty inputs");
    }

    /// The per-frame inputs really are per-frame: a chrome panel that stops
    /// rendering stops blocking.
    #[test]
    fn begin_frame_drops_stale_inputs() {
        let mut gate = ScenePickGate::default();
        let body = rect((0.0, 30.0), (200.0, 400.0));
        gate.begin_frame();
        gate.mark_rendered();
        gate.record_chrome_panel(body, body);
        gate.set_dock_rect(body);
        gate.resolve(hovering((100.0, 100.0)));
        assert_eq!(gate.resolved(), None);

        // Next frame the panel is gone from the layout.
        gate.begin_frame();
        gate.mark_rendered();
        gate.resolve(hovering((100.0, 100.0)));
        assert_eq!(gate.resolved(), Some(SceneTarget::MainViewport));
    }
}
