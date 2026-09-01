//! USD preview sessions and views.
//!
//! A session owns one projected composed USD stage. Views are presentation
//! surfaces over that stage: each has its own camera, light, orbit pose, and
//! render target, while all views of one session share the same scene root and
//! render layer. This keeps multi-view editing cheap and prevents hidden dock
//! tabs from consuming a render pass.
//!
//! The singleton panel paints the focused view, while the instance panel can
//! display any view in a dock tab or split. The body is a real Bevy 3D render;
//! each egui image receives the texture that its view camera renders into.
//!
//! ## Pipeline
//!
//! ```text
//! UsdDocument source text
//!         │
//!         ▼  (on OpenUsdPreview for an explicit doc and preview id)
//! authored layer → canonical composition → UsdStageAsset
//!         │
//!         ▼  (one session owns one stage handle)
//! Handle<UsdStageAsset>
//!         │  (UsdPrimPath { stage_handle, path: "" } on that session root)
//! sync_usd_visuals  →  child entities with meshes / transforms
//!         │
//!         ▼  (each view Camera3d targets its own render-to-texture Image)
//! Image  →  EguiUserTextures  →  egui::TextureId
//!         │
//!         ▼  (panel render)
//! UsdViewportPanel  ─────────  egui::Image in the dock
//! ```
//!
//! ## Lifecycle (observers)
//!
//! - [`OpenUsdPreview`] opens one explicit document/edit target session and its
//!   primary view.
//! - [`OpenUsdPreviewView`] allocates another camera/render target over the
//!   existing session projection; it never duplicates the USD stage.
//! - [`FocusUsdPreview`] and [`FocusUsdPreviewView`] select the dock focus.
//! - [`CloseUsdPreviewView`] releases one view; [`CloseUsdPreview`] releases
//!   the shared session projection and all its remaining views.
//! - [`lunco_doc_bevy::DocumentChanged`] wakes the shared
//!   `twin_projection` owner. It authors the typed edit to the live
//!   canonical stage and the normal USD projection refreshes the preview;
//!   this panel does not re-parse or mutate an asset in-place.
//! - [`DocumentClosed`] → close every preview session for that document and
//!   release its render resources.
//!
//! ## What this plugin does *not* do
//!
//! - The viewport does not compose source text itself. The canonical stage
//!   projection owns sublayers, references, payloads, and variants; this panel
//!   only selects the document whose live stage is projected.

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ImageRenderTarget, RenderTarget};
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureFormat};
use bevy_egui::egui;
use bevy_egui::{EguiTextureHandle, EguiUserTextures};
use lunco_assets::twin_source::TwinRoots;
use lunco_core::{on_command, register_commands, Command};
use lunco_doc::{Document, DocumentId, DocumentOrigin};
use lunco_doc_bevy::DocumentClosed;
use lunco_render::{GraphicsCameraDefaults, SceneCamera};
use lunco_render::{LightGraphicsDefaults, RenderingQualitySettings};
use lunco_usd_bevy::{UsdPreviewOnly, UsdPrimPath, UsdStageAsset, UsdVisualSynced};
use lunco_workbench::{
    CloseTab, InstancePanel, OpenTab, Panel, PanelCtx, PanelId, PanelRects, PanelScrollPolicy,
    PanelSlot, PendingTabCloses, ScenePickGate, SceneTarget, TabId, WorkbenchAppExt,
};
use lunco_workspace::TwinClosed;

use crate::document::{LayerId, UsdDocument};
use lunco_doc_bevy::DocumentRegistry;

use std::collections::{HashMap, HashSet};

/// Stable id of the workbench tab the viewport renders into.
pub const USD_VIEWPORT_PANEL_ID: PanelId = PanelId("usd::viewport");

/// Instance-panel kind for additional views over an existing USD preview
/// session. The instance value is [`UsdPreviewViewId::0`].
pub const USD_PREVIEW_VIEW_PANEL_ID: PanelId = PanelId("usd::preview_view");

/// Initial placeholder dimensions for the offscreen render target.
/// Tiny on purpose: `resize_viewport_image` resizes the asset to the
/// actual panel rect on the first frame after the panel has been
/// drawn, so a small placeholder avoids allocating a multi-megabyte
/// texture that we'll throw away one frame later. If the panel never
/// renders (binary doesn't include `UsdViewportPanel`), the wasted
/// buffer stays at this tiny size.
const PLACEHOLDER_WIDTH: u32 = 16;
const PLACEHOLDER_HEIGHT: u32 = 16;

/// Minimum panel-rect delta (in physical pixels, either axis) before
/// `resize_viewport_image` reallocates the Image. Smaller deltas are
/// ignored so sub-pixel drift / single-pixel layout jitter doesn't
/// thrash the wgpu texture allocator.
const RESIZE_DELTA_PX: u32 = 4;

/// Render policy for USD presentation views. This is a runtime rendering
/// budget, not authored USD state: a large dock or several split views must
/// not allocate unbounded render targets.
#[derive(Resource, Clone, Copy, Debug)]
pub struct UsdPreviewRenderBudget {
    /// Maximum width or height of one preview target.
    pub max_view_dimension: u32,
    /// Maximum pixels allocated to one preview target.
    pub max_view_pixels: u64,
    /// Maximum pixels submitted by visible preview views in one frame.
    pub max_total_pixels: u64,
}

impl Default for UsdPreviewRenderBudget {
    fn default() -> Self {
        Self {
            max_view_dimension: 2048,
            max_view_pixels: 4_194_304,
            max_total_pixels: 8_388_608,
        }
    }
}

#[derive(Resource, Default)]
struct UsdPreviewFrameVisibility {
    views: HashSet<UsdPreviewViewId>,
    pixels: u64,
}

/// `RenderLayers` channel used to isolate USD preview rendering from
/// the main simulation world. Every entity in the preview scene
/// (camera, light, scene_root, and propagated descendants) lives on
/// this layer; the live workbench window camera stays on the default
/// layer 0, so its rendered output never includes preview meshes and
/// the preview camera never sees the live scene. Layer 0 is Bevy's
/// default; using layer 1 here keeps us clear of any third-party
/// systems that might assume layer 0.
const FIRST_PREVIEW_RENDER_LAYER: usize = 1;
const LAST_PREVIEW_RENDER_LAYER: usize = 31;

/// Stable preview identity used by the desktop Assembly editor. Agents and
/// additional editor surfaces use their own explicit ids, so opening another
/// document never retargets this session or any other session.
pub const EDITOR_PREVIEW_ID: UsdPreviewId = UsdPreviewId(1);

/// Plugin that wires the viewport pipeline. Must be added together
/// with `DefaultPlugins` (or any plugin set that ships
/// `Assets<Image>` + the rendering schedule) — gated checks make the
/// observers no-op when those resources are absent so headless tests
/// still link cleanly.
pub struct UsdViewportPlugin;

impl Plugin for UsdViewportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UsdViewportState>();
        app.init_resource::<UsdPreviewRenderBudget>();
        app.init_resource::<UsdPreviewFrameVisibility>();
        app.init_resource::<RenderingQualitySettings>();
        app.register_panel(UsdViewportPanel);
        app.register_instance_panel(UsdPreviewViewPanel);
        app.add_observer(on_twin_closed_for_viewport);
        app.add_observer(on_doc_closed_for_viewport);
        app.add_observer(on_viewport_measured);
        app.add_observer(on_preview_view_measured);
        app.add_observer(on_viewport_orbit_input);
        app.add_systems(
            Update,
            (
                reset_preview_view_visibility,
                drain_preview_view_closes,
                propagate_preview_render_layer,
                frame_preview_views,
                resize_viewport_image,
            ),
        );
        register_all_commands(app);
    }
}

/// Pointer-driven orbit camera (Blender-style preview). Anchored on a
/// `target` point in scene space; left-drag spins yaw/pitch, scroll
/// zooms by adjusting `distance`. All thresholds are tunable per
/// AGENTS.md §3 — no hardcoded magic numbers below the constructor.
#[derive(Debug, Clone)]
pub struct OrbitCamera {
    /// Yaw rotation around +Y (radians).
    pub yaw: f32,
    /// Pitch rotation up/down (radians); clamped to avoid gimbal flip.
    pub pitch: f32,
    /// Distance from target. Scroll wheel scales it geometrically.
    pub distance: f32,
    /// Point the camera orbits around. Pannable in a follow-up.
    pub target: Vec3,
    /// Radians per drag-pixel for yaw + pitch.
    pub drag_sensitivity: f32,
    /// Fractional distance change per scroll unit (0.001 ≈ 0.1% per px).
    pub zoom_sensitivity: f32,
    /// Lower/upper clamps on `distance` so the user can't fly into
    /// the target or out to infinity.
    pub min_distance: f32,
    pub max_distance: f32,
    /// `pitch.abs()` is clamped below this so we never look exactly
    /// straight up/down (LookAt with Vec3::Y is undefined there).
    pub pitch_clamp: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        // The default pose frames the origin while leaving the camera
        // interactive through the orbit controls.
        Self {
            yaw: 0.6747,
            pitch: 0.4435,
            distance: 7.07,
            target: Vec3::ZERO,
            drag_sensitivity: 0.008,
            zoom_sensitivity: 0.0015,
            min_distance: 0.5,
            max_distance: 5_000.0,
            pitch_clamp: std::f32::consts::FRAC_PI_2 - 0.05,
        }
    }
}

