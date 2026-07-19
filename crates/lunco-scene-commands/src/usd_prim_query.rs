//! `QueryUsdPrim` — read composed USD attributes off the live stage.
//!
//! ## Why this exists
//!
//! Scripts could reach the *spawned result* of a USD prim (`find(path)` →
//! entity → `world_pos`/`get`) but never the **authored, composed** values behind
//! it. `param()` reads only the `lunco:params` namespace, so an arbitrary
//! attribute — `radius`, `lunco:wallThickness`, `points`, `uKnots` — was
//! unreadable from anything except Rust.
//!
//! That gap had a cost. HAB-1's components document their relationships to each
//! other in PROSE ("MUST equal shell_can's radius", "DERIVED from
//! lunco:wallThickness") and nothing checked them, because nothing outside Rust
//! *could*. They drifted: an `OuterSurface` picked up a stray
//! `xformOp:translate = (0, 3.6, 0)` and the habitat's outer shell floated 3.6 m
//! above its inner shell, visible from across the scene, while a green test
//! suite sat next to it. Asset invariants need to be checkable by the people
//! authoring assets, in the scripting language they already use.
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
//! point; an invariant check wants what the file says. `world_position` is
//! grid-absolute, matching [`QueryEntity`](crate::entity_query) and what
//! `MoveEntity` accepts, and is present only when the prim spawned an entity.
//!
//! ## Request
//!
//! ```json
//! {"command": "QueryUsdPrim", "params": {"path": "/Hab1/ShieldWall/OuterSurface"}}
//! {"command": "QueryUsdPrim", "params": {"path": "…", "attrs": ["radius", "points"]}}
//! ```
//!
//! Omitting `attrs` returns every authored attribute on the prim. Naming them is
//! much cheaper on a prim carrying big arrays (a trimmed `NurbsPatch` holds
//! thousands of control points), so a hot loop should name them.

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_usd_bevy::read::UsdRead;
use lunco_usd_bevy::view::StageView;
use lunco_usd_bevy::{CanonicalStages, UsdPrimPath};
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
    if let Some(v) = view.scalar::<bool>(prim, name) {
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
    // `int[]` last, matched on the raw `Value`: the fixed-array `TryFrom<Value>`
    // impls do not cover integer vectors, so `scalar::<Vec<i32>>` does not
    // compile, let alone read. Same direct match `read_int_array` in
    // lunco-usd-bevy uses (private there, so this restates it rather than
    // widening that crate's surface for one caller). Covers the `int[]` counts
    // a trimmed NurbsPatch carries: `trimCurve:counts`, `vertexCounts`, `orders`.
    match view.attr_value(prim, name) {
        Some(Value::IntVec(v)) if !v.is_empty() => json!(v),
        Some(Value::Int64Vec(v)) if !v.is_empty() => {
            json!(v.iter().map(|&x| x as i64).collect::<Vec<_>>())
        }
        _ => serde_json::Value::Null,
    }
}

/// `QueryUsdPrim { path, attrs? }` → composed attributes + world pose.
pub struct QueryUsdPrimProvider;

impl ApiQueryProvider for QueryUsdPrimProvider {
    fn name(&self) -> &'static str {
        "QueryUsdPrim"
    }

    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
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

        // Which stage? Prefer the one the prim actually spawned from — a session
        // can hold several (a twin plus referenced assets), and picking the wrong
        // one silently answers about a different prim of the same path.
        let spawned: Option<(Entity, bevy::asset::AssetId<lunco_usd_bevy::UsdStageAsset>)> = world
            .query::<(Entity, &UsdPrimPath)>()
            .iter(world)
            .find(|(_, p)| p.path == path)
            .map(|(e, p)| (e, p.stage_handle.id()));

        // Read everything under ONE short borrow: `CanonicalStages` is `!Send`
        // and aliases the world, so it must be dropped before we touch entities.
        // (Same shape as `lunco_usd::live_consume`.)
        let read: Option<(String, serde_json::Map<String, serde_json::Value>)> = {
            let Some(stages) = world.get_non_send::<CanonicalStages>() else {
                return ApiResponse::error(
                    ApiErrorCode::InternalError,
                    "QueryUsdPrim: no USD stage loaded".to_string(),
                );
            };

            // Named stage if the prim spawned; otherwise the first stage that
            // actually has this prim, so an unspawned prim (a `guide`, a
            // deactivated variant, a pure-data prim) is still queryable.
            let found = spawned
                .and_then(|(_, id)| stages.get(id))
                .filter(|cs| cs.view().type_name(&prim).is_some())
                .or_else(|| {
                    stages
                        .iter()
                        .map(|(_, cs)| cs)
                        .find(|cs| cs.view().type_name(&prim).is_some())
                });

            found.map(|cs| {
                let view = cs.view();
                let type_name = view.type_name(&prim).unwrap_or_default();
                let names = requested.clone().unwrap_or_else(|| view.attr_names(&prim));
                let mut map = serde_json::Map::new();
                for n in names {
                    map.insert(n.clone(), attr_json(&view, &prim, &n));
                }
                (type_name, map)
            })
        };

        let Some((type_name, attrs)) = read else {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("QueryUsdPrim: prim `{path}` not found on any loaded stage"),
            );
        };

        // Pose, only for prims that spawned. Grid-absolute, same contract as
        // `QueryEntity` — see this module's frame note.
        let mut out = serde_json::json!({
            "path": path,
            "type_name": type_name,
            "attrs": attrs,
            "spawned": spawned.is_some(),
        });

        if let Some((entity, _)) = spawned {
            use bevy::ecs::system::SystemState;
            use big_space::prelude::{CellCoord, Grid};
            let mut state: SystemState<(
                Query<&ChildOf>,
                Query<&Grid>,
                Query<(Option<&CellCoord>, &Transform)>,
            )> = SystemState::new(world);
            if let Ok((q_parents, q_grids, q_spatial)) = state.get(world) {
                let pos =
                    lunco_core::coords::grid_absolute(entity, &q_parents, &q_grids, &q_spatial)
                        .unwrap_or(bevy::math::DVec3::ZERO);
                out["world_position"] = serde_json::json!([pos.x, pos.y, pos.z]);
                out["position_frame"] = serde_json::json!("grid_absolute");
            }
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
