//! The tunable knobs — `ObstacleFieldSpec` and its distribution parameters.
//!
//! A spec is the single source of truth for a field: the same `(spec, seed)`
//! always produces the same field, so networking replicates the spec rather than
//! the generated geometry, and an experiment sweep just varies these numbers.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// A log-normal size distribution in metres (rock radius / crater rim radius).
///
/// Log-normal because real rock/crater size-frequency is heavy-tailed: many
/// small, few large. `mode` is the most-likely value; `sigma` is the spread in
/// log-space (0 → always `mode`). Samples are clamped to `[min, max]`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Reflect)]
pub struct SizeDist {
    pub min: f32,
    pub max: f32,
    pub mode: f32,
    pub sigma: f32,
}

impl SizeDist {
    pub const fn new(min: f32, mode: f32, max: f32, sigma: f32) -> Self {
        Self {
            min,
            max,
            mode,
            sigma,
        }
    }
}

/// Spatial placement pattern.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Reflect)]
pub enum Pattern {
    /// Independent uniform random positions. Cheapest; objects can overlap.
    Uniform,
    /// Blue-noise (Bridson Poisson-disk) — evenly spaced, no two closer than
    /// `min_spacing` metres. Realistic boulder scatter, but count is bounded by
    /// how many fit, so it may under-fill a high requested density.
    PoissonDisk { min_spacing: f32 },
    /// `clusters` Gaussian blobs of `spread` metres std-dev — debris fields,
    /// rubble piles, ejecta concentrations.
    Clustered { clusters: u32, spread: f32 },
}

/// Crater layer: depressions stamped directly into the generated heightfield.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Reflect)]
pub struct CraterLayer {
    pub enabled: bool,
    /// Expected count per hectare (10 000 m²).
    pub density: f32,
    /// Rim radius distribution.
    pub size: SizeDist,
    /// Bowl depth as a fraction of rim radius.
    pub depth_ratio: f32,
    /// Raised rim lip height as a fraction of bowl depth.
    pub rim_height_ratio: f32,
}

/// Rock layer: obstacles placed on the surface (static decoration + a pushable
/// fraction).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Reflect)]
pub struct RockLayer {
    pub enabled: bool,
    /// Expected count per hectare (10 000 m²).
    pub density: f32,
    /// Rock radius distribution.
    pub size: SizeDist,
    /// Fraction `[0,1]` spawned as interactive (pushable, networked) bodies; the
    /// rest are static collidable decoration.
    pub dynamic_fraction: f32,
}

/// Full description of a procedural obstacle field. The `Resource` the app
/// tunes and the experiment sweep varies.
#[derive(Resource, Clone, Debug, Serialize, Deserialize, Reflect)]
#[reflect(Resource)]
pub struct ObstacleFieldSpec {
    /// Master seed — combined with per-layer salts for reproducibility.
    pub seed: u64,
    /// Half-extent in metres; the field covers `[-h, h]` in X and Z.
    pub region_half_extent: f32,
    /// Heightfield resolution (samples per side) for both collider and visual mesh.
    pub grid_resolution: u32,
    pub pattern: Pattern,
    pub craters: CraterLayer,
    pub rocks: RockLayer,
}

impl ObstacleFieldSpec {
    /// Side length of the square region in metres.
    pub fn region_size(&self) -> f32 {
        self.region_half_extent * 2.0
    }

    /// Region area in m².
    pub fn region_area(&self) -> f32 {
        let s = self.region_size();
        s * s
    }

    /// Expected object count for a per-hectare `density` over this region.
    pub fn count_for_density(&self, density_per_hectare: f32) -> usize {
        ((density_per_hectare * self.region_area()) / 10_000.0)
            .round()
            .max(0.0) as usize
    }
}

impl Default for ObstacleFieldSpec {
    fn default() -> Self {
        // A ~400 m driveable test arena centred on the rover spawn (rovers sit
        // within ±15 m of the origin). Densities are deliberately gentle for the
        // pre-streaming phase: ~80 craters + ~480 rocks.
        Self {
            seed: 0xC0FFEE,
            region_half_extent: 200.0,
            grid_resolution: 401,
            pattern: Pattern::PoissonDisk { min_spacing: 3.0 },
            craters: CraterLayer {
                enabled: true,
                density: 5.0,
                size: SizeDist::new(2.0, 4.0, 12.0, 0.5),
                // Fresh simple-crater proportions: d/D ≈ 0.2 (depth = 0.4·r) and
                // rim/D ≈ 0.036 (rim = 0.18 × depth) — measured lunar values.
                depth_ratio: 0.4,
                rim_height_ratio: 0.18,
            },
            rocks: RockLayer {
                enabled: true,
                density: 30.0,
                size: SizeDist::new(0.2, 0.5, 2.5, 0.6),
                dynamic_fraction: 0.05,
            },
        }
    }
}
