//! Terrain spatial-query providers — expose the analytic DEM height field to the
//! API / scripting surface as generic geometry queries:
//! `query("TerrainHeight", #{x, z})` and `query("TerrainRaycast", #{origin, ...})`.
//!
//! These are the read-side twin of the `#[Command]` bus, registered into
//! `lunco_api`'s [`ApiQueryRegistry`] the same way `lunco-mobility` registers its
//! physics-backed `Raycast`/`GroundHeight` providers. A rhai scenario reaches them
//! generically via `query("TerrainHeight", #{x: 12.0, z: -8.0})`; HTTP/MCP callers
//! via an `ExecuteCommand` named `TerrainHeight`.
//!
//! `TerrainRaycast` is deliberately **domain-free** (docs 10/11): it answers
//! "does relief block this ray?" over the retained height oracle — reusable by
//! AI pathing, sensors, spawn placement, camera, and (once antenna positions are
//! bridged from the solar frame) the comms terrain constraint. It marches the
//! same [`HeightSource`] the height query samples, so it is collider-independent
//! and works on the headless server, before any physics tile streams in.
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
use lunco_terrain_core::HeightSource;

use crate::oracle::SurfaceOracle;
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
        // read. `GlobalTransform` is Copy; the oracle is shared via `Arc`.
        let mut q = world.query::<(Entity, &GlobalTransform, &DemHeightField)>();
        let terrains: Vec<(Entity, GlobalTransform, Arc<SurfaceOracle>)> = q
            .iter(world)
            .map(|(e, gt, hf)| (e, *gt, hf.0.clone()))
            .collect();

        // First terrain whose footprint covers the point wins. Coordinates follow
        // the sibling providers' convention: the query `(x, z)` is in the same
        // frame as `GlobalTransform` (DEM terrain anchors at the origin cell, so
        // local ≈ world near the working area).
        for (entity, gt, oracle) in terrains {
            let inv = gt.affine().inverse();
            let local = inv.transform_point3(Vec3::new(x as f32, 0.0, z as f32));
            let half = oracle.half_extent();
            if local.x.abs() > half || local.z.abs() > half {
                continue;
            }

            let (lx, lz) = (local.x as f64, local.z as f64);
            let eps = eps_override.unwrap_or_else(|| oracle.spacing() as f64).max(1e-6);
            let h = HeightSource::height_at(oracle.as_ref(), lx, lz);
            let n = HeightSource::normal_at(oracle.as_ref(), lx, lz, eps);
            let slope = HeightSource::slope_at(oracle.as_ref(), lx, lz, eps);

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

/// Read a `[x,y,z]` array or `{x,y,z}` map into a [`Vec3`]. `None` if malformed.
fn parse_point(v: Option<&serde_json::Value>) -> Option<Vec3> {
    let v = v?;
    if let Some(arr) = v.as_array() {
        if arr.len() < 3 {
            return None;
        }
        return Some(Vec3::new(
            arr[0].as_f64()? as f32,
            arr[1].as_f64()? as f32,
            arr[2].as_f64()? as f32,
        ));
    }
    Some(Vec3::new(
        v.get("x")?.as_f64()? as f32,
        v.get("y")?.as_f64()? as f32,
        v.get("z")?.as_f64()? as f32,
    ))
}

/// `TerrainRaycast` — does terrain relief block a ray? Marches the DEM height
/// oracle in the terrain's local frame; generic geometry, no physics, no domain.
///
/// params: `{ origin:[x,y,z], target:[x,y,z] }` **or** `{ origin, dir:[x,y,z],
/// max?:f64 }`. `origin`/`target`/`dir` also accept `{x,y,z}` maps. `max`
/// defaults to 1e6 m when only a `dir` is given.
///
/// returns: `{ hit }`, plus `{ distance, point:[x,y,z], entity }` on a hit —
/// `distance` in metres along the ray, `point` the world-space intercept. A
/// small vertical margin keeps an endpoint sitting ON the surface from
/// occluding itself (mirrors `segment_hits_sphere`). `{ hit:false }` when the
/// ray clears all relief or never crosses a DEM footprint.
pub struct TerrainRaycastProvider;

impl ApiQueryProvider for TerrainRaycastProvider {
    fn name(&self) -> &'static str {
        "TerrainRaycast"
    }

    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let Some(origin) = parse_point(params.get("origin")) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "TerrainRaycast: `origin` [x,y,z] required".to_string(),
            );
        };
        // Direction is either implied by `target` (segment form, exact range) or
        // an explicit `dir` + `max` (ray form, for sensors/AI).
        let (dir, max) = if let Some(target) = parse_point(params.get("target")) {
            let d = target - origin;
            let len = d.length();
            if len < 1e-6 {
                return ApiResponse::ok(serde_json::json!({ "hit": false }));
            }
            (d / len, len)
        } else if let Some(dir) = parse_point(params.get("dir")) {
            let d = dir.normalize_or_zero();
            if d.length_squared() < 0.5 {
                return ApiResponse::error(
                    ApiErrorCode::DeserializationError,
                    "TerrainRaycast: `dir` must be non-zero".to_string(),
                );
            }
            let max = params.get("max").and_then(serde_json::Value::as_f64).unwrap_or(1.0e6) as f32;
            (d, max)
        } else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "TerrainRaycast: give `target` [x,y,z] or `dir` + `max`".to_string(),
            );
        };

        let mut q = world.query::<(Entity, &GlobalTransform, &DemHeightField)>();
        let terrains: Vec<(Entity, GlobalTransform, Arc<SurfaceOracle>)> = q
            .iter(world)
            .map(|(e, gt, hf)| (e, *gt, hf.0.clone()))
            .collect();

        // Nearest intercept across all DEM footprints wins. The march is the pure
        // `lunco_terrain_core::los_hit` kernel (the single-ray sibling of
        // `ao_map`); this provider only maps world↔terrain-local — exactly how
        // `TerrainHeightProvider` wraps `HeightSource::height_at`. The DEM anchors
        // at the origin cell (no scale), so a rigid local frame keeps `distance`
        // in honest metres.
        let mut best: Option<(f64, Vec3, Entity)> = None;
        for (entity, gt, oracle) in terrains {
            let inv = gt.affine().inverse();
            let o_local = inv.transform_point3(origin);
            let d_local = (inv.matrix3 * Vec3A::new(dir.x, dir.y, dir.z)).normalize_or_zero();
            if d_local.length_squared() < 0.5 {
                continue;
            }
            let hit = lunco_terrain_core::los_hit(
                oracle.as_ref(),
                [o_local.x as f64, o_local.y as f64, o_local.z as f64],
                [d_local.x as f64, d_local.y as f64, d_local.z as f64],
                max as f64,
                oracle.half_extent() as f64,
                oracle.spacing().max(0.5) as f64,
                0.05, // don't let a surface-sitting endpoint self-occlude
            );
            if let Some(t) = hit {
                if best.map_or(true, |(bt, _, _)| t < bt) {
                    let p_world = gt.transform_point(o_local + Vec3::from(d_local) * (t as f32));
                    best = Some((t, p_world, entity));
                }
            }
        }

        match best {
            Some((dist, p, entity)) => {
                let api_entity = world
                    .get_resource::<ApiEntityRegistry>()
                    .and_then(|reg| reg.api_id_for(entity))
                    .map(|g| g.get());
                ApiResponse::ok(serde_json::json!({
                    "hit": true,
                    "distance": dist,
                    "point": [p.x, p.y, p.z],
                    "entity": api_entity,
                }))
            }
            None => ApiResponse::ok(serde_json::json!({ "hit": false })),
        }
    }
}