impl OrbitCamera {
    /// Camera world-space position derived from the orbit parameters.
    pub fn position(&self) -> Vec3 {
        let cp = self.pitch.cos();
        let sp = self.pitch.sin();
        let cy = self.yaw.cos();
        let sy = self.yaw.sin();
        self.target + Vec3::new(sy * cp, sp, cy * cp) * self.distance
    }

    /// Apply a drag delta (pixels) from the egui image response.
    /// Inverted-Y so dragging down tilts the camera down (Blender
    /// convention).
    pub fn apply_drag(&mut self, delta: egui::Vec2) {
        self.yaw -= delta.x * self.drag_sensitivity;
        self.pitch = (self.pitch + delta.y * self.drag_sensitivity)
            .clamp(-self.pitch_clamp, self.pitch_clamp);
    }

    /// Apply a scroll delta (vertical scroll wheel, pixels).
    pub fn apply_zoom(&mut self, scroll_y: f32) {
        let factor = (1.0 - scroll_y * self.zoom_sensitivity).clamp(0.1, 10.0);
        self.distance = (self.distance * factor).clamp(self.min_distance, self.max_distance);
    }

    /// Build the transform the camera entity should carry this frame.
    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.position()).looking_at(self.target, Vec3::Y)
    }
}

/// Stable identity of one isolated USD preview session.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect, serde::Serialize, serde::Deserialize,
)]
pub struct UsdPreviewId(pub u64);

impl Default for UsdPreviewId {
    fn default() -> Self {
        EDITOR_PREVIEW_ID
    }
}

/// Stable identity of one presentation view over a USD preview session.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Reflect, serde::Serialize, serde::Deserialize,
)]
pub struct UsdPreviewViewId(pub u64);

/// One isolated USD preview session. The document and edit layer are explicit;
/// the session owns only the projected USD stage and scene root. Presentation
/// resources live in [`UsdPreviewView`] so additional views share projection
/// state instead of duplicating the stage.
pub struct UsdPreviewSession {
    id: UsdPreviewId,
    doc: DocumentId,
    edit_target: LayerId,
    scene_root: Entity,
    stage_handle: Handle<UsdStageAsset>,
    render_layer: usize,
    projected_generation: u64,
    primary_view: UsdPreviewViewId,
}

impl UsdPreviewSession {
    pub fn id(&self) -> UsdPreviewId {
        self.id
    }

    pub fn doc(&self) -> DocumentId {
        self.doc
    }

    pub fn edit_target(&self) -> &LayerId {
        &self.edit_target
    }

    pub fn stage_handle(&self) -> &Handle<UsdStageAsset> {
        &self.stage_handle
    }

    pub fn scene_root(&self) -> Entity {
        self.scene_root
    }

    pub fn render_layer(&self) -> usize {
        self.render_layer
    }

    pub fn projected_generation(&self) -> u64 {
        self.projected_generation
    }
}

/// Presentation resources for one USD preview view. All views belonging to a
/// session share its scene root, stage handle, and render layer, but have
/// independent camera state and render targets.
pub struct UsdPreviewView {
    id: UsdPreviewViewId,
    preview: UsdPreviewId,
    image: Handle<Image>,
    tex_id: Option<egui::TextureId>,
    camera: Entity,
    light: Entity,
    orbit: OrbitCamera,
    auto_frame: bool,
}

impl UsdPreviewView {
    pub fn id(&self) -> UsdPreviewViewId {
        self.id
    }

    pub fn preview(&self) -> UsdPreviewId {
        self.preview
    }

    pub fn image(&self) -> &Handle<Image> {
        &self.image
    }

    pub fn camera(&self) -> Entity {
        self.camera
    }

    pub fn light(&self) -> Entity {
        self.light
    }

    pub fn orbit(&self) -> &OrbitCamera {
        &self.orbit
    }

    pub fn texture_id(&self) -> Option<egui::TextureId> {
        self.tex_id
    }
}

/// Session-scoped USD preview registry. Every session owns one projected stage
/// root and render layer; every view owns one render target/camera/light. The
/// dock may paint several views at once, while hidden views are inactive.
#[derive(Resource, Default)]
pub struct UsdViewportState {
    sessions: HashMap<UsdPreviewId, UsdPreviewSession>,
    views: HashMap<UsdPreviewViewId, UsdPreviewView>,
    focused: Option<UsdPreviewId>,
    focused_view: Option<UsdPreviewViewId>,
    next_view_id: u64,
}

impl UsdViewportState {
    pub fn focused_preview_id(&self) -> Option<UsdPreviewId> {
        self.focused
    }

    pub fn focused_session(&self) -> Option<&UsdPreviewSession> {
        self.focused.and_then(|id| self.sessions.get(&id))
    }

    pub fn focused_view_id(&self) -> Option<UsdPreviewViewId> {
        self.focused_view
    }

    pub fn focused_view(&self) -> Option<&UsdPreviewView> {
        self.focused_view.and_then(|id| self.views.get(&id))
    }

    pub fn session(&self, id: UsdPreviewId) -> Option<&UsdPreviewSession> {
        self.sessions.get(&id)
    }

    /// All open preview sessions. Native editor view-models use this iterator
    /// to derive state for every document independently; the dock paints the
    /// focused session and any open instance views.
    pub fn sessions(&self) -> impl Iterator<Item = &UsdPreviewSession> {
        self.sessions.values()
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn view(&self, id: UsdPreviewViewId) -> Option<&UsdPreviewView> {
        self.views.get(&id)
    }

    pub fn views(&self) -> impl Iterator<Item = &UsdPreviewView> {
        self.views.values()
    }

    pub fn view_count(&self) -> usize {
        self.views.len()
    }

    /// Return the next unused view identity for a UI action that needs to
    /// enqueue an explicit `OpenUsdPreviewView` command. `None` means the
    /// finite identity space is exhausted.
    pub fn next_view_id(&self) -> Option<UsdPreviewViewId> {
        let mut id = self.next_view_id.max(1);
        while self.views.contains_key(&UsdPreviewViewId(id)) {
            id = id.checked_add(1)?;
        }
        Some(UsdPreviewViewId(id))
    }

    pub fn has_preview_for(&self, doc: DocumentId) -> bool {
        self.sessions.values().any(|session| session.doc == doc)
    }

    pub fn focused_doc(&self) -> Option<DocumentId> {
        self.focused_session().map(UsdPreviewSession::doc)
    }

    pub fn focused_stage_handle(&self) -> Option<&Handle<UsdStageAsset>> {
        self.focused_session().map(UsdPreviewSession::stage_handle)
    }

    pub fn focused_scene_root(&self) -> Option<Entity> {
        self.focused_session().map(UsdPreviewSession::scene_root)
    }

    pub fn focused_edit_target(&self) -> Option<&LayerId> {
        self.focused_session().map(UsdPreviewSession::edit_target)
    }

    pub(crate) fn session_ids_for_doc(&self, doc: DocumentId) -> Vec<UsdPreviewId> {
        self.sessions
            .values()
            .filter(|session| session.doc == doc)
            .map(UsdPreviewSession::id)
            .collect()
    }

    pub(crate) fn preview_docs(&self) -> impl Iterator<Item = DocumentId> + '_ {
        self.sessions.values().map(UsdPreviewSession::doc)
    }

    fn render_layer_available_for(&self, replacing: Option<UsdPreviewId>) -> Option<usize> {
        (FIRST_PREVIEW_RENDER_LAYER..=LAST_PREVIEW_RENDER_LAYER).find(|layer| {
            self.sessions
                .iter()
                .all(|(id, session)| Some(*id) == replacing || session.render_layer != *layer)
        })
    }

    fn insert(&mut self, session: UsdPreviewSession) {
        self.focused = Some(session.id);
        self.focused_view = Some(session.primary_view);
        self.sessions.insert(session.id, session);
    }

