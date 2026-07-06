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
//! results on every platform (fixed integer bucketing; the min/max overprint
//! combine is order-independent by construction).

use std::collections::HashMap;
use std::sync::Arc;

use crate::overzoom::nyquist_fade;
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
    /// Intrinsic profile blur (normalised by the rim radius): the crater's
    /// **degradation state**. `0` = fresh (razor rim lip); larger values round
    /// the rim/apron off exactly like coarse sampling does — micrometeorite
    /// gardening IS a low-pass filter on relief — folded in quadrature with the
    /// consumer's sampling kernel in [`Crater::delta_at_limited`]. A population
    /// with varied softness is what reads as a real surface; identical fresh
    /// profiles everywhere read as procedural stamping.
    pub softness: f64,
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
        self.delta_at_limited(x, z, 0.0)
    }

    /// Band-limited height delta for a consumer sampling every `min_wavelength`
    /// metres (`0` = exact profile). Two gates keep coarse sampling honest:
    ///
    /// - features narrower than the sampling kernel **widen** with conserved
    ///   volume (see [`crater_profile_limited`]) instead of hitting vertices at
    ///   random phases — the aliasing that rendered rim lips as sawtooth edges
    ///   and dotted rings;
    /// - a crater whose whole bowl falls below a couple of samples **fades out**
    ///   (same [`nyquist_fade`] policy as the over-zoom synthesiser).
    ///
    /// The contribution is also continuous at the reach cutoff: the profile's
    /// residual tail at [`CRATER_REACH`] is subtracted so the delta lands on
    /// exactly zero there. The old hard cut left a centimetre-scale circular
    /// ledge at `1.6·r` that read as a bright "ring line" under raking light.
    pub fn delta_at_limited(&self, x: f64, z: f64, min_wavelength: f64) -> f64 {
        let r = self.radius;
        if r <= 0.0 {
            return 0.0;
        }
        let fade = nyquist_fade(2.0 * r, min_wavelength);
        if fade <= 0.0 {
            return 0.0;
        }
        let dx = x - self.center[0];
        let dz = z - self.center[1];
        let d = (dx * dx + dz * dz).sqrt() / r; // normalised radial distance
        if d >= CRATER_REACH {
            return 0.0;
        }
        // Sampling kernel width, normalised by the rim radius (σ ≈ half the
        // sample spacing — the classic anti-alias kernel), combined in
        // quadrature with the crater's own degradation blur.
        let sample_sigma = 0.5 * min_wavelength / r;
        let sigma_n = (sample_sigma * sample_sigma + self.softness * self.softness).sqrt();
        let tail = crater_profile_limited(CRATER_REACH, self.depth, self.rim_height, sigma_n);
        fade * (crater_profile_limited(d, self.depth, self.rim_height, sigma_n) - tail)
    }
}

/// Rim-lip Gaussian: centre and width in normalised radial distance. The
/// narrowest crater feature — first to need band-limiting under coarse sampling.
const RIM_CENTER: f64 = 0.98;
const RIM_SIGMA: f64 = 0.14;
/// Ejecta-apron Gaussian: centre, width, and amplitude as a fraction of rim height.
const APRON_CENTER: f64 = 1.15;
const APRON_SIGMA: f64 = 0.30;
const APRON_FRAC: f64 = 0.25;

/// Gaussian bump `exp(−((d−c)/σ)²)`.
#[inline]
fn gauss(d: f64, center: f64, sigma: f64) -> f64 {
    (-((d - center) / sigma).powi(2)).exp()
}

/// Crater cross-section (metres) at normalised radial distance `d` (0 = centre,
/// 1 = rim radius). The `f64` canonical of `lunco-obstacle-field`'s `crater_delta`
/// — same profile, sampled instead of rasterised: a fairly flat floor (`1 − d⁴`
/// holds near max depth across the floor) turning UP into a steep inner wall, a
/// SHARP raised rim lip at `d≈1` (the key cue under raking light), then a low
/// outward ejecta apron peaking near `d≈1.15`.
#[inline]
pub fn crater_profile(d: f64, depth: f64, rim_height: f64) -> f64 {
    crater_profile_limited(d, depth, rim_height, 0.0)
}

