//! Carve field — the **carve/mask channel**: where the surface is *absent*.
//!
//! Height modifiers ([`CraterField`](crate::crater::CraterField)) move a
//! single-valued surface up and down. Caves, skylights, pits, and lava tubes cannot
//! be expressed that way — a column can be solid, then void, then solid again
//! (multi-valued). The carve channel handles them by marking the surface **removed**
//! where a **signed-distance solid** breaches it: the visual baker drops the covered
//! triangles, and the collider skips them (a heightfield can't hold a hole, so a
//! breached tile swaps to a trimesh collider — a runtime step layered on top of this
//! pure core).
//!
//! [`CarveField`] is the same shape as `CraterField`: a list of primitives folded
//! into one field, with a **deterministic XZ bucket index** so `sdf`/`is_open`
//! evaluate only the primitives near the query, not all of them. That is exactly
//! what makes carves **dynamic**: a tool bores a tunnel by appending one
//! [`CarvePrimitive::Capsule`] to the list (authored as a `lunco:layer` USD op), and
//! the field recomposes with no rebuild of anything else — carves edit the same way
//! height brushes do. Primitives combine with a **smooth union** so intersecting
//! bores blend into one organic void instead of showing a hard seam.

use crate::source::HeightSource;

/// Soft cap on dense bucket-grid cells (mirrors [`crate::crater`]). The cell size is
/// derived from the largest primitive extent, so a field of ONLY tiny carves spread
/// over kilometres would otherwise mint millions of cells; doubling the cell size
/// until the grid fits is output-neutral (a larger cell only makes a query consider
/// more candidates, and a candidate beyond `2·smooth_k` folds as an exact `min`, so
/// it cannot change the result).
const MAX_BUCKET_CELLS: u128 = 1 << 21;

/// Largest world coordinate / radius a carve primitive may carry (metres). A
/// scripted or USD-authored divide-by-zero yields `inf`/`NaN`, and `(x / cell) as
/// i64` SATURATES for those — the cell AABB becomes `[i64::MIN, i64::MAX]`, whose
/// span overflows (debug panic) or wraps to a zero-sized grid the CSR fill then
/// indexes out of bounds (release panic). Anything past this bound is not a carve
/// anyone authored; such primitives are DROPPED at construction (they still exist
/// in the caller's list — the field simply carves nothing for them).
const MAX_COORD: f64 = 1e12;

/// A signed-distance primitive whose interior is void (carved away). Distances are
/// in metres; negative inside the solid, zero on its surface, positive outside.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CarvePrimitive {
    /// A ball — a chamber or a skylight bulb.
    Sphere { center: [f64; 3], radius: f64 },
    /// A capsule (round-ended cylinder) from `a` to `b` — a tunnel / lava-tube segment.
    Capsule { a: [f64; 3], b: [f64; 3], radius: f64 },
}

impl CarvePrimitive {
    /// Signed distance from `p` to this solid (negative inside).
    pub fn sdf(&self, p: [f64; 3]) -> f64 {
        match *self {
            CarvePrimitive::Sphere { center, radius } => length(sub(p, center)) - radius,
            CarvePrimitive::Capsule { a, b, radius } => {
                let pa = sub(p, a);
                let ba = sub(b, a);
                let bb = dot(ba, ba);
                // Project p onto the segment, clamped to its ends.
                let t = if bb > 0.0 { (dot(pa, ba) / bb).clamp(0.0, 1.0) } else { 0.0 };
                let closest = [a[0] + ba[0] * t, a[1] + ba[1] * t, a[2] + ba[2] * t];
                length(sub(p, closest)) - radius
            }
        }
    }

    /// Axis-aligned XZ footprint `(min_x, min_z, max_x, max_z)` of the solid — its
    /// shadow on the ground plane, radius included. Used to bucket the primitive.
    fn xz_bounds(&self) -> (f64, f64, f64, f64) {
        match *self {
            CarvePrimitive::Sphere { center, radius } => {
                (center[0] - radius, center[2] - radius, center[0] + radius, center[2] + radius)
            }
            CarvePrimitive::Capsule { a, b, radius } => (
                a[0].min(b[0]) - radius,
                a[2].min(b[2]) - radius,
                a[0].max(b[0]) + radius,
                a[2].max(b[2]) + radius,
            ),
        }
    }