    fn remove(&mut self, id: UsdPreviewId) -> Option<(UsdPreviewSession, Vec<UsdPreviewView>)> {
        let session = self.sessions.remove(&id)?;
        let view_ids: Vec<_> = self
            .views
            .values()
            .filter(|view| view.preview == id)
            .map(UsdPreviewView::id)
            .collect();
        let views = view_ids
            .into_iter()
            .filter_map(|view| self.views.remove(&view))
            .collect();
        if self.focused == Some(id) {
            self.focused = None;
            self.focused_view = None;
            self.focus_first_view();
        }
        Some((session, views))
    }

    fn focus(&mut self, id: UsdPreviewId) -> bool {
        let Some(session) = self.sessions.get(&id) else {
            return false;
        };
        if self.views.contains_key(&session.primary_view) {
            self.focused = Some(id);
            self.focused_view = Some(session.primary_view);
            true
        } else {
            false
        }
    }

    fn focus_view(&mut self, id: UsdPreviewViewId) -> bool {
        let Some(view) = self.views.get(&id) else {
            return false;
        };
        if self.sessions.contains_key(&view.preview) {
            self.focused = Some(view.preview);
            self.focused_view = Some(id);
            true
        } else {
            false
        }
    }

    fn focus_first_view(&mut self) {
        let Some(view) = self.views.values().min_by_key(|view| view.id.0) else {
            return;
        };
        self.focused = Some(view.preview);
        self.focused_view = Some(view.id);
    }

    fn reserve_view_id(&mut self) -> Option<UsdPreviewViewId> {
        let id = self.next_view_id()?;
        self.next_view_id = id.0.checked_add(1).unwrap_or(u64::MAX);
        Some(id)
    }

    fn insert_view(&mut self, view: UsdPreviewView) -> Result<(), UsdPreviewView> {
        if view.id.0 == 0 || !self.sessions.contains_key(&view.preview) {
            return Err(view);
        }
        let id = view.id;
        if self.views.contains_key(&id) {
            return Err(view);
        }
        self.views.insert(id, view);
        self.next_view_id = self
            .next_view_id
            .max(id.0.checked_add(1).unwrap_or(u64::MAX));
        Ok(())
    }

    fn remove_view(&mut self, id: UsdPreviewViewId) -> Option<UsdPreviewView> {
        let view = self.views.remove(&id)?;
        if self.focused_view == Some(id) {
            self.focused_view = None;
            self.focused = None;
            self.focus_first_view();
        }
        Some(view)
    }

    fn view_mut(&mut self, id: UsdPreviewViewId) -> Option<&mut UsdPreviewView> {
        self.views.get_mut(&id)
    }

    fn session_mut(&mut self, id: UsdPreviewId) -> Option<&mut UsdPreviewSession> {
        self.sessions.get_mut(&id)
    }

    pub(crate) fn mark_projected_generation(&mut self, doc: DocumentId, generation: u64) {
        for session in self
            .sessions
            .values_mut()
            .filter(|session| session.doc == doc)
        {
            session.projected_generation = generation;
        }
    }
}

/// UI measurement emitted by the viewport panel after egui lays out its body.
/// The observer owns the workbench interaction resources; the panel only
/// publishes this narrow fact.
#[derive(Event, Clone, Copy, Debug)]
struct UsdViewportMeasured {
    view: UsdPreviewViewId,
    over_scene: bool,
}

/// Measurement emitted by an instance preview panel. The workbench owns the
/// chrome pick gate; this event only records the view-specific render target
/// footprint and marks that camera visible for the current frame.
#[derive(Event, Clone, Copy, Debug)]
struct UsdPreviewViewMeasured {
    view: UsdPreviewViewId,
}

/// Pointer input emitted by the viewport panel. Camera state and the camera
/// entity are updated by the observer, outside the egui paint borrow.
#[derive(Event, Clone, Copy, Debug)]
struct UsdViewportOrbitInput {
    view: UsdPreviewViewId,
    drag: egui::Vec2,
    scroll_y: f32,
}

fn on_viewport_measured(
    trigger: On<UsdViewportMeasured>,
    rects: Res<PanelRects>,
    mut gate: ResMut<ScenePickGate>,
    state: Res<UsdViewportState>,
    mut visibility: ResMut<UsdPreviewFrameVisibility>,
    budget: Res<UsdPreviewRenderBudget>,
    mut cameras: Query<&mut Camera>,
) {
    let event = trigger.event();
    gate.record_scene_leaf(
        SceneTarget::Offscreen(USD_VIEWPORT_PANEL_ID),
        event.over_scene,
    );
    if let Some(rect) = rects.get(USD_VIEWPORT_PANEL_ID) {
        mark_view_visible(
            &state,
            event.view,
            rect.size,
            &mut visibility,
            &budget,
            &mut cameras,
        );
    }
}

fn on_preview_view_measured(
    trigger: On<UsdPreviewViewMeasured>,
    rects: Res<PanelRects>,
    state: Res<UsdViewportState>,
    mut visibility: ResMut<UsdPreviewFrameVisibility>,
    budget: Res<UsdPreviewRenderBudget>,
    mut cameras: Query<&mut Camera>,
) {
    let event = trigger.event();
    if let Some(rect) = rects.get_instance(USD_PREVIEW_VIEW_PANEL_ID, event.view.0) {
        mark_view_visible(
            &state,
            event.view,
            rect.size,
            &mut visibility,
            &budget,
            &mut cameras,
        );
    }
}

fn mark_view_visible(
    state: &UsdViewportState,
    id: UsdPreviewViewId,
    requested: UVec2,
    visibility: &mut UsdPreviewFrameVisibility,
    budget: &UsdPreviewRenderBudget,
    cameras: &mut Query<&mut Camera>,
) {
    if visibility.views.contains(&id) {
        return;
    }
    let Some(view) = state.view(id) else {
        return;
    };
    let Some(target) = bounded_view_size(requested, budget) else {
        return;
    };
    let pixels = u64::from(target.x) * u64::from(target.y);
    if visibility.pixels.saturating_add(pixels) > budget.max_total_pixels {
        return;
    }
    if let Ok(mut camera) = cameras.get_mut(view.camera()) {
        camera.is_active = true;
        visibility.views.insert(id);
        visibility.pixels = visibility.pixels.saturating_add(pixels);
    }
}

fn on_viewport_orbit_input(
    trigger: On<UsdViewportOrbitInput>,
    mut state: ResMut<UsdViewportState>,
    mut transforms: Query<&mut Transform>,
) {
    let input = trigger.event();
    if input.drag == egui::Vec2::ZERO && input.scroll_y == 0.0 {
        return;
    }
    let Some(view) = state.view_mut(input.view) else {
        return;
    };
    if input.drag != egui::Vec2::ZERO {
        view.orbit.apply_drag(input.drag);
    }
    if input.scroll_y != 0.0 {
        view.orbit.apply_zoom(input.scroll_y);
    }
    view.auto_frame = false;
    let camera = view.camera;
    if let Ok(mut transform) = transforms.get_mut(camera) {
        *transform = view.orbit.transform();
    }
}

// ─────────────────────────────────────────────────────────────────────
// Session render resources
// ─────────────────────────────────────────────────────────────────────

/// Allocate one isolated preview session. OpenUSD stage loading and composition
/// remain in the existing asset/projection pipeline; this function owns only
/// Bevy presentation resources and is called after the document coordinates
/// have been admitted by `viewport_twin_coords`.
fn create_preview_session(
    world: &mut World,
    id: UsdPreviewId,
    doc: DocumentId,
    edit_target: LayerId,
    stage_handle: Handle<UsdStageAsset>,
    render_layer: usize,
    primary_view: UsdPreviewViewId,
) -> Option<UsdPreviewSession> {
    if !world.contains_resource::<Assets<Image>>() {
        return None;
    }

    let preview_layers = RenderLayers::layer(render_layer);

    let scene_root = world
        .commands()
        .spawn((
            Transform::default(),
            Visibility::default(),
            Name::new(format!("UsdPreviewRoot-{}", id.0)),
            // Preview-only: usd-sim/usd-avian walk ChildOf up from each
            // candidate prim and bail when they reach this marker, so
            // the preview stage never spawns an Avatar Camera3d into
            // the workbench window (which would cause camera-order
            // ambiguity + gizmo warnings every frame) or activate
            // wheel physics / FSW.
            UsdPreviewOnly,
            // Render-layer seed — `propagate_preview_render_layer` copies it
            // down to every descendant so newly spawned USD prims join this
            // session and cannot leak into another preview or the mission.
            preview_layers,
        ))
        .id();

    world.flush();

    Some(UsdPreviewSession {
        id,
        doc,
        edit_target,
        scene_root,
        stage_handle,
        render_layer,
        projected_generation: 0,
        primary_view,
    })
}

