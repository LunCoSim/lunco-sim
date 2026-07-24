//! Height quantization — the cross-platform determinism firewall.
//!
//! Purity of a [`HeightSource`] guarantees *bit-identical inputs yield identical
//! outputs on one machine*, but transcendental ops (`exp`, `sqrt`, `powi`) are not
//! guaranteed bit-identical **across** platforms (native x86 vs wasm vs ARM may
//! differ in the last ULP). That last-ULP noise is harmless for a rendered vertex,
//! but it is poison the moment a height feeds something that must agree across
//! peers: an avian heightfield collider (contact solving diverges) or a content
//! hash / cache key (identical terrain hashes differently → no cache hit, and the
//! "ship spec + re-derive" networking story breaks).
//!
//! The fix is to **snap heights to a fixed lattice** just before they cross into
//! collider-build or hashing. Quantizing to, say, 1 mm collapses sub-millimetre
//! cross-arch disagreement to exactly nothing while being far below what any rover
//! contact or camera can perceive. Apply it as late as possible (only at the
//! collider/hash boundary) so intermediate math keeps full precision.

use crate::source::HeightSource;

/// Snap `h` to the nearest multiple of `step` metres. `step ≤ 0` is a no-op
/// (returns `h` unchanged). Uses round-half-away-from-zero (`f64::round`), which is
/// deterministic per IEEE-754 on every platform — the whole point.
///
/// Pick `step` well below any perceptible/physical threshold (1 mm = `1e-3` is a
/// safe default for lunar-surface work) yet well above cross-arch ULP noise.
#[inline]
pub fn quantize(h: f64, step: f64) -> f64 {
    if step <= 0.0 {
        return h;
    }
    (h / step).round() * step
}

/// A [`HeightSource`] wrapper that quantizes the inner source's height to a fixed
/// `step` lattice. Put this at the collider-build / hashing boundary so every peer
/// derives byte-identical geometry regardless of transcendental-op ULP drift.
///
/// Normals/slope are intentionally **left on the un-quantized source** (default
/// trait impls differentiate `height_at`, which here would be the *stepped*
/// surface and produce staircase normals). Quantization is a build-boundary
/// discipline for the collider/hash, not a surface the shader should differentiate.
#[derive(Debug, Clone, Copy)]
pub struct QuantizedHeightSource<S> {
    pub inner: S,
    /// Lattice step in metres (e.g. `1e-3` for 1 mm).
    pub step: f64,
}

impl<S> QuantizedHeightSource<S> {
    pub fn new(inner: S, step: f64) -> Self {
        Self { inner, step }
    }
}

impl<S: HeightSource> HeightSource for QuantizedHeightSource<S> {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        quantize(self.inner.height_at(x, z), self.step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ramp;
    impl HeightSource for Ramp {
        fn height_at(&self, x: f64, _z: f64) -> f64 {
            x * 0.123_456_789
        }
    }

    #[test]
    fn snaps_to_lattice() {
        // Every quantized value is an exact multiple of the step (to FP tolerance).
        let step = 1e-3;
        for i in -100..100 {
            let h = i as f64 * 0.007_137;
            let q = quantize(h, step);
            let n = (q / step).round();
            assert!((q - n * step).abs() < 1e-9, "not on lattice: {q}");
            assert!(
                (q - h).abs() <= 0.5 * step + 1e-12,
                "moved too far: {h} -> {q}"
            );
        }
    }

    #[test]
    fn zero_or_negative_step_is_noop() {
        assert_eq!(quantize(1.234_567, 0.0), 1.234_567);
        assert_eq!(quantize(1.234_567, -1.0), 1.234_567);
    }

    #[test]
    fn wrapper_matches_free_fn() {
        let q = QuantizedHeightSource::new(Ramp, 1e-3);
        for i in -50..50 {
            let x = i as f64 * 1.37;
            assert_eq!(q.height_at(x, 9.0), quantize(Ramp.height_at(x, 9.0), 1e-3));
        }
    }

    #[test]
    fn idempotent() {
        // Quantizing an already-quantized value changes nothing.
        let step = 1e-3;
        for i in -100..100 {
            let h = quantize(i as f64 * 0.0191, step);
            assert_eq!(quantize(h, step), h);
        }
    }
}
