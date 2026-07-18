//! `UsdViewportPanel` — 3D scene of the active USD document, rendered
//! to an offscreen [`Image`] and surfaced in egui as a regular
//! [`bevy_egui::egui::Image`].
//!
//! Mirrors the canvas pattern in spirit: one workbench panel, content
//! follows the active document. Different in execution because the
//! body is a real Bevy 3D render — we hand the egui panel a
//! `TextureId` whose underlying `Image` is what a [`Camera3d`] just
//! drew into.
//!
//! ## Why a singleton viewport (for now)
//!
//! Phase 6 ships **one shared viewport** that swaps which document
//! it shows when the user clicks a stage in the browser. That's what
//! the user-visible flow needs (one 3D scene at a time, just like
//! Omniverse's stage view) and avoids the per-document camera /
//! image / `BigSpace` triplication a multi-viewport implementation
//! would require. Multi-document side-by-side viewports are a
//! follow-up — the singleton seam is where they'll plug in.
//!
//! ## Pipeline
//!
//! ```text
//! UsdDocument source text
//!         │
//!         ▼  (on DocumentOpened / DocumentChanged for an active doc)
//! openusd::usda::parser  →  TextReader  →  UsdStageAsset
//!         │
//!         ▼  (Assets<UsdStageAsset>::get_mut, in-place swap)
//! Handle<UsdStageAsset>
//!         │
//!         ▼  (UsdPrimPath { stage_handle, path: "/" } on scene_root)
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
//! - [`DocumentOpened`] for our kind
//!   → bootstrap render scaffolding on first open, set this doc as
//!   the active viewport target, parse + install asset, mount on
//!   `scene_root`.
//! - [`lunco_doc_bevy::DocumentChanged`] for the
//!   active doc → re-parse, **mutate the asset in-place** so the
//!   `Handle<UsdStageAsset>` stays valid, despawn synced children,
//!   clear the `UsdVisualSynced` marker on `scene_root` so
//!   `sync_usd_visuals` re-runs.
//! - [`DocumentClosed`] → if it was
//!   the active doc, drop the asset and clear `scene_root`'s
//!   `UsdPrimPath`. Render scaffolding (image, camera, BigSpace) is
//!   kept warm so the next open doesn't pay the bootstrap cost.
//!
//! ## What this plugin does *not* do
//!
//! - Camera orbit / pan / zoom controls. Camera transform is fixed
//!   today; orbit lands as a follow-up that reads egui pointer
//!   events.
//! - Multiple simultaneous viewports / split views.
//! - USD composition (`UsdComposer::flatten`). Sublayers /
//!   references resolve only when the canonical asset loader is
//!   used (i.e. drag-drop / `asset_server.load`); workbench-driven
//!   docs walk only the root layer until the composer is wired into
//!   the in-place rebuild path.

use bevy::prelude::*;
use bevy::camera::{ImageRenderTarget, RenderTarget};
use bevy::image::Image;
use bevy::render::render_resource::{Extent3d, TextureFormat};
use bevy::camera::visibility::RenderLayers;
use bevy_egui::egui;
use bevy_egui::{EguiTextureHandle, EguiUserTextures};
use lunco_assets::twin_source::TwinRoots;
use lunco_doc::{Document, DocumentId, DocumentOrigin};
use lunco_doc_bevy::{DocumentChanged, DocumentClosed, DocumentOpened};
use lunco_usd_bevy::{UsdPreviewOnly, UsdPrimPath, UsdStageAsset, UsdVisualSynced};
use lunco_core::{Command, on_command, register_commands};
use lunco_workbench::{
    Panel, PanelCtx, PanelId, PanelRects, PanelSlot, ScenePickGate, SceneTarget, WorkbenchAppExt,
};

use crate::document::UsdDocument;
use lunco_doc_bevy::DocumentRegistry;

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
const PREVIEW_RENDER_LAYER: usize = 1;

/// Plugin that wires the viewport pipeline. Must be added together
/// with `DefaultPlugins` (or any plugin set that ships
/// `Assets<Image>` + the rendering schedule) — gated checks make the
/// observers no-op when those resources are absent so headless tests
/// still link cleanly.
pub struct UsdViewportPlugin;

impl Plugin for UsdViewportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UsdViewportState>();
        app.register_panel(UsdViewportPanel);
        app.add_observer(on_doc_opened_for_viewport);
        app.add_observer(on_doc_changed_for_viewport);
        app.add_observer(on_doc_closed_for_viewport);
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
        // Defaults derived from the previous fixed camera pose
        // (4, 3, 5) looking at the origin — same framing, now movable.
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
        self.pitch =
            (self.pitch + delta.y * self.drag_sensitivity).clamp(-self.pitch_clamp, self.pitch_clamp);
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

