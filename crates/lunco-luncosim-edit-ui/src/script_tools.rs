//! Script-authored CLICK TOOLS — the editor half of the `lunco_tools` registry.
//!
//! A tool library that exposes `on_click(context)` becomes an armable tool in
//! the Tools palette. Arm it, click in the scene, and the tool's own Rhai
//! handler receives the canonical click context, including whether the click
//! used the primary, secondary, or middle button. Nothing else is required:
//! there is no registration call, no palette edit, no Rust per tool. Drop
//! `assets/scripting/tools/<name>.rhai` with an `on_click` in it and the button
//! is there next launch.
//!
//! ```rhai
//! // assets/scripting/tools/recover.rhai
//! fn ui_label() { "Recover" }
//! fn ui_hint()  { "Click a stuck vessel to right it" }
//! fn on_click(context) { vessel(context.target_entity_id); }
//! ```
//!
//! WHY DISCOVERY BY SIGNATURE. `Tool::functions()` already reports `name/arity`
//! for every registered tool (for rhai tools it comes from parsing the source),
//! so "can this be clicked?" is answerable from what the tool actually
//! implements. A separate list of palette entries could disagree with the code —
//! a button with no handler, or a handler nobody can reach. This cannot.
//!
//! The click is handed over as a typed tool-hook command. It is queued and run
//! by `drain_world_scripts` with the prelude and every tool in scope, so an
//! authored interaction policy can do anything a scenario can.

use bevy::math::DVec3;
use bevy::picking::hover::PreviousHoverMap;
use bevy::picking::pointer::{PointerButton, PointerId};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use big_space::prelude::{CellCoord, Grid};
use lunco_api::queries::{ApiQueryProvider, SimulationQueryReadScope, api_param_u64};
use lunco_api::{ApiQueryError, ApiQueryResult};
use lunco_api_core::{ApiErrorCode, ApiQueryParameterSchema, ApiQuerySchema, ApiValue, api_value};
use lunco_command_contracts::{Ack, OpId};
use lunco_control_core::ControlLink;
use lunco_core::{Command, on_command, register_commands};
use lunco_doc::DocumentId;
use lunco_doc_bevy::DocumentRegistry;
use lunco_embodiment_core::roles::TheLocalEmbodiment;
use lunco_input_core::InputBindingsSettings;
use lunco_scene_selection::SelectedEntities;
use lunco_spatial::coords::{
    ACTIVE_FRAME_NAME, ActiveFrameCoordinates, GridPos, RENDER_FRAME_NAME, RenderPos,
};
use lunco_spatial::world::ActivePhysicsFrame;
use lunco_telemetry_core::{TelemetryEvent, TelemetryValue};
use lunco_terrain_surface::{GridSurfaceQuery, TerrainSurfaceSnapshot};
use lunco_usd_bevy_mesh::{TransientUsdCurveView, UsdCurveMesh};
use lunco_usd_document::document::{LayerId, UsdDocument};
use lunco_usd_geometry::ribbon::{RibbonPoint, build_ribbon_mesh};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Build the language-neutral map passed to a script tool. The map is an
/// interaction contract, not an API serialization format; the scripting
/// backend converts it directly to the target runtime's native value.
pub(crate) fn tool_map(entries: Vec<(String, TelemetryValue)>) -> TelemetryValue {
    TelemetryValue::Map(entries.into_iter().collect())
}

/// Keep one dispatch key for the duration of the frame so the Rhai interaction
/// policy sees one button-resolved, depth-ordered scene hit.
#[derive(Resource, Default)]
pub struct ScenePointerDispatch {
    seen: HashSet<ScenePointerKey>,
    seen_moves: HashSet<ScenePointerMoveKey>,
    pending_moves: Vec<(PointerId, ScenePointerMoveSample)>,
    move_hooks: HashMap<DocumentId, ScenePointerMoveHook>,
}

#[derive(Clone)]
struct ScenePointerMoveHook {
    interaction_id: String,
    tool: String,
    hook: String,
    context: BTreeMap<String, String>,
    owner_twin_id: Option<u64>,
}

const MAX_PENDING_CURVE_VIEW_REQUESTS: usize = 64;
const MAX_ACTIVE_CURVE_VIEW_BUILDS: usize = 2;

#[derive(Resource, Default)]
pub(crate) struct PendingUsdCurveViews {
    next_revision: u64,
    requests: HashMap<Entity, UsdCurveViewRequest>,
    tasks: HashMap<Entity, Task<UsdCurveViewBuild>>,
    latest_revision: HashMap<Entity, u64>,
    status: HashMap<Entity, UsdCurveViewStatus>,
}

#[derive(Clone, Debug)]
struct UsdCurveViewStatus {
    requested_revision: u64,
    completed_revision: u64,
    applied_revision: u64,
    vertex_count: usize,
    error: Option<String>,
}

struct UsdCurveViewRequest {
    revision: u64,
    root: Entity,
    curve: Entity,
    parent: Entity,
    stage_id: bevy::asset::AssetId<lunco_usd_bevy_stage::UsdStageAsset>,
    points: Vec<[f64; 3]>,
    width_m: f64,
    clearance_m: f64,
    sample_spacing_m: f64,
    max_samples: usize,
}

