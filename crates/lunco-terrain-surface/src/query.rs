//! Terrain spatial-query providers — expose the analytic DEM height field to the
//! API / scripting surface as generic geometry queries: `query("TerrainHeight",
//! #{x, z})` (one point), `query("TerrainField", #{field, x, z, half})` (a raster
//! region) and `query("TerrainRaycast", #{origin, ...})` (does relief block a ray).
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

use bevy::ecs::query::QueryState;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_api::queries::{
    api_param_f64, api_param_str, api_param_u64, ApiQueryError, ApiQueryProvider, ApiQueryRegistry,
    ApiQueryResult,
};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api_core::{api_value, ApiErrorCode, ApiValue};
use lunco_spatial::coords::GridPos;
use lunco_terrain_core::{
    field_map, normal_at_bounded, AspectField, BoundedHeightSource, ElevationField, SlopeField,
    Square, SurfaceField,
};

use crate::oracle::DemHeightField;
use crate::oracle::SurfaceOracle;
use crate::stream_viz::{TerrainDetailDemands, TerrainStreamStatus};

/// Largest raster a single `TerrainField` query may materialise per side (256×256 =
/// 65 k texels). A deliberate readback for a tool/analyst, not a streaming path — the
/// cap just bounds one response payload; larger coverage is a tiled/streamed concern.
const FIELD_MAX_RES: usize = 256;

/// Resolve a field id (the stable [`SurfaceField::id`]) to a boxed instance. The set
/// mirrors `lunco_terrain_core`'s geometric fields; a caller naming an unknown field
/// gets an error rather than a silent empty raster.
fn field_by_id(id: &str) -> Option<Box<dyn SurfaceField>> {
    match id {
        "slope" => Some(Box::new(SlopeField)),
        "aspect" => Some(Box::new(AspectField)),
        "elevation" => Some(Box::new(ElevationField)),
        _ => None,
    }
}

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

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let (Some(x), Some(z)) = (api_param_f64(params, "x"), api_param_f64(params, "z")) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "TerrainHeight: `x` and `z` required",
            ));
        };
        if !x.is_finite() || !z.is_finite() {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "TerrainHeight: `x` and `z` must be finite",
            ));
        }
        let eps_override = match params.get("eps") {
            None => None,
            Some(_) => Some(api_param_f64(params, "eps").ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "TerrainHeight: `eps` must be a number",
                )
            })?),
        };
        if eps_override.is_some_and(|eps| !eps.is_finite() || eps <= 0.0) {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "TerrainHeight: `eps` must be finite and positive",
            ));
        }
        let p = GridPos(DVec3::new(x, 0.0, z));

        // Snapshot the DEM terrains, releasing the world borrow before the
        // registry read. The oracle is shared via `Arc`.
        let Some(mut q) = QueryState::<(Entity, &DemHeightField)>::try_new(world) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "TerrainHeight: DEM query is unavailable",
            ));
        };
        let terrains: Vec<(Entity, Arc<SurfaceOracle>)> =
            q.iter(world).map(|(e, hf)| (e, hf.0.clone())).collect();

        // First terrain whose footprint covers the point wins. The DEM frame IS
        // the grid frame: the terrain entity is a grid-direct child at
        // `CellCoord::default()` (`terrain.rs`), so the query point — a
        // [`GridPos`], the same currency as avian `Position` and every other
        // port — samples the oracle DIRECTLY, in f64 (`.0` taken only at the
        // oracle boundary; the oracle itself is DEM-frame math). No
        // `GlobalTransform` belongs in between: a render GT is origin-relative,
        // and pushing grid-absolute coordinates through its f32 inverse silently
        // shifted the footprint test after a floating-origin XZ move (the same
        // frame bug the collider ring had — see the frame rule in
        // `collider_ring.rs`). Height comes back AS THE ORACLE GIVES IT — already
        // absolute body-datum metres (the DEM keeps the GeoTIFF's own values; see
        // `lunco-terrain-bake::dem`) — matching `world_pos`, the entity's own
        // `position_y` port, and the sibling `GroundHeight` raycast.
        for (entity, oracle) in terrains {
            // The footprint test + sample are `surface_query::height_in_footprint`
            // — one implementation shared with `GridSurfaceQuery`, so this
            // provider and the in-process placement path can never disagree about
            // which terrain covers a point.
            let Some(h) = crate::surface_query::height_in_footprint(oracle.as_ref(), p) else {
                continue;
            };

            let eps = eps_override
                .unwrap_or_else(|| oracle.spacing() as f64)
                .max(1e-6);
            let half = oracle.half_extent() as f64;
            let n = normal_at_bounded(oracle.as_ref(), p.0.x, p.0.z, eps, half);
            let slope = n[1].clamp(-1.0, 1.0).acos();

            let entity = world
                .get_resource::<ApiEntityRegistry>()
                .and_then(|reg| reg.api_id_for(entity))
                .map(|g| g.get());

            return Ok(Some(api_value!({
                "found": true,
                "height": h,
                "normal": api_value!([n[0], n[1], n[2]]),
                "slope": slope,
                "entity": entity,
            })));
        }

        Ok(Some(api_value!({ "found": false })))
    }
}

