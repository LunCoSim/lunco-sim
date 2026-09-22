//! Shared identity and dirty footprint for one committed terrain surface.
//!
//! Visual meshes, collider tiles, and derived maps all sample the same
//! [`crate::oracle::SurfaceOracle`], but each keeps its own resolution and
//! cache. This record carries the common source identity and the smallest
//! region that changed so each product can invalidate only its own affected
//! work.

use bevy::prelude::Component;

/// The latest committed oracle swap and its terrain-local XZ dirty bounds.
///
/// `dirty_bounds == None` means the complete surface changed. A consumer whose
/// source key, destination key, or revision does not match must also invalidate
/// the complete surface; bounded reuse is safe only from the immediately
/// replaced surface.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct TerrainSurfaceChange {
    pub revision: u64,
    pub previous_surface_key: u64,
    pub surface_key: u64,
    pub dirty_bounds: Option<[f64; 4]>,
}

impl TerrainSurfaceChange {
    /// Record a committed oracle swap. If the previous record does not describe
    /// the oracle being replaced, the incoming bounds cannot cover the missing
    /// change, so widen this commit to a whole-surface invalidation.
    pub(crate) fn next(
        previous: Option<&Self>,
        replaced_surface_key: u64,
        surface_key: u64,
        dirty_bounds: Option<[f64; 4]>,
    ) -> Self {
        let revision = previous.map_or(1, |change| change.revision.wrapping_add(1));
        let dirty_bounds = previous
            .filter(|change| change.surface_key != replaced_surface_key)
            .map_or(dirty_bounds, |_| None);
        Self {
            revision,
            previous_surface_key: replaced_surface_key,
            surface_key,
            dirty_bounds,
        }
    }

    /// Bounds that can safely scope a consumer at `surface_key`. A mismatched
    /// key or skipped revision returns `None`, which is the full-invalidation
    /// value at every product boundary.
    pub(crate) fn bounds_since(
        &self,
        previous_revision: u64,
        previous_surface_key: Option<u64>,
        surface_key: u64,
    ) -> Option<[f64; 4]> {
        if previous_surface_key == Some(self.previous_surface_key)
            && self.surface_key == surface_key
            && self.revision == previous_revision.wrapping_add(1)
        {
            self.dirty_bounds
        } else {
            None
        }
    }

    /// Whether this change is bounded for a product that observes every
    /// `DemHeightField` swap in order.
    pub(crate) fn is_bounded_from(&self, previous_surface_key: u64, surface_key: u64) -> bool {
        self.previous_surface_key == previous_surface_key
            && self.surface_key == surface_key
            && self.dirty_bounds.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_change_requires_the_matching_surface_and_next_revision() {
        let previous = TerrainSurfaceChange {
            revision: 4,
            previous_surface_key: 99,
            surface_key: 100,
            dirty_bounds: Some([-5.0, -4.0, 5.0, 4.0]),
        };
        let next =
            TerrainSurfaceChange::next(Some(&previous), 100, 101, Some([0.0, -2.0, 8.0, 2.0]));

        assert_eq!(next.revision, 5);
        assert_eq!(next.previous_surface_key, 100);
        assert_eq!(
            next.bounds_since(4, Some(100), 101),
            Some([0.0, -2.0, 8.0, 2.0])
        );
        assert_eq!(next.bounds_since(3, Some(100), 101), None);
        assert_eq!(next.bounds_since(4, Some(99), 101), None);
        assert_eq!(next.bounds_since(4, None, 101), None);
        assert!(next.is_bounded_from(100, 101));
        assert!(!next.is_bounded_from(99, 101));
    }

    #[test]
    fn missing_surface_history_widens_the_change_to_full_surface() {
        let previous = TerrainSurfaceChange {
            revision: 7,
            previous_surface_key: 99,
            surface_key: 100,
            dirty_bounds: Some([-5.0, -4.0, 5.0, 4.0]),
        };
        let next =
            TerrainSurfaceChange::next(Some(&previous), 99, 101, Some([0.0, -2.0, 8.0, 2.0]));

        assert_eq!(next.revision, 8);
        assert_eq!(next.dirty_bounds, None);
    }

    #[test]
    fn square_overlap_is_shared_and_includes_touching_edges() {
        let square = crate::Square {
            center: [0.0, 0.0],
            half: 2.0,
        };

        assert!(square.overlaps_aabb([2.0, -1.0, 4.0, 1.0]));
        assert!(!square.overlaps_aabb([2.01, -1.0, 4.0, 1.0]));
    }
}
