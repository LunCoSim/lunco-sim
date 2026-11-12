//! Certified terrain occlusion on the composed, continuous f64 height surface.
//! Raster-cell traversal bounds work independently of ray length. Conservative
//! polynomial and modifier enclosures reject clear intervals; unresolved
//! grazing geometry or exhausted work produces an error, never a clear ray.
use crate::oracle::SurfaceOracle;
use bevy::math::DVec3;
use bevy::platform::collections::HashMap;
use lunco_terrain_core::HeightSource;

/// Maximum interval examinations admitted by a physical terrain query.
#[derive(bevy::prelude::Resource, Clone, Copy, Debug)]
pub struct SurfaceOcclusionBudget {
    pub max_intervals: usize,
    pub max_depth: u32,
    pub max_tick_intervals: usize,
    pub max_cached_cells: usize,
    /// Canonical enclosure cuts attempted before the exact ray footprint.
    /// This bounds cache preparation, independently of interval admission.
    pub max_ray_cache_depth: u32,
}
impl Default for SurfaceOcclusionBudget {
    fn default() -> Self {
        Self {
            max_intervals: 8192,
            max_depth: 60,
            max_tick_intervals: 32768,
            max_cached_cells: 65536,
            max_ray_cache_depth: 8,
        }
    }
}

/// Cell geometry is fixed by its surface; subrectangles retain exact coordinates.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum CellBoundsKey {
    Cell {
        x: usize,
        z: usize,
    },
    Rectangle {
        cell: [usize; 2],
        coordinates: [u64; 4],
    },
}

impl CellBoundsKey {
    /// Raster cell owning this certificate.
    fn cell(self) -> [usize; 2] {
        match self {
            Self::Cell { x, z } => [x, z],
            Self::Rectangle { cell, .. } => cell,
        }
    }
}
type RayCoverEntry = ([f64; 4], [f64; 2], usize);

// Adjacent raster cells share a small page. The DDA visits a page without
// hashing each cell; exact subrectangle certificates retain their own keys.
const CELL_PAGE_SIDE: usize = 4;
const CELL_PAGE_LEN: usize = CELL_PAGE_SIDE * CELL_PAGE_SIDE;

#[derive(Clone, Copy)]
struct BoundsCertificate {
    bounds: [f64; 2],
    cell_cost: usize,
}

#[derive(Default)]
struct CellPage {
    values: [Option<BoundsCertificate>; CELL_PAGE_LEN],
    used: usize,
}

/// One bounded certificate store. Coarse cell pages have stable slots, so an
/// adjacent DDA cell can reuse its page without hashing or moving stored bounds.
/// Empty pages retire immediately; the exact page key fences the hot slot.
#[derive(Default)]
struct BoundsStore {
    rectangles: HashMap<CellBoundsKey, BoundsCertificate>,
    page_slots: HashMap<[usize; 2], usize>,
    pages: Vec<Option<CellPage>>,
    free: Vec<usize>,
    hot: Option<([usize; 2], usize)>,
    cells: usize,
}

impl BoundsStore {
    fn location(x: usize, z: usize) -> ([usize; 2], usize) {
        (
            [x / CELL_PAGE_SIDE, z / CELL_PAGE_SIDE],
            (z % CELL_PAGE_SIDE) * CELL_PAGE_SIDE + x % CELL_PAGE_SIDE,
        )
    }

    fn page_slot(&mut self, key: [usize; 2]) -> Option<usize> {
        if let Some((current, index)) = self.hot
            && current == key
        {
            return Some(index);
        }
        let index = *self.page_slots.get(&key)?;
        self.hot = Some((key, index));
        Some(index)
    }

    fn get(&mut self, key: &CellBoundsKey) -> Option<BoundsCertificate> {
        match *key {
            CellBoundsKey::Cell { x, z } => {
                let (key, index) = Self::location(x, z);
                let page = self.page_slot(key)?;
                self.pages[page].as_ref()?.values[index]
            }
            CellBoundsKey::Rectangle { .. } => self.rectangles.get(key).copied(),
        }
    }

    fn insert(&mut self, key: CellBoundsKey, value: BoundsCertificate) {
        match key {
            CellBoundsKey::Cell { x, z } => {
                let (key, index) = Self::location(x, z);
                let slot = self.page_slot(key).unwrap_or_else(|| {
                    let slot = self.free.pop().unwrap_or_else(|| {
                        self.pages.push(None);
                        self.pages.len() - 1
                    });
                    self.pages[slot] = Some(CellPage::default());
                    self.page_slots.insert(key, slot);
                    self.hot = Some((key, slot));
                    slot
                });
                let page = self.pages[slot].as_mut().expect("admitted bounds page");
                if page.values[index].replace(value).is_none() {
                    page.used += 1;
                    self.cells += 1;
                }
            }
            CellBoundsKey::Rectangle { .. } => {
                self.rectangles.insert(key, value);
            }
        }
    }

    fn remove(&mut self, key: &CellBoundsKey) {
        match *key {
            CellBoundsKey::Cell { x, z } => {
                let (key, index) = Self::location(x, z);
                if let Some(slot) = self.page_slot(key) {
                    let page = self.pages[slot].as_mut().expect("admitted bounds page");
                    if page.values[index].take().is_some() {
                        page.used -= 1;
                        self.cells -= 1;
                    }
                    if page.used == 0 {
                        self.pages[slot] = None;
                        self.page_slots.remove(&key);
                        self.free.push(slot);
                        self.hot = None;
                    }
                }
            }
            CellBoundsKey::Rectangle { .. } => {
                self.rectangles.remove(key);
            }
        }
    }

