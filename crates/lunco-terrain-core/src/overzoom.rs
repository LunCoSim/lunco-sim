//! Procedural **over-zoom** — deterministic sub-DEM detail synthesis.
//!
//! Below the authored data resolution (~5 m for a LOLA DEM) there is genuinely no
//! ground truth, and plain bilinear interpolation reads as smooth plastic up
//! close. [`Overzoom`] synthesises the two things a real regolith surface has at
//! that scale, as one more [`HeightModifier`] on the oracle stack:
//!
//! - **craterlet population** — small impact bowls in geometric size bands from
//!   `max_radius` down to `min_radius`, generated from spatial hashes over
//!   infinite extent, sizes following the lunar equilibrium size-frequency shape
//!   (`N(>r) ∝ r⁻²` within a band — many small, few large), each evaluated
//!   analytically through [`crater_profile_rim_limited`](crate::crater::crater_profile_rim_limited),
//!   which widens the lip to the sampling step so a craterlet rim never falls
//!   between samples and alias into a spike;
//! - **micro-relief** — a few octaves of value-noise FBM for the gentle undulation
//!   between craterlets.
//!
//! **Deterministic** from `seed` (pure lattice hashes, no RNG state) — visual
//! tiles, the collider, and every networked peer synthesise the identical
//! surface. **Nyquist-gated**: a consumer sampling at spacing `s` sets
//! [`min_wavelength`](Overzoom::min_wavelength) (via
//! `HeightModifier::with_min_wavelength`), and features too small for that
//! sampling fade out instead of aliasing — a far tile skips the synthesis
//! entirely (cheap), a near tile resolves it fully. This is "over-zoom gated by
//! LOD depth" from `docs/architecture/terrain-substrate.md`, lever 3.

use std::sync::Arc;

use crate::crater::{CRATER_REACH, crater_profile_rim_limited};
use crate::modifier::HeightModifier;
use crate::source::{hash01, vnoise};

const MAX_CELL_CRATERS: u32 = 8;
const RIM_RATIO_BASE: f64 = 0.15;
const RIM_RATIO_SPAN: f64 = 0.5;

/// Deterministic synthetic sub-DEM detail: hashed craterlet bands + FBM
/// micro-relief. See the module docs. All fields are authorable (the USD
/// `lunco:layer = "overzoom"` prim maps onto them).
#[derive(Debug, Clone)]
pub struct Overzoom {
    pub seed: u64,
    /// Largest synthetic craterlet rim radius (m). Keep below the authored data
    /// resolution — larger features are the DEM's/crater layer's job.
    pub max_radius: f64,
    /// Smallest synthetic craterlet rim radius (m).
    pub min_radius: f64,
    /// Mean craterlets per band cell (~density knob; 0 = no craterlets).
    pub crater_mean: f64,
    /// Craterlet depth/radius ratio range `(fresh_min, fresh_max)` — hashed per
    /// craterlet, so the population mixes degraded shallow bowls and fresher ones.
    pub depth_ratio: (f64, f64),
    /// Micro-relief amplitude (m) of the coarsest FBM octave. 0 = no relief.
    pub relief_amp: f64,
    /// Wavelength (m) of the coarsest micro-relief octave.
    pub relief_scale: f64,
    /// Nyquist gate: the sampling wavelength (m) of the consumer this instance
    /// serves. Features below ~this fade to zero instead of aliasing. `0` = no
    /// gate (full detail). Set per consumer via `with_min_wavelength`.
    pub min_wavelength: f64,
}

impl Default for Overzoom {
    fn default() -> Self {
        Self {
            // Grouped to spell "5EED 0F DE7A11" — SEED OF DETAIL. clippy's
            // `unusual_byte_groupings` wants even 4-nibble groups
            // (`0x5EED_0FDE_7A11`), which is the same number with the word
            // broken across the boundary. The grouping IS the documentation
            // here, so the lint is silenced rather than obeyed.
            #[allow(clippy::unusual_byte_groupings)]
            seed: 0x5EED_0F_DE7A11,
            // Hand off at 2 m to the analytic crater layer, whose power-law size
            // floor is 2 m — together they cover 0.4 m…60 m without doubling up.
            max_radius: 2.0,
            min_radius: 0.4,
            crater_mean: 0.9,
            depth_ratio: (0.06, 0.22),
            relief_amp: 0.08,
            relief_scale: 14.0,
            min_wavelength: 0.0,
        }
    }
}