/// Allocate one presentation view over an existing session. This function
/// never creates a scene root or stage handle: those belong to the session and
/// are deliberately shared by every view.
fn create_preview_view(
    world: &mut World,
    preview: UsdPreviewId,
    view: UsdPreviewViewId,
    render_layer: usize,
    preview_illuminance: f32,
) -> Option<UsdPreviewView> {
    if view.0 == 0 || !world.contains_resource::<Assets<Image>>() {
        return None;
    }

    let image = {
        let image = make_target_image(PLACEHOLDER_WIDTH, PLACEHOLDER_HEIGHT);
        world.resource_mut::<Assets<Image>>().add(image)
    };
    let tex_id = world
        .get_resource_mut::<EguiUserTextures>()
        .map(|mut tex| tex.add_image(EguiTextureHandle::Strong(image.clone())));
    let preview_layers = RenderLayers::layer(render_layer);
    let mut commands = world.commands();
    let camera = commands
        .spawn((
            SceneCamera::default(),
            GraphicsCameraDefaults,
            Camera3d::default(),
            Camera {
                clear_color: ClearColorConfig::Custom(Color::srgb(0.10, 0.10, 0.12)),
                order: render_layer as isize,
                // A view becomes active only when its panel paints. This
                // avoids rendering parked/hidden tabs and gives the dock one
                // authoritative visibility boundary.
                is_active: false,
                ..default()
            },
            RenderTarget::Image(ImageRenderTarget::from(image.clone())),
            OrbitCamera::default().transform(),
            preview_layers.clone(),
            Name::new(format!("UsdPreviewCamera-{}-{}", preview.0, view.0)),
        ))
        .id();
    let light = commands
        .spawn((
            DirectionalLight {
                illuminance: preview_illuminance,
                shadow_maps_enabled: false,
                ..default()
            },
            LightGraphicsDefaults {
                intensity_uses_graphics_default: true,
                intensity_scale: 1.0,
                range_uses_graphics_default: false,
            },
            Transform::from_xyz(5.0, 10.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
            preview_layers,
            Name::new(format!("UsdPreviewSun-{}-{}", preview.0, view.0)),
        ))
        .id();
    world.flush();
    Some(UsdPreviewView {
        id: view,
        preview,
        image,
        tex_id,
        camera,
        light,
        orbit: OrbitCamera::default(),
        auto_frame: true,
    })
}

/// Frame a newly projected view around the actual visual bounds of its USD
/// subtree. The bounds are Bevy's computed [`Aabb`]s, so this uses the same
/// render geometry that the camera will draw rather than re-reading USD or
/// inventing per-asset camera poses.
fn frame_preview_views(
    mut state: ResMut<UsdViewportState>,
    q_children: Query<&Children>,
    q_added_bounds: Query<Entity, Or<(Added<Aabb>, Added<Mesh3d>)>>,
    q_bounds: Query<(&GlobalTransform, &Aabb)>,
    mut q_cameras: Query<(&mut Transform, &Projection)>,
) {
    let scan_all = state.is_changed();
    if !scan_all && q_added_bounds.is_empty() {
        return;
    }

    let pending: Vec<_> = state
        .views()
        .filter(|view| view.auto_frame)
        .filter_map(|view| {
            state
                .session(view.preview())
                .map(|session| (view.id, view.camera, session.scene_root()))
        })
        .collect();

    for (view_id, camera, root) in pending {
        if !scan_all
            && !q_added_bounds
                .iter()
                .any(|entity| is_descendant_of(entity, root, &q_children))
        {
            continue;
        }
        let Some((center, radius)) = preview_visual_bounds(view_id, &state, &q_children, &q_bounds)
        else {
            continue;
        };
        let Ok((mut transform, projection)) = q_cameras.get_mut(camera) else {
            continue;
        };
        let Projection::Perspective(projection) = projection else {
            continue;
        };
        let Some(view) = state.view_mut(view_id) else {
            continue;
        };
        let vertical = (projection.fov * 0.5).tan();
        let horizontal = vertical * projection.aspect_ratio.max(f32::EPSILON);
        let half_fov_tangent = vertical.min(horizontal).max(f32::EPSILON);
        let distance = (radius / half_fov_tangent * 1.2)
            .clamp(view.orbit.min_distance, view.orbit.max_distance);
        view.orbit.target = center;
        view.orbit.distance = distance;
        *transform = view.orbit.transform();
        view.auto_frame = false;
    }
}

fn is_descendant_of(entity: Entity, ancestor: Entity, q_children: &Query<&Children>) -> bool {
    let mut stack = vec![ancestor];
    while let Some(parent) = stack.pop() {
        let Ok(children) = q_children.get(parent) else {
            continue;
        };
        for child in children.iter() {
            if child == entity {
                return true;
            }
            stack.push(child);
        }
    }
    false
}

fn preview_visual_bounds(
    view_id: UsdPreviewViewId,
    state: &UsdViewportState,
    q_children: &Query<&Children>,
    q_bounds: &Query<(&GlobalTransform, &Aabb)>,
) -> Option<(Vec3, f32)> {
    let root = state
        .view(view_id)
        .and_then(|view| state.session(view.preview()))
        .map(UsdPreviewSession::scene_root)?;
    let mut stack = vec![root];
    let mut min = Vec3A::splat(f32::INFINITY);
    let mut max = Vec3A::splat(f32::NEG_INFINITY);
    let mut found = false;

    while let Some(entity) = stack.pop() {
        if let Ok((global, aabb)) = q_bounds.get(entity) {
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    for z in [-1.0, 1.0] {
                        let local = aabb.center + aabb.half_extents * Vec3A::new(x, y, z);
                        let world = global.affine().transform_point3a(local);
                        if world.is_finite() {
                            min = min.min(world);
                            max = max.max(world);
                            found = true;
                        }
                    }
                }
            }
        }
        if let Ok(children) = q_children.get(entity) {
            stack.extend(children.iter());
        }
    }

    if !found {
        return None;
    }
    let center = Vec3::from((min + max) * 0.5);
    let radius = (Vec3::from(max - min) * 0.5).length().max(0.5);
    Some((center, radius))
}

/// Push each session's render layer onto every descendant of its root that
/// doesn't yet have a `RenderLayers` component.
///
/// `sync_usd_visuals` (in `lunco-usd-bevy`) spawns child prim entities
/// without `RenderLayers`, which means they default to layer 0 and
/// would otherwise show up in the live workbench window. Walking from
/// each root and inserting that session's layer on missing-RenderLayers
/// descendants gives us hierarchical scoping without modifying USD.
///
/// Entities that already have a `RenderLayers` (e.g. the camera, the
/// light, anything explicitly tagged elsewhere) are left alone — we
/// only seed the default-layer ones to prevent leakage.
fn propagate_preview_render_layer(
    state: Res<UsdViewportState>,
    q_children: Query<&Children>,
    q_has_layers: Query<(), With<RenderLayers>>,
    q_newly_parented: Query<(), Added<ChildOf>>,
    mut commands: Commands,
) {
    // Only re-walk the preview subtree when there's something new to seed:
    // either a session was opened/closed/focused (`state` changed this frame) or
    // some entity was newly parented (USD prims spawn incrementally as the
    // stage loads). Once the scene is static this DFS does no work.
    if !state.is_changed() && q_newly_parented.is_empty() {
        return;
    }

    for session in state.sessions.values() {
        let preview_layers = RenderLayers::layer(session.render_layer);
        // Iterative DFS over one preview session. USD scenes are shallow
        // (tens-to-hundreds of prims), so a small local stack is sufficient.
        let mut stack: Vec<Entity> = Vec::with_capacity(32);
        if let Ok(children) = q_children.get(session.scene_root) {
            for child in children.iter() {
                stack.push(child);
            }
        }
        while let Some(entity) = stack.pop() {
            if q_has_layers.get(entity).is_err() {
                commands.entity(entity).try_insert(preview_layers.clone());
            }
            if let Ok(children) = q_children.get(entity) {
                for child in children.iter() {
                    stack.push(child);
                }
            }
        }
    }
}

/// Park every preview camera before the egui pass. A visible singleton or
/// instance panel explicitly reactivates its view through a typed measurement
/// event, so cameras in background tabs do not render merely because their
/// entities remain alive.
fn reset_preview_view_visibility(
    state: Res<UsdViewportState>,
    mut visibility: ResMut<UsdPreviewFrameVisibility>,
    mut cameras: Query<&mut Camera>,
) {
    visibility.views.clear();
    visibility.pixels = 0;
    for view in state.views() {
        if let Ok(mut camera) = cameras.get_mut(view.camera()) {
            camera.is_active = false;
        }
    }
}

