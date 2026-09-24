//! `QueryUsdPrim` — read composed USD attributes and explicit relationships off
//! an explicit Editor document or the mounted live stage.
//!
//! ## Ownership
//!
//! An explicit `doc_id` uses the synchronized document-to-stage mapping when a
//! Twin owns the document, or the document's composed data for an isolated
//! Editor fork. Without `doc_id`, exactly one mounted live stage is required.
//! Preview focus, duplicate prim paths, and detached cached stages never choose
//! the query target.
//!
//! ## Why a query provider and not a rhai binding
//!
//! Registering here puts it on the SHARED surface: one implementation answers
//! rhai (`query("QueryUsdPrim", #{...})`), Python, raw HTTP, MCP and telemetry.
//! A `register_fn` on the rhai engine would have served rhai alone and left every
//! other consumer to reimplement it.
//!
//! ## Frames
//!
//! `attrs` are the **authored** values in the prim's own space — that is the
//! point; an invariant check wants what the file says. `world_position` is in
//! the semantic active physics frame, matching [`QueryEntity`](crate::entity_query)
//! and what `TransformEntity` accepts, and is present only when the prim spawned an
//! entity. Document queries instead return the authored placement in canonical
//! stage coordinates through the shared USD transform reader, marked with
//! `position_frame: "canonical_stage"`; they do not read a preview physics pose.
//! Quaternion attributes are arrays in USD component order `[w, x, y, z]`,
//! at authored precision promoted to f64, without a coordinate-basis change.
//! A request with `collision_bounds: true` adds the aggregate composed
//! collision AABB in canonical stage coordinates. It is derived by the shared
//! `lunco_usd_bevy_scene::collision::collision_aabb` reader, so nested compound
//! ownership, standard shape dimensions, purpose filtering, transforms, and
//! malformed-data errors have one owner for API, Rhai, and other consumers.
//! A request with `collision_geometry: true` adds exact composed vertices for
//! one collision Mesh or Cube in canonical stage coordinates. This is the
//! shape-level surface for interface checks that cannot be established by
//! overlapping AABBs alone. Mesh results include the authored collision
//! approximation; callers must reject approximations they cannot interpret.
//! A request with `geometry_bounds: true` adds the selected prim's composed
//! geometry AABB, including render-only shapes. This is the dimension-checking
//! primitive for visual requirements; callers do not need to reconstruct
//! bounds from `size`, `radius`, or transform opinions.
//! A request with `relationships: true` adds every composed relationship and
//! `connections: true` adds every composed attribute connection. Both are
//! opt-in because they enumerate the prim's full property surface; the default
//! response remains compatible with focused callers that request only selected
//! relationships. `schemas: true` adds the composed applied API schemas.
//!
//! A request with `topology: true` adds one scoped, read-only record containing
//! visual/collision parts, body ownership, per-part geometry bounds, local and
//! world transforms, source-layer stack heads, material/shader bindings,
//! joints, and the selected prim's projection/binding state. It is intentionally
//! opt-in because a topology walk is O(number of composed prims) and existing
//! callers do not need the extra payload.
//!
//! ## Request
//!
//! ```json
//! {"type":"ExecuteCommand","command": "QueryUsdPrim", "params": {"path": "/Hab1/ShieldWall/OuterSurface"}}
//! {"type":"ExecuteCommand","command": "QueryUsdPrim", "params": {"path": "…", "attrs": ["radius", "points"]}}
//! {"type":"ExecuteCommand","command": "QueryUsdPrim", "params": {"path": "…", "rels": ["lunco:mount:attachmentJoint"]}}
//! {"type":"ExecuteCommand","command": "QueryUsdPrim", "params": {"doc_id": 7, "path": "…", "collision_bounds": true}}
//! {"type":"ExecuteCommand","command": "QueryUsdPrim", "params": {"doc_id": 7, "path": "…", "collision_geometry": true}}
//! {"type":"ExecuteCommand","command": "QueryUsdPrim", "params": {"path": "…", "geometry_bounds": true}}
//! {"type":"ExecuteCommand","command": "QueryUsdPrim", "params": {"path": "…", "topology": true}}
//! {"type":"ExecuteCommand","command": "QueryUsdPrim", "params": {"path": "…", "relationships": true, "connections": true, "schemas": true}}
//! ```
//!
//! Omitting `attrs` returns every authored attribute on the prim. Naming them is
//! much cheaper on a prim carrying big arrays (a trimmed `NurbsPatch` holds
//! thousands of control points), so a hot loop should name them.