    /// Largest XZ extent (metres) — sizes the bucket cell.
    fn xz_extent(&self) -> f64 {
        let (x0, z0, x1, z1) = self.xz_bounds();
        (x1 - x0).max(z1 - z0)
    }

    /// Is this primitive *bucketable* — finite and inside [`MAX_COORD`]? A
    /// non-finite radius/centre (a divide-by-zero upstream) saturates the cell
    /// index; see [`MAX_COORD`]. Non-bucketable primitives are dropped.
    fn is_bucketable(&self) -> bool {
        let (x0, z0, x1, z1) = self.xz_bounds();
        [x0, z0, x1, z1].iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD)
    }
}

/// A composable set of carve primitives folded by smooth union, with a
/// deterministic XZ bucket index for bounded evaluation. Append a primitive to carve
/// more (dynamic edits); the field stays a pure function of its primitive list.
#[derive(Debug, Clone)]
pub struct CarveField {
    prims: Vec<CarvePrimitive>,
    /// Smooth-union radius (metres). `0` = hard union (`min`).
    smooth_k: f64,
    /// `1 / cell_size`, precomputed: `sdf`/`is_open` are the baker + collider inner
    /// loop, and `cell_of` did TWO f64 divisions per sample — now two multiplies.
    /// The SAME reciprocal is used to bucket the primitives at build time, so build
    /// and query agree bit-for-bit on which cell a coordinate falls in.
    inv_cell_size: f64,
    /// Bucket-grid AABB origin: the cell coordinate of grid slot `(0, 0)`.
    bucket_min: (i64, i64),
    /// Bucket-grid dimensions (cells).
    bucket_nx: usize,
    bucket_nz: usize,
    /// Dense row-major CSR bucket grid over the field's cell AABB (same layout as
    /// [`crate::crater`]'s index): cell `(cx, cz)` holds
    /// `entries[starts[k]..starts[k + 1]]` (`k = (cz − min_z)·nx + (cx − min_x)`) —
    /// indices into `prims` whose expanded footprint overlaps that cell. The per-
    /// sample `sdf`/`is_open` lookup is the baker + collider inner loop, so the tuple
    /// hash a `HashMap<(i64, i64), Vec<u32>>` paid per sample cost more than the two
    /// subtracts + multiply the dense grid pays. Queries outside the AABB → empty.
    bucket_starts: Vec<u32>,
    bucket_entries: Vec<u32>,
}