/// Band-limited crater cross-section: the profile convolved — in closed form,
/// term by term — with a sampling kernel of width `sigma_n` (normalised by the
/// rim radius). A Gaussian of width `σ` blurred by `σₙ` widens to
/// `√(σ² + σₙ²)`; the amplitude falls **quadratically** (`(σ/σ′)²`). Linear
/// (1D volume-conserving) decay was wrong for the rim: it is a thin 2D
/// *annulus*, and a 2D blur with a kernel comparable to the ring radius spreads
/// its volume over area, not length — the linear rule left every unresolvable
/// crater as a broad positive swell, turning coarse-LOD crater fields into
/// bump-scapes under raking light. Quadratic decay lets the rim melt into the
/// bowl as it should. The bowl term (wide, sign-defining) is untouched — small
/// craters vanish via the whole-crater fade in [`Crater::delta_at_limited`].
/// `sigma_n = 0` is the exact profile. The Gaussian tails are never windowed
/// here — [`Crater::delta_at_limited`] subtracts the residual at
/// [`CRATER_REACH`] so the summed field cuts off continuously.
#[inline]
pub fn crater_profile_limited(d: f64, depth: f64, rim_height: f64, sigma_n: f64) -> f64 {
    let bowl = if d < 1.0 { -depth * (1.0 - d * d * d * d) } else { 0.0 };
    let rim_sigma = (RIM_SIGMA * RIM_SIGMA + sigma_n * sigma_n).sqrt();
    let apron_sigma = (APRON_SIGMA * APRON_SIGMA + sigma_n * sigma_n).sqrt();
    let rim_amp = (RIM_SIGMA / rim_sigma) * (RIM_SIGMA / rim_sigma);
    let apron_amp = (APRON_SIGMA / apron_sigma) * (APRON_SIGMA / apron_sigma);
    let rim = rim_height * rim_amp * gauss(d, RIM_CENTER, rim_sigma);
    let apron = rim_height * APRON_FRAC * apron_amp * gauss(d, APRON_CENTER, apron_sigma);
    bowl + rim + apron
}

/// A bucket-indexed set of craters — the crater contribution as a reusable
/// [`HeightModifier`](crate::modifier::HeightModifier), independent of any base. Fold
/// it onto a surface directly ([`CraterField`]) or stack it with other edits in a
/// [`LayeredHeightSource`](crate::modifier::LayeredHeightSource). Craters *within*
/// one set overprint (see [`Craters::delta_at`]); several `Craters` modifiers
/// (multiple crater layers) still accumulate in stack order.
#[derive(Debug, Clone)]
pub struct Craters {
    /// Shared placement index — Nyquist-gated variants (per-bake, one per tile
    /// LOD) are `Arc` clones of the same index, never a re-placement.
    index: Arc<CraterIndex>,
    /// Sampling wavelength (m) of the consumer this instance serves: features
    /// below it widen/fade instead of aliasing. `0` = full detail. Set per
    /// consumer via [`HeightModifier::with_min_wavelength`].
    ///
    /// [`HeightModifier::with_min_wavelength`]: crate::modifier::HeightModifier::with_min_wavelength
    min_wavelength: f64,
}

/// The immutable placement set + spatial bucket index behind [`Craters`].
#[derive(Debug)]
struct CraterIndex {
    /// The crater set (order only matters for bucket construction determinism —
    /// the per-point min/max combine is order-independent).
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
        Self {
            index: Arc::new(CraterIndex { craters, cell_size, buckets }),
            min_wavelength: 0.0,
        }
    }

    /// Number of craters.
    pub fn crater_count(&self) -> usize {
        self.index.craters.len()
    }

    /// Combined crater delta (metres) at `(x, z)`, band-limited to this
    /// instance's Nyquist gate. Overlapping craters **overprint** rather than
    /// add — the deepest bowl and the tallest rim at the point win:
    ///
    /// ```text
    /// delta = min(0, min_i d_i) + max(0, max_i d_i)
    /// ```
    ///
    /// A young impact cuts *through* older relief; summing deltas instead
    /// doubled bowl depth where bowls crossed and minted double-rim mounds
    /// inside craters ("two craters in one") at any realistic density. Min/max
    /// is also order-independent, so determinism needs no fixed walk order.
    pub fn delta_at(&self, x: f64, z: f64) -> f64 {
        let key = cell_of(x, z, self.index.cell_size);
        let Some(indices) = self.index.buckets.get(&key) else {
            return 0.0;
        };
        let mut deepest = 0.0_f64;
        let mut tallest = 0.0_f64;
        for &i in indices {
            let d = self.index.craters[i as usize].delta_at_limited(x, z, self.min_wavelength);
            deepest = deepest.min(d);
            tallest = tallest.max(d);
        }
        deepest + tallest
    }
}

impl crate::modifier::HeightModifier for Craters {
    fn apply(&self, x: f64, z: f64, h_in: f64) -> f64 {
        h_in + self.delta_at(x, z)
    }