use bevy::ecs::query::QueryState;
use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::{
    api_param_array, api_param_bool, api_param_str, api_param_u64, ApiQueryError, ApiQueryResult,
};
use lunco_api_core::{api_value, ApiErrorCode, ApiValue};
use lunco_doc::{Document, DocumentId};
use lunco_doc_bevy::DocumentRegistry;
use lunco_usd_authoring::author::open_doc_stage;
use lunco_usd_bevy_scene::collision::{
    collision_aabb, prim_collision_geometry, prim_geometry_aabb, ObjectAabb,
};
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_usd_bevy_scene::UsdSceneRoot;
use lunco_usd_bevy_stage::read::UsdRead;
use lunco_usd_bevy_stage::view::StageView;
use lunco_usd_bevy_stage::{
    canonical::CanonicalStages, effective_purpose, is_descendant_or_self, resolve_bound_shader,
    MaterialPurpose, UsdStageAsset,
};
use lunco_usd_bevy_twin::{canonical_stage_for_document, scene_document_for, DocBackedTwinScenes};
use lunco_usd_document::document::UsdDocument;
use openusd::sdf::{Path as SdfPath, Value};
use std::collections::{HashMap, HashSet};

/// One attribute, converted to a typed API value by probing the USD readers.
///
/// USD's value types are distinct `sdf::Value` variants, so there is no single
/// "get me whatever this is" call — `scalar::<f64>` misses a `float` opinion,
/// `scalar::<String>` misses a `token`, and an array read that misses yields an
/// EMPTY vec rather than `None`. The tolerant helpers (`real`, `text`, `reals`,
/// `points3`) each collapse one of those traps; this walks them scalar-first,
/// then array, and reports `null` only when every reader declined.
///
/// Emptiness is why the array probes are guarded with `is_empty()`: an
/// unguarded `reals()` would answer `[]` for a `token` attribute and shadow the
/// text reader below it.
fn attr_api_value(view: &StageView<'_>, prim: &SdfPath, name: &str) -> ApiValue {
    // Scalars first — an array reader would answer `[]` for these, not `None`.
    if let Some(v) = view.real(prim, name) {
        return api_value!(v);
    }
    if let Some(v) = view.boolean(prim, name) {
        return api_value!(v);
    }
    if let Some(v) = view.scalar::<i32>(prim, name) {
        return api_value!(v);
    }
    if let Some(v) = view.text(prim, name) {
        return api_value!(v);
    }
    if let Some(v) = view.asset(prim, name) {
        return api_value!(v);
    }
    if let Some(q) = view.quat_d(prim, name) {
        return api_value!([q.w, q.x, q.y, q.z]);
    }

    // Arrays. `points3` before `reals` because a `point3f[]` also satisfies no
    // scalar reader and we want it shaped [[x,y,z], …], not flattened.
    let pts = view.points3(prim, name);
    if !pts.is_empty() {
        return ApiValue::Array(
            pts.into_iter()
                .map(|point| api_value!([point[0], point[1], point[2]]))
                .collect(),
        );
    }
    let reals = view.reals(prim, name);
    if !reals.is_empty() {
        return api_value!(reals);
    }
    let texts = view.texts(prim, name);
    if !texts.is_empty() {
        return api_value!(texts);
    }
    // Scalar vectors and integer arrays require the raw Value variants;
    // the typed readers above do not cover their shapes.
    match view.attr_value(prim, name) {
        // Scalar 2/3/4-vectors as flat JSON arrays — the same shape `points3`
        // gives each element of a `point3f[]`, so `v[1]` means "y" whether the
        // caller is reading one translate or one control point.
        Some(Value::Vec2f(v)) => api_value!([v.x, v.y]),
        Some(Value::Vec2d(v)) => api_value!([v.x, v.y]),
        Some(Value::Vec2i(v)) => api_value!([v.x, v.y]),
        Some(Value::Vec3f(v)) => api_value!([v.x, v.y, v.z]),
        Some(Value::Vec3d(v)) => api_value!([v.x, v.y, v.z]),
        Some(Value::Vec3i(v)) => api_value!([v.x, v.y, v.z]),
        Some(Value::Vec4f(v)) => api_value!([v.x, v.y, v.z, v.w]),
        Some(Value::Vec4d(v)) => api_value!([v.x, v.y, v.z, v.w]),
        Some(Value::Vec4i(v)) => api_value!([v.x, v.y, v.z, v.w]),

        Some(Value::IntVec(v)) if !v.is_empty() => api_value!(v.clone()),
        Some(Value::Int64Vec(v)) if !v.is_empty() => {
            api_value!(v.to_vec())
        }
        _ => ApiValue::Unit,
    }
}

fn purpose_name(purpose: lunco_usd_bevy_stage::Purpose) -> &'static str {
    match purpose {
        lunco_usd_bevy_stage::Purpose::Default => "default",
        lunco_usd_bevy_stage::Purpose::Render => "render",
        lunco_usd_bevy_stage::Purpose::Proxy => "proxy",
        lunco_usd_bevy_stage::Purpose::Guide => "guide",
    }
}

fn is_visual_geometry(type_name: &str) -> bool {
    matches!(
        type_name,
        "Mesh"
            | "NurbsPatch"
            | "BasisCurves"
            | "NurbsCurves"
            | "Cube"
            | "Sphere"
            | "Cylinder"
            | "Cone"
            | "Capsule"
            | "Plane"
    )
}