/// Resize visible offscreen render Images to match their panel rects.
///
/// Each active panel writes its view-specific rect during the egui pass. This
/// system reads the previous pass and resizes only those visible views. The
/// Image handle stays valid, so texture registration and render targets remain
/// stable while only the wgpu texture dimensions change.
fn resize_viewport_image(
    // `Option` so the system is headless-safe — `PanelRects` is owned by
    // the workbench UI plugin, absent in lifecycle / headless tests.
    rects: Option<Res<PanelRects>>,
    state: Res<UsdViewportState>,
    budget: Res<UsdPreviewRenderBudget>,
    images: Option<ResMut<Assets<Image>>>,
    mut last_applied: Local<HashMap<UsdPreviewViewId, UVec2>>,
) {
    let (Some(rects), Some(mut images)) = (rects, images) else {
        return;
    };
    for view in state.views() {
        let rect = if state.focused_view_id() == Some(view.id()) {
            rects
                .get(USD_VIEWPORT_PANEL_ID)
                .or_else(|| rects.get_instance(USD_PREVIEW_VIEW_PANEL_ID, view.id().0))
        } else {
            rects.get_instance(USD_PREVIEW_VIEW_PANEL_ID, view.id().0)
        };
        let Some(requested) = rect.map(|rect| rect.size) else {
            continue;
        };
        let Some(target) = bounded_view_size(requested, &budget) else {
            continue;
        };
        let previous = last_applied.get(&view.id()).copied().unwrap_or(UVec2::ZERO);
        let first_apply = previous.x == 0 || previous.y == 0;
        let dx = target.x.abs_diff(previous.x);
        let dy = target.y.abs_diff(previous.y);
        if !first_apply && dx < RESIZE_DELTA_PX && dy < RESIZE_DELTA_PX {
            continue;
        }
        if let Some(mut image) = images.get_mut(view.image()) {
            image.resize(Extent3d {
                width: target.x.max(1),
                height: target.y.max(1),
                depth_or_array_layers: 1,
            });
            last_applied.insert(view.id(), target);
        }
    }
}

/// Bound one visible view's render target by the presentation budget while
/// preserving its aspect ratio as closely as integer dimensions allow.
/// Invalid zero-valued limits produce no target; they are configuration errors,
/// not a reason to allocate an unbounded image.
fn bounded_view_size(requested: UVec2, budget: &UsdPreviewRenderBudget) -> Option<UVec2> {
    if budget.max_view_dimension == 0 || budget.max_view_pixels == 0 || budget.max_total_pixels == 0
    {
        return None;
    }

    let mut size = UVec2::new(
        requested.x.max(1).min(budget.max_view_dimension),
        requested.y.max(1).min(budget.max_view_dimension),
    );
    let pixels = u64::from(size.x) * u64::from(size.y);
    if pixels <= budget.max_view_pixels {
        return Some(size);
    }

    let scale = (budget.max_view_pixels as f64 / pixels as f64).sqrt();
    size.x = ((size.x as f64 * scale).floor() as u32).max(1);
    size.y = ((size.y as f64 * scale).floor() as u32).max(1);
    while u64::from(size.x) * u64::from(size.y) > budget.max_view_pixels {
        if size.x >= size.y && size.x > 1 {
            size.x -= 1;
        } else if size.y > 1 {
            size.y -= 1;
        } else {
            break;
        }
    }
    Some(size)
}

/// Construct a render-target image with sensible defaults
/// (Bgra8UnormSrgb, RENDER_ATTACHMENT).
fn make_target_image(width: u32, height: u32) -> Image {
    // `Image::new_target_texture` sets all three usage flags (incl.
    // RENDER_ATTACHMENT) and fills with zeros in 0.18, using
    // RenderAssetUsages::default(). We want a simple linear-RGBA
    // target — egui displays sRGB so Bgra8UnormSrgb keeps colours
    // right without an extra conversion pass.
    Image::new_target_texture(width, height, TextureFormat::Bgra8UnormSrgb, None)
}

fn validated_preview_illuminance(world: &World) -> Result<f32, String> {
    if !world.contains_resource::<Assets<Image>>() {
        return Err("USD preview rendering is unavailable in this host".to_string());
    }
    let settings = world
        .get_resource::<RenderingQualitySettings>()
        .ok_or_else(|| "Graphics quality settings are unavailable in this host".to_string())?;
    settings
        .validated_profile()
        .map(|profile| profile.distant_light_default_illuminance)
        .map_err(|reason| format!("invalid Graphics quality: {reason}"))
}

// ─────────────────────────────────────────────────────────────────────
// Preview session commands
// ─────────────────────────────────────────────────────────────────────

/// Open one explicit document and authored edit target in an isolated preview
/// session. Opening the same `preview` id replaces that session; other sessions
/// keep their roots, cameras, and stages untouched.
#[Command(default)]
pub struct OpenUsdPreview {
    /// Stable caller-owned identity of the preview session.
    pub preview: UsdPreviewId,
    /// The USD document to render.
    pub doc: DocumentId,
    /// The authored layer to use for editor mutations made from this preview.
    pub edit_target: LayerId,
}

/// Focus an already-open preview session in the USD dock.
#[Command(default)]
pub struct FocusUsdPreview {
    pub preview: UsdPreviewId,
}

/// Open an additional presentation view over an existing USD preview session.
/// The view id is explicit so persisted layouts and agents can address the
/// exact camera without relying on tab order or display names.
#[Command(default)]
pub struct OpenUsdPreviewView {
    pub preview: UsdPreviewId,
    pub view: UsdPreviewViewId,
}

/// Focus one presentation view and its parent USD preview session.
#[Command(default)]
pub struct FocusUsdPreviewView {
    pub view: UsdPreviewViewId,
}

/// Close one presentation view. Closing the final view also closes its parent
/// preview session because a session without a presentation view cannot be
/// reached from the editor.
#[Command(default)]
pub struct CloseUsdPreviewView {
    pub view: UsdPreviewViewId,
}

/// Close one preview session and release all of its presentation resources.
#[Command(default)]
pub struct CloseUsdPreview {
    pub preview: UsdPreviewId,
}

#[on_command(OpenUsdPreview)]
fn on_open_usd_preview(trigger: On<OpenUsdPreview>, mut commands: Commands) {
    let command = trigger.event();
    let preview = command.preview;
    let doc = command.doc;
    let edit_target = command.edit_target.clone();
    commands.queue(move |world: &mut World| {
        if !world
            .resource::<DocumentRegistry<UsdDocument>>()
            .contains(doc)
        {
            report_preview_error(
                world,
                "usd-preview-open-failed",
                format!("document {doc} is not open"),
            );
            return;
        }
        let target_valid = world
            .resource::<DocumentRegistry<UsdDocument>>()
            .host(doc)
            .and_then(|host| host.document().authored_prim_exists(&edit_target, "/").ok())
            .is_some();
        if !target_valid {
            report_preview_error(
                world,
                "usd-preview-open-failed",
                format!(
                    "unknown edit target `{}` for document {doc}",
                    edit_target.as_str()
                ),
            );
            return;
        }
        let preview_illuminance = match validated_preview_illuminance(world) {
            Ok(illuminance) => illuminance,
            Err(detail) => {
                report_preview_error(world, "usd-preview-open-failed", detail);
                return;
            }
        };
        let Some(render_layer) = world
            .resource::<UsdViewportState>()
            .render_layer_available_for(Some(preview))
        else {
            report_preview_error(
                world,
                "usd-preview-open-failed",
                "all preview render layers are in use".to_string(),
            );
            return;
        };
        let Some(primary_view) = world.resource_mut::<UsdViewportState>().reserve_view_id() else {
            report_preview_error(
                world,
                "usd-preview-open-failed",
                "USD preview view identity space is exhausted".to_string(),
            );
            return;
        };
        let Some((name, rel)) = viewport_twin_coords(world, doc) else {
            report_preview_error(
                world,
                "usd-preview-open-failed",
                format!("document {doc} has no loadable composed USD source"),
            );
            return;
        };
        let stage_handle = world
            .resource::<AssetServer>()
            .load::<UsdStageAsset>(lunco_assets::twin_uri(&name, &rel));
        if let Some(old_doc) = world
            .resource::<UsdViewportState>()
            .session(preview)
            .map(UsdPreviewSession::doc)
        {
            remove_preview_session(world, preview);
            release_preview_projection(world, old_doc);
        }
        let Some(session) = create_preview_session(
            world,
            preview,
            doc,
            edit_target,
            stage_handle,
            render_layer,
            primary_view,
        ) else {
            report_preview_error(
                world,
                "usd-preview-open-failed",
                "USD preview render resources could not be allocated".to_string(),
            );
            return;
        };
        world.resource_mut::<UsdViewportState>().insert(session);
        let Some(view) = create_preview_view(
            world,
            preview,
            primary_view,
            render_layer,
            preview_illuminance,
        ) else {
            let _ = remove_preview_session(world, preview);
            report_preview_error(
                world,
                "usd-preview-open-failed",
                "USD preview view resources could not be allocated".to_string(),
            );
            return;
        };
        if let Err(view) = world.resource_mut::<UsdViewportState>().insert_view(view) {
            despawn_preview_view(world, view);
            let _ = remove_preview_session(world, preview);
            report_preview_error(
                world,
                "usd-preview-open-failed",
                format!("primary view {} could not be registered", primary_view.0),
            );
            return;
        }
        world
            .resource_mut::<crate::twin_projection::DocBackedTwinScenes>()
            .track_preview(doc, name, rel);
        world
            .resource_mut::<crate::twin_projection::DocBackedTwinScenes>()
            .acquire_preview(doc);
        let claimed = world
            .resource_mut::<crate::twin_projection::DocBackedTwinScenes>()
            .claim_user(doc);
        if claimed {
            world.trigger(crate::twin_projection::UsdDocumentUserOwned { doc });
        }
        mount_preview_session(world, preview);
    });
}

