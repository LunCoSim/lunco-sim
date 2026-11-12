//! Quadtree + CDLOD node selection — the pure, render-free spine of the streamed
//! terrain (milestone S1).
//!
//! A square root region (the DEM extent, origin-centred) is recursively quartered.
//! Each node has a stable [`QuadCoord`] `{ depth, x, z }`, so a node addresses both
//! a visual draw instance AND a cache entry AND a physics tile.
//!
//! **This module owns the metric and the node algebra, not the cover.** The set of
//! nodes to draw is evolved INCREMENTALLY by `lunco-terrain-surface`'s `evolve_cover`,
//! which holds a persistent cover across frames and moves it a bounded step — that is
//! what bounds the per-frame tile budget and what keeps the cover *restricted*
//! (edge-adjacent nodes within one depth of each other, the CDLOD morph contract).
//! What lives here is what that walk is written against: [`Quadtree::error_refine_range`]
//! (the refine distance for a measured error), [`Quadtree::focus_distance`], and
//! [`Quadtree::selected`] (the geomorph window for one leaf). Sharing these means the
//! incremental cover refines under the SAME metric rather than a copy that can drift.
//!
//! Perspective selection compares nearest node distance to the error range from
//! the consumed output's physical pixels and field of view. Orthographic
//! selection compares projected error directly to its pixel threshold, independent
//! of camera distance. The caller clips orthographic nodes against the admitted
//! camera footprint. Physics uses its own fixed-depth node selection.
//!
//! `geometric_error(depth)` is 3D-Tiles-compatible (`rootError / 2^depth`), so the
//! same numbers can later be authored into a 3D-Tiles-style implicit-quadtree
//! descriptor (see `docs/architecture/terrain-substrate.md`).
//!
//! Pure + `no-bevy` → unit-tested and wasm-safe. The Bevy streaming manager (S3) and
//! the collider ring (S4) both consume this.

/// Fraction of a child node's refine range used for the start of its CDLOD morph.
/// A child finishes morphing to its parent at `r` and starts at `0.55 · r`, leaving
/// a broad distance-driven transition rather than a narrow visual snap.
pub const CDLOD_MORPH_START_RATIO: f64 = 0.55;

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

    /// Nearest distance to the box formed by this square and a height interval.
    pub fn distance_to_height_range(&self, p: [f64; 3], heights: [f64; 2]) -> f64 {
        let horizontal = self.distance_to([p[0], p[2]]);
        let vertical = (heights[0] - p[1]).max(p[1] - heights[1]).max(0.0);
        horizontal.hypot(vertical)
    }

    /// Side length.
    pub fn side(&self) -> f64 {
        2.0 * self.half
    }

    /// Whether this square in the XZ plane intersects
    /// `[min_x, min_z, max_x, max_z]`. Touching edges count as overlap so
    /// adjacent sampled products do not leave a seam along a dirty boundary.
    pub fn overlaps_aabb(&self, bounds: [f64; 4]) -> bool {
        let [min_x, min_z, max_x, max_z] = bounds;
        self.center[0] - self.half <= max_x
            && self.center[0] + self.half >= min_x
            && self.center[1] - self.half <= max_z
            && self.center[1] + self.half >= min_z
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
    pub const ROOT: QuadCoord = QuadCoord {
        depth: 0,
        x: 0,
        z: 0,
    };

    /// The coarser node whose region contains this one (depth − 1). `None` at the root.
    ///
    /// Region containment in a quadtree IS ancestry, so walking this chain is how a
    /// consumer asks "what coarser node covers my area" — the basis of LOD fallback.
    pub fn parent(self) -> Option<QuadCoord> {
        (self.depth > 0).then(|| QuadCoord {
            depth: self.depth - 1,
            x: self.x / 2,
            z: self.z / 2,
        })
    }

    /// Same-depth edge neighbours, clipped to the root square.
    pub fn edge_neighbours(self) -> impl Iterator<Item = QuadCoord> {
        let side = 1i64 << self.depth;
        [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)]
            .into_iter()
            .filter_map(move |(dx, dz)| {
                let x = self.x as i64 + dx;
                let z = self.z as i64 + dz;
                (x >= 0 && z >= 0 && x < side && z < side).then_some(QuadCoord {
                    depth: self.depth,
                    x: x as u32,
                    z: z as u32,
                })
            })
    }

    /// The four children (depth + 1).
    pub fn children(self) -> [QuadCoord; 4] {
        let d = self.depth + 1;
        let (x0, z0) = (self.x * 2, self.z * 2);
        [
            QuadCoord {
                depth: d,
                x: x0,
                z: z0,
            },
            QuadCoord {
                depth: d,
                x: x0 + 1,
                z: z0,
            },
            QuadCoord {
                depth: d,
                x: x0,
                z: z0 + 1,
            },
            QuadCoord {
                depth: d,
                x: x0 + 1,
                z: z0 + 1,
            },
        ]
    }
}