/// Register the terrain query providers into the [`ApiQueryRegistry`]. Init-if-
/// absent so plugin ordering vs. `LunCoApiPlugin` doesn't matter (mirrors
/// `lunco_mobility::sensing::register_physics_queries`).
pub fn register_terrain_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let mut reg = app.world_mut().resource_mut::<ApiQueryRegistry>();
    reg.register(TerrainHeightProvider);
    reg.register(TerrainRaycastProvider);
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
        let grid = lunco_obstacle_field::field::HeightGrid { res: 3, half_extent: 10.0, heights };
        world
            .spawn((
                GlobalTransform::IDENTITY,
                DemHeightField(Arc::new(SurfaceOracle::bare(Arc::new(grid)))),
            ))
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

    // ── TerrainRaycast ───────────────────────────────────────────────────────
    // Reuses `tilted_terrain`: height = 0.1·x over x∈[−10,10], flat in z.

    #[test]
    fn raycast_ray_into_the_slope_hits() {
        let mut world = World::new();
        tilted_terrain(&mut world);
        // From (0, 2, 0) toward (10, 0.5, 0): y(x)=2−0.15x, terrain=0.1x; the ray
        // dips below the surface past x≈8 → a hit near there.
        let d = ok_data(TerrainRaycastProvider.execute(
            &mut world,
            &json!({ "origin": [0.0, 2.0, 0.0], "target": [10.0, 0.5, 0.0] }),
        ));
        assert_eq!(d["hit"], json!(true), "{d}");
        let p = d["point"].as_array().unwrap();
        assert!((p[0].as_f64().unwrap() - 8.0).abs() < 0.5, "intercept x {d}");
        let dist = d["distance"].as_f64().unwrap();
        assert!(dist > 6.0 && dist < 9.0, "distance {d}");
    }

    #[test]
    fn raycast_ray_above_relief_clears() {
        let mut world = World::new();
        tilted_terrain(&mut world);
        // Horizontal ray well above the highest terrain (max height 1.0 at x=10).
        let d = ok_data(TerrainRaycastProvider.execute(
            &mut world,
            &json!({ "origin": [-10.0, 100.0, 0.0], "dir": [1.0, 0.0, 0.0], "max": 20.0 }),
        ));
        assert_eq!(d["hit"], json!(false), "{d}");
    }

    #[test]
    fn raycast_outside_footprint_no_hit() {
        let mut world = World::new();
        tilted_terrain(&mut world);
        let d = ok_data(TerrainRaycastProvider.execute(
            &mut world,
            &json!({ "origin": [200.0, 5.0, 0.0], "target": [210.0, 5.0, 0.0] }),
        ));
        assert_eq!(d["hit"], json!(false), "{d}");
    }

    #[test]
    fn raycast_accepts_object_points_and_needs_origin() {
        let mut world = World::new();
        tilted_terrain(&mut world);
        // {x,y,z} map form parses the same as the array form.
        let d = ok_data(TerrainRaycastProvider.execute(
            &mut world,
            &json!({ "origin": {"x": 0.0, "y": 2.0, "z": 0.0},
                     "target": {"x": 10.0, "y": 0.5, "z": 0.0} }),
        ));
        assert_eq!(d["hit"], json!(true), "{d}");
        // No origin → error.
        assert!(matches!(
            TerrainRaycastProvider.execute(&mut world, &json!({ "dir": [1.0, 0.0, 0.0] })),
            ApiResponse::Error { .. }
        ));
    }
}