fn nearest_rigid_body(view: &StageView<'_>, path: &SdfPath) -> Option<SdfPath> {
    let mut current = Some(path.clone());
    while let Some(candidate) = current {
        if view.has_api_schema(
            &candidate,
            openusd::schemas::physics::tokens::API_RIGID_BODY,
        ) {
            return Some(candidate);
        }
        current = candidate.parent();
    }
    None
}

fn aabb_api_value(aabb: ObjectAabb) -> ApiValue {
    api_value!({
        "min": api_value!([aabb.min.x, aabb.min.y, aabb.min.z]),
        "max": api_value!([aabb.max.x, aabb.max.y, aabb.max.z]),
        "center": api_value!([
            (aabb.min.x + aabb.max.x) * 0.5,
            (aabb.min.y + aabb.max.y) * 0.5,
            (aabb.min.z + aabb.max.z) * 0.5,
        ]),
        "half_extents": api_value!([
            (aabb.max.x - aabb.min.x) * 0.5,
            (aabb.max.y - aabb.min.y) * 0.5,
            (aabb.max.z - aabb.min.z) * 0.5,
        ]),
        "frame": "canonical_stage",
    })
}

fn transform_api_value(transform: Option<Transform>) -> ApiValue {
    transform.map_or(ApiValue::Unit, |transform| {
        api_value!({
            "translation": api_value!([
                transform.translation.x,
                transform.translation.y,
                transform.translation.z,
            ]),
            "rotation": api_value!([
                transform.rotation.w,
                transform.rotation.x,
                transform.rotation.y,
                transform.rotation.z,
            ]),
            "scale": api_value!([transform.scale.x, transform.scale.y, transform.scale.z]),
        })
    })
}

fn source_layer_api_value(view: &StageView<'_>, path: &SdfPath) -> ApiValue {
    let Ok(stack) = view.stage().prim(path.clone()).prim_stack() else {
        return ApiValue::Unit;
    };
    let Some((layer, authored_path)) = stack.first() else {
        return ApiValue::Unit;
    };
    api_value!({
        "layer": layer.to_string(),
        "path": authored_path.to_string(),
        "stack_depth": stack.len(),
    })
}

fn topology_for_stage(view: &StageView<'_>, selected: &SdfPath) -> ApiValue {
    let mut diagnostics = Vec::new();
    let body_owner = nearest_rigid_body(view, selected);
    let root = body_owner.clone().unwrap_or_else(|| selected.clone());
    let paths = view.prim_paths();
    let mut parts = Vec::new();
    let mut frames = Vec::new();

    for candidate in paths.iter().filter(|candidate| {
        is_descendant_or_self(candidate, root.as_str()) && view.is_active(candidate)
    }) {
        let Some(type_name) = view.type_name(candidate) else {
            continue;
        };
        let collider =
            view.has_api_schema(candidate, openusd::schemas::physics::tokens::API_COLLISION);
        let visual = is_visual_geometry(&type_name) && !view.is_invisible_or_guide(candidate);
        if !collider && !visual {
            continue;
        }

        let collision_enabled = match view.boolean(candidate, "physics:collisionEnabled") {
            Some(value) => Some(value),
            None if view.has_authored_attribute(candidate, "physics:collisionEnabled") => {
                diagnostics.push(format!(
                    "{candidate}: physics:collisionEnabled has an unsupported value type"
                ));
                None
            }
            None if collider => Some(true),
            None => None,
        };
        let bounds = match prim_geometry_aabb(view, candidate.as_str()) {
            Ok(Some(aabb)) => aabb_api_value(aabb),
            Ok(None) => {
                diagnostics.push(format!(
                    "{candidate}: geometry bounds are unavailable for {type_name}"
                ));
                ApiValue::Unit
            }
            Err(error) => {
                diagnostics.push(format!("{candidate}: geometry bounds failed: {error}"));
                ApiValue::Unit
            }
        };

        let local = match view.local_transform_at(candidate, 0.0) {
            Ok(value) => transform_api_value(value),
            Err(error) => {
                diagnostics.push(format!("{candidate}: local transform failed: {error}"));
                ApiValue::Unit
            }
        };
        let world = match lunco_usd_bevy_stage::world_transform(view, candidate) {
            Ok(value) => transform_api_value(Some(value)),
            Err(error) => {
                diagnostics.push(format!("{candidate}: world transform failed: {error}"));
                ApiValue::Unit
            }
        };

        frames.push(api_value!({
            "path": candidate.as_str(),
            "local_frame": "canonical_stage_parent",
            "local": local.clone(),
            "world_frame": "canonical_stage",
            "world": world.clone(),
        }));
        let render_material = view.bound_material(candidate, MaterialPurpose::Render);
        let physics_material = view.bound_material(candidate, MaterialPurpose::Physics);
        let shader = resolve_bound_shader(view, candidate).map(|path| path.as_str().to_string());

        parts.push(api_value!({
            "path": candidate.as_str(),
            "type_name": type_name,
            "purpose": purpose_name(effective_purpose(view, candidate)),
            "visual": visual,
            "collider": collider,
            "collision_enabled": collision_enabled,
            "body_owner": nearest_rigid_body(view, candidate)
                .map(|path| path.as_str().to_string()),
            "bounds": bounds,
            "transform": {
                "local_frame": "canonical_stage_parent",
                "local": local,
                "world_frame": "canonical_stage",
                "world": world,
            },
            "materials": {
                "render": render_material,
                "physics": physics_material,
                "shader": shader,
            },
            "source": source_layer_api_value(view, candidate),
        }));
    }

    let mut joints = Vec::new();
    for candidate in paths.iter().filter(|candidate| {
        is_descendant_or_self(candidate, root.as_str()) && view.is_active(candidate)
    }) {
        let Some(type_name) = view.type_name(candidate) else {
            continue;
        };
        if !(type_name.ends_with("Joint")
            && (type_name.starts_with("Physics") || type_name.starts_with("Physx")))
        {
            continue;
        }
        let body0 = view
            .rel_targets(candidate, "physics:body0")
            .into_iter()
            .next()
            .map(|path| path.as_str().to_string());
        let body1 = view
            .rel_targets(candidate, "physics:body1")
            .into_iter()
            .next()
            .map(|path| path.as_str().to_string());
        if body0.is_none() || body1.is_none() {
            diagnostics.push(format!(
                "{candidate}: joint is missing physics:body0 or physics:body1"
            ));
        }
        joints.push(api_value!({
            "path": candidate.as_str(),
            "type_name": type_name,
            "body0": body0,
            "body1": body1,
            "source": source_layer_api_value(view, candidate),
        }));
    }

    api_value!({
        "selection": {
            "path": selected.as_str(),
            "scope": root.as_str(),
            "body_owner": body_owner.map(|path| path.as_str().to_string()),
        },
        "parts": parts,
        "frames": frames,
        "joints": joints,
        "diagnostics": diagnostics,
    })
}

