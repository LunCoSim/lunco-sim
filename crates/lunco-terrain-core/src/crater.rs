//! Analytic crater field — craters as a composable [`HeightSource`] modifier,
//! not a baked grid.
//!
//! This is the pure heart of the crater-bug fix. The old path had **two
//! surfaces**: a coarse grid with crater bowls rasterised in (truth for tiles +
//! collider) *and* a separate high-fidelity overlay mesh floated over near craters
//! with a constant vertical `lift`. The overlay followed the smooth pre-crater
//! base while tiles/collider followed the stamped grid, so craters sat on a
//! pedestal, drifted free of the surrounding relief, and the rover collided with a
//! blocky bowl while seeing a crisp one.
//!
//! The cure is to make a crater a **function you sample**, not pixels you stamp.
//! [`CraterField`] wraps the source below it (`Craters ∘ Dem ∘ Globe`) and *adds*
//! each nearby crater's analytic cross-section to it. The visual tile baker and the
//! avian collider ring both sample this ONE composed source at their own
//! resolution, so they converge exactly — the crater is as crisp as whatever grid
//! samples it, unbounded by any DEM mip. Purity is preserved (see [`HeightSource`]),
//! so derived tiles/colliders stay content-addressable and peer-identical.
//!
//! Placement lookup is O(craters-near-the-query) via a deterministic spatial
//! bucket index, so `height_at` stays cheap even with thousands of craters over a
//! wide region. Determinism is load-bearing: identical crater lists yield identical
//! sums on every platform (fixed integer bucketing + fixed summation order).

use std::collections::HashMap;

use crate::source::HeightSource;

/// Radial reach of a crater's influence, as a multiple of its radius. Beyond this
/// the [`crater_profile`] contribution is exactly zero (bowl ends at `d=1`, rim at
/// `d≈1`, ejecta apron at `d<1.6`). Matches the rasteriser's `radius * 1.6` reach.
pub const CRATER_REACH: f64 = 1.6;

/// One crater placement in the terrain XZ plane (metres).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crater {
    /// Centre `[x, z]` in the terrain-local frame (metres).
    pub center: [f64; 2],
    /// Rim radius (metres): `d = 1` at this distance from centre.
    pub radius: f64,
    /// Bowl depth (metres, positive = how far the floor drops below the datum).
    pub depth: f64,
    /// Raised rim-lip height (metres above the datum at `d≈1`).
    pub rim_height: f64,
}

impl Crater {
    /// Absolute reach in metres — past this the crater adds nothing.
    #[inline]
    pub fn reach(&self) -> f64 {
        self.radius * CRATER_REACH
    }

    /// Height delta (metres) this crater contributes at world `(x, z)`. Zero
    /// outside its reach, so summing craters is naturally local.
    #[inline]
    pub fn delta_at(&self, x: f64, z: f64) -> f64 {
        let r = self.radius;
        if r <= 0.0 {
            return 0.0;
        }
        let dx = x - self.center[0];
        let dz = z - self.center[1];
        let d = (dx * dx + dz * dz).sqrt() / r; // normalised radial distance
        if d >= CRATER_REACH {
            return 0.0;
        }
        crater_profile(d, self.depth, self.rim_height)
    }
}

/// Crater cross-section (metres) at normalised radial distance `d` (0 = centre,
/// 1 = rim radius). The `f64` canonical of `lunco-obstacle-field`'s `crater_delta`
/// — same profile, sampled instead of rasterised: a fairly flat floor (`1 − d⁴`
/// holds near max depth across the floor) turning UP into a steep inner wall, a
/// SHARP raised rim lip at `d≈1` (the key cue under raking light), then a low
/// outward ejecta apron to `d≈1.6`.
#[inline]
pub fn crater_profile(d: f64, depth: f64, rim_height: f64) -> f64 {
    let bowl = if d < 1.0 { -depth * (1.0 - d * d * d * d) } else { 0.0 };
    let rim = rim_height * (-((d - 0.98) / 0.14).powi(2)).exp();
    let apron = if (1.0..1.6).contains(&d) {
        rim_height * 0.25 * (-((d - 1.15) / 0.30).powi(2)).exp()
    } else {
        0.0
    };
    bowl + rim + apron
}