/// A node in the selected cover, ready to draw / collide / cache.
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

/// Projection law used by visual error selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionMetric {
    Perspective,
    Orthographic,
}

/// A CDLOD quadtree over a square, origin-centred terrain region.
#[derive(Debug, Clone, Copy)]
pub struct Quadtree {
    pub projection_metric: ProjectionMetric,
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
    pub fn new(
        root_half_extent: f64,
        max_depth: u8,
        range_factor: f64,
        root_geometric_error: f64,
    ) -> Self {
        Quadtree {
            projection_metric: ProjectionMetric::Perspective,
            root_half_extent,
            max_depth,
            range_factor,
            root_geometric_error,
            morph_ratio: CDLOD_MORPH_START_RATIO,
        }
    }

    /// Construct deriving `range_factor` from the rendered camera's screen metric.
    /// `target_pixel_error` is the on-screen error (px) at which a node refines;
    /// `screen_height_px` and `fov_y_rad` are supplied by that camera. Matches the
    /// 3D-Tiles SSE formula
    /// `sse = error · screenHeight / (distance · 2·tan(fov/2))` solved for the
    /// distance where `sse = target_pixel_error`. `morph_ratio` is the explicit
    /// fraction of a node's refinement band at which geomorphing begins.
    pub fn from_screen_metric(
        root_half_extent: f64,
        max_depth: u8,
        root_geometric_error: f64,
        screen_height_px: f64,
        fov_y_rad: f64,
        target_pixel_error: f64,
        morph_ratio: f64,
    ) -> Self {
        assert!(
            target_pixel_error.is_finite() && target_pixel_error > 0.0,
            "target pixel error must be finite and greater than zero"
        );
        assert!(
            screen_height_px.is_finite() && screen_height_px > 0.0,
            "screen height must be finite and greater than zero"
        );
        assert!(
            fov_y_rad.is_finite() && fov_y_rad > 0.0 && fov_y_rad < std::f64::consts::PI,
            "vertical field of view must be finite and in (0, pi)"
        );
        let sse_denominator = 2.0 * (0.5 * fov_y_rad).tan();
        assert!(
            sse_denominator.is_finite() && sse_denominator > 0.0,
            "vertical field of view must produce a finite positive screen metric"
        );
        assert!(
            morph_ratio.is_finite() && (0.0..1.0).contains(&morph_ratio),
            "morph ratio must be finite and in [0, 1)"
        );
        let range_factor = screen_height_px / (sse_denominator * target_pixel_error);
        let mut quadtree = Quadtree::new(
            root_half_extent,
            max_depth,
            range_factor,
            root_geometric_error,
        );
        quadtree.morph_ratio = morph_ratio;
        quadtree
    }

    /// Orthographic error is projected in pixels per metre and is independent
    /// of camera distance. Distance morphing is disabled for these covers;
    /// zoom-driven cover transitions retain the normal exact topology stitches.
    pub fn from_orthographic_metric(
        root_half_extent: f64,
        max_depth: u8,
        root_geometric_error: f64,
        pixels_per_metre: f64,
        target_pixel_error: f64,
    ) -> Self {
        assert!(pixels_per_metre.is_finite() && pixels_per_metre > 0.0);
        assert!(target_pixel_error.is_finite() && target_pixel_error > 0.0);
        let mut quadtree = Self::new(
            root_half_extent,
            max_depth,
            pixels_per_metre / target_pixel_error,
            root_geometric_error,
        );
        quadtree.projection_metric = ProjectionMetric::Orthographic;
        quadtree
    }