struct UsdCurveViewBuild {
    revision: u64,
    root: Entity,
    curve: Entity,
    stage_id: bevy::asset::AssetId<lunco_usd_bevy_stage::UsdStageAsset>,
    result: Result<Option<(Mesh, DVec3)>, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ScenePointerKey {
    pointer: PointerId,
    button: PointerButton,
    click_count: u8,
    screen_position: [u32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ScenePointerMoveKey {
    pointer: PointerId,
    screen_position: [u32; 2],
}

#[derive(Clone, Copy, Debug)]
struct ScenePointerMoveSample {
    entity: Entity,
    position: Vec2,
    hit_position: Option<Vec3>,
    surface_hit: Option<lunco_terrain_surface::SurfaceHit>,
}

pub fn clear_scene_pointer_dispatch(mut dispatch: ResMut<ScenePointerDispatch>) {
    dispatch.seen.clear();
    dispatch.seen_moves.clear();
    dispatch.pending_moves.clear();
}

/// Set the presentation transform of a prim authored in the disposable USD
/// view layer. The target is resolved by stable entity identity and validated
/// against its owning document before its live Bevy transform is updated.
#[Command(default)]
pub struct SetUsdViewPreviewTransform {
    /// USD document which owns the view-layer prim.
    pub doc_id: u64,
    /// Stable API identity of the projected preview prim.
    pub entity_id: u64,
    /// Target translation in the active physics frame.
    pub translation: [f64; 3],
}

/// Subscribe one document to a typed Rhai pointer-move hook while an
/// interaction is active. Only one interaction owns movement updates per
/// document, and the scene-pointer adapter drops passive samples before scene
/// or terrain resolution when no subscription matches.
#[Command(default)]
pub struct SetScenePointerMoveHook {
    /// USD document whose pointer movement should reach the hook.
    pub doc_id: u64,
    /// Caller-owned identity used to prevent stale cleanup from ending a newer interaction.
    pub interaction_id: String,
    /// Registered Rhai tool namespace.
    pub tool: String,
    /// One-argument pointer-move hook name.
    pub hook: String,
    /// Stable context merged with each live pointer sample.
    pub context: BTreeMap<String, String>,
}

/// End the active scene-pointer movement subscription for one document.
#[Command(default)]
pub struct ClearScenePointerMoveHook {
    /// USD document whose pointer movement should stop reaching a hook.
    pub doc_id: u64,
    /// Identity supplied when the active interaction subscribed.
    pub interaction_id: String,
}

#[on_command(SetScenePointerMoveHook)]
fn on_set_scene_pointer_move_hook(
    trigger: On<SetScenePointerMoveHook>,
    mut dispatch: ResMut<ScenePointerDispatch>,
    documents: Res<DocumentRegistry<UsdDocument>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
) -> Result<Ack, String> {
    let command = trigger.event();
    let doc = DocumentId::new(command.doc_id);
    if doc.is_unassigned() {
        return Err("pointer-move hook requires an assigned USD document".to_string());
    }
    if documents.host(doc).is_none() {
        return Err(format!("USD document {doc} is not open"));
    }
    if command.interaction_id.is_empty()
        || command.tool.is_empty()
        || command.hook.is_empty()
        || !command
            .hook
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err("pointer-move hook requires a registered tool and hook identifier".to_string());
    }
    let signature = format!("{}/1", command.hook);
    if !lunco_tools::has_function(&command.tool, &signature) {
        return Err(format!(
            "Rhai tool '{}' has no {} handler",
            command.tool, signature
        ));
    }
    if dispatch.move_hooks.get(&doc).is_some_and(|active| {
        active.interaction_id != command.interaction_id
            || active.tool != command.tool
            || active.hook != command.hook
    }) {
        return Err(format!(
            "USD document {doc} already has an active pointer-move interaction"
        ));
    }
    let owner_twin_id = workspace.as_deref().and_then(|workspace| {
        workspace
            .0
            .document(doc)
            .and_then(|entry| workspace.0.twin_for(entry))
            .map(|twin| twin.raw())
    });
    dispatch.move_hooks.insert(
        doc,
        ScenePointerMoveHook {
            interaction_id: command.interaction_id.clone(),
            tool: command.tool.clone(),
            hook: command.hook.clone(),
            context: command.context.clone(),
            owner_twin_id,
        },
    );
    Ok(Ack::with_data(
        OpId::new(),
        api_value!({ "status": "registered" }),
    ))
}

#[on_command(ClearScenePointerMoveHook)]
fn on_clear_scene_pointer_move_hook(
    trigger: On<ClearScenePointerMoveHook>,
    mut dispatch: ResMut<ScenePointerDispatch>,
) -> Result<Ack, String> {
    let doc = DocumentId::new(trigger.event().doc_id);
    if doc.is_unassigned() {
        return Err("pointer-move hook requires an assigned USD document".to_string());
    }
    let command = trigger.event();
    if command.interaction_id.is_empty() {
        return Err("pointer-move hook cleanup requires its interaction identity".to_string());
    }
    if dispatch
        .move_hooks
        .get(&doc)
        .is_some_and(|active| active.interaction_id != command.interaction_id)
    {
        return Err(format!(
            "USD document {doc} has a different active pointer-move interaction"
        ));
    }
    dispatch.move_hooks.remove(&doc);
    Ok(Ack::with_data(
        OpId::new(),
        api_value!({ "status": "cleared" }),
    ))
}

pub(crate) fn clear_scene_pointer_move_hook_on_document_closed(
    trigger: On<lunco_doc_bevy::DocumentClosed>,
    mut dispatch: ResMut<ScenePointerDispatch>,
) {
    dispatch.move_hooks.remove(&trigger.event().doc);
}

pub(crate) fn clear_scene_pointer_move_hooks_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut dispatch: ResMut<ScenePointerDispatch>,
) {
    let Some(workspace) = workspace else {
        return;
    };
    let event = trigger.event();
    let closed_documents: HashSet<_> = workspace
        .0
        .documents()
        .iter()
        .filter(|entry| {
            lunco_workspace::document_belongs_to_twin_root(entry, event.twin, &event.root)
        })
        .map(|entry| entry.id)
        .collect();
    dispatch
        .move_hooks
        .retain(|doc, _| !closed_documents.contains(doc));
}

pub(crate) fn reset_scene_pointer_move_hooks(mut dispatch: ResMut<ScenePointerDispatch>) {
    dispatch.move_hooks.clear();
    dispatch.pending_moves.clear();
}

#[on_command(SetUsdViewPreviewTransform)]
fn on_set_usd_view_preview_transform(
    trigger: On<SetUsdViewPreviewTransform>,
    documents: Res<DocumentRegistry<UsdDocument>>,
    backed: Res<lunco_usd_bevy_twin::DocBackedTwinScenes>,
    asset_server: Res<AssetServer>,
    entities: Res<lunco_api::registry::ApiEntityRegistry>,
    active_frame: Option<Res<ActivePhysicsFrame>>,
    q_prim: Query<&lunco_usd_bevy_scene::UsdPrimPath>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    mut spatial: ParamSet<(
        Query<(Option<&CellCoord>, &Transform)>,
        Query<&mut Transform>,
    )>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let command = trigger.event();
    let doc = DocumentId::new(command.doc_id);
    if doc.is_unassigned() {
        return Err("preview transform requires an assigned USD document".to_string());
    }
    let entity_id = lunco_core::GlobalEntityId::from_raw(command.entity_id);
    let entity = entities
        .resolve(&entity_id)
        .ok_or_else(|| "preview transform target is not live".to_string())?;
    let prim = q_prim
        .get(entity)
        .map_err(|_| "preview transform target is not a USD prim".to_string())?;
    let target_doc =
        lunco_usd_bevy_twin::scene_document_for(&backed, &asset_server, prim.stage_handle.id())
            .ok_or_else(|| "preview transform target has no document-backed scene".to_string())?;
    if target_doc != doc {
        return Err("preview transform target belongs to another USD document".to_string());
    }
    let host = documents
        .host(doc)
        .ok_or_else(|| format!("USD document {doc} is not open"))?;
    if !host
        .document()
        .authored_prim_exists(&LayerId::view(), &prim.path)
        .map_err(|error| format!("cannot validate USD view prim {}: {error}", prim.path))?
    {
        return Err(format!(
            "preview transform target {} is not authored in the USD view layer",
            prim.path
        ));
    }
    let frame = active_frame
        .as_deref()
        .map(|frame| frame.0)
        .ok_or_else(|| "preview transform has no active physics frame".to_string())?;
    let position = DVec3::from_array(command.translation);
    if !position.is_finite() {
        return Err("preview transform position must be finite".to_string());
    }

    // Preview geometry is a render-only entity. Convert from the stable active
    // frame through its real parent hierarchy, using BigSpace's canonical cell
    // split before narrowing into Bevy's local render transform.
    let (old_cell, new_cell, local_translation) = {
        let spatial = spatial.p0();
        let (old_cell, _) = spatial
            .get(entity)
            .map_err(|_| "preview transform target has no live transform".to_string())?;
        let (new_cell, local_translation) =
            lunco_spatial::coords::position_in_grid_to_parent_local(
                entity, position, frame, &q_parents, &q_grids, &spatial,
            )
            .ok_or_else(|| {
                "preview transform target is disconnected from the active frame".to_string()
            })?;
        (old_cell.copied(), new_cell, local_translation)
    };
    {
        let mut transforms = spatial.p1();
        let mut transform = transforms
            .get_mut(entity)
            .map_err(|_| "preview transform target has no mutable transform".to_string())?;
        if transform.translation != local_translation {
            transform.translation = local_translation;
        }
    }
    match (new_cell, old_cell) {
        (Some(cell), previous) if previous != Some(cell) => {
            commands.entity(entity).try_insert(cell);
        }
        (None, Some(_)) => {
            commands.entity(entity).try_remove::<CellCoord>();
        }
        _ => {}
    }
    Ok(Ack::new(OpId::new()))
}

/// Queue a live render update for an existing USD curve entity. The request is
/// coalesced by target, terrain sampling and ribbon meshing run from immutable
/// snapshots on the compute pool, and the current result replaces only the
/// Bevy mesh. No USD layer or scene projection is changed.
#[Command(default)]
pub struct UpdateUsdCurveView {
    /// USD document which owns both target prims.
    pub doc_id: u64,
    /// Stable API identity of the Xform carrying the curve's anchor.
    pub root_entity_id: u64,
    /// Stable API identity of the projected UsdGeomBasisCurves entity.
    pub curve_entity_id: u64,
    /// Ordered centerline points in the route parent's local USD coordinates.
    pub points: Vec<[f64; 3]>,
    /// Full ribbon width in metres.
    pub width_m: f64,
    /// Separation from the sampled surface in metres.
    pub clearance_m: f64,
    /// Maximum horizontal distance between terrain samples in metres.
    pub sample_spacing_m: f64,
    /// Maximum sample count before spacing is increased to fit the request.
    pub max_samples: u64,
}

/// Read the latest requested USD curve-view result.
///
/// The result is presentation state: it does not read or mutate a USD layer.
/// Callers use the revision fields to distinguish a committed mesh or hidden
/// no-geometry state from an older mesh still displayed during preparation.
pub struct InspectUsdCurveViewProvider;

impl ApiQueryProvider for InspectUsdCurveViewProvider {
    fn name(&self) -> &'static str {
        "InspectUsdCurveView"
    }

    fn schema(&self) -> ApiQuerySchema {
        ApiQuerySchema {
            name: self.name().to_owned(),
            description: Some(
                "Inspect the latest Bevy presentation result for one document-backed USD curve view."
                    .to_owned(),
            ),
            parameters: Some(vec![
                ApiQueryParameterSchema {
                    name: "doc_id".to_owned(),
                    type_name: "u64".to_owned(),
                    required: true,
                    description: "Owning USD document identity.".to_owned(),
                    allowed_values: None,
                },
                ApiQueryParameterSchema {
                    name: "entity_id".to_owned(),
                    type_name: "u64".to_owned(),
                    required: true,
                    description: "Stable API identity of the projected curve prim.".to_owned(),
                    allowed_values: None,
                },
            ]),
            exactly_one_of: Vec::new(),
            response: Some(
                "{ doc_id, entity_id, path, state, requested_revision, completed_revision, applied_revision, local_visibility, result_vertex_count, error }"
                    .to_owned(),
            ),
        }
    }

    fn simulation_read_scope(&self, _params: &ApiValue) -> SimulationQueryReadScope {
        SimulationQueryReadScope::ScenarioDeclared
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(raw_doc) = api_param_u64(params, "doc_id") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "InspectUsdCurveView requires a numeric `doc_id`",
            ));
        };
        let Some(raw_entity) = api_param_u64(params, "entity_id") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "InspectUsdCurveView requires a numeric `entity_id`",
            ));
        };
        let entity_id = lunco_core::GlobalEntityId::from_raw(raw_entity);
        let Some(entity) = world
            .get_resource::<lunco_api::registry::ApiEntityRegistry>()
            .and_then(|registry| registry.resolve(&entity_id))
        else {
            return Err(ApiQueryError::new(
                ApiErrorCode::EntityNotFound,
                format!("InspectUsdCurveView entity {raw_entity} is not live"),
            ));
        };
        let Some(prim) = world.get::<lunco_usd_bevy_scene::UsdPrimPath>(entity) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "InspectUsdCurveView target is not a USD prim",
            ));
        };
        if world.get::<UsdCurveMesh>(entity).is_none() {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "InspectUsdCurveView target is not a projected USD curve mesh",
            ));
        }
        let Some(backed) = world.get_resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "InspectUsdCurveView document ownership is unavailable",
            ));
        };
        let Some(asset_server) = world.get_resource::<AssetServer>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "InspectUsdCurveView asset server is unavailable",
            ));
        };
        let target_doc =
            lunco_usd_bevy_twin::scene_document_for(backed, asset_server, prim.stage_handle.id())
                .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::EntityNotFound,
                    "InspectUsdCurveView target has no document-backed scene",
                )
            })?;
        if target_doc.raw() != raw_doc {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "InspectUsdCurveView target belongs to another USD document",
            ));
        }
        let status = world
            .get_resource::<PendingUsdCurveViews>()
            .and_then(|views| views.status.get(&entity))
            .ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::EntityNotFound,
                    "InspectUsdCurveView has no build result for this curve",
                )
            })?;
        let state = if status.completed_revision != status.requested_revision {
            "pending"
        } else if status.error.is_some() {
            "failed"
        } else {
            "ready"
        };
        let visibility = match world.get::<Visibility>(entity) {
            Some(Visibility::Hidden) => "hidden",
            Some(Visibility::Visible) => "visible",
            Some(Visibility::Inherited) => "inherited",
            None => {
                return Err(ApiQueryError::new(
                    ApiErrorCode::InternalError,
                    "InspectUsdCurveView target has no visibility state",
                ));
            }
        };
        Ok(Some(api_value!({
            "doc_id": raw_doc,
            "entity_id": raw_entity,
            "path": prim.path.clone(),
            "state": state,
            "requested_revision": status.requested_revision,
            "completed_revision": status.completed_revision,
            "applied_revision": status.applied_revision,
            "local_visibility": visibility,
            "result_vertex_count": status.vertex_count as u64,
            "error": status.error.clone().unwrap_or_default(),
        })))
    }
}

