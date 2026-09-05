//! `QueryUsdPrim` — read composed USD attributes and explicit relationships off
//! an explicit Editor document or the mounted live stage.
//!
//! ## Ownership
//!
//! An explicit `doc` resolves through the existing document-to-stage mapping
//! and requires its synchronized generation to match the open document. Without
//! `doc`, exactly one mounted live stage is required. Preview focus, duplicate
//! prim paths, and detached cached stages never choose the query target.
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
//!
//! ## Request
//!
//! ```json
//! {"type":"ExecuteCommand","command": "QueryUsdPrim", "params": {"path": "/Hab1/ShieldWall/OuterSurface"}}
//! {"type":"ExecuteCommand","command": "QueryUsdPrim", "params": {"path": "…", "attrs": ["radius", "points"]}}
//! {"type":"ExecuteCommand","command": "QueryUsdPrim", "params": {"path": "…", "rels": ["lunco:mount:attachmentJoint"]}}
//! ```
//!
//! Omitting `attrs` returns every authored attribute on the prim. Naming them is
//! much cheaper on a prim carrying big arrays (a trimmed `NurbsPatch` holds
//! thousands of control points), so a hot loop should name them.

use bevy::ecs::query::QueryState;
use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_doc::{Document, DocumentId};
use lunco_doc_bevy::DocumentRegistry;
use lunco_usd::{document::UsdDocument, twin_projection::DocBackedTwinScenes};
use lunco_usd_bevy::read::UsdRead;
use lunco_usd_bevy::view::StageView;
use lunco_usd_bevy::{CanonicalStages, UsdPrimPath, UsdSceneRoot};
use openusd::sdf::{Path as SdfPath, Value};

/// One attribute, converted to JSON by probing the typed readers in turn.
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
fn attr_json(view: &StageView<'_>, prim: &SdfPath, name: &str) -> serde_json::Value {
    use serde_json::json;

    // Scalars first — an array reader would answer `[]` for these, not `None`.
    if let Some(v) = view.real(prim, name) {
        return json!(v);
    }
    if let Some(v) = view.boolean(prim, name) {
        return json!(v);
    }
    if let Some(v) = view.scalar::<i32>(prim, name) {
        return json!(v);
    }
    if let Some(v) = view.text(prim, name) {
        return json!(v);
    }
    if let Some(v) = view.asset(prim, name) {
        return json!(v);
    }
    if let Some(q) = view.quat_d(prim, name) {
        return json!([q.w, q.x, q.y, q.z]);
    }

    // Arrays. `points3` before `reals` because a `point3f[]` also satisfies no
    // scalar reader and we want it shaped [[x,y,z], …], not flattened.
    let pts = view.points3(prim, name);
    if !pts.is_empty() {
        return json!(pts);
    }
    let reals = view.reals(prim, name);
    if !reals.is_empty() {
        return json!(reals);
    }
    let texts = view.texts(prim, name);
    if !texts.is_empty() {
        return json!(texts);
    }
    // Scalar vectors and integer arrays require the raw Value variants;
    // the typed readers above do not cover their shapes.
    match view.attr_value(prim, name) {
        // Scalar 2/3/4-vectors as flat JSON arrays — the same shape `points3`
        // gives each element of a `point3f[]`, so `v[1]` means "y" whether the
        // caller is reading one translate or one control point.
        Some(Value::Vec2f(v)) => json!([v.x, v.y]),
        Some(Value::Vec2d(v)) => json!([v.x, v.y]),
        Some(Value::Vec2i(v)) => json!([v.x, v.y]),
        Some(Value::Vec3f(v)) => json!([v.x, v.y, v.z]),
        Some(Value::Vec3d(v)) => json!([v.x, v.y, v.z]),
        Some(Value::Vec3i(v)) => json!([v.x, v.y, v.z]),
        Some(Value::Vec4f(v)) => json!([v.x, v.y, v.z, v.w]),
        Some(Value::Vec4d(v)) => json!([v.x, v.y, v.z, v.w]),
        Some(Value::Vec4i(v)) => json!([v.x, v.y, v.z, v.w]),

        Some(Value::IntVec(v)) if !v.is_empty() => json!(v),
        Some(Value::Int64Vec(v)) if !v.is_empty() => {
            json!(v.to_vec())
        }
        _ => serde_json::Value::Null,
    }
}