fn runtime_binding_api_value(world: &World, entity: Option<Entity>) -> ApiValue {
    let Some(entity) = entity else {
        return api_value!({ "state": "not_projected" });
    };
    let visual_synced = world
        .get::<lunco_usd_bevy_scene::UsdSceneProjected>(entity)
        .is_some();
    let visual_sync_failed = world.get::<lunco_usd_bevy_scene::UsdSceneProjectionFailed>(entity);
    let awaiting_stage = world
        .get::<lunco_usd_bevy_scene::UsdSceneAwaitingStage>(entity)
        .is_some();
    let state = if visual_sync_failed.is_some() {
        "visual_sync_failed"
    } else if awaiting_stage {
        "awaiting_stage"
    } else if visual_synced {
        "visual_synced"
    } else {
        "projected_without_visual_sync"
    };
    let joint_link = world
        .get::<lunco_physics::PhysicsJointLink>(entity)
        .map(|link| {
            api_value!({
                "body0": link.body0.to_bits(),
                "body1": link.body1.to_bits(),
            })
        });
    api_value!({
        "state": state,
        "entity": entity.to_bits(),
        "visual_synced": visual_synced,
        "visual_mesh_pending": world
            .get::<lunco_usd_bevy_scene::UsdSceneGeometryPending>(entity)
            .is_some(),
        "visual_sync_error": visual_sync_failed.map(|error| error.0.clone()),
        "awaiting_stage": awaiting_stage,
        "rigid_body": world.get::<avian3d::prelude::RigidBody>(entity).is_some(),
        "collider": world.get::<avian3d::prelude::Collider>(entity).is_some(),
        "pending_joint": world
            .get::<lunco_usd_avian_contracts::PendingUsdJoint>(entity)
            .is_some(),
        "physics_joint_pending": world
            .get::<lunco_physics::PhysicsJointPending>(entity)
            .is_some(),
        "physics_joint_link": joint_link,
    })
}

type PrimRead = (
    Option<Vec3>,
    String,
    Vec<(String, ApiValue)>,
    Vec<(String, ApiValue)>,
    Vec<(String, ApiValue)>,
    Vec<String>,
    Vec<String>,
    bool,
    Option<ApiValue>,
    Option<ApiValue>,
    Option<ApiValue>,
    Option<ApiValue>,
);

/// The read switches shared by the single- and multi-prim query surfaces.
/// Keeping this as one native request type is important: a batch query must
/// use exactly the same composed-data contract as `QueryUsdPrim`, rather than
/// growing a second, subtly different inspection implementation.
#[derive(Clone, Default)]
struct UsdPrimQueryOptions {
    requested: Option<Vec<String>>,
    requested_relationships: Option<Vec<String>>,
    include_relationships: bool,
    include_connections: bool,
    include_schemas: bool,
    include_children: bool,
    include_collision_bounds: bool,
    include_collision_geometry: bool,
    include_geometry_bounds: bool,
    include_topology: bool,
}