#[on_command(OpenUsdPreviewView)]
fn on_open_usd_preview_view(trigger: On<OpenUsdPreviewView>, mut commands: Commands) {
    let command = trigger.event();
    let preview = command.preview;
    let view = command.view;
    commands.queue(move |world: &mut World| {
        let Some(session) = world.resource::<UsdViewportState>().session(preview) else {
            report_preview_error(
                world,
                "usd-preview-view-open-failed",
                format!("preview {} is not open", preview.0),
            );
            return;
        };
        if view.0 == 0 || world.resource::<UsdViewportState>().view(view).is_some() {
            report_preview_error(
                world,
                "usd-preview-view-open-failed",
                format!("view {} is invalid or already open", view.0),
            );
            return;
        }
        let render_layer = session.render_layer();
        let illuminance = match validated_preview_illuminance(world) {
            Ok(value) => value,
            Err(detail) => {
                report_preview_error(world, "usd-preview-view-open-failed", detail);
                return;
            }
        };
        let Some(view_state) = create_preview_view(world, preview, view, render_layer, illuminance)
        else {
            report_preview_error(
                world,
                "usd-preview-view-open-failed",
                "USD preview view resources could not be allocated".to_string(),
            );
            return;
        };
        if let Err(view_state) = world
            .resource_mut::<UsdViewportState>()
            .insert_view(view_state)
        {
            despawn_preview_view(world, view_state);
            report_preview_error(
                world,
                "usd-preview-view-open-failed",
                format!("view {} could not be registered", view.0),
            );
            return;
        }
        world.trigger(OpenTab {
            kind: USD_PREVIEW_VIEW_PANEL_ID,
            instance: view.0,
        });
    });
}

#[on_command(FocusUsdPreview)]
fn on_focus_usd_preview(trigger: On<FocusUsdPreview>, mut commands: Commands) {
    let preview = trigger.event().preview;
    commands.queue(move |world: &mut World| {
        if !world.resource_mut::<UsdViewportState>().focus(preview) {
            report_preview_error(
                world,
                "usd-preview-focus-failed",
                format!("preview {} is not open", preview.0),
            );
        }
    });
}

#[on_command(FocusUsdPreviewView)]
fn on_focus_usd_preview_view(trigger: On<FocusUsdPreviewView>, mut commands: Commands) {
    let view = trigger.event().view;
    commands.queue(move |world: &mut World| {
        if !world.resource_mut::<UsdViewportState>().focus_view(view) {
            report_preview_error(
                world,
                "usd-preview-view-focus-failed",
                format!("view {} is not open", view.0),
            );
        }
    });
}

#[on_command(CloseUsdPreviewView)]
fn on_close_usd_preview_view(trigger: On<CloseUsdPreviewView>, mut commands: Commands) {
    let view = trigger.event().view;
    commands.queue(move |world: &mut World| {
        close_preview_view(world, view);
    });
}

#[on_command(CloseUsdPreview)]
fn on_close_usd_preview(trigger: On<CloseUsdPreview>, mut commands: Commands) {
    let preview = trigger.event().preview;
    commands.queue(move |world: &mut World| {
        let Some(doc) = remove_preview_session(world, preview) else {
            report_preview_error(
                world,
                "usd-preview-close-failed",
                format!("preview {} is not open", preview.0),
            );
            return;
        };
        release_preview_projection(world, doc);
    });
}

register_commands!(
    on_open_usd_preview,
    on_open_usd_preview_view,
    on_focus_usd_preview,
    on_focus_usd_preview_view,
    on_close_usd_preview_view,
    on_close_usd_preview,
);

// ─────────────────────────────────────────────────────────────────────
// Document lifecycle observers
// ─────────────────────────────────────────────────────────────────────

/// A preview may outlive the Twin that originally supplied its `twin://`
/// authority. Reinstall every affected session after the Twin-close observer
/// retires that authority, preserving each session's independent root/camera.
fn on_twin_closed_for_viewport(_trigger: On<TwinClosed>, mut commands: Commands) {
    commands.queue(|world: &mut World| {
        let docs: Vec<_> = world
            .resource::<UsdViewportState>()
            .preview_docs()
            .collect();
        for doc in docs {
            let needs_rehome = world
                .resource::<crate::twin_projection::DocBackedTwinScenes>()
                .coords_of(doc)
                .map(|(name, _)| matches!(world.resource::<TwinRoots>().root_for(&name), Ok(None)))
                .unwrap_or(true);
            if !needs_rehome {
                continue;
            }
            world
                .resource_mut::<crate::twin_projection::DocBackedTwinScenes>()
                .detach_projection(doc);
            let sessions = world
                .resource::<UsdViewportState>()
                .session_ids_for_doc(doc);
            for preview in sessions {
                mount_preview_session(world, preview);
            }
        }
    });
}

fn on_doc_closed_for_viewport(trigger: On<DocumentClosed>, mut commands: Commands) {
    let doc = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let sessions = world
            .resource::<UsdViewportState>()
            .session_ids_for_doc(doc);
        for preview in sessions {
            let _ = remove_preview_session(world, preview);
        }
        release_preview_projection(world, doc);
    });
}

/// Claim close requests for USD preview-view tabs. A view is presentation-only
/// and never dirty, so its tab close can immediately release the camera,
/// target, and (when final) the shared preview session.
fn drain_preview_view_closes(pending: Option<ResMut<PendingTabCloses>>, mut commands: Commands) {
    let Some(mut pending) = pending else {
        return;
    };
    let requested = pending.drain();
    let mut unclaimed = Vec::new();
    for tab in requested {
        let TabId::Instance { kind, instance } = tab else {
            unclaimed.push(tab);
            continue;
        };
        if kind != USD_PREVIEW_VIEW_PANEL_ID {
            unclaimed.push(tab);
            continue;
        }
        commands.trigger(CloseUsdPreviewView {
            view: UsdPreviewViewId(instance),
        });
        commands.trigger(CloseTab { kind, instance });
    }
    for tab in unclaimed {
        pending.push(tab);
    }
}

// ─────────────────────────────────────────────────────────────────────
// Asset install / rebuild
// ─────────────────────────────────────────────────────────────────────

/// Mount the document selected by one existing preview session. This is the
/// only viewport-side stage binding; all ordinary and structural edits are
/// still consumed by `sync_twin_overlays` and the canonical stage sink.
fn mount_preview_session(world: &mut World, preview: UsdPreviewId) {
    let Some(doc) = world
        .resource::<UsdViewportState>()
        .session(preview)
        .map(UsdPreviewSession::doc)
    else {
        return;
    };
    let Some((name, rel)) = viewport_twin_coords(world, doc) else {
        return;
    };
    let handle = world
        .resource::<AssetServer>()
        .load::<UsdStageAsset>(lunco_assets::twin_uri(&name, &rel));
    world
        .resource_mut::<crate::twin_projection::DocBackedTwinScenes>()
        .track_preview(doc, name, rel);
    world
        .resource_mut::<crate::twin_projection::TwinProjectionWake>()
        .wake();
    let Some(scene_root) = world
        .resource::<UsdViewportState>()
        .session(preview)
        .map(UsdPreviewSession::scene_root)
    else {
        return;
    };
    if let Ok(mut entity) = world.get_entity_mut(scene_root) {
        entity.remove::<UsdVisualSynced>();
        entity.despawn_related::<Children>();
        entity.insert(UsdPrimPath {
            stage_handle: handle.clone(),
            // The empty path is the scene-root sentinel. The visual projector
            // resolves it through the composed defaultPrim.
            path: String::new(),
        });
    }
    let generation = world
        .resource::<DocumentRegistry<UsdDocument>>()
        .host(doc)
        .map(|h| h.document().generation())
        .unwrap_or(0);
    if let Some(session) = world
        .resource_mut::<UsdViewportState>()
        .session_mut(preview)
    {
        session.stage_handle = handle;
        session.projected_generation = generation;
    }
}