/// `QueryUsdPrim { doc?, path, attrs?, rels?, children? }` → composed attributes,
/// requested relationships, optional direct children, and world pose.
pub struct QueryUsdPrimProvider;

impl ApiQueryProvider for QueryUsdPrimProvider {
    fn name(&self) -> &'static str {
        "QueryUsdPrim"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(path) = params.get("path").and_then(serde_json::Value::as_str) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "QueryUsdPrim: `path` (USD prim path) required".to_string(),
            );
        };
        let Ok(prim) = SdfPath::new(path) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                format!("QueryUsdPrim: `{path}` is not a valid USD prim path"),
            );
        };

        let requested: Option<Vec<String>> = params
            .get("attrs")
            .and_then(serde_json::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            });
        let requested_relationships: Option<Vec<String>> = params
            .get("rels")
            .and_then(serde_json::Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect()
            });
        let include_children = params
            .get("children")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let doc = if params.get("doc").is_some() {
            let Some(raw) = params.get("doc").and_then(serde_json::Value::as_u64) else {
                return ApiResponse::error(
                    ApiErrorCode::DeserializationError,
                    "QueryUsdPrim: doc must be an explicit numeric document id",
                );
            };
            Some(DocumentId::new(raw))
        } else {
            None
        };
        let generation = if let Some(doc) = doc {
            let Some(host) = world
                .get_resource::<DocumentRegistry<UsdDocument>>()
                .and_then(|registry| registry.host(doc))
            else {
                return ApiResponse::error(
                    ApiErrorCode::EntityNotFound,
                    format!("QueryUsdPrim: document {doc} is not open"),
                );
            };
            let generation = host.document().generation();
            if world
                .get_resource::<DocBackedTwinScenes>()
                .and_then(|scenes| scenes.synced_generation(doc))
                != Some(generation)
            {
                return ApiResponse::error(
                    ApiErrorCode::InternalError,
                    format!("QueryUsdPrim: document {doc} projection is not current"),
                );
            }
            Some(generation)
        } else {
            None
        };

        // Unscoped queries belong to the live simulation root. Preview stages
        // and detached cached stages cannot satisfy a live-scene query.
        let Some(mut live_roots) = QueryState::<&UsdPrimPath, With<UsdSceneRoot>>::try_new(world)
        else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "QueryUsdPrim: live scene ownership is unavailable",
            );
        };
        let mut live_stages = live_roots
            .iter(world)
            .map(|p| p.stage_handle.id())
            .collect::<std::collections::HashSet<_>>();
        if doc.is_none() && live_stages.len() != 1 {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "QueryUsdPrim: exactly one mounted live stage is required; pass doc for an Editor document",
            );
        }
        let live_stage = live_stages.drain().next();

        let Some(mut spawned_query) = QueryState::<(Entity, &UsdPrimPath)>::try_new(world) else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "QueryUsdPrim: USD entity query is unavailable",
            );
        };
        let spawned: Option<(Entity, bevy::asset::AssetId<lunco_usd_bevy::UsdStageAsset>)> =
            spawned_query
                .iter(world)
                .find(|(entity, p)| {
                    doc.is_none()
                        && p.path == path
                        && Some(p.stage_handle.id()) == live_stage
                        && !lunco_usd_bevy::is_preview_only_entity(world, *entity)
                })
                .map(|(e, p)| (e, p.stage_handle.id()));

        // Read everything under ONE short borrow: `CanonicalStages` is `!Send`
        // and aliases the world, so it must be dropped before we touch entities.
        // (Same shape as `lunco_usd::live_consume`.)
        let mut authored_position = None;
        let read: Option<(
            String,
            serde_json::Map<String, serde_json::Value>,
            serde_json::Map<String, serde_json::Value>,
            Vec<String>,
        )> = {
            let Some(stages) = world.get_non_send::<CanonicalStages>() else {
                return ApiResponse::error(
                    ApiErrorCode::InternalError,
                    "QueryUsdPrim: no USD stage loaded".to_string(),
                );
            };

            let found = match doc {
                Some(doc) => lunco_usd::assembly_api::canonical_stage_for_document(world, doc),
                None => live_stage.and_then(|id| stages.get(id)),
            }
            .filter(|stage| stage.view().has_prim(&prim));

            if let Some(stage) = found.filter(|_| doc.is_some()) {
                authored_position = match lunco_usd_avian::world_transform(&stage.view(), &prim) {
                    Ok(transform) => Some(transform.translation),
                    Err(error) => {
                        return ApiResponse::error(
                            ApiErrorCode::InternalError,
                            format!("QueryUsdPrim: invalid authored transform: {error}"),
                        );
                    }
                };
            }

            found.map(|cs| {
                let view = cs.view();
                let type_name = view.type_name(&prim).unwrap_or_default();
                let names = requested.clone().unwrap_or_else(|| view.attr_names(&prim));
                let mut map = serde_json::Map::new();
                for n in names {
                    map.insert(n.clone(), attr_json(&view, &prim, &n));
                }
                let mut relationships = serde_json::Map::new();
                if let Some(names) = requested_relationships.clone() {
                    for name in names {
                        let targets = view
                            .rel_targets(&prim, &name)
                            .into_iter()
                            .map(|path| path.as_str().to_string())
                            .collect::<Vec<_>>();
                        relationships.insert(name, serde_json::json!(targets));
                    }
                }
                let children = if include_children {
                    view.children(&prim)
                        .into_iter()
                        .map(|path| path.as_str().to_string())
                        .collect()
                } else {
                    Vec::new()
                };
                (type_name, map, relationships, children)
            })
        };

        let Some((type_name, attrs, relationships, children)) = read else {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!(
                    "QueryUsdPrim: prim `{path}` not found in the requested document or live stage"
                ),
            );
        };

        // Document placement is authored; live entity poses use the active
        // physics frame shared with `QueryEntity`.
        let mut out = serde_json::json!({
            "path": path,
            "type_name": type_name,
            "attrs": attrs,
            "spawned": spawned.is_some(),
        });
        if let Some(doc) = doc {
            out["doc"] = serde_json::json!(doc);
            out["generation"] = serde_json::json!(generation);
            if let Some(position) = authored_position {
                out["world_position"] = serde_json::json!([position.x, position.y, position.z]);
                out["position_frame"] = serde_json::json!("canonical_stage");
            }
        }
        if requested_relationships.is_some() {
            out["relationships"] = serde_json::Value::Object(relationships);
        }
        if include_children {
            out["children"] = serde_json::json!(children);
        }

        if let Some((entity, _)) = spawned {
            let Some(mut poses) = lunco_physics::SimulationPoseReadState::try_new(world) else {
                return ApiResponse::error(
                    ApiErrorCode::InternalError,
                    "QueryUsdPrim: active physics frame is unavailable".to_string(),
                );
            };
            let Some(pos) = poses.position(world, entity) else {
                return ApiResponse::error(
                    ApiErrorCode::InternalError,
                    format!(
                        "QueryUsdPrim: spawned prim `{path}` is disconnected from the active physics frame"
                    ),
                );
            };
            out["world_position"] = serde_json::json!([pos.0.x, pos.0.y, pos.0.z]);
            out["position_frame"] = serde_json::json!("active_physics");
        }

        ApiResponse::ok(out)
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
}