#[on_command(UpdateUsdCurveView)]
fn on_update_usd_curve_view(
    trigger: On<UpdateUsdCurveView>,
    backed: Res<lunco_usd_bevy_twin::DocBackedTwinScenes>,
    asset_server: Res<AssetServer>,
    entities: Res<lunco_api::registry::ApiEntityRegistry>,
    q_root: Query<&lunco_usd_bevy_scene::UsdPrimPath>,
    q_parents: Query<&ChildOf>,
    mut q_curve: Query<
        (&lunco_usd_bevy_scene::UsdPrimPath, &Mesh3d, &mut Visibility),
        With<UsdCurveMesh>,
    >,
    mut pending: ResMut<PendingUsdCurveViews>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let command = trigger.event();
    let doc = DocumentId::new(command.doc_id);
    if doc.is_unassigned() {
        return Err("curve view update requires an assigned USD document".to_string());
    }
    if command.points.len() > 4096 {
        return Err("curve view update accepts at most 4096 centerline points".to_string());
    }
    if !command.width_m.is_finite() || command.width_m <= 0.0 {
        return Err("curve view width must be finite and positive".to_string());
    }
    if !command.clearance_m.is_finite() || command.clearance_m < 0.0 {
        return Err("curve view clearance must be finite and non-negative".to_string());
    }
    if !command.sample_spacing_m.is_finite() || command.sample_spacing_m <= 0.0 {
        return Err("curve view sample spacing must be finite and positive".to_string());
    }
    if !(2..=4096).contains(&command.max_samples) {
        return Err("curve view max_samples must be between 2 and 4096".to_string());
    }
    if command
        .points
        .iter()
        .flatten()
        .any(|coordinate| !coordinate.is_finite())
    {
        return Err("curve view centerline contains a non-finite coordinate".to_string());
    }

    let root = entities
        .resolve(&lunco_core::GlobalEntityId::from_raw(
            command.root_entity_id,
        ))
        .ok_or_else(|| "curve view anchor is not live".to_string())?;
    let curve = entities
        .resolve(&lunco_core::GlobalEntityId::from_raw(
            command.curve_entity_id,
        ))
        .ok_or_else(|| "curve view geometry is not live".to_string())?;
    let root_prim = q_root
        .get(root)
        .map_err(|_| "curve view anchor is not a USD prim".to_string())?;
    let (curve_prim, _, mut visibility) = q_curve
        .get_mut(curve)
        .map_err(|_| "curve view geometry is not a projected USD curve".to_string())?;
    if root_prim.stage_handle.id() != curve_prim.stage_handle.id()
        || !curve_prim.path.starts_with(&(root_prim.path.clone() + "/"))
    {
        return Err("curve view geometry is not a descendant of its anchor".to_string());
    }
    let target_doc = lunco_usd_bevy_twin::scene_document_for(
        &backed,
        &asset_server,
        root_prim.stage_handle.id(),
    )
    .ok_or_else(|| "curve view target has no document-backed scene".to_string())?;
    if target_doc != doc {
        return Err("curve view targets belong to another USD document".to_string());
    }
    let parent = q_parents
        .get(root)
        .map_err(|_| "curve view anchor has no USD route parent".to_string())?
        .parent();
    let expected_parent_path = root_prim
        .path
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .ok_or_else(|| "curve view anchor path has no parent".to_string())?;
    let parent_prim = q_root
        .get(parent)
        .map_err(|_| "curve view route parent is not a USD prim".to_string())?;
    if parent_prim.stage_handle.id() != root_prim.stage_handle.id()
        || parent_prim.path != expected_parent_path
    {
        return Err("curve view anchor is not directly parented to its route prim".to_string());
    }

    if command.points.len() >= 2
        && pending.requests.len() + pending.tasks.len() >= MAX_PENDING_CURVE_VIEW_REQUESTS
        && !pending.requests.contains_key(&curve)
        && !pending.tasks.contains_key(&curve)
    {
        return Err("curve view preparation queue is full".to_string());
    }
    let revision = pending
        .next_revision
        .checked_add(1)
        .ok_or_else(|| "curve view operation revision is exhausted".to_string())?;
    pending.next_revision = revision;
    pending.latest_revision.insert(curve, revision);
    let applied_revision = pending
        .status
        .get(&curve)
        .map_or(0, |status| status.applied_revision);
    let completed_revision = pending
        .status
        .get(&curve)
        .map_or(0, |status| status.completed_revision);
    let vertex_count = pending
        .status
        .get(&curve)
        .map_or(0, |status| status.vertex_count);
    pending.status.insert(
        curve,
        UsdCurveViewStatus {
            requested_revision: revision,
            completed_revision,
            applied_revision,
            vertex_count,
            error: None,
        },
    );
    if command.points.len() < 2 {
        *visibility = Visibility::Hidden;
        pending.requests.remove(&curve);
        pending.latest_revision.remove(&curve);
        pending.status.insert(
            curve,
            UsdCurveViewStatus {
                requested_revision: revision,
                completed_revision: revision,
                applied_revision: revision,
                vertex_count: 0,
                error: None,
            },
        );
        commands.entity(curve).try_insert(TransientUsdCurveView);
        return Ok(Ack::new(OpId::new()));
    }
    pending.requests.insert(
        curve,
        UsdCurveViewRequest {
            revision,
            root,
            curve,
            parent,
            stage_id: root_prim.stage_handle.id(),
            points: command.points.clone(),
            width_m: command.width_m,
            clearance_m: command.clearance_m,
            sample_spacing_m: command.sample_spacing_m,
            max_samples: command.max_samples as usize,
        },
    );
    // Once the route tool takes ownership, subsequent authored stage changes
    // must not rebuild this mesh from its intentionally minimal USD seed.
    commands.entity(curve).try_insert(TransientUsdCurveView);
    Ok(Ack::new(OpId::new()))
}

/// Start coalesced route-curve builds from the latest immutable request. Only
/// two builds run at once; repeated edits to one curve replace its pending
/// request and stale worker completions are discarded by revision.
pub(crate) fn prepare_pending_usd_curve_views(
    mut pending: ResMut<PendingUsdCurveViews>,
    surface: GridSurfaceQuery,
    active_frame: Option<Res<ActivePhysicsFrame>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_root: Query<&lunco_usd_bevy_scene::UsdPrimPath>,
    mut q_curve: Query<
        (&lunco_usd_bevy_scene::UsdPrimPath, &Mesh3d, &mut Visibility),
        With<UsdCurveMesh>,
    >,
) {
    if pending.tasks.len() >= MAX_ACTIVE_CURVE_VIEW_BUILDS || pending.requests.is_empty() {
        return;
    }
    let has_sampled_route = pending
        .requests
        .values()
        .any(|request| request.points.len() >= 2);
    let terrain = if surface.has_terrain() && has_sampled_route {
        surface.snapshot()
    } else {
        None
    };
    if surface.has_terrain() && has_sampled_route && terrain.is_none() {
        warn!("[usd-curve-view] terrain exists without a committed active-frame snapshot");
    }
    let frame = active_frame.as_deref().map(|frame| frame.0);
    let mut entities: Vec<_> = pending.requests.keys().copied().collect();
    entities.sort_unstable_by_key(|entity| entity.to_bits());
    for entity in entities {
        if pending.tasks.len() >= MAX_ACTIVE_CURVE_VIEW_BUILDS {
            break;
        }
        // Keep each target's latest request queued behind its active build.
        // Dropping a Bevy task detaches its worker; replacing it here would
        // make rapid pointer updates exceed the explicit concurrency bound.
        if pending.tasks.contains_key(&entity) {
            continue;
        }
        let Some(queued) = pending.requests.get(&entity) else {
            continue;
        };
        let needs_surface = queued.points.len() >= 2 && surface.has_terrain();
        if needs_surface && terrain.is_none() {
            continue;
        }
        let Some(request) = pending.requests.remove(&entity) else {
            continue;
        };
        let Ok((curve, _, mut visibility)) = q_curve.get_mut(entity) else {
            pending.latest_revision.remove(&entity);
            pending.status.remove(&entity);
            continue;
        };
        let Ok(root) = q_root.get(request.root) else {
            pending.latest_revision.remove(&entity);
            *visibility = Visibility::Hidden;
            complete_curve_view_status(
                &mut pending,
                entity,
                request.revision,
                0,
                Some("curve view anchor is no longer live".to_owned()),
            );
            warn!("[usd-curve-view] queued anchor is no longer live");
            continue;
        };
        if curve.stage_handle.id() != request.stage_id
            || root.stage_handle.id() != request.stage_id
            || request.curve != entity
        {
            pending.latest_revision.remove(&entity);
            *visibility = Visibility::Hidden;
            complete_curve_view_status(
                &mut pending,
                entity,
                request.revision,
                0,
                Some("queued curve view target changed before preparation".to_owned()),
            );
            warn!("[usd-curve-view] queued target changed before mesh preparation");
            continue;
        }
        let Some(frame) = frame else {
            pending.requests.insert(entity, request);
            warn!("[usd-curve-view] no active physics frame is available for route geometry");
            continue;
        };
        let Some((parent_position, parent_rotation)) = lunco_spatial::coords::pose_in_grid(
            request.parent,
            frame,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            pending.requests.insert(entity, request);
            warn!("[usd-curve-view] route parent is disconnected from the active physics frame");
            continue;
        };
        let terrain = terrain.clone();
        let task_request = request;
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let result = build_usd_curve_view_mesh(
                &task_request,
                terrain.as_ref(),
                parent_position,
                parent_rotation,
            );
            UsdCurveViewBuild {
                revision: task_request.revision,
                root: task_request.root,
                curve: task_request.curve,
                stage_id: task_request.stage_id,
                result,
            }
        });
        pending.tasks.insert(entity, task);
    }
}

