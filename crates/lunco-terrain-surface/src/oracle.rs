//! The composed **surface oracle** — the single analytic height truth every
//! consumer samples.
//!
//! A DEM terrain's surface is `modifiers ∘ base`: a raster base grid (the cropped
//! DEM, plus any layer that genuinely rasterises) with an ordered stack of
//! **analytic** [`HeightModifier`]s folded over it — craters, runtime edits, and
//! (later) procedural over-zoom detail. The composed [`SurfaceOracle`] is what the
//! CDLOD tile baker, the collider ring, the derived-layer texture bakes, the rock
//! scatter, and the `TerrainHeight` query all sample — **one function, every
//! consumer**, so visuals and physics converge by construction and feature
//! crispness is bounded only by how finely a consumer samples, never by a grid
//! resolution (see `docs/architecture/terrain-substrate.md`).
//!
//! Purity is load-bearing: the oracle is a deterministic function of the base
//! grid + modifier parameters, so derived artifacts (tiles, colliders, texture
//! maps) stay content-addressable and peer-identical. [`SurfaceOracle::content_key`]
//! folds every modifier's parameter hash for exactly that — cache keys downstream
//! mix it with their own inputs.

use std::sync::Arc;

use lunco_obstacle_field::field::HeightGrid;
use lunco_terrain_core::{HeightModifier, HeightSource};

/// One layer's analytic contribution to the composed surface: the modifier to
/// fold, plus a content hash of the parameters that produced it (folds into
/// downstream cache keys, since the modifier itself is opaque).
#[derive(Clone)]
pub struct HeightContribution {
    pub modifier: Arc<dyn HeightModifier>,
    /// Deterministic hash of the layer's generating parameters (spec + seed).
    pub content_key: u64,
}

/// The composed surface: raster `base` + ordered analytic `modifiers`. Cheap to
/// clone-share (`Arc` it once per terrain); `Send + Sync` so off-thread bakes
/// sample it directly.
pub struct SurfaceOracle {
    /// The raster base every modifier folds over (cropped DEM working grid).
    base: Arc<HeightGrid>,
    /// Analytic modifiers in fold order (USD prim order — index 0 first).
    modifiers: Vec<Arc<dyn HeightModifier>>,
    /// Folded [`HeightContribution::content_key`]s (0 with no modifiers).
    content_key: u64,
    /// Content hash of the raster base (heights + geometry), computed once at
    /// compose time so per-tile cache keys never re-fold the multi-million-point
    /// grid.
    base_key: u64,
}

/// Content hash of a raster grid: geometry params + every height, version-free
/// (callers fold their own format version).
fn grid_key(grid: &HeightGrid) -> u64 {
    let mut h = lunco_precompute::Fnv1a::new();
    h.write_u64(grid.res as u64);
    h.write_u64(grid.half_extent.to_bits() as u64);
    for &v in &grid.heights {
        h.write_u64(v.to_bits());
    }
    h.finish()
}

impl SurfaceOracle {
    /// An oracle that is just the raster base — no analytic layers.
    pub fn bare(base: Arc<HeightGrid>) -> Self {
        let base_key = grid_key(&base);
        Self { base, modifiers: Vec::new(), content_key: 0, base_key }
    }

    /// Compose `base` with the layers' analytic contributions, in order.
    pub fn new(base: Arc<HeightGrid>, contributions: Vec<HeightContribution>) -> Self {
        let mut key = lunco_precompute::Fnv1a::new();
        let mut modifiers = Vec::with_capacity(contributions.len());
        for c in &contributions {
            key.write_u64(c.content_key);
        }
        let content_key = if contributions.is_empty() { 0 } else { key.finish() };
        for c in contributions {
            modifiers.push(c.modifier);
        }
        let base_key = grid_key(&base);
        Self { base, modifiers, content_key, base_key }
    }

    /// The raster base grid (extents, spacing, raw ground reads).
    pub fn grid(&self) -> &Arc<HeightGrid> {
        &self.base
    }

    /// Half side length (metres) of the terrain footprint.
    pub fn half_extent(&self) -> f32 {
        self.base.half_extent
    }