fn query_options(params: &ApiValue) -> Result<UsdPrimQueryOptions, ApiQueryError> {
    let requested = optional_string_array(params, "attrs")?;
    let requested_relationships = optional_string_array(params, "rels")?;
    Ok(UsdPrimQueryOptions {
        requested,
        requested_relationships,
        include_relationships: optional_bool(params, "relationships")?,
        include_connections: optional_bool(params, "connections")?,
        include_schemas: optional_bool(params, "schemas")?,
        include_children: optional_bool(params, "children")?,
        include_collision_bounds: optional_bool(params, "collision_bounds")?,
        include_collision_geometry: optional_bool(params, "collision_geometry")?,
        include_geometry_bounds: optional_bool(params, "geometry_bounds")?,
        include_topology: optional_bool(params, "topology")?,
    })
}

fn optional_string_array(
    params: &ApiValue,
    name: &str,
) -> Result<Option<Vec<String>>, ApiQueryError> {
    match params.get(name) {
        None | Some(ApiValue::Unit) => Ok(None),
        Some(ApiValue::Array(values)) => values
            .iter()
            .map(|value| match value {
                ApiValue::Str(value) => Ok(value.clone()),
                _ => Err(ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    format!("QueryUsdPrim: `{name}` must contain only strings"),
                )),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(_) => Err(ApiQueryError::new(
            ApiErrorCode::DeserializationError,
            format!("QueryUsdPrim: `{name}` must be an array of strings"),
        )),
    }
}

fn optional_bool(params: &ApiValue, name: &str) -> Result<bool, ApiQueryError> {
    match params.get(name) {
        None | Some(ApiValue::Unit) => Ok(false),
        Some(_) => api_param_bool(params, name).ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!("QueryUsdPrim: `{name}` must be a boolean"),
            )
        }),
    }
}

fn query_document_id(
    world: &World,
    params: &ApiValue,
) -> Result<(Option<DocumentId>, Option<u64>), ApiQueryError> {
    if params.get("doc_id").is_none() {
        return Ok((None, None));
    }
    let Some(raw) = api_param_u64(params, "doc_id") else {
        return Err(ApiQueryError::new(
            ApiErrorCode::DeserializationError,
            "QueryUsdPrim: doc_id must be an explicit numeric document id",
        ));
    };
    let doc = DocumentId::new(raw);
    let Some(host) = world
        .get_resource::<DocumentRegistry<UsdDocument>>()
        .and_then(|registry| registry.host(doc))
    else {
        return Err(ApiQueryError::new(
            ApiErrorCode::EntityNotFound,
            format!("QueryUsdPrim: document {doc} is not open"),
        ));
    };
    let generation = host.document().generation();
    if let Some(synced_generation) = world
        .get_resource::<DocBackedTwinScenes>()
        .and_then(|scenes| scenes.synced_generation(doc))
    {
        if synced_generation != generation {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                format!("QueryUsdPrim: document {doc} projection is not current"),
            ));
        }
    }
    Ok((Some(doc), Some(generation)))
}

fn spawned_entities_for_paths(
    world: &World,
    paths: &HashSet<&str>,
    doc: Option<DocumentId>,
    live_stage: Option<bevy::asset::AssetId<UsdStageAsset>>,
) -> Result<HashMap<String, Entity>, ApiQueryError> {
    if doc.is_some() {
        return Ok(HashMap::new());
    }
    let Some(mut query) = QueryState::<(Entity, &UsdPrimPath)>::try_new(world) else {
        return Err(ApiQueryError::new(
            ApiErrorCode::InternalError,
            "QueryUsdPrim: USD entity query is unavailable",
        ));
    };
    let mut candidates = query
        .iter(world)
        .filter(|(entity, prim)| {
            Some(prim.stage_handle.id()) == live_stage
                && paths.contains(prim.path.as_str())
                && !lunco_usd_bevy_scene::is_preview_only_entity(world, *entity)
        })
        .map(|(entity, prim)| (prim.path.clone(), entity))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.to_bits().cmp(&right.1.to_bits()))
    });
    let mut spawned = HashMap::new();
    for (path, entity) in candidates {
        if spawned.insert(path.clone(), entity).is_some() {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                format!("QueryUsdPrim: multiple live entities are bound to prim `{path}`"),
            ));
        }
    }
    Ok(spawned)
}

