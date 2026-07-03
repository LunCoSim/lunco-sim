//! Collider height conditioning — slope limiting + quantization before avian.
//!
//! A visual mesh can carry an arbitrarily sharp crater rim; a **heightfield
//! collider** cannot without pain. Rasterised at spacing `s`, a sharp lip becomes a
//! near-vertical step between two adjacent samples, and a rover wheel striking that
//! wall gets a huge normal impulse — contact flips, the vehicle launches or jitters.
//! (The rasterised-vs-crisp mismatch was one face of the original crater bug.)
//!
//! [`slope_limit_grid`] conditions the *collider* heights (never the visual mesh)
//! so no two 4-neighbours differ by more than `L = max_slope · spacing`. It is a
//! monotone **min-sweep** to a fixpoint — `h[i] = min(h[i], min_neighbour + L)` —
//! which yields the *largest* `L`-Lipschitz surface that is ≤ the input. That single
//! rule bounds every edge in both directions: a rim (peak) is shaved down until it
//! ramps to its neighbours, and the steep drop from a rim into a crater bowl is
//! bounded too, because the rim's lower inner neighbour pulls the rim down. It only
//! ever lowers, so it cannot oscillate, is idempotent, and leaves an already-within-
//! limit surface untouched. A crater floor (the lowest cell) is never raised, so the
//! depression is preserved — the rover simply meets a slightly rounded rim in physics
//! while the eye keeps the crisp one, exactly the trade you want for stable contact.
//!
//! [`prepare_collider_heights`] is the full collider-build discipline: slope-limit,
//! then [`quantize`](crate::quantize::quantize) to the determinism lattice, so the
//! heightfield handed to avian is both contact-stable and byte-identical across peers.

use crate::quantize::quantize;

/// Maximum Gauss-Seidel passes as a function of grid side: information propagates
/// one cell per pass, so a spike needs ~`res` passes to spread its cone fully. Plus
/// a small constant for the last-mile settling.
#[inline]
fn max_passes(res: usize) -> usize {
    res + 4
}

/// Enforce a maximum slope on a row-major `res × res` height grid in place: after
/// this, `|h[a] − h[b]| ≤ max_slope · spacing` for every 4-connected pair `(a, b)`.
///
/// Pure and deterministic (fixed scan order). `max_slope ≤ 0`, `spacing ≤ 0`, or
/// `res < 2` is a no-op. A grid already within the bound is left unchanged.
pub fn slope_limit_grid(heights: &mut [f64], res: usize, spacing: f64, max_slope: f64) {
    let max_step = max_slope * spacing;
    if max_step <= 0.0 || res < 2 || heights.len() != res * res {
        return;
    }
    let eps = max_step * 1e-12 + f64::MIN_POSITIVE;
    let idx = |x: usize, z: usize| z * res + x;

    // Lower one cell to at most `min_neighbour + max_step` (min-sweep step). Only
    // ever decreases → monotone, so the process converges and is idempotent.
    let lower_cell = |heights: &[f64], x: usize, z: usize| -> f64 {
        let mut min_n = f64::INFINITY;
        if x > 0 {
            min_n = min_n.min(heights[idx(x - 1, z)]);
        }
        if x + 1 < res {
            min_n = min_n.min(heights[idx(x + 1, z)]);
        }
        if z > 0 {
            min_n = min_n.min(heights[idx(x, z - 1)]);
        }
        if z + 1 < res {
            min_n = min_n.min(heights[idx(x, z + 1)]);
        }
        heights[idx(x, z)].min(min_n + max_step)
    };

    for _ in 0..max_passes(res) {
        let mut changed = 0.0f64;
        // Forward row-major sweep, then reverse, so the min-cone propagates in all
        // axis directions each iteration (chamfer-style).
        for z in 0..res {
            for x in 0..res {
                let new = lower_cell(heights, x, z);
                let d = heights[idx(x, z)] - new; // ≥ 0 (monotone)
                if d > changed {
                    changed = d;
                }
                heights[idx(x, z)] = new;
            }
        }
        for z in (0..res).rev() {
            for x in (0..res).rev() {
                let new = lower_cell(heights, x, z);
                let d = heights[idx(x, z)] - new;
                if d > changed {
                    changed = d;
                }
                heights[idx(x, z)] = new;
            }
        }
        if changed <= eps {
            break;
        }
    }
}