/// `TerrainField` — materialise a [`SurfaceField`] (slope / aspect / elevation) to a
/// row-major `res × res` raster over a world-space square, read straight from the DEM
/// height field. The **headless, tool-facing** twin of `TerrainHeight`: where that
/// answers one point, this answers a region, so a rover planner / GIS export / the
/// render VIEW all read the SAME derived numbers (fields are data; render is one
/// consumer — see `docs/architecture/terrain-layered-rendering.md`).
///
/// params: `{ field: "slope"|"aspect"|"elevation", x: f64, z: f64, half: f64,
/// res?: usize }` — `(x, z)` is the region centre (world), `half` its half side
/// (metres), `res` the per-side texel count (default 64, capped at
/// [`FIELD_MAX_RES`]). The finite-difference step is the field raster's own cell size
/// ([`field_map`]), matching the derived-map / tile-UV convention.
///
/// returns: `{ found, field, res, half, center:[x,z], min, max, data:[f32; res*res] }`
/// where `data` is row-major (row `iz` outer, `ix` inner), texel `(ix,iz)` sampled at
/// the texel centre. `{ found: false }` when the complete requested square is not
/// covered by one DEM. A finite footprint is required so a field never silently
/// turns an outside region into repeated edge terrain.
pub struct TerrainFieldProvider;

impl ApiQueryProvider for TerrainFieldProvider {
    fn name(&self) -> &'static str {
        "TerrainField"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let field_id = match params.get("field") {
            None => "slope",
            Some(_) => api_param_str(params, "field").ok_or_else(|| {
                ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "TerrainField: `field` must be a string",
                )
            })?,
        };
        let Some(field) = field_by_id(field_id) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                format!("TerrainField: unknown field `{field_id}` (slope|aspect|elevation)"),
            ));
        };
        let (Some(x), Some(z), Some(half)) = (
            api_param_f64(params, "x"),
            api_param_f64(params, "z"),
            api_param_f64(params, "half"),
        ) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "TerrainField: `x`, `z`, `half` required",
            ));
        };
        if half.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "TerrainField: `half` must be > 0",
            ));
        }
        if !x.is_finite() || !z.is_finite() || !half.is_finite() {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "TerrainField: `x`, `z`, and `half` must be finite",
            ));
        }
        let res = match params.get("res") {
            None => 64,
            Some(_) => api_param_u64(params, "res")
                .map(|r| r.clamp(1, FIELD_MAX_RES as u64) as usize)
                .ok_or_else(|| {
                    ApiQueryError::new(
                        ApiErrorCode::DeserializationError,
                        "TerrainField: `res` must be an unsigned integer",
                    )
                })?,
        };
        let center = GridPos(DVec3::new(x, 0.0, z));

        // Snapshot DEM terrains, releasing the world borrow (see `TerrainHeight`).
        let Some(mut q) = QueryState::<(Entity, &DemHeightField)>::try_new(world) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "TerrainField: DEM query is unavailable",
            ));
        };
        let terrains: Vec<Arc<SurfaceOracle>> = q.iter(world).map(|(_, hf)| hf.0.clone()).collect();

        // First terrain whose footprint covers the region centre wins. The DEM
        // frame IS the grid frame (see `TerrainHeight`), so the grid-absolute
        // centre addresses the oracle directly, in f64 (`.0` at the oracle
        // boundary — `Square` is DEM-frame math).
        for oracle in terrains {
            let hx = oracle.half_extent() as f64;
            if center.0.x.abs() + half > hx || center.0.z.abs() + half > hx {
                continue;
            }
            let region = Square {
                center: [center.0.x, center.0.z],
                half,
            };
            let bounded = BoundedHeightSource::new(oracle.as_ref(), hx);
            let data = field_map(field.as_ref(), &bounded, &region, res);
            let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
            for &v in &data {
                min = min.min(v);
                max = max.max(v);
            }
            return Ok(Some(api_value!({
                "found": true,
                "field": field_id,
                "res": res,
                "half": half,
                "center": api_value!([x, z]),
                "min": min,
                "max": max,
                "data": data,
            })));
        }

        Ok(Some(api_value!({ "found": false })))
    }
}

