//! `UsdViewportPanel` — 3D scene of the focused USD preview lease, rendered
//! to an offscreen [`Image`] and surfaced in egui as a regular
//! [`bevy_egui::egui::Image`].
//!
//! Mirrors the canvas pattern in spirit: one workbench panel paints the
//! focused lease while other leases keep their own Bevy 3D render state.
//! The body is a real Bevy 3D render — the egui panel receives a `TextureId`
//! whose underlying `Image` is what that lease's [`Camera3d`] drew into.
//!
//! ## Pipeline
//!
//! ```text
//! UsdDocument source text
//!         │
//!         ▼  (on OpenUsdPreview for an explicit doc and preview id)
//! authored layer → canonical composition → UsdStageAsset
//!         │
//!         ▼  (one session lease owns one stage handle)
//! Handle<UsdStageAsset>
//!         │  (UsdPrimPath { stage_handle, path: "" } on that session root)
//! sync_usd_visuals  →  child entities with meshes / transforms
//!         │
//!         ▼  (Camera3d targets a render-to-texture Image)
//! Image  →  EguiUserTextures  →  egui::TextureId
//!         │
//!         ▼  (panel render)
//! UsdViewportPanel  ─────────  egui::Image in the dock
//! ```
//!
//! ## Lifecycle (observers)
//!
//! - [`OpenUsdPreview`] for an explicit document, edit layer, and preview id
//!   → allocate one isolated render lease and mount that document on its root.
//! - [`FocusUsdPreview`] changes which already-open lease the dock displays.
//! - [`CloseUsdPreview`] releases the session root, camera, render target, and
//!   synthetic Twin authority when it is no longer shared.
//! - [`lunco_doc_bevy::DocumentChanged`] wakes the shared
//!   `twin_projection` owner. It authors the typed edit to the live
//!   canonical stage and the normal USD projection refreshes the preview;
//!   this panel does not re-parse or mutate an asset in-place.
//! - [`DocumentClosed`] → close every preview lease for that document and
//!   release its render resources.
//!
//! ## What this plugin does *not* do
//!
//! - A split-view layout. The session registry supports multiple isolated
//!   leases; the current dock surface paints one focused lease.
//! - The viewport does not compose source text itself. The canonical stage
//!   projection owns sublayers, references, payloads, and variants; this panel
//!   only selects the document whose live stage is projected.

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
    Panel, PanelCtx, PanelId, PanelRect, PanelRects, PanelScrollPolicy, PanelSlot, ScenePickGate,
    SceneTarget, WorkbenchAppExt,
};
use lunco_workspace::TwinClosed;

use crate::document::{LayerId, UsdDocument};
use lunco_doc_bevy::DocumentRegistry;

use std::collections::HashMap;

/// Stable id of the workbench tab the viewport renders into.
pub const USD_VIEWPORT_PANEL_ID: PanelId = PanelId("usd::viewport");

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
/// document never retargets this lease or any other lease.
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
        app.init_resource::<RenderingQualitySettings>();
        app.register_panel(UsdViewportPanel);
        app.add_observer(on_twin_closed_for_viewport);
        app.add_observer(on_doc_closed_for_viewport);
        app.add_observer(on_viewport_measured);
        app.add_observer(on_viewport_orbit_input);
        app.add_systems(
            Update,
            (propagate_preview_render_layer, resize_viewport_image),
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

/// Stable identity of one isolated USD preview lease.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect, serde::Serialize, serde::Deserialize,
)]
pub struct UsdPreviewId(pub u64);

impl Default for UsdPreviewId {
    fn default() -> Self {
        EDITOR_PREVIEW_ID
    }
}

/// One isolated USD preview lease. The document and edit layer are explicit;
/// the render resources are owned by this lease and are never selected by
/// entity count, path, or the mission viewport.
pub struct UsdPreviewSession {
    id: UsdPreviewId,
    doc: DocumentId,
    edit_target: LayerId,
    image: Handle<Image>,
    tex_id: Option<egui::TextureId>,
    scene_root: Entity,
    camera: Entity,
    light: Entity,
    stage_handle: Handle<UsdStageAsset>,
    render_layer: usize,
    projected_generation: u64,
    /// Pointer-driven orbit pose for this preview lease.
    pub orbit: OrbitCamera,
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

    pub fn camera(&self) -> Entity {
        self.camera
    }

    pub fn light(&self) -> Entity {
        self.light
    }