/// The full collider-build conditioning: slope-limit the grid to `max_slope`, then
/// snap every height to the `quant_step` determinism lattice. Result is both
/// contact-stable and peer-identical. `quant_step ≤ 0` skips quantization;
/// `max_slope ≤ 0` skips slope limiting.
pub fn prepare_collider_heights(
    heights: &mut [f64],
    res: usize,
    spacing: f64,
    max_slope: f64,
    quant_step: f64,
) {
    slope_limit_grid(heights, res, spacing, max_slope);
    if quant_step > 0.0 {
        for h in heights.iter_mut() {
            *h = quantize(*h, quant_step);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Max absolute 4-neighbour difference on a grid — the slope the collider presents.
    fn max_adjacent_diff(heights: &[f64], res: usize) -> f64 {
        let idx = |x: usize, z: usize| z * res + x;
        let mut m = 0.0f64;
        for z in 0..res {
            for x in 0..res {
                if x + 1 < res {
                    m = m.max((heights[idx(x + 1, z)] - heights[idx(x, z)]).abs());
                }
                if z + 1 < res {
                    m = m.max((heights[idx(x, z + 1)] - heights[idx(x, z)]).abs());
                }
            }
        }
        m
    }

    #[test]
    fn flat_unchanged() {
        let mut h = vec![5.0; 8 * 8];
        slope_limit_grid(&mut h, 8, 1.0, 0.5);
        assert!(h.iter().all(|&v| (v - 5.0).abs() < 1e-12));
    }

    #[test]
    fn gentle_ramp_within_limit_unchanged() {
        // A linear ramp at exactly the slope limit is already feasible → untouched.
        let res = 16;
        let spacing = 2.0;
        let max_slope = 0.5; // max_step = 1.0 per cell
        let mut h = vec![0.0; res * res];
        for z in 0..res {
            for x in 0..res {
                h[z * res + x] = x as f64 * (max_slope * spacing); // slope == limit
            }
        }
        let before = h.clone();
        slope_limit_grid(&mut h, res, spacing, max_slope);
        for (a, b) in before.iter().zip(&h) {
            assert!((a - b).abs() < 1e-9, "ramp at limit should be unchanged");
        }
    }

    #[test]
    fn spike_is_bounded_to_max_step() {
        // A tall central spike violates the slope badly; after limiting, no edge
        // exceeds max_step.
        let res = 33;
        let spacing = 1.0;
        let max_slope = 0.5; // max_step 0.5
        let mut h = vec![0.0; res * res];
        h[16 * res + 16] = 100.0;
        slope_limit_grid(&mut h, res, spacing, max_slope);
        let max_step = max_slope * spacing;
        assert!(
            max_adjacent_diff(&h, res) <= max_step + 1e-6,
            "slope {} exceeds limit {max_step}",
            max_adjacent_diff(&h, res)
        );
    }

    #[test]
    fn crater_rim_bounded_but_bowl_preserved_roughly() {
        // Sharp rim + deep bowl (the real case): after limiting the collider slope is
        // capped, yet the centre stays a depression (feature not erased).
        let res = 65;
        let spacing = 0.5;
        let max_slope = 1.0;
        let mut h = vec![0.0; res * res];
        let c = 32i32;
        for z in 0..res {
            for x in 0..res {
                let dx = (x as i32 - c) as f64;
                let dz = (z as i32 - c) as f64;
                let d = (dx * dx + dz * dz).sqrt() / 20.0; // radius 20 cells
                let bowl = if d < 1.0 { -8.0 * (1.0 - d * d * d * d) } else { 0.0 };
                let rim = 3.0 * (-((d - 0.98) / 0.05).powi(2)).exp(); // very sharp rim
                h[z * res + x] = bowl + rim;
            }
        }
        slope_limit_grid(&mut h, res, spacing, max_slope);
        assert!(max_adjacent_diff(&h, res) <= max_slope * spacing + 1e-6, "rim slope not bounded");
        assert!(h[c as usize * res + c as usize] < -1.0, "bowl centre should stay depressed");
    }

    #[test]
    fn idempotent() {
        let res = 24;
        let mut h = vec![0.0; res * res];
        for i in 0..res * res {
            h[i] = ((i * 37) % 13) as f64 * 3.0; // jagged
        }
        slope_limit_grid(&mut h, res, 1.0, 0.4);
        let once = h.clone();
        slope_limit_grid(&mut h, res, 1.0, 0.4);
        for (a, b) in once.iter().zip(&h) {
            assert!((a - b).abs() < 1e-6, "second pass changed a converged grid");
        }
    }

    #[test]
    fn nonpositive_params_are_noop() {
        let mut h = vec![0.0, 100.0, 0.0, 100.0];
        let before = h.clone();
        slope_limit_grid(&mut h, 2, 1.0, 0.0); // max_slope 0
        assert_eq!(h, before);
        slope_limit_grid(&mut h, 2, 0.0, 1.0); // spacing 0
        assert_eq!(h, before);
    }

    #[test]
    fn prepare_bounds_slope_and_snaps_to_lattice() {
        let res = 33;
        let spacing = 1.0;
        let max_slope = 0.5;
        let step = 1e-2;
        let mut h = vec![0.0; res * res];
        h[16 * res + 16] = 50.0;
        prepare_collider_heights(&mut h, res, spacing, max_slope, step);
        // Slope bounded (allow one lattice step of quantization slack).
        assert!(max_adjacent_diff(&h, res) <= max_slope * spacing + step + 1e-9);
        // Every height on the lattice.
        for &v in &h {
            let n = (v / step).round();
            assert!((v - n * step).abs() < 1e-9, "off lattice: {v}");
        }
    }
}