/// Commit only the newest current result to the existing renderer entity. This
/// presentation update does not touch USD documents, stage revisions, or the
/// structural scene projector.
pub(crate) fn poll_pending_usd_curve_views(
    mut pending: ResMut<PendingUsdCurveViews>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut q_root: Query<(&lunco_usd_bevy_scene::UsdPrimPath, &mut Transform)>,
    mut q_curve: Query<
        (&lunco_usd_bevy_scene::UsdPrimPath, &Mesh3d, &mut Visibility),
        With<UsdCurveMesh>,
    >,
) {
    let mut completed = Vec::new();
    for (&entity, task) in &mut pending.tasks {
        if let Some(result) = future::block_on(future::poll_once(task)) {
            completed.push((entity, result));
        }
    }
    for (entity, result) in completed {
        pending.tasks.remove(&entity);
        if pending.latest_revision.get(&entity) != Some(&result.revision) {
            continue;
        }
        let Ok((curve_prim, mesh_handle, mut visibility)) = q_curve.get_mut(result.curve) else {
            pending.latest_revision.remove(&entity);
            continue;
        };
        let Ok((root_prim, mut root_transform)) = q_root.get_mut(result.root) else {
            pending.latest_revision.remove(&entity);
            *visibility = Visibility::Hidden;
            complete_curve_view_status(
                &mut pending,
                entity,
                result.revision,
                0,
                Some("curve view anchor is no longer live".to_owned()),
            );
            warn!("[usd-curve-view] completed anchor is no longer live");
            continue;
        };
        if curve_prim.stage_handle.id() != result.stage_id
            || root_prim.stage_handle.id() != result.stage_id
        {
            pending.latest_revision.remove(&entity);
            *visibility = Visibility::Hidden;
            complete_curve_view_status(
                &mut pending,
                entity,
                result.revision,
                0,
                Some("completed target belongs to a replaced stage".to_owned()),
            );
            warn!("[usd-curve-view] completed target belongs to a replaced stage");
            continue;
        }
        let revision = result.revision;
        let build_result = match result.result {
            Ok(result) => result,
            Err(error) => {
                pending.latest_revision.remove(&entity);
                *visibility = Visibility::Hidden;
                complete_curve_view_status(&mut pending, entity, revision, 0, Some(error.clone()));
                warn!("[usd-curve-view] mesh preparation failed: {error}");
                continue;
            }
        };
        if let Some((mesh, anchor)) = build_result {
            let anchor = anchor.as_vec3();
            if !anchor.is_finite() {
                pending.latest_revision.remove(&entity);
                *visibility = Visibility::Hidden;
                complete_curve_view_status(
                    &mut pending,
                    entity,
                    revision,
                    0,
                    Some("computed anchor exceeds render-space range".to_owned()),
                );
                warn!("[usd-curve-view] computed anchor exceeds render-space range");
                continue;
            }
            let vertex_count = mesh.count_vertices();
            let Some(mut current_mesh) = meshes.get_mut(&mesh_handle.0) else {
                pending.latest_revision.remove(&entity);
                *visibility = Visibility::Hidden;
                complete_curve_view_status(
                    &mut pending,
                    entity,
                    revision,
                    0,
                    Some("projected curve mesh asset is unavailable".to_owned()),
                );
                warn!("[usd-curve-view] projected curve mesh asset is unavailable");
                continue;
            };
            *current_mesh = mesh;
            if root_transform.translation != anchor {
                root_transform.translation = anchor;
            }
            *visibility = Visibility::Inherited;
            complete_curve_view_status(&mut pending, entity, revision, vertex_count, None);
        } else {
            *visibility = Visibility::Hidden;
            complete_curve_view_status(&mut pending, entity, revision, 0, None);
        }
        if !pending.requests.contains_key(&entity) {
            pending.latest_revision.remove(&entity);
        }
    }
}

fn complete_curve_view_status(
    pending: &mut PendingUsdCurveViews,
    entity: Entity,
    revision: u64,
    vertex_count: usize,
    error: Option<String>,
) {
    let Some(status) = pending.status.get_mut(&entity) else {
        return;
    };
    if status.requested_revision != revision {
        return;
    }
    status.completed_revision = revision;
    status.vertex_count = vertex_count;
    status.error = error;
    if status.error.is_none() {
        status.applied_revision = revision;
    }
}

pub(crate) fn reset_pending_usd_curve_views(mut pending: ResMut<PendingUsdCurveViews>) {
    *pending = PendingUsdCurveViews::default();
}

fn build_usd_curve_view_mesh(
    request: &UsdCurveViewRequest,
    terrain: Option<&TerrainSurfaceSnapshot>,
    parent_position: DVec3,
    parent_rotation: bevy::math::DQuat,
) -> Result<Option<(Mesh, DVec3)>, String> {
    let authored_local_points: Vec<DVec3> = request
        .points
        .iter()
        .map(|point| DVec3::from_array(*point))
        .collect();
    if authored_local_points.len() < 2 {
        return Ok(None);
    }
    let authored_points: Vec<DVec3> = authored_local_points
        .iter()
        .map(|point| parent_position + parent_rotation * *point)
        .collect();
    let mut spacing = request.sample_spacing_m;
    let limit = request.max_samples.max(authored_points.len());
    let mut sample_count = route_ribbon_sample_count(&authored_points, spacing);
    let mut refinements = 0;
    while sample_count > limit {
        spacing *= 1.25;
        refinements += 1;
        if !spacing.is_finite() || refinements > 256 {
            return Err(
                "route ribbon sampling could not meet its bounded sample count".to_string(),
            );
        }
        sample_count = route_ribbon_sample_count(&authored_points, spacing);
    }
    let dense_points = route_ribbon_dense_points(&authored_points, spacing);
    if dense_points.len() < 2 {
        return Err("route ribbon needs at least two distinct centerline samples".to_string());
    }

    let first_surface =
        terrain.and_then(|terrain| terrain.sample_surface(GridPos(dense_points[0]), 1.0));
    let world_ribbon_points = if let Some(first_surface) = first_surface {
        let mut samples = Vec::with_capacity(dense_points.len());
        samples.push(RibbonPoint {
            position: first_surface.point.0,
            normal: first_surface.normal,
        });
        for point in dense_points.iter().skip(1) {
            if let Some(sample) =
                terrain.and_then(|terrain| terrain.sample_surface(GridPos(*point), 1.0))
            {
                samples.push(RibbonPoint {
                    position: sample.point.0,
                    normal: sample.normal,
                });
            } else {
                samples.push(RibbonPoint {
                    position: *point,
                    normal: DVec3::Y,
                });
            }
        }
        samples
    } else {
        authored_points
            .iter()
            .copied()
            .map(|position| RibbonPoint {
                position,
                normal: DVec3::Y,
            })
            .collect()
    };
    let anchor_world = world_ribbon_points[0].position;
    let inverse_parent_rotation = parent_rotation.inverse();
    let anchor = inverse_parent_rotation * (anchor_world - parent_position);
    let ribbon_points: Vec<RibbonPoint> = world_ribbon_points
        .into_iter()
        .map(|point| RibbonPoint {
            position: anchor + inverse_parent_rotation * (point.position - anchor_world),
            normal: inverse_parent_rotation * point.normal,
        })
        .collect();
    // Mesh uploads use f32 vertex buffers; authoring, terrain samples, and the
    // anchor remain f64 until this explicit renderer boundary.
    let half_width = (request.width_m * 0.5) as f32;
    let clearance = request.clearance_m as f32;
    if !half_width.is_finite() || !clearance.is_finite() {
        return Err("route ribbon dimensions exceed render-space range".to_string());
    }
    let mesh = build_ribbon_mesh(&ribbon_points, anchor, &[half_width], clearance, false)
        .ok_or_else(|| "route centerline cannot form a finite ribbon mesh".to_string())?;
    Ok(Some((mesh, anchor)))
}

fn route_ribbon_sample_count(points: &[DVec3], spacing: f64) -> usize {
    let mut count = 1usize;
    for pair in points.windows(2) {
        let distance = DVec3::new(pair[1].x - pair[0].x, 0.0, pair[1].z - pair[0].z).length();
        let steps = (distance / spacing).ceil().max(1.0) as usize;
        count = count.saturating_add(steps);
    }
    count
}

