//! Render runtime for USD preview sessions and views.
//!
//! A session owns one projected composed USD stage. Views are presentation
//! surfaces over that stage: each has its own camera, light, orbit pose, and
//! render target, while all views of one session share the same scene root and
//! render layer. This keeps multi-view editing cheap and prevents hidden dock
//! tabs from consuming a render pass.
//!
//! This package owns the Bevy cameras, lights, render targets, lifecycle, and
//! command/query boundary. The workbench panels that display those targets are
//! installed by `lunco-usd-viewport-ui`.
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
//!   the runtime does not re-parse or mutate an asset in-place.
//! - [`DocumentClosed`] → close every preview session for that document and
//!   release its render resources.
//!
//! ## What this plugin does *not* do
//!
//! - The viewport does not compose source text itself. The canonical stage
//!   projection owns sublayers, references, payloads, and variants; this
//!   runtime only binds the selected document's live stage to render resources.

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ImageRenderTarget, RenderTarget};
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureFormat};
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};
use bevy_egui::egui;
use bevy_egui::{EguiTextureHandle, EguiUserTextures};
use lunco_api::executor::{PendingApiRequest, finish_command_result};
use lunco_api_core::ApiErrorCode;
use lunco_assets_core::twin_source::TwinRoots;
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{ActiveCommandId, on_command, register_commands};
use lunco_doc::{Document, DocumentId, DocumentOrigin};
use lunco_doc_bevy::{DocumentChanged, DocumentClosed};
use lunco_render::{
    GraphicsCameraDefaults, LightGraphicsDefaults, RenderQualityProfile, RenderingQualitySettings,
    scene_camera_look_with_profile,
};
use lunco_settings::AppSettingsExt;
use lunco_usd_bevy_scene::{
    UsdPreviewOnly, UsdPrimPath, UsdSceneAwaitingStage, UsdSceneGeometryPending, UsdSceneProjected,
    UsdSceneProjectionFailed, UsdSceneProjectionQueued, UsdStageRevision, is_preview_entity,
};
use lunco_usd_bevy_stage::{UsdStageAsset, is_descendant_or_self};
use lunco_usd_viewport_core::{
    ApplyUsdInspectionPreset, CloseUsdPreview, CloseUsdPreviewView, DeleteUsdInspectionPreset,
    ExplodeUsdPreview, FocusUsdPreview, FocusUsdPreviewView, FrameUsdPreviewSelection,
    FrameUsdPreviewView, OpenUsdPreview, OpenUsdPreviewView, OrbitCamera, PanUsdPreviewView,
    ResetUsdPreviewView, SaveUsdInspectionPreset, SetUsdPreviewProjection, SetUsdPreviewTextLayer,
    SetUsdPreviewViewMode, UsdInspectionPreset, UsdInspectionSettings, UsdPreviewExplodeAction,
    UsdPreviewExplodeState, UsdPreviewExplodedPart, UsdPreviewId, UsdPreviewProjection,
    UsdPreviewSession, UsdPreviewView, UsdPreviewViewId, UsdPreviewViewMeasured,
    UsdViewportMeasured, UsdViewportOrbitInput, UsdViewportState, ZoomUsdPreviewView,
};
#[cfg(test)]
use lunco_usd_viewport_core::{UsdPreviewTextLayer, UsdPreviewViewMode};
use lunco_workbench_core::scene_pick::{ScenePickGate, SceneTarget};
use lunco_workbench_core::viewport::PanelRects;
use lunco_workbench_core::{
    PanelId, TabId,
    commands::{CloseTab, OpenTab},
    source::OpenTwinSource,
    tabs::PendingTabCloses,
};
use lunco_workspace::{TwinClosed, WorkspaceResource, document_belongs_to_twin_root};
use openusd::sdf::Path as SdfPath;

use lunco_doc_bevy::DocumentRegistry;
use lunco_usd_document::document::{LayerId, UsdDocument};

use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Stable id of the workbench tab the viewport renders into.
pub const USD_VIEWPORT_PANEL_ID: PanelId = PanelId("usd::viewport");

/// Instance-panel kind for additional views over an existing USD preview
/// session. The instance value is [`UsdPreviewViewId::0`].
pub const USD_PREVIEW_VIEW_PANEL_ID: PanelId = PanelId("usd::preview_view");

/// Initial placeholder dimensions for the offscreen render target.
/// Tiny on purpose: `resize_viewport_image` resizes the asset to the
/// actual panel rect on the first frame after the panel has been
/// drawn, so a small placeholder avoids allocating a multi-megabyte
/// texture that we'll throw away one frame later. If no preview panel
/// renders, the wasted buffer stays at this tiny size.
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

/// Render-only resources for one preview view. The session and presentation
/// state live in `lunco-usd-viewport-core`; this map keeps GPU/egui handles at
/// the render boundary so editor consumers do not depend on them.
struct UsdPreviewRenderTarget {
    image: Handle<Image>,
    tex_id: Option<egui::TextureId>,
}

#[derive(Resource, Default)]
pub struct UsdPreviewRenderTargets {
    views: HashMap<UsdPreviewViewId, UsdPreviewRenderTarget>,
}

impl UsdPreviewRenderTargets {
    fn insert(&mut self, view: UsdPreviewViewId, target: UsdPreviewRenderTarget) {
        self.views.insert(view, target);
    }

    fn remove(&mut self, view: UsdPreviewViewId) -> Option<UsdPreviewRenderTarget> {
        self.views.remove(&view)
    }

    fn get(&self, view: UsdPreviewViewId) -> Option<&UsdPreviewRenderTarget> {
        self.views.get(&view)
    }

    /// Return the egui texture registered for one preview view.
    pub fn texture_id(&self, view: UsdPreviewViewId) -> Option<egui::TextureId> {
        self.views.get(&view).and_then(|target| target.tex_id)
    }
}

#[derive(Resource, Default)]
struct PendingUsdPreviewTextReads {
    next_request: u64,
    tasks: Vec<PendingUsdPreviewTextRead>,
}

struct PendingUsdPreviewTextRead {
    preview: UsdPreviewId,
    doc: DocumentId,
    generation: u64,
    request: u64,
    #[cfg(not(target_arch = "wasm32"))]
    task: Task<Result<(String, String), String>>,
    #[cfg(target_arch = "wasm32")]
    result: crossbeam_channel::Receiver<Result<(String, String), String>>,
}

/// `RenderLayers` channel used to isolate USD preview rendering from
/// the main simulation world. Every entity in the preview scene
/// (camera, light, scene_root, and propagated descendants) lives on
/// this layer; the live workbench window camera stays on the default
/// layer 0, so its rendered output never includes preview meshes and
/// the preview camera never sees the live scene. Layer 0 is Bevy's
/// default; using layer 1 here keeps us clear of any third-party
/// systems that might assume layer 0.
#[cfg(test)]
const FIRST_PREVIEW_RENDER_LAYER: usize = 1;

/// Plugin that wires the USD preview render runtime. Must be added together
/// with `DefaultPlugins` (or any plugin set that ships
/// `Assets<Image>` + the rendering schedule) — gated checks make the
/// observers no-op when those resources are absent so headless tests
/// still link cleanly. Workbench panels are installed by the separate
/// `lunco-usd-viewport-ui` package.
pub struct UsdViewportPlugin;

