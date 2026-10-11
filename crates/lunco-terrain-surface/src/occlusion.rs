//! Certified terrain occlusion on the composed, continuous f64 height surface.
//! Raster-cell traversal bounds work independently of ray length. Conservative
//! polynomial and modifier enclosures reject clear intervals; unresolved
//! grazing geometry or exhausted work produces an error, never a clear ray.
use crate::oracle::SurfaceOracle;
use bevy::math::DVec3;
use lunco_terrain_core::HeightSource;

/// Maximum interval examinations admitted by a physical terrain query.
#[derive(bevy::prelude::Resource, Clone, Copy, Debug)]
pub struct SurfaceOcclusionBudget {
    pub max_intervals: usize,
    pub max_depth: u32,
    pub max_tick_intervals: usize,
    pub max_cached_cells: usize,
}
impl Default for SurfaceOcclusionBudget {
    fn default() -> Self {
        Self {
            max_intervals: 8192,
            max_depth: 60,
            max_tick_intervals: 32768,
            max_cached_cells: 65536,
        }
    }
}

/// Reusable conservative cell enclosures, invalidated by the complete surface key.
/// FIFO admission bounds memory without making cache residency authoritative.
#[derive(Default)]
pub struct SurfaceOcclusionCache {
    surface: Option<u64>,
    cells: std::collections::HashMap<([usize; 2], u64), [f64; 2]>,
    order: std::collections::VecDeque<([usize; 2], u64)>,
}
impl SurfaceOcclusionCache {
    fn bounds(
        &mut self,
        surface: &SurfaceOracle,
        cell: [usize; 2],
        node: u64,
        rect: [f64; 4],
        capacity: usize,
    ) -> Result<[f64; 2], String> {
        let key = surface.surface_key();
        if self.surface != Some(key) {
            self.surface = Some(key);
            self.cells.clear();
            self.order.clear();
        }
        while self.cells.len() > capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.cells.remove(&oldest);
            }
        }
        if let Some(bounds) = self.cells.get(&(cell, node)) {
            return Ok(*bounds);
        }
        let bounds = surface
            .height_bounds_in_cell(cell, rect)
            .ok_or("terrain modifier has no conservative height bounds")?;
        if !bounds[0].is_finite() || !bounds[1].is_finite() || bounds[0] > bounds[1] {
            return Err("invalid terrain height enclosure".into());
        }
        while self.cells.len() >= capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.cells.remove(&oldest);
            }
        }
        self.cells.insert((cell, node), bounds);
        self.order.push_back((cell, node));
        Ok(bounds)
    }
}

