//! Shared **filter policy** for band-limited surface products — the DRY seam
//! between what a wheel TOUCHES and what an eye SEES.
//!
//! ## Why this exists
//!
//! Every terrain consumer samples the same analytic truth ([`SurfaceOracle`]),
//! but through its own low-pass filter: [`SurfaceOracle::detail_limited`] gates
//! out features below a `min_wavelength`. That wavelength was historically a
//! private free constant at each call site — `2.0 * step` in `tile_cache.rs`
//! and `collider_ring.rs`, `4.0 * step` for the morph-target lattice, a texel
//! multiple in `derived_layers.rs` — written in files that do not import each
//! other. Two consumers picking different numbers sample subtly different
//! surfaces, and the gap between those surfaces is exactly the wheel-sinking
//! artifact: the collider keeps relief the drawn mesh flattens out.
//!
//! [`SurfaceBand`] makes the filtered surface a **first-class, named, shared
//! artifact**: one value type, one definition per band, N consumers. Two
//! consumers requesting [`SurfaceBand::contact`] provably sample the identical
//! gate, because there is only one definition to diverge from. See
//! `WHEEL_SINKING_ANALYSIS_v3.md` §4.1 — "make the filtered surface a
//! first-class, named, shared artifact, so this bug class becomes
//! unrepresentable rather than re-fixable."
//!
//! ## The bands
//!
//! - [`SurfaceBand::contact`] — what a wheel touches must be what the eye sees.
//!   Floored at the visual leaf's gate so the rover drives the band the drawn
//!   leaf actually carries (the doc's §5(2): gating the collider *down* is
//!   cheaper than raising tile resolution everywhere and also cuts contact
//!   noise). Built from both the visual leaf step and the collider step, since
//!   the contact invariant is a relation between the two paths.
//! - [`SurfaceBand::visual`] — a render mesh at a given sample spacing. The
//!   `2·step` Nyquist convention both paths share.
//! - [`SurfaceBand::visual_parent`] — the morph-target lattice lives on the
//!   parent's 2×-spaced grid, so its gate is `4·step` (a fully-morphed tile IS
//!   the parent surface).

use crate::oracle::SurfaceOracle;

/// A named, shared filter policy for a band-limited surface product.
///
/// Construct via the named constructors ([`Self::contact`], [`Self::visual`],
/// [`Self::visual_parent`]); the `min_wavelength` is the sole policy field.
/// Apply with [`Self::limited`] to get a gated [`SurfaceOracle`] view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceBand {
    /// Minimum wavelength (m) the surface keeps. Features below this are
    /// Nyquist-gated out by [`SurfaceOracle::detail_limited`].
    pub min_wavelength: f64,
}

impl SurfaceBand {
    /// A visual band at a given sample spacing: keeps features ≥ `2·step`
    /// (the Nyquist convention both the mesh and collider paths share — two
    /// samples per shortest wavelength).
    #[inline]
    pub fn visual(step: f64) -> Self {
        Self {
            min_wavelength: 2.0 * step,
        }
    }

    /// The morph-target band: morph targets sample the parent's 2×-spaced
    /// lattice, so a fully-morphed tile is the parent surface — its gate is
    /// `4·step` (one more 2× on top of [`Self::visual`]).
    #[inline]
    pub fn visual_parent(step: f64) -> Self {
        Self {
            min_wavelength: 4.0 * step,
        }
    }

    /// The **contact band** — the surface a wheel touches, floored so it agrees
    /// with what the drawn visual leaf carries.
    ///
    /// Takes both the visual leaf step and the collider step because the
    /// contact invariant is a *relation* between the two paths: what the rover
    /// touches must be no finer than what the eye sees, or the wheel drops into
    /// relief the mesh doesn't draw. The floor is `max(2·visual_leaf_step,
    /// 2·collider_step)` — whichever gate is coarser wins, so touch and sight
    /// converge on one band. (The doc's §5(2): gating the collider *down* to the
    /// leaf gate is cheaper than raising `TILE_RES` everywhere and also reduces
    /// contact-flipping noise.)
    #[inline]
    pub fn contact(visual_leaf_step: f64, collider_step: f64) -> Self {
        Self {
            min_wavelength: (2.0 * visual_leaf_step).max(2.0 * collider_step),
        }
    }

    /// Apply this band to an oracle, returning a gated view that suppresses
    /// features below [`Self::min_wavelength`]. Cheap (clones the modifier
    /// `Arc`s); call per bake.
    #[inline]
    pub fn limited(&self, oracle: &SurfaceOracle) -> SurfaceOracle {
        oracle.detail_limited(self.min_wavelength)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_band_is_nyquist_double_step() {
        assert!((SurfaceBand::visual(0.5).min_wavelength - 1.0).abs() < 1e-9);
        assert!((SurfaceBand::visual(0.305).min_wavelength - 0.61).abs() < 1e-9);
    }

    #[test]
    fn parent_band_is_four_times_step() {
        // The morph-target lattice is 2× coarser → 4·step gate.
        assert!((SurfaceBand::visual_parent(0.407).min_wavelength - 1.628).abs() < 1e-3);
    }

    #[test]
    fn contact_band_floors_at_the_coarser_gate() {
        // The collider's native gate (2·0.305 = 0.61) is FINER than the visual
        // leaf's (2·0.407 = 0.814). The contact floor must pick the coarser one,
        // so the rover touches the band the leaf draws — not a finer band the
        // mesh flattens out (the wheel-sinking gap).
        let band = SurfaceBand::contact(
            /* visual_leaf_step */ 0.407, /* collider_step */ 0.305,
        );
        assert!(
            (band.min_wavelength - 0.814).abs() < 1e-3,
            "contact floor = coarser gate"
        );
        // Symmetric: whichever input is coarser wins.
        let band2 = SurfaceBand::contact(0.305, 0.407);
        assert_eq!(band.min_wavelength, band2.min_wavelength);
    }

    #[test]
    fn contact_band_is_monotone_in_either_step() {
        // Growing either step can only widen (coarsen) the contact band — it
        // never narrows past the current floor, by the `max`.
        let base = SurfaceBand::contact(0.4, 0.3);
        assert!(SurfaceBand::contact(0.5, 0.3).min_wavelength >= base.min_wavelength);
        assert!(SurfaceBand::contact(0.4, 0.4).min_wavelength >= base.min_wavelength);
    }
}
