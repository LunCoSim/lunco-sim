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

use std::sync::{Arc, Mutex};

use crate::overzoom::nyquist_fade;
use crate::source::HeightSource;

/// Radial reach of a crater's influence, as a multiple of its radius. Beyond this
/// the [`crater_profile`] contribution is exactly zero (bowl ends at `d=1`, rim at
/// `d≈1`, ejecta apron at `d<1.6`). Matches the rasteriser's `radius * 1.6` reach.
pub const CRATER_REACH: f64 = 1.6;

/// Smallest rim radius (metres) the ANALYTIC crater field bothers to place, and
/// the floor the size-frequency population count is scaled against.
///
/// The SFD `N(>r) ∝ r^-1.8` means every halving of this floor roughly triples
/// the population, and the placements below it are the ones that cost most and
/// show least: a 2 m bowl is a couple of vertices even on the finest visual LOD,
/// is Nyquist-gated away on every LOD past the leaf ring, and is exactly the
/// band the [`Overzoom`](crate::overzoom::Overzoom) synthesiser already covers
/// procedurally — for free, at any resolution, with no placement to index. Let
/// the analytic field own the craters that need to be REAL geometry (colliders
/// agree with visuals, edits can address them) and let over-zoom own the carpet.
///
/// Consumers must apply this floor to BOTH the count scaling and the size draw,
/// or the population count stops describing the population.
pub const ANALYTIC_RADIUS_FLOOR_M: f64 = 4.0;

/// Ceiling on analytic craters per hectare after the SFD count scaling. The
/// authored `density` is a per-hectare rate for the AUTHORED size band; the SFD
/// scale-up can multiply it several-fold, and an authored value meant for a
/// sparse field then mints a population no sampling budget can carry. Bites only
/// pathological specs — the shipped defaults land near a quarter of it.
pub const MAX_CRATERS_PER_HA: f64 = 100.0;

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
    /// Bowl cross-section exponent `p` in `−depth·(1−dᵖ)`. `2` = paraboloid
    /// (fresh simple craters — continuously curving walls), larger = wider flat
    /// floor with a steep wall band (infilled/degraded morphology; the earlier
    /// profile was a fixed `4`). Tie it to the degradation state: a fresh sharp
    /// rim over a degraded flat floor is a strong "stamped" cue.
    pub bowl_power: f64,
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
        // Spatial reject FIRST — the bucket index hands every sample ~a dozen
        // candidates but only a couple are within reach, so the common case must
        // cost a few compares, not a divide + sqrt. (At 100k+ craters this loop
        // is the tile/collider bake inner loop.)
        let reach = r * CRATER_REACH;
        let dx = x - self.center[0];
        if dx >= reach || dx <= -reach {
            return 0.0;
        }
        let dz = z - self.center[1];
        if dz >= reach || dz <= -reach {
            return 0.0;
        }
        let d2 = dx * dx + dz * dz;
        if d2 >= reach * reach {
            return 0.0;
        }
        let fade = nyquist_fade(2.0 * r, min_wavelength);
        if fade <= 0.0 {
            return 0.0;
        }
        let d = d2.sqrt() / r; // normalised radial distance
                               // Sampling kernel width, normalised by the rim radius (σ ≈ half the
                               // sample spacing — the classic anti-alias kernel), combined in
                               // quadrature with the crater's own degradation blur.
        let sample_sigma = 0.5 * min_wavelength / r;
        let sigma_n = (sample_sigma * sample_sigma + self.softness * self.softness).sqrt();
        let tail = crater_profile_limited(
            CRATER_REACH,
            self.depth,
            self.rim_height,
            self.bowl_power,
            sigma_n,
        );
        fade * (crater_profile_limited(d, self.depth, self.rim_height, self.bowl_power, sigma_n)
            - tail)
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
/// 1 = rim radius). The canonical profile — `lunco-obstacle-field`'s
/// `crater_delta` is an f32 wrapper delegating here: a bowl `−depth·(1−dᵖ)`
/// (`bowl_power` p = 2 paraboloid fresh → larger = flat degraded floor) turning
/// UP into the inner wall, a SHARP raised rim lip at `d≈1` (the key cue under
/// raking light), then a low outward ejecta apron peaking near `d≈1.15`.
#[inline]
pub fn crater_profile(d: f64, depth: f64, rim_height: f64, bowl_power: f64) -> f64 {
    crater_profile_limited(d, depth, rim_height, bowl_power, 0.0)
}

/// [`crater_profile`] with the rim lip widened to at least `rim_sigma_n`
/// (normalised by the rim radius) at **full height** — the opposite trade from
/// [`crater_profile_limited`]'s quadratic melt, for the opposite regime:
/// `crater_profile_limited` serves craters whose whole ring may be unresolvable
/// (a full-height rim there smears into a broad swell, so it melts); this serves
/// craters whose bowl IS resolved while only the thin lip (σ = `RIM_SIGMA`·r)
/// falls between sample points — widening the lip to the sampling width keeps it
/// a sharp, representable ring instead of aliasing away.
///
/// The width clamps to `[RIM_SIGMA, 0.35]`: at 0 it is exactly
/// [`crater_profile`]; wider than 0.35 would itself read as a swell.
#[inline]
pub fn crater_profile_rim_limited(
    d: f64,
    depth: f64,
    rim_height: f64,
    bowl_power: f64,
    rim_sigma_n: f64,
) -> f64 {
    let bowl = if d < 1.0 {
        -depth * (1.0 - d.powf(bowl_power))
    } else {
        0.0
    };
    let rim_sigma = rim_sigma_n.clamp(RIM_SIGMA, 0.35);
    let rim = rim_height * gauss(d, RIM_CENTER, rim_sigma);
    let apron = rim_height * APRON_FRAC * gauss(d, APRON_CENTER, APRON_SIGMA);
    bowl + rim + apron
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
pub fn crater_profile_limited(
    d: f64,
    depth: f64,
    rim_height: f64,
    bowl_power: f64,
    sigma_n: f64,
) -> f64 {
    let bowl = if d < 1.0 {
        -depth * (1.0 - d.powf(bowl_power))
    } else {
        0.0
    };
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
    /// Shared placement index. Nyquist-gated variants (one per tile LOD, plus
    /// the contact and derived-map bands) are memoised PRUNED indexes — never a
    /// re-placement, and never the whole population re-walked per bake.
    index: Arc<CraterIndex>,
    /// Sampling wavelength (m) of the consumer this instance serves: features
    /// below it widen/fade instead of aliasing. `0` = full detail. Set per
    /// consumer via [`HeightModifier::with_min_wavelength`].
    ///
    /// [`HeightModifier::with_min_wavelength`]: crate::modifier::HeightModifier::with_min_wavelength
    min_wavelength: f64,
}

/// Number of radius-octave strata the overprint combine distinguishes, and the
/// octave (log₂ radius) mapped to stratum 0. Radii from 0.25 m up to 8 km land in
/// distinct strata; anything outside clamps to the nearest end.
const OCTAVE_BASE: i32 = -2;
const OCTAVE_COUNT: usize = 16;

/// Radius-octave stratum of a crater: same-scale craters overprint, craters an
/// octave apart superpose (see [`Craters::delta_at`]).
///
/// The stratum is `⌊log₂ radius⌋`, but computed **bit-exactly** from the f64's
/// binary exponent rather than `log2().floor()`. `log2` is not in the IEEE-754
/// correctly-rounded set, so a radius within a ULP of a power-of-two boundary
/// could floor to different octaves on x86 / ARM / wasm — regrouping the discrete
/// overprint strata and diverging collider heights + content hashes across peers
/// (a structural break the `quantize` firewall cannot repair). `radius.max(1e-9)`
/// is always a normal f64, so the biased-exponent field is exactly `⌊log₂ r⌋`.
#[inline]
fn octave_of(radius: f64) -> usize {
    let r = radius.max(1e-9);
    let exp2 = ((r.to_bits() >> 52) & 0x7ff) as i32 - 1023; // ⌊log₂ r⌋, bit-exact
    (exp2 - OCTAVE_BASE).clamp(0, OCTAVE_COUNT as i32 - 1) as usize
}

/// Soft ceiling on the dense cell count of ONE octave grid. Each grid's cell
/// size is derived from that octave's largest reach, so a stratum of tiny
/// craters spread over kilometres would otherwise mint millions of cells;
/// doubling that grid's cell size until it fits is output-neutral (bucketing
/// only decides which candidates a sample *considers* — out-of-reach candidates
/// contribute exactly zero either way).
const MAX_BUCKET_CELLS: u128 = 1 << 21;

/// Cells per crater one grid may spend before the doubling kicks in. Keeps grid
/// memory O(craters) instead of O(area / smallest reach²) — the growth that made
/// the smallest octaves unaffordable once the size-frequency distribution filled
/// them.
const CELLS_PER_CRATER: u128 = 4;

/// Largest world coordinate / reach a crater may carry (metres). Beyond it — and for
/// any non-finite value — the cell index saturates and the CSR build panics; such a
/// crater is simply not bucketed (it contributes nothing). Mirrors the same guard in
/// [`crate::carve`].
const MAX_COORD: f64 = 1e12;

/// How many Nyquist-gated variants one index memoises before the cache resets.
/// A terrain serves one gate per LOD depth plus the collider and derived-map
/// bands — a handful.
const MAX_GATED_VARIANTS: usize = 24;

/// The cell box a crater's reach bbox covers at `cell_size`, or `None` for a
/// crater that must not be bucketed at all.
///
/// A non-finite (or absurd) reach/centre — an authored divide-by-zero —
/// saturates `(x / cell) as i64` to `i64::MIN/MAX`, whose span overflows the CSR
/// sizing (debug panic) or wraps to a zero-sized grid the fill loop then indexes
/// out of bounds (release panic). Such a crater is simply never bucketed, so it
/// is never sampled either: it contributes nothing.
#[inline]
fn cell_box(c: &Crater, inv_cell: f64) -> Option<(i64, i64, i64, i64)> {
    let reach = c.reach();
    if !reach.is_finite() // NaN / ±inf
        || reach <= 0.0
        || reach > MAX_COORD
        || !c.center.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD)
    {
        return None;
    }
    let (min_cx, min_cz) = cell_of(c.center[0] - reach, c.center[1] - reach, inv_cell);
    let (max_cx, max_cz) = cell_of(c.center[0] + reach, c.center[1] + reach, inv_cell);
    Some((min_cx, min_cz, max_cx, max_cz))
}