fn route_ribbon_dense_points(points: &[DVec3], spacing: f64) -> Vec<DVec3> {
    if points.len() == 1 {
        return points.to_vec();
    }
    let mut dense = Vec::with_capacity(route_ribbon_sample_count(points, spacing));
    for pair in points.windows(2) {
        let distance = DVec3::new(pair[1].x - pair[0].x, 0.0, pair[1].z - pair[0].z).length();
        let steps = (distance / spacing).ceil().max(1.0) as usize;
        for step in 0..steps {
            let t = step as f64 / steps as f64;
            dense.push(pair[0].lerp(pair[1], t));
        }
    }
    if let Some(last) = points.last() {
        dense.push(*last);
    }
    dense
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SceneToolWorld<'w, 's> {
    q_selectable: Query<'w, 's, Entity, With<lunco_core::SelectableRoot>>,
    q_mobility: Query<'w, 's, Entity, With<lunco_core::MobilityRoot>>,
    q_ids: Query<'w, 's, &'static lunco_core::GlobalEntityId>,
    q_prim: Query<'w, 's, &'static lunco_usd_bevy_scene::UsdPrimPath>,
    q_pointer_policy: Query<'w, 's, &'static lunco_interaction_core::ScenePointerPolicy>,
    q_scene_roots: Query<
        'w,
        's,
        &'static lunco_usd_bevy_scene::UsdPrimPath,
        With<lunco_usd_bevy_scene::UsdSceneRoot>,
    >,
    q_parents: Query<'w, 's, &'static ChildOf>,
    selected: Res<'w, SelectedEntities>,
    local_avatar: Res<'w, TheLocalEmbodiment>,
    q_links: Query<'w, 's, &'static ControlLink>,
    q_scene_cameras: Query<
        'w,
        's,
        (&'static Camera, &'static GlobalTransform),
        (With<Camera3d>, With<lunco_render::SceneCamera>),
    >,
    q_lod_tiles: Query<'w, 's, &'static lunco_terrain_surface::stream_viz::LodTileOf>,
    viewport: Res<'w, lunco_viewport_core::SceneViewport>,
    surface: lunco_terrain_surface::GridSurfaceQuery<'w, 's>,
    input_bindings: Res<'w, InputBindingsSettings>,
    scene_interaction: Res<'w, lunco_interaction_core::SceneInteractionMode>,
    backed: Res<'w, lunco_usd_bevy_twin::DocBackedTwinScenes>,
    asset_server: Res<'w, AssetServer>,
    coordinates: ActiveFrameCoordinates<'w, 's>,
}

/// Disarm the armed script tool on Cancel (Esc), like every other cursor mode.
///
/// Arming is done by the palette (a click writes the tool name); this only
/// handles the keyboard exit, so that every mode backs out on the same key —
/// which is the whole point of `CancelIntent` being a shared intent rather than
/// a `KeyCode::Escape` test per tool.
pub fn disarm_script_tool_on_cancel(
    mut armed: ResMut<lunco_interaction_core::ArmedScriptTool>,
    cancel: lunco_control_core::CancelIntent,
) {
    if armed.armed() && cancel.just_pressed() {
        armed.0 = None;
    }
}

/// Forget an armed tool that is no longer registered.
///
/// Tool libraries are hot-replaceable (`RegisterToolLibrary`, and the Twin scan
/// on open), so the armed name can outlive the tool it names. Without this the
/// palette would show nothing armed while clicks still went to a dead namespace
/// and failed one snippet at a time.
pub fn forget_missing_script_tool(mut armed: ResMut<lunco_interaction_core::ArmedScriptTool>) {
    let Some(name) = armed.0.clone() else { return };
    if !lunco_tools::has_function(&name, lunco_tools::UI_CLICK_FN) {
        warn!("[script-tool] '{name}' is no longer registered — disarming");
        armed.0 = None;
    }
}

/// Scene click while a script tool is armed: hand a generic, structured context
/// to the tool's `on_click`. Target resolution is semantic where possible, but
/// an empty-space/terrain click is still a valid context for tools that operate
/// on positions rather than entities.
pub(crate) fn on_scene_click_script_tool(
    mut click: On<Pointer<Click>>,
    armed: Res<lunco_interaction_core::ArmedScriptTool>,
    keys: Res<ButtonInput<KeyCode>>,
    egui_focus: Res<lunco_control_core::EguiFocus>,
    world: SceneToolWorld,
    mut commands: Commands,
) {
    let Some(tool) = armed.0.clone() else { return };
    // Shared egui-vs-scene guard, as used by selection and placement: a click on
    // panel chrome is not a click on the world.
    if egui_focus.wants_pointer {
        return;
    }
    // `Pointer<Click>` bubbles leaf→parent→…→window. We resolve the target
    // ourselves, so stop the bubble here (this runs target-first, i.e. at the
    // picked leaf) rather than firing the tool once per ancestor.
    click.propagate(false);

    let context = scene_tool_context(
        &click,
        &keys,
        &world.q_selectable,
        &world.q_mobility,
        &world.q_ids,
        &world.q_prim,
        &world.q_pointer_policy,
        &world.q_scene_roots,
        &world.q_parents,
        &world.selected,
        &world.local_avatar,
        &world.q_links,
        &world.backed,
        &world.asset_server,
        &world.coordinates,
        &world.input_bindings,
        &world.scene_interaction,
        &world.q_scene_cameras,
        &world.q_lod_tiles,
        &world.viewport,
        &world.surface,
    );
    commands.trigger(lunco_scripting_rhai_runtime::commands::RunRhaiTool {
        tool,
        args: context,
    });
}

/// Build the shared typed pointer context for every scene-tool gesture. The
/// editor resolves identity and document ownership; the Rhai handler owns the
/// meaning of the gesture.
#[allow(clippy::too_many_arguments)]
fn scene_tool_context(
    click: &Pointer<Click>,
    keys: &ButtonInput<KeyCode>,
    q_selectable: &Query<Entity, With<lunco_core::SelectableRoot>>,
    q_mobility: &Query<Entity, With<lunco_core::MobilityRoot>>,
    q_ids: &Query<&lunco_core::GlobalEntityId>,
    q_prim: &Query<&lunco_usd_bevy_scene::UsdPrimPath>,
    q_pointer_policy: &Query<&lunco_interaction_core::ScenePointerPolicy>,
    q_scene_roots: &Query<
        &lunco_usd_bevy_scene::UsdPrimPath,
        With<lunco_usd_bevy_scene::UsdSceneRoot>,
    >,
    q_parents: &Query<&ChildOf>,
    selected: &SelectedEntities,
    local_avatar: &TheLocalEmbodiment,
    q_links: &Query<&ControlLink>,
    backed: &lunco_usd_bevy_twin::DocBackedTwinScenes,
    asset_server: &AssetServer,
    coordinates: &ActiveFrameCoordinates<'_, '_>,
    input_bindings: &InputBindingsSettings,
    scene_interaction: &lunco_interaction_core::SceneInteractionMode,
    q_scene_cameras: &Query<
        (&Camera, &GlobalTransform),
        (With<Camera3d>, With<lunco_render::SceneCamera>),
    >,
    q_lod_tiles: &Query<&lunco_terrain_surface::stream_viz::LodTileOf>,
    viewport: &lunco_viewport_core::SceneViewport,
    surface: &lunco_terrain_surface::GridSurfaceQuery<'_, '_>,
) -> TelemetryValue {
    let root = crate::selection::find_selectable(click.entity, q_selectable, q_mobility, q_parents);

    let mut prim_paths = Vec::new();
    let mut inspector_part = None;
    let mut pointer_policy = None;
    let mut ancestor = Some(click.entity);
    for _ in 0..32 {
        let Some(entity) = ancestor else { break };
        if pointer_policy.is_none() {
            pointer_policy = q_pointer_policy.get(entity).ok().copied();
        }
        if let Ok(path) = q_prim.get(entity) {
            prim_paths.push(TelemetryValue::String(path.path.clone()));
            if root != entity && inspector_part.is_none() {
                inspector_part = Some(entity);
            }
        }
        ancestor = q_parents.get(entity).ok().map(|parent| parent.0);
    }

    let target_prim = q_prim
        .get(root)
        .ok()
        .or_else(|| q_prim.get(click.entity).ok());
    let selected_entity = selected.primary();
    let controlled_entity = local_avatar
        .0
        .and_then(|avatar| q_links.get(avatar).ok().map(|link| link.target));
    let context_prim = controlled_entity
        .and_then(|entity| q_prim.get(entity).ok())
        .or_else(|| selected_entity.and_then(|entity| q_prim.get(entity).ok()))
        .or(target_prim);

    let modifiers = tool_map(vec![
        (
            "alt".to_string(),
            TelemetryValue::Bool(keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight])),
        ),
        (
            "shift".to_string(),
            TelemetryValue::Bool(keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight])),
        ),
        (
            "ctrl".to_string(),
            TelemetryValue::Bool(keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight])),
        ),
    ]);
    let button = match click.button {
        PointerButton::Primary => "primary",
        PointerButton::Secondary => "secondary",
        PointerButton::Middle => "middle",
    };
    let pointer_intents = input_bindings.pointer_intents(
        button,
        keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]),
        keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]),
        keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]),
    );
    let mut context = vec![
        (
            "button".to_string(),
            TelemetryValue::String(button.to_string()),
        ),
        (
            "scene_interaction_mode".to_string(),
            TelemetryValue::String(scene_interaction.as_str().to_string()),
        ),
        (
            "pointer_intents".to_string(),
            TelemetryValue::Array(
                pointer_intents
                    .into_iter()
                    .map(TelemetryValue::String)
                    .collect(),
            ),
        ),
        (
            "screen_position".to_string(),
            TelemetryValue::Array(
                click
                    .pointer_location
                    .position
                    .to_array()
                    .into_iter()
                    .map(|value| TelemetryValue::F64(value as f64))
                    .collect(),
            ),
        ),
        ("prim_paths".to_string(), TelemetryValue::Array(prim_paths)),
        ("modifiers".to_string(), modifiers),
    ];
    if let Some(policy) = pointer_policy {
        context.push((
            "pointer_policy".to_string(),
            tool_map(vec![
                (
                    "left".to_string(),
                    TelemetryValue::String(pointer_interaction_name(policy.left).to_string()),
                ),
                (
                    "right".to_string(),
                    TelemetryValue::String(pointer_interaction_name(policy.right).to_string()),
                ),
            ]),
        ));
    }
    if let Ok(id) = q_ids.get(click.entity) {
        context.push((
            "hit_entity_id".to_string(),
            TelemetryValue::I64(id.get() as i64),
        ));
    }
    if let Some(part) = inspector_part.and_then(|entity| q_ids.get(entity).ok()) {
        context.push((
            "inspector_part_entity_id".to_string(),
            TelemetryValue::I64(part.get() as i64),
        ));
    }
    if let Ok(id) = q_ids.get(root) {
        context.push((
            "target_entity_id".to_string(),
            TelemetryValue::I64(id.get() as i64),
        ));
    }
    if let Ok(path) = q_prim.get(click.entity) {
        context.push((
            "hit_path".to_string(),
            TelemetryValue::String(path.path.clone()),
        ));
    }
    if let Some(path) = target_prim {
        context.push((
            "target_path".to_string(),
            TelemetryValue::String(path.path.clone()),
        ));
        if let Some(doc) =
            lunco_usd_bevy_twin::scene_document_for(backed, asset_server, path.stage_handle.id())
        {
            context.push(("doc_id".to_string(), TelemetryValue::U64(doc.raw())));
        }
    }
    if let Some(entity) = selected_entity {
        if let Ok(id) = q_ids.get(entity) {
            context.push((
                "selected_entity_id".to_string(),
                TelemetryValue::I64(id.get() as i64),
            ));
        }
        if let Ok(path) = q_prim.get(entity) {
            context.push((
                "selected_path".to_string(),
                TelemetryValue::String(path.path.clone()),
            ));
        }
    }
    if let Some(entity) = controlled_entity {
        if let Ok(id) = q_ids.get(entity) {
            context.push((
                "controlled_entity_id".to_string(),
                TelemetryValue::I64(id.get() as i64),
            ));
        }
        if let Ok(path) = q_prim.get(entity) {
            context.push((
                "controlled_path".to_string(),
                TelemetryValue::String(path.path.clone()),
            ));
        }
    }
    if let Some(path) = context_prim {
        if let Some(doc) =
            lunco_usd_bevy_twin::scene_document_for(backed, asset_server, path.stage_handle.id())
        {
            if !context.iter().any(|(name, _)| name == "doc_id") {
                context.push(("doc_id".to_string(), TelemetryValue::U64(doc.raw())));
            }
        }
        if let Some(scene_root) = q_scene_roots
            .iter()
            .find(|root| root.stage_handle.id() == path.stage_handle.id())
        {
            context.push((
                "scene_root_path".to_string(),
                TelemetryValue::String(scene_root.path.clone()),
            ));
        }
    }
    if let Some(surface_position) = pointer_surface_render_position(
        click,
        q_prim,
        q_parents,
        q_lod_tiles,
        q_scene_cameras,
        viewport,
        surface,
    ) {
        let surface_position = RenderPos(surface_position);
        context.push((
            "surface_render_position".to_string(),
            coordinate_point(surface_position.0, RENDER_FRAME_NAME, "terrain_surface"),
        ));
        if let Some(surface_world_position) = coordinates.render_to_active(surface_position) {
            context.push((
                "surface_world_position".to_string(),
                coordinate_point(
                    surface_world_position.0,
                    ACTIVE_FRAME_NAME,
                    "terrain_surface",
                ),
            ));
        }
    }
    if let Some(position) = canonical_pointer_render_position(
        click,
        q_prim,
        q_parents,
        q_lod_tiles,
        q_scene_cameras,
        viewport,
        surface,
    ) {
        let render_position = RenderPos(position);
        context.push((
            "render_position".to_string(),
            coordinate_point(render_position.0, RENDER_FRAME_NAME, "pointer_hit"),
        ));
        if let Some(world_position) = coordinates.render_to_active(render_position) {
            context.push((
                "world_position".to_string(),
                coordinate_point(world_position.0, ACTIVE_FRAME_NAME, "pointer_hit"),
            ));
        } else {
            context.push((
                "position_error".to_string(),
                TelemetryValue::String("active scene coordinate frame is unavailable".to_string()),
            ));
        }
    }
    tool_map(context)
}