    /// Geometric error (m) of a node at `depth` (3D-Tiles-compatible: halves per level).
    pub fn geometric_error(&self, depth: u8) -> f64 {
        self.root_geometric_error / (1u64 << depth) as f64
    }

    /// Refine into children when the focus is within this distance of a `depth` node.
    pub fn refine_range(&self, depth: u8) -> f64 {
        self.range_factor * self.geometric_error(depth)
    }

    /// The depth-`depth` node whose region contains local `(x, z)` — the
    /// point→coord inverse of [`region`](Self::region), clamped to the root
    /// footprint. Every point→cell derivation (visual refinement, the collider
    /// ring's wanted set, the physics-readiness probe) goes through this one
    /// mapping so they cannot disagree on which node covers a body.
    pub fn node_containing(&self, depth: u8, xz: [f64; 2]) -> QuadCoord {
        let nodes = 1i64 << depth;
        let side = (2.0 * self.root_half_extent) / nodes as f64;
        let idx = |v: f64| {
            (((v + self.root_half_extent) / side).floor() as i64).clamp(0, nodes - 1) as u32
        };
        QuadCoord {
            depth,
            x: idx(xz[0]),
            z: idx(xz[1]),
        }
    }

    /// World-space square covered by `coord`.
    pub fn region(&self, coord: QuadCoord) -> Square {
        let nodes_per_side = (1u64 << coord.depth) as f64;
        let side = (2.0 * self.root_half_extent) / nodes_per_side;
        let half = 0.5 * side;
        let x = -self.root_half_extent + (coord.x as f64 + 0.5) * side;
        let z = -self.root_half_extent + (coord.z as f64 + 0.5) * side;
        Square {
            center: [x, z],
            half,
        }
    }

    /// Distance at which a node with measured surface error refines.
    /// Shared by incremental cover selection and the CDLOD morph windows.
    pub fn error_refine_range(&self, error: f64) -> f64 {
        self.range_factor * error.max(0.0)
    }

    /// Nearest 3D distance to a node's square lifted by its sampled height range.
    pub fn focus_distance(&self, coord: QuadCoord, focus: [f64; 3], height_range: [f64; 2]) -> f64 {
        if self.projection_metric == ProjectionMetric::Orthographic {
            return 1.0;
        }
        self.region(coord)
            .distance_to_height_range(focus, height_range)
    }

    /// The leaf's CDLOD geomorph window from its parent's measured refine range.
    /// The root has no parent and uses `f64::INFINITY`.
    pub fn selected(&self, coord: QuadCoord, parent_refine_range: f64) -> Selected {
        let morph_end = if self.projection_metric == ProjectionMetric::Orthographic {
            f64::INFINITY
        } else {
            parent_refine_range
        };
        let morph_start = if morph_end.is_finite() {
            self.morph_ratio * morph_end
        } else {
            f64::INFINITY
        };
        Selected {
            coord,
            region: self.region(coord),
            morph_start,
            morph_end,
        }
    }

    /// Constrain an exact visual cover to explicitly required view nodes.
    /// Required nodes refine coarser ancestors and preserve finer camera leaves, keeping
    /// full geometry at every camera distance. Other leaves retain their normal
    /// morph windows. This operation never changes physics selection.
    pub fn constrain_selection(
        &self,
        sel: &mut Vec<Selected>,
        required: &[QuadCoord],
        node_error: &impl Fn(QuadCoord, Square) -> f64,
    ) {
        if required.is_empty() {
            return;
        }
        let mut cover: std::collections::HashMap<QuadCoord, Selected> =
            sel.drain(..).map(|s| (s.coord, s)).collect();
        for &target in required {
            let finer = cover.keys().any(|coord| {
                coord.depth > target.depth
                    && (coord.x >> (coord.depth - target.depth)) == target.x
                    && (coord.z >> (coord.depth - target.depth)) == target.z
            });
            if !finer {
                self.force_refine(&mut cover, target, node_error);
            }
        }
        // Apply pins after all splits so every descendant of a required node
        // retains full geometry under the same view contract.
        for leaf in cover.values_mut() {
            if required.iter().any(|target| {
                leaf.coord.depth >= target.depth
                    && (leaf.coord.x >> (leaf.coord.depth - target.depth)) == target.x
                    && (leaf.coord.z >> (leaf.coord.depth - target.depth)) == target.z
            }) {
                leaf.morph_start = f64::INFINITY;
                leaf.morph_end = f64::INFINITY;
            }
        }
        sel.extend(cover.into_values());
        sel.sort_unstable_by_key(|s| (s.coord.depth, s.coord.x, s.coord.z));
    }

