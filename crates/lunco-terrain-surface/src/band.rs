//! Shared **filter policy** for band-limited terrain products.
//!
//! ## Why this exists
//!
//! Every terrain consumer samples the same analytic truth ([`SurfaceOracle`]),
//! but through its own low-pass filter: [`SurfaceOracle::detail_limited`] gates
//! out features below a `min_wavelength`. The policy is named so each product
//! derives its own band from its own authored lattice without duplicating the
//! Nyquist calculation.
//!
//! [`SurfaceBand`] makes the filtered surface a **first-class, named, shared
//! artifact**: one value type, one Nyquist definition, N independent products.
//! Visual selection never supplies physics parameters, and physics never reads
//! visual selection.
//!
//! ## The bands
//!
//! - [`SurfaceBand::physics`] — a physics product at its authored sample
//!   spacing. Its `2·step` Nyquist gate depends only on the physics lattice.
//! - [`SurfaceBand::visual`] — a render mesh at its selected sample spacing.
//!   Its `2·step` gate depends only on visual tile selection.
//! - [`SurfaceBand::visual_parent`] — the morph-target lattice lives on the
//!   parent's 2×-spaced grid, so its gate is `4·step` (a fully-morphed tile IS
//!   the parent surface).

use crate::oracle::SurfaceOracle;
use bevy::reflect::Reflect;

/// A named, shared filter policy for a band-limited surface product.
///
/// Construct via the named constructors ([`Self::physics`], [`Self::visual`],
/// [`Self::visual_parent`]); the `min_wavelength` is the sole policy field.
/// Apply with [`Self::limited`] to get a gated [`SurfaceOracle`] view.
#[derive(Debug, Clone, Copy, PartialEq, Reflect)]
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

    /// A physics band at the collider's authored sample spacing: keeps
    /// features ≥ `2·step`. The physics lattice is independent of visual LOD.
    #[inline]
    pub fn physics(step: f64) -> Self {
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

    /// Apply this band to an oracle, returning a gated view that suppresses
    /// features below [`Self::min_wavelength`]. Cheap (clones the modifier
    /// `Arc`s); call per bake.
    #[inline]
    pub fn limited(&self, oracle: &SurfaceOracle) -> SurfaceOracle {
        oracle.detail_limited(self.min_wavelength)
    }

    /// Apply this band to an oracle **scoped to one bake's footprint** — the
    /// square a tile / collider tile / map bake is about to sample, grown by
    /// `margin` metres so ghost rings and stencils stay inside the scope.
    ///
    /// Same filter policy as [`Self::limited`], plus the region prune: a bake
    /// that knows its box gathers the placement-backed modifiers (crater
    /// fields) once rather than per sample. Values inside the box are identical
    /// to [`Self::limited`] — see [`SurfaceOracle::detail_limited_region`] for
    /// the contract (do not sample outside the box).
    #[inline]
    pub fn limited_region(
        &self,
        oracle: &SurfaceOracle,
        region: lunco_terrain_core::quadtree::Square,
        margin: f64,
    ) -> SurfaceOracle {
        let half = region.half + margin;
        oracle.detail_limited_region(
            self.min_wavelength,
            [region.center[0] - half, region.center[1] - half],
            [region.center[0] + half, region.center[1] + half],
        )
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
    fn physics_band_is_independent_of_visual_band() {
        assert_eq!(SurfaceBand::physics(0.305), SurfaceBand::visual(0.305));
        assert_ne!(SurfaceBand::physics(0.305), SurfaceBand::visual(0.407));
    }
}