fn query_record_value(
    world: &World,
    path: &str,
    read: PrimRead,
    doc: Option<DocumentId>,
    generation: Option<u64>,
    live_document: Option<DocumentId>,
    spawned: Option<Entity>,
    options: &UsdPrimQueryOptions,
    poses: &mut Option<lunco_physics::SimulationPoseReadState>,
) -> Result<ApiValue, ApiQueryError> {
    let (
        authored_position,
        type_name,
        attrs,
        relationships,
        connections,
        schemas,
        children,
        active,
        collision_bounds,
        collision_geometry,
        geometry_bounds,
        topology,
    ) = read;

    // Document placement is authored; live entity poses use the active
    // physics frame shared with `QueryEntity`.
    let mut out = vec![
        ("path".to_string(), ApiValue::str(path)),
        ("type_name".to_string(), ApiValue::str(type_name)),
        ("attrs".to_string(), ApiValue::Map(attrs)),
        ("spawned".to_string(), ApiValue::Bool(spawned.is_some())),
        ("active".to_string(), ApiValue::Bool(active)),
    ];
    if let Some(doc) = doc {
        out.push(("doc_id".to_string(), api_value!(doc.raw())));
        out.push(("generation".to_string(), api_value!(generation)));
        if let Some(position) = authored_position {
            out.push((
                "world_position".to_string(),
                api_value!([position.x, position.y, position.z]),
            ));
            out.push((
                "position_frame".to_string(),
                ApiValue::str("canonical_stage"),
            ));
        }
    } else if let Some(doc) = live_document {
        out.push(("doc_id".to_string(), api_value!(doc.raw())));
    }
    if options.requested_relationships.is_some() || options.include_relationships {
        out.push(("relationships".to_string(), ApiValue::Map(relationships)));
    }
    if options.include_connections {
        out.push(("connections".to_string(), ApiValue::Map(connections)));
    }
    if options.include_schemas {
        out.push(("api_schemas".to_string(), api_value!(schemas)));
    }
    if options.include_children {
        out.push(("children".to_string(), api_value!(children)));
    }
    if options.include_collision_bounds {
        out.push((
            "collision_bounds".to_string(),
            collision_bounds.unwrap_or(ApiValue::Unit),
        ));
    }
    if options.include_collision_geometry {
        out.push((
            "collision_geometry".to_string(),
            collision_geometry.unwrap_or(ApiValue::Unit),
        ));
    }
    if options.include_geometry_bounds {
        out.push((
            "geometry_bounds".to_string(),
            geometry_bounds.unwrap_or(ApiValue::Unit),
        ));
    }
    if options.include_topology {
        let mut topology = topology.unwrap_or(ApiValue::Unit);
        let ApiValue::Map(object) = &mut topology else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "QueryUsdPrim: topology projection is not a map",
            ));
        };
        object.push((
            "projection".to_string(),
            api_value!({
                "source": if doc.is_some() { "document" } else { "live_stage" },
                "composed": true,
                "document_generation": generation,
                "projected_generation": generation,
            }),
        ));
        object.push((
            "binding".to_string(),
            runtime_binding_api_value(world, spawned),
        ));
        out.push(("topology".to_string(), topology));
    }

    if let Some(entity) = spawned {
        let Some(poses) = poses.as_mut() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "QueryUsdPrim: active physics frame is unavailable",
            ));
        };
        let Some(pos) = poses.position(world, entity) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                format!(
                    "QueryUsdPrim: spawned prim `{path}` is disconnected from the active physics frame"
                ),
            ));
        };
        out.push((
            "world_position".to_string(),
            api_value!([pos.0.x, pos.0.y, pos.0.z]),
        ));
        out.push((
            "position_frame".to_string(),
            ApiValue::str("active_physics"),
        ));
    }

    Ok(ApiValue::Map(out))
}