/// Smooth Nyquist fade for a feature of characteristic size `r` under a sampling
/// wavelength `w`: 1 when comfortably resolvable (`r ≥ 3w`), 0 when below the
/// sampling scale (`r ≤ w`), smoothstep between.
#[inline]
pub(crate) fn nyquist_fade(r: f64, w: f64) -> f64 {
    if w <= 0.0 {
        return 1.0;
    }
    let t = ((r / w - 1.0) / 2.0).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Deterministic Poisson draw: inverse-CDF walk of the Poisson pmf at `lambda`
/// against a single uniform `u`. Cells sharing a density reuse the initial
/// probability; each independent draw starts from that same mass. Counts are
/// capped so a pathological hash cannot explode a cell.
struct PoissonDistribution {
    lambda: f64,
    initial_probability: f64,
}
impl PoissonDistribution {
    #[inline]
    fn new(lambda: f64) -> Self {
        Self {
            lambda,
            initial_probability: (-lambda).exp(),
        }
    }

    #[inline]
    fn count(&self, u: f64) -> u32 {
        if self.lambda <= 0.0 {
            return 0;
        }
        let mut p = self.initial_probability;
        let mut cum = p;
        let mut k = 0u32;
        while u > cum && k < MAX_CELL_CRATERS {
            k += 1;
            p *= self.lambda / k as f64;
            cum += p;
        }
        k
    }
}

impl Overzoom {
    /// The synthetic height delta (m) at `(x, z)` under the current Nyquist gate.
    pub fn delta_at(&self, x: f64, z: f64) -> f64 {
        let mut d = self.relief_delta(x, z);
        // Craterlet bands: halving radius ranges from max_radius down. Bands are
        // evaluated in a gently warped domain so rims come out irregular ovals
        // instead of perfect circles (no two craterlets look alike up close).
        if self.crater_mean > 0.0 && self.max_radius > self.min_radius.max(0.05) {
            let (wx, wz) = self.warped(x, z);
            let mut r_hi = self.max_radius;
            let mut band: u64 = 0;
            while r_hi > self.min_radius.max(0.05) && band < 8 {
                let r_lo = (r_hi * 0.5).max(self.min_radius);
                d += self.band_delta(wx, wz, band, r_lo, r_hi);
                r_hi = r_lo;
                band += 1;
            }
        }
        d
    }

    /// Smooth domain warp applied to the craterlet query position. Amplitude is
    /// Nyquist-faded like every other feature so coarse consumers see the same
    /// (un)warped field they can resolve — LOD morphing stays continuous.
    #[inline]
    fn warped(&self, x: f64, z: f64) -> (f64, f64) {
        let wl = self.max_radius.max(1.0);
        let amp = 0.12 * wl * nyquist_fade(wl, self.min_wavelength);
        if amp <= 1e-6 {
            return (x, z);
        }
        let f = 1.0 / wl;
        (
            x + amp * vnoise(x * f, z * f, self.seed ^ 0x57A6_57A6_57A6_57A6),
            z + amp * vnoise(x * f, z * f, self.seed ^ 0x7A57_7A57_7A57_7A57),
        )
    }

    /// FBM micro-relief, octave-gated by the sampling wavelength.
    fn relief_delta(&self, x: f64, z: f64) -> f64 {
        if self.relief_amp <= 0.0 {
            return 0.0;
        }
        let mut sum = 0.0;
        let mut amp = self.relief_amp;
        let mut wavelength = self.relief_scale.max(0.5);
        for o in 0..5u64 {
            let fade = nyquist_fade(wavelength, self.min_wavelength);
            if fade <= 0.0 {
                break; // octaves only get finer — nothing below resolves either
            }
            let f = 1.0 / wavelength;
            sum += amp
                * fade
                * vnoise(
                    x * f,
                    z * f,
                    self.seed ^ (o.wrapping_mul(0x9E37_79B9_7F4A_7C15)),
                );
            amp *= 0.5;
            wavelength *= 0.5;
        }
        sum
    }

    /// Summed craterlet deltas of one size band `[r_lo, r_hi)` — craterlets are
    /// hashed per `cell = r_hi · REACH` lattice cell; a 3×3 neighbourhood bounds
    /// every craterlet that can reach `(x, z)`.
    fn band_delta(&self, x: f64, z: f64, band: u64, r_lo: f64, r_hi: f64) -> f64 {
        let mut sum = 0.0;
        self.visit_band_craters([x, z, x, z], band, r_lo, r_hi, |crater| {
            sum += crater.delta_at(x, z);
        });
        sum
    }

    /// Enumerate the same hashed population for point samples and regional
    /// enclosures. The expanded cell range covers every reaching crater.
    fn visit_band_craters(
        &self,
        rect: [f64; 4],
        band: u64,
        r_lo: f64,
        r_hi: f64,
        mut visit: impl FnMut(SyntheticCrater),
    ) {
        if nyquist_fade(r_hi, self.min_wavelength) < 0.05 {
            return;
        }
        let cell = r_hi * CRATER_REACH;
        let band_seed = self.seed ^ band.wrapping_mul(0xA076_1D64_78BD_642F);
        let dens_seed = band_seed ^ 0xD1CE_D1CE_D1CE_D1CE;
        // Density is constant over each 4x4 lattice group. Its distribution
        // can serve adjacent cells without changing their independent hashes.
        let mut density: Option<([i64; 2], PoissonDistribution)> = None;
        for cz in (rect[1] / cell).floor() as i64 - 1..=(rect[3] / cell).floor() as i64 + 1 {
            for cx in (rect[0] / cell).floor() as i64 - 1..=(rect[2] / cell).floor() as i64 + 1 {
                let group = [cx.div_euclid(4), cz.div_euclid(4)];
                if density.as_ref().map(|(key, _)| *key) != Some(group) {
                    let u_dens = hash01(group[0], group[1], dens_seed);
                    let lambda = self.crater_mean * (0.2 + 1.6 * u_dens);
                    density = Some((group, PoissonDistribution::new(lambda)));
                }
                let count = density
                    .as_ref()
                    .expect("density distribution admitted for this cell")
                    .1
                    .count(hash01(cx, cz, band_seed));
                for k in 0..count {
                    let ks = band_seed ^ ((k as u64 + 1).wrapping_mul(0xE703_7ED1_A0B4_28DB));
                    let ux = hash01(cx, cz, ks ^ 0x11);
                    let uz = hash01(cx, cz, ks ^ 0x22);
                    let ur = hash01(cx, cz, ks ^ 0x33);
                    let ud = hash01(cx, cz, ks ^ 0x44);
                    let ue = hash01(cx, cz, ks ^ 0x55);
                    let center = [(cx as f64 + ux) * cell, (cz as f64 + uz) * cell];
                    let inv2 = (1.0 - ur) / (r_lo * r_lo) + ur / (r_hi * r_hi);
                    let radius = 1.0 / inv2.sqrt();
                    let fade = nyquist_fade(radius, self.min_wavelength);
                    if fade <= 0.0 {
                        continue;
                    }
                    if (center[0] - center[0].clamp(rect[0], rect[2]))
                        .hypot(center[1] - center[1].clamp(rect[1], rect[3]))
                        >= radius * CRATER_REACH
                    {
                        continue;
                    }
                    let depth = radius
                        * (self.depth_ratio.0 + (self.depth_ratio.1 - self.depth_ratio.0) * ud);
                    let rim = depth * (RIM_RATIO_BASE + RIM_RATIO_SPAN * ue);
                    let power = 6.0 - 4.0 * ud;
                    let sigma = (0.5 * self.min_wavelength / radius).clamp(0.14, 0.35);
                    let tail = crater_profile_rim_limited(CRATER_REACH, depth, rim, power, sigma);
                    visit(SyntheticCrater {
                        center,
                        radius,
                        depth,
                        rim,
                        power,
                        sigma,
                        tail,
                        fade,
                    });
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct SyntheticCrater {
    center: [f64; 2],
    radius: f64,
    depth: f64,
    rim: f64,
    power: f64,
    sigma: f64,
    tail: f64,
    fade: f64,
}
impl SyntheticCrater {
    fn delta_at(self, x: f64, z: f64) -> f64 {
        let dx = x - self.center[0];
        let dz = z - self.center[1];
        let d = (dx * dx + dz * dz).sqrt() / self.radius;
        if d >= CRATER_REACH {
            return 0.0;
        }
        (crater_profile_rim_limited(d, self.depth, self.rim, self.power, self.sigma) - self.tail)
            * self.fade
    }
    fn bounds(self, rect: [f64; 4]) -> [f64; 2] {
        let d = crate::crater::radial_bounds(rect, self.center, self.radius);
        if d[0] >= CRATER_REACH {
            return [0.0; 2];
        }
        let b = crate::crater::rim_limited_profile_bounds(
            [d[0], d[1].min(CRATER_REACH)],
            self.depth,
            self.rim,
            self.power,
            self.sigma,
        );
        let mut result = [
            (b[0] - self.tail) * self.fade,
            (b[1] - self.tail) * self.fade,
        ];
        if d[1] >= CRATER_REACH {
            result[0] = result[0].min(0.0);
            result[1] = result[1].max(0.0);
        }
        result
    }
}

impl HeightModifier for Overzoom {
    fn height_bounds(&self, rect: [f64; 4], input: [f64; 2]) -> Option<[f64; 2]> {
        if rect
            .iter()
            .chain(input.iter())
            .copied()
            .chain([
                self.max_radius,
                self.min_radius,
                self.crater_mean,
                self.depth_ratio.0,
                self.depth_ratio.1,
                self.relief_amp,
                self.relief_scale,
                self.min_wavelength,
            ])
            .any(|v| !v.is_finite())
            || rect[0] > rect[2]
            || rect[1] > rect[3]
        {
            return None;
        }
        let x = (rect[0] + rect[2]) * 0.5;
        let z = (rect[1] + rect[3]) * 0.5;
        let radius = (rect[2] - rect[0]).hypot(rect[3] - rect[1]) * 0.5;
        let noise_slope =
            5.0 * 3.0 * 2.0_f64.sqrt() * self.relief_amp.abs() / self.relief_scale.max(0.5);
        let mid = self.relief_delta(x, z);
        let span = noise_slope * radius;
        let relief_bound = 1.9375 * self.relief_amp.max(0.0);
        let mut result = [
            input[0] + (mid - span).max(-relief_bound),
            input[1] + (mid + span).min(relief_bound),
        ];
        if self.crater_mean > 0.0 && self.max_radius > self.min_radius.max(0.05) {
            // The warp Jacobian has operator norm below two: each noise
            // derivative is at most three, with amplitude/wavelength = 0.12.
            let (wx, wz) = self.warped(x, z);
            let reach = 2.0 * radius;
            let region = [wx - reach, wz - reach, wx + reach, wz + reach];
            let mut r_hi = self.max_radius;
            let mut band = 0;
            while r_hi > self.min_radius.max(0.05) && band < 8 {
                let r_lo = (r_hi * 0.5).max(self.min_radius);
                // A point sees at most nine cells of eight craters each.
                // For positive depth d, the bowl is in [-d, 0], while
                // rim + apron and the subtracted reach tail are each in
                // [0, 0.65 * (1 + APRON_FRAC) * d]. Negative depths reverse
                // those signs. Keep both signs for authored inverted bowls.
                let mut delta = if nyquist_fade(r_hi, self.min_wavelength) < 0.05 {
                    [0.0; 2]
                } else {
                    let positive = r_hi * self.depth_ratio.0.max(self.depth_ratio.1).max(0.0);
                    let negative = -r_hi * self.depth_ratio.0.min(self.depth_ratio.1).min(0.0);
                    let rim = (RIM_RATIO_BASE + RIM_RATIO_SPAN) * (1.0 + crate::crater::APRON_FRAC);
                    let count = 9.0 * MAX_CELL_CRATERS as f64;
                    [
                        -count * ((1.0 + rim) * positive).max(rim * negative),
                        count * (rim * positive).max((1.0 + rim) * negative),
                    ]
                };
                let cell = r_hi * CRATER_REACH;
                if region[2] - region[0] <= 4.0 * cell && region[3] - region[1] <= 4.0 * cell {
                    let mut regional = [0.0, 0.0];
                    self.visit_band_craters(region, band, r_lo, r_hi, |crater| {
                        let bounds = crater.bounds(region);
                        regional[0] += bounds[0];
                        regional[1] += bounds[1];
                    });
                    delta = [delta[0].max(regional[0]), delta[1].min(regional[1])];
                }
                result[0] += delta[0];
                result[1] += delta[1];
                r_hi = r_lo;
                band += 1;
            }
        }
        let pad = 128.0 * f64::EPSILON * result[0].abs().max(result[1].abs()).max(1.0);
        Some([result[0] - pad, result[1] + pad])
    }
    fn apply(&self, x: f64, z: f64, h_in: f64) -> f64 {
        h_in + self.delta_at(x, z)
    }

    fn with_min_wavelength(&self, min_wavelength: f64) -> Option<Arc<dyn HeightModifier>> {
        Some(Arc::new(Overzoom {
            min_wavelength,
            ..self.clone()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oz() -> Overzoom {
        Overzoom {
            min_wavelength: 0.0,
            ..Default::default()
        }
    }

    #[test]
    fn deterministic() {
        let a = oz();
        let b = oz();
        for k in 0..200 {
            let (x, z) = (k as f64 * 3.7 - 300.0, k as f64 * -2.3 + 100.0);
            assert_eq!(a.delta_at(x, z), b.delta_at(x, z));
        }
    }

    #[test]
    fn poisson_draw_matches_distribution() {
        // Poisson(0.9): P(0)=.407, P(≤1)=.772, P(≤2)=.937.
        // Shared distribution draws remain independent of call order.
        let distribution = PoissonDistribution::new(0.9);
        for (u, count) in [(0.90, 2), (0.30, 0), (0.60, 1), (0.90, 2), (0.30, 0)] {
            assert_eq!(distribution.count(u), count);
        }
        assert!(distribution.count(0.995) >= 3);
        assert_eq!(PoissonDistribution::new(0.0).count(0.5), 0);
    }

    #[test]
    fn crater_counts_have_poisson_variance() {
        // Sweep many band-0 cells: some must be empty AND some must hold ≥ 2
        // craterlets — a jittered grid (0-or-1 everywhere) fails this.
        let s = oz();
        let band_seed = s.seed ^ 0u64.wrapping_mul(0xA076_1D64_78BD_642F);
        let dens_seed = band_seed ^ 0xD1CE_D1CE_D1CE_D1CE;
        let (mut empty, mut multi) = (0, 0);
        for c in -50i64..50 {
            for r in -50i64..50 {
                let u_dens = hash01(c.div_euclid(4), r.div_euclid(4), dens_seed);
                let lambda = s.crater_mean * (0.2 + 1.6 * u_dens);
                match PoissonDistribution::new(lambda).count(hash01(c, r, band_seed)) {
                    0 => empty += 1,
                    n if n >= 2 => multi += 1,
                    _ => {}
                }
            }
        }
        assert!(empty > 1000, "too few empty cells ({empty}/10000)");
        assert!(multi > 500, "too few multi-craterlet cells ({multi}/10000)");
    }

    #[test]
    fn bounded_amplitude() {
        // Relief FBM < 2·amp; craterlets bounded by max depth · (worst-case
        // Poisson pile-up: capped 8/cell across the 3×3 neighbourhood).
        let s = oz();
        let bound = 2.0 * s.relief_amp + s.max_radius * s.depth_ratio.1 * 12.0;
        for gx in -40..40 {
            for gz in -40..40 {
                let d = s.delta_at(gx as f64 * 5.1, gz as f64 * 5.1);
                assert!(d.abs() < bound, "delta {d} out of bound {bound}");
            }
        }
    }

    #[test]
    fn has_actual_relief() {
        // The synthesised field is not flat: some spread over a small area.
        let s = oz();
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for gx in 0..100 {
            for gz in 0..100 {
                let d = s.delta_at(gx as f64 * 0.8, gz as f64 * 0.8);
                lo = lo.min(d);
                hi = hi.max(d);
            }
        }
        assert!(hi - lo > 0.05, "spread {} too flat", hi - lo);
    }

    #[test]
    fn coarse_sampling_keeps_the_rim_lip() {
        // 10 m craterlet under a 5.2 m sampling wavelength: the bowl resolves but
        // the exact lip (σ = 1.4 m) is sub-vertex. The rim-limited profile must
        // keep the lip at ~full height.
        use crate::crater::{crater_profile, crater_profile_rim_limited};
        let (depth, rim, p) = (1.5, 0.6, 3.0);
        let sigma_n = 0.5 * 5.2 / 10.0;
        let exact = crater_profile(0.98, depth, rim, p);
        let wide = crater_profile_rim_limited(0.98, depth, rim, p, sigma_n);
        assert!(
            (wide - exact).abs() < 0.05 * exact.abs().max(0.1),
            "widened lip should hold full height: exact {exact}, wide {wide}"
        );
        // …and rim_sigma_n = 0 is bit-for-bit the exact profile (ungated
        // consumers see no change).
        for d in [0.0, 0.5, 0.98, 1.15, 1.4] {
            assert_eq!(
                crater_profile_rim_limited(d, depth, rim, p, 0.0),
                crater_profile(d, depth, rim, p)
            );
        }
    }

    #[test]
    fn gated_field_is_continuous_at_the_craterlet_reach() {
        // The reach-tail subtraction must hold under a coarse gate too: walk
        // radially across band cells and require no step anywhere.
        let s = Overzoom {
            min_wavelength: 5.2,
            ..Default::default()
        };
        let eps = 1e-4;
        for k in 0..400 {
            let x = k as f64 * 0.25;
            let d = (s.delta_at(x + eps, 7.7) - s.delta_at(x - eps, 7.7)).abs();
            assert!(d < 0.05, "step {d} at x={x} under a 5.2 m gate");
        }
    }

    #[test]
    fn nyquist_gate_kills_fine_detail() {
        // Sampling at 20 m wavelength: every craterlet (≤ 6 m) and every relief
        // octave (≤ 14 m) is below ~the gate → the field collapses to ~0.
        let coarse = Overzoom {
            min_wavelength: 20.0,
            ..Default::default()
        };
        for k in 0..100 {
            let (x, z) = (k as f64 * 7.3, k as f64 * -4.1);
            assert!(
                coarse.delta_at(x, z).abs() < 1e-9,
                "gated field should vanish"
            );
        }
        // …while the ungated field is alive at the same points.
        let fine = oz();
        let alive = (0..100).any(|k| fine.delta_at(k as f64 * 7.3, k as f64 * -4.1).abs() > 0.01);
        assert!(alive);
    }

    #[test]
    fn continuous_no_cell_seams() {
        // Walk across band-cell boundaries: no discontinuities (craterlets are
        // generated per cell but evaluated as smooth profiles).
        let s = oz();
        let eps = 1e-4;
        for k in -200..200 {
            let x = k as f64 * 0.6;
            let d = (s.delta_at(x + eps, 3.3) - s.delta_at(x - eps, 3.3)).abs();
            assert!(d < 0.05, "jump {d} at x={x}");
        }
    }

    #[test]
    fn with_min_wavelength_variant_differs_from_full() {
        let s = oz();
        let v = s
            .with_min_wavelength(5.0)
            .expect("overzoom produces variants");
        // The gated variant is a genuinely different (reduced-detail) field.
        let differs = (0..200).any(|k| {
            let (x, z) = (k as f64 * 2.9, k as f64 * -1.7);
            (s.apply(x, z, 0.0) - v.apply(x, z, 0.0)).abs() > 1e-6
        });
        assert!(
            differs,
            "5 m gate should attenuate sub-5 m detail somewhere"
        );
    }
}

#[cfg(test)]
mod enclosure_tests {
    use super::*;
    #[test]
    fn broad_enclosures_cover_positive_inverted_and_mixed_craters() {
        for depth_ratio in [(0.06, 0.22), (-0.22, -0.06), (-0.22, 0.22)] {
            for gate in [0.0, 0.2, 1.0, 2.7] {
                let surface = Overzoom {
                    depth_ratio,
                    min_wavelength: gate,
                    ..Default::default()
                };
                for side in [2.0, 8.0, 32.0, 100.0] {
                    let rect = [-40.5, -20.3, -40.5 + side, -20.3 + side];
                    let bounds = surface.height_bounds(rect, [0.0; 2]).unwrap();
                    for iz in 0..=32 {
                        for ix in 0..=32 {
                            let h = surface.delta_at(
                                rect[0] + ix as f64 * side / 32.0,
                                rect[1] + iz as f64 * side / 32.0,
                            );
                            assert!(
                                bounds[0] <= h && h <= bounds[1],
                                "{depth_ratio:?}, gate={gate}, {bounds:?} misses {h}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn regional_overzoom_enclosures_cover_warped_population_and_noise() {
        let invalid = Overzoom {
            depth_ratio: (f64::NAN, 0.22),
            ..Default::default()
        };
        assert!(
            invalid
                .height_bounds([0.0, 0.0, 1.0, 1.0], [0.0; 2])
                .is_none()
        );
        for gate in [0.0, 0.2, 1.0] {
            let surface = Overzoom {
                min_wavelength: gate,
                ..Default::default()
            };
            for i in 0..60 {
                let x = (i as f64 * 1.37) - 40.0;
                let z = (i as f64 * 0.71) - 20.0;
                let rect = [x, z, x + 0.31, z + 0.27];
                let bounds = surface.height_bounds(rect, [0.0; 2]).unwrap();
                for iz in 0..=12 {
                    for ix in 0..=12 {
                        let h = surface
                            .delta_at(x + ix as f64 * 0.31 / 12.0, z + iz as f64 * 0.27 / 12.0);
                        assert!(bounds[0] <= h && h <= bounds[1], "{bounds:?} misses {h}");
                    }
                }
            }
        }
    }
}