    /// Split the selected ancestor of `target` (if coarser) down to
    /// `target.depth`, splitting coarser neighbours first to preserve 2:1 edges.
    /// New leaves carry the normal error-derived morph window. The completed
    /// required cover installs pins after every split has landed.
    fn force_refine(
        &self,
        cover: &mut std::collections::HashMap<QuadCoord, Selected>,
        target: QuadCoord,
        node_error: &impl Fn(QuadCoord, Square) -> f64,
    ) {
        fn covers(anc: QuadCoord, target: QuadCoord) -> bool {
            anc.depth <= target.depth
                && (target.x >> (target.depth - anc.depth)) == anc.x
                && (target.z >> (target.depth - anc.depth)) == anc.z
        }
        // The covering node of `target` in an exact cover is an ancestor (or
        // `target` itself): walk the parent chain instead of scanning.
        let mut anc = target;
        let covering = loop {
            if cover.contains_key(&anc) {
                break anc;
            }
            match anc.parent() {
                Some(p) => anc = p,
                None => return, // not covered (shouldn't happen for an exact cover)
            }
        };
        if covering.depth >= target.depth {
            return;
        }
        let mut cur = cover
            .remove(&covering)
            .expect("covering node was just found in the map");
        while cur.coord.depth < target.depth {
            // Preserve the same 2:1 seam contract as discretionary selection.
            // Any coarser edge neighbour must reach this node's depth before
            // its children can become part of the cover. Recursion moves toward
            // coarser ancestors, so it is bounded by the tree depth.
            for neighbour in cur.coord.edge_neighbours() {
                let mut ancestor = neighbour.parent();
                while let Some(coord) = ancestor {
                    if cover.contains_key(&coord) {
                        self.force_refine(cover, neighbour, node_error);
                        break;
                    }
                    ancestor = coord.parent();
                }
            }
            // Children's geomorph window ends where THIS node would refine —
            // the same band the error-driven walk would have assigned them.
            let refine_range = self.range_factor * node_error(cur.coord, cur.region).max(0.0);
            let d = cur.coord.depth + 1;
            let mut next: Option<Selected> = None;
            for (ox, oz) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
                let cc = QuadCoord {
                    depth: d,
                    x: cur.coord.x * 2 + ox,
                    z: cur.coord.z * 2 + oz,
                };
                let is_target_path = covers(cc, target);
                let s = self.selected(cc, refine_range);
                if is_target_path {
                    next = Some(s);
                } else {
                    cover.insert(cc, s);
                }
            }
            cur = next.expect("exactly one child covers the target");
        }
        cover.insert(cur.coord, cur);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orthographic_error_tracks_zoom_and_never_distance_morphs() {
        let wide = Quadtree::from_orthographic_metric(100.0, 8, 10.0, 2.0, 2.0);
        let zoomed = Quadtree::from_orthographic_metric(100.0, 8, 10.0, 20.0, 2.0);
        for eye in [[0.0, 1.0, 0.0], [0.0, 1.0e12, 0.0], [1.0e12, -4.0, 0.0]] {
            assert_eq!(wide.focus_distance(QuadCoord::ROOT, eye, [-1.0, 1.0]), 1.0);
        }
        assert!(wide.error_refine_range(0.2) < 1.0);
        assert!(zoomed.error_refine_range(0.2) > 1.0);
        let leaf = zoomed.selected(QuadCoord::ROOT.children()[0], 12.0);
        assert!(leaf.morph_start.is_infinite() && leaf.morph_end.is_infinite());
        let mut cover = vec![zoomed.selected(QuadCoord::ROOT, f64::INFINITY)];
        let required = QuadCoord {
            depth: 3,
            x: 2,
            z: 2,
        };
        zoomed.constrain_selection(&mut cover, &[required], &|_, _| 1.0);
        assert!(
            cover
                .iter()
                .all(|leaf| leaf.morph_start.is_infinite() && leaf.morph_end.is_infinite()),
            "mandatory splits and their neighbours share the projection's morph contract"
        );
        let perspective = Quadtree::new(100.0, 8, 2.0, 10.0);
        assert_eq!(
            perspective
                .selected(QuadCoord::ROOT.children()[0], 12.0)
                .morph_end,
            12.0
        );
    }

