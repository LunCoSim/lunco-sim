//! Quadtree + CDLOD node selection — the pure, render-free spine of the streamed
//! terrain (milestone S1).
//!
//! A square root region (the DEM extent, origin-centred) is recursively quartered.
//! Each node has a stable [`QuadCoord`] `{ depth, x, z }`, so a node addresses both
//! a visual draw instance AND a cache entry AND a physics tile. [`Quadtree::select`]
//! walks the tree from the root and emits the set of nodes to draw at the current
//! focus point, with CDLOD morph bands so neighbouring LODs blend without a pop.
//!
//! **Selection metric — distance-range, not screen-space-error.** A node at `depth`
//! is refined into its four children when the focus is within `refine_range(depth)`,
//! where `refine_range = range_factor · geometric_error(depth)`. The `range_factor`
//! is computed once from a *canonical* screen metric ([`Quadtree::from_screen_metric`])
//! — fixed pixel-error + FOV — so the selected set depends only on world geometry,
//! never on a client's resolution or camera FOV. That is what lets the physics tile
//! ring (driven by a rover's world position at a fixed depth) be **identical across
//! peers and the headless server** (networked determinism). Screen-space-error, by
//! contrast, is view-dependent and would diverge the tile set per client.
//!
//! `geometric_error(depth)` is 3D-Tiles-compatible (`rootError / 2^depth`), so the
//! same numbers can later be authored into a 3D-Tiles-style implicit-quadtree
//! descriptor (see `docs/terrain-streaming-IMPL.md`).
//!
//! Pure + `no-bevy` → unit-tested and wasm-safe. The Bevy streaming manager (S3) and
//! the collider ring (S4) both consume this.

/// An axis-aligned **square** region in the terrain XZ plane (metres).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Square {
    /// Centre `[x, z]`.
    pub center: [f64; 2],
    /// Half side length.
    pub half: f64,
}

impl Square {
    /// Nearest distance from `p` to this square (0 if `p` is inside).
    pub fn distance_to(&self, p: [f64; 2]) -> f64 {
        let dx = (p[0] - self.center[0]).abs() - self.half;
        let dz = (p[1] - self.center[1]).abs() - self.half;
        let dx = dx.max(0.0);
        let dz = dz.max(0.0);
        (dx * dx + dz * dz).sqrt()
    }

    /// Side length.
    pub fn side(&self) -> f64 {
        2.0 * self.half
    }
}

/// Stable address of a quadtree node. `depth` 0 is the root (one node covering the
/// whole region); depth `d` has a `2^d × 2^d` grid of nodes, `x`/`z` in `0..2^d`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QuadCoord {
    pub depth: u8,
    pub x: u32,
    pub z: u32,
}

impl QuadCoord {
    pub const ROOT: QuadCoord = QuadCoord { depth: 0, x: 0, z: 0 };

    /// The four children (depth + 1).
    pub fn children(self) -> [QuadCoord; 4] {
        let d = self.depth + 1;
        let (x0, z0) = (self.x * 2, self.z * 2);
        [
            QuadCoord { depth: d, x: x0, z: z0 },
            QuadCoord { depth: d, x: x0 + 1, z: z0 },
            QuadCoord { depth: d, x: x0, z: z0 + 1 },
            QuadCoord { depth: d, x: x0 + 1, z: z0 + 1 },
        ]
    }
}

/// A node chosen by [`Quadtree::select`], ready to draw / collide / cache.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Selected {
    pub coord: QuadCoord,
    pub region: Square,
    /// Distance band `[start, end]` over which this node's geometry morphs toward
    /// its parent (coarser) node — the CDLOD geomorph window for the vertex shader.
    /// `end` is the distance at which the parent takes over; `start = morph_ratio·end`.
    pub morph_start: f64,
    pub morph_end: f64,
}