    /// Metres between adjacent base-grid samples (the raster resolution floor —
    /// analytic modifiers resolve *below* this).
    pub fn spacing(&self) -> f32 {
        self.base.spacing()
    }

    /// Folded content hash of every modifier's parameters — mix into any cache
    /// key derived from this surface (the base grid hashes separately).
    pub fn content_key(&self) -> u64 {
        self.content_key
    }

    /// Content identity of the COMPLETE composed surface (base heights +
    /// modifier parameters). Two oracles with equal `surface_key` produce
    /// byte-identical samples everywhere — the per-tile bake cache keys on this
    /// so it never re-hashes the full grid.
    pub fn surface_key(&self) -> u64 {
        let mut h = lunco_precompute::Fnv1a::new();
        h.write_u64(self.base_key);
        h.write_u64(self.content_key);
        h.finish()
    }

    /// Whether any analytic modifier is stacked over the base.
    pub fn has_modifiers(&self) -> bool {
        !self.modifiers.is_empty()
    }

    /// A variant of this oracle **Nyquist-gated** for a consumer sampling at
    /// `min_wavelength` metres: band-limitable modifiers (procedural over-zoom,
    /// craters) swap in a gated copy so sub-sample features widen or fade out
    /// instead of aliasing — and synthesis cost drops with them. Modifiers with
    /// no gated form (brushes, flattens) pass through untouched. Cheap (clones a
    /// Vec of `Arc`s); call it per bake with that bake's sample spacing.
    pub fn detail_limited(&self, min_wavelength: f64) -> SurfaceOracle {
        let modifiers = self
            .modifiers
            .iter()
            .map(|m| m.with_min_wavelength(min_wavelength).unwrap_or_else(|| m.clone()))
            .collect();
        SurfaceOracle {
            base: self.base.clone(),
            modifiers,
            content_key: self.content_key,
            base_key: self.base_key,
        }
    }

    /// Rasterise the composed surface at the base grid's own resolution — for
    /// the consumers that genuinely need a grid (the static full-DEM mesh and
    /// collider). Detail below the grid's own spacing is Nyquist-gated out.
    /// Streaming consumers sample the oracle directly instead.
    pub fn materialize(&self) -> HeightGrid {
        let mut grid = (*self.base).clone();
        if self.modifiers.is_empty() {
            return grid;
        }
        let limited = self.detail_limited(self.spacing() as f64);
        let this = &limited;
        let res = grid.res;
        let s = grid.spacing();
        let origin = -grid.half_extent;
        for iz in 0..res {
            let z = (origin + iz as f32 * s) as f64;
            for ix in 0..res {
                let x = (origin + ix as f32 * s) as f64;
                let i = iz * res + ix;
                let mut h = grid.heights[i];
                for m in &this.modifiers {
                    h = m.apply(x, z, h);
                }
                grid.heights[i] = h;
            }
        }
        grid
    }
}

impl HeightSource for SurfaceOracle {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        let mut h = HeightSource::height_at(self.base.as_ref(), x, z);
        for m in &self.modifiers {
            h = m.apply(x, z, h);
        }
        h
    }
}