/// One radius octave's dense row-major CSR bucket grid.
///
/// **Why per octave.** A single grid must size its cell to the LARGEST crater's
/// reach or the biggest bowl spans unboundedly many cells. But the population is
/// a power law: the overwhelming majority are near the size floor, and with one
/// global grid each of those tiny craters is looked up in a cell two orders of
/// magnitude wider than its own footprint — so every sample walked ~all craters
/// within `max_reach²` instead of ~all within its own reach². That single fact
/// is what made a dense analytic crater field too slow to ship. Stratifying by
/// [`octave_of`] (which the per-point combine already stratifies by) lets each
/// grid pick a cell matched to ITS craters, and a query does one O(1) lookup per
/// occupied octave.
///
/// Cell `(cx, cz)` holds entries `starts[k]..starts[k + 1]`
/// (`k = (cz − min.1)·nx + (cx − min.0)`). A crater is inserted into every cell
/// its `[center ± reach]` box touches, so the single cell containing a query
/// point holds every crater of this octave that can affect it — one lookup, no
/// neighbour scan. Queries outside the AABB fall back to empty.
///
/// Entries are **SoA and inlined**, not indices into `craters`: the reject test
/// needs only `(center, reach)` but a `Vec<Crater>` gather pulled a 56-byte
/// struct through cache for each of the ~98 % of candidates that reject. Three
/// contiguous `f64` runs vectorise; only survivors touch the full [`Crater`] via
/// `idx`.
#[derive(Debug)]
struct OctaveGrid {
    /// Metres per cell — this octave's largest reach (then doubled to fit).
    /// `1 / cell_size`, stored so the per-sample lookup multiplies instead of
    /// dividing (a query hits one cell per occupied octave — the divisions added
    /// up). Insertion uses the SAME reciprocal, so the partition stays
    /// consistent; see [`cell_of`].
    inv_cell_size: f64,
    /// Cell coordinate of grid slot `(0, 0)`.
    min: (i64, i64),
    /// Grid dimensions (cells).
    nx: usize,
    nz: usize,
    /// CSR row offsets (`nx·nz + 1` entries).
    starts: Vec<u32>,
    /// Per-entry crater centre X / centre Z / reach — the reject-test hot arrays.
    cx: Vec<f64>,
    cz: Vec<f64>,
    reach: Vec<f64>,
    /// Per-entry index into [`CraterIndex::craters`] for the survivors.
    idx: Vec<u32>,
}

impl OctaveGrid {
    /// Build the grid for `members` (indices into `craters`, ascending).
    /// `None` if no member is bucketable.
    fn build(craters: &[Crater], members: &[u32]) -> Option<OctaveGrid> {
        // Cell just big enough that this octave's biggest crater spans a bounded
        // 3×3. Only SANE reaches size it (a non-finite one would make it `inf`).
        let max_reach = members
            .iter()
            .map(|&i| craters[i as usize].reach())
            .filter(|r| r.is_finite() && *r <= MAX_COORD)
            .fold(0.0_f64, f64::max);
        if max_reach <= 0.0 {
            return None;
        }
        let mut cell_size = max_reach;
        let mut inv_cell = 1.0 / cell_size;
        // Cells this grid may spend: O(members), never past the hard ceiling.
        let budget = ((members.len() as u128) * CELLS_PER_CRATER).clamp(1024, MAX_BUCKET_CELLS);
        let (min, nx, nz) = loop {
            let (mut min_cx, mut min_cz) = (i64::MAX, i64::MAX);
            let (mut max_cx, mut max_cz) = (i64::MIN, i64::MIN);
            for &i in members {
                let Some((x0, z0, x1, z1)) = cell_box(&craters[i as usize], inv_cell) else {
                    continue;
                };
                min_cx = min_cx.min(x0);
                min_cz = min_cz.min(z0);
                max_cx = max_cx.max(x1);
                max_cz = max_cz.max(z1);
            }
            if min_cx > max_cx {
                return None; // no member with a bucketable reach
            }
            // i128 so a saturated span can never overflow the subtraction.
            let nx = (max_cx as i128 - min_cx as i128 + 1) as u128;
            let nz = (max_cz as i128 - min_cz as i128 + 1) as u128;
            if nx * nz <= budget {
                break ((min_cx, min_cz), nx as usize, nz as usize);
            }
            cell_size *= 2.0;
            inv_cell = 1.0 / cell_size;
        };
        // CSR fill: count per cell, prefix-sum into starts, then place entries in
        // ascending crater order per cell.
        let cells = nx * nz;
        let slot =
            |cx: i64, cz: i64| -> usize { (cz - min.1) as usize * nx + (cx - min.0) as usize };
        let mut counts = vec![0u32; cells];
        for &i in members {
            let Some((x0, z0, x1, z1)) = cell_box(&craters[i as usize], inv_cell) else {
                continue;
            };
            for cz in z0..=z1 {
                for cx in x0..=x1 {
                    counts[slot(cx, cz)] += 1;
                }
            }
        }
        let mut starts = vec![0u32; cells + 1];
        for k in 0..cells {
            starts[k + 1] = starts[k] + counts[k];
        }
        let total = starts[cells] as usize;
        let mut cursor: Vec<u32> = starts[..cells].to_vec();
        let mut grid = OctaveGrid {
            inv_cell_size: inv_cell,
            min,
            nx,
            nz,
            starts,
            cx: vec![0.0; total],
            cz: vec![0.0; total],
            reach: vec![0.0; total],
            idx: vec![0; total],
        };
        for &i in members {
            let c = &craters[i as usize];
            let Some((x0, z0, x1, z1)) = cell_box(c, inv_cell) else {
                continue;
            };
            for cz in z0..=z1 {
                for cx in x0..=x1 {
                    let k = slot(cx, cz);
                    let e = cursor[k] as usize;
                    grid.cx[e] = c.center[0];
                    grid.cz[e] = c.center[1];
                    grid.reach[e] = c.reach();
                    grid.idx[e] = i;
                    cursor[k] += 1;
                }
            }
        }
        Some(grid)
    }