impl SurfaceOracle {
    /// Certify whether any terrain blocks a ray. This remains independent of
    /// collider residency, camera LOD and GPU shadow-cache completion.
    pub fn occludes_ray(
        &self,
        origin: DVec3,
        direction: DVec3,
        budget: SurfaceOcclusionBudget,
        remaining: &mut usize,
        cache: &mut SurfaceOcclusionCache,
    ) -> Result<bool, String> {
        if !origin.is_finite()
            || !direction.is_finite()
            || (direction.length_squared() - 1.0).abs() > 1.0e-8
        {
            return Err("terrain occlusion requires a finite unit ray".into());
        }
        if budget.max_intervals == 0 || budget.max_cached_cells == 0 || budget.max_depth > 62 {
            return Err("invalid terrain occlusion budget".into());
        }
        let half = self.half_extent() as f64;
        let slab = |o: f64, d: f64| -> Option<(f64, f64)> {
            if d == 0.0 {
                (o.abs() <= half).then_some((f64::NEG_INFINITY, f64::INFINITY))
            } else {
                let a = (-half - o) / d;
                let b = (half - o) / d;
                Some((a.min(b), a.max(b)))
            }
        };
        let (Some((xn, xf)), Some((zn, zf))) =
            (slab(origin.x, direction.x), slab(origin.z, direction.z))
        else {
            return Ok(false);
        };
        let mut near = xn.max(zn).max(0.0);
        let far = xf.min(zf);
        if near > far {
            return Ok(false);
        }
        if direction.x == 0.0 && direction.z == 0.0 {
            let height = self.height_at(origin.x, origin.z);
            if !height.is_finite() {
                return Err("non-finite terrain sample".into());
            }
            return Ok(origin.y < height || direction.y < 0.0);
        }
        if !near.is_finite() || !far.is_finite() {
            return Err("terrain ray interval exceeds finite coordinates".into());
        }
        let spacing = self.spacing() as f64;
        let mut examined = 0;
        let mut ambiguous = false;
        let mut pending = std::collections::VecDeque::new();
        let mut classify = |cell, a: f64, b: f64, node, rect| -> Result<Option<bool>, String> {
            if *remaining == 0 {
                return Err("solar fixed-tick interval budget exceeded".into());
            }
            *remaining -= 1;
            examined += 1;
            if examined > budget.max_intervals {
                return Err("terrain occlusion interval budget exceeded".into());
            }
            let pa = origin + direction * a;
            let pb = origin + direction * b;
            if !pa.is_finite() || !pb.is_finite() {
                return Err("terrain ray interval exceeds finite coordinates".into());
            }
            let bounds = cache.bounds(self, cell, node, rect, budget.max_cached_cells)?;
            if pa.y.min(pb.y) > bounds[1] {
                return Ok(Some(false));
            }
            if pa.y.max(pb.y) < bounds[0] {
                return Ok(Some(true));
            }
            // Actual samples only certify blockers. A clear ray still requires
            // conservative enclosures over every interval it traverses.
            for t in [a, a + (b - a) * 0.5, b] {
                let point = origin + direction * t;
                let height = self.height_at(point.x, point.z);
                if !height.is_finite() {
                    return Err("non-finite terrain sample".into());
                }
                if point.y < height {
                    return Ok(Some(true));
                }
            }
            Ok(None)
        };
        while near < far {
            let p = origin + direction * near;
            let cell_axis = |v: f64, d: f64| {
                let f = ((v + half) / spacing).clamp(0.0, (self.base_resolution() - 1) as f64);
                let i = if d < 0.0 && (f - f.round()).abs() < 1.0e-9 {
                    f.round() as isize - 1
                } else {
                    f.floor() as isize
                };
                i.clamp(0, self.base_resolution() as isize - 2) as usize
            };
            let cell = [cell_axis(p.x, direction.x), cell_axis(p.z, direction.z)];
            let exit = |axis: usize, d: f64, o: f64| {
                if d == 0.0 {
                    f64::INFINITY
                } else {
                    let i = cell[axis] + usize::from(d > 0.0);
                    let boundary = if i == self.base_resolution() - 1 {
                        half
                    } else {
                        -half + i as f64 * spacing
                    };
                    (boundary - o) / d
                }
            };
            let end = far
                .min(exit(0, direction.x, origin.x))
                .min(exit(1, direction.z, origin.z));
            if end <= near {
                return Err("terrain cell traversal failed to advance".into());
            }
            let boundary = |i: usize| {
                if i == self.base_resolution() - 1 {
                    half
                } else {
                    -half + i as f64 * spacing
                }
            };
            let rect = [
                boundary(cell[0]),
                boundary(cell[1]),
                boundary(cell[0] + 1),
                boundary(cell[1] + 1),
            ];
            match classify(cell, near, end, 1_u64, rect)? {
                Some(true) => return Ok(true),
                Some(false) => {}
                None => pending.push_back((cell, near, end, 0, 1_u64, rect)),
            }
            near = end;
        }
        // Search all coarse cells before refining uncertain boundaries. A
        // grazing contact must not consume the budget before a later definite
        // blocker can be found. Breadth-first refinement has the same property
        // across unresolved cells and its storage is bounded by the work quota.
        while let Some((cell, a, b, depth, node, rect)) = pending.pop_front() {
            if depth != 0 {
                match classify(cell, a, b, node, rect)? {
                    Some(true) => return Ok(true),
                    Some(false) => continue,
                    None => {}
                }
            }
            if depth >= budget.max_depth {
                ambiguous = true;
                continue;
            }
            // Bisect a canonical spatial enclosure, rather than the moving
            // ray interval: nearby rays reuse exactly the same certificates.
            let axis = (depth % 2) as usize;
            let m = (rect[axis] + rect[axis + 2]) * 0.5;
            if m <= rect[axis] || m >= rect[axis + 2] {
                ambiguous = true;
                continue;
            }
            let mut low = rect;
            low[axis + 2] = m;
            let mut high = rect;
            high[axis] = m;
            let clip = |r: [f64; 4]| -> Option<(f64, f64)> {
                let mut n = a;
                let mut f = b;
                for i in 0..2 {
                    let o = if i == 0 { origin.x } else { origin.z };
                    let d = if i == 0 { direction.x } else { direction.z };
                    if d == 0.0 {
                        if o < r[i] || o > r[i + 2] {
                            return None;
                        }
                    } else {
                        let t0 = (r[i] - o) / d;
                        let t1 = (r[i + 2] - o) / d;
                        n = n.max(t0.min(t1));
                        f = f.min(t0.max(t1));
                    }
                }
                (n < f).then_some((n, f))
            };
            let l = clip(low).map(|(n, f)| (cell, n, f, depth + 1, node * 2, low));
            let h = clip(high).map(|(n, f)| (cell, n, f, depth + 1, node * 2 + 1, high));
            match (l, h) {
                (Some(l), Some(h)) => {
                    pending.push_back(l);
                    pending.push_back(h);
                }
                (Some(l), None) => pending.push_back(l),
                (None, Some(h)) => pending.push_back(h),
                (None, None) => return Err("terrain enclosure clipping lost its ray".into()),
            }
        }
        if ambiguous {
            Err("terrain grazing ray could not be certified".into())
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle::HeightContribution;
    use lunco_obstacle_field::field::HeightGrid;
    use lunco_terrain_core::{BrushModifier, HeightModifier};
    use std::sync::Arc;

    #[test]
    fn certified_occlusion_reuses_procedural_enclosures_for_moving_rays() {
        let surface = SurfaceOracle::new(
            Arc::new(HeightGrid::new_flat(257, 128.0)),
            vec![HeightContribution {
                modifier: Arc::new(lunco_terrain_core::Overzoom::default()),
                content_key: 7,
            }],
        );
        let budget = SurfaceOcclusionBudget::default();
        let mut cache = SurfaceOcclusionCache::default();
        let direction = DVec3::new(0.0, 0.163, 0.987).normalize();
        for tick in 0..3 {
            let mut remaining = budget.max_tick_intervals;
            let started = std::time::Instant::now();
            for x in [-40.0, -20.0, 0.0, 20.0] {
                for z in [-1.0, 0.0, 1.0] {
                    for dx in [-2.0, 0.0, 2.0] {
                        let origin = DVec3::new(x + dx + tick as f64 * 0.001, 1.5, z);
                        assert!(
                            !surface
                                .occludes_ray(origin, direction, budget, &mut remaining, &mut cache)
                                .unwrap()
                        );
                    }
                }
            }
            eprintln!(
                "procedural moving rays: tick={tick}, intervals={}, elapsed={:?}, cached={}",
                budget.max_tick_intervals - remaining,
                started.elapsed(),
                cache.cells.len()
            );
        }
    }

    #[test]
    fn certified_occlusion_finds_blockers_before_refining_uncertain_clear_cells() {
        struct LooseEnclosure(BrushModifier);
        impl HeightModifier for LooseEnclosure {
            fn apply(&self, x: f64, z: f64, height: f64) -> f64 {
                self.0.apply(x, z, height)
            }
            fn height_bounds(&self, _: [f64; 4], _: [f64; 2]) -> Option<[f64; 2]> {
                Some([-100.0, 100.0])
            }
        }
        let surface = SurfaceOracle::new(
            Arc::new(HeightGrid::new_flat(17, 4.0)),
            vec![HeightContribution {
                modifier: Arc::new(LooseEnclosure(BrushModifier::new([2.0, 0.0], 1.0, 2.0))),
                content_key: 1,
            }],
        );
        let budget = SurfaceOcclusionBudget {
            max_intervals: 64,
            ..Default::default()
        };
        let mut remaining = budget.max_tick_intervals;
        assert!(
            surface
                .occludes_ray(
                    DVec3::new(-3.0, 1.0, 0.0),
                    DVec3::X,
                    budget,
                    &mut remaining,
                    &mut SurfaceOcclusionCache::default()
                )
                .unwrap()
        );
    }

    #[test]
    fn certified_occlusion_handles_narrow_features_and_failure_boundaries() {
        let grid = Arc::new(HeightGrid::new_flat(9, 4.0));
        let surface = SurfaceOracle::new(
            grid.clone(),
            vec![HeightContribution {
                modifier: Arc::new(BrushModifier::new([0.125, 0.0], 0.02, 2.0)),
                content_key: 1,
            }],
        );
        let budget = SurfaceOcclusionBudget::default();
        let mut remaining = usize::MAX;
        assert!(
            surface
                .occludes_ray(
                    DVec3::new(0.125, 2.0 - 1.0e-10, 0.0),
                    DVec3::X,
                    budget,
                    &mut remaining,
                    &mut SurfaceOcclusionCache::default()
                )
                .unwrap()
        );
        assert!(
            surface
                .occludes_ray(
                    DVec3::new(-3.0, 1.0, 0.0),
                    DVec3::X,
                    budget,
                    &mut remaining,
                    &mut SurfaceOcclusionCache::default()
                )
                .unwrap()
        );
        assert!(
            !surface
                .occludes_ray(
                    DVec3::new(-3.0, 3.0, 0.0),
                    DVec3::X,
                    budget,
                    &mut remaining,
                    &mut SurfaceOcclusionCache::default()
                )
                .unwrap()
        );
        assert!(
            surface
                .occludes_ray(
                    DVec3::new(0.0, 1.0, 0.0),
                    -DVec3::Y,
                    budget,
                    &mut remaining,
                    &mut SurfaceOcclusionCache::default()
                )
                .unwrap()
        );
        assert!(
            !surface
                .occludes_ray(
                    DVec3::new(0.0, 1.0, 0.0),
                    DVec3::Y,
                    budget,
                    &mut remaining,
                    &mut SurfaceOcclusionCache::default()
                )
                .unwrap()
        );
        assert!(
            surface
                .occludes_ray(
                    DVec3::new(-3.0, 1.0, 0.0),
                    DVec3::X,
                    SurfaceOcclusionBudget {
                        max_intervals: 1,
                        ..budget
                    },
                    &mut remaining,
                    &mut SurfaceOcclusionCache::default()
                )
                .is_err()
        );
        let mut cache = SurfaceOcclusionCache::default();
        let small_cache = SurfaceOcclusionBudget {
            max_cached_cells: 1,
            ..budget
        };
        assert!(
            !surface
                .occludes_ray(
                    DVec3::new(-3.0, 3.0, 0.0),
                    DVec3::X,
                    budget,
                    &mut remaining,
                    &mut cache
                )
                .unwrap()
        );
        assert!(cache.cells.len() > 1);
        assert!(
            !surface
                .occludes_ray(
                    DVec3::new(-3.0, 3.0, 0.0),
                    DVec3::X,
                    small_cache,
                    &mut remaining,
                    &mut cache
                )
                .unwrap()
        );
        assert!(cache.cells.len() <= 1);
        let replacement = SurfaceOracle::new(
            Arc::new(HeightGrid::new_flat(9, 4.0)),
            vec![HeightContribution {
                modifier: Arc::new(BrushModifier::new([0.125, 0.0], 0.02, 4.0)),
                content_key: 3,
            }],
        );
        assert!(
            replacement
                .occludes_ray(
                    DVec3::new(-3.0, 3.0, 0.0),
                    DVec3::X,
                    small_cache,
                    &mut remaining,
                    &mut cache
                )
                .unwrap()
        );
        struct Unbounded;
        impl HeightModifier for Unbounded {
            fn apply(&self, _: f64, _: f64, h: f64) -> f64 {
                h
            }
        }
        let unknown = SurfaceOracle::new(
            grid,
            vec![HeightContribution {
                modifier: Arc::new(Unbounded),
                content_key: 2,
            }],
        );
        assert!(
            unknown
                .occludes_ray(
                    DVec3::new(-3.0, 1.0, 0.0),
                    DVec3::X,
                    budget,
                    &mut remaining,
                    &mut SurfaceOcclusionCache::default()
                )
                .is_err()
        );
        assert!(
            surface
                .occludes_ray(
                    DVec3::ZERO,
                    DVec3::ZERO,
                    budget,
                    &mut remaining,
                    &mut SurfaceOcclusionCache::default()
                )
                .is_err()
        );
        assert!(
            surface
                .occludes_ray(
                    DVec3::ZERO,
                    DVec3::new(1.0e-320, 1.0, 0.0),
                    budget,
                    &mut remaining,
                    &mut SurfaceOcclusionCache::default()
                )
                .is_err()
        );
    }
}