    fn len(&self) -> usize {
        self.cells + self.rectangles.len()
    }
    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Reusable conservative spatial enclosures, invalidated by the complete surface key.
/// FIFO admission bounds memory without making cache residency authoritative.
#[derive(Default)]
pub struct SurfaceOcclusionCache {
    surface: Option<u64>,
    cells: BoundsStore,
    order: std::collections::VecDeque<CellBoundsKey>,
    ray_cover: std::collections::VecDeque<RayCoverEntry>,
    batch: Option<HashMap<CellBoundsKey, BoundsCertificate>>,
    batch_needs_reset: bool,
}
impl SurfaceOcclusionCache {
    /// Start a deterministic, bounded group of related terrain rays. Each
    /// enclosure is admitted and charged once within this group, regardless of
    /// whether it was retained from an earlier group.
    pub(crate) fn begin_batch(&mut self) {
        self.batch_needs_reset = true;
    }

    /// Fence retained bounds once for the immutable query surface.
    fn prepare(&mut self, surface: &SurfaceOracle, capacity: usize) {
        if self.batch_needs_reset {
            self.batch.get_or_insert_with(HashMap::new).clear();
            self.batch_needs_reset = false;
        }
        let key = surface.surface_key();
        if self.surface != Some(key) {
            self.surface = Some(key);
            self.cells.clear();
            self.order.clear();
            if let Some(batch) = self.batch.as_mut() {
                batch.clear();
            }
        }
        while self.cells.len() > capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.cells.remove(&oldest);
            }
        }
    }

    fn bounds(
        &mut self,
        surface: &SurfaceOracle,
        key: CellBoundsKey,
        rect: [f64; 4],
        capacity: usize,
        remaining: &mut usize,
    ) -> Result<[f64; 2], String> {
        if let Some(batch) = self.batch.as_ref() {
            if let Some(certificate) = batch.get(&key) {
                return Ok(certificate.bounds);
            }
            if batch.len() >= capacity {
                return Err("terrain occlusion batch certificate budget exceeded".into());
            }
        }
        if let Some(certificate) = self.cells.get(&key) {
            if *remaining < certificate.cell_cost {
                *remaining = 0;
                return Err("regional DEM cell enclosure budget exceeded".into());
            }
            *remaining -= certificate.cell_cost;
            if let Some(batch) = self.batch.as_mut() {
                batch.insert(key, certificate);
            }
            return Ok(certificate.bounds);
        }
        let before = *remaining;
        let bounds = {
            let _span = bevy::log::debug_span!("terrain_occlusion_bounds_miss").entered();
            surface.height_bounds_in_cell(key.cell(), rect, remaining)?
        };
        while self.cells.len() >= capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.cells.remove(&oldest);
            }
        }
        let certificate = BoundsCertificate {
            bounds,
            cell_cost: before - *remaining,
        };
        self.cells.insert(key, certificate);
        self.order.push_back(key);
        if let Some(batch) = self.batch.as_mut() {
            batch.insert(key, certificate);
        }
        Ok(bounds)
    }

    /// Every segment of a fresh ray must be contained by a conservative
    /// rectangle certificate. Canonical cuts share complete covers between
    /// nearby rays without quantizing their coordinates or heights.
    fn refine_ray_enclosure(
        &mut self,
        surface: &SurfaceOracle,
        cell: [usize; 2],
        rect: [f64; 4],
        ray: OcclusionRay,
        interval: [f64; 2],
        budget: SurfaceOcclusionBudget,
        remaining: &mut usize,
    ) -> Result<Option<bool>, String> {
        let mut pending = std::mem::take(&mut self.ray_cover);
        pending.push_back((rect, interval, 0));
        let result = (|| {
            let mut cuts = 0;
            while let Some((rect, interval, level)) = pending.pop_front() {
                let points = ray.endpoints(interval);
                if points.iter().any(|point| {
                    point.x < rect[0] || point.z < rect[1] || point.x > rect[2] || point.z > rect[3]
                }) {
                    return Ok(None);
                }
                if cuts >= budget.max_ray_cache_depth.min(budget.max_depth) {
                    return Ok(None);
                }
                let coordinates = points.map(|point| [point.x, point.z]);
                let axes = [level % 2, (level + 1) % 2];
                let mut children = [None, None];
                // Prefer a single containing half before splitting a crossing ray.
                for axis in axes {
                    let mid = (rect[axis] + rect[axis + 2]) * 0.5;
                    if mid <= rect[axis] || mid >= rect[axis + 2] {
                        continue;
                    }
                    let mut child = rect;
                    if coordinates[0][axis] <= mid && coordinates[1][axis] <= mid {
                        child[axis + 2] = mid;
                    } else if coordinates[0][axis] >= mid && coordinates[1][axis] >= mid {
                        child[axis] = mid;
                    } else {
                        continue;
                    }
                    children[0] = Some((child, interval));
                    break;
                }
                if children[0].is_none() {
                    for axis in axes {
                        let mid = (rect[axis] + rect[axis + 2]) * 0.5;
                        if mid <= rect[axis] || mid >= rect[axis + 2] {
                            continue;
                        }
                        let mut low = rect;
                        low[axis + 2] = mid;
                        let mut high = rect;
                        high[axis] = mid;
                        children = [low, high]
                            .map(|child| ray.clip(interval, child).map(|span| (child, span)));
                        if children.iter().all(Option::is_none) {
                            return Err("terrain enclosure clipping lost its ray".into());
                        }
                        break;
                    }
                }
                if children.iter().all(Option::is_none) {
                    return Ok(None);
                }
                cuts += 1;
                for (child, span) in children.into_iter().flatten() {
                    let points = ray.endpoints(span);
                    if points.iter().any(|point| {
                        point.x < child[0]
                            || point.z < child[1]
                            || point.x > child[2]
                            || point.z > child[3]
                    }) {
                        return Ok(None);
                    }
                    let key = CellBoundsKey::Rectangle {
                        cell,
                        coordinates: child.map(f64::to_bits),
                    };
                    let bounds =
                        self.bounds(surface, key, child, budget.max_cached_cells, remaining)?;
                    match classify_enclosure(bounds, points) {
                        Some(true) => return Ok(Some(true)),
                        Some(false) => {}
                        None => pending.push_back((child, span, level + 1)),
                    }
                }
            }
            Ok(Some(false))
        })();
        pending.clear();
        self.ray_cover = pending;
        result
    }
}