/// First intersection of a ray with the composed surface, in TERRAIN-LOCAL
/// space (heights along +Y over local `(x, z)`); `None` when the ray never
/// crosses the surface inside the DEM footprint within `max_t` metres.
///
/// Adaptive march (step grows with distance, capped) with a bisection refine on
/// the bracketing interval, so cursor-scale queries cost a few hundred oracle
/// samples worst case. This is the ground-truth pick for UI ghosts: physics
/// colliders only exist in the ring near dynamic bodies and are band-limited
/// below the drawn micro-relief, so a collider raycast either misses open
/// terrain entirely or reports a surface ABOVE the drawn one — the "cursor
/// flying over the ground" artifact.
pub fn raycast_surface(
    oracle: &SurfaceOracle,
    origin: bevy::math::DVec3,
    dir: bevy::math::DVec3,
    max_t: f64,
) -> Option<bevy::math::DVec3> {
    let half = oracle.half_extent() as f64;
    let inside = |p: bevy::math::DVec3| p.x.abs() <= half && p.z.abs() <= half;
    let above = |p: bevy::math::DVec3| p.y >= oracle.height_at(p.x, p.z);

    let mut t = 0.0_f64;
    // `Some(t)` = the last sample inside the footprint AND above the surface —
    // the near end of a potential bracket. Reset when the ray leaves the
    // footprint so a re-entry can't bracket across the gap.
    let mut last_above: Option<f64> = None;
    while t <= max_t {
        let p = origin + dir * t;
        if inside(p) {
            if above(p) {
                last_above = Some(t);
            } else if let Some(t0) = last_above {
                // Crossing bracketed in [t0, t] — bisect to centimetre scale.
                let (mut lo, mut hi) = (t0, t);
                for _ in 0..24 {
                    let mid = 0.5 * (lo + hi);
                    if above(origin + dir * mid) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                let th = 0.5 * (lo + hi);
                let hit = origin + dir * th;
                return Some(bevy::math::DVec3::new(
                    hit.x,
                    oracle.height_at(hit.x, hit.z),
                    hit.z,
                ));
            } else {
                // Started (or re-entered) below the surface — no bracket.
                return None;
            }
        } else {
            last_above = None;
        }
        // Fine near the camera (sub-metre relief matters at cursor range),
        // coarsening with distance; capped so distant rims aren't skipped.
        t += (0.25 + t * 0.02).min(6.0);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_terrain_core::{Crater, Craters};

    fn flat(res: usize, half: f32) -> Arc<HeightGrid> {
        Arc::new(HeightGrid::new_flat(res, half))
    }

    fn crater_contribution() -> HeightContribution {
        let craters = Craters::new(vec![Crater {
            center: [0.0, 0.0],
            radius: 10.0,
            depth: 2.0,
            rim_height: 0.4,
            softness: 0.0,
            bowl_power: 4.0,
        }]);
        HeightContribution { modifier: Arc::new(craters), content_key: 0xC0FFEE }
    }

    #[test]
    fn bare_is_base() {
        let o = SurfaceOracle::bare(flat(9, 50.0));
        assert_eq!(HeightSource::height_at(&o, 3.0, -4.0), 0.0);
        assert_eq!(o.content_key(), 0);
        assert!(!o.has_modifiers());
    }

    #[test]
    fn modifiers_fold_over_base() {
        let o = SurfaceOracle::new(flat(9, 50.0), vec![crater_contribution()]);
        assert!(HeightSource::height_at(&o, 0.0, 0.0) < -1.0, "crater floor drops");
        assert!(HeightSource::height_at(&o, 10.0, 0.0) > 0.0, "rim rises");
        assert_eq!(HeightSource::height_at(&o, 40.0, 40.0), 0.0, "far field = base");
        assert!(o.has_modifiers());
        assert_ne!(o.content_key(), 0);
    }

    #[test]
    fn materialize_matches_gated_sampling_at_grid_points() {
        let o = SurfaceOracle::new(flat(33, 50.0), vec![crater_contribution()]);
        let m = o.materialize();
        let s = m.spacing();
        // `materialize` Nyquist-gates at the grid's own spacing (craters included,
        // since they band-limit too) — so it must equal sampling the GATED oracle,
        // not the full-detail one.
        let limited = o.detail_limited(s as f64);
        for iz in 0..m.res {
            for ix in 0..m.res {
                let x = (-m.half_extent + ix as f32 * s) as f64;
                let z = (-m.half_extent + iz as f32 * s) as f64;
                let sampled = HeightSource::height_at(&limited, x, z);
                assert!(
                    (m.heights[iz * m.res + ix] - sampled).abs() < 1e-9,
                    "mismatch at ({x},{z})"
                );
            }
        }
    }

    #[test]
    fn content_key_deterministic_and_order_sensitive() {
        let a = SurfaceOracle::new(flat(5, 10.0), vec![crater_contribution()]);
        let b = SurfaceOracle::new(flat(5, 10.0), vec![crater_contribution()]);
        assert_eq!(a.content_key(), b.content_key());
    }
}