    pub fn render_layer(&self) -> usize {
        self.render_layer
    }

    pub fn projected_generation(&self) -> u64 {
        self.projected_generation
    }

    pub fn texture_id(&self) -> Option<egui::TextureId> {
        self.tex_id
    }
}

/// Session-scoped USD preview registry. Every session owns one render target,
/// camera, light, scene root, and render layer. The dock only paints the
/// focused session; all open sessions continue receiving canonical USD stage
/// updates independently.
#[derive(Resource, Default)]
pub struct UsdViewportState {
    sessions: HashMap<UsdPreviewId, UsdPreviewSession>,
    focused: Option<UsdPreviewId>,
}

impl UsdViewportState {
    pub fn focused_preview_id(&self) -> Option<UsdPreviewId> {
        self.focused
    }

    pub fn focused_session(&self) -> Option<&UsdPreviewSession> {
        self.focused.and_then(|id| self.sessions.get(&id))
    }

    pub fn session(&self, id: UsdPreviewId) -> Option<&UsdPreviewSession> {
        self.sessions.get(&id)
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
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
        self.sessions.insert(session.id, session);
    }

    fn remove(&mut self, id: UsdPreviewId) -> Option<UsdPreviewSession> {
        let session = self.sessions.remove(&id)?;
        if self.focused == Some(id) {
            self.focused = None;
        }
        Some(session)
    }

    fn focus(&mut self, id: UsdPreviewId) -> bool {
        if self.sessions.contains_key(&id) {
            self.focused = Some(id);
            true
        } else {
            false
        }
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
    rect: PanelRect,
    over_scene: bool,
}

/// Pointer input emitted by the viewport panel. Camera state and the camera
/// entity are updated by the observer, outside the egui paint borrow.
#[derive(Event, Clone, Copy, Debug)]
struct UsdViewportOrbitInput {
    drag: egui::Vec2,
    scroll_y: f32,
}

fn on_viewport_measured(
    trigger: On<UsdViewportMeasured>,
    mut rects: ResMut<PanelRects>,
    mut gate: ResMut<ScenePickGate>,
) {
    rects.record(USD_VIEWPORT_PANEL_ID, trigger.event().rect);
    gate.record_scene_leaf(
        SceneTarget::Offscreen(USD_VIEWPORT_PANEL_ID),
        trigger.event().over_scene,
    );
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
    let Some(preview) = state.focused else {
        return;
    };
    let Some(session) = state.session_mut(preview) else {
        return;
    };
    if input.drag != egui::Vec2::ZERO {
        session.orbit.apply_drag(input.drag);
    }
    if input.scroll_y != 0.0 {
        session.orbit.apply_zoom(input.scroll_y);
    }
    let camera = session.camera;
    if let Ok(mut transform) = transforms.get_mut(camera) {
        *transform = session.orbit.transform();
    }
}

// ─────────────────────────────────────────────────────────────────────
// Session render leases
// ─────────────────────────────────────────────────────────────────────

/// Allocate one isolated render lease. OpenUSD stage loading and composition
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
    preview_illuminance: f32,
) -> Option<UsdPreviewSession> {
    if !world.contains_resource::<Assets<Image>>() {
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
                // The main window camera owns order 0. Each preview gets a
                // stable non-zero order, even though its render target is
                // isolated, so Bevy never has to resolve an ambiguous camera
                // order while several leases are open.
                order: render_layer as isize,
                ..default()
            },
            // `RenderTarget::Image` keeps `sync_gizmo_camera` from
            // tagging this camera (it filters on `RenderTarget::Window`).
            RenderTarget::Image(ImageRenderTarget::from(image.clone())),
            OrbitCamera::default().transform(),
            // Preview-only render layer: this camera will render
            // *only* entities tagged with this session's render layer, so
            // the live sim scene (default layer 0) stays invisible to
            // it. Propagated to every USD prim descendant of
            // the session root by `propagate_preview_render_layer`.
            preview_layers.clone(),
            Name::new(format!("UsdPreviewCamera-{}", id.0)),
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
            preview_layers.clone(),
            Name::new(format!("UsdPreviewSun-{}", id.0)),
        ))
        .id();

    let scene_root = commands
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
            // lease and cannot leak into another preview or the mission.
            preview_layers,
        ))
        .id();

    world.flush();

    Some(UsdPreviewSession {
        id,
        doc,
        edit_target,
        image,
        tex_id,
        scene_root,
        camera,
        light,
        stage_handle,
        render_layer,
        projected_generation: 0,
        orbit: OrbitCamera::default(),
    })
}

