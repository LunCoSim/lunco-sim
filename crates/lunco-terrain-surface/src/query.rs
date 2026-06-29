//! Terrain spatial-query provider — exposes the analytic DEM height field to the
//! API / scripting surface as `query("TerrainHeight", #{x, z})`.
//!
//! This is the read-side twin of the `#[Command]` bus, registered into
//! `lunco_api`'s [`ApiQueryRegistry`] the same way `lunco-mobility` registers its
//! physics-backed `Raycast`/`GroundHeight` providers. A rhai scenario reaches it
//! generically via `query("TerrainHeight", #{x: 12.0, z: -8.0})`; HTTP/MCP callers
//! via an `ExecuteCommand` named `TerrainHeight`.
//!
//! **Why a dedicated provider instead of `GroundHeight`.** `GroundHeight` casts a
//! ray straight down against avian colliders — so it only answers once the
//! physics collider for that spot has been built/streamed, and it pays a raycast.
//! This provider reads the authoritative [`HeightSource`] (the retained DEM grid)
//! *directly*: it works before any collider streams in, returns the analytic
//! normal + slope in one call, and is avian-free. The terrain grid is a pure
//! function of position, so the answer is deterministic across peers.

use std::sync::Arc;

use bevy::math::{Vec3, Vec3A};
use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_obstacle_field::field::HeightGrid;
use lunco_terrain_core::HeightSource;

use crate::stream_viz::DemHeightField;

/// `TerrainHeight` — analytic elevation / normal / slope at a world `(x, z)`,
/// read straight from the DEM height field (no physics raycast).
///
/// params: `{ x: f64, z: f64, eps?: f64 }` — `eps` is the central-difference step
/// (metres) for the normal/slope; defaults to the DEM sample spacing.
///
/// returns: `{ found, height, normal:[x,y,z], slope, entity }` where `slope` is
/// the angle from vertical in radians and `entity` is the terrain's API id (or
/// `null` if unregistered). `{ found: false }` when no DEM terrain covers the
/// point.
pub struct TerrainHeightProvider;

impl ApiQueryProvider for TerrainHeightProvider {
    fn name(&self) -> &'static str {
        "TerrainHeight"
    }

    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let (Some(x), Some(z)) = (
            params.get("x").and_then(serde_json::Value::as_f64),
            params.get("z").and_then(serde_json::Value::as_f64),
        ) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "TerrainHeight: `x` and `z` required".to_string(),
            );
        };
        let eps_override = params.get("eps").and_then(serde_json::Value::as_f64);

        // Snapshot the DEM terrains, releasing the world borrow before the registry
        // read. `GlobalTransform` is Copy; the grid is shared via `Arc`.
        let mut q = world.query::<(Entity, &GlobalTransform, &DemHeightField)>();
        let terrains: Vec<(Entity, GlobalTransform, Arc<HeightGrid>)> = q
            .iter(world)
            .map(|(e, gt, hf)| (e, *gt, hf.0.clone()))
            .collect();

        // First terrain whose footprint covers the point wins. Coordinates follow
        // the sibling providers' convention: the query `(x, z)` is in the same
        // frame as `GlobalTransform` (DEM terrain anchors at the origin cell, so
        // local ≈ world near the working area).
        for (entity, gt, grid) in terrains {
            let inv = gt.affine().inverse();
            let local = inv.transform_point3(Vec3::new(x as f32, 0.0, z as f32));
            let half = grid.half_extent;
            if local.x.abs() > half || local.z.abs() > half {
                continue;
            }

            let (lx, lz) = (local.x as f64, local.z as f64);
            let eps = eps_override.unwrap_or_else(|| grid.spacing() as f64).max(1e-6);
            // Trait calls are fully-qualified: `HeightGrid` also has an inherent
            // f32 `height_at`, so the f64 `HeightSource` method needs UFCS.
            let h = HeightSource::height_at(grid.as_ref(), lx, lz);
            let n = HeightSource::normal_at(grid.as_ref(), lx, lz, eps);
            let slope = HeightSource::slope_at(grid.as_ref(), lx, lz, eps);

            // Local height → world Y, and local normal → world frame, through the
            // terrain transform (identity for an origin-anchored DEM, but correct
            // under translation/rotation too).
            let world_y = gt.transform_point(Vec3::new(local.x, h as f32, local.z)).y as f64;
            let wn = (gt.affine().matrix3 * Vec3A::new(n[0] as f32, n[1] as f32, n[2] as f32))
                .normalize_or_zero();

            let entity = world
                .get_resource::<ApiEntityRegistry>()
                .and_then(|reg| reg.api_id_for(entity))
                .map(|g| g.get());

            return ApiResponse::ok(serde_json::json!({
                "found": true,
                "height": world_y,
                "normal": [wn.x, wn.y, wn.z],
                "slope": slope,
                "entity": entity,
            }));
        }

        ApiResponse::ok(serde_json::json!({ "found": false }))
    }
}

