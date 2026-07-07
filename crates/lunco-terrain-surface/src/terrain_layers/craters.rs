//! Built-in **craters** layer.
//!
//! A crater layer **stamps** its impact basins into the working height grid, so the
//! streamed tiles AND the heightfield collider read one and the same surface: the
//! rover drives exactly the bowl it sees, and craters follow the surrounding relief
//! because they are part of it.
//!
//! It used to also **scatter** a separate high-fidelity overlay mesh over each near
//! crater, floated a hair above the tiles with a constant `lift` so it won the depth
//! test. That was the source of the "craters sit on a pedestal / don't follow the
//! ground / the collider disagrees with what you see" bug: the overlay followed the
//! smooth pre-crater base and carried a lift, while the tiles and collider followed
//! the stamped grid — two surfaces that never agreed. The overlay is gone; a crater
//! is now a single stamped surface.
//!
//! Crisper-than-grid rim detail returns the right way once the streamer samples the
//! crater profile analytically (`lunco_terrain_core::CraterField`) in the tile baker
//! *and* the collider ring — one composed `HeightSource` both sample at their own
//! resolution, so detail is unbounded by the grid and the two still agree. See
//! `docs/architecture/terrain-substrate.md`.

use std::sync::Arc;

use bevy::log::info;
use lunco_obstacle_field::field::HeightGrid;
use lunco_obstacle_field::spec::{CraterLayer, SizeDist};

use super::{LayerAttrSource, TerrainLayer};

/// Stamps the drivable impact basins into the DEM grid. Tiles and collider both read
/// the stamped grid, so the crater the rover hits is the crater it sees.
struct CraterStampLayer {
    craters: CraterLayer,
    seed: u64,
}

impl TerrainLayer for CraterStampLayer {
    fn id(&self) -> &'static str {
        "craters"
    }

    fn stamp(&self, grid: &mut HeightGrid) {
        let n = lunco_terrain_bake::stamp::stamp_spec_craters(grid, &self.craters, self.seed);
        if n > 0 {
            info!("[terrain-layer/craters] stamped {n} crater(s) (±{:.0} m)", grid.half_extent);
        }
    }

    fn stamp_spec(&self) -> Option<lunco_terrain_bake::StampSpec> {
        Some(lunco_terrain_bake::StampSpec::Craters { layer: self.craters, seed: self.seed })
    }
}

/// Parse a `lunco:layer = "craters"` prim: `density` (per ha, required > 0),
/// `sizeMode` (modal rim radius m), `depthRatio`, `rimRatio`, `seed`. DEM-scale size
/// range brackets the modal radius.
pub(super) fn parse_crater_layer(a: &dyn LayerAttrSource) -> Option<Arc<dyn TerrainLayer>> {
    let density = a.get_f32("density").unwrap_or(0.0);
    if density <= 0.0 {
        return None;
    }
    let mode = a.get_f32("sizeMode").unwrap_or(22.0);
    let craters = CraterLayer {
        enabled: true,
        density,
        size: SizeDist::new(8.0, mode, 40.0, 0.7),
        depth_ratio: a.get_f32("depthRatio").unwrap_or(0.3),
        rim_height_ratio: a.get_f32("rimRatio").unwrap_or(0.5),
    };
    let seed = a.get_i64("seed").map(|s| s as u64).unwrap_or(0xC0FFEE);
    Some(Arc::new(CraterStampLayer { craters, seed }))
}

/// Build a crater layer from a typed [`CraterLayer`] (e.g. the Inspector's
/// `ObstacleFieldSpec.craters`) so live tuning can rebuild the terrain's crater
/// layer directly — honouring every authored field (density, depth, rim, full size
/// distribution).
pub fn crater_layer(craters: CraterLayer, seed: u64) -> Arc<dyn TerrainLayer> {
    Arc::new(CraterStampLayer { craters, seed })
}

/// Construct a crater layer directly (the quick `SpawnDemTerrain` command path /
/// programmatic use; the USD path uses [`parse_crater_layer`]).
pub fn make_crater_layer(density: f32, size_mode: f32, depth_ratio: f32, seed: u64) -> Arc<dyn TerrainLayer> {
    Arc::new(CraterStampLayer {
        craters: CraterLayer {
            enabled: true,
            density,
            size: SizeDist::new(8.0, size_mode, 40.0, 0.7),
            depth_ratio,
            rim_height_ratio: 0.5,
        },
        seed,
    })
}
