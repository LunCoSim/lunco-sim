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
//! **Selection metric — distance-range, not screen-space-error.** A node refines when
//! the focus is within `range_factor · error` of it. The `range_factor` is computed
//! once from a *canonical* screen metric ([`Quadtree::from_screen_metric`])
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

/// LOD **hysteresis** factor: a node refines when the focus is inside its refine
/// range `r`, and coarsens back only past `1.15 · r`. The bare `dist < r` test has no
/// dead band, so a focus resting ON a boundary re-splits and re-merges that node
/// every frame — a despawn + spawn + reveal animation per flip on a tile whose LOD
/// never changed. 15 % is wide enough to swallow camera jitter and narrow enough that
/// the coarsen edge still lands inside the node's geomorph band (`morph_ratio` 0.7),
/// so the swap that eventually happens is still blended, not popped.
pub const REFINE_HYSTERESIS: f64 = 1.15;

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

    /// The coarser node whose region contains this one (depth − 1). `None` at the root.
    ///
    /// Region containment in a quadtree IS ancestry, so walking this chain is how a
    /// consumer asks "what coarser node covers my area" — the basis of LOD fallback.
    pub fn parent(self) -> Option<QuadCoord> {
        (self.depth > 0).then(|| QuadCoord { depth: self.depth - 1, x: self.x / 2, z: self.z / 2 })
    }

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
        // Guard BOTH divisors. A `target_pixel_error` of 0 (an Inspector knob set to
        // zero) or a `fov_y_rad` of 0 (an uninitialised camera) makes `range_factor`
        // infinite → every node refines to `max_depth` → a triangle/tile blow-up
        // (and `inf · 0 = NaN` downstream). A sub-pixel epsilon floor would only
        // dodge the inf: at 1e-3 px the range factor is still ~1000× sane and the
        // tree STILL refines to max_depth. So clamp to a USABLE band — the same one
        // the caller's own knob uses (`stream_viz` clamps 0.5..32 px).
        let target_pixel_error =
            if target_pixel_error.is_nan() { 2.0 } else { target_pixel_error.clamp(0.25, 64.0) };
        // `tan` of a 0 / non-finite fov → floor at a small positive denominator
        // (`f64::max` also returns the finite side of a NaN).
        let sse_denominator = (2.0 * (0.5 * fov_y_rad).tan()).max(1e-4);
        let range_factor = screen_height_px / (sse_denominator * target_pixel_error);
        Quadtree::new(root_half_extent, max_depth, range_factor, root_geometric_error)
    }

    /// Geometric error (m) of a node at `depth` (3D-Tiles-compatible: halves per level).
    pub fn geometric_error(&self, depth: u8) -> f64 {
        self.root_geometric_error / (1u64 << depth) as f64
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

    /// Distance at which a node with MEASURED surface `error` refines — the same
    /// `range_factor · error` the recursive walk uses, exposed so an incremental
    /// selector evolves the cover under the identical metric instead of a copy of
    /// it that can drift.
    pub fn error_refine_range(&self, error: f64) -> f64 {
        self.range_factor * error.max(0.0)
    }

    /// Camera distance the refine test compares against (horizontal distance to the
    /// node's square, lifted by the eye height).
    pub fn focus_distance(&self, coord: QuadCoord, focus_xz: [f64; 2], eye_height: f64) -> f64 {
        let horizontal = self.region(coord).distance_to(focus_xz);
        (horizontal * horizontal + eye_height * eye_height).sqrt()
    }

    /// The [`Selected`] record for a leaf drawn at `coord`, given the refine range of
    /// its PARENT (`f64::INFINITY` for the root). Shares the geomorph-window rule
    /// with the recursive walk, so a cover built either way morphs identically.
    pub fn selected(&self, coord: QuadCoord, parent_refine_range: f64) -> Selected {
        let morph_end = parent_refine_range;
        let morph_start =
            if morph_end.is_finite() { self.morph_ratio * morph_end } else { f64::INFINITY };
        Selected { coord, region: self.region(coord), morph_start, morph_end }
    }

    /// Force `sel` (a valid REPLACE-refinement cover, as read off
    /// `evolve_cover`'s cover via [`selected`](Self::selected)) to include the **max-depth** node containing
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
        assert!(q.selected(QuadCoord::ROOT, f64::INFINITY).morph_end.is_infinite());
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
        let q = Quadtree::from_screen_metric(8000.0, 6, 8000.0, 1080.0, FRAC_PI_4, 2.0);
        assert!(q.range_factor > 0.0);
        // Tighter pixel error → larger range_factor (refine sooner / from farther).
        let tight = Quadtree::from_screen_metric(8000.0, 6, 8000.0, 1080.0, FRAC_PI_4, 1.0);
        assert!(tight.range_factor > q.range_factor);
    }

    #[test]
    fn degenerate_screen_metric_stays_finite_and_clamped() {
        use std::f64::consts::FRAC_PI_4;
        let mk = |fov: f64, px: f64| Quadtree::from_screen_metric(8000.0, 6, 8000.0, 1080.0, fov, px);

        // `target_pixel_error = 0` (an Inspector knob dragged to zero) must NOT give
        // an infinite range factor — it clamps to the usable floor (0.25 px), i.e. the
        // SAME tree a 0.25-px request builds, not a 1e-3-px one (≈1000× off).
        let zero_px = mk(FRAC_PI_4, 0.0);
        assert!(zero_px.range_factor.is_finite());
        assert_eq!(zero_px.range_factor, mk(FRAC_PI_4, 0.25).range_factor);
        // …and NOT the old 1e-3-px floor, which was ~250× tighter (every node to
        // max_depth — the blow-up the guard claimed to prevent).
        let old_floor_factor = 1080.0 / (2.0 * (0.5 * FRAC_PI_4).tan() * 1e-3);
        assert!(zero_px.range_factor < old_floor_factor * 0.01);

        // `fov_y_rad = 0` (uninitialised camera): `tan(0) = 0` used to zero the
        // divisor ⇒ inf range factor ⇒ every node to max_depth + `inf·0 = NaN` morph
        // bands. The floored denominator keeps everything finite.
        let zero_fov = mk(0.0, 2.0);
        assert!(zero_fov.range_factor.is_finite());
        // The geomorph window is derived from the range factor, so an infinite factor
        // used to surface here as `inf · 0 = NaN` bands. Both ends must stay a number.
        let parent_range = zero_fov.error_refine_range(zero_fov.geometric_error(0));
        let s = zero_fov.selected(QuadCoord::ROOT.children()[0], parent_range);
        assert!(!s.morph_start.is_nan() && !s.morph_end.is_nan());

        // NaN / inf knobs stay finite too (NaN → the 2 px default; inf → the ceiling).
        assert!(mk(FRAC_PI_4, f64::NAN).range_factor.is_finite());
        assert!(mk(FRAC_PI_4, f64::INFINITY).range_factor.is_finite());
        assert_eq!(mk(FRAC_PI_4, f64::INFINITY).range_factor, mk(FRAC_PI_4, 64.0).range_factor);
        assert!(mk(f64::NAN, 2.0).range_factor.is_finite());
    }

    #[test]
    fn forced_refinement_reaches_max_depth_and_keeps_cover_exact() {
        // A far/coarse selection forced to refine around a "rover" position
        // must contain max-depth nodes there while staying an exact disjoint
        // cover of the root (REPLACE refinement invariant).
        let q = qt();
        let flat = |_c: QuadCoord, _r: Square| 0.05; // near-flat → coarse everywhere
        // The coarsest legal cover — one root leaf. This is the shape `evolve_cover`
        // starts from and reads off via `selected`, so forcing detail into it is
        // exactly what production does under a rover.
        let mut sel = vec![q.selected(QuadCoord::ROOT, f64::INFINITY)];
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

    /// [`REFINE_HYSTERESIS`] is consumed by `lunco-terrain-surface`'s `evolve_cover`,
    /// which owns the band's BEHAVIOUR and tests it against a live cover. What this
    /// module still owes that walk is the constant's shape: a real dead band, so a
    /// node coarsens strictly later than it refines and a focus resting on the
    /// boundary cannot flip it every frame.
    #[test]
    fn hysteresis_is_a_real_dead_band() {
        let q = qt();
        assert!(REFINE_HYSTERESIS > 1.0, "a band at or below 1.0 is no band at all");
        let r = q.error_refine_range(q.geometric_error(1));
        assert!(r * REFINE_HYSTERESIS > r, "coarsening must happen strictly later than refining");
    }
}