    /// Entry range of the cell containing `(x, z)` — empty outside the AABB.
    #[inline]
    fn entries_at(&self, x: f64, z: f64) -> std::ops::Range<usize> {
        let (qx, qz) = cell_of(x, z, self.inv_cell_size);
        let ux = qx.wrapping_sub(self.min.0);
        let uz = qz.wrapping_sub(self.min.1);
        if ux < 0 || uz < 0 || ux >= self.nx as i64 || uz >= self.nz as i64 {
            return 0..0;
        }
        let k = uz as usize * self.nx + ux as usize;
        self.starts[k] as usize..self.starts[k + 1] as usize
    }

    /// Entry ranges of every cell overlapping the world box `[min, max]`, as a
    /// cell-coordinate span clamped to the grid AABB (`None` = no overlap).
    fn cell_span(&self, min: [f64; 2], max: [f64; 2]) -> Option<(usize, usize, usize, usize)> {
        let (x0, z0) = cell_of(min[0], min[1], self.inv_cell_size);
        let (x1, z1) = cell_of(max[0], max[1], self.inv_cell_size);
        let lo_x = x0.max(self.min.0).wrapping_sub(self.min.0);
        let lo_z = z0.max(self.min.1).wrapping_sub(self.min.1);
        let hi_x = x1
            .min(self.min.0 + self.nx as i64 - 1)
            .wrapping_sub(self.min.0);
        let hi_z = z1
            .min(self.min.1 + self.nz as i64 - 1)
            .wrapping_sub(self.min.1);
        if lo_x > hi_x || lo_z > hi_z || hi_x < 0 || hi_z < 0 {
            return None;
        }
        Some((
            lo_x.max(0) as usize,
            lo_z.max(0) as usize,
            hi_x as usize,
            hi_z as usize,
        ))
    }
}

/// Everything [`Crater::delta_at_limited`] recomputes per sample that depends
/// only on `(crater, gate)`: the Nyquist fade, both blurred Gaussian widths and
/// their amplitudes folded into the height factors, and the reach `tail` — which
/// is a **whole second profile evaluation** (2 `exp` + a `powf`) the old hot path
/// paid on every hit. The breakdown report put 78 % of a leaf-pitch sample in
/// profile math at ≈39 ns per in-reach crater; this is what that pays for.
///
/// One record, not a side array: an earlier attempt hoisted the same constants
/// into a `Vec` parallel to `craters` and measured *slower*, because a hit then
/// took two random accesses instead of one. Here the prepared record REPLACES
/// the `Crater` load in the hot path, so the stream count is unchanged.
///
/// Every field is grouped exactly as [`crater_profile_limited`] groups it, so
/// the fast path is bit-for-bit the slow one (pinned by
/// `band_limited_prune_is_bitwise_identical_to_the_unpruned_gate`, which now
/// compares a prepared index against an un-prepared reference).
#[derive(Debug, Clone, Copy)]
struct Prepared {
    radius: f64,
    depth: f64,
    bowl_power: f64,
    rim_sigma: f64,
    /// `rim_height * rim_amp`.
    rim_k: f64,
    apron_sigma: f64,
    /// `(rim_height * APRON_FRAC) * apron_amp`.
    apron_k: f64,
    /// The profile's residual at [`CRATER_REACH`], subtracted for continuity.
    tail: f64,
    /// Whole-crater Nyquist fade at this gate (`0` = contributes nothing).
    fade: f64,
}

impl Prepared {
    fn new(c: &Crater, min_wavelength: f64) -> Prepared {
        let r = c.radius;
        let fade = if r > 0.0 {
            nyquist_fade(2.0 * r, min_wavelength)
        } else {
            0.0
        };
        let sample_sigma = 0.5 * min_wavelength / r;
        let sigma_n = (sample_sigma * sample_sigma + c.softness * c.softness).sqrt();
        let rim_sigma = (RIM_SIGMA * RIM_SIGMA + sigma_n * sigma_n).sqrt();
        let apron_sigma = (APRON_SIGMA * APRON_SIGMA + sigma_n * sigma_n).sqrt();
        let rim_amp = (RIM_SIGMA / rim_sigma) * (RIM_SIGMA / rim_sigma);
        let apron_amp = (APRON_SIGMA / apron_sigma) * (APRON_SIGMA / apron_sigma);
        Prepared {
            radius: r,
            depth: c.depth,
            bowl_power: c.bowl_power,
            rim_sigma,
            rim_k: c.rim_height * rim_amp,
            apron_sigma,
            apron_k: c.rim_height * APRON_FRAC * apron_amp,
            tail: crater_profile_limited(
                CRATER_REACH,
                c.depth,
                c.rim_height,
                c.bowl_power,
                sigma_n,
            ),
            fade,
        }
    }

    /// Height delta at squared distance `d2` from the centre — the caller
    /// already computed it for the reach reject, so it is never recomputed here.
    /// In-reach only: the caller rejects outside.
    #[inline]
    fn delta_at(&self, d2: f64) -> f64 {
        if self.fade.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return 0.0;
        }
        let d = d2.sqrt() / self.radius;
        let bowl = if d < 1.0 {
            -self.depth * (1.0 - d.powf(self.bowl_power))
        } else {
            0.0
        };
        let rim = self.rim_k * gauss(d, RIM_CENTER, self.rim_sigma);
        let apron = self.apron_k * gauss(d, APRON_CENTER, self.apron_sigma);
        self.fade * (bowl + rim + apron - self.tail)
    }
}