fn pointer_interaction_name(
    interaction: lunco_interaction_core::PointerInteraction,
) -> &'static str {
    match interaction {
        lunco_interaction_core::PointerInteraction::Block => "block",
        lunco_interaction_core::PointerInteraction::PassThrough => "pass_through",
        lunco_interaction_core::PointerInteraction::Context => "context",
    }
}

fn inherited_pointer_interaction(
    target: Entity,
    button: PointerButton,
    policies: &Query<&lunco_interaction_core::ScenePointerPolicy>,
    parents: &Query<&ChildOf>,
) -> Option<lunco_interaction_core::PointerInteraction> {
    let mut ancestor = Some(target);
    for _ in 0..32 {
        let entity = ancestor?;
        if let Ok(policy) = policies.get(entity) {
            return Some(crate::ui::scene_context::interaction_for_button(
                *policy, button,
            ));
        }
        ancestor = parents.get(entity).ok().map(|parent| parent.0);
    }
    None
}

/// Return a hit in the renderer's floating-origin frame.
///
/// The mesh picker reports positions in the picked entity's local frame. That
/// is valid for an ordinary prop, but a streamed terrain tile is nested under
/// BigSpace and its local hit can be near zero even when the terrain is ~2 km
/// below the body datum. The analytic surface query owns the canonical
/// screen-ray → active-grid conversion, so use it only when the visible pick
/// belongs to that terrain. Props, vehicles, and the sky keep their mesh hit
/// or no-hit result; hidden analytic terrain is never an interaction target.
fn canonical_pointer_render_position(
    click: &Pointer<Click>,
    q_prim: &Query<&lunco_usd_bevy_scene::UsdPrimPath>,
    q_parents: &Query<&ChildOf>,
    q_lod_tiles: &Query<&lunco_terrain_surface::stream_viz::LodTileOf>,
    q_scene_cameras: &Query<
        (&Camera, &GlobalTransform),
        (With<Camera3d>, With<lunco_render::SceneCamera>),
    >,
    viewport: &lunco_viewport_core::SceneViewport,
    surface: &lunco_terrain_surface::GridSurfaceQuery<'_, '_>,
) -> Option<bevy::math::DVec3> {
    let mesh_position = click.hit.position?.as_dvec3();
    let Some(surface_position) = pointer_surface_render_position(
        click,
        q_prim,
        q_parents,
        q_lod_tiles,
        q_scene_cameras,
        viewport,
        surface,
    ) else {
        return Some(mesh_position);
    };
    // The surface helper already established that the picked entity belongs to
    // the visible terrain. Returning this analytic point fixes the tile-local
    // picker coordinate while keeping ordinary prop and sky clicks on their
    // original mesh/no-hit path.
    Some(surface_position)
}

/// Resolve the analytic terrain point below a scene pointer in the renderer's
/// floating-origin frame. This is a second, explicitly named coordinate fact:
/// the ordinary mesh hit may be local to a streamed tile, while this value is
/// suitable for terrain-aware authored tools.
fn pointer_surface_render_position(
    click: &Pointer<Click>,
    q_prim: &Query<&lunco_usd_bevy_scene::UsdPrimPath>,
    q_parents: &Query<&ChildOf>,
    q_lod_tiles: &Query<&lunco_terrain_surface::stream_viz::LodTileOf>,
    q_scene_cameras: &Query<
        (&Camera, &GlobalTransform),
        (With<Camera3d>, With<lunco_render::SceneCamera>),
    >,
    viewport: &lunco_viewport_core::SceneViewport,
    surface: &lunco_terrain_surface::GridSurfaceQuery<'_, '_>,
) -> Option<bevy::math::DVec3> {
    if let Some(terrain_hit) = click.hit.extra_as::<lunco_terrain_surface::SurfaceHit>() {
        return surface.to_render(terrain_hit.point).map(|point| point.0);
    }
    let camera_entity = viewport.active_camera?;
    let (camera, camera_transform) = q_scene_cameras.get(camera_entity).ok()?;
    let ray = lunco_viewport_core::scene_click_ray(
        false,
        camera,
        camera_transform,
        click.pointer_location.position,
    )?;
    let terrain_hit = surface.raycast_render(
        RenderPos(ray.origin.as_dvec3()),
        ray.direction,
        f64::INFINITY,
    )?;
    let hit_is_terrain = std::iter::successors(Some(click.entity), |entity| {
        q_parents.get(*entity).ok().map(|parent| parent.0)
    })
    .any(|entity| {
        entity == terrain_hit.terrain
            || q_lod_tiles
                .get(entity)
                .is_ok_and(|tile| tile.0 == terrain_hit.terrain)
    });
    let hit_has_authored_prim = std::iter::successors(Some(click.entity), |entity| {
        q_parents.get(*entity).ok().map(|parent| parent.0)
    })
    .any(|entity| q_prim.get(entity).is_ok());
    if !hit_is_terrain && hit_has_authored_prim {
        return None;
    }
    // Streamed DEM tiles are render-only entities: they intentionally have no
    // USD prim identity, while the analytic surface query is the authoritative
    // terrain owner. If the winning render hit has no authored prim, the ray
    // intersection above is sufficient to classify it as terrain. An authored
    // prop/vehicle/ribbon remains a hard boundary, so a terrain point behind it
    // is never silently substituted for the clicked object.
    surface.to_render(terrain_hit.point).map(|point| point.0)
}

/// Encode a point for the Rhai coordinate contract. The values stay native
/// `TelemetryValue` data; this is not a JSON transport or an untagged vector.
fn coordinate_point(position: bevy::math::DVec3, frame: &str, source: &str) -> TelemetryValue {
    tool_map(vec![
        (
            "kind".to_string(),
            TelemetryValue::String("point3".to_string()),
        ),
        (
            "values".to_string(),
            TelemetryValue::Array(
                position
                    .to_array()
                    .into_iter()
                    .map(TelemetryValue::F64)
                    .collect(),
            ),
        ),
        (
            "frame".to_string(),
            TelemetryValue::String(frame.to_string()),
        ),
        (
            "source".to_string(),
            TelemetryValue::String(source.to_string()),
        ),
    ])
}