#[derive(Clone, Copy)]
struct OcclusionRay {
    origin: DVec3,
    direction: DVec3,
}

impl OcclusionRay {
    fn endpoints(self, interval: [f64; 2]) -> [DVec3; 2] {
        interval.map(|t| self.origin + self.direction * t)
    }

    fn clip(self, [mut near, mut far]: [f64; 2], rect: [f64; 4]) -> Option<[f64; 2]> {
        for axis in 0..2 {
            let origin = if axis == 0 {
                self.origin.x
            } else {
                self.origin.z
            };
            let direction = if axis == 0 {
                self.direction.x
            } else {
                self.direction.z
            };
            if direction == 0.0 {
                if origin < rect[axis] || origin > rect[axis + 2] {
                    return None;
                }
            } else {
                let t0 = (rect[axis] - origin) / direction;
                let t1 = (rect[axis + 2] - origin) / direction;
                near = near.max(t0.min(t1));
                far = far.min(t0.max(t1));
            }
        }
        (near < far).then_some([near, far])
    }
}

fn classify_enclosure(bounds: [f64; 2], [a, b]: [DVec3; 2]) -> Option<bool> {
    if a.y.min(b.y) > bounds[1] {
        Some(false)
    } else if a.y.max(b.y) < bounds[0] {
        Some(true)
    } else {
        None
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
        if budget.max_intervals == 0
            || budget.max_cached_cells == 0
            || budget.max_depth > 62
            || budget.max_ray_cache_depth > 62
        {
            return Err("invalid terrain occlusion budget".into());
        }
        let ray = OcclusionRay { origin, direction };
        let half = self.domain_half_extent() as f64;
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
        cache.prepare(self, budget.max_cached_cells);
        let shared_regional_cells = self.has_regional_surface() && cache.batch.is_some();
        let spacing = self.occlusion_spacing();
        let mut examined = 0;
        let mut ambiguous = false;
        let mut pending = std::collections::VecDeque::new();
        // Adjacent cells and refinement intervals share endpoints. The oracle
        // is immutable during this ray; reuse only identical f64 coordinates.
        // Sample residency cannot change interval admission or classification.
        let mut point_samples = HashMap::new();
        let mut classify =
            |key: CellBoundsKey, a: f64, b: f64, rect| -> Result<Option<bool>, String> {
                let cell = key.cell();
                if *remaining == 0 {
                    return Err("solar fixed-tick interval budget exceeded".into());
                }
                *remaining -= 1;
                examined += 1;
                if examined > budget.max_intervals {
                    return Err("terrain occlusion interval budget exceeded".into());
                }
                let [pa, pb] = ray.endpoints([a, b]);
                if !pa.is_finite() || !pb.is_finite() {
                    return Err("terrain ray interval exceeds finite coordinates".into());
                }
                let bounds = cache.bounds(self, key, rect, budget.max_cached_cells, remaining)?;
                if let Some(result) = classify_enclosure(bounds, [pa, pb]) {
                    return Ok(Some(result));
                }
                if let Some(result) =
                    cache.refine_ray_enclosure(self, cell, rect, ray, [a, b], budget, remaining)?
                {
                    return Ok(Some(result));
                }
                // The shared cell certificate includes terrain beside the ray.
                // Restrict the same oracle enclosure to the segment's footprint
                // before subdividing: transverse relief cannot block this segment.
                let bounds = {
                    let _span =
                        bevy::log::debug_span!("terrain_occlusion_exact_footprint").entered();
                    self.height_bounds_in_cell(
                        cell,
                        [
                            pa.x.min(pb.x),
                            pa.z.min(pb.z),
                            pa.x.max(pb.x),
                            pa.z.max(pb.z),
                        ],
                        remaining,
                    )?
                };
                if let Some(result) = classify_enclosure(bounds, [pa, pb]) {
                    return Ok(Some(result));
                }
                // Actual samples only certify blockers. A clear ray still requires
                // conservative enclosures over every interval it traverses.
                for t in [a, a + (b - a) * 0.5, b] {
                    let point = origin + direction * t;
                    let height = *point_samples
                        .entry([point.x.to_bits(), point.z.to_bits()])
                        .or_insert_with(|| {
                            let _span =
                                bevy::log::debug_span!("terrain_occlusion_point_sample").entered();
                            self.height_at(point.x, point.z)
                        });
                    if !height.is_finite() {
                        return Err("non-finite terrain sample".into());
                    }
                    if point.y < height {
                        return Ok(Some(true));
                    }
                }
                Ok(None)
            };
        let p = origin + direction * near;
        let cell_axis = |v: f64, d: f64| {
            let f = ((v + half) / spacing).clamp(0.0, (self.domain_resolution() - 1) as f64);
            let i = if d < 0.0 {
                f.ceil() as isize - 1
            } else {
                f.floor() as isize
            };
            i.clamp(0, self.domain_resolution() as isize - 2) as usize
        };
        let mut cell = [cell_axis(p.x, direction.x), cell_axis(p.z, direction.z)];
        // Advance integer cell identities at the crossed boundaries. Recovering
        // them from origin + direction * t can round into the cell just exited.
        // Each iteration crosses at least one boundary, including zero-length
        // entry spans; no offset or epsilon changes the physical ray.
        let coarse_span = bevy::log::debug_span!("terrain_occlusion_coarse_traversal").entered();
        while near < far {
            let exit = |axis: usize, d: f64, o: f64| {
                if d == 0.0 {
                    f64::INFINITY
                } else {
                    let i = cell[axis] + usize::from(d > 0.0);
                    let boundary = if i == self.domain_resolution() - 1 {
                        half
                    } else {
                        -half + i as f64 * spacing
                    };
                    (boundary - o) / d
                }
            };
            let exits = [
                exit(0, direction.x, origin.x),
                exit(1, direction.z, origin.z),
            ];
            let end = far.min(exits[0]).min(exits[1]);
            let boundary = |i: usize| {
                if i == self.domain_resolution() - 1 {
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
            if end > near {
                // A regional cell may contain hundreds of finer site cells.
                // Enclose the ray footprint, not the full coarse square, so
                // transverse site detail cannot exhaust the fixed work quota.
                let (key, rect) = if self.has_regional_surface() && !shared_regional_cells {
                    let [a, b] = ray.endpoints([near, end]);
                    let footprint = [
                        a.x.min(b.x).clamp(rect[0], rect[2]),
                        a.z.min(b.z).clamp(rect[1], rect[3]),
                        a.x.max(b.x).clamp(rect[0], rect[2]),
                        a.z.max(b.z).clamp(rect[1], rect[3]),
                    ];
                    (
                        CellBoundsKey::Rectangle {
                            cell,
                            coordinates: footprint.map(f64::to_bits),
                        },
                        footprint,
                    )
                } else {
                    (
                        CellBoundsKey::Cell {
                            x: cell[0],
                            z: cell[1],
                        },
                        rect,
                    )
                };
                match classify(key, near, end, rect)? {
                    Some(true) => return Ok(true),
                    Some(false) => {}
                    None => pending.push_back((cell, near, end, 0, rect)),
                }
            }
            near = near.max(end);
            if near >= far {
                break;
            }
            for axis in 0..2 {
                if exits[axis] <= end {
                    let d = if axis == 0 { direction.x } else { direction.z };
                    cell[axis] = cell[axis]
                        .checked_add_signed(if d > 0.0 { 1 } else { -1 })
                        .filter(|index| *index < self.domain_resolution() - 1)
                        .ok_or("terrain cell traversal left its clipped interval")?;
                }
            }
        }
        drop(coarse_span);
        // Search all coarse cells before refining uncertain boundaries. A
        // grazing contact must not consume the budget before a later definite
        // blocker can be found. Breadth-first refinement has the same property
        // across unresolved cells and its storage is bounded by the work quota.
        let _refinement_span = bevy::log::debug_span!("terrain_occlusion_refinement").entered();
        while let Some((cell, a, b, depth, rect)) = pending.pop_front() {
            if depth != 0 {
                let key = CellBoundsKey::Rectangle {
                    cell,
                    coordinates: rect.map(f64::to_bits),
                };
                match classify(key, a, b, rect)? {
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
            let l = ray
                .clip([a, b], low)
                .map(|[n, f]| (cell, n, f, depth + 1, low));
            let h = ray
                .clip([a, b], high)
                .map(|[n, f]| (cell, n, f, depth + 1, high));
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
    fn regional_solar_batch_shares_bounded_enclosures_deterministically() {
        use lunco_geotiff::{GeoTransform, LunarFrame};
        use lunco_terrain_bake::regional::RegisteredRegionalSurface;

        let geo = |half: f64, res| {
            GeoTransform::centred_square(half * 2.0, res, 1_737_400.0, 26.0, 3.0)
                .with_frame(LunarFrame::MoonMe)
        };
        let local = Arc::new(HeightGrid::new_flat(512, 500.0));
        let regional = Arc::new(HeightGrid::new_flat(801, 8000.0));
        let pair = RegisteredRegionalSurface::new(
            local,
            geo(500.0, 512),
            regional,
            geo(8000.0, 801),
            500.0,
            0.0,
        )
        .unwrap();
        let surface = SurfaceOracle::registered(Arc::new(pair), Vec::new());
        let budget = SurfaceOcclusionBudget::default();
        let direction = DVec3::new(0.98538, 0.14716, 0.08582).normalize();
        let mut cache = SurfaceOcclusionCache::default();
        let mut used = Vec::new();
        for _ in 0..2 {
            cache.begin_batch();
            let mut remaining = budget.max_tick_intervals;
            for sample in 0..9 {
                let origin = DVec3::new(-380.03, 30.0, -380.50 + sample as f64 * 0.1);
                assert_eq!(
                    surface.occludes_ray(origin, direction, budget, &mut remaining, &mut cache),
                    Ok(false)
                );
            }
            used.push(budget.max_tick_intervals - remaining);
        }
        assert_eq!(used[0], used[1]);
        assert!(used[0] < budget.max_tick_intervals);
        cache.begin_batch();
        let mut remaining = budget.max_tick_intervals;
        assert_eq!(
            surface.occludes_ray(
                DVec3::new(-380.03, -1.0, -380.5),
                direction,
                budget,
                &mut remaining,
                &mut cache,
            ),
            Ok(true)
        );
        cache.begin_batch();
        let mut remaining = 1;
        assert!(
            surface
                .occludes_ray(
                    DVec3::new(-380.03, 30.0, -380.5),
                    direction,
                    budget,
                    &mut remaining,
                    &mut cache,
                )
                .is_err()
        );
    }

    #[test]
    fn regional_solar_ray_bounds_only_its_fine_grid_footprint() {
        use lunco_geotiff::{GeoTransform, LunarFrame};
        use lunco_terrain_bake::regional::RegisteredRegionalSurface;

        let geo = |half: f64, res| {
            GeoTransform::centred_square(half * 2.0, res, 1_737_400.0, 26.0, 3.0)
                .with_frame(LunarFrame::MoonMe)
        };
        let local = Arc::new(HeightGrid::new_flat(512, 500.0));
        let regional = Arc::new(HeightGrid::new_flat(801, 8000.0));
        let pair = RegisteredRegionalSurface::new(
            local,
            geo(500.0, 512),
            regional,
            geo(8000.0, 801),
            500.0,
            0.0,
        )
        .unwrap();
        let surface = SurfaceOracle::registered(Arc::new(pair), Vec::new());
        let budget = SurfaceOcclusionBudget::default();
        let origin = DVec3::new(-380.03, 30.0, -380.50);
        let direction = DVec3::new(0.98538, 0.14716, 0.08582).normalize();
        let mut cache = SurfaceOcclusionCache::default();
        let mut remaining = budget.max_tick_intervals;
        assert_eq!(
            surface.occludes_ray(origin, direction, budget, &mut remaining, &mut cache),
            Ok(false)
        );
        let cold_remaining = remaining;
        remaining = budget.max_tick_intervals;
        assert_eq!(
            surface.occludes_ray(origin, direction, budget, &mut remaining, &mut cache),
            Ok(false)
        );
        assert_eq!(remaining, cold_remaining);
    }

    #[test]
    fn registered_surface_occlusion_and_cache_work_are_consistent() {
        use lunco_geotiff::{GeoTransform, LunarFrame};
        use lunco_terrain_bake::regional::RegisteredRegionalSurface;

        let local = Arc::new(HeightGrid::new_flat(9, 4.0));
        let mut regional = HeightGrid::new_flat(17, 16.0);
        for z in 0..regional.res {
            regional.heights[z * regional.res + 13] = 10.0;
        }
        let geo = |half: f64, res| {
            GeoTransform::centred_square(half * 2.0, res, 1_737_400.0, 26.0, 3.0)
                .with_frame(LunarFrame::MoonMe)
        };
        let pair = RegisteredRegionalSurface::new(
            local,
            geo(4.0, 9),
            Arc::new(regional),
            geo(16.0, 17),
            3.0,
            0.0,
        )
        .unwrap();
        let surface = SurfaceOracle::registered(Arc::new(pair), Vec::new());
        let sampled = crate::surface_query::height_in_footprint(
            &surface,
            lunco_spatial::coords::GridPos(DVec3::new(10.0, 0.0, 0.0)),
        );
        assert!((sampled.unwrap() - 10.0).abs() < 1e-9);
        assert!(
            crate::surface_query::height_in_footprint(
                &surface,
                lunco_spatial::coords::GridPos(DVec3::new(17.0, 0.0, 0.0)),
            )
            .is_none()
        );
        let hit =
            crate::oracle::raycast_surface(&surface, DVec3::new(10.0, 20.0, 0.0), -DVec3::Y, 30.0)
                .unwrap()
                .unwrap();
        assert!((hit.y - 10.0).abs() < 0.01);
        let budget = SurfaceOcclusionBudget::default();
        let mut cache = SurfaceOcclusionCache::default();
        for (y, expected) in [(2.0, true), (20.0, false)] {
            let mut cold_remaining = budget.max_tick_intervals;
            let cold = surface
                .occludes_ray(
                    DVec3::new(-15.0, y, 0.3),
                    DVec3::X,
                    budget,
                    &mut cold_remaining,
                    &mut cache,
                )
                .unwrap();
            let mut warm_remaining = budget.max_tick_intervals;
            let warm = surface
                .occludes_ray(
                    DVec3::new(-15.0, y, 0.3),
                    DVec3::X,
                    budget,
                    &mut warm_remaining,
                    &mut cache,
                )
                .unwrap();
            assert_eq!(cold, expected);
            assert_eq!((warm, warm_remaining), (cold, cold_remaining));
        }
        let mut cold_budget = 1;
        let mut warm_budget = 1;
        let cold = surface.occludes_ray(
            DVec3::new(-15.0, 20.0, 0.3),
            DVec3::X,
            budget,
            &mut cold_budget,
            &mut SurfaceOcclusionCache::default(),
        );
        let warm = surface.occludes_ray(
            DVec3::new(-15.0, 20.0, 0.3),
            DVec3::X,
            budget,
            &mut warm_budget,
            &mut cache,
        );
        assert!(cold.is_err() && warm.is_err());
        assert_eq!(cold_budget, warm_budget);
    }

    #[test]
    fn certified_cache_preserves_quotas_capacity_and_surface_invalidation() {
        let surface = SurfaceOracle::bare(Arc::new(HeightGrid::new_flat(129, 64.0)));
        let mut raised = HeightGrid::new_flat(129, 64.0);
        raised.heights.fill(10.0);
        let raised = SurfaceOracle::bare(Arc::new(raised));
        for capacity in [1, 3, 32, 65536] {
            let budget = SurfaceOcclusionBudget {
                max_cached_cells: capacity,
                ..Default::default()
            };
            for axis in [DVec3::X, DVec3::Z] {
                for sign in [-1.0, 1.0] {
                    let direction = sign * axis;
                    let origin = -63.5 * direction
                        + DVec3::Y * 2.0
                        + if axis == DVec3::X { DVec3::Z } else { DVec3::X } * 0.25;
                    let mut cache = SurfaceOcclusionCache::default();
                    let mut remaining = budget.max_tick_intervals;
                    assert_eq!(
                        surface.occludes_ray(origin, direction, budget, &mut remaining, &mut cache),
                        Ok(false)
                    );
                    for (ray_quota, tick_quota) in [
                        (300, 400),
                        (3, 5),
                        (5, 3),
                        (3, 3),
                        (128, 128),
                        (127, 200),
                        (200, 127),
                    ] {
                        let budget = SurfaceOcclusionBudget {
                            max_intervals: ray_quota,
                            ..budget
                        };
                        let mut cold_remaining = tick_quota;
                        let mut warm_remaining = tick_quota;
                        let cold = surface.occludes_ray(
                            origin,
                            direction,
                            budget,
                            &mut cold_remaining,
                            &mut SurfaceOcclusionCache::default(),
                        );
                        let warm = surface.occludes_ray(
                            origin,
                            direction,
                            budget,
                            &mut warm_remaining,
                            &mut cache,
                        );
                        assert_eq!(
                            (warm, warm_remaining),
                            (cold.clone(), cold_remaining),
                            "cache residency must not change quota admission: capacity={capacity}, ray={ray_quota}, tick={tick_quota}"
                        );
                        if (ray_quota, tick_quota) == (3, 5) {
                            assert_eq!(
                                (cold, cold_remaining),
                                (Err("terrain occlusion interval budget exceeded".into()), 1)
                            );
                        } else if (ray_quota, tick_quota) == (5, 3) {
                            assert_eq!(
                                (cold, cold_remaining),
                                (Err("solar fixed-tick interval budget exceeded".into()), 0)
                            );
                        }
                        assert!(cache.cells.len() <= capacity && cache.order.len() <= capacity);
                    }
                    let grazing = SurfaceOcclusionBudget {
                        max_depth: 4,
                        max_intervals: 8,
                        ..budget
                    };
                    let origin = origin - DVec3::Y * 2.0;
                    let mut cold_remaining = 100;
                    let mut warm_remaining = 100;
                    let cold = surface.occludes_ray(
                        origin,
                        direction,
                        grazing,
                        &mut cold_remaining,
                        &mut SurfaceOcclusionCache::default(),
                    );
                    assert!(
                        cold.is_err(),
                        "unresolved grazing geometry must remain unavailable"
                    );
                    assert_eq!(
                        (
                            surface.occludes_ray(
                                origin,
                                direction,
                                grazing,
                                &mut warm_remaining,
                                &mut cache
                            ),
                            warm_remaining
                        ),
                        (cold, cold_remaining)
                    );
                    let mut remaining = budget.max_tick_intervals;
                    assert_eq!(
                        raised.occludes_ray(
                            origin + DVec3::Y * 2.0,
                            direction,
                            budget,
                            &mut remaining,
                            &mut cache
                        ),
                        Ok(true),
                        "a replacement surface must invalidate every retained certificate"
                    );
                }
            }
        }
    }

    #[test]
    fn certified_cache_matches_fresh_caches_for_moving_diagonal_rays() {
        let mut grid = HeightGrid::new_flat(33, 16.0);
        for z in 0..grid.res {
            for x in 0..grid.res {
                grid.heights[z * grid.res + x] =
                    (x as f64 * 0.7).sin() * 0.1 + (z as f64 * 0.43).cos() * 0.15;
            }
        }
        let surface = SurfaceOracle::bare(Arc::new(grid));
        for capacity in [4, 65536] {
            let budget = SurfaceOcclusionBudget {
                max_cached_cells: capacity,
                ..Default::default()
            };
            let mut cache = SurfaceOcclusionCache::default();
            for direction in [
                DVec3::new(1.0, 0.0, 0.001),
                DVec3::new(0.001, 0.0, 1.0),
                DVec3::new(1.0, 0.0, 1.0),
                DVec3::new(-0.7388253175039277, 0.141, 0.6592623470722384),
            ] {
                for sx in [-1.0, 1.0] {
                    for sz in [-1.0, 1.0] {
                        let direction = (direction * DVec3::new(sx, 1.0, sz)).normalize();
                        for height in [-0.5, 0.0, 0.2, 1.0, 5.0] {
                            for offset in [0.0, 1e-8, -1e-8, 0.1] {
                                let origin =
                                    DVec3::new(-15.5 * sx + offset, height, -15.5 * sz + offset);
                                let mut cold_remaining = budget.max_tick_intervals;
                                let mut warm_remaining = budget.max_tick_intervals;
                                let cold = surface.occludes_ray(
                                    origin,
                                    direction,
                                    budget,
                                    &mut cold_remaining,
                                    &mut SurfaceOcclusionCache::default(),
                                );
                                let warm = surface.occludes_ray(
                                    origin,
                                    direction,
                                    budget,
                                    &mut warm_remaining,
                                    &mut cache,
                                );
                                assert_eq!(
                                    (warm, warm_remaining),
                                    (cold, cold_remaining),
                                    "moving rays retain geometry and quota: capacity={capacity}, origin={origin:?}, direction={direction:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn certified_occlusion_excludes_transverse_relief_from_grazing_rays() {
        let mut grid = HeightGrid::new_flat(17, 8.0);
        for z in 0..grid.res {
            for x in 0..grid.res {
                grid.heights[z * grid.res + x] = (z as f64 - 8.0) * 1000.0;
            }
        }
        let surface = SurfaceOracle::bare(Arc::new(grid));
        let budget = SurfaceOcclusionBudget {
            max_intervals: 32,
            ..Default::default()
        };
        let mut remaining = budget.max_tick_intervals;
        assert_eq!(
            surface.occludes_ray(
                DVec3::new(-7.5, 0.001, 0.0),
                DVec3::X,
                budget,
                &mut remaining,
                &mut SurfaceOcclusionCache::default(),
            ),
            Ok(false)
        );
    }

    #[test]
    fn certified_occlusion_advances_across_rounded_cell_boundaries() {
        let surface = SurfaceOracle::new(Arc::new(HeightGrid::new_flat(257, 500.0)), vec![]);
        let budget = SurfaceOcclusionBudget::default();
        let x = -0.7388253175039277_f64;
        let z = 0.6592623470722384_f64;
        let y = (1.0 - x * x - z * z).sqrt();
        for sx in [-1.0, 1.0] {
            for sz in [-1.0, 1.0] {
                let mut remaining = budget.max_tick_intervals;
                assert_eq!(
                    surface.occludes_ray(
                        DVec3::new(sx * 9.132788318890832, 1.5, sz * 2.583824965637831),
                        DVec3::new(sx * x, y, sz * z),
                        budget,
                        &mut remaining,
                        &mut SurfaceOcclusionCache::default(),
                    ),
                    Ok(false),
                    "a finite upward ray must cross every raster boundary"
                );
            }
        }
    }

    #[test]
    fn certified_occlusion_reuses_shared_interval_endpoints() {
        struct CountedSurface(Arc<std::sync::atomic::AtomicUsize>);
        impl HeightModifier for CountedSurface {
            fn apply(&self, _: f64, _: f64, height: f64) -> f64 {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                height
            }
            fn height_bounds(&self, rect: [f64; 4], _: [f64; 2]) -> Option<[f64; 2]> {
                // Coarse intervals need actual samples; their children certify
                // the same flat surface without further point evaluations.
                Some(if rect[2] - rect[0] >= 1.0 {
                    [0.0, 2.0]
                } else {
                    [0.0, 0.0]
                })
            }
        }
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let surface = SurfaceOracle::new(
            Arc::new(HeightGrid::new_flat(9, 4.0)),
            vec![HeightContribution {
                modifier: Arc::new(CountedSurface(calls.clone())),
                content_key: 1,
            }],
        );
        let budget = SurfaceOcclusionBudget::default();
        let mut remaining = budget.max_tick_intervals;
        assert!(
            !surface
                .occludes_ray(
                    DVec3::new(-3.5, 1.0, 0.1),
                    DVec3::X,
                    budget,
                    &mut remaining,
                    &mut SurfaceOcclusionCache::default(),
                )
                .unwrap()
        );
        // The half-cell entry certifies from its ray footprint. The other seven
        // coarse spans have eight distinct endpoints and seven midpoints.
        // Repeated endpoints must not evaluate the modifier again.
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 15);
        assert_eq!(budget.max_tick_intervals - remaining, 22);
    }

    #[test]
    fn certified_occlusion_reuses_rectangular_covers_for_crossing_rays() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct CountedRidge(Arc<AtomicUsize>, Arc<AtomicUsize>);
        impl HeightModifier for CountedRidge {
            fn apply(&self, x: f64, z: f64, height: f64) -> f64 {
                self.1.fetch_add(1, Ordering::Relaxed);
                height + (x - z).powi(2)
            }
            fn height_bounds(&self, rect: [f64; 4], input: [f64; 2]) -> Option<[f64; 2]> {
                self.0.fetch_add(1, Ordering::Relaxed);
                let low = rect[0] - rect[3];
                let high = rect[2] - rect[1];
                Some([
                    input[0]
                        + if low <= 0.0 && high >= 0.0 {
                            0.0
                        } else {
                            low.powi(2).min(high.powi(2))
                        },
                    input[1] + low.powi(2).max(high.powi(2)),
                ])
            }
        }
        let bounds = Arc::new(AtomicUsize::new(0));
        let points = Arc::new(AtomicUsize::new(0));
        let surface = SurfaceOracle::new(
            Arc::new(HeightGrid::new_flat(17, 4.0)),
            vec![HeightContribution {
                modifier: Arc::new(CountedRidge(bounds.clone(), points.clone())),
                content_key: 1,
            }],
        );
        let budget = SurfaceOcclusionBudget::default();
        let direction = DVec3::new(0.5, 0.5_f64.sqrt(), 0.5);
        let mut cache = SurfaceOcclusionCache::default();
        let mut remaining = budget.max_tick_intervals;
        assert!(
            !surface
                .occludes_ray(
                    DVec3::new(-4.0, 0.07, -4.0),
                    direction,
                    budget,
                    &mut remaining,
                    &mut cache
                )
                .unwrap()
        );
        let prepared = [
            bounds.load(Ordering::Relaxed),
            points.load(Ordering::Relaxed),
        ];
        for step in 1..=3 {
            let offset = step as f64 / 1048576.0;
            let mut remaining = budget.max_tick_intervals;
            assert!(
                !surface
                    .occludes_ray(
                        DVec3::new(-4.0 + offset, 0.07, -4.0 + offset),
                        direction,
                        budget,
                        &mut remaining,
                        &mut cache
                    )
                    .unwrap()
            );
        }
        let reused = [
            bounds.load(Ordering::Relaxed),
            points.load(Ordering::Relaxed),
        ];
        eprintln!(
            "crossing-ray enclosure evaluations: prepared={prepared:?}, after_moves={reused:?}"
        );
        assert_eq!(
            reused, prepared,
            "a ray crossing cell midplanes must reuse a complete conservative rectangle cover"
        );
        let mut remaining = budget.max_tick_intervals;
        assert!(
            surface
                .occludes_ray(
                    DVec3::new(-4.0, 0.07, -3.6),
                    direction,
                    budget,
                    &mut remaining,
                    &mut cache
                )
                .unwrap(),
            "a ray outside the clear cover must still observe the real ridge"
        );
    }

    #[test]
    fn certified_occlusion_reuses_enclosures_covering_moving_ray_footprints() {
        struct CountedSlope(Arc<std::sync::atomic::AtomicUsize>);
        impl HeightModifier for CountedSlope {
            fn apply(&self, _: f64, z: f64, _: f64) -> f64 {
                100.0 * z
            }
            fn height_bounds(&self, rect: [f64; 4], _: [f64; 2]) -> Option<[f64; 2]> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Some([100.0 * rect[1], 100.0 * rect[3]])
            }
        }
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let surface = SurfaceOracle::new(
            Arc::new(HeightGrid::new_flat(17, 4.0)),
            vec![HeightContribution {
                modifier: Arc::new(CountedSlope(calls.clone())),
                content_key: 1,
            }],
        );
        let budget = SurfaceOcclusionBudget::default();
        let mut cache = SurfaceOcclusionCache::default();
        let mut remaining = budget.max_tick_intervals;
        assert!(
            !surface
                .occludes_ray(
                    DVec3::new(-3.5, 11.0, 0.1),
                    DVec3::X,
                    budget,
                    &mut remaining,
                    &mut cache
                )
                .unwrap()
        );
        let prepared = calls.load(std::sync::atomic::Ordering::Relaxed);
        for offset in [0.000001, -0.000001, 0.000002] {
            let mut remaining = budget.max_tick_intervals;
            assert!(
                !surface
                    .occludes_ray(
                        DVec3::new(-3.5, 11.0, 0.1 + offset),
                        DVec3::X,
                        budget,
                        &mut remaining,
                        &mut cache
                    )
                    .unwrap()
            );
        }
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::Relaxed),
            prepared,
            "nearby rays must reuse a conservative enclosure covering their new footprints"
        );
        // A new ray outside those clear certificates still sees the real slope.
        let mut remaining = budget.max_tick_intervals;
        assert!(
            surface
                .occludes_ray(
                    DVec3::new(-3.5, 11.0, 0.2),
                    DVec3::X,
                    budget,
                    &mut remaining,
                    &mut cache
                )
                .unwrap()
        );
    }

    #[test]
    #[ignore = "bounded moving-ray cache timing diagnostic"]
    fn certified_cache_moving_ray_diagnostic() {
        for resolution in [129, 257] {
            let surface = SurfaceOracle::new(
                Arc::new(HeightGrid::new_flat(resolution, 128.0)),
                vec![HeightContribution {
                    modifier: Arc::new(lunco_terrain_core::Overzoom::default()),
                    content_key: 7,
                }],
            );
            let direction = DVec3::new(0.0, 0.163, 0.987).normalize();
            for capacity in [256, 65536] {
                let budget = SurfaceOcclusionBudget {
                    max_cached_cells: capacity,
                    ..Default::default()
                };
                let mut cache = SurfaceOcclusionCache::default();
                let mut intervals = 0;
                let started = std::time::Instant::now();
                let ticks = if capacity == 256 { 32 } else { 512 };
                for tick in 0..ticks {
                    let mut remaining = budget.max_tick_intervals;
                    for x in [-40.0, -20.0, 0.0, 20.0] {
                        for z in [-1.0, 0.0, 1.0] {
                            for dx in [-2.0, 0.0, 2.0] {
                                let origin = DVec3::new(x + dx + tick as f64 * 0.001, 1.5, z);
                                assert_eq!(
                                    std::hint::black_box(surface.occludes_ray(
                                        origin,
                                        direction,
                                        budget,
                                        &mut remaining,
                                        &mut cache,
                                    )),
                                    Ok(false)
                                );
                            }
                        }
                    }
                    intervals += budget.max_tick_intervals - remaining;
                    assert_eq!(cache.cells.len(), cache.order.len());
                    assert!(cache.cells.len() <= capacity);
                }
                eprintln!(
                    "moving-ray diagnostic: resolution={resolution}, capacity={capacity}, ticks={ticks}, rays={}, intervals={intervals}, cached={}, elapsed_ns={}",
                    ticks * 36,
                    cache.cells.len(),
                    started.elapsed().as_nanos()
                );
            }
        }
    }

    #[test]
    fn certified_occlusion_admits_procedural_rays_with_bounded_refinement() {
        let surface = SurfaceOracle::new(
            Arc::new(HeightGrid::new_flat(129, 128.0)),
            vec![HeightContribution {
                modifier: Arc::new(lunco_terrain_core::Overzoom::default()),
                content_key: 7,
            }],
        );
        let budget = SurfaceOcclusionBudget {
            max_intervals: 200,
            ..Default::default()
        };
        let direction = DVec3::new(0.0, 0.163, 0.987).normalize();
        let mut cache = SurfaceOcclusionCache::default();
        let mut remaining = budget.max_tick_intervals;
        for x in [-42.0, -20.0, 2.0, 22.0] {
            for z in [-1.0, 0.0, 1.0] {
                let origin = DVec3::new(x, 1.5, z);
                assert_eq!(
                    surface.occludes_ray(origin, direction, budget, &mut remaining, &mut cache),
                    Ok(false)
                );
            }
        }
        let mut remaining = budget.max_tick_intervals;
        assert_eq!(
            surface.occludes_ray(
                DVec3::new(-42.0, 1.5, -1.0),
                direction,
                SurfaceOcclusionBudget {
                    max_intervals: 1,
                    ..budget
                },
                &mut remaining,
                &mut cache,
            ),
            Err("terrain occlusion interval budget exceeded".into())
        );
    }

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