fn report_preview_error(world: &mut World, name: &str, detail: String) {
    world.trigger(lunco_core::TelemetryEvent {
        name: name.to_string(),
        source: 0,
        severity: lunco_core::Severity::Error,
        data: lunco_core::TelemetryValue::String(detail),
        timestamp: 0.0,
    });
}

/// Remove the Bevy resources owned by one preview session. The document
/// projection is released separately so shared coordinates survive until
/// the final session closes.
fn remove_preview_session(world: &mut World, preview: UsdPreviewId) -> Option<DocumentId> {
    let (session, views) = world.resource_mut::<UsdViewportState>().remove(preview)?;
    let doc = session.doc;
    if let Ok(mut entity) = world.get_entity_mut(session.scene_root) {
        entity.despawn_related::<Children>();
        entity.despawn();
    }
    for view in views {
        despawn_preview_view(world, view);
    }
    Some(doc)
}

fn despawn_preview_view(world: &mut World, view: UsdPreviewView) {
    if let Ok(entity) = world.get_entity_mut(view.camera) {
        entity.despawn();
    }
    if let Ok(entity) = world.get_entity_mut(view.light) {
        entity.despawn();
    }
    if let Some(mut textures) = world.get_resource_mut::<EguiUserTextures>() {
        textures.remove_image(view.image.id());
    }
    if let Some(mut images) = world.get_resource_mut::<Assets<Image>>() {
        images.remove(view.image.id());
    }
}

fn close_preview_view(world: &mut World, view: UsdPreviewViewId) {
    let Some(view_state) = world.resource_mut::<UsdViewportState>().remove_view(view) else {
        report_preview_error(
            world,
            "usd-preview-view-close-failed",
            format!("view {} is not open", view.0),
        );
        return;
    };
    let preview = view_state.preview;
    despawn_preview_view(world, view_state);
    let has_remaining = world
        .resource::<UsdViewportState>()
        .views()
        .any(|candidate| candidate.preview() == preview);
    if !has_remaining {
        if let Some(doc) = remove_preview_session(world, preview) {
            release_preview_projection(world, doc);
        }
    }
}

fn release_preview_projection(world: &mut World, doc: DocumentId) {
    let Some((name, _rel)) = world
        .resource_mut::<crate::twin_projection::DocBackedTwinScenes>()
        .release_preview(doc)
    else {
        return;
    };
    if let Err(error) = world.resource::<TwinRoots>().unregister_name(&name) {
        report_preview_error(
            world,
            "twin-asset-unmount-failed",
            format!("could not unregister preview Twin `{name}`: {error}"),
        );
    }
}

/// The `twin://` coordinates (`name`, `rel`) to load `doc` through the async
/// twin source. Reuses the coordinates the document is already doc-backed under
/// (a default twin scene → shared overlay + asset), else registers a synthetic
/// per-document twin root and serves the doc's **composed** (`base ⊕ runtime`)
/// source as a byte-overlay so the async loader composes from the editable
/// document via storage — references resolve relative to the doc's base dir
/// through the twin source. `None` when the document is gone.
fn viewport_twin_coords(world: &mut World, doc: DocumentId) -> Option<(String, String)> {
    // Already doc-backed (e.g. the default twin scene)? Reuse its overlay + asset.
    if let Some(coords) = world
        .resource::<crate::twin_projection::DocBackedTwinScenes>()
        .coords_of(doc)
    {
        match world.resource::<TwinRoots>().root_for(&coords.0) {
            Ok(Some(_)) => return Some(coords),
            Ok(None) => {}
            Err(error) => {
                error!(
                    "cannot inspect viewport Twin authority `{}`: {error}",
                    coords.0
                );
                return None;
            }
        }
        world
            .resource_mut::<crate::twin_projection::DocBackedTwinScenes>()
            .detach_projection(doc);
    }
    let host = world
        .resource::<DocumentRegistry<UsdDocument>>()
        .host(doc)?;
    let composed = host.document().composed_source();
    let (base, rel) = match host.document().origin() {
        DocumentOrigin::File { path, .. } => (
            path.parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from(".")),
            path.file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "scene.usda".to_string()),
        ),
        // Untitled / in-memory: no external refs to resolve; a placeholder root
        // is enough for the overlay to serve the composed source.
        _ => (std::path::PathBuf::from("."), "scene.usda".to_string()),
    };
    // A stable, URI-safe synthetic twin name for this document.
    let name =
        format!("__viewport_{doc}").replace(|c: char| !c.is_ascii_alphanumeric() && c != '_', "_");
    // Use the ASSIGNED name: if this synthetic name is already bound to a
    // different base (same doc re-registered from a new location), the registry
    // hands back a disambiguated one — and the overlay must be keyed to that,
    // or the viewport serves its composed source under a name nobody reads.
    let name = match world.resource::<TwinRoots>().register(&name, base) {
        Ok(name) => name,
        Err(error) => {
            world.trigger(lunco_core::TelemetryEvent {
                name: "twin-asset-mount-failed".into(),
                source: 0,
                severity: lunco_core::Severity::Error,
                data: lunco_core::TelemetryValue::String(error.to_string()),
                timestamp: 0.0,
            });
            return None;
        }
    };
    if let Err(error) = world.resource::<TwinRoots>().set_overlay(
        &name,
        &rel,
        std::sync::Arc::new(composed.into_bytes()),
    ) {
        world.trigger(lunco_core::TelemetryEvent {
            name: "twin-asset-mount-failed".into(),
            source: 0,
            severity: lunco_core::Severity::Error,
            data: lunco_core::TelemetryValue::String(error.to_string()),
            timestamp: 0.0,
        });
        if let Err(cleanup_error) = world.resource::<TwinRoots>().unregister_name(&name) {
            world.trigger(lunco_core::TelemetryEvent {
                name: "twin-asset-unmount-failed".into(),
                source: 0,
                severity: lunco_core::Severity::Error,
                data: lunco_core::TelemetryValue::String(cleanup_error.to_string()),
                timestamp: 0.0,
            });
        }
        return None;
    }
    Some((name, rel))
}

// ─────────────────────────────────────────────────────────────────────
// UsdViewportPanel
// ─────────────────────────────────────────────────────────────────────

/// Workbench panel displaying the focused USD preview view. Other sessions and
/// dockable views remain independently addressable and editable.
pub struct UsdViewportPanel;

impl Panel for UsdViewportPanel {
    fn id(&self) -> PanelId {
        USD_VIEWPORT_PANEL_ID
    }

    fn title(&self) -> String {
        "USD Preview".to_string()
    }

    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn scene_target(&self) -> Option<SceneTarget> {
        // This is NOT the full-window 3D scene: it renders a camera to an offscreen
        // `Image` and shows it as an `egui::Image` with its own `click_and_drag`
        // orbit handling (below). Declaring `MainViewport` here made every drag over
        // the preview ALSO drive the main avatar camera, and let `bevy_picking` mesh
        // hits fire in the main scene *behind* the image. As an `Offscreen` target it
        // owns its own input and the gate keeps the main scene out of it — while the
        // dock dispatch still records it as an opaque blocked region (it has the
        // default opaque background), so nothing leaks through.
        Some(SceneTarget::Offscreen(USD_VIEWPORT_PANEL_ID))
    }

    fn closable(&self) -> bool {
        false
    }

    fn scroll_policy(&self) -> PanelScrollPolicy {
        PanelScrollPolicy::SelfManaged
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let Some(view) = ctx
            .resource::<UsdViewportState>()
            .and_then(UsdViewportState::focused_view_id)
        else {
            ui.centered_and_justified(|ui| ui.label("No USD preview is open."));
            return;
        };
        render_preview_view(ui, ctx, view, true);
    }
}