/// Execute one or more prim reads against one validated stage/document
/// context. The stage is borrowed once for the entire batch, so component
/// verification can ask for hundreds of paths without reopening the composed
/// stage or repeating projection/generation checks for every path.
fn execute_query_paths(
    world: &World,
    params: &ApiValue,
    paths: &[(String, SdfPath)],
) -> Result<Vec<ApiValue>, ApiQueryError> {
    let options = query_options(params)?;
    let (doc, generation) = query_document_id(world, params)?;

    // Unscoped queries belong to the live simulation root. Preview stages and
    // detached cached stages cannot satisfy a live-scene query.
    let Some(mut live_roots) =
        QueryState::<(Entity, &UsdPrimPath), With<UsdSceneRoot>>::try_new(world)
    else {
        return Err(ApiQueryError::new(
            ApiErrorCode::InternalError,
            "QueryUsdPrim: live scene ownership is unavailable",
        ));
    };
    let mut live_stages = live_roots
        .iter(world)
        .filter(|(entity, _)| !lunco_usd_bevy_scene::is_preview_only_entity(world, *entity))
        .map(|(_, path)| path.stage_handle.id())
        .collect::<HashSet<_>>();
    if doc.is_none() && live_stages.len() != 1 {
        return Err(ApiQueryError::new(
            ApiErrorCode::InternalError,
            "QueryUsdPrim: exactly one mounted live stage is required; pass doc_id for an Editor document",
        ));
    }
    let live_stage = live_stages.drain().next();
    let live_document = if doc.is_none() {
        live_stage.and_then(|stage| {
            let backed = world.get_resource::<DocBackedTwinScenes>()?;
            let asset_server = world.get_resource::<AssetServer>()?;
            scene_document_for(backed, asset_server, stage)
        })
    } else {
        None
    };

    let requested_paths = paths
        .iter()
        .map(|(path, _)| path.as_str())
        .collect::<HashSet<_>>();
    let spawned = spawned_entities_for_paths(world, &requested_paths, doc, live_stage)?;
    let mut poses = if spawned.is_empty() {
        None
    } else {
        Some(
            lunco_physics::SimulationPoseReadState::try_new(world).ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::InternalError,
                    "QueryUsdPrim: active physics frame is unavailable",
                )
            })?,
        )
    };

    let mut read_paths = |view: &StageView<'_>| -> Result<Vec<ApiValue>, ApiQueryError> {
        paths
            .iter()
            .map(|(path, prim)| {
                let read = read_prim_from_view(
                    view,
                    prim,
                    path,
                    &options.requested,
                    &options.requested_relationships,
                    options.include_relationships,
                    options.include_connections,
                    options.include_schemas,
                    options.include_children,
                    options.include_collision_bounds,
                    options.include_collision_geometry,
                    options.include_geometry_bounds,
                    options.include_topology,
                    doc,
                )
                .map_err(|error| ApiQueryError::new(ApiErrorCode::InternalError, error))?
                .ok_or_else(|| {
                    ApiQueryError::new(
                        ApiErrorCode::EntityNotFound,
                        format!(
                            "QueryUsdPrim: prim `{path}` not found in the requested document or live stage"
                        ),
                    )
                })?;
                query_record_value(
                    world,
                    path,
                    read,
                    doc,
                    generation,
                    live_document,
                    spawned.get(path).copied(),
                    &options,
                    &mut poses,
                )
            })
            .collect()
    };

    // Read everything under one short canonical-stage borrow. An Editor fork
    // is not a Twin and therefore has no canonical mapping; its composed
    // document is opened once as the explicit fallback stage owner.
    if let Some(stage) = world
        .get_non_send::<CanonicalStages>()
        .and_then(|stages| match doc {
            Some(doc) => canonical_stage_for_document(world, doc),
            None => live_stage.and_then(|id| stages.get(id)),
        })
    {
        return read_paths(&stage.view());
    }

    let Some(doc) = doc else {
        return Err(ApiQueryError::new(
            ApiErrorCode::InternalError,
            "QueryUsdPrim: no USD stage loaded",
        ));
    };
    let Some(document) = world
        .get_resource::<DocumentRegistry<UsdDocument>>()
        .and_then(|registry| registry.host(doc))
        .map(|host| host.document())
    else {
        return Err(ApiQueryError::new(
            ApiErrorCode::EntityNotFound,
            format!("QueryUsdPrim: document {doc} is not open"),
        ));
    };
    let stage = open_doc_stage(document.composed_arc().as_ref()).map_err(|error| {
        ApiQueryError::new(
            ApiErrorCode::InternalError,
            format!("QueryUsdPrim: document stage could not be opened: {error}"),
        )
    })?;
    read_paths(&StageView::new(&stage))
}

fn read_prim_from_view(
    view: &StageView<'_>,
    prim: &SdfPath,
    path: &str,
    requested: &Option<Vec<String>>,
    requested_relationships: &Option<Vec<String>>,
    include_relationships: bool,
    include_connections: bool,
    include_schemas: bool,
    include_children: bool,
    include_collision_bounds: bool,
    include_collision_geometry: bool,
    include_geometry_bounds: bool,
    include_topology: bool,
    doc: Option<DocumentId>,
) -> Result<Option<PrimRead>, String> {
    if !view.has_prim(prim) {
        return Ok(None);
    }

    let authored_position = if doc.is_some() {
        Some(
            lunco_usd_bevy_stage::world_transform(view, prim)
                .map_err(|error| format!("QueryUsdPrim: invalid authored transform: {error}"))?
                .translation,
        )
    } else {
        None
    };

    let geometry_bounds = if include_geometry_bounds {
        match prim_geometry_aabb(view, path) {
            Ok(Some(aabb)) => Some(aabb_api_value(aabb)),
            Ok(None) => Some(ApiValue::Unit),
            Err(error) => {
                return Err(format!(
                    "QueryUsdPrim: invalid geometry bounds at `{path}`: {error}"
                ));
            }
        }
    } else {
        None
    };

    let collision_bounds = if include_collision_bounds {
        match collision_aabb(view, path) {
            Ok(Some(aabb)) => Some(api_value!({
                "min": api_value!([aabb.min.x, aabb.min.y, aabb.min.z]),
                "max": api_value!([aabb.max.x, aabb.max.y, aabb.max.z]),
                "center": api_value!([
                    (aabb.min.x + aabb.max.x) * 0.5,
                    (aabb.min.y + aabb.max.y) * 0.5,
                    (aabb.min.z + aabb.max.z) * 0.5,
                ]),
                "half_extents": api_value!([
                    (aabb.max.x - aabb.min.x) * 0.5,
                    (aabb.max.y - aabb.min.y) * 0.5,
                    (aabb.max.z - aabb.min.z) * 0.5,
                ]),
                "frame": "canonical_stage",
                "rest_depth": aabb.rest_depth(),
            })),
            Ok(None) => Some(ApiValue::Unit),
            Err(error) => {
                return Err(format!(
                    "QueryUsdPrim: invalid collision bounds at `{path}`: {error}"
                ));
            }
        }
    } else {
        None
    };

    let collision_geometry = if include_collision_geometry {
        match prim_collision_geometry(view, path) {
            Ok(Some(geometry)) => {
                let vertices = geometry
                    .vertices
                    .iter()
                    .map(|point| api_value!([point[0], point[1], point[2]]))
                    .collect::<Vec<_>>();
                Some(api_value!({
                    "type_name": geometry.type_name,
                    "approximation": geometry.approximation,
                    "vertices": ApiValue::Array(vertices),
                    "frame": "canonical_stage",
                }))
            }
            Ok(None) => Some(ApiValue::Unit),
            Err(error) => {
                return Err(format!(
                    "QueryUsdPrim: invalid collision geometry at `{path}`: {error}"
                ));
            }
        }
    } else {
        None
    };

    let type_name = view.type_name(prim).unwrap_or_default();
    let names = requested.clone().unwrap_or_else(|| view.attr_names(prim));
    let mut attrs = Vec::new();
    for name in names {
        attrs.push((name.clone(), attr_api_value(view, prim, &name)));
    }

    let mut relationships = Vec::new();
    let relationship_names = if include_relationships {
        view.relationship_names(prim)
    } else {
        requested_relationships.clone().unwrap_or_default()
    };
    if requested_relationships.is_some() || include_relationships {
        for name in relationship_names {
            let targets = view
                .rel_targets(prim, &name)
                .into_iter()
                .map(|path| path.as_str().to_string())
                .collect::<Vec<_>>();
            relationships.push((name, api_value!(targets)));
        }
    }

    let mut connections = Vec::new();
    if include_connections {
        for name in view.attr_names(prim) {
            let sources = view.connections(prim, &name);
            if !sources.is_empty() {
                connections.push((name, api_value!(sources)));
            }
        }
    }

    let schemas = if include_schemas {
        view.api_schemas(prim)
    } else {
        Vec::new()
    };
    let children = if include_children {
        view.children(prim)
            .into_iter()
            .map(|path| path.as_str().to_string())
            .collect()
    } else {
        Vec::new()
    };
    let topology = include_topology.then(|| topology_for_stage(view, prim));

    Ok(Some((
        authored_position,
        type_name,
        attrs,
        relationships,
        connections,
        schemas,
        children,
        view.is_active(prim),
        collision_bounds,
        collision_geometry,
        geometry_bounds,
        topology,
    )))
}

