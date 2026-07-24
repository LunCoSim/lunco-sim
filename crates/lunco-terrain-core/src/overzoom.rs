//! Procedural **over-zoom** — deterministic sub-DEM detail synthesis.
//!
//! Below the authored data resolution (~5 m for a LOLA DEM) there is genuinely no
//! ground truth, and plain bilinear interpolation reads as smooth plastic up
//! close. [`Overzoom`] synthesises the two things a real regolith surface has at
//! that scale, as one more [`HeightModifier`] on the oracle stack:
//!
//! - **craterlet population** — small impact bowls in geometric size bands from
//!   `max_radius` down to `min_radius`, hashed per spatial cell (nothing stored,
//!   infinite extent), sizes following the lunar equilibrium size-frequency shape
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

use crate::crater::{crater_profile_rim_limited, CRATER_REACH};
use crate::modifier::HeightModifier;
use crate::source::{hash01, vnoise};

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
/// against a single uniform `u`. Real crater populations are Poisson — clumps,
/// occasional pairs, and empty stretches — where the old floor+Bernoulli split
/// produced 0-or-1 per cell, i.e. a jittered grid that reads as a repeating
/// pattern. Capped so a pathological hash can't explode a cell.
#[inline]
fn poisson_count(u: f64, lambda: f64) -> u32 {
    if lambda <= 0.0 {
        return 0;
    }
    let mut p = (-lambda).exp();
    let mut cum = p;
    let mut k = 0u32;
    while u > cum && k < 8 {
        k += 1;
        p *= lambda / k as f64;
        cum += p;
    }
    k
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
        // Skip the whole band when even its biggest feature is (near-)unresolvable
        // at the consumer's sampling — this is what makes coarse consumers cheap.
        // The 5% floor matters: a band contributing centimetres would otherwise
        // still cost full hashing/profile evaluation per sample (the multi-second
        // full-DEM materialize regression that delayed the static collider).
        if nyquist_fade(r_hi, self.min_wavelength) < 0.05 {
            return 0.0;
        }
        let cell = r_hi * CRATER_REACH;
        let cx0 = (x / cell).floor() as i64;
        let cz0 = (z / cell).floor() as i64;
        let band_seed = self.seed ^ band.wrapping_mul(0xA076_1D64_78BD_642F);
        let mut sum = 0.0;
        // Density varies at super-cell (4×4 cell) scale — mean-preserving factor
        // in [0.2, 1.8] — so the population clumps and thins at a scale bigger
        // than a single cell (the large-scale texture real crater fields have).
        let dens_seed = band_seed ^ 0xD1CE_D1CE_D1CE_D1CE;
        for dz in -1..=1i64 {
            for dx in -1..=1i64 {
                let (cx, cz) = (cx0 + dx, cz0 + dz);
                let u_dens = hash01(cx.div_euclid(4), cz.div_euclid(4), dens_seed);
                let lambda = self.crater_mean * (0.2 + 1.6 * u_dens);
                // Deterministic Poisson count: clusters + voids, not 0-or-1.
                let count = poisson_count(hash01(cx, cz, band_seed), lambda);
                for k in 0..count {
                    let ks = band_seed ^ ((k as u64 + 1).wrapping_mul(0xE703_7ED1_A0B4_28DB));
                    let ux = hash01(cx, cz, ks ^ 0x11);
                    let uz = hash01(cx, cz, ks ^ 0x22);
                    let ur = hash01(cx, cz, ks ^ 0x33);
                    let ud = hash01(cx, cz, ks ^ 0x44);
                    let ue = hash01(cx, cz, ks ^ 0x55);
                    let ctr_x = (cx as f64 + ux) * cell;
                    let ctr_z = (cz as f64 + uz) * cell;
                    // Inverse-CDF of N(>r) ∝ r⁻² on [r_lo, r_hi]: mass near r_lo.
                    let inv2 = (1.0 - ur) / (r_lo * r_lo) + ur / (r_hi * r_hi);
                    let r = 1.0 / inv2.sqrt();
                    let fade = nyquist_fade(r, self.min_wavelength);
                    if fade <= 0.0 {
                        continue;
                    }
                    let ddx = x - ctr_x;
                    let ddz = z - ctr_z;
                    let dist = (ddx * ddx + ddz * ddz).sqrt() / r;
                    if dist >= CRATER_REACH {
                        continue;
                    }
                    let depth =
                        r * (self.depth_ratio.0 + (self.depth_ratio.1 - self.depth_ratio.0) * ud);
                    // Rim prominence varies per craterlet — degraded soft-rimmed
                    // bowls next to fresh sharp-lipped ones, instead of one
                    // identical profile everywhere.
                    let rim = depth * (0.15 + 0.5 * ue);
                    // Deep (fresh) craterlets are paraboloid, shallow (degraded)
                    // ones flat dishes — same morphology tie as the crater layer.
                    let bowl_power = 6.0 - 4.0 * ud;
                    // The fade above guarantees the BOWL is resolvable; the thin
                    // lip is not — widen it to the sampling step at full height
                    // so it stays representable. The profile's residual at the
                    // reach is subtracted so the field cuts off continuously.
                    let rim_sigma_n = 0.5 * self.min_wavelength / r;
                    let prof =
                        crater_profile_rim_limited(dist, depth, rim, bowl_power, rim_sigma_n);
                    let tail = crater_profile_rim_limited(
                        CRATER_REACH,
                        depth,
                        rim,
                        bowl_power,
                        rim_sigma_n,
                    );
                    sum += (prof - tail) * fade;
                }
            }
        }
        sum
    }
}

impl HeightModifier for Overzoom {
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
    fn poisson_count_matches_distribution() {
        // Inverse-CDF walk: quantiles of Poisson(0.9) — P(0)=.407, P(≤1)=.772,
        // P(≤2)=.937. Counts above 1 exist (the clumping the jittered grid lacked).
        assert_eq!(poisson_count(0.30, 0.9), 0);
        assert_eq!(poisson_count(0.60, 0.9), 1);
        assert_eq!(poisson_count(0.90, 0.9), 2);
        assert!(poisson_count(0.995, 0.9) >= 3);
        assert_eq!(poisson_count(0.5, 0.0), 0);
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
                match poisson_count(hash01(c, r, band_seed), lambda) {
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
