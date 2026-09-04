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
use lunco_api::executor::{finish_command_result, PendingApiRequest};
use lunco_api::queries::ApiQueryProvider;
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_assets::twin_source::TwinRoots;
use lunco_core::{on_command, register_commands, Ack, ActiveCommandId, Command, OpId};
use lunco_doc::{Document, DocumentId, DocumentOrigin};
use lunco_doc_bevy::DocumentClosed;
#[cfg(test)]
use lunco_render::SceneCamera;
use lunco_render::{
    scene_camera_look_with_profile, GraphicsCameraDefaults, LightGraphicsDefaults,
    RenderQualityProfile, RenderingQualitySettings,
};
use lunco_usd_bevy::{
    PendingUsdMesh, SdfPath, UsdAwaitingStage, UsdPreviewOnly, UsdPrimPath, UsdStageAsset,
    UsdStageRevision, UsdVisualMeshPending, UsdVisualProjectionQueued, UsdVisualSyncFailed,
    UsdVisualSynced,
};
use lunco_workbench::{
    CloseTab, InstancePanel, OpenTab, Panel, PanelCtx, PanelId, PanelRects, PanelScrollPolicy,
    PanelSlot, PendingTabCloses, ScenePickGate, SceneTarget, TabId, WorkbenchAppExt,
};
use lunco_workspace::{document_belongs_to_twin_root, TwinClosed, WorkspaceResource};

use crate::document::{LayerId, UsdDocument};
use lunco_doc_bevy::DocumentRegistry;

use std::collections::{HashMap, HashSet};

/// Stable id of the workbench tab the viewport renders into.
pub const USD_VIEWPORT_PANEL_ID: PanelId = PanelId("usd::viewport");

/// Instance-panel kind for additional views over an existing USD preview
/// session. The instance value is [`UsdPreviewViewId::0`].
pub const USD_PREVIEW_VIEW_PANEL_ID: PanelId = PanelId("usd::preview_view");

/// Read-only query for the exact USD presentation state currently open in the
/// Assembly Editor.
///
/// Unlike document inspection, this query is session-scoped: it reports every
/// explicit preview lease and its independent presentation views, plus the
/// focused preview/view pair. It never infers a document from a tab title, a
/// filesystem name, or the live simulation scene.
pub struct InspectUsdViewportProvider;

impl ApiQueryProvider for InspectUsdViewportProvider {
    fn name(&self) -> &'static str {
        "InspectUsdViewport"
    }

    fn execute(&self, world: &World, _params: &serde_json::Value) -> ApiResponse {
        let Some(viewport) = world.get_resource::<UsdViewportState>() else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "InspectUsdViewport requires UsdViewportPlugin",
            );
        };

        let mut previews: Vec<_> = viewport
            .sessions()
            .map(|session| {
                let mut views: Vec<_> = viewport
                    .views()
                    .filter(|view| view.preview() == session.id())
                    .map(|view| {
                        serde_json::json!({
                            "view": view.id().0,
                            "focused": viewport.focused_view_id() == Some(view.id()),
                            "projection": view.projection().as_str(),
                            "target": view.orbit().target.to_array(),
                            "distance": view.orbit().distance,
                            "orthographic_scale": view.orthographic_scale(),
                        })
                    })
                    .collect();
                views.sort_by_key(|view| {
                    view.get("view")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or_default()
                });
                serde_json::json!({
                    "preview": session.id().0,
                    "doc": session.doc(),
                    "edit_target": session.edit_target().as_str(),
                    "projected_generation": session.projected_generation(),
                    "projection_ready": session.projection_ready(),
                    "explode": session.explode.as_ref().map(|explode| {
                        serde_json::json!({
                            "assembly": explode.assembly,
                            "parts": explode.parts.iter().map(|part| &part.path).collect::<Vec<_>>(),
                            "axis": explode.axis.as_str(),
                            "spacing": explode.spacing,
                        })
                    }),
                    "focused": viewport.focused_preview_id() == Some(session.id()),
                    "views": views,
                })
            })
            .collect();
        previews.sort_by_key(|preview| {
            preview
                .get("preview")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default()
        });

        ApiResponse::ok(serde_json::json!({
            "focused_preview": viewport.focused_preview_id().map(|id| id.0),
            "focused_view": viewport.focused_view_id().map(|id| id.0),
            "previews": previews,
            "preview_count": viewport.session_count(),
            "view_count": viewport.view_count(),
        }))
    }
}

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
        app.add_observer(on_browser_usd_document_ready);
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
                reconcile_preview_projection_state
                    .run_if(preview_projection_inputs_changed)
                    .after(lunco_usd_bevy::UsdVisualProjectionSet),
            ),
        );
        register_all_commands(app);
    }
}

/// Bind a browser-admitted document to the one editor preview lease and focus
/// the USD viewport. The document and root edit target remain explicit, while
/// repeated clicks reuse the same `EDITOR_PREVIEW_ID` by the lease contract.
fn on_browser_usd_document_ready(
    trigger: On<crate::commands::BrowserUsdDocumentReady>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<Res<WorkspaceResource>>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;
    // Keep USD's two editor surfaces paired: the native 3D preview is the
    // composed/edit-target view, while the source tab is the lossless USDA
    // text view used for inspection and explicit text edits. Resolve the
    // source tab from the document's canonical origin instead of from a tab
    // title or the active scene path.
    if let (Some(path), Some(workspace)) = (
        registry
            .host(doc)
            .and_then(|host| host.document().origin().canonical_path())
            .map(std::path::Path::to_path_buf),
        workspace.as_deref(),
    ) {
        if let Some(root) = workspace
            .twins()
            .map(|(_, twin)| &twin.root)
            .filter(|root| path.strip_prefix(root).is_ok())
            .max_by_key(|root| root.components().count())
        {
            if let Ok(relative) = path.strip_prefix(root) {
                commands.trigger(lunco_workbench::OpenTwinSource {
                    twin_root: root.to_string_lossy().into_owned(),
                    relative_path: relative.to_string_lossy().into_owned(),
                    pinned: false,
                });
            }
        }
    }
    commands.trigger(OpenUsdPreview {
        preview: EDITOR_PREVIEW_ID,
        doc,
        edit_target: LayerId::root(),
    });
    commands.trigger(lunco_workbench::FocusPanel {
        id: USD_VIEWPORT_PANEL_ID.0.to_string(),
    });
}

/// Pointer-driven orbit camera (CAD-style preview). Anchored on a `target`
/// point in scene space; primary-drag pans, secondary-drag orbits,
/// middle-drag pans, and scroll zooms. The camera state is presentation state owned by one
/// [`UsdPreviewView`], never by the projected USD stage.
#[derive(Debug, Clone)]
pub struct OrbitCamera {
    /// Yaw rotation around +Y (radians).
    pub yaw: f32,
    /// Pitch rotation up/down (radians); clamped to avoid gimbal flip.
    pub pitch: f32,
    /// Distance from target. Scroll wheel scales it geometrically.
    pub distance: f32,
    /// Point the camera orbits around.
    pub target: Vec3,
    /// Radians per drag-pixel for yaw + pitch.
    pub drag_sensitivity: f32,
    /// Fractional distance change per scroll unit (0.001 ≈ 0.1% per px).
    pub zoom_sensitivity: f32,
    /// Lower/upper clamps on `distance` so the user can't fly into
    /// the target or out to infinity.
    pub min_distance: f32,
    pub max_distance: f32,
    /// Lower/upper clamps on orthographic view scale.
    pub min_orthographic_scale: f32,
    pub max_orthographic_scale: f32,
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
            min_orthographic_scale: 0.01,
            max_orthographic_scale: 5_000.0,
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

    /// Pan the orbit target in the camera's screen plane.
    ///
    /// `delta` and `viewport_size` are egui logical points. The conversion is
    /// derived from the active presentation projection, so perspective pans
    /// track the orbit target at any distance and orthographic pans remain
    /// independent of the orbit distance. A positive screen-Y delta moves the
    /// target up so the rendered assembly follows the pointer downward.
    pub fn apply_pan(
        &mut self,
        delta: egui::Vec2,
        viewport_size: Vec2,
        projection: &Projection,
        mode: UsdPreviewProjection,
        orthographic_scale: f32,
    ) -> bool {
        let Some(world_delta) =
            self.pan_delta(delta, viewport_size, projection, mode, orthographic_scale)
        else {
            return false;
        };
        self.target += world_delta;
        true
    }

    fn pan_delta(
        &self,
        delta: egui::Vec2,
        viewport_size: Vec2,
        projection: &Projection,
        mode: UsdPreviewProjection,
        orthographic_scale: f32,
    ) -> Option<Vec3> {
        if !delta.is_finite()
            || !viewport_size.is_finite()
            || viewport_size.x <= f32::EPSILON
            || viewport_size.y <= f32::EPSILON
        {
            return None;
        }

        let aspect_ratio = viewport_size.x / viewport_size.y;
        let vertical_extent = match (mode, projection) {
            (UsdPreviewProjection::Perspective, Projection::Perspective(projection)) => {
                if !projection.fov.is_finite()
                    || projection.fov <= 0.0
                    || projection.fov >= std::f32::consts::PI
                    || !self.distance.is_finite()
                    || self.distance <= 0.0
                {
                    return None;
                }
                2.0 * self.distance * (projection.fov * 0.5).tan()
            }
            (UsdPreviewProjection::Orthographic, Projection::Orthographic(_)) => {
                if !orthographic_scale.is_finite() || orthographic_scale <= 0.0 {
                    return None;
                }
                2.0 * orthographic_scale
            }
            _ => return None,
        };
        let horizontal_extent = vertical_extent * aspect_ratio;
        let transform = self.transform();
        let right = transform.rotation * Vec3::X;
        let up = transform.rotation * Vec3::Y;
        Some(
            -right * (delta.x * horizontal_extent / viewport_size.x)
                + up * (delta.y * vertical_extent / viewport_size.y),
        )
    }

