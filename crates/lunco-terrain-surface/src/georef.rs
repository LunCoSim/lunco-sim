//! Terrain georeference (#5): the lat/lon/height anchor + stage units authored on
//! a USD terrain prim (`lunco:anchor:*` + `metersPerUnit`), CesiumGeoreference-style.
//!
//! The DEM height math is unit-less local XZ metres; this component records WHERE
//! on the body that local frame sits, so a point on the surface can be reported
//! back in body coordinates (and, later, blended into the globe via
//! [`lunco_terrain_core::CompositeHeightSource`]'s georeferenced region). It is
//! pure data — no projection is applied here; full lat/lon↔XZ reprojection (polar
//! stereographic for the lunar sites) is a deferred upgrade.
//!
//! The DEM raster already states its own extent and projection in its GeoTIFF tags
//! (see [`lunco_terrain_bake::dem::read_geotiff_transform`]); USD `lunco:anchor:*`
//! lets a scene override or author the anchor declaratively, the same way
//! `lunco:terrain:*` authors the build.

use bevy::prelude::*;

/// Where a terrain's local XZ frame is anchored on the body, + the stage's unit
/// scale. Attached to the terrain prim entity by the USD→DEM bridge.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct TerrainGeoref {
    /// NAIF id of the body this terrain sits on (301 Moon, 399 Earth) —
    /// `lunco:anchor:body`, defaulting to [`DEFAULT_ANCHOR_BODY`].
    ///
    /// This is what makes the terrain's own curvature a property of the TERRAIN
    /// rather than of ECS iteration order. The body's radius is folded into the
    /// surface oracle as the final `BodyCurvature` modifier, so it is not
    /// metadata — it changes the composed geometry AND the `content_key` every
    /// downstream cache keys on. It was previously resolved by picking whatever
    /// `SiteAnchor` a `q_site.iter().next()` returned first, i.e. by archetype
    /// order: a scene carrying a second anchor (a ground station authors body
    /// 399) could adopt Earth's 6371 km radius for a lunar DEM, and *which* one
    /// won varied per launch with async USD load order. That is the "terrain is
    /// different every launch" bug — generation must be a pure function of the
    /// document, never of load order.
    pub body: i32,
    /// Latitude (degrees) of the DEM frame origin (local XZ 0,0).
    pub center_lat_deg: f64,
    /// Longitude (degrees) of the DEM frame origin.
    pub center_lon_deg: f64,
    /// Height (metres, body datum) the local Y=0 plane sits at.
    pub anchor_height_m: f64,
    /// USD stage `metersPerUnit`. The terrain pipeline assumes **1** (1 unit = 1 m);
    /// anything else is recorded but flagged — the height/collider math is metres.
    pub meters_per_unit: f64,
}

/// Body a terrain anchors to when the scene does not author `lunco:anchor:body`.
/// Matches the celestial bridge's own default (Moon) — the two MUST agree, or an
/// unauthored scene curves to one body and pins its site frame to another.
pub const DEFAULT_ANCHOR_BODY: i32 = 301;

impl Default for TerrainGeoref {
    fn default() -> Self {
        Self {
            body: DEFAULT_ANCHOR_BODY,
            center_lat_deg: 0.0,
            center_lon_deg: 0.0,
            anchor_height_m: 0.0,
            meters_per_unit: 1.0,
        }
    }
}

impl TerrainGeoref {
    /// True when the stage units are the assumed 1 m/unit (within tolerance).
    pub fn units_are_metres(&self) -> bool {
        (self.meters_per_unit - 1.0).abs() < 1e-6
    }
}