impl CarveField {
    /// Build a carve field from `prims`, blended with smooth-union radius `smooth_k`
    /// (`0` for a hard union). An empty field carves nothing.
    pub fn new(prims: Vec<CarvePrimitive>, smooth_k: f64) -> Self {
        let smooth_k = smooth_k.max(0.0);
        // Drop primitives whose XZ footprint is non-finite / absurdly far (see
        // [`MAX_COORD`]): their cell index saturates and the CSR build would panic.
        // Dropping preserves the ASCENDING order of the survivors, which the
        // (non-associative) `smin` fold in `sdf` depends on for bit-exactness.
        let prims: Vec<CarvePrimitive> = prims.into_iter().filter(|p| p.is_bucketable()).collect();
        // Cell just big enough that a primitive (plus its blend margin) spans a
        // bounded neighbourhood of cells.
        let max_extent = prims.iter().map(|p| p.xz_extent()).fold(0.0_f64, f64::max);
        let mut cell_size = (max_extent + 4.0 * smooth_k).max(1.0);
        // Expand each footprint by `2·smooth_k`: with the *gated* smooth-min (exact
        // `min` once the nearest surface is ≥ k away), a primitive can only blend
        // within `2k` of the query — so a `2k` margin captures every primitive the
        // full fold would, making the bucketing exact.
        let m = 2.0 * smooth_k;
        let cell_box = |p: &CarvePrimitive, inv: f64| -> (i64, i64, i64, i64) {
            let (x0, z0, x1, z1) = p.xz_bounds();
            let (min_cx, min_cz) = cell_of(x0 - m, z0 - m, inv);
            let (max_cx, max_cz) = cell_of(x1 + m, z1 + m, inv);
            (min_cx, min_cz, max_cx, max_cz)
        };
        // Grid AABB over every primitive's cell box; grow the cell until the dense
        // grid stays bounded (see [`MAX_BUCKET_CELLS`] — output-neutral).
        let (bucket_min, bucket_nx, bucket_nz, inv_cell_size) = loop {
            let inv = 1.0 / cell_size;
            let (mut min_cx, mut min_cz) = (i64::MAX, i64::MAX);
            let (mut max_cx, mut max_cz) = (i64::MIN, i64::MIN);
            for p in &prims {
                let (x0, z0, x1, z1) = cell_box(p, inv);
                min_cx = min_cx.min(x0);
                min_cz = min_cz.min(z0);
                max_cx = max_cx.max(x1);
                max_cz = max_cz.max(z1);
            }
            if min_cx > max_cx {
                break ((0, 0), 0, 0, inv); // empty field
            }
            // i128 so a saturated span can never overflow the subtraction.
            let nx = (max_cx as i128 - min_cx as i128 + 1) as u128;
            let nz = (max_cz as i128 - min_cz as i128 + 1) as u128;
            if nx * nz <= MAX_BUCKET_CELLS {
                break ((min_cx, min_cz), nx as usize, nz as usize, inv);
            }
            cell_size *= 2.0;
        };
        // CSR fill: count per cell straight into `starts[k + 1]`, prefix-sum in
        // place, then place indices in ascending primitive order (the same order the
        // old HashMap push gave — the `smin` fold is non-associative, so this
        // ordering is the bit-exactness invariant). Counting into `bucket_starts`
        // instead of a separate `counts` Vec saves a full cells-sized allocation
        // (~8 MB at the MAX_BUCKET_CELLS ceiling) on every carve edit.
        let cells = bucket_nx * bucket_nz;
        let slot = |cx: i64, cz: i64| -> usize {
            (cz - bucket_min.1) as usize * bucket_nx + (cx - bucket_min.0) as usize
        };
        let mut bucket_starts = vec![0u32; cells + 1];
        if cells > 0 {
            for p in &prims {
                let (x0, z0, x1, z1) = cell_box(p, inv_cell_size);
                if x0 > x1 {
                    continue;
                }
                for cz in z0..=z1 {
                    for cx in x0..=x1 {
                        bucket_starts[slot(cx, cz) + 1] += 1;
                    }
                }
            }
            for k in 0..cells {
                bucket_starts[k + 1] += bucket_starts[k];
            }
        }
        let mut cursor: Vec<u32> = bucket_starts[..cells].to_vec();
        let mut bucket_entries = vec![0u32; bucket_starts[cells] as usize];
        if cells > 0 {
            for (i, p) in prims.iter().enumerate() {
                let (x0, z0, x1, z1) = cell_box(p, inv_cell_size);
                if x0 > x1 {
                    continue;
                }
                for cz in z0..=z1 {
                    for cx in x0..=x1 {
                        let k = slot(cx, cz);
                        bucket_entries[cursor[k] as usize] = i as u32;
                        cursor[k] += 1;
                    }
                }
            }
        }
        Self { prims, smooth_k, inv_cell_size, bucket_min, bucket_nx, bucket_nz, bucket_starts, bucket_entries }
    }

    /// Candidate primitives for bucket cell `(cx, cz)` — empty outside the grid AABB.
    #[inline]
    fn bucket(&self, cx: i64, cz: i64) -> &[u32] {
        let ux = cx.wrapping_sub(self.bucket_min.0);
        let uz = cz.wrapping_sub(self.bucket_min.1);
        if ux < 0 || uz < 0 || ux >= self.bucket_nx as i64 || uz >= self.bucket_nz as i64 {
            return &[];
        }
        let k = uz as usize * self.bucket_nx + ux as usize;
        &self.bucket_entries[self.bucket_starts[k] as usize..self.bucket_starts[k + 1] as usize]
    }

    /// Number of carve primitives the field actually carries — non-bucketable ones
    /// (non-finite / beyond [`MAX_COORD`]) were dropped at construction.
    pub fn primitive_count(&self) -> usize {
        self.prims.len()
    }