/// Singleton state for the shared USD preview viewport. One render
/// target, one camera, one scene_root; retargets to whichever doc is
/// currently active. Built lazily on first preview request and kept
/// warm afterwards.
#[derive(Resource, Default)]
pub struct UsdViewportState {
    bootstrapped: bool,
    image: Option<Handle<Image>>,
    tex_id: Option<egui::TextureId>,
    scene_root: Option<Entity>,
    camera: Option<Entity>,
    light: Option<Entity>,
    current_handle: Option<Handle<UsdStageAsset>>,
    active_doc: Option<DocumentId>,
    last_rebuilt_generation: Option<u64>,
    /// Pointer-driven orbit pose. Pushed onto the camera each input
    /// frame the panel receives drag / scroll input.
    pub orbit: OrbitCamera,
}

impl UsdViewportState {
    /// The doc currently surfaced in the viewport, if any.
    pub fn active_doc(&self) -> Option<DocumentId> {
        self.active_doc
    }
}

// ─────────────────────────────────────────────────────────────────────
// Bootstrap
// ─────────────────────────────────────────────────────────────────────

/// First-time setup of the shared render scaffolding. Idempotent;
/// no-ops when `Assets<Image>` is absent (headless tests / server
/// bins).
///
/// The preview camera is spawned with `Camera::order = 1` so it never
/// collides with the main window camera (`order = 0`); they target
/// different surfaces anyway (window vs. image), but explicit ordering
/// silences Bevy's order-ambiguity warning that compares all active
/// cameras regardless of target.
fn bootstrap(world: &mut World) {
    if world.resource::<UsdViewportState>().bootstrapped {
        return;
    }
    if !world.contains_resource::<Assets<Image>>() {
        return;
    }

    // Bootstrap with a tiny placeholder. `resize_viewport_image` will
    // grow it to the actual `UsdViewportPanel` rect on the first
    // Update tick after the panel records its rect into `PanelRects`.
    // This keeps wgpu's initial alloc small (32×32×4 = 4KB instead of
    // 1280×800×4 = ~4MB) while still presenting a valid texture for
    // the camera and the egui `Image` widget on frame 1.
    let image_handle = {
        let image = make_target_image(PLACEHOLDER_WIDTH, PLACEHOLDER_HEIGHT);
        world.resource_mut::<Assets<Image>>().add(image)
    };

    let tex_id = world
        .get_resource_mut::<EguiUserTextures>()
        .map(|mut tex| tex.add_image(EguiTextureHandle::Strong(image_handle.clone())));

    let preview_layers = RenderLayers::layer(PREVIEW_RENDER_LAYER);

    let mut commands = world.commands();
    let camera = commands
        .spawn((
            Camera3d::default(),
            Camera {
                clear_color: ClearColorConfig::Custom(Color::srgb(0.10, 0.10, 0.12)),
                // Explicit non-zero order so Bevy's camera-order-
                // ambiguity check ignores us. The main window camera
                // ships at order 0; we sit at 1.
                order: 1,
                ..default()
            },
            // `RenderTarget::Image` keeps `sync_gizmo_camera` from
            // tagging this camera (it filters on `RenderTarget::Window`).
            RenderTarget::Image(ImageRenderTarget::from(image_handle.clone())),
            OrbitCamera::default().transform(),
            // Preview-only render layer: this camera will render
            // *only* entities tagged with `PREVIEW_RENDER_LAYER`, so
            // the live sim scene (default layer 0) stays invisible to
            // it. Propagated to every USD prim descendant of
            // `scene_root` by `propagate_preview_render_layer`.
            preview_layers.clone(),
            Name::new("UsdViewportCamera"),
        ))
        .id();

    let light = commands
        .spawn((
            DirectionalLight {
                illuminance: 8_000.0,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_xyz(5.0, 10.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
            preview_layers.clone(),
            Name::new("UsdViewportSun"),
        ))
        .id();

    let scene_root = commands
        .spawn((
            Transform::default(),
            Visibility::default(),
            Name::new("UsdViewportSceneRoot"),
            // Preview-only: usd-sim/usd-avian walk ChildOf up from each
            // candidate prim and bail when they reach this marker, so
            // the preview stage never spawns an Avatar Camera3d into
            // the workbench window (which would cause camera-order
            // ambiguity + gizmo warnings every frame) or activate
            // wheel physics / FSW.
            UsdPreviewOnly,
            // Render-layer seed — `propagate_preview_render_layer`
            // copies it down to every descendant each frame so newly
            // spawned USD prims (meshes inherited from
            // `sync_usd_visuals`) automatically join the preview-only
            // render layer.
            preview_layers,
        ))
        .id();

    world.flush();

    let mut state = world.resource_mut::<UsdViewportState>();
    state.bootstrapped = true;
    state.image = Some(image_handle);
    state.tex_id = tex_id;
    state.camera = Some(camera);
    state.light = Some(light);
    state.scene_root = Some(scene_root);
}

/// Push `PREVIEW_RENDER_LAYER` onto every descendant of the preview
/// `scene_root` that doesn't yet have a `RenderLayers` component.
///
/// `sync_usd_visuals` (in `lunco-usd-bevy`) spawns child prim entities
/// without `RenderLayers`, which means they default to layer 0 and
/// would otherwise show up in the live workbench window. Walking from
/// `scene_root` each frame and inserting the preview layer on
/// missing-RenderLayers descendants gives us hierarchical scoping
/// without modifying the USD layer.
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
    let Some(root) = state.scene_root else { return };

    // Only re-walk the preview subtree when there's something new to seed:
    // either the scene root was just (re)assigned (`state` changed this
    // frame) or some entity was newly parented this frame (USD prims spawn
    // incrementally as the stage loads). Once the scene is static this DFS
    // would otherwise run every frame for no effect.
    if !state.is_changed() && q_newly_parented.is_empty() {
        return;
    }

    let preview_layers = RenderLayers::layer(PREVIEW_RENDER_LAYER);

    // Iterative DFS over the subtree rooted at scene_root. USD scenes
    // are shallow (tens-hundreds of prims) so allocation-free
    // traversal isn't worth the complexity.
    let mut stack: Vec<Entity> = Vec::with_capacity(32);
    if let Ok(children) = q_children.get(root) {
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
/// First-apply (`last_applied == 0`) fires unconditionally so the
/// placeholder texture from `bootstrap` snaps to panel size on the
/// first frame the panel is visible.
fn resize_viewport_image(
    // `Option` so the system is headless-safe — `PanelRects` is owned by
    // the workbench UI plugin, absent in lifecycle / headless tests.
    rects: Option<Res<PanelRects>>,
    state: Res<UsdViewportState>,
    images: Option<ResMut<Assets<Image>>>,
    mut last_applied: Local<UVec2>,
) {
    let Some(handle) = state.image.as_ref() else {
        return;
    };
    let (Some(rects), Some(mut images)) = (rects, images) else {
        return;
    };
    let Some(rect) = rects.get(USD_VIEWPORT_PANEL_ID) else {
        return;
    };
    let target = rect.size;
    let first_apply = last_applied.x == 0 || last_applied.y == 0;
    let dx = target.x.abs_diff(last_applied.x);
    let dy = target.y.abs_diff(last_applied.y);
    if !first_apply && dx < RESIZE_DELTA_PX && dy < RESIZE_DELTA_PX {
        return;
    }
    let Some(mut image) = images.get_mut(handle) else {
        return;
    };
    image.resize(Extent3d {
        width: target.x.max(1),
        height: target.y.max(1),
        depth_or_array_layers: 1,
    });
    *last_applied = target;
}

/// Construct a render-target image with sensible defaults
/// (Bgra8UnormSrgb, RENDER_ATTACHMENT). Wrapped so the bootstrap
/// reads cleanly.
fn make_target_image(width: u32, height: u32) -> Image {
    // `Image::new_target_texture` sets all three usage flags (incl.
    // RENDER_ATTACHMENT) and fills with zeros in 0.18, using
    // RenderAssetUsages::default(). We want a simple linear-RGBA
    // target — egui displays sRGB so Bgra8UnormSrgb keeps colours
    // right without an extra conversion pass.
    Image::new_target_texture(width, height, TextureFormat::Bgra8UnormSrgb, None)
}

// ─────────────────────────────────────────────────────────────────────
// SetActiveUsdViewport — typed command for "show this stage"
// ─────────────────────────────────────────────────────────────────────

/// Retarget the shared USD viewport at `doc`. Browser row clicks fire
/// this; HTTP API / MCP / scripts can fire it directly. Idempotent —
/// calling with the already-active doc is a no-op.
#[Command(default)]
pub struct SetActiveUsdViewport {
    /// The USD document to surface in the viewport.
    pub doc: DocumentId,
}

#[on_command(SetActiveUsdViewport)]
fn on_set_active_usd_viewport(
    trigger: On<SetActiveUsdViewport>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        if !world.resource::<DocumentRegistry<UsdDocument>>().contains(doc) {
            return;
        }
        if world.resource::<UsdViewportState>().active_doc == Some(doc) {
            return;
        }
        bootstrap(world);
        // Detach the prior stage so its asset ref-count drops before
        // we install the new one. `sync_usd_visuals` will respawn
        // children once the new `UsdPrimPath` lands.
        if let Some(scene_root) = world.resource::<UsdViewportState>().scene_root {
            if let Ok(mut entity) = world.get_entity_mut(scene_root) {
                entity.remove::<UsdPrimPath>();
                entity.remove::<UsdVisualSynced>();
                entity.despawn_related::<Children>();
            }
        }
        install_active_doc(world, doc);
    });
}

register_commands!(on_set_active_usd_viewport,);

// ─────────────────────────────────────────────────────────────────────
// Document lifecycle observers
// ─────────────────────────────────────────────────────────────────────

fn on_doc_opened_for_viewport(
    trigger: On<DocumentOpened>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        // Gate on USD ownership so Modelica / SysML opens skip.
        if !world.resource::<DocumentRegistry<UsdDocument>>().contains(doc) {
            return;
        }
        // Make this the active doc if nothing else is showing. The
        // user can switch later by clicking a different row in the
        // browser (which fires `SetActiveUsdViewport`).
        if world.resource::<UsdViewportState>().active_doc.is_none() {
            bootstrap(world);
            install_active_doc(world, doc);
        }
    });
}