/// Read a `[x,y,z]` array or `{x,y,z}` map into a bare [`DVec3`] — for typed
/// values that are frame-free vectors (`dir`). `None` if malformed.
fn parse_vec3(v: Option<&ApiValue>) -> Option<DVec3> {
    let v = v?;
    if let ApiValue::Array(arr) = v {
        if arr.len() != 3 {
            return None;
        }
        let value = DVec3::new(arr[0].as_f64()?, arr[1].as_f64()?, arr[2].as_f64()?);
        return value.is_finite().then_some(value);
    }
    let value = DVec3::new(
        v.get("x")?.as_f64()?,
        v.get("y")?.as_f64()?,
        v.get("z")?.as_f64()?,
    );
    value.is_finite().then_some(value)
}

/// Read a typed API value as a GRID-ABSOLUTE point (`origin`/`target`).
fn parse_point(v: Option<&ApiValue>) -> Option<GridPos> {
    parse_vec3(v).map(GridPos)
}

/// `TerrainRaycast` — does terrain relief block a ray? Marches the DEM height
/// oracle in the grid-absolute frame; generic geometry, no physics, no domain.
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

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(origin) = parse_point(params.get("origin")) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "TerrainRaycast: `origin` [x,y,z] required",
            ));
        };
        if params.get("target").is_some()
            && (params.get("dir").is_some() || params.get("max").is_some())
        {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "TerrainRaycast: use either `target` or `dir` with optional `max`, not both forms",
            ));
        }
        // Direction is either implied by `target` (segment form, exact range) or
        // an explicit `dir` + `max` (ray form, for sensors/AI).
        let (dir, max) = if params.get("target").is_some() {
            let Some(target) = parse_point(params.get("target")) else {
                return Err(ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "TerrainRaycast: `target` must be a [x,y,z] point",
                ));
            };
            let d = target - origin;
            let len = d.length();
            if !len.is_finite() {
                return Err(ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "TerrainRaycast: `target` must be a finite distance from `origin`",
                ));
            }
            if len < 1e-6 {
                return Ok(Some(api_value!({ "hit": false })));
            }
            (d / len, len)
        } else if params.get("dir").is_some() {
            let Some(dir) = parse_vec3(params.get("dir")) else {
                return Err(ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "TerrainRaycast: `dir` must be a [x,y,z] vector",
                ));
            };
            let d = dir.normalize_or_zero();
            if d.length_squared() < 0.5 {
                return Err(ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "TerrainRaycast: `dir` must be non-zero",
                ));
            }
            let max = match params.get("max") {
                None => 1.0e6,
                Some(_) => api_param_f64(params, "max").ok_or_else(|| {
                    ApiQueryError::new(
                        ApiErrorCode::DeserializationError,
                        "TerrainRaycast: `max` must be a number",
                    )
                })?,
            };
            if !max.is_finite() || max <= 0.0 {
                return Err(ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "TerrainRaycast: `max` must be finite and positive",
                ));
            }
            (d, max)
        } else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "TerrainRaycast: give `target` [x,y,z] or `dir` + `max`",
            ));
        };

        let Some(mut q) = QueryState::<(Entity, &DemHeightField)>::try_new(world) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "TerrainRaycast: DEM query is unavailable",
            ));
        };
        let terrains: Vec<(Entity, Arc<SurfaceOracle>)> =
            q.iter(world).map(|(e, hf)| (e, hf.0.clone())).collect();

        // Nearest intercept across all DEM footprints wins. The march is the pure
        // `lunco_terrain_core::los_hit` kernel (the single-ray sibling of
        // `ao_map`); this provider only parses/reports — exactly how
        // `TerrainHeightProvider` wraps `HeightSource::height_at`. The DEM frame
        // IS the grid frame (see `TerrainHeight`), so the grid-absolute ray
        // marches the oracle directly, in f64 (`.0` at the `los_hit` boundary —
        // the kernel is DEM-frame math), and `distance` is honest metres.
        let mut best: Option<(f64, GridPos, Entity)> = None;
        for (entity, oracle) in terrains {
            let hit = lunco_terrain_core::los_hit(
                oracle.as_ref(),
                [origin.0.x, origin.0.y, origin.0.z],
                [dir.x, dir.y, dir.z],
                max,
                oracle.half_extent() as f64,
                oracle.spacing().max(0.5) as f64,
                0.05, // don't let a surface-sitting endpoint self-occlude
            );
            if let Some(t) = hit {
                if best.is_none_or(|(bt, _, _)| t < bt) {
                    best = Some((t, origin + dir * t, entity));
                }
            }
        }

        match best {
            Some((dist, p, entity)) => {
                let api_entity = world
                    .get_resource::<ApiEntityRegistry>()
                    .and_then(|reg| reg.api_id_for(entity))
                    .map(|g| g.get());
                Ok(Some(api_value!({
                    "hit": true,
                    "distance": dist,
                    "point": api_value!([p.0.x, p.0.y, p.0.z]),
                    "entity": api_entity,
                })))
            }
            None => Ok(Some(api_value!({ "hit": false }))),
        }
    }
}