    /// Signed distance to the carved void at `p` (negative inside the void). Folds
    /// only the primitives bucketed near `p`; empty/away → `f64::INFINITY` (solid).
    pub fn sdf(&self, p: [f64; 3]) -> f64 {
        let (cx, cz) = cell_of(p[0], p[2], self.inv_cell_size);
        let indices = self.bucket(cx, cz);
        let mut d = f64::INFINITY;
        for &i in indices {
            d = smin(d, self.prims[i as usize].sdf(p), self.smooth_k);
        }
        d
    }

    /// Is `p` inside the carved void (surface removed here)?
    pub fn is_carved(&self, p: [f64; 3]) -> bool {
        self.sdf(p) < 0.0
    }

    /// Is the **surface** open at column `(x, z)` — i.e. does the void breach the
    /// ground at height `surface_y`? A skylight/mouth returns `true`, so the baker
    /// clips the tile and the collider swaps to a trimesh there.
    pub fn is_open(&self, x: f64, z: f64, surface_y: f64) -> bool {
        self.is_carved([x, surface_y, z])
    }

    /// Convenience: sample the surface from `src` and test whether the void breaches
    /// it at `(x, z)`. Ties the carve channel to the height oracle.
    pub fn is_open_on(&self, src: &dyn HeightSource, x: f64, z: f64) -> bool {
        self.is_open(x, z, src.height_at(x, z))
    }
}

/// **Gated** polynomial smooth-minimum: blends two distance fields over a radius `k`
/// so intersecting solids merge without a crease, but reverts to an exact `min` once
/// the nearer surface is at least `k` away. That locality is deliberate — carves far
/// apart must not blend — and it is what makes the field exactly bucketable: a
/// primitive can only influence points within `2k` of it, so a `2k` insertion margin
/// captures every blend. `k ≤ 0` degrades to hard `min`. Always `≤ min(a, b)`.
#[inline]
fn smin(a: f64, b: f64, k: f64) -> f64 {
    if k <= 0.0 {
        return a.min(b);
    }
    let m = a.min(b);
    if m >= k {
        return m; // nearest surface ≥ k away → no blend (exact, local)
    }
    let h = ((k - (a - b).abs()) / k).max(0.0);
    if h <= 0.0 {
        return m;
    }
    // Fade the blend out as the nearest surface recedes toward `k`; full blend when
    // inside the void (`m < 0`). Continuous with the `m ≥ k` branch at `m = k`.
    let g = ((k - m) / k).clamp(0.0, 1.0);
    m - h * h * k * 0.25 * g
}