/// A CDLOD quadtree over a square, origin-centred terrain region.
#[derive(Debug, Clone, Copy)]
pub struct Quadtree {
    /// Half side length of the root region (e.g. DEM `half_extent`).
    pub root_half_extent: f64,
    /// Deepest subdivision level (finest detail). `0` = root only.
    pub max_depth: u8,
    /// `refine_range(depth) = range_factor · geometric_error(depth)`.
    pub range_factor: f64,
    /// Geometric error (m) of the root rendered without children. Halves per depth.
    pub root_geometric_error: f64,
    /// Fraction of the morph band over which geomorph runs (`start = ratio·end`).
    pub morph_ratio: f64,
}

impl Quadtree {
    /// Construct with an explicit `range_factor`.
    pub fn new(root_half_extent: f64, max_depth: u8, range_factor: f64, root_geometric_error: f64) -> Self {
        Quadtree { root_half_extent, max_depth, range_factor, root_geometric_error, morph_ratio: 0.7 }
    }

    /// Construct deriving `range_factor` from a **canonical** screen metric, so the
    /// selected set is independent of any actual client viewport. `target_pixel_error`
    /// is the on-screen error (px) at which a node refines; `screen_height_px` /
    /// `fov_y_rad` are the fixed canonical viewport. Matches the 3D-Tiles SSE formula
    /// `sse = error · screenHeight / (distance · 2·tan(fov/2))` solved for the
    /// distance where `sse = target_pixel_error`.
    pub fn from_screen_metric(
        root_half_extent: f64,
        max_depth: u8,
        root_geometric_error: f64,
        screen_height_px: f64,
        fov_y_rad: f64,
        target_pixel_error: f64,
    ) -> Self {
        let sse_denominator = 2.0 * (0.5 * fov_y_rad).tan();
        // Guard the divisor: a `target_pixel_error` of 0 (an Inspector knob set to
        // zero) would make `range_factor` infinite → every node refines to
        // `max_depth` → a triangle/tile blow-up. Floor it at a sub-pixel epsilon.
        let target_pixel_error = target_pixel_error.max(1e-3);
        let range_factor = screen_height_px / (sse_denominator * target_pixel_error);
        Quadtree::new(root_half_extent, max_depth, range_factor, root_geometric_error)
    }

    /// Geometric error (m) of a node at `depth` (3D-Tiles-compatible: halves per level).
    pub fn geometric_error(&self, depth: u8) -> f64 {
        self.root_geometric_error / (1u64 << depth) as f64
    }

    /// Refine into children when the focus is within this distance of a `depth` node.
    pub fn refine_range(&self, depth: u8) -> f64 {
        self.range_factor * self.geometric_error(depth)
    }

    /// World-space square covered by `coord`.
    pub fn region(&self, coord: QuadCoord) -> Square {
        let nodes_per_side = (1u64 << coord.depth) as f64;
        let side = (2.0 * self.root_half_extent) / nodes_per_side;
        let half = 0.5 * side;
        let x = -self.root_half_extent + (coord.x as f64 + 0.5) * side;
        let z = -self.root_half_extent + (coord.z as f64 + 0.5) * side;
        Square { center: [x, z], half }
    }

    /// Select the set of nodes to realize for a focus point `focus_xz` (camera for
    /// visuals; a rover position for the deterministic physics ring). Coverage of the
    /// root region is exact — every emitted region is disjoint and their union is the
    /// root (REPLACE refinement).
    pub fn select(&self, focus_xz: [f64; 2]) -> Vec<Selected> {
        self.select_3d(focus_xz, 0.0)
    }

    /// Select using the **full 3D distance**: each node's horizontal distance is
    /// combined with `eye_height` (the camera's height above the terrain surface)
    /// as `sqrt(horizontal² + eye_height²)`. A camera high above the ground then
    /// coarsens the tiles directly below it instead of refining them to max depth
    /// as a purely-XZ metric would. `select(focus)` is the `eye_height = 0` case.
    pub fn select_3d(&self, focus_xz: [f64; 2], eye_height: f64) -> Vec<Selected> {
        let mut out = Vec::new();
        self.select_node(QuadCoord::ROOT, focus_xz, eye_height, &mut out);
        out
    }