impl Plugin for UsdViewportPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UsdViewportState>();
        app.register_settings_section::<UsdInspectionSettings>();
        app.init_resource::<PendingUsdPreviewTextReads>();
        app.init_resource::<UsdPreviewRenderTargets>();
        app.init_resource::<UsdPreviewRenderBudget>();
        app.init_resource::<UsdPreviewFrameVisibility>();
        app.init_resource::<RenderingQualitySettings>();
        app.add_observer(on_twin_closed_for_viewport);
        app.add_observer(on_doc_closed_for_viewport);
        app.add_observer(on_doc_changed_for_preview_text);
        app.add_observer(on_usd_document_ready);
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
                drain_pending_usd_preview_text_reads,
                reconcile_preview_projection_state
                    .run_if(preview_projection_inputs_changed)
                    .after(lunco_usd_bevy_scene::UsdVisualProjectionSet),
            ),
        );
        register_all_commands(app);
    }
}

/// Queue one authored/composed text snapshot for a preview session. The
/// document is cloned at the generation boundary and serialized on the async
/// worker, so egui never performs USDA serialization. A second request while
/// one is running only advances `requested_generation`; the completion drain
/// starts the newest generation once, which keeps rapid edits coalesced.
fn request_preview_text_read(world: &mut World, preview: UsdPreviewId) {
    let Some((doc, generation, document)) = world
        .get_resource::<DocumentRegistry<UsdDocument>>()
        .and_then(|registry| {
            let session_doc = world
                .get_resource::<UsdViewportState>()
                .and_then(|state| state.session(preview))
                .map(UsdPreviewSession::doc)?;
            let host = registry.host(session_doc)?;
            Some((session_doc, host.generation(), host.document().clone()))
        })
    else {
        return;
    };

    let request = {
        let mut pending = world.resource_mut::<PendingUsdPreviewTextReads>();
        pending.next_request = pending.next_request.wrapping_add(1);
        pending.next_request
    };
    let should_spawn = {
        let mut state = world.resource_mut::<UsdViewportState>();
        let Some(session) = state.session_mut(preview) else {
            return;
        };
        if session.doc != doc {
            return;
        }
        session.text.requested_generation = Some(generation);
        if session.text.loading
            || (session.text.displayed_generation == Some(generation)
                && session.text.authored.is_some()
                && session.text.composed.is_some())
        {
            false
        } else {
            session.text.loading = true;
            session.text.error = None;
            session.text.request = request;
            true
        }
    };
    if !should_spawn {
        return;
    }

    #[cfg(not(target_arch = "wasm32"))]
    let task = AsyncComputeTaskPool::get()
        .spawn(async move { Ok((document.source(), document.composed_source())) });
    #[cfg(not(target_arch = "wasm32"))]
    world
        .resource_mut::<PendingUsdPreviewTextReads>()
        .tasks
        .push(PendingUsdPreviewTextRead {
            preview,
            doc,
            generation,
            request,
            task,
        });

    #[cfg(target_arch = "wasm32")]
    {
        let (tx, result) = crossbeam_channel::bounded(1);
        wasm_bindgen_futures::spawn_local(async move {
            let _ = tx.send(Ok((document.source(), document.composed_source())));
        });
        world
            .resource_mut::<PendingUsdPreviewTextReads>()
            .tasks
            .push(PendingUsdPreviewTextRead {
                preview,
                doc,
                generation,
                request,
                result,
            });
    }
}

/// Document edits invalidate the text snapshot for every view over that
/// document. The observer queues a single session read; the helper coalesces
/// further edits behind its in-flight generation.
fn on_doc_changed_for_preview_text(trigger: On<DocumentChanged>, mut commands: Commands) {
    let doc = trigger.event().doc;
    commands.queue(move |world: &mut World| {
        // Document generation is not visual readiness. Invalidate every
        // preview lease before the canonical stage applies this generation;
        // transient explode transforms must not leak into the rebuilt stage.
        let restores = world
            .resource_mut::<UsdViewportState>()
            .invalidate_projection(doc);
        for (entity, transform) in restores {
            if let Ok(mut entity) = world.get_entity_mut(entity) {
                if let Some(mut current) = entity.get_mut::<Transform>() {
                    *current = transform;
                }
            }
        }
        let previews = world
            .resource::<UsdViewportState>()
            .session_ids_for_doc(doc);
        for preview in previews {
            request_preview_text_read(world, preview);
        }
    });
}

/// Apply completed text snapshots only when both the request and document
/// generation still match. A stale completion is discarded and immediately
/// replaced by the newest requested generation.
fn drain_pending_usd_preview_text_reads(world: &mut World) {
    let tasks = std::mem::take(&mut world.resource_mut::<PendingUsdPreviewTextReads>().tasks);
    let mut pending = Vec::new();
    for mut read in tasks {
        #[cfg(not(target_arch = "wasm32"))]
        let result = block_on(future::poll_once(&mut read.task));
        #[cfg(target_arch = "wasm32")]
        let result = read.result.try_recv().ok();
        let Some(result) = result else {
            pending.push(read);
            continue;
        };

        let current_generation = world
            .resource::<DocumentRegistry<UsdDocument>>()
            .host(read.doc)
            .map(|host| host.generation());
        let mut retry = false;
        {
            let mut state = world.resource_mut::<UsdViewportState>();
            let Some(session) = state.session_mut(read.preview) else {
                continue;
            };
            if session.doc != read.doc || session.text.request != read.request {
                continue;
            }
            let current = current_generation == Some(read.generation)
                && session.text.requested_generation == Some(read.generation);
            session.text.loading = false;
            if !current {
                retry = true;
            } else {
                match result {
                    Ok((authored, composed)) => {
                        session.text.authored = Some(authored);
                        session.text.composed = Some(composed);
                        session.text.displayed_generation = Some(read.generation);
                        session.text.error = None;
                    }
                    Err(error) => session.text.error = Some(error),
                }
            }
        }
        if retry {
            request_preview_text_read(world, read.preview);
        }
    }
    world.resource_mut::<PendingUsdPreviewTextReads>().tasks = pending;
}

/// Bind a browser-admitted document to its document-scoped preview lease and
/// focus its identifiable USD view tab. The document and root edit target
/// remain explicit, while repeated clicks reuse the same document and view.
fn on_usd_document_ready(
    trigger: On<lunco_usd_core::commands::UsdDocumentReady>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    viewport: Res<UsdViewportState>,
    workspace: Option<Res<WorkspaceResource>>,
    mut commands: Commands,
) {
    let doc = trigger.event().doc;
    let preview = viewport
        .preview_for_document(doc)
        .unwrap_or_else(|| UsdPreviewId::for_document(doc));
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
                commands.trigger(OpenTwinSource {
                    twin_root: root.to_string_lossy().into_owned(),
                    relative_path: relative.to_string_lossy().into_owned(),
                    pinned: false,
                    focus: Some(false),
                });
            }
        }
    }
    commands.trigger(OpenUsdPreview {
        preview,
        doc_id: doc,
        edit_target: LayerId::root(),
    });
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
            Added<UsdSceneProjected>,
            Changed<UsdSceneProjected>,
            Added<UsdSceneAwaitingStage>,
            Changed<UsdSceneAwaitingStage>,
            Added<UsdSceneProjectionQueued>,
            Changed<UsdSceneProjectionQueued>,
            Added<UsdSceneGeometryPending>,
            Changed<UsdSceneGeometryPending>,
            Added<UsdSceneProjectionFailed>,
            Changed<UsdSceneProjectionFailed>,
        )>,
    >,
    mut removed_paths: RemovedComponents<UsdPrimPath>,
    mut removed_awaiting: RemovedComponents<UsdSceneAwaitingStage>,
    mut removed_queued: RemovedComponents<UsdSceneProjectionQueued>,
    mut removed_meshes: RemovedComponents<UsdSceneGeometryPending>,
    mut removed_failures: RemovedComponents<UsdSceneProjectionFailed>,
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
            Has<UsdSceneProjected>,
            Has<UsdSceneProjectionFailed>,
            Has<UsdSceneAwaitingStage>,
            Has<UsdSceneProjectionQueued>,
            Has<UsdSceneGeometryPending>,
        ),
        With<UsdPreviewOnly>,
    >,
    prims: Query<(
        Entity,
        &UsdPrimPath,
        Has<UsdSceneProjected>,
        Has<UsdSceneAwaitingStage>,
        Has<UsdSceneProjectionQueued>,
        Has<UsdSceneGeometryPending>,
        Has<UsdSceneProjectionFailed>,
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
            |(_, path, synced, failed, awaiting, queued, mesh_pending)| {
                path.stage_handle.id() == stage_id
                    && synced
                    && !failed
                    && !awaiting
                    && !queued
                    && !mesh_pending
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
                    path.stage_handle.id() == stage_id && is_preview_entity(*entity, root, &parents)
                })
                .all(|(_, _, synced, awaiting, queued, mesh_pending, failed)| {
                    synced && !awaiting && !queued && !mesh_pending && !failed
                });
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