    /// Return the multiplicative zoom factor represented by one scroll delta.
    /// Orthographic views use the same input curve on their projection scale.
    pub fn zoom_factor(&self, scroll_y: f32) -> f32 {
        (1.0 - scroll_y * self.zoom_sensitivity).clamp(0.1, 10.0)
    }

    /// Apply a scroll delta (vertical scroll wheel, pixels).
    pub fn apply_zoom(&mut self, scroll_y: f32) {
        let factor = self.zoom_factor(scroll_y);
        self.distance = (self.distance * factor).clamp(self.min_distance, self.max_distance);
    }

    /// Build the transform the camera entity should carry this frame.
    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.position()).looking_at(self.target, Vec3::Y)
    }
}

/// Projection mode of one USD preview view. This is a presentation choice;
/// authored `UsdGeomCamera` projection remains owned by the document and is
/// never rewritten by an editor camera gesture.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Reflect, serde::Serialize, serde::Deserialize,
)]
#[reflect(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsdPreviewProjection {
    #[default]
    Perspective,
    Orthographic,
}

impl UsdPreviewProjection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Perspective => "perspective",
            Self::Orthographic => "orthographic",
        }
    }
}

/// Operation applied to the transient presentation pose of a USD preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsdPreviewExplodeAction {
    Enable,
    Update,
    Reset,
}

impl UsdPreviewExplodeAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::Update => "update",
            Self::Reset => "reset",
        }
    }
}

/// Principal axis of an explode operation in the selected assembly's local
/// coordinate frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum UsdPreviewExplodeAxis {
    X,
    Y,
    Z,
}

impl UsdPreviewExplodeAxis {
    fn as_str(self) -> &'static str {
        match self {
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
        }
    }

    fn vector(self) -> Vec3 {
        match self {
            Self::X => Vec3::X,
            Self::Y => Vec3::Y,
            Self::Z => Vec3::Z,
        }
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
    projection_ready: bool,
    primary_view: UsdPreviewViewId,
    explode: Option<UsdPreviewExplodeState>,
}

/// The complete baseline for one session's transient explode presentation.
/// Baselines are captured before the first offset is applied and are never
/// derived from an already-exploded pose.
#[derive(Clone)]
struct UsdPreviewExplodeState {
    assembly: String,
    parts: Vec<UsdPreviewExplodedPart>,
    axis: UsdPreviewExplodeAxis,
    spacing: f32,
    assembly_to_root: Mat4,
}

#[derive(Clone)]
struct UsdPreviewExplodedPart {
    path: String,
    entity: Entity,
    baseline: Transform,
    parent_to_root: Mat4,
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