fn on_doc_changed_for_viewport(_trigger: On<DocumentChanged>) {
    // Doc-backed viewport rebuilds are owned by `sync_twin_overlays`: when the
    // document generation moves it refreshes the twin overlay and reloads the
    // asset, re-projecting the preview. Nothing to do here per edit.
}

fn on_doc_closed_for_viewport(
    trigger: On<DocumentClosed>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        let mut state = world.resource_mut::<UsdViewportState>();
        if state.active_doc != Some(doc) {
            return;
        }
        state.active_doc = None;
        state.current_handle = None;
        let scene_root = state.scene_root;
        drop(state);
        if let Some(root) = scene_root {
            if let Ok(mut entity) = world.get_entity_mut(root) {
                entity.remove::<UsdPrimPath>();
                entity.remove::<UsdVisualSynced>();
                entity.despawn_related::<Children>();
            }
        }
    });
}

// ─────────────────────────────────────────────────────────────────────
// Asset install / rebuild
// ─────────────────────────────────────────────────────────────────────

/// Install `doc` as the active stage on the shared scene_root. Composes the
/// document through the **storage-based async twin loader** (no filesystem
/// compose): the doc is served as a `twin://` byte-overlay and loaded via the
/// async [`UsdLoader`], which resolves references relative to the doc's base dir
/// through the twin source (web-ready). Later edits are re-projected by
/// [`sync_twin_overlays`](crate::twin_projection::sync_twin_overlays). No-op
/// when scaffolding hasn't been bootstrapped (headless).
fn install_active_doc(world: &mut World, doc: DocumentId) {
    let Some(scene_root) = world.resource::<UsdViewportState>().scene_root else {
        return;
    };
    let doc_generation = world
        .resource::<DocumentRegistry<UsdDocument>>()
        .host(doc)
        .map(|h| h.document().generation());
    let Some((name, rel)) = viewport_twin_coords(world, doc) else {
        bevy::log::warn!("[UsdViewport] no composed source for {} — not mounting", doc);
        return;
    };
    let handle = world
        .resource::<AssetServer>()
        .load::<UsdStageAsset>(lunco_assets::twin_uri(&name, &rel));
    // Track so `sync_twin_overlays` keeps the overlay + preview in step with the
    // document generation (it owns rebuilds — the viewport no longer re-parses).
    world
        .resource_mut::<crate::twin_projection::DocBackedTwinScenes>()
        .track(doc, name, rel);
    if let Ok(mut entity) = world.get_entity_mut(scene_root) {
        entity.remove::<UsdVisualSynced>();
        entity.despawn_related::<Children>();
        entity.insert(UsdPrimPath {
            stage_handle: handle.clone(),
            path: "/".to_string(),
        });
    }
    let mut state = world.resource_mut::<UsdViewportState>();
    state.active_doc = Some(doc);
    state.current_handle = Some(handle);
    state.last_rebuilt_generation = doc_generation;
}