/// A bucket-indexed set of craters — the crater contribution as a reusable
/// [`HeightModifier`](crate::modifier::HeightModifier), independent of any base. Fold
/// it onto a surface directly ([`CraterField`]) or stack it with other edits in a
/// [`LayeredHeightSource`](crate::modifier::LayeredHeightSource). Several `Craters`
/// modifiers stack cleanly (multiple crater layers) — deltas accumulate in order.
#[derive(Debug, Clone)]
pub struct Craters {
    /// Craters in a stable order — summation walks this order for FP determinism.
    craters: Vec<Crater>,
    /// Metres per bucket cell.
    cell_size: f64,
    /// Bucket → indices into `craters` whose reach bbox overlaps that cell. A crater
    /// is inserted into every cell its `[center ± reach]` box touches, so the single
    /// cell containing a query point holds every crater that can affect it — one cell
    /// lookup, no neighbour scan.
    buckets: HashMap<(i64, i64), Vec<u32>>,
}

impl Craters {
    /// Build the bucket index. Cell size is derived from the largest crater reach so
    /// each bucket holds a bounded candidate set; an empty set contributes nothing.
    pub fn new(craters: Vec<Crater>) -> Self {
        // Cell just big enough that the biggest crater spans a bounded 3×3 of cells.
        let max_reach = craters.iter().map(|c| c.reach()).fold(0.0_f64, f64::max);
        let cell_size = max_reach.max(1.0);
        let mut buckets: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
        for (i, c) in craters.iter().enumerate() {
            let reach = c.reach();
            if reach <= 0.0 {
                continue;
            }
            let (min_cx, min_cz) = cell_of(c.center[0] - reach, c.center[1] - reach, cell_size);
            let (max_cx, max_cz) = cell_of(c.center[0] + reach, c.center[1] + reach, cell_size);
            for cz in min_cz..=max_cz {
                for cx in min_cx..=max_cx {
                    buckets.entry((cx, cz)).or_default().push(i as u32);
                }
            }
        }
        Self { craters, cell_size, buckets }
    }

    /// Number of craters.
    pub fn crater_count(&self) -> usize {
        self.craters.len()
    }

    /// Summed crater delta (metres) at `(x, z)`. Candidate craters come from the
    /// single bucket containing the point; summation follows crater index order
    /// (deterministic across platforms).
    pub fn delta_at(&self, x: f64, z: f64) -> f64 {
        let key = cell_of(x, z, self.cell_size);
        let Some(indices) = self.buckets.get(&key) else {
            return 0.0;
        };
        let mut sum = 0.0;
        for &i in indices {
            sum += self.craters[i as usize].delta_at(x, z);
        }
        sum
    }
}

impl crate::modifier::HeightModifier for Craters {
    fn apply(&self, x: f64, z: f64, h_in: f64) -> f64 {
        h_in + self.delta_at(x, z)
    }
}

/// A composable [`HeightSource`]: `base` plus a [`Craters`] modifier folded over it.
/// Wrap the surface below it (`CraterField::new(dem, …)`) so the composed source is
/// the single truth the baker and collider both sample.
#[derive(Debug, Clone)]
pub struct CraterField<S> {
    /// The surface below the craters (DEM, globe, or another modifier).
    base: S,
    /// The crater contribution.
    craters: Craters,
}

impl<S> CraterField<S> {
    /// Wrap `base` with `craters`; an empty set degrades to just sampling `base`.
    pub fn new(base: S, craters: Vec<Crater>) -> Self {
        Self { base, craters: Craters::new(craters) }
    }

    /// Number of craters in the field.
    pub fn crater_count(&self) -> usize {
        self.craters.crater_count()
    }

    /// Summed crater delta (metres) at `(x, z)`, ignoring `base`.
    pub fn crater_delta_at(&self, x: f64, z: f64) -> f64 {
        self.craters.delta_at(x, z)
    }

    /// The underlying crater modifier (to stack it elsewhere).
    pub fn craters(&self) -> &Craters {
        &self.craters
    }
}

impl<S: HeightSource> HeightSource for CraterField<S> {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        self.base.height_at(x, z) + self.craters.delta_at(x, z)
    }
}