/// Push each session's render layer onto every descendant of its root that
/// doesn't yet have a `RenderLayers` component.
///
/// `sync_usd_visuals` (in `lunco-usd-bevy`) spawns child prim entities
/// without `RenderLayers`, which means they default to layer 0 and
/// would otherwise show up in the live workbench window. Walking from
/// each root and inserting that lease's layer on missing-RenderLayers
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
    // either a lease was opened/closed/focused (`state` changed this frame) or
    // some entity was newly parented (USD prims spawn incrementally as the
    // stage loads). Once the scene is static this DFS does no work.
    if !state.is_changed() && q_newly_parented.is_empty() {
        return;
    }

    for session in state.sessions.values() {
        let preview_layers = RenderLayers::layer(session.render_layer);
        // Iterative DFS over one preview lease. USD scenes are shallow
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

/// Resize the offscreen render Image to match the `UsdViewportPanel`'s
/// recorded screen rect.
///
/// Runs every Update. `UsdViewportPanel::render` writes its current
/// rect (in physical pixels) into `PanelRects` each frame; this system
/// reads it back and calls `Image::resize` on the asset if the
/// requested size differs from the last applied by more than
/// `RESIZE_DELTA_PX` in either axis. The Image handle stays valid, so
/// `EguiUserTextures` registration and `RenderTarget::Image(handle)`
/// on the camera also stay valid — only the wgpu texture's pixel
/// dimensions change.
///
/// First-apply fires unconditionally for each focused session so its
/// placeholder texture snaps to panel size when that session is shown.
fn resize_viewport_image(
    // `Option` so the system is headless-safe — `PanelRects` is owned by
    // the workbench UI plugin, absent in lifecycle / headless tests.
    rects: Option<Res<PanelRects>>,
    state: Res<UsdViewportState>,
    images: Option<ResMut<Assets<Image>>>,
    mut last_applied: Local<Option<(UsdPreviewId, UVec2)>>,
) {
    let Some(session) = state.focused_session() else {
        return;
    };
    let (Some(rects), Some(mut images)) = (rects, images) else {
        return;
    };
    let Some(rect) = rects.get(USD_VIEWPORT_PANEL_ID) else {
        return;
    };
    let target = rect.size;
    let previous = last_applied
        .filter(|(id, _)| *id == session.id)
        .map(|(_, size)| size)
        .unwrap_or(UVec2::ZERO);
    let first_apply = previous.x == 0 || previous.y == 0;
    let dx = target.x.abs_diff(previous.x);
    let dy = target.y.abs_diff(previous.y);
    if !first_apply && dx < RESIZE_DELTA_PX && dy < RESIZE_DELTA_PX {
        return;
    }
    let Some(mut image) = images.get_mut(&session.image) else {
        return;
    };
    image.resize(Extent3d {
        width: target.x.max(1),
        height: target.y.max(1),
        depth_or_array_layers: 1,
    });
    *last_applied = Some((session.id, target));
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
// Preview lease commands
// ─────────────────────────────────────────────────────────────────────

/// Open one explicit document and authored edit target in an isolated preview
/// session. Opening the same `preview` id replaces that lease; other sessions
/// keep their roots, cameras, and stages untouched.
#[Command(default)]
pub struct OpenUsdPreview {
    /// Stable caller-owned identity of the preview lease.
    pub preview: UsdPreviewId,
    /// The USD document to render.
    pub doc: DocumentId,
    /// The authored layer to use for editor mutations made from this preview.
    pub edit_target: LayerId,
}

/// Focus an already-open preview lease in the USD dock.
#[Command(default)]
pub struct FocusUsdPreview {
    pub preview: UsdPreviewId,
}

/// Close one preview lease and release all of its presentation resources.
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
            preview_illuminance,
        ) else {
            report_preview_error(
                world,
                "usd-preview-open-failed",
                "USD preview render resources could not be allocated".to_string(),
            );
            return;
        };
        world.resource_mut::<UsdViewportState>().insert(session);
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
    on_focus_usd_preview,
    on_close_usd_preview,
);

// ─────────────────────────────────────────────────────────────────────
// Document lifecycle observers
// ─────────────────────────────────────────────────────────────────────