/// Publish one typed scene-pointer event for Rhai policy programs. The event
/// contains the same context as an armed tool, including all modifier flags;
/// no Rust code assigns meaning to Alt, Shift, or Ctrl.
#[allow(clippy::too_many_arguments)]
pub(crate) fn on_scene_pointer_event(
    mut click: On<Pointer<Click>>,
    keys: Res<ButtonInput<KeyCode>>,
    previous_hover_map: Res<PreviousHoverMap>,
    armed: Res<lunco_interaction_core::ArmedScriptTool>,
    spawn_state: Res<lunco_luncosim_edit_core::SpawnState>,
    terrain_active: Res<lunco_interaction_core::TerrainToolActive>,
    mut dispatch: ResMut<ScenePointerDispatch>,
    world: SceneToolWorld,
    mut commands: Commands,
) {
    // ScenePickGate emits a foreground capture hit for UI chrome. Resolve the
    // actual scene target from the previous hover map used to emit this Click.
    // EguiFocus is published after picking and can still describe the previous
    // cursor location when the pointer has just left a menu.
    if armed.armed()
        || !matches!(
            spawn_state.as_ref(),
            lunco_luncosim_edit_core::SpawnState::Idle
        )
        || terrain_active.0
    {
        return;
    }
    let Some(target) = ordered_scene_pointer_target(
        &previous_hover_map,
        click.pointer_id,
        click.button,
        &world.q_ids,
        &world.q_prim,
        &world.q_pointer_policy,
        &world.q_parents,
    ) else {
        return;
    };
    // Bevy dispatches hovered entities from a hash map, so observer order is
    // unrelated to hit depth. Only the entity selected from the ordered hit
    // set may publish this gesture; a lower terrain event cannot win merely
    // because its observer ran first.
    if click.entity != target {
        click.propagate(false);
        return;
    }
    let key = ScenePointerKey {
        pointer: click.pointer_id,
        button: click.button,
        click_count: click.count,
        screen_position: [
            click.pointer_location.position.x.to_bits(),
            click.pointer_location.position.y.to_bits(),
        ],
    };
    if !dispatch.seen.insert(key) {
        return;
    }
    let mut context = scene_tool_context(
        &click,
        &keys,
        &world.q_selectable,
        &world.q_mobility,
        &world.q_ids,
        &world.q_prim,
        &world.q_pointer_policy,
        &world.q_scene_roots,
        &world.q_parents,
        &world.selected,
        &world.local_avatar,
        &world.q_links,
        &world.backed,
        &world.asset_server,
        &world.coordinates,
        &world.input_bindings,
        &world.scene_interaction,
        &world.q_scene_cameras,
        &world.q_lod_tiles,
        &world.viewport,
        &world.surface,
    );
    if let TelemetryValue::Map(fields) = &mut context {
        if let Some(TelemetryValue::U64(doc)) = fields.get("doc_id") {
            if let Some(active) = dispatch.move_hooks.get(&DocumentId::new(*doc)) {
                fields.insert(
                    "active_pointer_move".to_string(),
                    tool_map(vec![
                        (
                            "interaction_id".to_string(),
                            TelemetryValue::String(active.interaction_id.clone()),
                        ),
                        (
                            "tool".to_string(),
                            TelemetryValue::String(active.tool.clone()),
                        ),
                        (
                            "hook".to_string(),
                            TelemetryValue::String(active.hook.clone()),
                        ),
                        (
                            "context".to_string(),
                            TelemetryValue::Map(
                                active
                                    .context
                                    .iter()
                                    .map(|(key, value)| {
                                        (key.clone(), TelemetryValue::String(value.clone()))
                                    })
                                    .collect(),
                            ),
                        ),
                    ]),
                );
            }
        }
    }
    let source = world
        .q_ids
        .get(click.entity)
        .map(|id| id.get())
        .unwrap_or_default();
    commands.trigger(lunco_scripting_rhai_runtime::commands::RunRhaiToolHook {
        tool: "scene_interaction".to_string(),
        hook: "on_pointer".to_string(),
        args: context.clone(),
        owner_twin_id: None,
    });
    commands.trigger(TelemetryEvent {
        name: "scene.pointer".to_string(),
        source,
        severity: lunco_telemetry_core::Severity::Info,
        data: context,
        timestamp: 0.0,
        sim_secs: 0.0,
        sim_tick: 0,
    });
}

fn ordered_scene_pointer_target<'w, 's>(
    previous_hover_map: &PreviousHoverMap,
    pointer: PointerId,
    button: PointerButton,
    ids: &Query<'w, 's, &'static lunco_core::GlobalEntityId>,
    prims: &Query<'w, 's, &'static lunco_usd_bevy_scene::UsdPrimPath>,
    policies: &Query<'w, 's, &'static lunco_interaction_core::ScenePointerPolicy>,
    parents: &Query<'w, 's, &'static ChildOf>,
) -> Option<Entity> {
    let hits = previous_hover_map.get(&pointer)?;
    let mut candidates = Vec::with_capacity(hits.len());
    for (&entity, hit) in hits {
        let scene_hit =
            nearest_scene_prim(entity, prims, parents).is_some() || ids.get(entity).is_ok();
        if scene_hit {
            candidates.push((entity, hit.depth));
        }
    }
    ordered_scene_hit_target(candidates, |entity| {
        inherited_pointer_interaction(entity, button, policies, parents)
            .unwrap_or(lunco_interaction_core::PointerInteraction::Block)
    })
}

fn ordered_scene_hit_target(
    hits: impl IntoIterator<Item = (Entity, f32)>,
    mut interaction: impl FnMut(Entity) -> lunco_interaction_core::PointerInteraction,
) -> Option<Entity> {
    let mut hits: Vec<_> = hits
        .into_iter()
        .filter(|(_, depth)| depth.is_finite())
        .collect();
    hits.sort_by(|(left_entity, left_depth), (right_entity, right_depth)| {
        left_depth
            .total_cmp(right_depth)
            .then_with(|| left_entity.to_bits().cmp(&right_entity.to_bits()))
    });
    hits.into_iter().find_map(|(entity, _)| {
        (interaction(entity) != lunco_interaction_core::PointerInteraction::PassThrough)
            .then_some(entity)
    })
}

/// Feed one coalesced scene-hover position to the authored interaction policy.
/// Pointer movement is presentation input: it only updates a disposable view
/// ghost and never waits for a physics event or rebuilds route geometry.
pub(crate) fn on_scene_pointer_move_event(
    mut pointer_move: On<Pointer<Move>>,
    armed: Res<lunco_interaction_core::ArmedScriptTool>,
    spawn_state: Res<lunco_luncosim_edit_core::SpawnState>,
    terrain_active: Res<lunco_interaction_core::TerrainToolActive>,
    mut dispatch: ResMut<ScenePointerDispatch>,
    world: SceneToolWorld,
) {
    if dispatch.move_hooks.is_empty() {
        return;
    }
    // Like clicks, movement is admitted from the picked scene hit below. The
    // EguiFocus snapshot is published after picking, so it can reject the only
    // movement sample when a cursor leaves a popup for the viewport.
    if armed.armed()
        || !matches!(
            spawn_state.as_ref(),
            lunco_luncosim_edit_core::SpawnState::Idle
        )
        || terrain_active.0
    {
        return;
    }
    if pointer_move.hit.position.is_none()
        && nearest_scene_prim(pointer_move.entity, &world.q_prim, &world.q_parents).is_none()
    {
        return;
    }
    // A pass-through preview still appears in Bevy's ordered hit stream. If
    // it wins scene-pointer deduplication, the terrain beneath it never gets
    // a chance to provide the placement point. Apply the same authored policy
    // used by click routing before inserting the pointer into that stream.
    if inherited_pointer_interaction(
        pointer_move.entity,
        PointerButton::Primary,
        &world.q_pointer_policy,
        &world.q_parents,
    ) == Some(lunco_interaction_core::PointerInteraction::PassThrough)
    {
        pointer_move.propagate(false);
        return;
    }
    queue_scene_pointer_move(
        pointer_move.pointer_id,
        pointer_move.entity,
        pointer_move.pointer_location.position,
        pointer_move.hit.position,
        pointer_move
            .hit
            .extra_as::<lunco_terrain_surface::SurfaceHit>()
            .copied(),
        &mut dispatch,
    );
}

pub(crate) fn on_scene_pointer_enter_event(
    mut pointer_enter: On<Pointer<Enter>>,
    armed: Res<lunco_interaction_core::ArmedScriptTool>,
    spawn_state: Res<lunco_luncosim_edit_core::SpawnState>,
    terrain_active: Res<lunco_interaction_core::TerrainToolActive>,
    mut dispatch: ResMut<ScenePointerDispatch>,
    world: SceneToolWorld,
) {
    if dispatch.move_hooks.is_empty() {
        return;
    }
    // A cursor can cross from UI chrome into the viewport while the previous
    // frame's capture hit is still being retired. The refreshed scene hit emits
    // Enter even when the cursor did not move again; use that current hit as
    // the first preview sample.
    if armed.armed()
        || !matches!(
            spawn_state.as_ref(),
            lunco_luncosim_edit_core::SpawnState::Idle
        )
        || terrain_active.0
        || (pointer_enter.hit.position.is_none()
            && nearest_scene_prim(pointer_enter.entity, &world.q_prim, &world.q_parents).is_none())
    {
        return;
    }
    if inherited_pointer_interaction(
        pointer_enter.entity,
        PointerButton::Primary,
        &world.q_pointer_policy,
        &world.q_parents,
    ) == Some(lunco_interaction_core::PointerInteraction::PassThrough)
    {
        pointer_enter.propagate(false);
        return;
    }
    queue_scene_pointer_move(
        pointer_enter.pointer_id,
        pointer_enter.entity,
        pointer_enter.pointer_location.position,
        pointer_enter.hit.position,
        pointer_enter
            .hit
            .extra_as::<lunco_terrain_surface::SurfaceHit>()
            .copied(),
        &mut dispatch,
    );
}

fn queue_scene_pointer_move(
    pointer: PointerId,
    entity: Entity,
    position: Vec2,
    hit_position: Option<Vec3>,
    surface_hit: Option<lunco_terrain_surface::SurfaceHit>,
    dispatch: &mut ScenePointerDispatch,
) {
    let key = ScenePointerMoveKey {
        pointer,
        screen_position: [position.x.to_bits(), position.y.to_bits()],
    };
    if !dispatch.seen_moves.insert(key) {
        return;
    }
    coalesce_scene_pointer_move(
        dispatch,
        pointer,
        ScenePointerMoveSample {
            entity,
            position,
            hit_position,
            surface_hit,
        },
    );
}

fn coalesce_scene_pointer_move(
    dispatch: &mut ScenePointerDispatch,
    pointer: PointerId,
    sample: ScenePointerMoveSample,
) {
    if let Some((_, pending)) = dispatch
        .pending_moves
        .iter_mut()
        .find(|(pending_pointer, _)| *pending_pointer == pointer)
    {
        *pending = sample;
    } else {
        dispatch.pending_moves.push((pointer, sample));
    }
}

struct ScenePointerMoveContext {
    doc: DocumentId,
    args: TelemetryValue,
}