    #[test]
    fn node_distance_uses_its_height_range_and_signed_camera_altitude() {
        let q = Quadtree::new(10.0, 1, 1.0, 1.0);
        let c = QuadCoord::ROOT;
        assert_eq!(q.focus_distance(c, [0.0, 100.0, 0.0], [90.0, 95.0]), 5.0);
        assert_eq!(
            q.focus_distance(c, [0.0, 100.0, 0.0], [-50.0, -40.0]),
            140.0
        );
        assert_eq!(
            q.focus_distance(c, [0.0, -100.0, 0.0], [-50.0, -40.0]),
            50.0
        );
        assert_eq!(q.focus_distance(c, [13.0, 104.0, 10.0], [90.0, 100.0]), 5.0);
        assert_eq!(q.focus_distance(c, [0.0, 95.0, 0.0], [90.0, 100.0]), 0.0);
    }

    fn qt() -> Quadtree {
        // 16 km root region, up to 6 levels deep, error-derived ranges.
        Quadtree::new(8000.0, 6, 4.0, 8000.0)
    }

    #[test]
    fn geometric_error_halves_and_range_monotonic() {
        let q = qt();
        for d in 0..6 {
            assert!((q.geometric_error(d + 1) - q.geometric_error(d) / 2.0).abs() < 1e-9);
            // The refine distance is monotone in the error, so a halving schedule
            // gives strictly shrinking ranges with depth.
            assert!(
                q.error_refine_range(q.geometric_error(d + 1))
                    < q.error_refine_range(q.geometric_error(d)),
                "refine range must shrink with depth"
            );
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

    /// The geomorph window of a leaf ends where its PARENT would have refined, so a
    /// child blends into the geometry that replaces it. The root has no coarser
    /// parent and so never morphs. This is the contract `evolve_cover` reads its
    /// selection off — see [`Quadtree::selected`].
    #[test]
    fn morph_end_matches_parent_refine_range() {
        let q = qt();
        let err = |c: QuadCoord| q.geometric_error(c.depth);
        assert!(
            q.selected(QuadCoord::ROOT, f64::INFINITY)
                .morph_end
                .is_infinite()
        );
        for child in QuadCoord::ROOT.children() {
            let parent_range = q.error_refine_range(err(QuadCoord::ROOT));
            let s = q.selected(child, parent_range);
            assert!((s.morph_end - parent_range).abs() < 1e-9);
            assert!(s.morph_start < s.morph_end);
        }
    }

    #[test]
    fn screen_metric_factor_is_positive_and_scales() {
        use std::f64::consts::FRAC_PI_4;
        let q = Quadtree::from_screen_metric(8000.0, 6, 8000.0, 1080.0, FRAC_PI_4, 2.0, 0.55);
        assert!(q.range_factor > 0.0);
        // Tighter pixel error → larger range_factor (refine sooner / from farther).
        let tight = Quadtree::from_screen_metric(8000.0, 6, 8000.0, 1080.0, FRAC_PI_4, 1.0, 0.55);
        assert!(tight.range_factor > q.range_factor);
        let higher_resolution =
            Quadtree::from_screen_metric(8000.0, 6, 8000.0, 2160.0, FRAC_PI_4, 2.0, 0.55);
        let zoomed =
            Quadtree::from_screen_metric(8000.0, 6, 8000.0, 1080.0, FRAC_PI_4 * 0.5, 2.0, 0.55);
        let range = q.error_refine_range(1.0);
        assert_eq!(higher_resolution.error_refine_range(1.0), 2.0 * range);
        assert!(zoomed.error_refine_range(1.0) > 2.0 * range);
        assert!(q.focus_distance(QuadCoord::ROOT, [0.0, range * 0.5, 0.0], [0.0; 2]) < range);
        assert!(q.focus_distance(QuadCoord::ROOT, [0.0, range * 1.5, 0.0], [0.0; 2]) > range);
    }

    #[test]
    #[should_panic(expected = "target pixel error must be finite and greater than zero")]
    fn invalid_target_pixel_error_is_rejected() {
        Quadtree::from_screen_metric(
            8000.0,
            6,
            8000.0,
            1080.0,
            std::f64::consts::FRAC_PI_4,
            0.0,
            0.55,
        );
    }

    #[test]
    #[should_panic(expected = "vertical field of view must be finite and in (0, pi)")]
    fn invalid_field_of_view_is_rejected() {
        Quadtree::from_screen_metric(8000.0, 6, 8000.0, 1080.0, 0.0, 2.0, 0.55);
    }

    #[test]
    fn support_constraints_preserve_finer_camera_cover_and_restricted_edges() {
        let q = qt();
        let target = q.node_containing(3, [1234.5, -2345.6]);
        let mut selected = vec![q.selected(QuadCoord::ROOT, f64::INFINITY)];
        q.constrain_selection(&mut selected, &[target], &|_, _| 0.05);
        let required = selected.iter().find(|s| s.coord == target).unwrap();
        assert!(required.morph_end.is_infinite());
        let area: f64 = selected.iter().map(|s| s.region.side().powi(2)).sum();
        assert!((area - (2.0 * q.root_half_extent).powi(2)).abs() < 1e-3);
        let finer = q.node_containing(q.max_depth, q.region(target).center);
        q.constrain_selection(&mut selected, &[finer], &|_, _| 0.05);
        assert!(selected.iter().any(|s| s.coord == finer));
        q.constrain_selection(&mut selected, &[target], &|_, _| 0.05);
        assert!(
            selected
                .iter()
                .any(|s| s.coord == finer && s.morph_end.is_infinite())
        );
        let area: f64 = selected.iter().map(|s| s.region.side().powi(2)).sum();
        assert!((area - (2.0 * q.root_half_extent).powi(2)).abs() < 1e-3);
        for a in &selected {
            for b in &selected {
                let dx = (a.region.center[0] - b.region.center[0]).abs();
                let dz = (a.region.center[1] - b.region.center[1]).abs();
                let span = a.region.half + b.region.half;
                let adjacent = ((dx - span).abs() < 1e-6 && dz < span - 1e-6)
                    || ((dz - span).abs() < 1e-6 && dx < span - 1e-6);
                assert!(
                    !adjacent || a.coord.depth.abs_diff(b.coord.depth) <= 1,
                    "unstitcheable seam between {:?} and {:?}",
                    a.coord,
                    b.coord
                );
            }
        }
        let mut combined = vec![q.selected(QuadCoord::ROOT, f64::INFINITY)];
        q.constrain_selection(&mut combined, &[target, finer], &|_, _| 0.05);
        assert!(
            combined
                .iter()
                .filter(|leaf| leaf.coord.depth >= target.depth
                    && (leaf.coord.x >> (leaf.coord.depth - target.depth)) == target.x
                    && (leaf.coord.z >> (leaf.coord.depth - target.depth)) == target.z)
                .all(|leaf| leaf.morph_end.is_infinite())
        );
        let before = selected.iter().map(|s| s.coord).collect::<Vec<_>>();
        selected.reverse();
        q.constrain_selection(&mut selected, &[target], &|_, _| 0.05);
        assert_eq!(before, selected.iter().map(|s| s.coord).collect::<Vec<_>>());
        assert!(
            selected
                .iter()
                .any(|s| s.coord != target && s.morph_end.is_finite())
        );
    }
}