/// A preview may outlive the Twin that originally supplied its `twin://`
/// authority. Reinstall every affected lease after the Twin-close observer
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
/// projection lease is released separately so shared coordinates survive until
/// the final session closes.
fn remove_preview_session(world: &mut World, preview: UsdPreviewId) -> Option<DocumentId> {
    let session = world.resource_mut::<UsdViewportState>().remove(preview)?;
    let doc = session.doc;
    if let Ok(mut entity) = world.get_entity_mut(session.scene_root) {
        entity.despawn_related::<Children>();
        entity.despawn();
    }
    if let Ok(entity) = world.get_entity_mut(session.camera) {
        entity.despawn();
    }
    if let Ok(entity) = world.get_entity_mut(session.light) {
        entity.despawn();
    }
    if let Some(mut textures) = world.get_resource_mut::<EguiUserTextures>() {
        textures.remove_image(session.image.id());
    }
    if let Some(mut images) = world.get_resource_mut::<Assets<Image>>() {
        images.remove(session.image.id());
    }
    Some(doc)
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

/// Workbench panel displaying the focused USD preview lease. Other open leases
/// continue rendering to their own isolated targets and remain editable.
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
        // Record the panel's screen rect into `PanelRects` so
        // `resize_viewport_image` can match the offscreen Image's
        // pixel dimensions to it next tick. Measured here from the
        // read-only `ui` (before any widgets draw, so the rect reflects
        // the full panel body) and emitted after paint via a typed event —
        // the panel has no domain-world access.
        let panel_rect = PanelRects::panel_rect_from_ui(ui);

        let (tex_id, focused_doc) = ctx
            .resource::<UsdViewportState>()
            .and_then(|state| {
                state
                    .focused_session()
                    .map(|session| (session.texture_id(), Some(session.doc())))
            })
            .unwrap_or((None, None));
        let name = focused_doc
            .and_then(|d| {
                ctx.resource::<DocumentRegistry<UsdDocument>>()
                    .and_then(|r| r.host(d))
                    .map(|h| h.document().origin().display_name())
            })
            .unwrap_or_else(|| "(no stage)".to_string());

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&name).strong());
        });
        ui.separator();

        let Some(tex_id) = tex_id else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new(
                        "Open a USD preview from the Twin browser or the \
                         OpenUsdPreview command.",
                    )
                    .weak()
                    .italics(),
                );
            });
            return;
        };

        // Stretch the Image widget to the panel rect. The underlying
        // texture is auto-resized to match this rect by
        // `resize_viewport_image` (one frame of lag), so aspect ratio
        // stays correct and the preview never gets blurry-stretched.
        let size = ui.available_size();
        let response = ui.add(
            egui::Image::new(egui::load::SizedTexture::new(tex_id, size))
                .sense(egui::Sense::click_and_drag()),
        );

        // The pointer is over THIS scene only when it's over the image itself —
        // measured from the image's own rect, after it's laid out. (Measuring the
        // panel's `available_rect_before_wrap()` up front, as the workbench viewport
        // leaf does, would have put this panel's title row inside its "scene".)
        let over_scene = ui.rect_contains_pointer(response.rect);
        ctx.trigger(UsdViewportMeasured {
            rect: panel_rect,
            over_scene,
        });

        // Orbit: drag spins yaw/pitch, scroll zooms.
        let drag = response.drag_delta();
        let hovered = response.hovered();
        let scroll_y = if hovered {
            ui.ctx().input(|i| i.smooth_scroll_delta.y)
        } else {
            0.0
        };
        if drag != egui::Vec2::ZERO || scroll_y != 0.0 {
            ctx.trigger(UsdViewportOrbitInput { drag, scroll_y });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::UsdCommandsPlugin;

    /// Without any rendering plugins (`Assets<Image>` absent), opening a
    /// document does not allocate a preview lease or panic.
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
    /// a preview lease until an explicit `OpenUsdPreview` command arrives.
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
        let session = create_preview_session(
            app.world_mut(),
            UsdPreviewId(1),
            DocumentId::new(1),
            LayerId::root(),
            Handle::default(),
            FIRST_PREVIEW_RENDER_LAYER,
            illuminance,
        )
        .expect("preview resources are available");

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
            .get_entity(session.camera)
            .expect("preview session creates its camera");
        assert!(camera.contains::<SceneCamera>());
        assert!(camera.contains::<GraphicsCameraDefaults>());
    }
}