#[inline]
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn length(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

/// Cell index of `(x, z)` — `inv` is `1 / cell_size` (a multiply, not a divide: this
/// runs twice per `sdf` sample). `as i64` SATURATES for non-finite inputs, which is
/// why [`CarveField::new`] drops non-bucketable primitives up front.
#[inline]
fn cell_of(x: f64, z: f64, inv: f64) -> (i64, i64) {
    ((x * inv).floor() as i64, (z * inv).floor() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sphere(c: [f64; 3], r: f64) -> CarvePrimitive {
        CarvePrimitive::Sphere { center: c, radius: r }
    }

    #[test]
    fn empty_field_carves_nothing() {
        let f = CarveField::new(vec![], 0.0);
        assert!(f.sdf([0.0, 0.0, 0.0]).is_infinite());
        assert!(!f.is_carved([1.0, 2.0, 3.0]));
    }

    #[test]
    fn sphere_inside_outside_surface() {
        let f = CarveField::new(vec![sphere([0.0, 0.0, 0.0], 10.0)], 0.0);
        assert!(f.is_carved([0.0, 0.0, 0.0]), "centre is void");
        assert!(f.sdf([0.0, 0.0, 0.0]) < 0.0);
        assert!(!f.is_carved([100.0, 0.0, 0.0]), "far is solid");
        // Surface at y=0 through the ball's equator → open (skylight).
        assert!(f.is_open(0.0, 0.0, 0.0));
        // Surface high above the ball → not breached.
        assert!(!f.is_open(0.0, 0.0, 50.0));
    }

    #[test]
    fn capsule_is_a_tunnel() {
        // A horizontal tube 4 m below the surface, radius 3.
        let tube = CarvePrimitive::Capsule { a: [-50.0, -4.0, 0.0], b: [50.0, -4.0, 0.0], radius: 3.0 };
        let f = CarveField::new(vec![tube], 0.0);
        assert!(f.is_carved([0.0, -4.0, 0.0]), "on the axis → void");
        assert!(f.is_carved([25.0, -4.0, 1.0]), "near the axis → void");
        assert!(!f.is_carved([0.0, -4.0, 20.0]), "off to the side → solid");
        assert!(!f.is_carved([200.0, -4.0, 0.0]), "past the end → solid");
        // Buried tube does not breach a surface at y=0 (4 m + radius 3 = top at -1).
        assert!(!f.is_open(0.0, 0.0, 0.0));
    }

    #[test]
    fn smooth_union_deepens_the_seam() {
        // Two overlapping spheres: the smooth field is ≤ the hard-min field between
        // them (the union bulges/deepens at the seam).
        let prims = vec![sphere([-4.0, 0.0, 0.0], 6.0), sphere([4.0, 0.0, 0.0], 6.0)];
        let hard = CarveField::new(prims.clone(), 0.0);
        let soft = CarveField::new(prims, 4.0);
        let seam = [0.0, 0.0, 0.0];
        assert!(soft.sdf(seam) <= hard.sdf(seam) + 1e-12, "smooth union must not be shallower");
    }

    #[test]
    fn bucketed_matches_brute_force() {
        // The index is an optimisation. The SDF only carries meaning near/inside the
        // voids, so the guarantee is: the **sign** (is-carved) matches the full fold
        // everywhere, and the **value** matches wherever the nearest surface is within
        // the `2k` capture radius — the region the field is actually used in. Far from
        // every void a bucket may be empty (returns `+∞` = solidly solid); the exact
        // metres-to-a-distant-cave is irrelevant.
        let prims = vec![
            sphere([0.0, 0.0, 0.0], 8.0),
            sphere([30.0, 2.0, -10.0], 12.0),
            CarvePrimitive::Capsule { a: [-40.0, -3.0, 20.0], b: [10.0, -3.0, 20.0], radius: 4.0 },
        ];
        let k = 3.0;
        let f = CarveField::new(prims.clone(), k);
        for gx in -60..60 {
            for gz in -60..60 {
                let p = [gx as f64 * 1.7, 0.0, gz as f64 * 1.7];
                let brute = prims.iter().fold(f64::INFINITY, |d, pr| smin(d, pr.sdf(p), k));
                // Sign is exact everywhere.
                assert_eq!(f.is_carved(p), brute < 0.0, "sign mismatch at {p:?}");
                // Value is exact wherever the field matters (nearest surface ≤ 2k).
                if brute < 2.0 * k {
                    assert!(
                        (f.sdf(p) - brute).abs() < 1e-9,
                        "near-field value mismatch at {p:?}: {} vs {brute}",
                        f.sdf(p)
                    );
                }
            }
        }
    }

    #[test]
    fn dynamic_append_carves_more() {
        // "Boring a tunnel" = building a new field with one more primitive. What was
        // solid becomes void; nothing else has to change.
        let before = CarveField::new(vec![sphere([0.0, 0.0, 0.0], 5.0)], 0.0);
        assert!(!before.is_carved([40.0, -4.0, 0.0]));
        let mut prims = vec![sphere([0.0, 0.0, 0.0], 5.0)];
        prims.push(CarvePrimitive::Capsule { a: [0.0, -4.0, 0.0], b: [80.0, -4.0, 0.0], radius: 3.0 });
        let after = CarveField::new(prims, 0.0);
        assert!(after.is_carved([40.0, -4.0, 0.0]), "the new tunnel carves the point");
    }

    #[test]
    fn deterministic() {
        let f = CarveField::new(vec![sphere([1.0, 2.0, 3.0], 7.0)], 2.0);
        assert_eq!(f.sdf([2.0, 1.0, 4.0]), f.sdf([2.0, 1.0, 4.0]));
    }

    #[test]
    fn non_finite_primitives_are_dropped_not_panicked_on() {
        // A scripted divide-by-zero (`radius = 1/0`) used to saturate the cell index
        // and blow the CSR build up (debug: subtract overflow; release: OOB index).
        for bad in [
            sphere([0.0, 0.0, 0.0], f64::INFINITY),
            sphere([0.0, 0.0, 0.0], f64::NAN),
            sphere([f64::INFINITY, 0.0, 0.0], 10.0),
            sphere([f64::NEG_INFINITY, 0.0, f64::NAN], 10.0),
            sphere([1e300, 0.0, 0.0], 1e300),
            CarvePrimitive::Capsule { a: [0.0, 0.0, 0.0], b: [f64::INFINITY, 0.0, 0.0], radius: 2.0 },
            CarvePrimitive::Capsule { a: [0.0, 0.0, 0.0], b: [1.0, 0.0, 0.0], radius: f64::INFINITY },
        ] {
            let f = CarveField::new(vec![bad], 3.0);
            assert_eq!(f.primitive_count(), 0, "non-finite primitive must be dropped: {bad:?}");
            assert!(f.sdf([0.0, 0.0, 0.0]).is_infinite());
            assert!(!f.is_carved([0.0, 0.0, 0.0]));
        }
    }

    #[test]
    fn a_bad_primitive_does_not_poison_the_good_ones() {
        let good = sphere([0.0, 0.0, 0.0], 10.0);
        let f = CarveField::new(vec![sphere([0.0, 0.0, 0.0], f64::INFINITY), good], 0.0);
        assert_eq!(f.primitive_count(), 1);
        assert!(f.is_carved([0.0, 0.0, 0.0]), "the finite sphere still carves");
        assert!(!f.is_carved([100.0, 0.0, 0.0]));
    }

    #[test]
    fn csr_entries_are_in_ascending_primitive_order() {
        // THE invariant: `smin` is non-associative, so the per-cell candidate list
        // must be folded in ascending primitive order (the order the old HashMap
        // push gave). Every bucket must therefore be strictly ascending.
        let prims = vec![
            sphere([0.0, 0.0, 0.0], 8.0),
            sphere([6.0, 0.0, 3.0], 9.0),
            CarvePrimitive::Capsule { a: [-20.0, 0.0, 0.0], b: [20.0, 0.0, 0.0], radius: 5.0 },
            sphere([-10.0, 0.0, -4.0], 7.0),
        ];
        let f = CarveField::new(prims.clone(), 2.0);
        let mut seen = 0usize;
        for cz in 0..f.bucket_nz as i64 {
            for cx in 0..f.bucket_nx as i64 {
                let b = f.bucket(cx + f.bucket_min.0, cz + f.bucket_min.1);
                assert!(b.windows(2).all(|w| w[0] < w[1]), "bucket not ascending: {b:?}");
                seen += b.len();
            }
        }
        assert!(seen >= prims.len(), "every primitive must be bucketed somewhere");
        // …and the fold matches the brute-force ascending fold BIT-for-bit where the
        // bucket holds every primitive (the near field).
        let k = 2.0;
        for gx in -10..10 {
            for gz in -10..10 {
                let p = [gx as f64, 0.0, gz as f64];
                let brute = prims.iter().fold(f64::INFINITY, |d, pr| smin(d, pr.sdf(p), k));
                if brute < 2.0 * k {
                    assert_eq!(f.sdf(p), brute, "fold order diverged at {p:?}");
                }
            }
        }
    }

    #[test]
    fn is_open_on_oracle() {
        use crate::source::AnalyticHeightSource;
        // A big void centred on the surface height at the origin breaches it.
        let src = AnalyticHeightSource::new(9, 5.0, 64.0, 4);
        let y = src.height_at(0.0, 0.0);
        let f = CarveField::new(vec![sphere([0.0, y, 0.0], 8.0)], 0.0);
        assert!(f.is_open_on(&src, 0.0, 0.0), "void on the surface should breach it");
        assert!(!f.is_open_on(&src, 200.0, 200.0), "far column is untouched");
    }
}
