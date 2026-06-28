//! Physics-backed spatial-query providers exposed to the API / scripting.
//!
//! Registered into [`ApiQueryRegistry`] so any caller — HTTP API, MCP, or a
//! rhai scenario via `query("Raycast", #{...})` — can sense geometry WITHOUT
//! taking an avian dependency. avian stays contained behind this string-keyed
//! provider seam (the read-side twin of the `#[Command]` bus).

use avian3d::prelude::*;
use bevy::ecs::system::SystemState;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::schema::{ApiErrorCode, ApiResponse};

/// Parse a `[x, y, z]` JSON array under `key`.
fn parse_vec3(params: &serde_json::Value, key: &str) -> Option<DVec3> {
    let a = params.get(key)?.as_array()?;
    if a.len() < 3 {
        return None;
    }
    Some(DVec3::new(a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?))
}

/// `Raycast` — cast a world-space ray, return the first collider hit.
/// params: `{ origin:[x,y,z], dir:[x,y,z], max?:f64 }` ·
/// returns: `{ hit, entity:gid|null, distance, point:[x,y,z], normal:[x,y,z] }`.
pub struct RaycastProvider;
impl ApiQueryProvider for RaycastProvider {
    fn name(&self) -> &'static str {
        "Raycast"
    }
    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let (Some(origin), Some(dir_v)) =
            (parse_vec3(params, "origin"), parse_vec3(params, "dir"))
        else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "Raycast: `origin` and `dir` [x,y,z] required".to_string(),
            );
        };
        let max = params
            .get("max")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0e6);
        let Ok(dir) = Dir3::new(dir_v.as_vec3()) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "Raycast: `dir` must be non-zero".to_string(),
            );
        };
        cast_ray_response(world, origin, dir, max)
    }
}

/// `GroundHeight` — collider/terrain height under a world `(x, z)` by casting
/// straight down. params: `{ x:f64, z:f64, from?:f64, max?:f64 }` ·
/// returns: `{ hit, height, entity:gid|null, normal:[x,y,z] }`.
pub struct GroundHeightProvider;
impl ApiQueryProvider for GroundHeightProvider {
    fn name(&self) -> &'static str {
        "GroundHeight"
    }
    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let (Some(x), Some(z)) = (
            params.get("x").and_then(serde_json::Value::as_f64),
            params.get("z").and_then(serde_json::Value::as_f64),
        ) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "GroundHeight: `x` and `z` required".to_string(),
            );
        };
        let from = params
            .get("from")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0e5);
        let max = params
            .get("max")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(2.0e5);
        let origin = DVec3::new(x, from, z);
        match cast_ray_response(world, origin, Dir3::NEG_Y, max) {
            ApiResponse::Ok {
                data: Some(d), ..
            } => {
                let hit = d.get("hit").and_then(serde_json::Value::as_bool).unwrap_or(false);
                let height = d
                    .get("point")
                    .and_then(|p| p.get(1))
                    .and_then(serde_json::Value::as_f64);
                ApiResponse::ok(serde_json::json!({
                    "hit": hit,
                    "height": height,
                    "entity": d.get("entity").cloned().unwrap_or(serde_json::Value::Null),
                    "normal": d.get("normal").cloned().unwrap_or(serde_json::Value::Null),
                }))
            }
            other => other,
        }
    }
}

/// Shared cast → JSON. Maps the hit collider back to its `GlobalEntityId` (null
/// when the collider has no registered id, e.g. unregistered terrain).
fn cast_ray_response(world: &mut World, origin: DVec3, dir: Dir3, max: f64) -> ApiResponse {
    let mut state: SystemState<(SpatialQuery, Res<ApiEntityRegistry>)> = SystemState::new(world);
    let (spatial, registry) = state.get(world);
    match spatial.cast_ray(origin, dir, max, true, &SpatialQueryFilter::default()) {
        Some(hit) => {
            let point = origin + (*dir).as_dvec3() * hit.distance;
            let entity = registry.api_id_for(hit.entity).map(|g| g.get());
            ApiResponse::ok(serde_json::json!({
                "hit": true,
                "entity": entity,
                "distance": hit.distance,
                "point": [point.x, point.y, point.z],
                "normal": [hit.normal.x, hit.normal.y, hit.normal.z],
            }))
        }
        None => ApiResponse::ok(serde_json::json!({ "hit": false })),
    }
}

/// Register the physics-backed spatial providers. Idempotent re: the registry
/// resource (init-if-absent), so plugin ordering vs. `LunCoApiPlugin` doesn't
/// matter.
pub fn register_physics_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let mut reg = app.world_mut().resource_mut::<ApiQueryRegistry>();
    reg.register(RaycastProvider);
    reg.register(GroundHeightProvider);
}