fn scene_pointer_move_args(
    sample: ScenePointerMoveSample,
    world: &SceneToolWorld,
    move_hooks: &HashMap<DocumentId, ScenePointerMoveHook>,
) -> Option<ScenePointerMoveContext> {
    let ScenePointerMoveSample {
        entity,
        position,
        hit_position,
        surface_hit,
    } = sample;
    // Picking can target a collider or terrain LOD child that has no prim
    // component of its own. Resolve the closest authored scene identity just
    // as click routing does before scoping the hover to its document.
    let hit_prim = nearest_scene_prim(entity, &world.q_prim, &world.q_parents)?;
    let Some(doc) = lunco_usd_bevy_twin::scene_document_for(
        &world.backed,
        &world.asset_server,
        hit_prim.stage_handle.id(),
    ) else {
        return None;
    };
    if !move_hooks.contains_key(&doc) {
        return None;
    }
    let hit_is_terrain = std::iter::successors(Some(entity), |entity| {
        world.q_parents.get(*entity).ok().map(|parent| parent.0)
    })
    .any(|entity| world.q_lod_tiles.get(entity).is_ok());
    let surface_render_position = surface_hit
        .and_then(|hit| world.surface.to_render(hit.point))
        .or_else(|| {
            if !hit_is_terrain {
                return None;
            }
            let camera_entity = world.viewport.active_camera?;
            let (camera, camera_transform) = world.q_scene_cameras.get(camera_entity).ok()?;
            let ray =
                lunco_viewport_core::scene_click_ray(false, camera, camera_transform, position)?;
            let _span = bevy::log::info_span!("scene_pointer_move_terrain_raycast").entered();
            let hit = world.surface.raycast_render(
                RenderPos(ray.origin.as_dvec3()),
                ray.direction,
                f64::INFINITY,
            )?;
            world.surface.to_render(hit.point)
        });
    let is_surface_hit = surface_render_position.is_some();
    let render_position = if let Some(surface_position) = surface_render_position {
        surface_position
    } else if !hit_is_terrain {
        let position = hit_position?;
        RenderPos(position.as_dvec3())
    } else {
        return None;
    };
    let world_position = world.coordinates.render_to_active(render_position)?;
    let scene_root = world
        .q_scene_roots
        .iter()
        .find(|root| root.stage_handle.id() == hit_prim.stage_handle.id());
    let mut entries = vec![
        ("doc_id".to_string(), TelemetryValue::U64(doc.raw())),
        (
            "screen_position".to_string(),
            TelemetryValue::Array(vec![
                TelemetryValue::F64(position.x as f64),
                TelemetryValue::F64(position.y as f64),
            ]),
        ),
        (
            "world_position".to_string(),
            coordinate_point(
                world_position.0,
                ACTIVE_FRAME_NAME,
                if is_surface_hit {
                    "terrain_surface"
                } else {
                    "pointer_hit"
                },
            ),
        ),
    ];
    if let Some(selected) = world.selected.primary()
        && let Ok(selected_path) = world.q_prim.get(selected)
        && selected_path.stage_handle.id() == hit_prim.stage_handle.id()
    {
        entries.push((
            "selected_path".to_string(),
            TelemetryValue::String(selected_path.path.clone()),
        ));
    }
    if let Some(avatar) = world.local_avatar.0
        && let Ok(link) = world.q_links.get(avatar)
        && let Ok(controlled_path) = world.q_prim.get(link.target)
        && controlled_path.stage_handle.id() == hit_prim.stage_handle.id()
    {
        entries.push((
            "controlled_path".to_string(),
            TelemetryValue::String(controlled_path.path.clone()),
        ));
    }
    if is_surface_hit {
        entries.push((
            "surface_world_position".to_string(),
            coordinate_point(world_position.0, ACTIVE_FRAME_NAME, "terrain_surface"),
        ));
    }
    if let Some(scene_root) = scene_root {
        entries.push((
            "scene_root_path".to_string(),
            TelemetryValue::String(scene_root.path.clone()),
        ));
    }
    Some(ScenePointerMoveContext {
        doc,
        args: tool_map(entries),
    })
}

fn nearest_scene_prim<'q, 'w, 's>(
    entity: Entity,
    prims: &'q Query<'w, 's, &'static lunco_usd_bevy_scene::UsdPrimPath>,
    parents: &'q Query<'w, 's, &'static ChildOf>,
) -> Option<&'q lunco_usd_bevy_scene::UsdPrimPath> {
    let mut current = Some(entity);
    while let Some(candidate) = current {
        if let Ok(prim) = prims.get(candidate) {
            return Some(prim);
        }
        current = parents.get(candidate).ok().map(|parent| parent.0);
    }
    None
}

/// Dispatch only the latest pointer sample collected during this picking
/// pass. The UI-hook queue is intentionally bounded, so enqueuing every mouse
/// sample would make a visual preview replay an ever older cursor trail.
pub(crate) fn flush_scene_pointer_moves(
    mut dispatch: ResMut<ScenePointerDispatch>,
    mut commands: Commands,
    world: SceneToolWorld,
) {
    if dispatch.move_hooks.is_empty() {
        dispatch.pending_moves.clear();
        return;
    }
    let pending_moves = std::mem::take(&mut dispatch.pending_moves);
    for (_, sample) in pending_moves {
        // Resolve the retained raw hit once, after the picking pass has
        // coalesced all samples for this pointer. Terrain fallback raycasts
        // therefore run at most once per pointer per picking frame.
        let Some(context) = scene_pointer_move_args(sample, &world, &dispatch.move_hooks) else {
            continue;
        };
        let Some(subscription) = dispatch.move_hooks.get(&context.doc) else {
            continue;
        };
        let args = merge_pointer_move_context(context.args, &subscription.context);
        commands.trigger(lunco_scripting_rhai_runtime::commands::RunRhaiToolHook {
            tool: subscription.tool.clone(),
            hook: subscription.hook.clone(),
            args,
            owner_twin_id: subscription.owner_twin_id,
        });
    }
}

fn merge_pointer_move_context(
    event: TelemetryValue,
    stable: &BTreeMap<String, String>,
) -> TelemetryValue {
    let TelemetryValue::Map(mut event) = event else {
        return event;
    };
    for (key, value) in stable {
        event
            .entry(key.clone())
            .or_insert_with(|| TelemetryValue::String(value.clone()));
    }
    TelemetryValue::Map(event)
}

register_commands!(
    on_set_usd_view_preview_transform,
    on_set_scene_pointer_move_hook,
    on_clear_scene_pointer_move_hook,
    on_update_usd_curve_view
);

#[cfg(test)]
mod usd_curve_view_tests {
    use super::*;
    use bevy::math::DQuat;

    fn request(points: Vec<[f64; 3]>, max_samples: usize) -> UsdCurveViewRequest {
        UsdCurveViewRequest {
            revision: 1,
            root: Entity::PLACEHOLDER,
            curve: Entity::PLACEHOLDER,
            parent: Entity::PLACEHOLDER,
            stage_id: bevy::asset::AssetId::default(),
            points,
            width_m: 0.12,
            clearance_m: 0.03,
            sample_spacing_m: 3.0,
            max_samples,
        }
    }

    #[test]
    fn curve_view_mesh_uses_parent_local_route_frame_and_preserves_endpoints() {
        let points = vec![[1.0, 2.0, 3.0], [1.0, 2.0, -7.0]];
        let request = request(points.clone(), 256);
        let parent_position = DVec3::new(100.0, -20.0, 300.0);
        let parent_rotation = DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2);
        let result = build_usd_curve_view_mesh(&request, None, parent_position, parent_rotation)
            .unwrap()
            .unwrap();

        assert!(result.1.abs_diff_eq(DVec3::from_array(points[0]), 1.0e-9));
        assert!(result.0.count_vertices() >= 4);
        let bevy::mesh::VertexAttributeValues::Float32x3(vertices) = result
            .0
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("ribbon mesh must contain vertex positions")
        else {
            panic!("ribbon mesh positions must use the renderer vertex format");
        };
        assert!(vertices.iter().flatten().all(|value| value.is_finite()));
    }

    #[test]
    fn curve_view_mesh_adapts_spacing_before_allocating_long_routes() {
        let request = request(vec![[0.0, 0.0, 0.0], [0.0, 0.0, -10_000.0]], 256);
        let (mesh, _) = build_usd_curve_view_mesh(&request, None, DVec3::ZERO, DQuat::IDENTITY)
            .unwrap()
            .unwrap();

        assert!(mesh.count_vertices() <= 512);
    }

    #[test]
    fn short_curve_views_have_no_mesh_to_render() {
        let request = request(vec![[1.0, 2.0, 3.0]], 256);
        assert!(
            build_usd_curve_view_mesh(&request, None, DVec3::ZERO, DQuat::IDENTITY,)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn failed_current_curve_view_completes_without_advancing_applied_revision() {
        let entity = Entity::PLACEHOLDER;
        let mut pending = PendingUsdCurveViews::default();
        pending.status.insert(
            entity,
            UsdCurveViewStatus {
                requested_revision: 8,
                completed_revision: 7,
                applied_revision: 6,
                vertex_count: 12,
                error: None,
            },
        );

        complete_curve_view_status(
            &mut pending,
            entity,
            8,
            0,
            Some("test mesh failure".to_owned()),
        );

        let status = pending.status.get(&entity).unwrap();
        assert_eq!(status.completed_revision, 8);
        assert_eq!(status.applied_revision, 6);
        assert_eq!(status.vertex_count, 0);
        assert_eq!(status.error.as_deref(), Some("test mesh failure"));
    }

    #[test]
    fn stale_curve_view_completion_cannot_replace_newer_status() {
        let entity = Entity::PLACEHOLDER;
        let mut pending = PendingUsdCurveViews::default();
        pending.status.insert(
            entity,
            UsdCurveViewStatus {
                requested_revision: 9,
                completed_revision: 7,
                applied_revision: 7,
                vertex_count: 12,
                error: None,
            },
        );

        complete_curve_view_status(&mut pending, entity, 8, 4, None);

        let status = pending.status.get(&entity).unwrap();
        assert_eq!(status.requested_revision, 9);
        assert_eq!(status.completed_revision, 7);
        assert_eq!(status.applied_revision, 7);
        assert_eq!(status.vertex_count, 12);
        assert_eq!(status.error, None);
    }

    #[test]
    fn scene_click_target_uses_depth_and_button_policy_not_event_order() {
        use lunco_interaction_core::PointerInteraction;

        let marker = Entity::from_raw_u32(1).unwrap();
        let terrain = Entity::from_raw_u32(2).unwrap();
        let hits = [(terrain, 8.0), (marker, 2.0)];

        assert_eq!(
            ordered_scene_hit_target(hits, |entity| {
                if entity == marker {
                    PointerInteraction::Context
                } else {
                    PointerInteraction::Block
                }
            }),
            Some(marker),
        );
        assert_eq!(
            ordered_scene_hit_target(hits, |entity| {
                if entity == marker {
                    PointerInteraction::PassThrough
                } else {
                    PointerInteraction::Block
                }
            }),
            Some(terrain),
        );
    }
}