/// The immutable placement set + per-octave spatial index behind [`Craters`].
#[derive(Debug)]
struct CraterIndex {
    /// The crater set (order only matters for bucket construction determinism —
    /// the per-point min/max combine is order-independent).
    craters: Vec<Crater>,
    /// Per-crater constants baked for [`gate`](CraterIndex::gate), parallel to
    /// `craters`. Sampling at any OTHER gate falls back to `craters`.
    prepared: Vec<Prepared>,
    /// The Nyquist gate `prepared` was baked for. An index is always built for
    /// one band (see [`Craters::band_limited`]); this makes that structural
    /// rather than assumed, so a hand-built `Craters` at a mismatched gate is
    /// slow but never wrong.
    gate: f64,
    /// One grid per OCCUPIED radius octave, in ascending octave order — the
    /// order the combine sums in.
    grids: Vec<OctaveGrid>,
    /// Memoised Nyquist-gated sub-indexes, keyed by `min_wavelength.to_bits()`.
    ///
    /// Gating is a real PRUNE (see [`Craters::band_limited`]), and composing a
    /// terrain re-derives the same handful of gates — one per LOD depth, plus
    /// the contact and derived-map bands — for every tile and collider bake.
    /// Building the pruned index once per gate instead of once per bake is the
    /// difference between "the coarse LODs are free" and "every coarse tile
    /// re-walks the whole small-crater carpet to reject it".
    gated: Mutex<Vec<(u64, Arc<CraterIndex>)>>,
}

impl CraterIndex {
    /// Stratify by radius octave, build one grid per occupied stratum, and bake
    /// the per-crater constants for `gate`.
    fn build(craters: Vec<Crater>, gate: f64) -> CraterIndex {
        let mut members: Vec<Vec<u32>> = vec![Vec::new(); OCTAVE_COUNT];
        for (i, c) in craters.iter().enumerate() {
            members[octave_of(c.radius)].push(i as u32);
        }
        let grids = members
            .iter()
            .filter(|m| !m.is_empty())
            .filter_map(|m| OctaveGrid::build(&craters, m))
            .collect();
        let prepared = craters.iter().map(|c| Prepared::new(c, gate)).collect();
        CraterIndex {
            craters,
            prepared,
            gate,
            grids,
            gated: Mutex::new(Vec::new()),
        }
    }
}

impl Craters {
    /// Build the spatial index (one bucket grid per radius octave). An empty set
    /// contributes nothing.
    pub fn new(craters: Vec<Crater>) -> Self {
        Self {
            index: Arc::new(CraterIndex::build(craters, 0.0)),
            min_wavelength: 0.0,
        }
    }

    /// Number of craters **in this variant** — a gated or region-scoped variant
    /// (see [`band_limited`] / [`for_region`]) reports only what it kept.
    ///
    /// [`band_limited`]: Craters::band_limited
    /// [`for_region`]: Craters::for_region
    pub fn crater_count(&self) -> usize {
        self.index.craters.len()
    }

    /// This field's Nyquist gate (metres); `0` = full detail.
    pub fn min_wavelength(&self) -> f64 {
        self.min_wavelength
    }

    /// A variant gated for a consumer sampling every `min_wavelength` metres,
    /// with every crater the gate would zero out **removed from the index**.
    ///
    /// [`nyquist_fade`] returns exactly `0` for `2·radius ≤ min_wavelength`, so
    /// dropping those craters cannot change a single sampled value — but it is
    /// the difference between a 64 m-pitch tile walking the entire sub-metre
    /// carpet to reject it per sample and never seeing it at all. Results are
    /// memoised on the root index (see [`CraterIndex::gated`]), so the pruned
    /// index for a given band is built once per terrain, not once per bake.
    pub fn band_limited(&self, min_wavelength: f64) -> Craters {
        // No gate → nothing is zeroed, so nothing may be dropped (and the
        // ungated index is the one we already hold).
        if min_wavelength.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return Craters {
                index: self.index.clone(),
                min_wavelength: 0.0,
            };
        }
        let key = min_wavelength.to_bits();
        {
            let cache = self.index.gated.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((_, idx)) = cache.iter().find(|(k, _)| *k == key) {
                return Craters {
                    index: idx.clone(),
                    min_wavelength,
                };
            }
        }
        let kept: Vec<Crater> = self
            .index
            .craters
            .iter()
            .copied()
            .filter(|c| 2.0 * c.radius > min_wavelength)
            .collect();
        let index = Arc::new(CraterIndex::build(kept, min_wavelength));
        let mut cache = self.index.gated.lock().unwrap_or_else(|e| e.into_inner());
        // Live slider-drag tuning mints a distinct band per value; cap so a
        // session of tweaking cannot grow this without bound.
        if cache.len() >= MAX_GATED_VARIANTS {
            cache.clear();
        }
        cache.push((key, index.clone()));
        Craters {
            index,
            min_wavelength,
        }
    }

    /// A compact variant holding only the craters that can affect the world box
    /// `[min, max]` at `min_wavelength` — the form every REGION-scoped consumer
    /// (tile bake, collider tile, derived-map bake) should sample.
    ///
    /// A bake knows its footprint and its pitch up front, so the per-sample loop
    /// has no business rediscovering "which craters are near?" tens of thousands
    /// of times. Gathering once turns the inner loop into a walk over a handful
    /// of contiguous entries that stay in L1, and lets a coarse tile drop the
    /// whole small-crater population in one pass.
    ///
    /// Sampled values are **identical to the full field anywhere inside
    /// `[min, max]`** (dropped craters are out of reach or Nyquist-zeroed there,
    /// so they contribute exactly `0.0`); outside it they are not — this is a
    /// bake-scoped view, never a replacement for the field.
    pub fn for_region(&self, min: [f64; 2], max: [f64; 2], min_wavelength: f64) -> Craters {
        let gated = self.band_limited(min_wavelength);
        let index = &gated.index;
        // Gather from the grids rather than scanning the placement list: a leaf
        // tile covers a few cells of each octave, and the scan is otherwise
        // O(population) per tile bake.
        let mut seen = vec![0u64; index.craters.len().div_ceil(64)];
        let mut kept: Vec<Crater> = Vec::new();
        for g in &index.grids {
            let Some((lo_x, lo_z, hi_x, hi_z)) = g.cell_span(min, max) else {
                continue;
            };
            for uz in lo_z..=hi_z {
                for ux in lo_x..=hi_x {
                    let k = uz * g.nx + ux;
                    for e in g.starts[k] as usize..g.starts[k + 1] as usize {
                        let reach = g.reach[e];
                        // Reach bbox vs the region box — a cell overlapping the
                        // region does not mean the crater in it does.
                        if g.cx[e] + reach < min[0]
                            || g.cx[e] - reach > max[0]
                            || g.cz[e] + reach < min[1]
                            || g.cz[e] - reach > max[1]
                        {
                            continue;
                        }
                        let i = g.idx[e] as usize;
                        let (w, b) = (i / 64, 1u64 << (i % 64));
                        if seen[w] & b != 0 {
                            continue; // already gathered from another cell
                        }
                        seen[w] |= b;
                        kept.push(index.craters[i]);
                    }
                }
            }
        }
        Craters {
            index: Arc::new(CraterIndex::build(kept, min_wavelength)),
            min_wavelength,
        }
    }

    /// Combined crater delta (metres) at `(x, z)`, band-limited to this
    /// instance's Nyquist gate. **Same-scale** overlapping craters overprint —
    /// within each radius octave the deepest bowl and the tallest rim at the
    /// point win — while **octaves superpose**:
    ///
    /// ```text
    /// delta = Σ_octave [ min(0, min_i d_i) + max(0, max_i d_i) ]
    /// ```
    ///
    /// A young impact cuts *through* comparable older relief; summing same-scale
    /// deltas doubled bowl depth where bowls crossed and minted double-rim
    /// mounds inside craters ("two craters in one"). But a global min/max erased
    /// every SMALL crater inside a big bowl (the big negative won the `min`, so
    /// only the small rim survived — floating rings on crater floors), when a
    /// real saturated surface is exactly big floors pockmarked by small bowls:
    /// scale-separated impacts are physically additive. Per-octave min/max +
    /// cross-octave sum gives both, stays order-independent, and needs no fixed
    /// walk order for determinism.
    /// Each octave is its own grid, so the walk is one O(1) cell lookup per
    /// occupied stratum and the min/max pair lives in registers instead of a
    /// 16-wide scratch array. Summation order is unchanged (ascending octave); a
    /// stratum with no candidate here would have contributed an exact
    /// `0.0 + 0.0`, so skipping it leaves every bit identical.
    pub fn delta_at(&self, x: f64, z: f64) -> f64 {
        // The index bakes its per-crater constants for ONE gate; sampling at
        // that gate (every path that goes through `band_limited`/`for_region`,
        // i.e. every bake) takes the prepared record, anything else recomputes.
        let prepared = self.min_wavelength.to_bits() == self.index.gate.to_bits();
        let mut sum = 0.0;
        for g in &self.index.grids {
            let entries = g.entries_at(x, z);
            if entries.is_empty() {
                continue;
            }
            let (mut deepest, mut tallest) = (0.0_f64, 0.0_f64);
            for e in entries {
                // Reject FIRST, off the contiguous SoA arrays: the bucket hands
                // us candidates whose CELL overlaps, and only a couple are
                // within reach. Rejecting here keeps the 56-byte `Crater` (and
                // its transcendentals) out of the common path entirely.
                let reach = g.reach[e];
                let dx = x - g.cx[e];
                if dx >= reach || dx <= -reach {
                    continue;
                }
                let dz = z - g.cz[e];
                if dz >= reach || dz <= -reach {
                    continue;
                }
                let d2 = dx * dx + dz * dz;
                if d2 >= reach * reach {
                    continue;
                }
                let i = g.idx[e] as usize;
                let d = if prepared {
                    self.index.prepared[i].delta_at(d2)
                } else {
                    self.index.craters[i].delta_at_limited(x, z, self.min_wavelength)
                };
                if d == 0.0 {
                    continue;
                }
                deepest = deepest.min(d);
                tallest = tallest.max(d);
            }
            sum += deepest + tallest;
        }
        sum
    }
}