/// `TerrainLodStatus` — the live inputs and fulfilment state of the visual
/// terrain streamer. It reports the already-composed camera demands and the
/// selected-cover progress; it does not calculate coordinates or change LOD.
///
/// This makes a visual LOD report testable through the same API session that
/// renders it. In particular, a mounted avatar camera must appear here at its
/// full grid-absolute pose, never at its rig-local transform offset.
/// `stream.pending` includes both off-thread bakes and mesh entities waiting for
/// `ShaderLookReady` (including the root fallback), so a zero pending count
/// means the required work has crossed the render-resource boundary rather than
/// merely leaving the worker queue.
pub struct TerrainLodStatusProvider;

impl ApiQueryProvider for TerrainLodStatusProvider {
    fn name(&self) -> &'static str {
        "TerrainLodStatus"
    }

    fn execute(&self, world: &World, _params: &ApiValue) -> ApiQueryResult {
        let Some(status) = world.get_resource::<TerrainStreamStatus>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "TerrainLodStatus: terrain streaming is unavailable",
            ));
        };
        let Some(settings) = world.get_resource::<lunco_render::RenderingQualitySettings>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "TerrainLodStatus: rendering quality settings are unavailable",
            ));
        };
        let profile = match settings.validated_profile() {
            Ok(profile) => profile,
            Err(reason) => {
                return Err(ApiQueryError::new(
                    ApiErrorCode::InternalError,
                    format!("TerrainLodStatus: rendering quality settings are invalid: {reason}"),
                ));
            }
        };
        let Some(demands) = world.get_resource::<TerrainDetailDemands>() else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "TerrainLodStatus: terrain detail demands are unavailable",
            ));
        };
        let visual_foci = demands
            .visual_focus_snapshot()
            .into_iter()
            .map(|(entity, position, forward, screen_height_px, fov_y_rad)| {
                api_value!({
                    "entity": entity,
                    "position": position,
                    "forward": forward,
                    "screen_height_px": screen_height_px,
                    "fov_y_rad": fov_y_rad,
                })
            })
            .collect::<Vec<_>>();
        Ok(Some(api_value!({
            "config": {
                "pixel_error": profile.terrain_lod_pixel_error,
                "max_depth": profile.terrain_lod_max_depth,
                "bakes_per_frame": profile.terrain_lod_bakes_per_frame,
                "max_inflight_bakes": profile.terrain_lod_max_inflight_bakes,
                "tile_budget": profile.terrain_lod_tile_budget,
                "cover_edits_per_frame": profile.terrain_lod_cover_edits_per_frame,
                "hysteresis_ratio": profile.terrain_lod_hysteresis_ratio,
                "morph_start_ratio": profile.terrain_lod_morph_start_ratio,
                "tile_resolution": profile.terrain_lod_tile_resolution,
                "cinematic_resolution": profile.terrain_lod_cinematic_resolution,
                "probe_resolution": profile.terrain_lod_probe_resolution,
            },
            "stream": {
                "wanted": status.wanted,
                "resident": status.resident,
                "pending": status.pending,
                "stale_cancelled": status.stale_cancelled,
                "budget_refused": status.budget_refused,
                "focus_wanted": status.focus_wanted,
                "focus_resident": status.focus_resident,
            },
            "viewport_camera": world
                .get_resource::<lunco_viewport_core::SceneViewport>()
                .and_then(|viewport| viewport.active_camera)
                .map(|entity| entity.to_bits()),
            "visual_foci": visual_foci,
        })))
    }
}