    /// Select using a **measured per-node geometric error** instead of the uniform
    /// `root / 2^depth` schedule. `node_error(coord, region)` returns the vertical
    /// error (metres) of drawing that node at its own mesh resolution — see
    /// [`crate::error::measure_node_error`]. A node refines when the focus is within
    /// `range_factor · node_error(coord)`, so flat ground stays coarse and rims /
    /// peaks earn deeper subdivision automatically.
    ///
    /// `node_error` must be a **pure function of the surface** (same inputs → same
    /// output on every platform) to keep selection peer-deterministic like
    /// [`select`](Self::select). It is called only for nodes the walk visits (lazy,
    /// O(visited) — no eager error map). To bound the coarsest tile size over a truly
    /// flat region, have the caller clamp `node_error` to a floor.
    pub fn select_with_error(
        &self,
        focus_xz: [f64; 2],
        eye_height: f64,
        node_error: impl Fn(QuadCoord, Square) -> f64,
    ) -> Vec<Selected> {
        let mut out = Vec::new();
        self.select_node_with_error(QuadCoord::ROOT, f64::INFINITY, focus_xz, eye_height, &node_error, &mut out);
        out
    }

    /// Force `sel` (a valid REPLACE-refinement cover, as produced by the
    /// `select*` walks) to include the **max-depth** node containing
    /// `focus_xz` plus its 8 neighbours, splitting whatever coarser ancestors
    /// currently cover them. The cover stays exact and disjoint.
    ///
    /// Why: the visual selection is CAMERA-driven while the physics collider
    /// ring is fixed-resolution — a rover far from the camera (or under a
    /// budget-coarsened selection) stands on collider features its coarse
    /// visual tile doesn't draw, visibly hovering above the rendered ground.
    /// Feeding each dynamic body through this after selection guarantees the
    /// ground UNDER bodies is drawn at the same finest detail the collider
    /// samples. Split-off siblings inherit geomorph windows from the same
    /// `node_error` metric the walk used, so bands stay consistent.
    pub fn refine_selection_at(
        &self,
        sel: &mut Vec<Selected>,
        focus_xz: [f64; 2],
        node_error: impl Fn(QuadCoord, Square) -> f64,
    ) {
        let nodes = 1i64 << self.max_depth;
        let side = (2.0 * self.root_half_extent) / nodes as f64;
        let cx = (((focus_xz[0] + self.root_half_extent) / side).floor() as i64).clamp(0, nodes - 1);
        let cz = (((focus_xz[1] + self.root_half_extent) / side).floor() as i64).clamp(0, nodes - 1);
        for dz in -1..=1i64 {
            for dx in -1..=1i64 {
                let (nx, nz) = (cx + dx, cz + dz);
                if nx < 0 || nz < 0 || nx >= nodes || nz >= nodes {
                    continue;
                }
                let target =
                    QuadCoord { depth: self.max_depth, x: nx as u32, z: nz as u32 };
                self.force_refine(sel, target, &node_error);
            }
        }
    }