    /// Craters ARE band-limitable: the rim lip (σ = 0.14·r) is far narrower than
    /// a coarse tile's vertex spacing, so an ungated crater renders as sawtooth
    /// rims and dotted rings on distant LODs. The gated variant shares the
    /// placement index (cheap `Arc` clone per bake).
    fn with_min_wavelength(
        &self,
        min_wavelength: f64,
    ) -> Option<Arc<dyn crate::modifier::HeightModifier>> {
        Some(Arc::new(Craters { index: self.index.clone(), min_wavelength }))
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
        Crater { center: [cx, cz], radius: r, depth: 2.0, rim_height: 0.4, softness: 0.0 }
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
    fn matches_direct_combine_regardless_of_bucketing() {
        // The bucket index is an optimisation: the result must equal a brute-force
        // min/max overprint combine over every crater, at every sampled point.
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
                let deepest = craters.iter().map(|c| c.delta_at(x, z)).fold(0.0_f64, f64::min);
                let tallest = craters.iter().map(|c| c.delta_at(x, z)).fold(0.0_f64, f64::max);
                let brute = 2.0 + deepest + tallest;
                assert!(
                    (f.height_at(x, z) - brute).abs() < 1e-12,
                    "mismatch at ({x},{z}): {} vs {brute}",
                    f.height_at(x, z)
                );
            }
        }
    }

    #[test]
    fn overlapping_craters_overprint_not_add() {
        // A young impact cuts through old relief: coincident craters must yield
        // the SAME bowl as one crater, not a doubled one ("two craters in one").
        let one = CraterField::new(Flat(0.0), vec![crater(0.0, 0.0, 10.0)]);
        let two = CraterField::new(Flat(0.0), vec![crater(0.0, 0.0, 10.0), crater(0.0, 0.0, 10.0)]);
        assert!((two.height_at(0.0, 0.0) - one.height_at(0.0, 0.0)).abs() < 1e-12);
        // Offset overlap: the point in both bowls takes the DEEPER contribution.
        let a = crater(0.0, 0.0, 10.0);
        let b = crater(8.0, 0.0, 10.0);
        let f = CraterField::new(Flat(0.0), vec![a, b]);
        let (x, z) = (4.0, 0.0);
        let expect = a.delta_at(x, z).min(b.delta_at(x, z)).min(0.0)
            + a.delta_at(x, z).max(b.delta_at(x, z)).max(0.0);
        assert!((f.height_at(x, z) - expect).abs() < 1e-12);
    }

    #[test]
    fn delta_continuous_at_reach_hard_zero_beyond() {
        // The contribution must land on exactly zero at the reach with no step —
        // a hard cut of the apron tail leaves a circular ledge that reads as a
        // "ring line" around every crater under raking light.
        let c = crater(0.0, 0.0, 10.0);
        assert!(c.delta_at(15.9999, 0.0).abs() < 1e-3, "no ledge just inside the reach");
        assert_eq!(c.delta_at(16.0, 0.0), 0.0); // d = 1.6 exactly
        assert_eq!(c.delta_at(20.0, 0.0), 0.0); // d = 2.0
        // Floor is a deep depression, rim is positive.
        assert!(crater_profile(0.0, 3.0, 0.5) < -2.0);
        assert!(crater_profile(0.98, 0.0, 0.5) > 0.0);
    }

    #[test]
    fn band_limited_rim_flattens_under_coarse_sampling() {
        let c = crater(0.0, 0.0, 10.0);
        let sharp = c.delta_at_limited(9.8, 0.0, 0.0); // at the rim lip
        let soft = c.delta_at_limited(9.8, 0.0, 8.0); // 8 m samples on a 10 m crater
        assert!(sharp > 0.3, "ungated lip stays sharp");
        assert!(soft < sharp * 0.5, "gated lip must widen/flatten, not alias");
        // Still continuous at the reach when gated.
        assert!(c.delta_at_limited(15.9999, 0.0, 8.0).abs() < 1e-3);
        assert_eq!(c.delta_at_limited(16.0, 0.0, 8.0), 0.0);
    }

    #[test]
    fn sub_sample_craters_fade_out_entirely() {
        // A bowl smaller than a sample cannot be represented — it must vanish,
        // not degenerate into single-vertex noise.
        let c = crater(0.0, 0.0, 5.0);
        assert_eq!(c.delta_at_limited(0.0, 0.0, 10.0), 0.0);
        assert_eq!(c.delta_at_limited(4.9, 0.0, 12.0), 0.0);
    }

    #[test]
    fn gated_modifier_variant_matches_ungated_at_zero() {
        use crate::modifier::HeightModifier;
        let cs = Craters::new(vec![crater(0.0, 0.0, 10.0), crater(15.0, -8.0, 6.0)]);
        let gated = cs.with_min_wavelength(0.0).expect("craters produce gated variants");
        for k in 0..40 {
            let (x, z) = (k as f64 * 0.7 - 14.0, k as f64 * 0.4 - 8.0);
            assert_eq!(gated.apply(x, z, 1.0), cs.apply(x, z, 1.0));
        }
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