/// Register the terrain query providers into the [`ApiQueryRegistry`]. Init-if-
/// absent so plugin ordering vs. `LunCoApiPlugin` doesn't matter (mirrors
/// `lunco_mobility::sensing::register_physics_queries`).
pub fn register_terrain_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(TerrainHeightProvider);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A 3×3 grid spanning ±10 m, tilted along +X so height = 0.1·x and the
    /// gradient (hence slope) is constant and known.
    fn tilted_terrain(world: &mut World) -> Entity {
        // sample x at ix 0,1,2 = -10, 0, 10 → height -1, 0, 1, every row.
        let heights = vec![-1.0, 0.0, 1.0, -1.0, 0.0, 1.0, -1.0, 0.0, 1.0];
        let grid = HeightGrid { res: 3, half_extent: 10.0, heights };
        world
            .spawn((GlobalTransform::IDENTITY, DemHeightField(Arc::new(grid))))
            .id()
    }

    fn ok_data(resp: ApiResponse) -> serde_json::Value {
        match resp {
            ApiResponse::Ok { data: Some(d), .. } => d,
            other => panic!("expected Ok with data, got {other:?}"),
        }
    }

    #[test]
    fn samples_height_and_slope_inside_footprint() {
        let mut world = World::new();
        tilted_terrain(&mut world);

        // Mid-slope: x=5 bilinearly between height(0)=0 and height(10)=1 → 0.5.
        // Small `eps` keeps the central difference inside the linear region (the
        // default eps = sample spacing = 10 m would clamp at the ±10 m edge).
        let d = ok_data(
            TerrainHeightProvider.execute(&mut world, &json!({"x": 5.0, "z": 0.0, "eps": 1.0})),
        );
        assert_eq!(d["found"], json!(true));
        assert!((d["height"].as_f64().unwrap() - 0.5).abs() < 1e-4, "height {d}");
        // slope = atan(0.1) ≈ 0.0997 rad from the constant 0.1 gradient.
        assert!((d["slope"].as_f64().unwrap() - 0.1f64.atan()).abs() < 1e-3, "slope {d}");
        // Up-normal tilts away from the climb (−x), still mostly +Y.
        let n = d["normal"].as_array().unwrap();
        assert!(n[0].as_f64().unwrap() < 0.0 && n[1].as_f64().unwrap() > 0.9);
    }

    #[test]
    fn reports_not_found_outside_footprint() {
        let mut world = World::new();
        tilted_terrain(&mut world);
        let d = ok_data(TerrainHeightProvider.execute(&mut world, &json!({"x": 100.0, "z": 0.0})));
        assert_eq!(d["found"], json!(false));
    }

    #[test]
    fn missing_params_error() {
        let mut world = World::new();
        let resp = TerrainHeightProvider.execute(&mut world, &json!({"x": 1.0}));
        assert!(matches!(resp, ApiResponse::Error { .. }));
    }
}
