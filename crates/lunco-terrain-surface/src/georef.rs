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
//! `DemMetadata` already carries `center_lat`/`center_lon` parsed from the DEM's
//! `metadata.yaml`; USD `lunco:anchor:*` lets a scene override/author it
//! declaratively, the same way `lunco:terrain:*` authors the build.

use bevy::prelude::*;

/// Where a terrain's local XZ frame is anchored on the body, + the stage's unit
/// scale. Attached to the terrain prim entity by the USD→DEM bridge.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct TerrainGeoref {
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

impl Default for TerrainGeoref {
    fn default() -> Self {
        Self {
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
