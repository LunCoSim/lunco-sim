//! Built-in **craters** layer.
//!
//! A crater layer contributes an **analytic** [`Craters`] height modifier to the
//! terrain's [`SurfaceOracle`](crate::oracle::SurfaceOracle) — craters as *math you
//! sample*, not pixels you stamp. The CDLOD tile baker and the collider ring both
//! sample the ONE composed source at their own resolution, so:
//!
//! - a rim resolves as sharply as the nearest tile tessellates (sub-metre near the
//!   camera), unbounded by the DEM grid spacing;
//! - the rover drives exactly the bowl it sees — visuals and contact converge by
//!   construction;
//! - craters follow the surrounding relief because they are deltas ON it.
//!
//! History: v1 rasterised bowls into the working `HeightGrid` (rims bounded by grid
//! resolution — the blocky "staircase" rims) after v0's separate floating overlay
//! mesh caused the pedestal/mismatch bugs. The analytic modifier is the design the
//! whole oracle substrate was built for; see `docs/architecture/terrain-substrate.md`.
//!
//! Placement is the deterministic complete-spatial-randomness set from
//! [`crate::terrain::crater_placements`] — identical on every peer from the seed.

use std::sync::Arc;

use lunco_obstacle_field::spec::{CraterLayer, SizeDist};
use lunco_terrain_core::{Crater, Craters};

use super::{LayerAttrSource, TerrainLayer};
use crate::oracle::HeightContribution;

/// The composable crater layer: deterministic placements → analytic [`Craters`]
/// modifier on the surface oracle.
struct CraterFieldLayer {
    craters: CraterLayer,
    seed: u64,
}

impl TerrainLayer for CraterFieldLayer {
    fn id(&self) -> &'static str {
        "craters"
    }

    fn height_modifier(&self, half_extent: f32) -> Option<HeightContribution> {
        let placements = crate::terrain::crater_placements(&self.craters, self.seed, half_extent);
        if placements.is_empty() {
            return None;
        }
        let craters: Vec<Crater> = placements
            .iter()
            .map(|p| {
                let depth = p.size * self.craters.depth_ratio;
                Crater {
                    center: [p.pos.x as f64, p.pos.y as f64],
                    radius: p.size as f64,
                    depth: depth as f64,
                    rim_height: (depth * self.craters.rim_height_ratio) as f64,
                }
            })
            .collect();
        // Content key: every parameter that shapes the placements + profile, so
        // downstream content-addressed bakes (derived maps, tiles) re-key on any
        // live crater tweak.
        let mut key = lunco_precompute::Fnv1a::new();
        // Placement-algorithm version: bump when the same spec yields different
        // placements (blue-noise → complete spatial randomness = v2).
        key.write_u64(2);
        key.write_u64(self.seed);
        key.write_u64(half_extent.to_bits() as u64);
        key.write_u64(self.craters.density.to_bits() as u64);
        key.write_u64(self.craters.size.min.to_bits() as u64);
        key.write_u64(self.craters.size.mode.to_bits() as u64);
        key.write_u64(self.craters.size.max.to_bits() as u64);
        key.write_u64(self.craters.depth_ratio.to_bits() as u64);
        key.write_u64(self.craters.rim_height_ratio.to_bits() as u64);
        bevy::log::info!(
            "[terrain-layer/craters] composed {} analytic crater(s) (±{:.0} m)",
            craters.len(),
            half_extent
        );
        Some(HeightContribution {
            modifier: Arc::new(Craters::new(craters)),
            content_key: key.finish(),
        })
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
    Some(Arc::new(CraterFieldLayer { craters, seed }))
}

/// Build a crater layer from a typed [`CraterLayer`] (e.g. the Inspector's
/// `ObstacleFieldSpec.craters`) so live tuning can rebuild the terrain's crater
/// layer directly — honouring every authored field (density, depth, rim, full size
/// distribution).
pub fn crater_layer(craters: CraterLayer, seed: u64) -> Arc<dyn TerrainLayer> {
    Arc::new(CraterFieldLayer { craters, seed })
}

/// Construct a crater layer directly (the quick `SpawnDemTerrain` command path /
/// programmatic use; the USD path uses [`parse_crater_layer`]).
pub fn make_crater_layer(density: f32, size_mode: f32, depth_ratio: f32, seed: u64) -> Arc<dyn TerrainLayer> {
    Arc::new(CraterFieldLayer {
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