    /// Split the selected ancestor of `target` (if coarser) down to
    /// `target.depth`, pushing the split-off siblings at each level.
    fn force_refine(
        &self,
        sel: &mut Vec<Selected>,
        target: QuadCoord,
        node_error: &impl Fn(QuadCoord, Square) -> f64,
    ) {
        fn covers(anc: QuadCoord, target: QuadCoord) -> bool {
            anc.depth <= target.depth
                && (target.x >> (target.depth - anc.depth)) == anc.x
                && (target.z >> (target.depth - anc.depth)) == anc.z
        }
        let Some(i) = sel.iter().position(|s| covers(s.coord, target)) else {
            return; // not covered (shouldn't happen for an exact cover)
        };
        if sel[i].coord.depth >= target.depth {
            return; // already fine enough
        }
        let mut cur = sel.swap_remove(i);
        while cur.coord.depth < target.depth {
            // Children's geomorph window ends where THIS node would refine —
            // the same band the error-driven walk would have assigned them.
            let refine_range = self.range_factor * node_error(cur.coord, cur.region).max(0.0);
            let morph_start = if refine_range.is_finite() {
                self.morph_ratio * refine_range
            } else {
                f64::INFINITY
            };
            let d = cur.coord.depth + 1;
            let mut next: Option<Selected> = None;
            for (ox, oz) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
                let cc = QuadCoord { depth: d, x: cur.coord.x * 2 + ox, z: cur.coord.z * 2 + oz };
                let s = Selected {
                    coord: cc,
                    region: self.region(cc),
                    morph_start,
                    morph_end: refine_range,
                };
                if covers(cc, target) {
                    next = Some(s);
                } else {
                    sel.push(s);
                }
            }
            cur = next.expect("exactly one child covers the target");
        }
        sel.push(cur);
    }

    /// [`select_with_error`](Self::select_with_error) under a **hard tile budget**:
    /// nodes are refined in *priority* order (highest `refine_range / distance`,
    /// i.e. worst on-screen error, first) until either nothing wants refinement or
    /// splitting would exceed `max_tiles`. With a non-binding budget the result is
    /// identical to the unbudgeted walk; when the budget binds, near/feature nodes
    /// keep their detail and far ground coarsens under its geomorph band.
    ///
    /// Why: the recursive walk's cost is unbounded in the terrain, not the budget —
    /// at realistic crater densities EVERY mid-distance node carries metres of
    /// measured error, so a 3 px target refined a ~1 km disc to max depth
    /// (thousands of tiles, tens of millions of triangles). A budget makes the
    /// cost knob explicit while keeping the same error-driven priorities.
    /// Deterministic: the heap orders by `(priority, coord)` with total float
    /// ordering, and `node_error` is a pure function of the surface.
    pub fn select_with_error_budgeted(
        &self,
        focus_xz: [f64; 2],
        eye_height: f64,
        node_error: impl Fn(QuadCoord, Square) -> f64,
        max_tiles: usize,
    ) -> Vec<Selected> {
        use std::collections::BinaryHeap;

        /// A leaf of the in-progress selection that still WANTS refinement.
        struct Refinable {
            coord: QuadCoord,
            region: Square,
            /// Distance at which this node's PARENT refined (∞ for the root) —
            /// this node's geomorph window end if it stays a leaf.
            parent_refine_range: f64,
            /// `range_factor · node_error(self)` — the distance under which this
            /// node refines, and the morph window end of its children.
            refine_range: f64,
            /// `refine_range / distance` (> 1 ⇔ wants refinement). Max-heap key.
            priority: f64,
        }
        impl PartialEq for Refinable {
            fn eq(&self, other: &Self) -> bool {
                self.cmp(other) == std::cmp::Ordering::Equal
            }
        }
        impl Eq for Refinable {}
        impl PartialOrd for Refinable {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }
        impl Ord for Refinable {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                // Total order for peer determinism: priority, then coord.
                self.priority
                    .total_cmp(&other.priority)
                    .then_with(|| self.coord.depth.cmp(&other.coord.depth))
                    .then_with(|| self.coord.x.cmp(&other.coord.x))
                    .then_with(|| self.coord.z.cmp(&other.coord.z))
            }
        }

        let mut out: Vec<Selected> = Vec::new();
        let mut heap: BinaryHeap<Refinable> = BinaryHeap::new();

        let finalize = |n: &Refinable, out: &mut Vec<Selected>| {
            let morph_end = n.parent_refine_range;
            let morph_start =
                if morph_end.is_finite() { self.morph_ratio * morph_end } else { f64::INFINITY };
            out.push(Selected { coord: n.coord, region: n.region, morph_start, morph_end });
        };
        // Classify a node: heap if it wants refinement, else final.
        let classify = |coord: QuadCoord,
                        parent_refine_range: f64,
                        heap: &mut BinaryHeap<Refinable>,
                        out: &mut Vec<Selected>| {
            let region = self.region(coord);
            let horizontal = region.distance_to(focus_xz);
            let dist = (horizontal * horizontal + eye_height * eye_height).sqrt();
            let refine_range = self.range_factor * node_error(coord, region).max(0.0);
            let n = Refinable {
                coord,
                region,
                parent_refine_range,
                refine_range,
                priority: refine_range / dist.max(1e-9),
            };
            if coord.depth < self.max_depth && dist < refine_range {
                heap.push(n);
            } else {
                finalize(&n, out);
            }
        };

        classify(QuadCoord::ROOT, f64::INFINITY, &mut heap, &mut out);
        // Each split replaces one leaf with four (net +3 leaves).
        while let Some(top) = heap.pop() {
            if out.len() + heap.len() + 1 + 3 > max_tiles.max(1) {
                // Budget bound: the popped node (and everything below it in the
                // heap) stays a leaf.
                finalize(&top, &mut out);
                break;
            }
            for child in top.coord.children() {
                classify(child, top.refine_range, &mut heap, &mut out);
            }
        }
        for n in heap {
            finalize(&n, &mut out);
        }
        out
    }

    fn select_node_with_error(
        &self,
        coord: QuadCoord,
        parent_refine_range: f64,
        focus: [f64; 2],
        eye_height: f64,
        node_error: &impl Fn(QuadCoord, Square) -> f64,
        out: &mut Vec<Selected>,
    ) {
        let region = self.region(coord);
        let horizontal = region.distance_to(focus);
        let dist = (horizontal * horizontal + eye_height * eye_height).sqrt();
        let refine_range = self.range_factor * node_error(coord, region).max(0.0);
        let refine = coord.depth < self.max_depth && dist < refine_range;
        if refine {
            for child in coord.children() {
                self.select_node_with_error(child, refine_range, focus, eye_height, node_error, out);
            }
            return;
        }
        // Morph toward the parent: `parent_refine_range` is the distance at which the
        // parent stopped refining (∞ for the root), i.e. where this node's geometry
        // would instead be the coarser parent — the CDLOD geomorph window end.
        let morph_end = parent_refine_range;
        let morph_start = if morph_end.is_finite() { self.morph_ratio * morph_end } else { f64::INFINITY };
        out.push(Selected { coord, region, morph_start, morph_end });
    }

    fn select_node(&self, coord: QuadCoord, focus: [f64; 2], eye_height: f64, out: &mut Vec<Selected>) {
        let region = self.region(coord);
        let horizontal = region.distance_to(focus);
        let dist = (horizontal * horizontal + eye_height * eye_height).sqrt();
        let refine = coord.depth < self.max_depth && dist < self.refine_range(coord.depth);
        if refine {
            for child in coord.children() {
                self.select_node(child, focus, eye_height, out);
            }
            return;
        }
        // Drawn at this depth. Morph band runs up to where the *parent* refines (the
        // distance at which this node would instead be the coarser parent geometry).
        let morph_end = if coord.depth == 0 {
            f64::INFINITY // root has no coarser parent to morph toward
        } else {
            self.refine_range(coord.depth - 1)
        };
        let morph_start = if morph_end.is_finite() { self.morph_ratio * morph_end } else { f64::INFINITY };
        out.push(Selected { coord, region, morph_start, morph_end });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qt() -> Quadtree {
        // 16 km root region, up to 6 levels deep, error-derived ranges.
        Quadtree::new(8000.0, 6, 4.0, 8000.0)
    }

    #[test]
    fn geometric_error_halves_and_range_monotonic() {
        let q = qt();
        for d in 0..6 {
            assert!((q.geometric_error(d + 1) - q.geometric_error(d) / 2.0).abs() < 1e-9);
            assert!(q.refine_range(d + 1) < q.refine_range(d), "refine_range must shrink with depth");
        }
    }

    #[test]
    fn region_root_and_children_tile_exactly() {
        let q = qt();
        let root = q.region(QuadCoord::ROOT);
        assert_eq!(root.center, [0.0, 0.0]);
        assert_eq!(root.half, 8000.0);
        // Four children exactly cover the root, disjoint, each quarter side.
        let kids = QuadCoord::ROOT.children().map(|c| q.region(c));
        let area: f64 = kids.iter().map(|s| s.side() * s.side()).sum();
        assert!((area - root.side() * root.side()).abs() < 1e-6);
        for s in &kids {
            assert!((s.half - 4000.0).abs() < 1e-9);
        }
    }

    #[test]
    fn select_is_deterministic() {
        let q = qt();
        let a = q.select([1234.0, -567.0]);
        let b = q.select([1234.0, -567.0]);
        assert_eq!(a, b);
    }

    #[test]
    fn select_covers_root_exactly_and_disjoint() {
        let q = qt();
        let sel = q.select([100.0, 200.0]);
        // Areas sum to the root area (exact tiling).
        let area: f64 = sel.iter().map(|s| s.region.side() * s.region.side()).sum();
        let root_area = (2.0 * q.root_half_extent).powi(2);
        assert!((area - root_area).abs() < 1e-3, "area {area} vs {root_area}");
        // Sample interior points (nudged by 0.137 m off the 250 m node grid so none
        // land on a shared edge); each must fall in exactly one selected region.
        for gx in 0..40 {
            for gz in 0..40 {
                let p = [
                    -8000.0 + (gx as f64 + 0.5) * (16000.0 / 40.0) + 0.137,
                    -8000.0 + (gz as f64 + 0.5) * (16000.0 / 40.0) + 0.137,
                ];
                let hits = sel.iter().filter(|s| s.region.distance_to(p) <= 1e-6).count();
                assert_eq!(hits, 1, "point {p:?} covered {hits} times");
            }
        }
    }

    #[test]
    fn budgeted_matches_unbudgeted_when_budget_is_ample() {
        let q = qt();
        // Uniform measured error = the per-depth schedule → same walk as select().
        let err = |c: QuadCoord, _r: Square| q.geometric_error(c.depth);
        let free = q.select_with_error([100.0, 200.0], 0.0, err);
        let bud = q.select_with_error_budgeted([100.0, 200.0], 0.0, err, usize::MAX);
        let key = |s: &Selected| (s.coord.depth, s.coord.x, s.coord.z);
        let mut a: Vec<_> = free.iter().map(key).collect();
        let mut b: Vec<_> = bud.iter().map(key).collect();
        a.sort();
        b.sort();
        assert_eq!(a, b);
        // Morph windows survive the reordering too.
        for s in &bud {
            let twin = free.iter().find(|f| f.coord == s.coord).unwrap();
            assert_eq!((s.morph_start, s.morph_end), (twin.morph_start, twin.morph_end));
        }
    }

    #[test]
    fn budgeted_respects_cap_covers_root_and_keeps_near_detail() {
        let q = qt();
        let focus = [100.0, 200.0];
        let err = |c: QuadCoord, _r: Square| q.geometric_error(c.depth);
        let unb = q.select_with_error(focus, 0.0, err);
        let cap = unb.len() / 2; // force the budget to bind
        let sel = q.select_with_error_budgeted(focus, 0.0, err, cap);
        assert!(sel.len() <= cap, "{} tiles > cap {cap}", sel.len());
        // Coverage stays exact under the cap.
        let area: f64 = sel.iter().map(|s| s.region.side() * s.region.side()).sum();
        let root_area = (2.0 * q.root_half_extent).powi(2);
        assert!((area - root_area).abs() < 1e-3, "area {area} vs {root_area}");
        // Priority order spends the budget near the focus: the focus leaf must be
        // at least as deep as every far-corner leaf.
        let depth_at = |p: [f64; 2]| {
            sel.iter().find(|s| s.region.distance_to(p) <= 1e-6).map(|s| s.coord.depth).unwrap()
        };
        assert!(depth_at(focus) >= depth_at([7900.0, 7900.0]));
        // Deterministic under identical inputs.
        let again = q.select_with_error_budgeted(focus, 0.0, err, cap);
        assert_eq!(sel.len(), again.len());
        assert!(sel.iter().zip(&again).all(|(a, b)| a.coord == b.coord));
    }

    #[test]
    fn finest_under_focus_coarsest_far() {
        let q = qt();
        let focus = [0.0, 0.0];
        let sel = q.select(focus);
        // The node containing the focus is at max depth (closest → finest).
        let under = sel.iter().filter(|s| s.region.distance_to(focus) <= 1e-6).min_by_key(|s| s.region.half as u64).unwrap();
        assert_eq!(under.coord.depth, q.max_depth, "focus should sit on a max-depth leaf");
        // A far corner is coarser than the focus leaf.
        let corner = [7999.0, 7999.0];
        let far = sel.iter().filter(|s| s.region.distance_to(corner) <= 1e-6).next().unwrap();
        assert!(far.coord.depth < q.max_depth, "far corner should be coarse");
    }

    #[test]
    fn neighbour_depth_differs_by_at_most_one() {
        // CDLOD invariant: adjacent selected nodes differ by ≤1 level (so a single
        // morph band / skirt closes the seam). Check via sampled point pairs.
        let q = qt();
        let sel = q.select([0.0, 0.0]);
        let depth_at = |p: [f64; 2]| -> Option<u8> {
            sel.iter().find(|s| s.region.distance_to(p) <= 1e-6).map(|s| s.coord.depth)
        };
        let step = 16000.0 / 64.0;
        for gx in 0..64 {
            for gz in 0..64 {
                let p = [-8000.0 + (gx as f64 + 0.5) * step, -8000.0 + (gz as f64 + 0.5) * step];
                if let (Some(d), Some(dr)) = (depth_at(p), depth_at([p[0] + step, p[1]])) {
                    assert!(d.abs_diff(dr) <= 1, "depth jump {d}->{dr} at {p:?}");
                }
            }
        }
    }

    #[test]
    fn morph_end_matches_parent_refine_range() {
        let q = qt();
        let sel = q.select([0.0, 0.0]);
        for s in &sel {
            if s.coord.depth == 0 {
                assert!(s.morph_end.is_infinite());
            } else {
                assert!((s.morph_end - q.refine_range(s.coord.depth - 1)).abs() < 1e-9);
                assert!(s.morph_start < s.morph_end);
            }
        }
    }

    #[test]
    fn screen_metric_factor_is_positive_and_scales() {
        let q = Quadtree::from_screen_metric(8000.0, 6, 8000.0, 1080.0, 0.7854, 2.0);
        assert!(q.range_factor > 0.0);
        // Tighter pixel error → larger range_factor (refine sooner / from farther).
        let tight = Quadtree::from_screen_metric(8000.0, 6, 8000.0, 1080.0, 0.7854, 1.0);
        assert!(tight.range_factor > q.range_factor);
    }

    #[test]
    fn error_driven_zero_error_stays_root() {
        // A dead-flat surface (error 0 everywhere) never earns refinement → the whole
        // region is a single root tile, however close the focus.
        let q = qt();
        let sel = q.select_with_error([0.0, 0.0], 0.0, |_, _| 0.0);
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].coord, QuadCoord::ROOT);
    }

    #[test]
    fn error_driven_matches_uniform_when_error_is_uniform() {
        // Feeding the uniform `root/2^depth` error back in must reproduce `select`
        // exactly — error-driven selection is a strict generalisation.
        let q = qt();
        let focus = [1234.0, -567.0];
        let uniform = q.select(focus);
        let via_error = q.select_with_error(focus, 0.0, |c, _| q.geometric_error(c.depth));
        assert_eq!(uniform, via_error);
    }

    #[test]
    fn error_driven_refines_locally_around_a_feature() {
        // A "feature" at a fixed point: only nodes whose region contains it carry big
        // error. Those refine to max depth; nodes far away stay coarse.
        let q = qt();
        let feature = [3000.0, -2000.0];
        let node_error = |_c: QuadCoord, region: Square| -> f64 {
            if region.distance_to(feature) <= 1e-6 {
                q.root_geometric_error // huge → always refine while it contains the feature
            } else {
                0.0 // flat elsewhere → never refine
            }
        };
        // Focus far away so distance never drives refinement — only the feature error does.
        let sel = q.select_with_error([7000.0, 7000.0], 0.0, node_error);
        let leaf = sel.iter().find(|s| s.region.distance_to(feature) <= 1e-6).unwrap();
        assert_eq!(leaf.coord.depth, q.max_depth, "feature cell should reach max depth");
        // A point far from the feature stays coarse: its node stops as soon as its
        // branch no longer contains the feature (here, depth 1 — a root quadrant).
        let corner = [7999.0, 7999.0];
        let far = sel.iter().find(|s| s.region.distance_to(corner) <= 1e-6).unwrap();
        assert!(far.coord.depth <= 1, "flat far field should stay coarse, got depth {}", far.coord.depth);
    }

    #[test]
    fn error_driven_covers_root_exactly_and_disjoint() {
        // Coverage invariant must hold for the error-driven path too (REPLACE refinement).
        let q = qt();
        let feature = [1500.0, 2500.0];
        let sel = q.select_with_error([0.0, 0.0], 0.0, |_c, region| {
            if region.distance_to(feature) <= 1e-6 { q.root_geometric_error } else { 0.0 }
        });
        let area: f64 = sel.iter().map(|s| s.region.side() * s.region.side()).sum();
        let root_area = (2.0 * q.root_half_extent).powi(2);
        assert!((area - root_area).abs() < 1e-3, "area {area} vs {root_area}");
        for gx in 0..40 {
            for gz in 0..40 {
                let p = [
                    -8000.0 + (gx as f64 + 0.5) * (16000.0 / 40.0) + 0.137,
                    -8000.0 + (gz as f64 + 0.5) * (16000.0 / 40.0) + 0.137,
                ];
                let hits = sel.iter().filter(|s| s.region.distance_to(p) <= 1e-6).count();
                assert_eq!(hits, 1, "point {p:?} covered {hits} times");
            }
        }
    }

    #[test]
    fn forced_refinement_reaches_max_depth_and_keeps_cover_exact() {
        // A far/coarse selection forced to refine around a "rover" position
        // must contain max-depth nodes there while staying an exact disjoint
        // cover of the root (REPLACE refinement invariant).
        let q = qt();
        let flat = |_c: QuadCoord, _r: Square| 0.05; // near-flat → coarse everywhere
        let mut sel = q.select_with_error([0.0, 0.0], 4000.0, flat);
        assert!(
            sel.iter().all(|s| s.coord.depth < q.max_depth),
            "precondition: coarse selection"
        );
        let rover = [1234.5, -2345.6];
        q.refine_selection_at(&mut sel, rover, flat);
        // The node under the rover is now max depth.
        let under = sel
            .iter()
            .find(|s| s.region.distance_to(rover) <= 1e-6)
            .expect("cover contains the rover");
        assert_eq!(under.coord.depth, q.max_depth, "ground under the body must be finest");
        // Cover stays exact + disjoint.
        let area: f64 = sel.iter().map(|s| s.region.side() * s.region.side()).sum();
        let root_area = (2.0 * q.root_half_extent).powi(2);
        assert!((area - root_area).abs() < 1e-3, "area {area} vs {root_area}");
        for gx in 0..40 {
            for gz in 0..40 {
                let p = [
                    -8000.0 + (gx as f64 + 0.5) * (16000.0 / 40.0) + 0.137,
                    -8000.0 + (gz as f64 + 0.5) * (16000.0 / 40.0) + 0.137,
                ];
                let hits = sel.iter().filter(|s| s.region.distance_to(p) <= 1e-6).count();
                assert_eq!(hits, 1, "point {p:?} covered {hits} times");
            }
        }
        // Idempotent: forcing again changes nothing.
        let n = sel.len();
        q.refine_selection_at(&mut sel, rover, flat);
        assert_eq!(sel.len(), n);
    }

    #[test]
    fn error_driven_deterministic() {
        let q = qt();
        let f = |_c: QuadCoord, r: Square| r.distance_to([100.0, 100.0]);
        let a = q.select_with_error([50.0, 50.0], 3.0, f);
        let b = q.select_with_error([50.0, 50.0], 3.0, f);
        assert_eq!(a, b);
    }
}