/// Render one view through either the focused singleton surface or an
/// instance tab. The USD stage and editor state are read-only here; all focus,
/// sizing, and orbit changes are emitted as typed intents after paint.
fn render_preview_view(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    view_id: UsdPreviewViewId,
    singleton: bool,
) {
    let (tex_id, focused_doc, next_view) = ctx
        .resource::<UsdViewportState>()
        .and_then(|state| {
            let view = state.view(view_id)?;
            let session = state.session(view.preview())?;
            let next_view = singleton
                .then(|| state.next_view_id())
                .flatten()
                .map(|view| (session.id(), view));
            Some((view.texture_id(), Some(session.doc()), next_view))
        })
        .unwrap_or((None, None, None));
    let name = focused_doc
        .and_then(|doc| {
            ctx.resource::<DocumentRegistry<UsdDocument>>()
                .and_then(|registry| registry.host(doc))
                .map(|host| host.document().origin().display_name())
        })
        .unwrap_or_else(|| "(no stage)".to_string());

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&name).strong());
        if let Some((preview, view)) = next_view {
            if ui.button("Open view").clicked() {
                ctx.trigger(OpenUsdPreviewView { preview, view });
            }
        }
    });
    ui.separator();

    let Some(tex_id) = tex_id else {
        ui.centered_and_justified(|ui| {
            ui.label(
                egui::RichText::new("The USD preview is still being projected.")
                    .weak()
                    .italics(),
            );
        });
        return;
    };

    let size = ui.available_size();
    let response = ui.add(
        egui::Image::new(egui::load::SizedTexture::new(tex_id, size))
            .sense(egui::Sense::click_and_drag()),
    );
    let over_scene = ui.rect_contains_pointer(response.rect);
    if singleton {
        ctx.trigger(UsdViewportMeasured {
            view: view_id,
            over_scene,
        });
    } else {
        ctx.trigger(UsdPreviewViewMeasured { view: view_id });
        // Selecting a dock tab is a view-focus action. The instance renderer
        // publishes that choice so all native editor panels follow the same
        // view/session binding.
        let already_focused = ctx
            .resource::<UsdViewportState>()
            .is_some_and(|state| state.focused_view_id() == Some(view_id));
        if !already_focused {
            ctx.trigger(FocusUsdPreviewView { view: view_id });
        }
    }

    let drag = response.drag_delta();
    let scroll_y = if response.hovered() {
        ui.ctx().input(|input| input.smooth_scroll_delta.y)
    } else {
        0.0
    };
    if drag != egui::Vec2::ZERO || scroll_y != 0.0 {
        ctx.trigger(UsdViewportOrbitInput {
            view: view_id,
            drag,
            scroll_y,
        });
    }
}

/// A dockable view over one existing USD preview session. Multiple instances
/// share the session's projected stage but render through independent cameras.
pub struct UsdPreviewViewPanel;

impl InstancePanel for UsdPreviewViewPanel {
    fn kind(&self) -> PanelId {
        USD_PREVIEW_VIEW_PANEL_ID
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn title(&self, world: &World, instance: u64) -> String {
        let view = UsdPreviewViewId(instance);
        let name = world
            .get_resource::<UsdViewportState>()
            .and_then(|state| state.view(view))
            .and_then(|view| {
                world.resource::<DocumentRegistry<UsdDocument>>().host(
                    world
                        .resource::<UsdViewportState>()
                        .session(view.preview())?
                        .doc(),
                )
            })
            .map(|host| host.document().origin().display_name())
            .unwrap_or_else(|| "USD".to_string());
        format!("{name} · View {instance}")
    }

    fn scroll_policy(&self) -> PanelScrollPolicy {
        PanelScrollPolicy::SelfManaged
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx, instance: u64) {
        render_preview_view(ui, ctx, UsdPreviewViewId(instance), false);
    }

    fn tab_context_menu(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx, instance: u64) {
        if ui.button("Close view").clicked() {
            let view = UsdPreviewViewId(instance);
            ctx.trigger(CloseUsdPreviewView { view });
            ctx.trigger(CloseTab {
                kind: USD_PREVIEW_VIEW_PANEL_ID,
                instance,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::UsdCommandsPlugin;

    /// Without any rendering plugins (`Assets<Image>` absent), opening a
    /// document does not allocate a preview session or panic.
    #[test]
    fn lifecycle_is_headless_safe() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.add_plugins(UsdViewportPlugin);
        app.update();

        let _doc = {
            let mut reg = app
                .world_mut()
                .resource_mut::<DocumentRegistry<UsdDocument>>();
            reg.open_file("/tmp/x.usda", "#usda 1.0\n".to_string()).0
        };
        // Drain pending events twice to settle the document lifecycle.
        app.update();
        app.update();

        let state = app.world().resource::<UsdViewportState>();
        assert_eq!(state.session_count(), 0);
        assert_eq!(state.focused_doc(), None);
    }

    /// Opening a USD document registers it for authoring, but does not create
    /// a preview session until an explicit `OpenUsdPreview` command arrives.
    #[test]
    fn document_open_requires_explicit_preview_selection() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        app.add_plugins(UsdCommandsPlugin);
        app.add_plugins(UsdViewportPlugin);

        let doc = {
            let mut registry = app
                .world_mut()
                .resource_mut::<DocumentRegistry<UsdDocument>>();
            registry
                .open_file("/tmp/assembly.usda", "#usda 1.0\n".to_string())
                .0
        };
        app.update();
        app.update();

        let state = app.world().resource::<UsdViewportState>();
        assert_eq!(state.session_count(), 0);
        assert_eq!(state.focused_doc(), None);
        assert!(app
            .world()
            .resource::<DocumentRegistry<UsdDocument>>()
            .contains(doc));
    }

    #[test]
    fn preview_light_uses_graphics_distant_light_default() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>();
        let mut settings = RenderingQualitySettings::default();
        settings.distant_light_default_illuminance = 42_000.0;
        app.insert_resource(settings);

        let illuminance = validated_preview_illuminance(app.world()).expect("quality is valid");
        let _session = create_preview_session(
            app.world_mut(),
            UsdPreviewId(1),
            DocumentId::new(1),
            LayerId::root(),
            Handle::default(),
            FIRST_PREVIEW_RENDER_LAYER,
            UsdPreviewViewId(1),
        )
        .expect("preview resources are available");
        let view = create_preview_view(
            app.world_mut(),
            UsdPreviewId(1),
            UsdPreviewViewId(1),
            FIRST_PREVIEW_RENDER_LAYER,
            illuminance,
        )
        .expect("preview view resources are available");

        let mut lights = app
            .world_mut()
            .query::<(&DirectionalLight, &LightGraphicsDefaults)>();
        let (light, defaults) = lights
            .iter(app.world())
            .next()
            .expect("preview session creates its Graphics-owned sun");
        assert_eq!(light.illuminance, 42_000.0);
        assert!(defaults.intensity_uses_graphics_default);
        assert_eq!(defaults.intensity_scale, 1.0);
        let camera = app
            .world()
            .get_entity(view.camera())
            .expect("preview session creates its camera");
        assert!(camera.contains::<SceneCamera>());
        assert!(camera.contains::<GraphicsCameraDefaults>());
    }

    #[test]
    fn preview_budget_bounds_dimensions_and_pixels() {
        let budget = UsdPreviewRenderBudget::default();
        let target = bounded_view_size(UVec2::new(8192, 4096), &budget)
            .expect("default presentation budget is valid");
        assert!(target.x <= budget.max_view_dimension);
        assert!(target.y <= budget.max_view_dimension);
        assert!(u64::from(target.x) * u64::from(target.y) <= budget.max_view_pixels);
        assert!(bounded_view_size(
            UVec2::new(800, 600),
            &UsdPreviewRenderBudget {
                max_view_dimension: 0,
                max_view_pixels: 1,
                max_total_pixels: 1,
            },
        )
        .is_none());
    }

    #[test]
    fn views_share_projection_and_keep_presentation_isolated() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>();
        let first = create_preview_session(
            app.world_mut(),
            UsdPreviewId(1),
            DocumentId::new(1),
            LayerId::root(),
            Handle::default(),
            FIRST_PREVIEW_RENDER_LAYER,
            UsdPreviewViewId(1),
        )
        .expect("session resources are available");
        let root = first.scene_root();
        let layer = first.render_layer();
        let mut state = UsdViewportState::default();
        state.insert(first);
        let first_view = create_preview_view(
            app.world_mut(),
            UsdPreviewId(1),
            UsdPreviewViewId(1),
            layer,
            1_000.0,
        )
        .expect("first view resources are available");
        let second_view = create_preview_view(
            app.world_mut(),
            UsdPreviewId(1),
            UsdPreviewViewId(2),
            layer,
            1_000.0,
        )
        .expect("second view resources are available");
        let first_camera = first_view.camera();
        let second_camera = second_view.camera();
        let first_image = first_view.image().clone();
        let second_image = second_view.image().clone();
        assert!(state.insert_view(first_view).is_ok());
        assert!(state.insert_view(second_view).is_ok());

        let session = state.session(UsdPreviewId(1)).expect("session is retained");
        assert_eq!(session.scene_root(), root);
        assert_eq!(session.render_layer(), layer);
        assert_ne!(first_camera, second_camera);
        assert_ne!(first_image, second_image);
        assert_eq!(state.view_count(), 2);
        assert_eq!(state.focused_view_id(), Some(UsdPreviewViewId(1)));
        assert!(state.focus_view(UsdPreviewViewId(2)));
        assert_eq!(state.focused_view_id(), Some(UsdPreviewViewId(2)));

        let (_session, views) = state.remove(UsdPreviewId(1)).expect("session can close");
        assert_eq!(views.len(), 2);
        assert_eq!(state.session_count(), 0);
        assert_eq!(state.view_count(), 0);
        assert_eq!(state.focused_view_id(), None);
    }
}