/// Integer bucket coordinate of a world position under a given cell size. `floor`
/// keeps the mapping continuous and identical on every platform (no rounding-mode
/// surprises).
#[inline]
fn cell_of(x: f64, z: f64, cell_size: f64) -> (i64, i64) {
    ((x / cell_size).floor() as i64, (z / cell_size).floor() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::HeightSource;

    /// Constant-height base so we can read the crater contribution directly.
    struct Flat(f64);
    impl HeightSource for Flat {
        fn height_at(&self, _x: f64, _z: f64) -> f64 {
            self.0
        }
    }

    fn crater(cx: f64, cz: f64, r: f64) -> Crater {
        Crater { center: [cx, cz], radius: r, depth: 2.0, rim_height: 0.4 }
    }

    #[test]
    fn empty_field_is_base() {
        let f = CraterField::new(Flat(7.0), vec![]);
        assert_eq!(f.height_at(0.0, 0.0), 7.0);
        assert_eq!(f.height_at(123.0, -456.0), 7.0);
    }

    #[test]
    fn center_is_depressed_rim_raised_far_flat() {
        let f = CraterField::new(Flat(0.0), vec![crater(0.0, 0.0, 10.0)]);
        assert!(f.height_at(0.0, 0.0) < -1.0, "floor should drop");
        assert!(f.height_at(10.0, 0.0) > 0.0, "rim lip should rise");
        // Beyond reach (1.6·r = 16 m) the field is exactly the base.
        assert_eq!(f.height_at(40.0, 40.0), 0.0);
    }

    #[test]
    fn deterministic() {
        let f = CraterField::new(Flat(1.0), vec![crater(3.0, -4.0, 8.0), crater(20.0, 5.0, 12.0)]);
        assert_eq!(f.height_at(2.5, -3.0), f.height_at(2.5, -3.0));
    }

    #[test]
    fn matches_direct_summation_regardless_of_bucketing() {
        // The bucket index is an optimisation: the result must equal a brute-force
        // sum over every crater, at every sampled point.
        let craters = vec![
            crater(0.0, 0.0, 10.0),
            crater(5.0, 3.0, 6.0),
            crater(-40.0, 25.0, 20.0),
            crater(100.0, -100.0, 4.0),
        ];
        let f = CraterField::new(Flat(2.0), craters.clone());
        for gx in -60..60 {
            for gz in -60..60 {
                let (x, z) = (gx as f64 * 2.3, gz as f64 * 2.3);
                let brute: f64 = 2.0 + craters.iter().map(|c| c.delta_at(x, z)).sum::<f64>();
                assert!(
                    (f.height_at(x, z) - brute).abs() < 1e-12,
                    "mismatch at ({x},{z}): {} vs {brute}",
                    f.height_at(x, z)
                );
            }
        }
    }

    #[test]
    fn overlapping_craters_accumulate() {
        // Two coincident craters deepen additively (the rasteriser's behaviour).
        let one = CraterField::new(Flat(0.0), vec![crater(0.0, 0.0, 10.0)]);
        let two = CraterField::new(Flat(0.0), vec![crater(0.0, 0.0, 10.0), crater(0.0, 0.0, 10.0)]);
        assert!((two.height_at(0.0, 0.0) - 2.0 * one.height_at(0.0, 0.0)).abs() < 1e-9);
    }

    #[test]
    fn profile_negligible_at_reach_delta_hard_zero_beyond() {
        // The raw profile's rim Gaussian has an infinite tail, so it is only
        // *negligibly* small at the reach, not exactly zero.
        assert!(crater_profile(1.6, 3.0, 0.5).abs() < 1e-6);
        assert!(crater_profile(2.0, 3.0, 0.5).abs() < 1e-6);
        // The hard cutoff is enforced by the reach test in `delta_at`, so the field
        // contribution is *exactly* zero at and beyond 1.6·r.
        let c = crater(0.0, 0.0, 10.0);
        assert_eq!(c.delta_at(16.0, 0.0), 0.0); // d = 1.6 exactly
        assert_eq!(c.delta_at(20.0, 0.0), 0.0); // d = 2.0
        // Floor is a deep depression, rim is positive.
        assert!(crater_profile(0.0, 3.0, 0.5) < -2.0);
        assert!(crater_profile(0.98, 0.0, 0.5) > 0.0);
    }

    #[test]
    fn continuous_across_bucket_boundaries() {
        // A crater straddling a cell edge must sample continuously — no seam where
        // the query crosses from one bucket to the next.
        let f = CraterField::new(Flat(0.0), vec![crater(0.0, 0.0, 30.0)]);
        let eps = 1e-4;
        // cell_size = reach = 48 m; walk across the x=0 axis and cell edges near it.
        for k in -100..100 {
            let x = k as f64 * 0.5;
            let d = (f.height_at(x + eps, 1.0) - f.height_at(x - eps, 1.0)).abs();
            assert!(d < 0.5, "discontinuity {d} at x={x}");
        }
    }
}