/// Register the terrain query providers into the [`ApiQueryRegistry`]. Init-if-
/// absent so plugin ordering vs. `LunCoApiPlugin` doesn't matter (mirrors
/// `lunco_mobility::sensing::register_physics_queries`).
pub fn register_terrain_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let mut reg = app.world_mut().resource_mut::<ApiQueryRegistry>();
    reg.register(TerrainHeightProvider);
    reg.register(TerrainFieldProvider);
    reg.register(TerrainRaycastProvider);
    reg.register(TerrainLodStatusProvider);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3×3 grid spanning ±10 m, tilted along +X so height = 0.1·x and the
    /// gradient (hence slope) is constant and known.
    fn tilted_terrain(world: &mut World) -> Entity {
        // sample x at ix 0,1,2 = -10, 0, 10 → height -1, 0, 1, every row.
        let heights = vec![-1.0, 0.0, 1.0, -1.0, 0.0, 1.0, -1.0, 0.0, 1.0];
        let grid = lunco_obstacle_field::field::HeightGrid {
            res: 3,
            half_extent: 10.0,
            heights,
        };
        world
            .spawn(DemHeightField(Arc::new(SurfaceOracle::bare(Arc::new(
                grid,
            )))))
            .id()
    }

    fn ok_data(result: ApiQueryResult) -> ApiValue {
        result.expect("query succeeds").expect("query returns data")
    }

    fn field<'a>(value: &'a ApiValue, name: &str) -> &'a ApiValue {
        value
            .get(name)
            .unwrap_or_else(|| panic!("missing field `{name}`"))
    }

    fn array(value: &ApiValue) -> &[ApiValue] {
        let ApiValue::Array(values) = value else {
            panic!("expected array, got {value:?}");
        };
        values
    }

    fn number(value: &ApiValue) -> f64 {
        value
            .as_f64()
            .unwrap_or_else(|| panic!("expected number, got {value:?}"))
    }

    #[test]
    fn samples_height_and_slope_inside_footprint() {
        let mut world = World::new();
        tilted_terrain(&mut world);

        // Mid-slope: x=5 bilinearly between height(0)=0 and height(10)=1 → 0.5.
        // Small `eps` keeps the central difference inside the linear region (the
        // default eps = sample spacing = 10 m would clamp at the ±10 m edge).
        let d = ok_data(
            TerrainHeightProvider.execute(&world, &api_value!({"x": 5.0, "z": 0.0, "eps": 1.0})),
        );
        assert_eq!(field(&d, "found").as_bool(), Some(true));
        assert!(
            (number(field(&d, "height")) - 0.5).abs() < 1e-4,
            "height {d:?}"
        );
        // slope = atan(0.1) ≈ 0.0997 rad from the constant 0.1 gradient.
        assert!(
            (number(field(&d, "slope")) - 0.1f64.atan()).abs() < 1e-3,
            "slope {d:?}"
        );
        // Up-normal tilts away from the climb (−x), still mostly +Y.
        let n = array(field(&d, "normal"));
        assert!(number(&n[0]) < 0.0 && number(&n[1]) > 0.9);
    }

    /// **The query must answer in the grid frame, not the render frame.**
    /// Regression for the collider-ring class of bug: the footprint test used to
    /// go through `GlobalTransform.affine().inverse()`, so a floating-origin XZ
    /// shift (the terrain's render GT picking up a cell offset) silently moved
    /// the footprint and mis-answered. A grid-absolute read is untouched by
    /// wherever the render origin sits — assert that an offset GT changes nothing.
    #[test]
    fn query_ignores_the_render_transform() {
        let mut world = World::new();
        let terrain = tilted_terrain(&mut world);
        // The render origin lands a whole cell away in XZ (moonbase-scale).
        world
            .entity_mut(terrain)
            .insert(GlobalTransform::from_translation(Vec3::new(
                -2000.0, 1945.0, 2000.0,
            )));

        let d = ok_data(
            TerrainHeightProvider.execute(&world, &api_value!({"x": 5.0, "z": 0.0, "eps": 1.0})),
        );
        assert_eq!(field(&d, "found").as_bool(), Some(true), "{d:?}");
        assert!(
            (number(field(&d, "height")) - 0.5).abs() < 1e-4,
            "height {d:?}"
        );
    }

    #[test]
    fn reports_not_found_outside_footprint() {
        let mut world = World::new();
        tilted_terrain(&mut world);
        let d = ok_data(TerrainHeightProvider.execute(&world, &api_value!({"x": 100.0, "z": 0.0})));
        assert_eq!(field(&d, "found").as_bool(), Some(false));
    }

    #[test]
    fn missing_params_error() {
        let world = World::new();
        let result = TerrainHeightProvider.execute(&world, &api_value!({"x": 1.0}));
        assert!(matches!(result, Err(ApiQueryError { .. })));
    }

    // ── TerrainField ─────────────────────────────────────────────────────────

    #[test]
    fn field_raster_is_constant_slope_over_a_tilt() {
        let mut world = World::new();
        tilted_terrain(&mut world); // height = 0.1·x → slope atan(0.1) everywhere
        let d = ok_data(TerrainFieldProvider.execute(
            &world,
            // ±5 m region well inside the ±10 m footprint, so every texel-centred
            // finite difference stays in the linear region.
            &api_value!({"field": "slope", "x": 0.0, "z": 0.0, "half": 5.0, "res": 4}),
        ));
        assert_eq!(field(&d, "found").as_bool(), Some(true));
        assert_eq!(field(&d, "res").as_i64(), Some(4));
        let want = 0.1f64.atan();
        assert!((number(field(&d, "min")) - want).abs() < 1e-3, "min {d:?}");
        assert!((number(field(&d, "max")) - want).abs() < 1e-3, "max {d:?}");
        let data = array(field(&d, "data"));
        assert_eq!(data.len(), 16); // res*res
        assert!(data.iter().all(|v| (number(v) - want).abs() < 1e-3));
    }

    #[test]
    fn field_reports_not_found_outside_footprint() {
        let mut world = World::new();
        tilted_terrain(&mut world);
        let d = ok_data(TerrainFieldProvider.execute(
            &world,
            &api_value!({"field": "slope", "x": 100.0, "z": 0.0, "half": 5.0}),
        ));
        assert_eq!(field(&d, "found").as_bool(), Some(false));
    }

    #[test]
    fn field_reports_not_found_when_region_crosses_footprint() {
        let mut world = World::new();
        tilted_terrain(&mut world);
        let d = ok_data(TerrainFieldProvider.execute(
            &world,
            &api_value!({"field": "elevation", "x": 8.0, "z": 0.0, "half": 5.0}),
        ));
        assert_eq!(field(&d, "found").as_bool(), Some(false));
    }

    #[test]
    fn field_unknown_id_and_bad_half_error() {
        let mut world = World::new();
        tilted_terrain(&mut world);
        let bad_field = TerrainFieldProvider.execute(
            &world,
            &api_value!({"field": "mineral", "x": 0.0, "z": 0.0, "half": 5.0}),
        );
        assert!(matches!(bad_field, Err(ApiQueryError { .. })));
        let bad_half = TerrainFieldProvider.execute(
            &world,
            &api_value!({"field": "slope", "x": 0.0, "z": 0.0, "half": 0.0}),
        );
        assert!(matches!(bad_half, Err(ApiQueryError { .. })));
    }

    #[test]
    fn field_res_is_capped() {
        let mut world = World::new();
        tilted_terrain(&mut world);
        let d = ok_data(TerrainFieldProvider.execute(
            &world,
            &api_value!({"field": "elevation", "x": 0.0, "z": 0.0, "half": 5.0, "res": 100000}),
        ));
        assert_eq!(field(&d, "res").as_i64(), Some(FIELD_MAX_RES as i64));
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
            &world,
            &api_value!({ "origin": [0.0, 2.0, 0.0], "target": [10.0, 0.5, 0.0] }),
        ));
        assert_eq!(field(&d, "hit").as_bool(), Some(true), "{d:?}");
        let p = array(field(&d, "point"));
        assert!((number(&p[0]) - 8.0).abs() < 0.5, "intercept x {d:?}");
        let dist = number(field(&d, "distance"));
        assert!(dist > 6.0 && dist < 9.0, "distance {d:?}");
    }

    #[test]
    fn raycast_ray_above_relief_clears() {
        let mut world = World::new();
        tilted_terrain(&mut world);
        // Horizontal ray well above the highest terrain (max height 1.0 at x=10).
        let d = ok_data(TerrainRaycastProvider.execute(
            &world,
            &api_value!({ "origin": [-10.0, 100.0, 0.0], "dir": [1.0, 0.0, 0.0], "max": 20.0 }),
        ));
        assert_eq!(field(&d, "hit").as_bool(), Some(false), "{d:?}");
    }

    #[test]
    fn raycast_outside_footprint_no_hit() {
        let mut world = World::new();
        tilted_terrain(&mut world);
        let d = ok_data(TerrainRaycastProvider.execute(
            &world,
            &api_value!({ "origin": [200.0, 5.0, 0.0], "target": [210.0, 5.0, 0.0] }),
        ));
        assert_eq!(field(&d, "hit").as_bool(), Some(false), "{d:?}");
    }

    #[test]
    fn raycast_accepts_object_points_and_needs_origin() {
        let mut world = World::new();
        tilted_terrain(&mut world);
        // {x,y,z} map form parses the same as the array form.
        let origin = ApiValue::map([
            ("x", api_value!(0.0)),
            ("y", api_value!(2.0)),
            ("z", api_value!(0.0)),
        ]);
        let target = ApiValue::map([
            ("x", api_value!(10.0)),
            ("y", api_value!(0.5)),
            ("z", api_value!(0.0)),
        ]);
        let params = ApiValue::map([("origin", origin), ("target", target)]);
        let d = ok_data(TerrainRaycastProvider.execute(&world, &params));
        assert_eq!(field(&d, "hit").as_bool(), Some(true), "{d:?}");
        // No origin → error.
        assert!(matches!(
            TerrainRaycastProvider.execute(&world, &api_value!({ "dir": [1.0, 0.0, 0.0] })),
            Err(ApiQueryError { .. })
        ));
    }
}