fn on_viewport_measured(
    trigger: On<UsdViewportMeasured>,
    rects: Res<PanelRects>,
    mut gate: ResMut<ScenePickGate>,
    mut state: ResMut<UsdViewportState>,
    mut visibility: ResMut<UsdPreviewFrameVisibility>,
    budget: Res<UsdPreviewRenderBudget>,
    mut cameras: Query<&mut Camera>,
) {
    let event = trigger.event();
    gate.record_scene_leaf(
        SceneTarget::Offscreen(USD_VIEWPORT_PANEL_ID),
        event.over_scene,
    );
    if let Some(view) = state.view_mut(event.view) {
        view.interactive_rect = event.visible.then_some(event.image_rect).flatten();
    }
    if event.visible {
        if let Some(rect) = event
            .image_rect
            .or_else(|| rects.get(USD_VIEWPORT_PANEL_ID))
        {
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
}

fn on_preview_view_measured(
    trigger: On<UsdPreviewViewMeasured>,
    rects: Res<PanelRects>,
    mut state: ResMut<UsdViewportState>,
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
    if let Some(view) = state.view_mut(event.view) {
        view.interactive_rect = event.visible.then_some(event.image_rect).flatten();
    }
    if event.visible {
        if let Some(rect) = event
            .image_rect
            .or_else(|| rects.get_instance(USD_PREVIEW_VIEW_PANEL_ID, event.view.0))
        {
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
    if input.drag == Vec2::ZERO && input.pan == Vec2::ZERO && input.scroll_y == 0.0 {
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
    let perspective_fov = match &*projection {
        Projection::Perspective(projection) => Some(projection.fov),
        Projection::Orthographic(_) => None,
        Projection::Custom(_) => None,
    };
    if input.pan != Vec2::ZERO
        && !view.orbit.apply_pan(
            [input.pan.x, input.pan.y],
            Vec2::new(input.viewport_size.x, input.viewport_size.y),
            perspective_fov,
            view.projection,
            view.orthographic_scale,
        )
    {
        return;
    }
    if input.drag != Vec2::ZERO {
        view.orbit.apply_drag([input.drag.x, input.drag.y]);
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
            // the preview stage never spawns an Embodiment Camera3d into
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

    Some(UsdPreviewSession::new(
        id,
        doc,
        edit_target,
        scene_root,
        stage_handle,
        render_layer,
        primary_view,
    ))
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
) -> Option<(UsdPreviewView, UsdPreviewRenderTarget)> {
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
    // A physical lunar scene can legitimately have near-black shadow cores,
    // but an editor preview must still expose the shape of an assembly. This
    // presentation-only, shadow-free fill is scoped to this view's render
    // layer and tracked in the view state so its lifecycle is deterministic.
    let fill_light = commands
        .spawn((
            DirectionalLight {
                color: Color::linear_rgb(0.55, 0.62, 0.75),
                illuminance: profile.distant_light_default_illuminance * 0.12,
                shadow_maps_enabled: false,
                ..default()
            },
            LightGraphicsDefaults {
                intensity_uses_graphics_default: true,
                intensity_scale: 1.0,
                range_uses_graphics_default: false,
            },
            Transform::from_xyz(-5.0, 6.0, -5.0).looking_at(Vec3::ZERO, Vec3::Y),
            RenderLayers::layer(render_layer),
            Name::new(format!("UsdPreviewFill-{}-{}", preview.0, view.0)),
        ))
        .id();
    world.flush();
    Some((
        UsdPreviewView::new(view, preview, camera, light, fill_light),
        UsdPreviewRenderTarget { image, tex_id },
    ))
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
    q_paths: Query<(Entity, &UsdPrimPath)>,
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
            state.session(view.preview()).map(|session| {
                (
                    view.id,
                    view.camera,
                    session.scene_root(),
                    view.frame_target.clone(),
                )
            })
        })
        .collect();

    for (view_id, camera, root, frame_target) in pending {
        if !scan_all
            && !q_added_bounds
                .iter()
                .any(|entity| is_descendant_of(entity, root, &q_children))
        {
            continue;
        }
        let bounds_root = if let Some(path) = frame_target.as_deref() {
            let Some(stage_id) = state
                .view(view_id)
                .and_then(|view| state.session(view.preview()))
                .map(|session| session.stage_handle().id())
            else {
                continue;
            };
            let Some((entity, _)) = q_paths
                .iter()
                .find(|(_, prim)| prim.stage_handle.id() == stage_id && prim.path == path)
            else {
                // Keep the target pending until the exact prim is projected;
                // framing the whole assembly here would hide a stale path.
                continue;
            };
            entity
        } else {
            root
        };
        let Some((center, radius)) = preview_visual_bounds(bounds_root, &q_children, &q_bounds)
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
        view.frame_target = None;
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
    root: Entity,
    q_children: &Query<&Children>,
    q_bounds: &Query<(&GlobalTransform, &Aabb)>,
) -> Option<(Vec3, f32)> {
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

    for session in state.sessions() {
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
    render_targets: Res<UsdPreviewRenderTargets>,
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
        let Some(requested) = view.interactive_rect.or(rect).map(|rect| rect.size) else {
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
        let Some(render_target) = render_targets.get(view.id()) else {
            continue;
        };
        if let Some(mut image) = images.get_mut(&render_target.image) {
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
#[on_command(OpenUsdPreview)]
fn on_open_usd_preview(trigger: On<OpenUsdPreview>, mut commands: Commands) {
    let command = trigger.event();
    let preview = command.preview;
    let doc = command.doc_id;
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
        }
        if world
            .resource::<UsdViewportState>()
            .session(preview)
            .is_some_and(|session| session.doc() == doc)
        {
            let primary_view = {
                let mut state = world.resource_mut::<UsdViewportState>();
                if let Some(session) = state.session_mut(preview) {
                    session.edit_target = edit_target;
                }
                state.focus(preview);
                state.session(preview).map(UsdPreviewSession::primary_view)
            };
            if let Some(view) = primary_view {
                world.trigger(OpenTab {
                    kind: USD_PREVIEW_VIEW_PANEL_ID,
                    instance: view.0,
                });
            }
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
            .load::<UsdStageAsset>(lunco_assets_core::twin_uri(&name, &rel));
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
        let Some((view, render_target)) =
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
        world
            .resource_mut::<UsdPreviewRenderTargets>()
            .insert(primary_view, render_target);
        if let Err(view) = world.resource_mut::<UsdViewportState>().insert_view(view) {
            despawn_preview_view(world, *view);
            let _ = remove_preview_session(world, preview);
            report_preview_error(
                world,
                "usd-preview-open-failed",
                format!("primary view {} could not be registered", primary_view.0),
            );
            return;
        }
        world.trigger(OpenTab {
            kind: USD_PREVIEW_VIEW_PANEL_ID,
            instance: primary_view.0,
        });
        request_preview_text_read(world, preview);
        world
            .resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
            .track_preview(doc, name, rel);
        world
            .resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
            .acquire_preview(doc);
        let claimed = world
            .resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
            .claim_user(doc);
        if claimed {
            world.trigger(lunco_usd_bevy_twin::UsdDocumentUserOwned { doc });
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
        let Some((view_state, render_target)) =
            create_preview_view(world, preview, view, render_layer, profile)
        else {
            report_preview_error(
                world,
                "usd-preview-view-open-failed",
                "USD preview view resources could not be allocated".to_string(),
            );
            return;
        };
        world
            .resource_mut::<UsdPreviewRenderTargets>()
            .insert(view, render_target);
        if let Err(view_state) = world
            .resource_mut::<UsdViewportState>()
            .insert_view(view_state)
        {
            despawn_preview_view(world, *view_state);
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
        let primary_view = {
            let mut state = world.resource_mut::<UsdViewportState>();
            if !state.focus(preview) {
                None
            } else {
                state.session(preview).map(UsdPreviewSession::primary_view)
            }
        };
        if let Some(view) = primary_view {
            world.trigger(OpenTab {
                kind: USD_PREVIEW_VIEW_PANEL_ID,
                instance: view.0,
            });
        } else {
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
        if world.resource_mut::<UsdViewportState>().focus_view(view) {
            world.trigger(OpenTab {
                kind: USD_PREVIEW_VIEW_PANEL_ID,
                instance: view.0,
            });
        } else {
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

#[on_command(SetUsdPreviewViewMode)]
fn on_set_usd_preview_view_mode(trigger: On<SetUsdPreviewViewMode>, mut commands: Commands) {
    let command = trigger.event();
    let view = command.view;
    let mode = command.mode;
    commands.queue(move |world: &mut World| {
        let missing = {
            let mut state = world.resource_mut::<UsdViewportState>();
            if let Some(view_state) = state.view_mut(view) {
                view_state.mode = mode;
                false
            } else {
                true
            }
        };
        if missing {
            report_preview_error(
                world,
                "usd-preview-mode-failed",
                format!("view {} is not open", view.0),
            );
        }
    });
}

#[on_command(SetUsdPreviewTextLayer)]
fn on_set_usd_preview_text_layer(trigger: On<SetUsdPreviewTextLayer>, mut commands: Commands) {
    let command = trigger.event();
    let view = command.view;
    let layer = command.layer;
    commands.queue(move |world: &mut World| {
        let missing = {
            let mut state = world.resource_mut::<UsdViewportState>();
            if let Some(view_state) = state.view_mut(view) {
                view_state.text_layer = layer;
                false
            } else {
                true
            }
        };
        if missing {
            report_preview_error(
                world,
                "usd-preview-text-layer-failed",
                format!("view {} is not open", view.0),
            );
        }
    });
}

#[on_command(SetUsdPreviewProjection)]
fn on_set_usd_preview_projection(trigger: On<SetUsdPreviewProjection>, mut commands: Commands) {
    let command = trigger.event();
    let view = command.view;
    let projection = command.projection;
    commands.queue(move |world: &mut World| {
        let view_data = {
            let mut viewport = world.resource_mut::<UsdViewportState>();
            if let Some(view_state) = viewport.view_mut(view) {
                view_state.projection = projection;
                view_state.auto_frame = true;
                Some((view_state.camera, view_state.orthographic_scale))
            } else {
                None
            }
        };
        let Some((camera, scale)) = view_data else {
            report_preview_error(
                world,
                "usd-preview-projection-failed",
                format!("view {} is not open", view.0),
            );
            return;
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
        let missing = {
            let mut viewport = world.resource_mut::<UsdViewportState>();
            if let Some(view_state) = viewport.view_mut(view) {
                view_state.auto_frame = true;
                false
            } else {
                true
            }
        };
        if missing {
            report_preview_error(
                world,
                "usd-preview-frame-failed",
                format!("view {} is not open", view.0),
            );
        }
    });
}

#[on_command(FrameUsdPreviewSelection)]
fn on_frame_usd_preview_selection(trigger: On<FrameUsdPreviewSelection>, mut commands: Commands) {
    let command = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let valid = world
            .resource::<UsdViewportState>()
            .view(command.view)
            .is_some_and(|view| view.preview() == command.preview);
        if !valid {
            report_preview_error(
                world,
                "usd-preview-frame-selection-failed",
                format!(
                    "view {} is not open for preview {}",
                    command.view.0, command.preview.0
                ),
            );
            return;
        }
        if command.path.is_empty() || !command.path.starts_with('/') {
            report_preview_error(
                world,
                "usd-preview-frame-selection-failed",
                "frame target must be an absolute USD prim path".to_string(),
            );
            return;
        }
        let mut viewport = world.resource_mut::<UsdViewportState>();
        let view = viewport
            .view_mut(command.view)
            .expect("preview view was validated above");
        view.frame_target = Some(command.path);
        view.auto_frame = true;
    });
}

fn preset_from_view(view: &UsdPreviewView, name: String) -> UsdInspectionPreset {
    UsdInspectionPreset {
        name,
        projection: view.projection,
        target: view.orbit.target.to_array(),
        yaw: view.orbit.yaw,
        pitch: view.orbit.pitch,
        distance: view.orbit.distance,
        orthographic_scale: view.orthographic_scale,
    }
}

fn apply_preset_to_view(view: &mut UsdPreviewView, preset: &UsdInspectionPreset) {
    view.projection = preset.projection;
    view.orbit.target = Vec3::from_array(preset.target);
    view.orbit.yaw = preset.yaw;
    view.orbit.pitch = preset
        .pitch
        .clamp(-view.orbit.pitch_clamp, view.orbit.pitch_clamp);
    view.orbit.distance = preset
        .distance
        .clamp(view.orbit.min_distance, view.orbit.max_distance);
    view.orthographic_scale = preset.orthographic_scale.clamp(
        view.orbit.min_orthographic_scale,
        view.orbit.max_orthographic_scale,
    );
    view.active_preset = Some(preset.name.clone());
}

#[on_command(SaveUsdInspectionPreset)]
fn on_save_usd_inspection_preset(trigger: On<SaveUsdInspectionPreset>, mut commands: Commands) {
    let command = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let name = command.name.trim();
        if name.is_empty() || name.len() > 96 {
            report_preview_error(
                world,
                "usd-inspection-preset-save-failed",
                "preset name must be 1..=96 characters".to_string(),
            );
            return;
        }
        let Some(preset) = world
            .resource::<UsdViewportState>()
            .view(command.view)
            .map(|view| preset_from_view(view, name.to_string()))
        else {
            report_preview_error(
                world,
                "usd-inspection-preset-save-failed",
                format!("view {} is not open", command.view.0),
            );
            return;
        };
        let mut settings = world.resource_mut::<UsdInspectionSettings>();
        if let Some(existing) = settings
            .presets
            .iter_mut()
            .find(|existing| existing.name == preset.name)
        {
            *existing = preset.clone();
        } else if settings.presets.len() < 32 {
            settings.presets.push(preset.clone());
        } else {
            report_preview_error(
                world,
                "usd-inspection-preset-save-failed",
                "the persisted USD inspection preset limit (32) is reached".to_string(),
            );
            return;
        }
        if let Some(view) = world
            .resource_mut::<UsdViewportState>()
            .view_mut(command.view)
        {
            view.active_preset = Some(preset.name);
        }
    });
}

#[on_command(ApplyUsdInspectionPreset)]
fn on_apply_usd_inspection_preset(trigger: On<ApplyUsdInspectionPreset>, mut commands: Commands) {
    let command = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let Some(preset) = world
            .resource::<UsdInspectionSettings>()
            .presets
            .iter()
            .find(|preset| preset.name == command.name)
            .cloned()
        else {
            report_preview_error(
                world,
                "usd-inspection-preset-apply-failed",
                format!("preset '{}' is not persisted", command.name),
            );
            return;
        };
        let Some(camera) = world
            .resource::<UsdViewportState>()
            .view(command.view)
            .map(UsdPreviewView::camera)
        else {
            report_preview_error(
                world,
                "usd-inspection-preset-apply-failed",
                format!("view {} is not open", command.view.0),
            );
            return;
        };
        {
            let mut viewport = world.resource_mut::<UsdViewportState>();
            let view = viewport
                .view_mut(command.view)
                .expect("preview view was validated above");
            apply_preset_to_view(view, &preset);
        }
        let orbit_transform = world
            .resource::<UsdViewportState>()
            .view(command.view)
            .map(|view| view.orbit.transform());
        if let Some(orbit_transform) = orbit_transform {
            if let Some(mut transform) = world.get_mut::<Transform>(camera) {
                *transform = orbit_transform;
            }
        }
        if let Some(view) = world.resource::<UsdViewportState>().view(command.view) {
            let projection = view.projection;
            let scale = view.orthographic_scale;
            if let Some(mut camera_projection) = world.get_mut::<Projection>(camera) {
                *camera_projection = preview_projection(projection, scale);
            }
        }
    });
}

#[on_command(DeleteUsdInspectionPreset)]
fn on_delete_usd_inspection_preset(trigger: On<DeleteUsdInspectionPreset>, mut commands: Commands) {
    let name = trigger.event().name.clone();
    commands.queue(move |world: &mut World| {
        let before = {
            let mut settings = world.resource_mut::<UsdInspectionSettings>();
            let before = settings.presets.len();
            settings.presets.retain(|preset| preset.name != name);
            before
        };
        let removed = before != world.resource::<UsdInspectionSettings>().presets.len();
        if !removed {
            report_preview_error(
                world,
                "usd-inspection-preset-delete-failed",
                format!("preset '{}' is not persisted", name),
            );
            return;
        }
        for view in world.resource_mut::<UsdViewportState>().views_mut() {
            if view.active_preset.as_deref() == Some(name.as_str()) {
                view.active_preset = None;
            }
        }
    });
}

#[on_command(ResetUsdPreviewView)]
fn on_reset_usd_preview_view(trigger: On<ResetUsdPreviewView>, mut commands: Commands) {
    let view = trigger.event().view;
    commands.queue(move |world: &mut World| {
        let view_data = {
            let mut viewport = world.resource_mut::<UsdViewportState>();
            if let Some(view_state) = viewport.view_mut(view) {
                view_state.orbit = OrbitCamera::default();
                view_state.orthographic_scale = 1.0;
                view_state.auto_frame = true;
                Some((
                    view_state.camera,
                    view_state.projection,
                    view_state.orthographic_scale,
                ))
            } else {
                None
            }
        };
        let Some((camera, mode, scale)) = view_data else {
            report_preview_error(
                world,
                "usd-preview-reset-failed",
                format!("view {} is not open", view.0),
            );
            return;
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
            let perspective_fov = match &projection {
                Projection::Perspective(projection) => Some(projection.fov),
                Projection::Orthographic(_) => None,
                Projection::Custom(_) => None,
            };
            let applied = view_state.orbit.apply_pan(
                delta,
                viewport_size,
                perspective_fov,
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
        let view_data = {
            let mut viewport = world.resource_mut::<UsdViewportState>();
            if let Some(view_state) = viewport.view_mut(view) {
                match view_state.projection {
                    UsdPreviewProjection::Perspective => {
                        view_state.orbit.distance = (view_state.orbit.distance * factor)
                            .clamp(view_state.orbit.min_distance, view_state.orbit.max_distance);
                    }
                    UsdPreviewProjection::Orthographic => {
                        view_state.orthographic_scale = (view_state.orthographic_scale * factor)
                            .clamp(
                                view_state.orbit.min_orthographic_scale,
                                view_state.orbit.max_orthographic_scale,
                            );
                    }
                }
                view_state.auto_frame = false;
                Some((view_state.camera, view_state.orthographic_scale))
            } else {
                None
            }
        };
        let Some((camera, scale)) = view_data else {
            report_preview_error(
                world,
                "usd-preview-zoom-failed",
                format!("view {} is not open", view.0),
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
        if session.doc() != command.doc_id {
            return Err(format!(
                "USD preview {} belongs to document {}, not document {}",
                command.preview.0,
                session.doc(),
                command.doc_id
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
        Has<UsdSceneProjected>,
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
                local: *local,
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
        if !is_descendant_or_self(&sdf_path, &command.assembly) || path == &command.assembly {
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
        locals.insert(snapshot.entity, snapshot.local);
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
                .is_none_or(|target| target.entity != part.entity)
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
                *transform = part.baseline;
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
            lunco_api_core::api_value!({
                "preview": command.preview.0,
                "doc_id": command.doc_id.raw(),
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
                        baseline: target.local,
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
        let mut transform = part.baseline;
        transform.translation += local_delta;
        if !finite_transform(&transform) {
            return Err(format!(
                "USD preview explode part `{}` produced a non-finite transform",
                part.path
            ));
        }
        applied.push((part.entity, transform));
        offsets.push(lunco_api_core::api_value!({
            "path": part.path.clone(),
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
        lunco_api_core::api_value!({
            "preview": command.preview.0,
            "doc_id": command.doc_id.raw(),
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
    on_set_usd_preview_view_mode,
    on_set_usd_preview_text_layer,
    on_set_usd_preview_projection,
    on_frame_usd_preview_view,
    on_frame_usd_preview_selection,
    on_reset_usd_preview_view,
    on_pan_usd_preview_view,
    on_zoom_usd_preview_view,
    on_save_usd_inspection_preset,
    on_apply_usd_inspection_preset,
    on_delete_usd_inspection_preset,
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
                .resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
                .coords_of(doc)
                .map(|(name, _)| matches!(world.resource::<TwinRoots>().root_for(&name), Ok(None)))
                .unwrap_or(true);
            if !needs_rehome {
                continue;
            }
            world
                .resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
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
        .load::<UsdStageAsset>(lunco_assets_core::twin_uri(&name, &rel));
    world
        .resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
        .track_preview(doc, name, rel);
    lunco_usd_bevy_twin::wake_twin_projection(world);
    let Some(scene_root) = world
        .resource::<UsdViewportState>()
        .session(preview)
        .map(UsdPreviewSession::scene_root)
    else {
        return;
    };
    if let Ok(mut entity) = world.get_entity_mut(scene_root) {
        entity.remove::<UsdSceneProjected>();
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
    world.trigger(lunco_telemetry_core::TelemetryEvent {
        name: name.to_string(),
        source: 0,
        severity: lunco_telemetry_core::Severity::Error,
        data: lunco_telemetry_core::TelemetryValue::String(detail),
        timestamp: 0.0,
    });
}

/// Remove the Bevy resources owned by one preview session. The document
/// projection is released separately so shared coordinates survive until
/// the final session closes.
fn remove_preview_session(world: &mut World, preview: UsdPreviewId) -> Option<DocumentId> {
    if let Some(mut pending) = world.get_resource_mut::<PendingUsdPreviewTextReads>() {
        pending.tasks.retain(|read| read.preview != preview);
    }
    let (session, views) = world.resource_mut::<UsdViewportState>().remove(preview)?;
    let doc = session.doc;
    if let Ok(mut entity) = world.get_entity_mut(session.scene_root) {
        entity.despawn_related::<Children>();
        entity.despawn();
    }
    for view in views {
        world.trigger(CloseTab {
            kind: USD_PREVIEW_VIEW_PANEL_ID,
            instance: view.id().0,
        });
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
    if let Ok(entity) = world.get_entity_mut(view.fill_light) {
        entity.despawn();
    }
    let Some(render_target) = world
        .resource_mut::<UsdPreviewRenderTargets>()
        .remove(view.id)
    else {
        return;
    };
    if let Some(mut textures) = world.get_resource_mut::<EguiUserTextures>() {
        textures.remove_image(render_target.image.id());
    }
    if let Some(mut images) = world.get_resource_mut::<Assets<Image>>() {
        images.remove(render_target.image.id());
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
        .resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
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
        .resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
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
            .resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
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

    // Resolve file-backed documents against the workspace first. The workspace
    // owns which Twin contains a file; the asset-root registry is a separate
    // projection and can briefly lag a Twin open/replacement. Ensure the exact
    // owning Twin is mounted before asking AssetServer to compose the preview.
    // Otherwise the synthetic viewport authority below makes every authored
    // `twin://<twin>/components/...` reference in the document unresolved.
    if matches!(host.document().origin(), DocumentOrigin::File { .. }) {
        let file_path = base.join(&rel);
        let workspace_result = world.get_resource::<WorkspaceResource>().map(|workspace| {
            workspace_twin_coordinates(workspace, world.resource::<TwinRoots>(), &file_path)
        });
        if let Some(result) = workspace_result {
            match result {
                Ok(Some(coords)) => return Some(coords),
                Ok(None) => {}
                Err(error) => {
                    report_preview_error(
                        world,
                        "twin-asset-mount-failed",
                        format!("could not mount the document's workspace Twin: {error}"),
                    );
                    return None;
                }
            }
        }

        // Preserve compatibility for a Twin authority mounted by a host that
        // does not install WorkspacePlugin. Among matching roots, choose the
        // deepest one so nested authorities remain deterministic.
        let roots = world.resource::<TwinRoots>();
        if let Ok(names) = roots.names() {
            let mut matches = names
                .into_iter()
                .filter_map(|name| {
                    let root = roots.root_for(&name).ok().flatten()?;
                    let relative = base.join(&rel).strip_prefix(&root).ok()?.to_path_buf();
                    Some((root, name, relative))
                })
                .collect::<Vec<_>>();
            matches.sort_by(|left, right| {
                right
                    .0
                    .components()
                    .count()
                    .cmp(&left.0.components().count())
                    .then_with(|| left.1.cmp(&right.1))
            });
            if let Some((_, name, relative)) = matches.into_iter().next() {
                return Some((name, relative.to_string_lossy().replace('\\', "/")));
            }
        }
    }

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
            world.trigger(lunco_telemetry_core::TelemetryEvent {
                name: "twin-asset-mount-failed".into(),
                source: 0,
                severity: lunco_telemetry_core::Severity::Error,
                data: lunco_telemetry_core::TelemetryValue::String(error.to_string()),
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
        world.trigger(lunco_telemetry_core::TelemetryEvent {
            name: "twin-asset-mount-failed".into(),
            source: 0,
            severity: lunco_telemetry_core::Severity::Error,
            data: lunco_telemetry_core::TelemetryValue::String(error.to_string()),
            timestamp: 0.0,
        });
        if let Err(cleanup_error) = world.resource::<TwinRoots>().unregister_name(&name) {
            world.trigger(lunco_telemetry_core::TelemetryEvent {
                name: "twin-asset-unmount-failed".into(),
                source: 0,
                severity: lunco_telemetry_core::Severity::Error,
                data: lunco_telemetry_core::TelemetryValue::String(cleanup_error.to_string()),
                timestamp: 0.0,
            });
        }
        return None;
    }
    Some((name, rel))
}

/// Return the canonical Twin authority and Twin-relative source path for a
/// file owned by the open workspace. Registering here is idempotent for an
/// already-mounted root and closes the ordering gap between workspace admission
/// and isolated preview creation.
fn workspace_twin_coordinates(
    workspace: &WorkspaceResource,
    roots: &TwinRoots,
    file_path: &Path,
) -> Result<Option<(String, String)>, lunco_assets_core::TwinRootsError> {
    let Some((twin, relative)) = workspace
        .twins()
        .filter_map(|(_, twin)| {
            let relative = file_path.strip_prefix(&twin.root).ok()?.to_path_buf();
            Some((twin, relative))
        })
        .max_by_key(|(twin, _)| twin.root.components().count())
    else {
        return Ok(None);
    };

    let name = roots.register_twin(twin)?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    Ok(Some((name, relative)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_render::SceneCamera;
    use lunco_usd_commands::UsdCommandsPlugin;
    use lunco_usd_document::document::UsdOp;
    use lunco_usd_viewport_core::UsdPreviewExplodeAxis;

    fn preview_quality() -> RenderQualityProfile {
        RenderQualityProfile {
            directional_shadow_map_size: 1024,
            point_shadow_map_size: 512,
            directional_cascades: 2,
            shadow_filtering_quality: lunco_render::ShadowFilteringQuality::Hardware2x2,
            max_directional_shadow_casters: 1,
            max_point_shadow_casters: 1,
            max_spot_shadow_casters: 1,
            shadow_budget_bytes: 64 * 1024 * 1024,
            horizon_shadow_cache_sun_threshold_deg: 0.2,
            horizon_march_steps: 24,
            horizon_cache_samples_per_axis: 1,
            shadow_minimum_distance: 0.1,
            shadow_first_cascade_far_bound: 20.0,
            shadow_maximum_distance: 600.0,
            shadow_cascade_overlap: 0.1,
            shadow_depth_bias: 0.1,
            shadow_normal_bias: 4.0,
            camera_exposure_ev100: 16.0,
            render_failure_quiet_period_secs: 0.5,
            render_failure_give_up_after_secs: 5.0,
            camera_bloom_intensity: 0.0,
            camera_bloom_low_frequency_boost: 0.0,
            distant_light_default_illuminance: 128_000.0,
            local_light_default_intensity: 1_000.0,
            rect_light_default_intensity: 10_000.0,
            dome_default_intensity: 1_000.0,
            local_light_default_range: 30.0,
            local_shadow_map_near_z: 0.2,
            dome_cubemap_face_size: 512,
            primitive_sphere_longitudes: 24,
            primitive_sphere_latitudes: 16,
            primitive_radial_segments: 32,
            primitive_capsule_longitudes: 16,
            primitive_capsule_latitudes: 8,
            terrain_mesh_cache_bytes: 256 * 1024 * 1024,
            terrain_derived_map_resolution: 512,
            terrain_derived_ao_directions: 4,
            terrain_derived_ao_steps: 4,
            terrain_derived_ao_radius_fraction: 0.1,
            terrain_derived_roughness_base: 0.6,
            terrain_derived_roughness_saturation_radians: 0.6,
            terrain_derived_texture_anisotropy: 1,
            terrain_rock_max_instances: 2_000,
            terrain_rock_mesh_buckets: 3,
            terrain_rock_mesh_cube_count: 2,
            terrain_rock_lod_start_distance: 1_500.0,
            terrain_rock_lod_fade_distance: 300.0,
            terrain_lod_tile_resolution: 33,
            terrain_lod_cinematic_resolution: 1025,
            terrain_lod_pixel_error: 4.0,
            terrain_lod_max_depth: 6,
            terrain_lod_probe_resolution: 5,
            terrain_lod_bakes_per_frame: 8,
            terrain_lod_max_inflight_bakes: 16,
            terrain_lod_tile_budget: 256,
            terrain_lod_cover_edits_per_frame: 16,
            terrain_lod_hysteresis_ratio: 1.2,
            terrain_lod_morph_start_ratio: 0.45,
            nurbs_surface_samples_per_control_span: 3,
            nurbs_surface_minimum_subdivisions: 6,
            nurbs_surface_maximum_subdivisions: 64,
            nurbs_trim_curve_samples: 12,
            nurbs_trim_minimum_subdivisions: 8,
            nurbs_trim_maximum_subdivisions: 48,
            curve_samples_per_segment: 4,
            curve_radial_segments: 6,
            ..Default::default()
        }
    }

    #[test]
    fn workspace_owned_preview_uses_the_mounted_twin_authority() {
        let temp_dir = if Path::new("/tmp").is_dir() {
            Path::new("/tmp").to_path_buf()
        } else {
            std::env::temp_dir()
        };
        let root = tempfile::Builder::new()
            .prefix("luncosim-preview-twin-")
            .tempdir_in(temp_dir)
            .expect("temporary Twin root");
        let twin = match lunco_twin::TwinMode::open(root.path()).expect("open Twin folder") {
            lunco_twin::TwinMode::Folder(twin) | lunco_twin::TwinMode::Twin(twin) => twin,
            lunco_twin::TwinMode::Orphan(_) => panic!("a folder must open as a Twin"),
        };
        let twin_root = twin.root.clone();
        let expected_name = twin_root
            .file_name()
            .expect("temporary folder has a name")
            .to_string_lossy()
            .into_owned();
        let file_path = twin_root.join("vehicles/griffin_1_visual.usda");

        let mut workspace = WorkspaceResource::new();
        workspace.add_twin(twin);
        let roots = TwinRoots::default();

        let (name, relative) = workspace_twin_coordinates(&workspace, &roots, &file_path)
            .expect("workspace Twin should mount")
            .expect("document path belongs to the workspace Twin");

        assert_eq!(name, expected_name);
        assert_eq!(relative, "vehicles/griffin_1_visual.usda");
        assert_eq!(
            roots.root_for(&name).expect("read mounted root"),
            Some(twin_root.canonicalize().expect("canonical Twin root"))
        );
    }

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
        assert!(
            app.world()
                .resource::<DocumentRegistry<UsdDocument>>()
                .contains(doc)
        );
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
            UsdSceneProjected,
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
                UsdSceneProjected,
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
                UsdSceneProjected,
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
                UsdSceneProjected,
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
                UsdSceneProjected,
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
            doc_id: doc,
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
        let baseline_a = *app.world().get::<Transform>(part_a).unwrap();
        let baseline_b = *app.world().get::<Transform>(part_b).unwrap();

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
        let expected_parts =
            lunco_api_core::api_value!(["/Scene/Assembly/Group/PartB", "/Scene/Assembly/PartA",]);
        assert_eq!(
            enabled.data.as_ref().and_then(|data| data.get("parts")),
            Some(&expected_parts)
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
        assert!(
            app.world()
                .resource::<UsdViewportState>()
                .session(preview)
                .unwrap()
                .explode
                .is_none()
        );
    }

    #[test]
    fn explode_rejects_non_assembly_and_stale_targets_without_mutation() {
        let (mut app, preview, doc, part_a, _) = explode_fixture();
        let before = *app.world().get::<Transform>(part_a).unwrap();
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
        let baseline_a = *app.world().get::<Transform>(part_a).unwrap();
        let baseline_b = *app.world().get::<Transform>(part_b).unwrap();
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
            "preview", "doc_id", "assembly", "parts", "action", "axis", "spacing",
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
    fn preview_mode_commands_preserve_the_session_and_view_state() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Assets<Image>>();
        app.add_plugins(UsdViewportPlugin);

        let preview = UsdPreviewId(3);
        let view_id = UsdPreviewViewId(9);
        let session = create_preview_session(
            app.world_mut(),
            preview,
            DocumentId::new(4),
            LayerId::root(),
            Handle::default(),
            FIRST_PREVIEW_RENDER_LAYER,
            view_id,
        )
        .expect("preview session resources are available");
        let (view, _) = create_preview_view(
            app.world_mut(),
            preview,
            view_id,
            session.render_layer(),
            preview_quality(),
        )
        .expect("preview view resources are available");
        let orbit = view.orbit().clone();
        let scene_root = session.scene_root();
        let mut state = UsdViewportState::default();
        state.insert(session);
        assert!(state.insert_view(view).is_ok());
        app.insert_resource(state);

        app.world_mut().trigger(SetUsdPreviewViewMode {
            view: view_id,
            mode: UsdPreviewViewMode::Text,
        });
        app.world_mut().trigger(SetUsdPreviewTextLayer {
            view: view_id,
            layer: UsdPreviewTextLayer::Composed,
        });
        app.update();

        let state = app.world().resource::<UsdViewportState>();
        assert_eq!(state.session_count(), 1);
        assert_eq!(state.view_count(), 1);
        assert_eq!(state.session(preview).unwrap().scene_root(), scene_root);
        let view = state.view(view_id).expect("preview view remains open");
        assert_eq!(view.mode(), UsdPreviewViewMode::Text);
        assert_eq!(view.text_layer(), UsdPreviewTextLayer::Composed);
        assert_eq!(view.projection(), UsdPreviewProjection::Perspective);
        assert_eq!(view.orbit().yaw, orbit.yaw);
        assert_eq!(view.orbit().pitch, orbit.pitch);
        assert_eq!(view.orbit().distance, orbit.distance);
        assert_eq!(view.orbit().target, orbit.target);

        let registry = app.world().resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let schema = lunco_api::discovery::discover_commands(&registry, None);
        for (name, fields) in [
            ("SetUsdPreviewViewMode", ["view", "mode"].as_slice()),
            ("SetUsdPreviewTextLayer", ["view", "layer"].as_slice()),
        ] {
            let command = schema
                .iter()
                .find(|command| command.name == name)
                .unwrap_or_else(|| panic!("{name} command is registered"));
            assert!(!command.defaulted);
            for field in fields {
                assert!(
                    command
                        .fields
                        .iter()
                        .any(|candidate| candidate.name == *field),
                    "{name} schema must expose `{field}`"
                );
            }
        }
    }

    #[test]
    fn preview_text_reads_are_generation_matched_and_coalesced() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Assets<Image>>();
        app.init_resource::<DocumentRegistry<UsdDocument>>();
        app.init_resource::<UsdViewportState>();
        app.init_resource::<PendingUsdPreviewTextReads>();
        app.add_systems(Update, drain_pending_usd_preview_text_reads);

        let source = "#usda 1.0\ndef Xform \"Initial\" {}\n";
        let doc = app
            .world_mut()
            .resource_mut::<DocumentRegistry<UsdDocument>>()
            .open_file("/tmp/usd_preview_text_generation.usda", source.to_string())
            .0;
        let preview = UsdPreviewId(8);
        let view_id = UsdPreviewViewId(12);
        let session = create_preview_session(
            app.world_mut(),
            preview,
            doc,
            LayerId::root(),
            Handle::default(),
            FIRST_PREVIEW_RENDER_LAYER,
            view_id,
        )
        .expect("preview session resources are available");
        let (view, _) = create_preview_view(
            app.world_mut(),
            preview,
            view_id,
            session.render_layer(),
            preview_quality(),
        )
        .expect("preview view resources are available");
        let mut state = UsdViewportState::default();
        state.insert(session);
        assert!(state.insert_view(view).is_ok());
        app.insert_resource(state);

        request_preview_text_read(app.world_mut(), preview);
        assert_eq!(
            app.world()
                .resource::<PendingUsdPreviewTextReads>()
                .tasks
                .len(),
            1
        );
        for _ in 0..100 {
            app.update();
            if app
                .world()
                .resource::<UsdViewportState>()
                .session(preview)
                .unwrap()
                .text_ready()
            {
                break;
            }
            std::thread::yield_now();
        }
        let session = app
            .world()
            .resource::<UsdViewportState>()
            .session(preview)
            .unwrap();
        assert!(session.text_ready());
        assert!(
            session
                .text
                .authored
                .as_deref()
                .unwrap()
                .contains("Initial")
        );
        assert!(
            session
                .text
                .composed
                .as_deref()
                .unwrap()
                .contains("Initial")
        );

        let updated = "#usda 1.0\ndef Xform \"Second\" {}\n";
        let latest = "#usda 1.0\ndef Xform \"Latest\" {}\n";
        {
            let mut registry = app
                .world_mut()
                .resource_mut::<DocumentRegistry<UsdDocument>>();
            registry
                .apply(
                    doc,
                    UsdOp::ReplaceSource {
                        edit_target: LayerId::root(),
                        text: updated.to_string(),
                    },
                )
                .expect("first replacement applies");
            registry
                .apply(
                    doc,
                    UsdOp::ReplaceSource {
                        edit_target: LayerId::root(),
                        text: latest.to_string(),
                    },
                )
                .expect("second replacement applies");
        }
        request_preview_text_read(app.world_mut(), preview);
        request_preview_text_read(app.world_mut(), preview);
        assert_eq!(
            app.world()
                .resource::<PendingUsdPreviewTextReads>()
                .tasks
                .len(),
            1,
            "rapid document edits remain behind one text read"
        );

        for _ in 0..100 {
            app.update();
            let session = app
                .world()
                .resource::<UsdViewportState>()
                .session(preview)
                .unwrap();
            if session.text_ready()
                && session
                    .text
                    .authored
                    .as_deref()
                    .is_some_and(|text| text.contains("Latest"))
            {
                break;
            }
            std::thread::yield_now();
        }
        let session = app
            .world()
            .resource::<UsdViewportState>()
            .session(preview)
            .unwrap();
        assert!(session.text_ready());
        assert!(session.text.authored.as_deref().unwrap().contains("Latest"));
        assert!(session.text.composed.as_deref().unwrap().contains("Latest"));
        assert!(!session.text.authored.as_deref().unwrap().contains("Second"));
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
            UsdSceneProjected,
        ));
        app.world_mut().spawn((
            UsdPrimPath {
                stage_handle: stage.clone(),
                path: "/World/Rover".into(),
            },
            UsdSceneProjected,
            ChildOf(preview_root),
        ));

        // The active Twin intentionally shares the deduplicated stage handle
        // with its editor preview. Its entities must not participate in the
        // preview's readiness fence.
        let live_root = app
            .world_mut()
            .spawn((
                lunco_usd_bevy_scene::UsdSceneRoot,
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/World".into(),
                },
                UsdSceneProjected,
            ))
            .id();
        app.world_mut().spawn((
            UsdPrimPath {
                stage_handle: stage,
                path: "/World/Rover".into(),
            },
            UsdSceneProjected,
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
    fn document_preview_ids_follow_document_instance_identity() {
        assert_eq!(
            UsdPreviewId::for_document(DocumentId::new(1)),
            UsdPreviewId(1)
        );
        assert_ne!(
            UsdPreviewId::for_document(DocumentId::new(1)),
            UsdPreviewId::for_document(DocumentId::new(2))
        );
    }

    #[test]
    fn preview_light_uses_graphics_distant_light_default() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>();
        let mut settings = RenderingQualitySettings::default();
        settings.apply_profile(preview_quality());
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
        let (view, _) = create_preview_view(
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
    fn preview_budget_bounds_dimensions_and_pixels() {
        let budget = UsdPreviewRenderBudget::default();
        let target = bounded_view_size(UVec2::new(8192, 4096), &budget)
            .expect("default presentation budget is valid");
        assert!(target.x <= budget.max_view_dimension);
        assert!(target.y <= budget.max_view_dimension);
        assert!(u64::from(target.x) * u64::from(target.y) <= budget.max_view_pixels);
        assert!(
            bounded_view_size(
                UVec2::new(800, 600),
                &UsdPreviewRenderBudget {
                    max_view_dimension: 0,
                    max_view_pixels: 1,
                    max_total_pixels: 1,
                },
            )
            .is_none()
        );
    }

    #[test]
    fn preview_pointer_buttons_match_view_navigation_contract() {
        use lunco_usd_viewport_core::preview_drag_channels;

        assert_eq!(
            preview_drag_channels(true, false, false, false, false),
            (false, true)
        );
        assert_eq!(
            preview_drag_channels(false, true, false, false, false),
            (false, true)
        );
        assert_eq!(
            preview_drag_channels(false, false, true, false, false),
            (true, false)
        );
        assert_eq!(
            preview_drag_channels(false, false, true, true, false),
            (false, true)
        );
        assert_eq!(
            preview_drag_channels(true, false, false, true, false),
            (false, true)
        );
        assert_eq!(
            preview_drag_channels(true, false, false, false, true),
            (false, false),
            "a primary drag captured by a gizmo must not pan the preview"
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
        let profile = preview_quality();
        let (first_view, first_render_target) = create_preview_view(
            app.world_mut(),
            UsdPreviewId(1),
            UsdPreviewViewId(1),
            layer,
            profile,
        )
        .expect("first view resources are available");
        let (second_view, second_render_target) = create_preview_view(
            app.world_mut(),
            UsdPreviewId(1),
            UsdPreviewViewId(2),
            layer,
            profile,
        )
        .expect("second view resources are available");
        let first_camera = first_view.camera();
        let second_camera = second_view.camera();
        let first_image = first_render_target.image.clone();
        let second_image = second_render_target.image.clone();
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