/// `QueryUsdPrim { doc_id?, path, attrs?, rels?, children?, collision_bounds?, collision_geometry?, geometry_bounds?, topology? }`
/// → composed attributes, active state, requested relationships, optional
/// direct children, optional aggregate collision bounds, optional scoped
/// topology facts, and world pose.
pub struct QueryUsdPrimProvider;

impl ApiQueryProvider for QueryUsdPrimProvider {
    fn name(&self) -> &'static str {
        "QueryUsdPrim"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(path) = api_param_str(params, "path") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "QueryUsdPrim: `path` (USD prim path) required",
            ));
        };
        let Ok(prim) = SdfPath::new(path) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!("QueryUsdPrim: `{path}` is not a valid USD prim path"),
            ));
        };
        let records = match execute_query_paths(world, params, &[(path.to_string(), prim)]) {
            Ok(records) => records,
            Err(error) => return Err(error),
        };
        let Some(record) = records.into_iter().next() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "QueryUsdPrim: one validated path produced no record",
            ));
        };
        Ok(Some(record))
    }
}

/// `QueryUsdPrims { doc_id?, paths, attrs?, rels?, children?, collision_bounds?, collision_geometry?, geometry_bounds?, topology? }`
/// → the same records as `QueryUsdPrim`, read from one composed-stage snapshot.
///
/// This is the preferred surface for authored verification and Editor
/// inspection. It is intentionally strict: paths are validated up front and
/// one missing or stale path fails the whole request rather than returning a
/// misleading partial verdict.
pub struct QueryUsdPrimsProvider;

impl ApiQueryProvider for QueryUsdPrimsProvider {
    fn name(&self) -> &'static str {
        "QueryUsdPrims"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(values) = api_param_array(params, "paths") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "QueryUsdPrims: `paths` (array of USD prim paths) required",
            ));
        };
        if values.is_empty() {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "QueryUsdPrims: `paths` must contain at least one USD prim path",
            ));
        }

        let mut paths = Vec::with_capacity(values.len());
        for value in values {
            let Some(path) = value.as_str() else {
                return Err(ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "QueryUsdPrims: every entry in `paths` must be a string",
                ));
            };
            let Ok(prim) = SdfPath::new(path) else {
                return Err(ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    format!("QueryUsdPrims: `{path}` is not a valid USD prim path"),
                ));
            };
            paths.push((path.to_string(), prim));
        }

        let records = match execute_query_paths(world, params, &paths) {
            Ok(records) => records,
            Err(error) => return Err(error),
        };
        Ok(Some(api_value!({
            "records": records,
            "count": paths.len(),
        })))
    }
}

/// Register the provider. Called by `SpawnCommandPlugin` beside
/// [`QueryEntity`](crate::entity_query::register), so any binary with the scene
/// verbs also answers `QueryUsdPrim` — including the headless server, which is
/// where asset-invariant checks want to run in CI.
pub fn register(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(QueryUsdPrimProvider);
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(QueryUsdPrimsProvider);
}