/// The `twin://` coordinates (`name`, `rel`) to load `doc` through the async
/// twin source. Reuses the coordinates the document is already doc-backed under
/// (a default twin scene → shared overlay + asset), else registers a synthetic
/// per-document twin root and serves the doc's **composed** (`base ⊕ runtime`)
/// source as a byte-overlay so the async loader composes from the editable
/// document via storage — references resolve relative to the doc's base dir
/// through the twin source. `None` when the document is gone.
fn viewport_twin_coords(world: &World, doc: DocumentId) -> Option<(String, String)> {
    // Already doc-backed (e.g. the default twin scene)? Reuse its overlay + asset.
    if let Some(coords) = world
        .resource::<crate::twin_projection::DocBackedTwinScenes>()
        .coords_of(doc)
    {
        return Some(coords);
    }
    let host = world.resource::<DocumentRegistry<UsdDocument>>().host(doc)?;
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
    let name = format!("__viewport_{doc}")
        .replace(|c: char| !c.is_ascii_alphanumeric() && c != '_', "_");
    let roots = world.resource::<TwinRoots>();
    // Use the ASSIGNED name: if this synthetic name is already bound to a
    // different base (same doc re-registered from a new location), the registry
    // hands back a disambiguated one — and the overlay must be keyed to that,
    // or the viewport serves its composed source under a name nobody reads.
    let name = roots.register(&name, base);
    roots.set_overlay(&name, &rel, std::sync::Arc::new(composed.into_bytes()));
    Some((name, rel))
}