    /// Whether the current document generation has completed the preview's
    /// structural USD projection and all queued CPU mesh work.
    pub fn projection_ready(&self) -> bool {
        self.projection_ready
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
    projection: UsdPreviewProjection,
    orthographic_scale: f32,
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

    pub fn projection(&self) -> UsdPreviewProjection {
        self.projection
    }

    pub fn orthographic_scale(&self) -> f32 {
        self.orthographic_scale
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

    /// Invalidate preview presentation before a document generation is
    /// projected. A stale generation must not remain editable while its USD
    /// entities are being rebuilt.
    pub(crate) fn invalidate_projection(&mut self, doc: DocumentId) -> Vec<(Entity, Transform)> {
        let mut restores = Vec::new();
        for session in self
            .sessions
            .values_mut()
            .filter(|session| session.doc == doc)
        {
            if let Some(explode) = session.explode.take() {
                restores.extend(
                    explode
                        .parts
                        .into_iter()
                        .map(|part| (part.entity, part.baseline)),
                );
            }
            session.projected_generation = 0;
            session.projection_ready = false;
        }
        restores
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
/// chrome pick gate; this event records the view-specific scene hit and render
/// target footprint, then marks that camera visible for the current frame.
#[derive(Event, Clone, Copy, Debug)]
struct UsdPreviewViewMeasured {
    view: UsdPreviewViewId,
    over_scene: bool,
}

/// Return true when a preview's authoritative USD projection inputs changed.
///
/// The readiness reconciler is not a render-loop scan. It wakes for session
/// lifecycle changes and for the same USD queue/mesh markers owned by
/// `lunco-usd-bevy`; removal readers are drained here so a completed async mesh
/// also wakes the state transition.
fn preview_projection_inputs_changed(
    revision: Option<Res<UsdStageRevision>>,
    changed: Query<
        (),
        Or<(
            Added<UsdPrimPath>,
            Changed<UsdPrimPath>,
            Added<UsdVisualSynced>,
            Changed<UsdVisualSynced>,
            Added<UsdAwaitingStage>,
            Changed<UsdAwaitingStage>,
            Added<UsdVisualProjectionQueued>,
            Changed<UsdVisualProjectionQueued>,
            Added<UsdVisualMeshPending>,
            Changed<UsdVisualMeshPending>,
            Added<UsdVisualSyncFailed>,
            Changed<UsdVisualSyncFailed>,
        )>,
    >,
    mut removed_paths: RemovedComponents<UsdPrimPath>,
    mut removed_awaiting: RemovedComponents<UsdAwaitingStage>,
    mut removed_queued: RemovedComponents<UsdVisualProjectionQueued>,
    mut removed_meshes: RemovedComponents<UsdVisualMeshPending>,
    mut removed_failures: RemovedComponents<UsdVisualSyncFailed>,
) -> bool {
    // `UsdViewportState` also stores camera orbit/focus data. Its change tick
    // must not wake this structural scan when a user merely navigates a view.
    // The existing USD projection revision is the event-like signal for live
    // stage edits; marker changes cover the bounded queue and async mesh fence.
    let removed_paths = removed_paths.read().next().is_some();
    let removed_awaiting = removed_awaiting.read().next().is_some();
    let removed_queued = removed_queued.read().next().is_some();
    let removed_meshes = removed_meshes.read().next().is_some();
    let removed_failures = removed_failures.read().next().is_some();
    revision.is_some_and(|revision| revision.is_changed())
        || !changed.is_empty()
        || removed_paths
        || removed_awaiting
        || removed_queued
        || removed_meshes
        || removed_failures
}

/// Reconcile each preview lease's generation against the completed USD visual
/// projection. Document sync owns generation changes; this UI owner only marks
/// a generation ready after the preview root and every projected descendant
/// have cleared the USD visual queue and CPU mesh phase.
fn reconcile_preview_projection_state(
    mut state: ResMut<UsdViewportState>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    roots: Query<
        (
            Entity,
            &UsdPrimPath,
            Has<UsdVisualSynced>,
            Has<UsdVisualSyncFailed>,
            Has<UsdAwaitingStage>,
            Has<UsdVisualProjectionQueued>,
            Has<UsdVisualMeshPending>,
            Has<PendingUsdMesh>,
        ),
        With<UsdPreviewOnly>,
    >,
    prims: Query<(
        Entity,
        &UsdPrimPath,
        Has<UsdVisualSynced>,
        Has<UsdAwaitingStage>,
        Has<UsdVisualProjectionQueued>,
        Has<UsdVisualMeshPending>,
        Has<PendingUsdMesh>,
        Has<UsdVisualSyncFailed>,
    )>,
    parents: Query<&ChildOf>,
) {
    let sessions: Vec<_> = state
        .sessions()
        .map(|session| {
            (
                session.id(),
                session.doc(),
                session.scene_root(),
                session.stage_handle().id(),
            )
        })
        .collect();

    for (preview, doc, root, stage_id) in sessions {
        let root_ready = roots.get(root).is_ok_and(
            |(_, path, synced, failed, awaiting, queued, mesh_pending, pending_mesh)| {
                path.stage_handle.id() == stage_id
                    && synced
                    && !failed
                    && !awaiting
                    && !queued
                    && !mesh_pending
                    && !pending_mesh
            },
        );
        let descendants_ready = root_ready
            && prims
                .iter()
                // A preview normally reuses the active Twin's deduplicated
                // stage asset. Restrict the fence to this preview root before
                // checking readiness; entities from the live Twin have the
                // same stage id but are outside this session's ownership.
                .filter(|(entity, path, ..)| {
                    path.stage_handle.id() == stage_id
                        && preview_entity_belongs_to_root(*entity, root, &parents)
                })
                .all(
                    |(_, _, synced, awaiting, queued, mesh_pending, pending_mesh, failed)| {
                        synced && !awaiting && !queued && !mesh_pending && !pending_mesh && !failed
                    },
                );
        let generation = registry.host(doc).map(|host| host.document().generation());
        let Some(session) = state.session_mut(preview) else {
            continue;
        };
        if descendants_ready {
            if let Some(generation) = generation {
                if !session.projection_ready || session.projected_generation != generation {
                    session.projected_generation = generation;
                    session.projection_ready = true;
                }
            }
        } else {
            session.projection_ready = false;
            session.projected_generation = 0;
        }
    }
}

fn preview_entity_belongs_to_root(entity: Entity, root: Entity, parents: &Query<&ChildOf>) -> bool {
    if entity == root {
        return true;
    }
    let mut current = entity;
    for _ in 0..1024 {
        let Ok(parent) = parents.get(current) else {
            return false;
        };
        current = parent.parent();
        if current == root {
            return true;
        }
    }
    false
}

/// Pointer input emitted by the viewport panel. Camera state and the camera
/// entity are updated by the observer, outside the egui paint borrow.
#[derive(Event, Clone, Copy, Debug)]
struct UsdViewportOrbitInput {
    view: UsdPreviewViewId,
    drag: egui::Vec2,
    pan: egui::Vec2,
    viewport_size: egui::Vec2,
    scroll_y: f32,
}

/// Resolve the preview's pointer buttons into the shared View interaction
/// contract: left/middle drag pans and right drag orbits. Shift keeps the
/// explicit pan chord available when the secondary button is used.
fn preview_drag_channels(
    primary: bool,
    middle: bool,
    secondary: bool,
    shift: bool,
) -> (bool, bool) {
    let pan = primary || middle || (secondary && shift);
    let orbit = secondary && !pan;
    (orbit, pan)
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
    mut gate: ResMut<ScenePickGate>,
    mut visibility: ResMut<UsdPreviewFrameVisibility>,
    budget: Res<UsdPreviewRenderBudget>,
    mut cameras: Query<&mut Camera>,
) {
    let event = trigger.event();
    gate.record_scene_leaf(
        SceneTarget::Offscreen(USD_VIEWPORT_PANEL_ID),
        event.over_scene,
    );
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
    mut cameras: Query<(&mut Transform, &mut Projection)>,
) {
    let input = trigger.event();
    if input.drag == egui::Vec2::ZERO && input.pan == egui::Vec2::ZERO && input.scroll_y == 0.0 {
        return;
    }
    let Some(camera) = state.view(input.view).map(|view| view.camera) else {
        return;
    };
    let Ok((mut transform, mut projection)) = cameras.get_mut(camera) else {
        return;
    };
    let Some(view) = state.view_mut(input.view) else {
        return;
    };
    if input.pan != egui::Vec2::ZERO
        && !view.orbit.apply_pan(
            input.pan,
            Vec2::new(input.viewport_size.x, input.viewport_size.y),
            &projection,
            view.projection,
            view.orthographic_scale,
        )
    {
        return;
    }
    if input.drag != egui::Vec2::ZERO {
        view.orbit.apply_drag(input.drag);
    }
    if input.scroll_y != 0.0 {
        match view.projection {
            UsdPreviewProjection::Perspective => view.orbit.apply_zoom(input.scroll_y),
            UsdPreviewProjection::Orthographic => {
                let factor = view.orbit.zoom_factor(input.scroll_y);
                view.orthographic_scale = (view.orthographic_scale * factor).clamp(
                    view.orbit.min_orthographic_scale,
                    view.orbit.max_orthographic_scale,
                );
            }
        }
    }
    view.auto_frame = false;
    *transform = view.orbit.transform();
    if let Projection::Orthographic(projection) = &mut *projection {
        projection.scale = view.orthographic_scale;
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
        projection_ready: false,
        primary_view,
        explode: None,
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
    profile: RenderQualityProfile,
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
    // Preview cameras use the same renderer-owned look as an unauthored scene
    // camera. The explicit profile supplies the exposure that matches the
    // graphics light; relying on Bevy's implicit exposure against the canonical
    // lunar sun overexposes the USD assembly before any authored camera opinion.
    let (camera_intent, exposure) = scene_camera_look_with_profile(None, profile);
    let mut commands = world.commands();
    let camera = commands
        .spawn((
            camera_intent,
            exposure,
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
                illuminance: profile.distant_light_default_illuminance,
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
        projection: UsdPreviewProjection::default(),
        orthographic_scale: 1.0,
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
    mut q_cameras: Query<(&mut Transform, &mut Projection)>,
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
        let Ok((mut transform, mut projection)) = q_cameras.get_mut(camera) else {
            continue;
        };
        let Some(view) = state.view_mut(view_id) else {
            continue;
        };
        let distance = match (&mut *projection, view.projection) {
            (Projection::Perspective(projection), UsdPreviewProjection::Perspective) => {
                let vertical = (projection.fov * 0.5).tan();
                let horizontal = vertical * projection.aspect_ratio.max(f32::EPSILON);
                let half_fov_tangent = vertical.min(horizontal).max(f32::EPSILON);
                (radius / half_fov_tangent * 1.2)
                    .clamp(view.orbit.min_distance, view.orbit.max_distance)
            }
            (Projection::Orthographic(projection), UsdPreviewProjection::Orthographic) => {
                projection.scaling_mode = bevy::camera::ScalingMode::FixedVertical {
                    viewport_height: 2.0,
                };
                view.orthographic_scale = (radius * 1.2).max(0.01);
                projection.scale = view.orthographic_scale;
                (radius * 3.0).clamp(view.orbit.min_distance, view.orbit.max_distance)
            }
            _ => continue,
        };
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

fn preview_projection(mode: UsdPreviewProjection, orthographic_scale: f32) -> Projection {
    match mode {
        UsdPreviewProjection::Perspective => lunco_render::usd_default_perspective_projection(),
        UsdPreviewProjection::Orthographic => {
            Projection::Orthographic(bevy::camera::OrthographicProjection {
                near: 0.1,
                far: 1.0e6,
                scaling_mode: bevy::camera::ScalingMode::FixedVertical {
                    viewport_height: 2.0,
                },
                scale: orthographic_scale,
                ..bevy::camera::OrthographicProjection::default_3d()
            })
        }
    }
}

fn validated_preview_profile(world: &World) -> Result<RenderQualityProfile, String> {
    if !world.contains_resource::<Assets<Image>>() {
        return Err("USD preview rendering is unavailable in this host".to_string());
    }
    let settings = world
        .get_resource::<RenderingQualitySettings>()
        .ok_or_else(|| "Graphics quality settings are unavailable in this host".to_string())?;
    settings
        .validated_profile()
        .map_err(|reason| format!("invalid Graphics quality: {reason}"))
}

// ─────────────────────────────────────────────────────────────────────
// Preview session commands
// ─────────────────────────────────────────────────────────────────────

/// Open one explicit document and authored edit target in an isolated preview
/// session. Reopening the same `preview` id for its current document focuses
/// and updates that lease in place; another document replaces only that
/// explicit lease. Other sessions keep their roots, cameras, and stages
/// untouched.
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

/// Change the projection of one isolated USD preview view. This changes only
/// the editor camera; authored USD camera opinions stay read-only presentation
/// input and are never rewritten by a navigation gesture.
#[Command]
pub struct SetUsdPreviewProjection {
    pub view: UsdPreviewViewId,
    pub projection: UsdPreviewProjection,
}

/// Fit one preview view to the projected visual bounds of its USD stage.
#[Command]
pub struct FrameUsdPreviewView {
    pub view: UsdPreviewViewId,
}

/// Restore one preview view's default orbit pose and fit it to its stage.
#[Command]
pub struct ResetUsdPreviewView {
    pub view: UsdPreviewViewId,
}

/// Pan one preview view in egui logical screen points. The view converts the
/// delta to its camera plane using the current projection and render-target
/// viewport.
#[Command]
pub struct PanUsdPreviewView {
    pub view: UsdPreviewViewId,
    pub delta: [f32; 2],
}

/// Zoom one preview view by a positive multiplicative factor. Perspective
/// views change orbit distance; orthographic views change projection scale.
#[Command]
pub struct ZoomUsdPreviewView {
    pub view: UsdPreviewViewId,
    pub factor: f32,
}

/// Apply a transient, session-scoped explode pose to an explicit USD preview.
/// This command changes only projected Bevy transforms; it never enters the
/// USD document, journal, save state, or simulation projection.
#[Command]
pub struct ExplodeUsdPreview {
    pub preview: UsdPreviewId,
    pub doc: DocumentId,
    /// Exact composed `kind = "assembly"` prim path.
    pub assembly: String,
    /// Exact composed prim paths below `assembly`. Rust sorts these paths for
    /// stable offsets, so repeated calls do not depend on caller ordering.
    pub parts: Vec<String>,
    pub action: UsdPreviewExplodeAction,
    /// Required for `enable` and `update`; `null` is accepted for `reset`.
    #[serde(default)]
    #[reflect(default)]
    pub axis: Option<UsdPreviewExplodeAxis>,
    /// Required for `enable` and `update`; `null` is accepted for `reset`.
    #[serde(default)]
    #[reflect(default)]
    pub spacing: Option<f32>,
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
        if world
            .resource::<UsdViewportState>()
            .session(preview)
            .is_some_and(|session| session.doc() == doc)
        {
            let mut state = world.resource_mut::<UsdViewportState>();
            if let Some(session) = state.session_mut(preview) {
                session.edit_target = edit_target;
            }
            state.focus(preview);
            return;
        }
        let preview_profile = match validated_preview_profile(world) {
            Ok(profile) => profile,
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
        let Some(view) =
            create_preview_view(world, preview, primary_view, render_layer, preview_profile)
        else {
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
        let profile = match validated_preview_profile(world) {
            Ok(profile) => profile,
            Err(detail) => {
                report_preview_error(world, "usd-preview-view-open-failed", detail);
                return;
            }
        };
        let Some(view_state) = create_preview_view(world, preview, view, render_layer, profile)
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

#[on_command(SetUsdPreviewProjection)]
fn on_set_usd_preview_projection(trigger: On<SetUsdPreviewProjection>, mut commands: Commands) {
    let command = trigger.event();
    let view = command.view;
    let projection = command.projection;
    commands.queue(move |world: &mut World| {
        let (camera, scale) = {
            let mut viewport = world.resource_mut::<UsdViewportState>();
            let Some(view_state) = viewport.view_mut(view) else {
                drop(viewport);
                report_preview_error(
                    world,
                    "usd-preview-projection-failed",
                    format!("view {} is not open", view.0),
                );
                return;
            };
            view_state.projection = projection;
            view_state.auto_frame = true;
            (view_state.camera, view_state.orthographic_scale)
        };
        let Some(mut camera_projection) = world.get_mut::<Projection>(camera) else {
            report_preview_error(
                world,
                "usd-preview-projection-failed",
                format!("view {} camera is unavailable", view.0),
            );
            return;
        };
        *camera_projection = preview_projection(projection, scale);
    });
}

#[on_command(FrameUsdPreviewView)]
fn on_frame_usd_preview_view(trigger: On<FrameUsdPreviewView>, mut commands: Commands) {
    let view = trigger.event().view;
    commands.queue(move |world: &mut World| {
        let mut viewport = world.resource_mut::<UsdViewportState>();
        let Some(view_state) = viewport.view_mut(view) else {
            drop(viewport);
            report_preview_error(
                world,
                "usd-preview-frame-failed",
                format!("view {} is not open", view.0),
            );
            return;
        };
        view_state.auto_frame = true;
    });
}

#[on_command(ResetUsdPreviewView)]
fn on_reset_usd_preview_view(trigger: On<ResetUsdPreviewView>, mut commands: Commands) {
    let view = trigger.event().view;
    commands.queue(move |world: &mut World| {
        let (camera, mode, scale) = {
            let mut viewport = world.resource_mut::<UsdViewportState>();
            let Some(view_state) = viewport.view_mut(view) else {
                drop(viewport);
                report_preview_error(
                    world,
                    "usd-preview-reset-failed",
                    format!("view {} is not open", view.0),
                );
                return;
            };
            view_state.orbit = OrbitCamera::default();
            view_state.orthographic_scale = 1.0;
            view_state.auto_frame = true;
            (
                view_state.camera,
                view_state.projection,
                view_state.orthographic_scale,
            )
        };
        if let Some(mut transform) = world.get_mut::<Transform>(camera) {
            *transform = OrbitCamera::default().transform();
        }
        if let Some(mut camera_projection) = world.get_mut::<Projection>(camera) {
            *camera_projection = preview_projection(mode, scale);
        }
    });
}

#[on_command(PanUsdPreviewView)]
fn on_pan_usd_preview_view(trigger: On<PanUsdPreviewView>, mut commands: Commands) {
    let command = trigger.event();
    let view = command.view;
    let delta = command.delta;
    commands.queue(move |world: &mut World| {
        if !delta.iter().all(|value| value.is_finite()) {
            report_preview_error(
                world,
                "usd-preview-pan-failed",
                format!("view {} received a non-finite pan delta", view.0),
            );
            return;
        }
        let Some((camera, mode, orthographic_scale)) = world
            .resource::<UsdViewportState>()
            .view(view)
            .map(|view_state| {
                (
                    view_state.camera,
                    view_state.projection,
                    view_state.orthographic_scale,
                )
            })
        else {
            report_preview_error(
                world,
                "usd-preview-pan-failed",
                format!("view {} is not open", view.0),
            );
            return;
        };
        let Some(viewport_size) = world
            .get::<Camera>(camera)
            .and_then(|camera| camera.logical_viewport_size())
        else {
            report_preview_error(
                world,
                "usd-preview-pan-failed",
                format!("view {} camera viewport is unavailable", view.0),
            );
            return;
        };
        let Some(projection) = world.get::<Projection>(camera).cloned() else {
            report_preview_error(
                world,
                "usd-preview-pan-failed",
                format!("view {} camera projection is unavailable", view.0),
            );
            return;
        };
        let applied = {
            let mut viewport = world.resource_mut::<UsdViewportState>();
            let view_state = viewport
                .view_mut(view)
                .expect("preview view remains registered");
            let applied = view_state.orbit.apply_pan(
                egui::Vec2::new(delta[0], delta[1]),
                viewport_size,
                &projection,
                mode,
                orthographic_scale,
            );
            if applied {
                view_state.auto_frame = false;
            }
            applied
        };
        if !applied {
            report_preview_error(
                world,
                "usd-preview-pan-failed",
                format!("view {} received an invalid pan geometry", view.0),
            );
            return;
        };
        let transform = world
            .resource::<UsdViewportState>()
            .view(view)
            .expect("preview view remains registered")
            .orbit()
            .transform();
        if let Some(mut target_transform) = world.get_mut::<Transform>(camera) {
            *target_transform = transform;
        } else {
            report_preview_error(
                world,
                "usd-preview-pan-failed",
                format!("view {} camera is unavailable", view.0),
            );
        }
    });
}

#[on_command(ZoomUsdPreviewView)]
fn on_zoom_usd_preview_view(trigger: On<ZoomUsdPreviewView>, mut commands: Commands) {
    let command = trigger.event();
    let view = command.view;
    let factor = command.factor;
    commands.queue(move |world: &mut World| {
        if !factor.is_finite() || factor <= 0.0 {
            report_preview_error(
                world,
                "usd-preview-zoom-failed",
                format!("view {} received an invalid zoom factor", view.0),
            );
            return;
        }
        let (camera, scale) = {
            let mut viewport = world.resource_mut::<UsdViewportState>();
            let Some(view_state) = viewport.view_mut(view) else {
                drop(viewport);
                report_preview_error(
                    world,
                    "usd-preview-zoom-failed",
                    format!("view {} is not open", view.0),
                );
                return;
            };
            match view_state.projection {
                UsdPreviewProjection::Perspective => {
                    view_state.orbit.distance = (view_state.orbit.distance * factor)
                        .clamp(view_state.orbit.min_distance, view_state.orbit.max_distance);
                }
                UsdPreviewProjection::Orthographic => {
                    view_state.orthographic_scale = (view_state.orthographic_scale * factor).clamp(
                        view_state.orbit.min_orthographic_scale,
                        view_state.orbit.max_orthographic_scale,
                    );
                }
            }
            view_state.auto_frame = false;
            (view_state.camera, view_state.orthographic_scale)
        };
        let transform = world
            .resource::<UsdViewportState>()
            .view(view)
            .expect("preview view remains registered")
            .orbit()
            .transform();
        if let Some(mut target_transform) = world.get_mut::<Transform>(camera) {
            *target_transform = transform;
        } else {
            report_preview_error(
                world,
                "usd-preview-zoom-failed",
                format!("view {} camera is unavailable", view.0),
            );
            return;
        }
        if let Some(mut projection) = world.get_mut::<Projection>(camera) {
            if let Projection::Orthographic(projection) = &mut *projection {
                projection.scale = scale;
            }
        }
    });
}

#[derive(Clone)]
struct PreviewPrimSnapshot {
    entity: Entity,
    path: String,
    parent: Option<Entity>,
    local: Transform,
    kind: Option<String>,
    synced: bool,
}

fn preview_entity_reaches_root(
    entity: Entity,
    root: Entity,
    parents: &HashMap<Entity, Entity>,
) -> bool {
    let mut current = entity;
    for _ in 0..64 {
        if current == root {
            return true;
        }
        let Some(parent) = parents.get(&current) else {
            return false;
        };
        current = *parent;
    }
    false
}

fn preview_local_to_root(
    entity: Entity,
    root: Entity,
    parents: &HashMap<Entity, Entity>,
    locals: &HashMap<Entity, Transform>,
) -> Option<Mat4> {
    let mut chain = Vec::new();
    let mut current = entity;
    for _ in 0..64 {
        chain.push(locals.get(&current)?.to_matrix());
        if current == root {
            let mut result = Mat4::IDENTITY;
            for local in chain.into_iter().rev() {
                result *= local;
            }
            return Some(result);
        }
        current = *parents.get(&current)?;
    }
    None
}

fn finite_transform(transform: &Transform) -> bool {
    transform.translation.is_finite()
        && transform.rotation.is_finite()
        && transform.scale.is_finite()
}

fn finite_matrix(matrix: Mat4) -> bool {
    matrix.to_cols_array().iter().all(|value| value.is_finite())
}

fn canonical_explode_parts(parts: &[String]) -> Result<Vec<String>, String> {
    if parts.is_empty() {
        return Err("USD preview explode requires at least one part path".to_string());
    }
    let mut canonical = parts.to_vec();
    if canonical.iter().any(|path| {
        path.trim() != path || path.is_empty() || !path.starts_with('/') || path.ends_with('/')
    }) {
        return Err("USD preview explode part paths must be absolute prim paths".to_string());
    }
    canonical.sort();
    if canonical.windows(2).any(|paths| paths[0] == paths[1]) {
        return Err("USD preview explode part paths must be unique".to_string());
    }
    for path in &canonical {
        SdfPath::new(path)
            .map_err(|error| format!("invalid explode part path `{path}`: {error}"))?;
    }
    Ok(canonical)
}

fn collect_preview_explode_targets(
    world: &mut World,
    command: &ExplodeUsdPreview,
) -> Result<
    (
        Entity,
        Entity,
        String,
        Vec<PreviewPrimSnapshot>,
        HashMap<Entity, Transform>,
        HashMap<Entity, Entity>,
    ),
    String,
> {
    let (root, stage_id, ready) = {
        let viewport = world
            .get_resource::<UsdViewportState>()
            .ok_or_else(|| "USD preview viewport state is unavailable".to_string())?;
        let session = viewport
            .session(command.preview)
            .ok_or_else(|| format!("USD preview {} is not open", command.preview.0))?;
        if session.doc() != command.doc {
            return Err(format!(
                "USD preview {} belongs to document {}, not document {}",
                command.preview.0,
                session.doc(),
                command.doc
            ));
        }
        (
            session.scene_root(),
            session.stage_handle().id(),
            session.projection_ready(),
        )
    };
    if !ready {
        return Err(format!(
            "USD preview {} has no ready composed projection",
            command.preview.0
        ));
    }

    if command.assembly.trim() != command.assembly
        || command.assembly.is_empty()
        || !command.assembly.starts_with('/')
        || command.assembly.ends_with('/')
    {
        return Err("USD preview explode assembly must be an absolute prim path".to_string());
    }
    SdfPath::new(&command.assembly).map_err(|error| {
        format!(
            "invalid explode assembly path `{}`: {error}",
            command.assembly
        )
    })?;
    let part_paths = canonical_explode_parts(&command.parts)?;

    let mut query = world.query::<(
        Entity,
        &UsdPrimPath,
        &Transform,
        Option<&ChildOf>,
        Option<&lunco_core::UsdPrimKind>,
        Has<UsdVisualSynced>,
    )>();
    let mut snapshots = HashMap::new();
    let mut parents = HashMap::new();
    for (entity, prim, local, parent, kind, synced) in query.iter(world) {
        if prim.stage_handle.id() != stage_id {
            continue;
        }
        if !finite_transform(local) {
            return Err(format!(
                "USD preview explode target `{}` has a non-finite transform",
                prim.path
            ));
        }
        if let Some(parent) = parent {
            parents.insert(entity, parent.parent());
        }
        snapshots.insert(
            entity,
            PreviewPrimSnapshot {
                entity,
                path: prim.path.clone(),
                parent: parent.map(ChildOf::parent),
                local: local.clone(),
                kind: kind.map(|kind| kind.0.clone()),
                synced,
            },
        );
    }

    let Some(root_snapshot) = snapshots.get(&root) else {
        return Err(format!(
            "USD preview {} has no projected scene root",
            command.preview.0
        ));
    };
    if !root_snapshot.synced || !preview_entity_reaches_root(root, root, &parents) {
        return Err(format!(
            "USD preview {} scene root is stale",
            command.preview.0
        ));
    }

    let mut by_path: HashMap<String, Vec<PreviewPrimSnapshot>> = HashMap::new();
    for snapshot in snapshots.values() {
        if snapshot.synced && preview_entity_reaches_root(snapshot.entity, root, &parents) {
            by_path
                .entry(snapshot.path.clone())
                .or_default()
                .push(snapshot.clone());
        }
    }

    let assembly = match by_path.get(&command.assembly) {
        Some(matches) if matches.len() == 1 => &matches[0],
        Some(matches) => {
            return Err(format!(
                "USD preview explode assembly `{}` is ambiguous ({} projected entities)",
                command.assembly,
                matches.len()
            ));
        }
        None => {
            return Err(format!(
                "USD preview explode assembly `{}` is stale or missing",
                command.assembly
            ));
        }
    };
    if !assembly
        .kind
        .as_deref()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("assembly"))
    {
        return Err(format!(
            "USD preview explode target `{}` is not an authored assembly",
            command.assembly
        ));
    }

    let mut targets = Vec::with_capacity(part_paths.len());
    for path in &part_paths {
        let sdf_path = SdfPath::new(path).expect("part paths were validated");
        if !lunco_usd_bevy::is_descendant_or_self(&sdf_path, &command.assembly)
            || path == &command.assembly
        {
            return Err(format!(
                "USD preview explode part `{path}` is not below assembly `{}`",
                command.assembly
            ));
        }
        let matches = by_path.get(path).map(Vec::as_slice).unwrap_or_default();
        let Some(target) = matches.first() else {
            return Err(format!(
                "USD preview explode part `{path}` is stale or missing"
            ));
        };
        if matches.len() != 1 {
            return Err(format!(
                "USD preview explode part `{path}` is ambiguous ({} projected entities)",
                matches.len()
            ));
        }
        if target.parent.is_none() {
            return Err(format!(
                "USD preview explode part `{path}` has no projected parent frame"
            ));
        }
        targets.push(target.clone());
    }

    let mut locals = HashMap::with_capacity(snapshots.len());
    for snapshot in snapshots.values() {
        locals.insert(snapshot.entity, snapshot.local.clone());
    }
    if preview_local_to_root(root, root, &parents, &locals).is_none() {
        return Err(format!(
            "USD preview {} has an invalid projected hierarchy",
            command.preview.0
        ));
    }
    Ok((
        root,
        assembly.entity,
        assembly.path.clone(),
        targets,
        locals,
        parents,
    ))
}

fn execute_explode_usd_preview(
    world: &mut World,
    command: ExplodeUsdPreview,
) -> Result<Ack, String> {
    let (root, assembly_entity, assembly_path, targets, locals, parents) =
        collect_preview_explode_targets(world, &command)?;
    let part_paths: Vec<_> = targets.iter().map(|target| target.path.clone()).collect();
    let existing = world
        .resource::<UsdViewportState>()
        .session(command.preview)
        .and_then(|session| session.explode.as_ref())
        .cloned();
    if let Some(existing) = &existing {
        if existing.assembly != assembly_path
            || existing
                .parts
                .iter()
                .map(|part| part.path.as_str())
                .ne(part_paths.iter().map(String::as_str))
        {
            return Err(format!(
                "USD preview {} already has an explode target; reset it before changing assembly or parts",
                command.preview.0
            ));
        }
        if existing.parts.iter().any(|part| {
            targets
                .iter()
                .find(|target| target.path == part.path)
                .map_or(true, |target| target.entity != part.entity)
        }) {
            return Err(format!(
                "USD preview {} explode state is stale after reprojection",
                command.preview.0
            ));
        }
    }

    let (axis, spacing) = match command.action {
        UsdPreviewExplodeAction::Reset => (None, None),
        UsdPreviewExplodeAction::Enable | UsdPreviewExplodeAction::Update => {
            let axis = command
                .axis
                .ok_or_else(|| "USD preview explode enable/update requires an axis".to_string())?;
            let spacing = command.spacing.ok_or_else(|| {
                "USD preview explode enable/update requires positive spacing".to_string()
            })?;
            if !spacing.is_finite() || spacing <= 0.0 {
                return Err("USD preview explode spacing must be finite and positive".to_string());
            }
            (Some(axis), Some(spacing))
        }
    };

    if command.action == UsdPreviewExplodeAction::Update && existing.is_none() {
        return Err(format!(
            "USD preview {} has no explode state to update",
            command.preview.0
        ));
    }

    if command.action == UsdPreviewExplodeAction::Reset {
        let changed = if let Some(existing) = existing {
            for part in &existing.parts {
                let Some(mut entity) = world.get_entity_mut(part.entity).ok() else {
                    return Err(format!(
                        "USD preview {} explode part `{}` disappeared before reset",
                        command.preview.0, part.path
                    ));
                };
                let Some(mut transform) = entity.get_mut::<Transform>() else {
                    return Err(format!(
                        "USD preview explode part `{}` has no transform",
                        part.path
                    ));
                };
                *transform = part.baseline.clone();
            }
            true
        } else {
            false
        };
        if let Some(session) = world
            .resource_mut::<UsdViewportState>()
            .session_mut(command.preview)
        {
            session.explode = None;
        }
        return Ok(Ack::with_data(
            OpId::new(),
            serde_json::json!({
                "preview": command.preview.0,
                "doc": command.doc,
                "action": command.action.as_str(),
                "assembly": assembly_path,
                "parts": part_paths,
                "changed": changed,
                "preview_only": true,
                "authored": false,
            }),
        ));
    }

    let (baseline_parts, assembly_to_root) = if let Some(existing) = existing {
        (existing.parts, existing.assembly_to_root)
    } else {
        let assembly_to_root = preview_local_to_root(assembly_entity, root, &parents, &locals)
            .ok_or_else(|| "USD preview explode assembly has an invalid hierarchy".to_string())?;
        if !finite_matrix(assembly_to_root) {
            return Err("USD preview explode assembly frame is non-finite".to_string());
        }
        (
            targets
                .iter()
                .map(|target| {
                    let parent = target.parent.expect("target parent validated");
                    let parent_to_root = preview_local_to_root(parent, root, &parents, &locals)
                        .ok_or_else(|| {
                            format!(
                                "USD preview explode part `{}` has an invalid parent hierarchy",
                                target.path
                            )
                        })?;
                    if !finite_matrix(parent_to_root) {
                        return Err(format!(
                            "USD preview explode part `{}` has a non-finite parent frame",
                            target.path
                        ));
                    }
                    Ok(UsdPreviewExplodedPart {
                        path: target.path.clone(),
                        entity: target.entity,
                        baseline: target.local.clone(),
                        parent_to_root,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
            assembly_to_root,
        )
    };

    let axis = axis.expect("enable/update axis validated");
    let spacing = spacing.expect("enable/update spacing validated");
    let mut applied = Vec::with_capacity(baseline_parts.len());
    let mut offsets = Vec::with_capacity(baseline_parts.len());
    for (index, part) in baseline_parts.iter().enumerate() {
        let assembly_delta = axis.vector() * spacing * (index as f32 + 1.0);
        let root_delta = assembly_to_root.transform_vector3(assembly_delta);
        let local_delta = part.parent_to_root.inverse().transform_vector3(root_delta);
        if !root_delta.is_finite() || !local_delta.is_finite() {
            return Err(format!(
                "USD preview explode part `{}` produced a non-finite offset",
                part.path
            ));
        }
        let mut transform = part.baseline.clone();
        transform.translation += local_delta;
        if !finite_transform(&transform) {
            return Err(format!(
                "USD preview explode part `{}` produced a non-finite transform",
                part.path
            ));
        }
        applied.push((part.entity, transform));
        offsets.push(serde_json::json!({
            "path": part.path,
            "assembly_delta": assembly_delta.to_array(),
            "parent_local_delta": local_delta.to_array(),
            "order": index + 1,
        }));
    }
    for (entity, transform) in applied {
        let Some(mut entity) = world.get_entity_mut(entity).ok() else {
            return Err(format!(
                "USD preview {} explode target disappeared before apply",
                command.preview.0
            ));
        };
        let Some(mut current) = entity.get_mut::<Transform>() else {
            return Err("USD preview explode target has no transform".to_string());
        };
        *current = transform;
    }

    let state = UsdPreviewExplodeState {
        assembly: assembly_path.clone(),
        parts: baseline_parts,
        axis,
        spacing,
        assembly_to_root,
    };
    if let Some(session) = world
        .resource_mut::<UsdViewportState>()
        .session_mut(command.preview)
    {
        session.explode = Some(state);
    }
    Ok(Ack::with_data(
        OpId::new(),
        serde_json::json!({
            "preview": command.preview.0,
            "doc": command.doc,
            "action": command.action.as_str(),
            "assembly": assembly_path,
            "parts": part_paths,
            "axis": axis.as_str(),
            "spacing": spacing,
            "offsets": offsets,
            "preview_only": true,
            "authored": false,
        }),
    ))
}

#[on_command(ExplodeUsdPreview)]
fn on_explode_usd_preview(
    trigger: On<ExplodeUsdPreview>,
    mut commands: Commands,
    active_id: Option<Res<ActiveCommandId>>,
    pending_request: Option<Res<PendingApiRequest>>,
) {
    let command = trigger.event().clone();
    let command_id = active_id.and_then(|id| id.get());
    let correlation_id = pending_request
        .map(|request| request.correlation_id)
        .filter(|id| *id != 0);
    commands.queue(move |world: &mut World| {
        let outcome = execute_explode_usd_preview(world, command);
        finish_command_result(
            world,
            command_id,
            correlation_id,
            outcome,
            ApiErrorCode::CommandRejected,
        );
    });
}

register_commands!(
    on_open_usd_preview,
    on_open_usd_preview_view,
    on_focus_usd_preview,
    on_focus_usd_preview_view,
    on_close_usd_preview_view,
    on_close_usd_preview,
    on_set_usd_preview_projection,
    on_frame_usd_preview_view,
    on_reset_usd_preview_view,
    on_pan_usd_preview_view,
    on_zoom_usd_preview_view,
    on_explode_usd_preview,
);

// ─────────────────────────────────────────────────────────────────────
// Document lifecycle observers
// ─────────────────────────────────────────────────────────────────────

/// Retire previews whose documents belonged to the closed Twin. A preview is
/// a user-facing document session, so preserving it after its source Twin has
/// closed would keep the old project's stage visible in the replacement Twin.
/// Previews backed by another still-open Twin remain mounted; only those whose
/// authority disappeared are closed.
fn on_twin_closed_for_viewport(trigger: On<TwinClosed>, mut commands: Commands) {
    let event = trigger.event();
    let closed_twin = event.twin;
    let closed_root = event.root.clone();
    commands.queue(move |world: &mut World| {
        let closed_docs: HashSet<DocumentId> = world
            .get_resource::<WorkspaceResource>()
            .map(|workspace| {
                workspace
                    .documents()
                    .iter()
                    .filter(|entry| document_belongs_to_twin_root(entry, closed_twin, &closed_root))
                    .map(|entry| entry.id)
                    .collect()
            })
            .unwrap_or_default();
        let docs: Vec<_> = world
            .resource::<UsdViewportState>()
            .preview_docs()
            .collect();
        for doc in docs {
            if closed_docs.contains(&doc) {
                let sessions = world
                    .resource::<UsdViewportState>()
                    .session_ids_for_doc(doc);
                for preview in sessions {
                    let _ = remove_preview_session(world, preview);
                }
                release_preview_projection(world, doc);
                continue;
            }
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
    if let Some(session) = world
        .resource_mut::<UsdViewportState>()
        .session_mut(preview)
    {
        session.stage_handle = handle;
        session.projected_generation = 0;
        session.projection_ready = false;
        session.explode = None;
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
    let (tex_id, focused_doc, next_view, projection) = ctx
        .resource::<UsdViewportState>()
        .and_then(|state| {
            let view = state.view(view_id)?;
            let session = state.session(view.preview())?;
            let next_view = singleton
                .then(|| state.next_view_id())
                .flatten()
                .map(|view| (session.id(), view));
            Some((
                view.texture_id(),
                Some(session.doc()),
                next_view,
                view.projection(),
            ))
        })
        .unwrap_or((None, None, None, UsdPreviewProjection::default()));
    let name = focused_doc
        .and_then(|doc| {
            ctx.resource::<DocumentRegistry<UsdDocument>>()
                .and_then(|registry| registry.host(doc))
                .map(|host| host.document().origin().display_name())
        })
        .unwrap_or_else(|| "(no stage)".to_string());

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&name).strong());
        if ui
            .selectable_label(
                projection == UsdPreviewProjection::Perspective,
                "Perspective",
            )
            .clicked()
        {
            ctx.trigger(SetUsdPreviewProjection {
                view: view_id,
                projection: UsdPreviewProjection::Perspective,
            });
        }
        if ui
            .selectable_label(
                projection == UsdPreviewProjection::Orthographic,
                "Orthographic",
            )
            .clicked()
        {
            ctx.trigger(SetUsdPreviewProjection {
                view: view_id,
                projection: UsdPreviewProjection::Orthographic,
            });
        }
        if ui.button("Frame").clicked() {
            ctx.trigger(FrameUsdPreviewView { view: view_id });
        }
        if ui.button("Reset").clicked() {
            ctx.trigger(ResetUsdPreviewView { view: view_id });
        }
        if let Some((preview, view)) = next_view {
            if ui.button("Open view").clicked() {
                ctx.trigger(OpenUsdPreviewView { preview, view });
            }
        }
    });
    ui.small("L-drag pan · R-drag orbit · M-drag pan · wheel zoom");
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
        ctx.trigger(UsdPreviewViewMeasured {
            view: view_id,
            over_scene,
        });
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

    let (drag, pan) = if response.dragged() {
        let shift = ui.ctx().input(|input| input.modifiers.shift);
        let (orbit, pan) = preview_drag_channels(
            response.dragged_by(egui::PointerButton::Primary),
            response.dragged_by(egui::PointerButton::Middle),
            response.dragged_by(egui::PointerButton::Secondary),
            shift,
        );
        let delta = response.drag_delta();
        (
            orbit.then_some(delta).unwrap_or_default(),
            pan.then_some(delta).unwrap_or_default(),
        )
    } else {
        (egui::Vec2::ZERO, egui::Vec2::ZERO)
    };
    let scroll_y = if response.hovered() || response.contains_pointer() {
        ui.ctx().input(|input| input.smooth_scroll_delta.y)
    } else {
        0.0
    };
    if drag != egui::Vec2::ZERO || pan != egui::Vec2::ZERO || scroll_y != 0.0 {
        ctx.trigger(UsdViewportOrbitInput {
            view: view_id,
            drag,
            pan,
            viewport_size: response.rect.size(),
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
    use lunco_workbench::{BrowserAction, BrowserActions};

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
        app.init_asset::<UsdStageAsset>();
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

    fn explode_fixture() -> (App, UsdPreviewId, DocumentId, Entity, Entity) {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>();
        let preview = UsdPreviewId(77);
        let doc = DocumentId::new(9);
        let stage = Handle::<UsdStageAsset>::default();
        let session = create_preview_session(
            app.world_mut(),
            preview,
            doc,
            LayerId::root(),
            stage.clone(),
            FIRST_PREVIEW_RENDER_LAYER,
            UsdPreviewViewId(1),
        )
        .expect("fixture session resources are available");
        let root = session.scene_root();
        app.world_mut().entity_mut(root).insert((
            UsdPrimPath {
                stage_handle: stage.clone(),
                path: "/Scene".into(),
            },
            UsdVisualSynced,
        ));
        let assembly = app
            .world_mut()
            .spawn((
                Name::new("Assembly"),
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Scene/Assembly".into(),
                },
                Transform::from_xyz(5.0, 0.0, 0.0),
                UsdVisualSynced,
                lunco_core::UsdPrimKind("assembly".into()),
                ChildOf(root),
            ))
            .id();
        let part_a = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Scene/Assembly/PartA".into(),
                },
                Transform::from_xyz(1.0, 0.0, 0.0),
                UsdVisualSynced,
                ChildOf(assembly),
            ))
            .id();
        let group = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Scene/Assembly/Group".into(),
                },
                Transform::from_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)),
                UsdVisualSynced,
                ChildOf(assembly),
            ))
            .id();
        let part_b = app
            .world_mut()
            .spawn((
                UsdPrimPath {
                    stage_handle: stage,
                    path: "/Scene/Assembly/Group/PartB".into(),
                },
                Transform::from_xyz(0.0, 0.0, 3.0),
                UsdVisualSynced,
                ChildOf(group),
            ))
            .id();
        let mut state = UsdViewportState::default();
        state.insert(session);
        state.session_mut(preview).unwrap().projected_generation = 1;
        state.session_mut(preview).unwrap().projection_ready = true;
        app.insert_resource(state);
        (app, preview, doc, part_a, part_b)
    }

    fn explode_command(
        preview: UsdPreviewId,
        doc: DocumentId,
        assembly: &str,
        parts: &[&str],
        action: UsdPreviewExplodeAction,
        axis: Option<UsdPreviewExplodeAxis>,
        spacing: Option<f32>,
    ) -> ExplodeUsdPreview {
        ExplodeUsdPreview {
            preview,
            doc,
            assembly: assembly.into(),
            parts: parts.iter().map(|part| (*part).into()).collect(),
            action,
            axis,
            spacing,
        }
    }

    #[test]
    fn explode_enable_update_reset_is_hierarchical_and_idempotent() {
        let (mut app, preview, doc, part_a, part_b) = explode_fixture();
        let assembly = "/Scene/Assembly";
        let parts = ["/Scene/Assembly/Group/PartB", "/Scene/Assembly/PartA"];
        let baseline_a = app.world().get::<Transform>(part_a).unwrap().clone();
        let baseline_b = app.world().get::<Transform>(part_b).unwrap().clone();

        let enabled = execute_explode_usd_preview(
            app.world_mut(),
            explode_command(
                preview,
                doc,
                assembly,
                &parts,
                UsdPreviewExplodeAction::Enable,
                Some(UsdPreviewExplodeAxis::X),
                Some(2.0),
            ),
        )
        .expect("valid assembly explode enables");
        assert_eq!(
            enabled.data.as_ref().unwrap()["parts"],
            serde_json::json!(["/Scene/Assembly/Group/PartB", "/Scene/Assembly/PartA"])
        );
        assert_eq!(
            app.world().get::<Transform>(part_a).unwrap().translation,
            baseline_a.translation + Vec3::new(4.0, 0.0, 0.0)
        );
        assert!(
            (app.world().get::<Transform>(part_b).unwrap().translation
                - (baseline_b.translation + Vec3::new(0.0, -2.0, 0.0)))
            .length()
                < 1.0e-5
        );

        execute_explode_usd_preview(
            app.world_mut(),
            explode_command(
                preview,
                doc,
                assembly,
                &parts,
                UsdPreviewExplodeAction::Enable,
                Some(UsdPreviewExplodeAxis::Y),
                Some(1.0),
            ),
        )
        .expect("repeated enable reuses the original baseline");
        assert_eq!(
            app.world().get::<Transform>(part_a).unwrap().translation,
            baseline_a.translation + Vec3::Y * 2.0
        );
        assert!(
            (app.world().get::<Transform>(part_b).unwrap().translation
                - (baseline_b.translation + Vec3::X))
                .length()
                < 1.0e-5
        );

        execute_explode_usd_preview(
            app.world_mut(),
            explode_command(
                preview,
                doc,
                assembly,
                &parts,
                UsdPreviewExplodeAction::Reset,
                None,
                None,
            ),
        )
        .expect("valid assembly explode resets");
        assert_eq!(app.world().get::<Transform>(part_a).unwrap(), &baseline_a);
        assert_eq!(app.world().get::<Transform>(part_b).unwrap(), &baseline_b);
        assert!(app
            .world()
            .resource::<UsdViewportState>()
            .session(preview)
            .unwrap()
            .explode
            .is_none());
    }

    #[test]
    fn explode_rejects_non_assembly_and_stale_targets_without_mutation() {
        let (mut app, preview, doc, part_a, _) = explode_fixture();
        let before = app.world().get::<Transform>(part_a).unwrap().clone();
        let error = execute_explode_usd_preview(
            app.world_mut(),
            explode_command(
                preview,
                doc,
                "/Scene/Assembly/PartA",
                &["/Scene/Assembly/Group/PartB"],
                UsdPreviewExplodeAction::Enable,
                Some(UsdPreviewExplodeAxis::X),
                Some(1.0),
            ),
        )
        .expect_err("a component is not an assembly target");
        assert!(error.contains("not an authored assembly"));
        assert_eq!(app.world().get::<Transform>(part_a).unwrap(), &before);

        let error = execute_explode_usd_preview(
            app.world_mut(),
            explode_command(
                preview,
                doc,
                "/Scene/Assembly",
                &["/Scene/Assembly/Missing"],
                UsdPreviewExplodeAction::Enable,
                Some(UsdPreviewExplodeAxis::X),
                Some(1.0),
            ),
        )
        .expect_err("a missing part is stale");
        assert!(error.contains("stale or missing"));
        assert_eq!(app.world().get::<Transform>(part_a).unwrap(), &before);
    }

    #[test]
    fn reprojection_invalidates_explode_and_returns_captured_baselines() {
        let (mut app, preview, doc, part_a, part_b) = explode_fixture();
        let baseline_a = app.world().get::<Transform>(part_a).unwrap().clone();
        let baseline_b = app.world().get::<Transform>(part_b).unwrap().clone();
        execute_explode_usd_preview(
            app.world_mut(),
            explode_command(
                preview,
                doc,
                "/Scene/Assembly",
                &["/Scene/Assembly/PartA", "/Scene/Assembly/Group/PartB"],
                UsdPreviewExplodeAction::Enable,
                Some(UsdPreviewExplodeAxis::Z),
                Some(3.0),
            ),
        )
        .expect("valid assembly explode enables");

        let restores = app
            .world_mut()
            .resource_mut::<UsdViewportState>()
            .invalidate_projection(doc);
        assert_eq!(restores.len(), 2);
        for (entity, transform) in restores {
            *app.world_mut()
                .get_mut::<Transform>(entity)
                .expect("projected explode target remains during reprojection") = transform;
        }

        assert_eq!(app.world().get::<Transform>(part_a).unwrap(), &baseline_a);
        assert_eq!(app.world().get::<Transform>(part_b).unwrap(), &baseline_b);
        let session = app
            .world()
            .resource::<UsdViewportState>()
            .session(preview)
            .expect("preview session remains open while reprojection starts");
        assert!(session.explode.is_none());
        assert!(!session.projection_ready());
        assert_eq!(session.projected_generation(), 0);
    }

    #[test]
    fn explode_command_is_discoverable_with_explicit_lifecycle_fields() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdViewportPlugin);
        let registry = app.world().resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let schema = lunco_api::discovery::discover_commands(&registry, None);
        let command = schema
            .iter()
            .find(|command| command.name == "ExplodeUsdPreview")
            .expect("explode command is registered by the viewport plugin");
        assert!(!command.defaulted);
        for field in [
            "preview", "doc", "assembly", "parts", "action", "axis", "spacing",
        ] {
            assert!(
                command
                    .fields
                    .iter()
                    .any(|candidate| candidate.name == field),
                "explode schema must expose `{field}`"
            );
        }
    }

    #[test]
    fn reopening_the_same_preview_reuses_its_lease() {
        let path = std::env::temp_dir().join("lunco_usd_preview_reopen_test.usda");
        std::fs::write(&path, "#usda 1.0\ndef Xform \"X\" {}\n").unwrap();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        app.init_asset::<UsdStageAsset>();
        app.add_plugins(UsdCommandsPlugin);
        app.add_plugins(UsdViewportPlugin);

        let doc = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>()
            .open_file(&path, "#usda 1.0\ndef Xform \"X\" {}\n".to_string())
            .0;
        app.update();
        app.world_mut()
            .trigger(crate::commands::BrowserUsdDocumentReady { doc });
        app.update();
        app.update();
        let first_root = app
            .world()
            .resource::<UsdViewportState>()
            .session(EDITOR_PREVIEW_ID)
            .expect("first preview lease")
            .scene_root();

        app.world_mut()
            .trigger(crate::commands::BrowserUsdDocumentReady { doc });
        app.update();
        app.update();
        let state = app.world().resource::<UsdViewportState>();
        assert_eq!(state.session_count(), 1);
        assert_eq!(state.focused_doc(), Some(doc));
        assert_eq!(
            state.session(EDITOR_PREVIEW_ID).unwrap().scene_root(),
            first_root
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preview_readiness_ignores_live_projection_with_the_same_stage_handle() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Assets<Image>>();
        app.init_resource::<DocumentRegistry<UsdDocument>>();
        app.init_resource::<UsdViewportState>();
        app.add_systems(Update, reconcile_preview_projection_state);

        let doc = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>()
            .open_file("/tmp/shared_preview_stage.usda", "#usda 1.0\n".to_string())
            .0;
        let stage = Handle::<UsdStageAsset>::default();
        let session = create_preview_session(
            app.world_mut(),
            UsdPreviewId(1),
            doc,
            LayerId::root(),
            stage.clone(),
            FIRST_PREVIEW_RENDER_LAYER,
            UsdPreviewViewId(1),
        )
        .expect("preview session resources are available");
        let preview_root = session.scene_root();
        app.world_mut().entity_mut(preview_root).insert((
            UsdPrimPath {
                stage_handle: stage.clone(),
                path: "/World".into(),
            },
            UsdVisualSynced,
        ));
        app.world_mut().spawn((
            UsdPrimPath {
                stage_handle: stage.clone(),
                path: "/World/Rover".into(),
            },
            UsdVisualSynced,
            ChildOf(preview_root),
        ));

        // The active Twin intentionally shares the deduplicated stage handle
        // with its editor preview. Its entities must not participate in the
        // preview's readiness fence.
        let live_root = app
            .world_mut()
            .spawn((
                lunco_usd_bevy::UsdSceneRoot,
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/World".into(),
                },
                UsdVisualSynced,
            ))
            .id();
        app.world_mut().spawn((
            UsdPrimPath {
                stage_handle: stage,
                path: "/World/Rover".into(),
            },
            UsdVisualSynced,
            ChildOf(live_root),
        ));

        app.world_mut()
            .resource_mut::<UsdViewportState>()
            .insert(session);
        app.update();

        let session = app
            .world()
            .resource::<UsdViewportState>()
            .session(UsdPreviewId(1))
            .expect("preview session remains registered");
        assert!(session.projection_ready());
        assert_eq!(
            session.projected_generation(),
            app.world()
                .resource::<DocumentRegistry<UsdDocument>>()
                .host(doc)
                .expect("document remains open")
                .document()
                .generation()
        );
    }

    #[test]
    fn browser_file_action_admits_and_focuses_the_existing_preview_lease() {
        let path = std::env::temp_dir().join("lunco_usd_browser_preview_test.usd");
        std::fs::write(&path, "#usda 1.0\ndef Xform \"X\" {}\n").unwrap();

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        app.init_asset::<UsdStageAsset>();
        app.add_plugins(UsdCommandsPlugin);
        app.add_plugins(UsdViewportPlugin);
        app.init_resource::<BrowserActions>();
        app.add_systems(
            Update,
            crate::ui::browser_dispatch::drain_browser_actions_for_usd,
        );
        app.update();

        app.world_mut()
            .resource_mut::<BrowserActions>()
            .push(BrowserAction::OpenFile {
                relative_path: path.clone(),
            });
        for _ in 0..20 {
            app.update();
            if app.world().resource::<UsdViewportState>().session_count() == 1 {
                break;
            }
            std::thread::yield_now();
        }

        let state = app.world().resource::<UsdViewportState>();
        let doc = app
            .world()
            .resource::<DocumentRegistry<UsdDocument>>()
            .doc_for_file(&path)
            .expect("browser action admitted the USD document");
        assert_eq!(state.session_count(), 1);
        assert_eq!(state.focused_preview_id(), Some(EDITOR_PREVIEW_ID));
        assert_eq!(state.focused_doc(), Some(doc));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preview_light_uses_graphics_distant_light_default() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>();
        let mut settings = RenderingQualitySettings::default();
        settings.distant_light_default_illuminance = 42_000.0;
        app.insert_resource(settings);

        let profile = validated_preview_profile(app.world()).expect("quality is valid");
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
            profile,
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
        assert_eq!(
            camera
                .get::<bevy::camera::Exposure>()
                .expect("preview camera uses the graphics exposure")
                .ev100,
            profile.camera_exposure_ev100
        );
    }

    #[test]
    fn viewport_query_reports_explicit_focus_and_view_handles() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>();
        let session = create_preview_session(
            app.world_mut(),
            UsdPreviewId(1),
            DocumentId::new(7),
            LayerId::root(),
            Handle::default(),
            FIRST_PREVIEW_RENDER_LAYER,
            UsdPreviewViewId(1),
        )
        .expect("session resources are available");
        let layer = session.render_layer();
        let profile = RenderingQualitySettings::default()
            .validated_profile()
            .expect("default quality is valid");
        let first_view = create_preview_view(
            app.world_mut(),
            UsdPreviewId(1),
            UsdPreviewViewId(1),
            layer,
            profile,
        )
        .expect("first view resources are available");
        let second_view = create_preview_view(
            app.world_mut(),
            UsdPreviewId(1),
            UsdPreviewViewId(2),
            layer,
            profile,
        )
        .expect("second view resources are available");
        let mut state = UsdViewportState::default();
        state.insert(session);
        assert!(state.insert_view(first_view).is_ok());
        assert!(state.insert_view(second_view).is_ok());
        assert!(state.focus_view(UsdPreviewViewId(2)));
        app.insert_resource(state);

        let response = InspectUsdViewportProvider.execute(app.world(), &serde_json::json!({}));
        let ApiResponse::Ok { data: Some(data) } = response else {
            panic!("viewport query must return presentation data");
        };
        assert_eq!(data["focused_preview"], serde_json::json!(1));
        assert_eq!(data["focused_view"], serde_json::json!(2));
        assert_eq!(data["preview_count"], serde_json::json!(1));
        assert_eq!(data["view_count"], serde_json::json!(2));
        assert_eq!(data["previews"][0]["doc"], serde_json::json!(7));
        assert_eq!(
            data["previews"][0]["views"][0]["view"],
            serde_json::json!(1)
        );
        assert_eq!(
            data["previews"][0]["views"][1]["view"],
            serde_json::json!(2)
        );
        assert_eq!(
            data["previews"][0]["views"][1]["focused"],
            serde_json::json!(true)
        );
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
    fn orbit_pan_and_zoom_keep_view_state_finite() {
        let mut orbit = OrbitCamera::default();
        let original_target = orbit.target;
        let original_distance = orbit.distance;

        let projection = preview_projection(UsdPreviewProjection::Perspective, 1.0);
        assert!(orbit.apply_pan(
            egui::Vec2::new(40.0, -18.0),
            Vec2::new(800.0, 600.0),
            &projection,
            UsdPreviewProjection::Perspective,
            1.0,
        ));
        orbit.apply_zoom(12.0);

        assert_ne!(orbit.target, original_target);
        assert!(orbit.target.is_finite());
        assert!(orbit.distance.is_finite());
        assert!(orbit.distance < original_distance);
        assert!((orbit.zoom_factor(12.0) - 0.982).abs() < 1.0e-6);
    }

    #[test]
    fn preview_pan_follows_pointer_in_both_screen_axes() {
        let projection = preview_projection(UsdPreviewProjection::Perspective, 1.0);
        let mut orbit = OrbitCamera::default();
        let transform = orbit.transform();
        let right = transform.rotation * Vec3::X;
        let up = transform.rotation * Vec3::Y;

        assert!(orbit.apply_pan(
            egui::Vec2::new(20.0, 30.0),
            Vec2::new(800.0, 600.0),
            &projection,
            UsdPreviewProjection::Perspective,
            1.0,
        ));

        let target_delta = orbit.target;
        assert!(target_delta.dot(right) < 0.0);
        assert!(target_delta.dot(up) > 0.0);
    }

    #[test]
    fn preview_pan_uses_projection_scale_not_fixed_sensitivity() {
        let perspective = preview_projection(UsdPreviewProjection::Perspective, 1.0);
        let mut near = OrbitCamera::default();
        near.distance = 2.0;
        let mut far = near.clone();
        far.distance = 4.0;
        assert!(near.apply_pan(
            egui::Vec2::new(40.0, 20.0),
            Vec2::new(800.0, 600.0),
            &perspective,
            UsdPreviewProjection::Perspective,
            1.0,
        ));
        assert!(far.apply_pan(
            egui::Vec2::new(40.0, 20.0),
            Vec2::new(800.0, 600.0),
            &perspective,
            UsdPreviewProjection::Perspective,
            1.0,
        ));
        assert!((far.target.length() / near.target.length() - 2.0).abs() < 1.0e-5);

        let orthographic = preview_projection(UsdPreviewProjection::Orthographic, 2.0);
        let mut low = OrbitCamera::default();
        low.distance = 2.0;
        let mut high = low.clone();
        high.distance = 200.0;
        assert!(low.apply_pan(
            egui::Vec2::new(40.0, 20.0),
            Vec2::new(800.0, 600.0),
            &orthographic,
            UsdPreviewProjection::Orthographic,
            2.0,
        ));
        assert!(high.apply_pan(
            egui::Vec2::new(40.0, 20.0),
            Vec2::new(800.0, 600.0),
            &orthographic,
            UsdPreviewProjection::Orthographic,
            2.0,
        ));
        assert!((high.target.length() - low.target.length()).abs() < 1.0e-5);
    }

    #[test]
    fn preview_pointer_buttons_match_view_navigation_contract() {
        assert_eq!(
            preview_drag_channels(true, false, false, false),
            (false, true)
        );
        assert_eq!(
            preview_drag_channels(false, true, false, false),
            (false, true)
        );
        assert_eq!(
            preview_drag_channels(false, false, true, false),
            (true, false)
        );
        assert_eq!(
            preview_drag_channels(false, false, true, true),
            (false, true)
        );
        assert_eq!(
            preview_drag_channels(true, false, false, true),
            (false, true)
        );
    }

    #[test]
    fn preview_projection_modes_use_explicit_presentation_contract() {
        let perspective = preview_projection(UsdPreviewProjection::Perspective, 3.0);
        assert!(matches!(perspective, Projection::Perspective(_)));

        let orthographic = preview_projection(UsdPreviewProjection::Orthographic, 3.0);
        let Projection::Orthographic(orthographic) = orthographic else {
            panic!("orthographic preview mode must create an orthographic camera");
        };
        assert_eq!(orthographic.scale, 3.0);
        assert!(matches!(
            orthographic.scaling_mode,
            bevy::camera::ScalingMode::FixedVertical {
                viewport_height: 2.0
            }
        ));
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
        let profile = RenderingQualitySettings::default()
            .validated_profile()
            .expect("default quality is valid");
        let first_view = create_preview_view(
            app.world_mut(),
            UsdPreviewId(1),
            UsdPreviewViewId(1),
            layer,
            profile,
        )
        .expect("first view resources are available");
        let second_view = create_preview_view(
            app.world_mut(),
            UsdPreviewId(1),
            UsdPreviewViewId(2),
            layer,
            profile,
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
