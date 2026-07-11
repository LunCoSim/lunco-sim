//! Generic celestial geometry queries — domain-free spatial answers exposed to
//! the API / scripting surface via `query(name, params)`, the same mechanism as
//! the terrain providers (`TerrainHeight` / `TerrainRaycast`).
//!
//! These are the geometry substrate that AUTHORED subsystems (comms, solar,
//! thermal) compose over; **none of them names a domain** (docs 10/12). They
//! reuse the ephemeris, the body registry, and the analytic `segment_hits_sphere`
//! occlusion — so a link-availability or sun-exposure rule can be authored in
//! rhai over `query("Occultation", …)` with no comms/solar Rust. As `comms.rs`
//! is retired, its geometry half lives here; the domain half moves to `comms.rhai`.

use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_time::WorldTime;

use crate::comms::segment_hits_sphere;
use crate::coords::ecliptic_to_bevy;
use crate::ephemeris::EphemerisResource;
use crate::registry::CelestialBodyRegistry;

/// Read a `[x,y,z]` array or `{x,y,z}` map into a solar-frame [`DVec3`].
fn parse_point(v: Option<&serde_json::Value>) -> Option<DVec3> {
    let v = v?;
    if let Some(a) = v.as_array() {
        if a.len() < 3 {
            return None;
        }
        return Some(DVec3::new(a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?));
    }
    Some(DVec3::new(
        v.get("x")?.as_f64()?,
        v.get("y")?.as_f64()?,
        v.get("z")?.as_f64()?,
    ))
}

/// `Occultation` — does a celestial body block the segment `origin→target`?
/// Analytic ray–sphere over the body registry at the current epoch, in the
/// solar frame (metres). This is the generic occlusion primitive a comms link
/// or a sun-exposure test composes; it knows nothing about antennas.
///
/// params: `{ origin:[x,y,z], target:[x,y,z] }` (also accepts `{x,y,z}` maps).
/// returns: `{ occluded, by }` — `by` = blocking body name, or null when clear.
/// Clear (`occluded:false`) when no ephemeris/registry/clock is present.
pub struct OccultationProvider;

impl ApiQueryProvider for OccultationProvider {
    fn name(&self) -> &'static str {
        "Occultation"
    }

    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let (Some(o), Some(t)) = (
            parse_point(params.get("origin")),
            parse_point(params.get("target")),
        ) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "Occultation: `origin` and `target` [x,y,z] required".to_string(),
            );
        };
        let clear = || ApiResponse::ok(serde_json::json!({ "occluded": false, "by": null }));
        let Some(jd) = world.get_resource::<WorldTime>().map(|w| w.epoch_jd) else {
            return clear();
        };
        let (Some(eph), Some(reg)) = (
            world.get_resource::<EphemerisResource>(),
            world.get_resource::<CelestialBodyRegistry>(),
        ) else {
            return clear();
        };
        let mut by: Option<String> = None;
        for b in reg.bodies.iter().filter(|b| b.radius_m > 0.0) {
            let center = ecliptic_to_bevy(eph.provider.global_position(b.ephemeris_id, jd));
            if segment_hits_sphere(o, t, center, b.radius_m) {
                by = Some(b.name.clone());
                break;
            }
        }
        ApiResponse::ok(serde_json::json!({ "occluded": by.is_some(), "by": by }))
    }
}

/// `BodyPosition` — solar-frame position + radius of a registry body at the
/// current epoch, so an authored subsystem can compute range / direction /
/// elevation itself.
///
/// params: `{ body: <NAIF id> }`. returns: `{ found, pos:[x,y,z], radius }`.
pub struct BodyPositionProvider;

impl ApiQueryProvider for BodyPositionProvider {
    fn name(&self) -> &'static str {
        "BodyPosition"
    }

    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let Some(naif) = params.get("body").and_then(serde_json::Value::as_i64) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "BodyPosition: `body` (NAIF id) required".to_string(),
            );
        };
        let naif = naif as i32;
        let Some(jd) = world.get_resource::<WorldTime>().map(|w| w.epoch_jd) else {
            return ApiResponse::ok(serde_json::json!({ "found": false }));
        };
        let Some(eph) = world.get_resource::<EphemerisResource>() else {
            return ApiResponse::ok(serde_json::json!({ "found": false }));
        };
        let radius = world
            .get_resource::<CelestialBodyRegistry>()
            .and_then(|r| r.bodies.iter().find(|b| b.ephemeris_id == naif).map(|b| b.radius_m))
            .unwrap_or(0.0);
        let p = ecliptic_to_bevy(eph.provider.global_position(naif, jd));
        ApiResponse::ok(serde_json::json!({
            "found": true,
            "pos": [p.x, p.y, p.z],
            "radius": radius,
        }))
    }
}

/// Register the generic celestial geometry providers into the [`ApiQueryRegistry`]
/// (init-if-absent, mirroring `register_terrain_queries`). Called from
/// [`CelestialPlugin`](crate::CelestialPlugin) — these are generic geometry, not
/// comms, so they do NOT ride on `CommsPlugin`.
pub fn register_celestial_queries(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let mut reg = app.world_mut().resource_mut::<ApiQueryRegistry>();
    reg.register(OccultationProvider);
    reg.register(BodyPositionProvider);
}