impl crate::modifier::HeightModifier for Craters {
    fn apply(&self, x: f64, z: f64, h_in: f64) -> f64 {
        h_in + self.delta_at(x, z)
    }

    /// Craters ARE band-limitable: the rim lip (σ = 0.14·r) is far narrower than
    /// a coarse tile's vertex spacing, so an ungated crater renders as sawtooth
    /// rims and dotted rings on distant LODs. The gated variant is a memoised
    /// PRUNED index — craters the gate zeroes are dropped, not rejected per
    /// sample (see [`Craters::band_limited`]).
    fn with_min_wavelength(
        &self,
        min_wavelength: f64,
    ) -> Option<Arc<dyn crate::modifier::HeightModifier>> {
        Some(Arc::new(self.band_limited(min_wavelength)))
    }

    /// Craters are region-scopable: a bake gathers the craters over its own
    /// footprint once instead of resolving "which craters are near?" per sample
    /// (see [`Craters::for_region`]).
    fn for_region(
        &self,
        min: [f64; 2],
        max: [f64; 2],
        min_wavelength: f64,
    ) -> Option<Arc<dyn crate::modifier::HeightModifier>> {
        Some(Arc::new(self.for_region(min, max, min_wavelength)))
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
        Self {
            base,
            craters: Craters::new(craters),
        }
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

/// Integer bucket coordinate of a world position, given the RECIPROCAL of the
/// cell size. `floor` keeps the mapping continuous and identical on every
/// platform (no rounding-mode surprises), and both IEEE multiply and the
/// reciprocal are correctly rounded, so the partition is peer-identical.
///
/// It takes `1/cell` rather than `cell` because this is the per-sample entry
/// point: a query touches one cell per occupied radius octave, so a division
/// here is ~6 divisions per height sample. `x * inv` is NOT `x / cell` to the
/// last bit — which is fine and invisible, because bucketing only PARTITIONS.
/// The one hard requirement is that insertion and query use the SAME mapping;
/// they do (both go through here, off the grid's stored `inv_cell_size`).
#[inline]
fn cell_of(x: f64, z: f64, inv_cell: f64) -> (i64, i64) {
    ((x * inv_cell).floor() as i64, (z * inv_cell).floor() as i64)
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
        Crater {
            center: [cx, cz],
            radius: r,
            depth: 2.0,
            rim_height: 0.4,
            softness: 0.0,
            bowl_power: 4.0,
        }
    }

    /// Brute-force reference of the octave-stratified overprint combine.
    fn brute_combine(craters: &[Crater], x: f64, z: f64) -> f64 {
        let mut deepest = [0.0_f64; OCTAVE_COUNT];
        let mut tallest = [0.0_f64; OCTAVE_COUNT];
        for c in craters {
            let d = c.delta_at(x, z);
            let o = octave_of(c.radius);
            deepest[o] = deepest[o].min(d);
            tallest[o] = tallest[o].max(d);
        }
        deepest.iter().sum::<f64>() + tallest.iter().sum::<f64>()
    }

    #[test]
    fn non_finite_craters_do_not_panic_the_index() {
        // An authored divide-by-zero (`radius = 1/0`, an `inf`/`NaN` centre) used to
        // saturate the cell index → CSR span overflow (debug) / OOB fill (release).
        // Such craters are not bucketed; the finite ones still stamp.
        let mut bad = crater(0.0, 0.0, f64::INFINITY);
        let mut nan = crater(f64::NAN, 0.0, 10.0);
        nan.depth = f64::NAN;
        bad.rim_height = f64::INFINITY;
        let huge = crater(1e300, 1e300, 1e300);
        let good = crater(0.0, 0.0, 20.0);
        let f = CraterField::new(Flat(5.0), vec![bad, nan, huge, good]);
        // The good crater still depresses its centre; the bad ones contribute nothing.
        assert!(f.height_at(0.0, 0.0) < 5.0);
        assert_eq!(f.height_at(5000.0, 5000.0), 5.0);
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
        let f = CraterField::new(
            Flat(1.0),
            vec![crater(3.0, -4.0, 8.0), crater(20.0, 5.0, 12.0)],
        );
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
                let brute = 2.0 + brute_combine(&craters, x, z);
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
        // A young impact cuts through comparable old relief: coincident
        // SAME-SCALE craters must yield the SAME bowl as one crater, not a
        // doubled one ("two craters in one").
        let one = CraterField::new(Flat(0.0), vec![crater(0.0, 0.0, 10.0)]);
        let two = CraterField::new(
            Flat(0.0),
            vec![crater(0.0, 0.0, 10.0), crater(0.0, 0.0, 10.0)],
        );
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
    fn small_crater_survives_inside_large_bowl() {
        // Scale-separated impacts superpose: a 2 m crater on a 30 m crater's
        // floor must still dig its own bowl (a global min/max erased it — the
        // big bowl won the `min`, leaving only the small rim as a floating ring).
        let big = crater(0.0, 0.0, 30.0);
        let small = crater(6.0, 0.0, 2.0);
        let with = CraterField::new(Flat(0.0), vec![big, small]);
        let without = CraterField::new(Flat(0.0), vec![big]);
        let dug = without.height_at(6.0, 0.0) - with.height_at(6.0, 0.0);
        assert!(
            dug > small.depth * 0.5,
            "small bowl should deepen the big floor by ~its own depth, dug {dug}"
        );
        // …and its rim rises RELATIVE to the local big-bowl floor.
        let rim = with.height_at(8.0, 0.0) - without.height_at(8.0, 0.0);
        assert!(
            rim > 0.0,
            "small rim should ride on the big floor, got {rim}"
        );
    }

    #[test]
    fn delta_continuous_at_reach_hard_zero_beyond() {
        // The contribution must land on exactly zero at the reach with no step —
        // a hard cut of the apron tail leaves a circular ledge that reads as a
        // "ring line" around every crater under raking light.
        let c = crater(0.0, 0.0, 10.0);
        assert!(
            c.delta_at(15.9999, 0.0).abs() < 1e-3,
            "no ledge just inside the reach"
        );
        assert_eq!(c.delta_at(16.0, 0.0), 0.0); // d = 1.6 exactly
        assert_eq!(c.delta_at(20.0, 0.0), 0.0); // d = 2.0
                                                // Floor is a deep depression, rim is positive.
        assert!(crater_profile(0.0, 3.0, 0.5, 4.0) < -2.0);
        assert!(crater_profile(0.98, 0.0, 0.5, 4.0) > 0.0);
    }

    #[test]
    fn band_limited_rim_flattens_under_coarse_sampling() {
        let c = crater(0.0, 0.0, 10.0);
        let sharp = c.delta_at_limited(9.8, 0.0, 0.0); // at the rim lip
        let soft = c.delta_at_limited(9.8, 0.0, 8.0); // 8 m samples on a 10 m crater
        assert!(sharp > 0.3, "ungated lip stays sharp");
        assert!(
            soft < sharp * 0.5,
            "gated lip must widen/flatten, not alias"
        );
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
        let gated = cs
            .with_min_wavelength(0.0)
            .expect("craters produce gated variants");
        for k in 0..40 {
            let (x, z) = (k as f64 * 0.7 - 14.0, k as f64 * 0.4 - 8.0);
            assert_eq!(gated.apply(x, z, 1.0), cs.apply(x, z, 1.0));
        }
    }

    /// The dense CSR bucket grid + touched-octave-range combine must reproduce
    /// the original `HashMap`-bucketed implementation BIT-FOR-BIT — the crater
    /// field feeds content-addressed caches (CRATER_CACHE, tile disk cache)
    /// whose version constant must not need bumping. Includes a far-flung pair
    /// that trips the `MAX_BUCKET_CELLS` cell-size doubling (a coarser cell
    /// changes only the candidate supersets, never the sampled value).
    #[test]
    fn dense_bucket_grid_matches_hashmap_reference_bitwise() {
        use std::collections::HashMap;
        // Deterministic LCG population: several radius octaves, overlapping bowls.
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut rng = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut craters: Vec<Crater> = (0..400)
            .map(|_| Crater {
                center: [rng() * 800.0 - 400.0, rng() * 800.0 - 400.0],
                radius: 0.3 + rng() * rng() * 60.0,
                depth: 0.5 + rng() * 4.0,
                rim_height: rng(),
                softness: rng() * 0.3,
                bowl_power: 2.0 + rng() * 3.0,
            })
            .collect();
        // Far corners: ~160 km span at ~96 m cells > MAX_BUCKET_CELLS → doubling.
        craters.push(crater(-80_000.0, -80_000.0, 0.3));
        craters.push(crater(80_000.0, 80_000.0, 0.3));
        let cs = Craters::new(craters.clone());

        // Reference: the pre-dense-grid implementation, verbatim (HashMap
        // buckets at the UNDOUBLED cell size + full 0..OCTAVE_COUNT sum).
        let max_reach = craters.iter().map(|c| c.reach()).fold(0.0_f64, f64::max);
        let inv_cell = 1.0 / max_reach.max(1.0);
        let mut buckets: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
        for (i, c) in craters.iter().enumerate() {
            let reach = c.reach();
            if reach <= 0.0 {
                continue;
            }
            let (min_cx, min_cz) = cell_of(c.center[0] - reach, c.center[1] - reach, inv_cell);
            let (max_cx, max_cz) = cell_of(c.center[0] + reach, c.center[1] + reach, inv_cell);
            for cz in min_cz..=max_cz {
                for cx in min_cx..=max_cx {
                    buckets.entry((cx, cz)).or_default().push(i as u32);
                }
            }
        }
        let reference = |x: f64, z: f64| -> f64 {
            let Some(indices) = buckets.get(&cell_of(x, z, inv_cell)) else {
                return 0.0;
            };
            let mut deepest = [0.0_f64; OCTAVE_COUNT];
            let mut tallest = [0.0_f64; OCTAVE_COUNT];
            for &i in indices {
                let d = craters[i as usize].delta_at(x, z);
                if d == 0.0 {
                    continue;
                }
                let o = octave_of(craters[i as usize].radius);
                deepest[o] = deepest[o].min(d);
                tallest[o] = tallest[o].max(d);
            }
            let mut sum = 0.0;
            for o in 0..OCTAVE_COUNT {
                sum += deepest[o] + tallest[o];
            }
            sum
        };
        for gz in -80..=80 {
            for gx in -80..=80 {
                let (x, z) = (gx as f64 * 5.37, gz as f64 * 5.37);
                assert_eq!(
                    cs.delta_at(x, z).to_bits(),
                    reference(x, z).to_bits(),
                    "bit mismatch at ({x},{z})"
                );
            }
        }
        // The doubled-cell far crater still resolves exactly…
        for k in 0..20 {
            let (x, z) = (-80_000.0 + k as f64 * 0.05, -80_000.0 + k as f64 * 0.03);
            assert_eq!(cs.delta_at(x, z).to_bits(), reference(x, z).to_bits());
        }
        // …and far outside every reach AND the grid AABB the field is exact zero.
        assert_eq!(cs.delta_at(1.0e6, -1.0e6), 0.0);
        assert_eq!(cs.delta_at(-1.0e6, 1.0e6), 0.0);
    }

    /// A deterministic LCG population spanning several radius octaves — the
    /// shape the perf work is about (many small craters, a few large).
    fn population(n: usize, span: f64) -> Vec<Crater> {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut rng = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        (0..n)
            .map(|_| Crater {
                center: [rng() * span - span * 0.5, rng() * span - span * 0.5],
                radius: 0.3 + rng() * rng() * 60.0,
                depth: 0.5 + rng() * 4.0,
                rim_height: rng(),
                softness: rng() * 0.3,
                bowl_power: 2.0 + rng() * 3.0,
            })
            .collect()
    }

    /// The Nyquist prune must be INVISIBLE: a dropped crater is one whose
    /// `nyquist_fade` is exactly 0 at that gate, so the pruned index must agree
    /// with the un-pruned one BIT-FOR-BIT — the gated field feeds content-addressed
    /// tile/collider bakes.
    #[test]
    fn band_limited_prune_is_bitwise_identical_to_the_unpruned_gate() {
        let craters = population(600, 900.0);
        let full = Craters::new(craters);
        for &wl in &[0.5, 4.0, 20.0, 96.0] {
            let pruned = full.band_limited(wl);
            // Reference: same placements, NOT pruned, same gate.
            let reference = Craters {
                index: full.index.clone(),
                min_wavelength: wl,
            };
            assert!(
                pruned.crater_count() <= reference.crater_count(),
                "the prune may only drop"
            );
            for gz in -60..=60 {
                for gx in -60..=60 {
                    let (x, z) = (gx as f64 * 7.3, gz as f64 * 7.3);
                    assert_eq!(
                        pruned.delta_at(x, z).to_bits(),
                        reference.delta_at(x, z).to_bits(),
                        "bit mismatch at ({x},{z}) under gate {wl}"
                    );
                }
            }
        }
        // …and the prune actually bites: a 96 m gate keeps only craters ≥ 48 m.
        assert!(full.band_limited(96.0).crater_count() < full.crater_count() / 10);
    }

    /// A region-scoped variant is a BAKE VIEW: identical to the full field
    /// everywhere inside the box it was built for (outside it, nothing is
    /// promised — the craters that reach in from further away are gone).
    #[test]
    fn for_region_matches_the_full_field_inside_the_box() {
        let craters = population(600, 900.0);
        let full = Craters::new(craters);
        let (min, max) = ([-120.0, 40.0], [30.0, 210.0]);
        for &wl in &[0.0, 3.0, 15.0] {
            let scoped = full.for_region(min, max, wl);
            let reference = Craters {
                index: full.index.clone(),
                min_wavelength: wl,
            };
            assert!(scoped.crater_count() < full.crater_count(), "scope drops");
            for gz in 0..=40 {
                for gx in 0..=40 {
                    let x = min[0] + (max[0] - min[0]) * gx as f64 / 40.0;
                    let z = min[1] + (max[1] - min[1]) * gz as f64 / 40.0;
                    assert_eq!(
                        scoped.delta_at(x, z).to_bits(),
                        reference.delta_at(x, z).to_bits(),
                        "bit mismatch at ({x},{z}) in region under gate {wl}"
                    );
                }
            }
        }
    }

    /// The gated index is memoised on the root — a second request for the same
    /// band must hand back the SAME allocation, not rebuild it (every tile and
    /// collider bake re-requests its LOD's band).
    #[test]
    fn gated_variants_are_memoized_per_band() {
        let full = Craters::new(population(200, 400.0));
        let a = full.band_limited(8.0);
        let b = full.band_limited(8.0);
        assert!(Arc::ptr_eq(&a.index, &b.index), "same band → same index");
        let c = full.band_limited(16.0);
        assert!(
            !Arc::ptr_eq(&a.index, &c.index),
            "different band → own index"
        );
    }

    /// Not an assertion — a report. `cargo test -p lunco-terrain-core --
    /// crater_sampling_report --ignored --nocapture` prints what a tile bake
    /// actually pays per sample, so the next person tuning this argues with
    /// numbers instead of the cost model in
    /// `docs/architecture/terrain-crater-perf-plan.md`.
    /// A REPRESENTATIVE field: the layer's own size-frequency distribution
    /// (`N(>r) ∝ r^-1.8` over `[ANALYTIC_RADIUS_FLOOR_M, 60]`) at the shipped
    /// density — 8 craters/ha scaled by the SFD factor over a 1 km half-extent
    /// (400 ha). Uniform-ish radii would misreport the whole problem: the cost
    /// model is ABOUT the population being dominated by craters near the floor.
    fn sfd_population() -> Vec<Crater> {
        let mut state = 0xD1B5_4A32_D192_ED03_u64;
        let mut rng = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let (rmin, rmax, a) = (ANALYTIC_RADIUS_FLOOR_M, 60.0_f64, 1.8_f64);
        let q = (rmin / rmax).powf(a);
        let count = (8.0 * (8.0_f64 / rmin).powf(a) * 400.0) as usize;
        (0..count)
            .map(|_| {
                let radius = rmin * (1.0 - rng() * (1.0 - q)).powf(-1.0 / a);
                let u = rng();
                Crater {
                    center: [rng() * 2000.0 - 1000.0, rng() * 2000.0 - 1000.0],
                    radius,
                    depth: radius * 0.4 * (1.0 - 0.85 * u.powf(0.7)),
                    rim_height: radius * 0.4 * 0.18 * (1.0 - u) * (1.0 - u),
                    softness: 0.03 + 0.45 * u * u,
                    bowl_power: 2.0 + 4.0 * u,
                }
            })
            .collect()
    }

    /// One CDLOD tile's lattice, in bake order (row-major, the order
    /// `bake_tile_mesh` walks it).
    fn lattice(side: f64, res: usize) -> Vec<(f64, f64)> {
        let mut pts = Vec::with_capacity(res * res);
        for iz in 0..res {
            for ix in 0..res {
                pts.push((
                    -side * 0.5 + side * ix as f64 / (res - 1) as f64,
                    -side * 0.5 + side * iz as f64 / (res - 1) as f64,
                ));
            }
        }
        pts
    }

    #[test]
    #[ignore = "perf report, not a pass/fail test"]
    fn crater_sampling_report() {
        use std::time::Instant;
        let craters = sfd_population();
        let full = Craters::new(craters.clone());

        // Candidates a sample considers: per-octave grids vs. the ONE global
        // grid this replaced (cell = the largest reach, so every tiny crater
        // shares a cell two orders of magnitude wider than its own footprint).
        let global_cell = craters.iter().map(|c| c.reach()).fold(0.0_f64, f64::max);
        let (mut per_octave, mut one_grid, mut n) = (0usize, 0usize, 0usize);
        for gz in -20..=20 {
            for gx in -20..=20 {
                let (x, z) = (gx as f64 * 23.0, gz as f64 * 23.0);
                for g in &full.index.grids {
                    per_octave += g.entries_at(x, z).len();
                }
                let (cx, cz) = cell_of(x, z, 1.0 / global_cell);
                one_grid += craters
                    .iter()
                    .filter(|c| {
                        cell_box(c, 1.0 / global_cell).is_some_and(|(x0, z0, x1, z1)| {
                            cx >= x0 && cx <= x1 && cz >= z0 && cz <= z1
                        })
                    })
                    .count();
                n += 1;
            }
        }
        println!(
            "population {} | candidates/sample: {:.1} (per-octave) vs {:.1} (single grid) — {:.1}× fewer",
            full.crater_count(),
            per_octave as f64 / n as f64,
            one_grid as f64 / n as f64,
            one_grid as f64 / per_octave.max(1) as f64,
        );

        // Per-sample cost at three bands, and what a region-scoped bake pays.
        // 129² is one CDLOD tile's lattice.
        let tile = 64.0_f64;
        let res = 129;
        for &(label, step) in &[("leaf", 0.5), ("mid", 4.0), ("coarse", 32.0)] {
            let wl = 2.0 * step;
            let side = tile * (step / 0.5);
            let sample = |c: &Craters| {
                let t = Instant::now();
                let mut acc = 0.0;
                for iz in 0..res {
                    for ix in 0..res {
                        let x = -side * 0.5 + side * ix as f64 / (res - 1) as f64;
                        let z = -side * 0.5 + side * iz as f64 / (res - 1) as f64;
                        acc += c.delta_at(x, z);
                    }
                }
                (t.elapsed().as_secs_f64() / (res * res) as f64 * 1e9, acc)
            };
            let gated = full.band_limited(wl);
            let scoped = full.for_region(
                [-side * 0.5 - side, -side * 0.5 - side],
                [side * 0.5 + side, side * 0.5 + side],
                wl,
            );
            let (t_gated, _) = sample(&gated);
            let (t_scoped, _) = sample(&scoped);
            println!(
                "{label:>6} tile ({side:>5.0} m, gate {wl:>4.0} m): {:>6} craters after prune, \
                 {:>5} after region scope | {t_gated:>6.1} ns/sample → {t_scoped:>6.1} ns/sample",
                gated.crater_count(),
                scoped.crater_count(),
            );
        }
    }

    /// Where the per-sample nanoseconds actually GO. The sampling report says
    /// what a bake pays; this says which stage it pays it to, by running the
    /// same query set through three nested prefixes of `delta_at`:
    ///
    /// - **lookup** — `entries_at` per octave grid and nothing else (the cell
    ///   index: `cell_of` + two `starts[]` loads per grid);
    /// - **walk** — lookup plus the SoA reject loop (`cx`/`cz`/`reach` loads and
    ///   the bbox + radius compares), stopping before any profile call;
    /// - **total** — the real `delta_at`.
    ///
    /// So `walk − lookup` is the candidate scan and `total − walk` is the
    /// profile math. The same set is then run in a **shuffled** order: identical
    /// work, destroyed locality, so the gap is the memory-hierarchy component.
    /// A stage that is cache-bound moves a lot between the two columns; one that
    /// is compute-bound barely moves.
    #[test]
    #[ignore = "perf report, not a pass/fail test"]
    fn crater_sampling_breakdown() {
        use std::hint::black_box;
        use std::time::Instant;

        let full = Craters::new(sfd_population());
        let res = 129usize;
        let n = (res * res) as f64;

        // Min of several runs: we want the achievable cost, not the scheduler's
        // opinion of this machine's afternoon.
        let bench = |f: &dyn Fn(&[(f64, f64)]) -> f64, pts: &[(f64, f64)]| -> f64 {
            let mut best = f64::INFINITY;
            for _ in 0..15 {
                let t = Instant::now();
                black_box(f(pts));
                best = best.min(t.elapsed().as_secs_f64() / n * 1e9);
            }
            best
        };

        println!(
            "{:>6} {:>7} {:>7} {:>6} {:>6} | {:>7} {:>7} {:>7} {:>7} | {:>8} {:>5} {:>7}",
            "tile",
            "craters",
            "idx KiB",
            "cand",
            "hits",
            "lookup",
            "walk",
            "profile",
            "total",
            "unprep",
            "×",
            "-powf"
        );
        for &(label, step) in &[("leaf", 0.5), ("mid", 4.0), ("coarse", 32.0)] {
            let wl = 2.0 * step;
            let side = 64.0 * (step / 0.5);
            let scoped = full.for_region(
                [-side * 0.5 - side, -side * 0.5 - side],
                [side * 0.5 + side, side * 0.5 + side],
                wl,
            );
            let pts = lattice(side, res);
            let mut shuffled = pts.clone();
            let mut state = 0x243F_6A88_85A3_08D3_u64;
            for i in (1..shuffled.len()).rev() {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                shuffled.swap(i, (state >> 33) as usize % (i + 1));
            }

            let lookup = |pts: &[(f64, f64)]| {
                let mut acc = 0usize;
                for &(x, z) in pts {
                    for g in &scoped.index.grids {
                        acc += g.entries_at(x, z).len();
                    }
                }
                acc as f64
            };
            let walk = |pts: &[(f64, f64)]| {
                let mut hits = 0usize;
                for &(x, z) in pts {
                    for g in &scoped.index.grids {
                        for e in g.entries_at(x, z) {
                            let reach = g.reach[e];
                            let dx = x - g.cx[e];
                            if dx >= reach || dx <= -reach {
                                continue;
                            }
                            let dz = z - g.cz[e];
                            if dz >= reach || dz <= -reach {
                                continue;
                            }
                            if dx * dx + dz * dz >= reach * reach {
                                continue;
                            }
                            hits += 1;
                        }
                    }
                }
                hits as f64
            };
            let total = |pts: &[(f64, f64)]| {
                let mut acc = 0.0;
                for &(x, z) in pts {
                    acc += scoped.delta_at(x, z);
                }
                acc
            };
            // The SAME field sampled through the pre-`Prepared` path: identical
            // craters and grids, but a gate the index was not baked for, so
            // `delta_at` falls back to recomputing the constants per hit. An A/B
            // inside one process — machine noise here is ±2× across runs, so a
            // cross-run before/after would measure the afternoon, not the code.
            let unprepared = Craters {
                index: Arc::new(CraterIndex::build(scoped.index.craters.clone(), -1.0)),
                min_wavelength: wl,
            };
            let before = |pts: &[(f64, f64)]| {
                let mut acc = 0.0;
                for &(x, z) in pts {
                    acc += unprepared.delta_at(x, z);
                }
                acc
            };
            assert_eq!(
                total(&pts).to_bits(),
                before(&pts).to_bits(),
                "the prepared path must be bit-identical to the recomputing one"
            );
            // `delta_at` with the bowl's `d.powf(bowl_power)` replaced by `d*d`
            // — NOT a shippable variant (it changes every sampled height); a
            // probe for how much of the residual profile cost is that one
            // `powf`, which is the only transcendental left that depends on a
            // per-crater exponent and so cannot be hoisted into `Prepared`.
            let nopowf = |pts: &[(f64, f64)]| {
                let mut acc = 0.0;
                for &(x, z) in pts {
                    for g in &scoped.index.grids {
                        let (mut deepest, mut tallest) = (0.0_f64, 0.0_f64);
                        for e in g.entries_at(x, z) {
                            let reach = g.reach[e];
                            let dx = x - g.cx[e];
                            let dz = z - g.cz[e];
                            let d2 = dx * dx + dz * dz;
                            if dx >= reach
                                || dx <= -reach
                                || dz >= reach
                                || dz <= -reach
                                || d2 >= reach * reach
                            {
                                continue;
                            }
                            let p = &scoped.index.prepared[g.idx[e] as usize];
                            let d = d2.sqrt() / p.radius;
                            let bowl = if d < 1.0 {
                                -p.depth * (1.0 - d * d)
                            } else {
                                0.0
                            };
                            let v = p.fade
                                * (bowl
                                    + p.rim_k * gauss(d, RIM_CENTER, p.rim_sigma)
                                    + p.apron_k * gauss(d, APRON_CENTER, p.apron_sigma)
                                    - p.tail);
                            deepest = deepest.min(v);
                            tallest = tallest.max(v);
                        }
                        acc += deepest + tallest;
                    }
                }
                acc
            };

            let (t_lookup, t_walk, t_total) = (
                bench(&lookup, &pts),
                bench(&walk, &pts),
                bench(&total, &pts),
            );
            let (t_nopowf, t_before) = (bench(&nopowf, &pts), bench(&before, &pts));
            let t_shuf = bench(&total, &shuffled);
            // Resident index bytes: the cell directory + the SoA candidate arrays
            // + the placement records the profile call dereferences.
            let bytes: usize = scoped
                .index
                .grids
                .iter()
                .map(|g| g.starts.len() * 4 + g.idx.len() * (8 * 3 + 4))
                .sum::<usize>()
                + scoped.index.prepared.len() * std::mem::size_of::<Prepared>();
            println!(
                "{label:>6} {:>7} {:>7.0} {:>6.1} {:>6.2} | {t_lookup:>7.1} {:>7.1} {:>7.1} \
                 {t_total:>7.1} | {t_before:>8.1} {:>5.2} {:>7.1}",
                scoped.crater_count(),
                bytes as f64 / 1024.0,
                lookup(&pts) / n,
                walk(&pts) / n,
                t_walk - t_lookup,
                t_total - t_walk,
                t_before / t_total,
                t_total - t_nopowf,
            );
            println!(
                "       shuffled order: {t_shuf:.1} ns/sample ({:.2}× the bake's row-major walk)",
                t_shuf / t_total,
            );
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