// ─────────────────────────────────────────────────────────────────────
// UsdViewportPanel
// ─────────────────────────────────────────────────────────────────────

/// Singleton workbench panel displaying the shared USD preview.
/// Retargets on `SetActiveUsdViewport`; one camera, one scene_root.
pub struct UsdViewportPanel;

impl Panel for UsdViewportPanel {
    fn id(&self) -> PanelId {
        USD_VIEWPORT_PANEL_ID
    }

    fn title(&self) -> String {
        "USD Preview".to_string()
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

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        // Record the panel's screen rect into `PanelRects` so
        // `resize_viewport_image` can match the offscreen Image's
        // pixel dimensions to it next tick. Measured here from the
        // read-only `ui` (before any widgets draw, so the rect reflects
        // the full panel body) and written after paint via `defer` —
        // the panel has no `&mut World`.
        let panel_rect = PanelRects::panel_rect_from_ui(ui);
        ctx.defer(move |world| {
            if let Some(mut rects) = world.get_resource_mut::<PanelRects>() {
                rects.record(USD_VIEWPORT_PANEL_ID, panel_rect);
            }
        });

        let (tex_id, active_doc) = ctx
            .resource::<UsdViewportState>()
            .map(|s| (s.tex_id, s.active_doc))
            .unwrap_or((None, None));
        let name = active_doc
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
                        "Click a stage in the USD section of the Twin browser \
                         to preview it here.",
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
        ctx.defer(move |world| {
            if let Some(mut gate) = world.get_resource_mut::<ScenePickGate>() {
                gate.record_scene_leaf(
                    SceneTarget::Offscreen(USD_VIEWPORT_PANEL_ID),
                    over_scene,
                );
            }
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
            // Apply the orbit input after the egui pass — mutation needs
            // `&mut World`, which the panel never holds during paint.
            ctx.defer(move |world| {
                let (camera_entity, transform) = {
                    let mut state = world.resource_mut::<UsdViewportState>();
                    if drag != egui::Vec2::ZERO {
                        state.orbit.apply_drag(drag);
                    }
                    if scroll_y != 0.0 {
                        state.orbit.apply_zoom(scroll_y);
                    }
                    (state.camera, state.orbit.transform())
                };
                if let Some(cam) = camera_entity {
                    if let Ok(mut entity) = world.get_entity_mut(cam) {
                        if let Some(mut tf) = entity.get_mut::<Transform>() {
                            *tf = transform;
                        }
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::UsdCommandsPlugin;

    /// Without any rendering plugins (`Assets<Image>` absent) the
    /// observers gracefully no-op — the state stays
    /// non-bootstrapped, no panic.
    #[test]
    fn lifecycle_is_headless_safe() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(UsdCommandsPlugin);
        app.add_plugins(UsdViewportPlugin);
        app.update();

        let _doc = {
            let mut reg = app.world_mut().resource_mut::<DocumentRegistry<UsdDocument>>();
            reg.open_file("/tmp/x.usda", "#usda 1.0\n".to_string()).0
        };
        // Drain pending events twice so the DocumentOpened trigger
        // fires and our observer runs.
        app.update();
        app.update();

        let state = app.world().resource::<UsdViewportState>();
        // No render scaffolding in MinimalPlugins → bootstrap bails.
        assert!(!state.bootstrapped);
        assert!(state.image.is_none());
        assert!(state.tex_id.is_none());
        // active_doc gates on bootstrap so we don't half-attach.
        assert!(state.active_doc.is_none());
    }
}
