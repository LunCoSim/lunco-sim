//! Trim curves for `UsdGeomNurbsPatch` — the only mechanism in standard USD
//! that puts a genuine hole in a parametric surface.
//!
//! # What a trim is
//!
//! A trimmed patch is an ordinary tensor-product surface plus a set of closed
//! **loops in its (u, v) parameter space**. The loops say which part of the
//! domain survives; the surface itself is untouched. That is what keeps a hole
//! parametric — a porthole stays a curve in parameter space rather than becoming
//! a mesh someone has to reverse-engineer.
//!
//! USD spells this across six arrays, all concatenated and indexed in lockstep
//! (`UsdGeomNurbsPatch`):
//!
//! | Attribute | Meaning |
//! |---|---|
//! | `trimCurve:counts` | curves per loop |
//! | `trimCurve:orders` | order (degree + 1) per curve |
//! | `trimCurve:vertexCounts` | control points per curve |
//! | `trimCurve:knots` | knots, concatenated; `vertexCount + order` each |
//! | `trimCurve:ranges` | the `(min, max)` parameter interval to evaluate |
//! | `trimCurve:points` | control points as **homogeneous 2D** `(x, y, w)` |
//!
//! `points` being homogeneous is the detail worth stating: the parameter-space
//! position is `(x/w, y/w)`, not `(x, y)`. Skipping the divide yields a loop
//! that is subtly the wrong shape and still plausible-looking — the same class
//! of bug as forgetting to pre-multiply weights on the surface itself.
//!
//! Domain classification uses even-odd containment with the patch rectangle as
//! an implicit outer loop. Nested boundaries alternate holes and retained
//! islands independently of loop orientation.
//!
//! Spade owns constrained triangulation. Constraint insertion splits crossing
//! edges while retaining the complete loop boundary. Vertex insertion failure
//! rejects the domain instead of producing an incomplete boundary.

use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};
use truck_geometry::prelude::{BSplineCurve, KnotVec, NurbsCurve, ParametricCurve, Vector3};

/// Closed loops in normalised patch parameter space, `[0, 1]²`.
///
/// Normalised rather than raw `uKnots`/`vKnots` range so the triangulator and
/// the surface sampler agree on one coordinate system regardless of how the
/// patch was knotted.
#[derive(Debug, Clone, Default)]
pub struct TrimLoops {
    pub loops: Vec<Vec<[f64; 2]>>,
}

impl TrimLoops {
    pub fn is_empty(&self) -> bool {
        self.loops.iter().all(|l| l.len() < 3)
    }
}

/// Discretise one trim curve into a polyline.
///
/// Samples at `steps` uniform positions **plus every distinct knot** in range.
///
/// THE KNOTS ARE NOT OPTIONAL. A knot is where the curve loses continuity — for
/// an order-2 (linear) curve, the knots are precisely its corners. Sampling only
/// uniformly cuts every corner that happens to fall between two samples, and
/// replaces it with a chord.
///
/// Deduplicating on `t` matters: knots repeat (clamped ends, and any interior
/// multiplicity), and a duplicated parameter yields a zero-length segment, which
/// is exactly the degenerate constraint edge the CDT should never be handed.
fn tessellate_curve(
    cvs: &[[f64; 3]],
    knots: &[f64],
    order: usize,
    range: [f64; 2],
    steps: usize,
) -> Vec<[f64; 2]> {
    if steps == 0 {
        return Vec::new();
    }
    let (t0, t1) = (range[0], range[1]);
    if !t0.is_finite()
        || !t1.is_finite()
        || t1 <= t0
        || knots.len() != cvs.len().saturating_add(order)
        || order < 2
        || cvs.len() < order
    {
        return Vec::new();
    }
    let lo = knots[order - 1];
    let hi = knots[cvs.len()];
    if hi <= lo || t0 < lo || t1 > hi {
        return Vec::new();
    }
    if cvs
        .iter()
        .any(|[x, y, w]| !x.is_finite() || !y.is_finite() || !w.is_finite() || *w <= 0.0)
        || knots
            .windows(2)
            .any(|pair| !pair[0].is_finite() || pair[1] < pair[0])
        || knots.last().is_some_and(|knot| !knot.is_finite())
    {
        return Vec::new();
    }
    let control_points = cvs.iter().map(|&[x, y, w]| Vector3::new(x, y, w)).collect();
    let Ok(spline) = BSplineCurve::try_new(KnotVec::from(knots.to_vec()), control_points) else {
        return Vec::new();
    };
    let curve = NurbsCurve::new(spline);

    let mut ts: Vec<f64> = (0..=steps)
        .map(|i| t0 + (t1 - t0) * (i as f64 / steps as f64))
        .collect();
    ts.extend(knots.iter().copied().filter(|&k| k > t0 && k < t1));

    ts.sort_by(f64::total_cmp);
    // Relative to the span so this behaves the same whether the curve is knotted
    // 0..1 or 0..15.
    let eps = (t1 - t0) * 1e-9;
    ts.dedup_by(|a, b| (*a - *b).abs() <= eps);

    ts.into_iter()
        .map(|t| {
            let point = curve.subs(t);
            (point.x.is_finite() && point.y.is_finite()).then_some([point.x, point.y])
        })
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

/// Assemble `trimCurve:*` arrays into normalised closed loops.
///
/// `u_range` / `v_range` are the patch's parametric extent, used to normalise
/// into `[0, 1]²`. `steps_per_curve` controls discretisation: too coarse and
/// nearly-tangent loops discretise into crossing polylines, which is the one
/// real tuning knob in this file.
#[allow(clippy::too_many_arguments)]
pub fn assemble_loops(
    counts: &[i32],
    orders: &[i32],
    vertex_counts: &[i32],
    knots: &[f64],
    ranges: &[[f64; 2]],
    points: &[[f64; 3]],
    u_range: [f64; 2],
    v_range: [f64; 2],
    steps_per_curve: usize,
) -> TrimLoops {
    let mut out = TrimLoops::default();
    let (mut curve_i, mut knot_i, mut pt_i) = (0usize, 0usize, 0usize);

    let Some(curve_count) = counts.iter().try_fold(0usize, |total, &count| {
        usize::try_from(count)
            .ok()
            .filter(|count| *count > 0)
            .and_then(|count| total.checked_add(count))
    }) else {
        return TrimLoops::default();
    };
    if orders.len() != curve_count
        || vertex_counts.len() != curve_count
        || (!ranges.is_empty() && ranges.len() != curve_count)
    {
        return TrimLoops::default();
    }

    let (du, dv) = (u_range[1] - u_range[0], v_range[1] - v_range[0]);
    if !(du.is_finite() && dv.is_finite()) || du <= 0.0 || dv <= 0.0 {
        return TrimLoops::default();
    }

    for &n_curves in counts {
        let Some(n_curves) = usize::try_from(n_curves).ok().filter(|count| *count > 0) else {
            return TrimLoops::default();
        };
        let mut loop_pts: Vec<[f64; 2]> = Vec::new();

        for _ in 0..n_curves {
            let Some(&order) = orders.get(curve_i) else {
                return TrimLoops::default();
            };
            let Some(&vc) = vertex_counts.get(curve_i) else {
                return TrimLoops::default();
            };
            let Some(order) = usize::try_from(order).ok().filter(|order| *order >= 2) else {
                return TrimLoops::default();
            };
            let Some(vc) = usize::try_from(vc).ok().filter(|vc| *vc >= order) else {
                return TrimLoops::default();
            };
            let Some(n_knots) = vc.checked_add(order) else {
                return TrimLoops::default();
            };

            let Some(pt_end) = pt_i.checked_add(vc) else {
                return TrimLoops::default();
            };
            let Some(knot_end) = knot_i.checked_add(n_knots) else {
                return TrimLoops::default();
            };
            if pt_end > points.len() || knot_end > knots.len() {
                return TrimLoops::default();
            }

            let cvs = &points[pt_i..pt_end];
            let kn = &knots[knot_i..knot_end];
            let range = if ranges.is_empty() {
                [kn[order - 1], kn[vc]]
            } else {
                let Some(range) = ranges.get(curve_i).copied() else {
                    return TrimLoops::default();
                };
                range
            };

            let seg = tessellate_curve(cvs, kn, order, range, steps_per_curve);
            if seg.len() < 2 {
                return TrimLoops::default();
            }
            // Drop the duplicated joint between consecutive curves of one loop.
            let seg = if loop_pts.is_empty() {
                seg
            } else {
                seg.into_iter().skip(1).collect()
            };
            loop_pts.extend(seg);

            curve_i += 1;
            pt_i = pt_end;
            knot_i = knot_end;
        }

        // Normalise, and close the loop if the author left it open.
        let mut norm: Vec<[f64; 2]> = loop_pts
            .into_iter()
            .map(|[u, v]| [(u - u_range[0]) / du, (v - v_range[0]) / dv])
            .collect();
        if norm.iter().any(|[u, v]| {
            !u.is_finite() || !v.is_finite() || !(0.0..=1.0).contains(u) || !(0.0..=1.0).contains(v)
        }) || norm.len() < 3
        {
            return TrimLoops::default();
        }
        let (first, last) = (norm[0], norm[norm.len() - 1]);
        if (first[0] - last[0]).abs() > 1e-9 || (first[1] - last[1]).abs() > 1e-9 {
            norm.push(first);
        }
        out.loops.push(norm);
    }
    if curve_i != curve_count || knot_i != knots.len() || pt_i != points.len() {
        return TrimLoops::default();
    }
    out
}

/// Is `p` inside the closed polyline `poly`? Standard ray-crossing test.
fn point_in_loop(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let dy = b[1] - a[1];
            if dy.abs() > f64::EPSILON {
                let x = a[0] + (p[1] - a[1]) / dy * (b[0] - a[0]);
                if p[0] < x {
                    inside = !inside;
                }
            }
        }
        j = i;
    }
    inside
}

/// A triangulated, trimmed parameter domain: normalised `(u, v)` vertices plus
/// triangle indices into them.
pub struct TrimmedDomain {
    pub uvs: Vec<[f64; 2]>,
    pub indices: Vec<u32>,
}

/// Triangulate `[0, 1]²` minus the trimmed-away regions.
///
/// `grid` seeds interior points so untrimmed areas still tessellate finely
/// enough to follow surface curvature; the loops are inserted as constraints so
/// the hole boundary is honoured exactly rather than approximated by whichever
/// grid cells happen to straddle it.
pub fn triangulate_trimmed(loops: &TrimLoops, grid: usize) -> Option<TrimmedDomain> {
    if grid < 2
        || loops.loops.iter().any(|loop_points| {
            loop_points.len() < 4
                || loop_points.iter().any(|[u, v]| {
                    !u.is_finite()
                        || !v.is_finite()
                        || !(0.0..=1.0).contains(u)
                        || !(0.0..=1.0).contains(v)
                })
        })
    {
        return None;
    }
    let mut cdt: ConstrainedDelaunayTriangulation<Point2<f64>> =
        ConstrainedDelaunayTriangulation::new();

    // Domain corners and a seeded interior grid.
    let g = grid;
    for iv in 0..=g {
        for iu in 0..=g {
            let (u, v) = (iu as f64 / g as f64, iv as f64 / g as f64);
            // Skip seeds that fall inside a loop — they would be discarded later
            // anyway, and leaving them out keeps the triangulation smaller.
            let p = [u, v];
            if loops.loops.iter().any(|l| point_in_loop(p, l)) {
                continue;
            }
            if cdt.insert(Point2::new(u, v)).is_err() {
                return None;
            }
        }
    }

    // Domain boundary as constraints.
    //
    // Without these the triangulation's outer edge is just the convex hull of
    // whatever got seeded. That is *usually* the unit square, but a loop meeting
    // the boundary removes the seeds that would have pinned that stretch, and the
    // hull then cuts the corner — producing a long thin triangle that spans the
    // opening. Constraining the rectangle explicitly makes the domain edge a
    // hard edge regardless of what the loops do near it.
    let corners: Vec<_> = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
        .iter()
        .map(|&[u, v]| cdt.insert(Point2::new(u, v)))
        .collect::<Result<_, _>>()
        .ok()?;
    for i in 0..4 {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        if a == b {
            return None;
        }
        cdt.add_constraint_and_split(a, b, |p| p);
    }

    // Loop vertices + constraint edges.
    for l in &loops.loops {
        // Insert every vertex FIRST, and bail on the whole loop if any fails.
        //
        // A missing vertex invalidates the boundary; skipping it would cut a
        // chord through the opening instead of retaining the authored loop.
        let mut handles = Vec::with_capacity(l.len());
        let mut ok = true;
        for &[u, v] in l {
            match cdt.insert(Point2::new(u, v)) {
                Ok(h) => handles.push(h),
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok || handles.len() < 4 {
            bevy_log::warn!(
                "[usd-bevy] trim loop could not be inserted: {} of {} vertices",
                handles.len(),
                l.len()
            );
            return None;
        }
        for w in handles.windows(2) {
            if w[0] == w[1] {
                continue;
            }
            // `add_constraint_and_split`, NOT `add_constraint_edge`: the latter
            // panics when loops cross, and skipping the edge would leave the
            // hole with a missing side. See the module docs.
            cdt.add_constraint_and_split(w[0], w[1], |p| p);
        }
        // Close the ring explicitly. `assemble_loops` appends the first point to
        // close the loop, so the final windows(2) pair already spans last->first
        // — but only when that duplicate actually resolved to the same handle.
        // If floating-point drift made it a distinct vertex, the ring is open by
        // one edge and the hole leaks. Constraining first<->last is idempotent
        // when already closed and repairs it when not.
        let (first, last) = (handles[0], handles[handles.len() - 1]);
        if first != last {
            cdt.add_constraint_and_split(first, last, |p| p);
        }
    }

    let mut uvs: Vec<[f64; 2]> = Vec::new();
    let mut index_of = std::collections::HashMap::new();
    let mut indices: Vec<u32> = Vec::new();

    for face in cdt.inner_faces() {
        let vs = face.vertices();
        let c = [
            (vs[0].position().x + vs[1].position().x + vs[2].position().x) / 3.0,
            (vs[0].position().y + vs[1].position().y + vs[2].position().y) / 3.0,
        ];

        // Even-odd, counting the domain rectangle as an implicit outer loop.
        // Inside the domain is one crossing; each enclosing trim loop adds one.
        // Odd survives. Orientation-independent, so USD's unstated winding
        // convention never has to be guessed. See the module docs.
        let mut crossings = 1usize;
        for l in &loops.loops {
            if point_in_loop(c, l) {
                crossings += 1;
            }
        }
        if crossings.is_multiple_of(2) {
            continue;
        }

        for v in vs {
            let p = v.position();
            let key = ((p.x * 1e9) as i64, (p.y * 1e9) as i64);
            let idx = *index_of.entry(key).or_insert_with(|| {
                uvs.push([p.x, p.y]);
                (uvs.len() - 1) as u32
            });
            indices.push(idx);
        }
    }

    if indices.is_empty() {
        return None;
    }
    Some(TrimmedDomain { uvs, indices })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square loop, inserted as a hole in the middle of the domain.
    fn square(cx: f64, cy: f64, h: f64) -> Vec<[f64; 2]> {
        vec![
            [cx - h, cy - h],
            [cx + h, cy - h],
            [cx + h, cy + h],
            [cx - h, cy + h],
            [cx - h, cy - h],
        ]
    }

    /// An order-two closed arch with sharp sill and jamb corners.
    fn arched_opening_curve() -> (Vec<[f64; 3]>, Vec<f64>, [f64; 2]) {
        let cvs: Vec<[f64; 3]> = vec![
            [2.911488, 0.004000, 1.0],
            [3.088512, 0.004000, 1.0],
            [3.088512, 0.161111, 1.0],
            [3.085549, 0.194901, 1.0],
            [3.076843, 0.226389, 1.0],
            [3.062939, 0.253428, 1.0],
            [3.044701, 0.274176, 1.0],
            [3.023268, 0.287218, 1.0],
            [3.000000, 0.291667, 1.0],
            [2.976732, 0.287218, 1.0],
            [2.955299, 0.274176, 1.0],
            [2.937061, 0.253428, 1.0],
            [2.923157, 0.226389, 1.0],
            [2.914451, 0.194901, 1.0],
            [2.911488, 0.161111, 1.0],
            [2.911488, 0.004000, 1.0],
        ];
        // Authored knots: 0,0,1..15,15 — 18 values for vc 16 + order 2.
        let mut knots = vec![0.0, 0.0];
        knots.extend((1..=15).map(|i| i as f64));
        knots.push(15.0);
        (cvs, knots, [0.0, 15.0])
    }

    /// An order-two curve must retain every control-point corner.
    #[test]
    fn order_two_trim_curve_reproduces_every_corner() {
        let (cvs, knots, range) = arched_opening_curve();
        let poly = tessellate_curve(&cvs, &knots, 2, range, 24);

        for (i, cv) in cvs.iter().enumerate() {
            let hit = poly
                .iter()
                .any(|p| (p[0] - cv[0]).abs() < 1e-6 && (p[1] - cv[1]).abs() < 1e-6);
            assert!(
                hit,
                "corner {i} ({}, {}) was cut off — a chord replaced it, which \
                 renders as a triangle across that corner of the opening",
                cv[0], cv[1]
            );
        }
    }

    /// Loop normalization retains the sharp corners of the sampled curve.
    #[test]
    fn assembled_loop_preserves_corners() {
        let (cvs, knots, _) = arched_opening_curve();
        let points = cvs;

        let loops = assemble_loops(
            &[1],
            &[2],
            &[16],
            &knots,
            &[],
            &points,
            [0.0, 4.0], // uRange, as authored on the patch
            [0.0, 1.0], // vRange
            24,
        );
        assert_eq!(loops.loops.len(), 1, "one door loop expected");
        let l = &loops.loops[0];

        // Both sill corners, normalised: u/4, v/1.
        for (name, u, v) in [
            ("left sill", 2.911488 / 4.0, 0.004),
            ("right sill", 3.088512 / 4.0, 0.004),
        ] {
            assert!(
                l.iter()
                    .any(|p| (p[0] - u).abs() < 1e-6 && (p[1] - v).abs() < 1e-6),
                "{name} corner missing from the assembled loop — it will render chamfered"
            );
        }
    }

    /// Triangles must stay outside the curved opening.
    #[test]
    fn arched_opening_contains_no_geometry() {
        let (cvs, knots, _) = arched_opening_curve();
        let points = cvs;
        let loops = assemble_loops(
            &[1],
            &[2],
            &[16],
            &knots,
            &[],
            &points,
            [0.0, 4.0],
            [0.0, 1.0],
            24,
        );
        let domain = triangulate_trimmed(&loops, 54).expect("door patch must triangulate");

        let door = &loops.loops[0];
        for tri in domain.indices.chunks(3) {
            let p: Vec<[f64; 2]> = tri.iter().map(|&i| domain.uvs[i as usize]).collect();
            let c = [
                (p[0][0] + p[1][0] + p[2][0]) / 3.0,
                (p[0][1] + p[1][1] + p[2][1]) / 3.0,
            ];
            assert!(
                !point_in_loop(c, door),
                "triangle {p:?} sits inside the doorway — the opening is not clear"
            );
            // Sample STRICTLY INSIDE the triangle, not on its edges. A triangle
            // that straddles the boundary has interior area inside the loop, so
            // interior samples catch it while the centroid alone might not.
            //
            // Edge midpoints are the obvious choice and are WRONG here: a
            // triangle legitimately outside the loop may still have an edge
            // lying ALONG the loop boundary — the sliver of wall between the
            // domain edge v=0 and the sill at v=0.004 does exactly that. Its
            // midpoint is on the boundary, where a ray-crossing test is
            // undefined and answers arbitrarily. Barycentric interior points
            // have no such ambiguity.
            for w in [[0.6, 0.2, 0.2], [0.2, 0.6, 0.2], [0.2, 0.2, 0.6]] {
                let q = [
                    w[0] * p[0][0] + w[1] * p[1][0] + w[2] * p[2][0],
                    w[0] * p[0][1] + w[1] * p[1][1] + w[2] * p[2][1],
                ];
                assert!(
                    !point_in_loop(q, door),
                    "triangle {p:?} overlaps the doorway — chord across a corner"
                );
            }
        }
    }

    #[test]
    fn point_in_loop_basic() {
        let s = square(0.5, 0.5, 0.2);
        assert!(point_in_loop([0.5, 0.5], &s));
        assert!(!point_in_loop([0.1, 0.1], &s));
        assert!(!point_in_loop([0.95, 0.5], &s));
    }

    #[test]
    fn hole_is_removed_from_domain() {
        let loops = TrimLoops {
            loops: vec![square(0.5, 0.5, 0.2)],
        };
        let d = triangulate_trimmed(&loops, 12).expect("domain triangulates");
        // No surviving triangle may have its centroid inside the hole.
        for tri in d.indices.chunks(3) {
            let c = [
                (d.uvs[tri[0] as usize][0] + d.uvs[tri[1] as usize][0] + d.uvs[tri[2] as usize][0])
                    / 3.0,
                (d.uvs[tri[0] as usize][1] + d.uvs[tri[1] as usize][1] + d.uvs[tri[2] as usize][1])
                    / 3.0,
            ];
            assert!(
                !point_in_loop(c, &loops.loops[0]),
                "triangle centroid {c:?} survived inside the hole"
            );
        }
    }

    #[test]
    fn untrimmed_domain_is_fully_covered() {
        let d = triangulate_trimmed(&TrimLoops::default(), 8).expect("triangulates");
        // Area of the unit square, within tessellation slack.
        let area: f64 = d
            .indices
            .chunks(3)
            .map(|t| {
                let (a, b, c) = (
                    d.uvs[t[0] as usize],
                    d.uvs[t[1] as usize],
                    d.uvs[t[2] as usize],
                );
                ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
            })
            .sum();
        assert!((area - 1.0).abs() < 1e-6, "expected unit area, got {area}");
    }

    /// An arched loop — flat sill, vertical jambs,
    /// semicircular head — sitting near the domain edge.
    #[test]
    fn arch_loop_near_domain_edge_leaves_no_spanning_triangle() {
        let (uc, half, sill, spring, crown) = (0.75, 0.025, 0.004, 0.16, 0.29);
        let mut l = vec![[uc - half, sill], [uc + half, sill], [uc + half, spring]];
        for i in 1..12 {
            let th = std::f64::consts::PI * i as f64 / 12.0;
            l.push([uc + half * th.cos(), spring + (crown - spring) * th.sin()]);
        }
        l.push([uc - half, spring]);
        l.push([uc - half, sill]);
        let loops = TrimLoops { loops: vec![l] };
        let d = triangulate_trimmed(&loops, 54).expect("triangulates");
        for t in d.indices.chunks(3) {
            let (a, b, c) = (
                d.uvs[t[0] as usize],
                d.uvs[t[1] as usize],
                d.uvs[t[2] as usize],
            );
            let ctr = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0];
            assert!(
                !point_in_loop(ctr, &loops.loops[0]),
                "triangle {a:?}/{b:?}/{c:?} survived spanning the doorway"
            );
        }
    }

    /// The domain edge must be a hard edge: total surviving area is the unit
    /// square minus the hole, never more. A hull that cuts a corner shows up
    /// here as missing area.
    #[test]
    fn domain_boundary_is_constrained() {
        let loops = TrimLoops {
            loops: vec![vec![
                [0.5, 0.0],
                [0.6, 0.0],
                [0.6, 0.2],
                [0.5, 0.2],
                [0.5, 0.0],
            ]],
        };
        let d = triangulate_trimmed(&loops, 20).expect("triangulates");
        let area: f64 = d
            .indices
            .chunks(3)
            .map(|t| {
                let (a, b, c) = (
                    d.uvs[t[0] as usize],
                    d.uvs[t[1] as usize],
                    d.uvs[t[2] as usize],
                );
                ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
            })
            .sum();
        // unit square (1.0) minus a 0.1 x 0.2 hole (0.02)
        assert!(
            (area - 0.98).abs() < 1e-3,
            "expected 0.98 of domain to survive, got {area}"
        );
    }

    #[test]
    fn nested_loop_keeps_inner_island() {
        // Outer trim loop with a smaller loop inside it: the island in the middle
        // survives (3 crossings = odd). This is the case a winding-rule guess
        // gets backwards.
        let loops = TrimLoops {
            loops: vec![square(0.5, 0.5, 0.35), square(0.5, 0.5, 0.12)],
        };
        let d = triangulate_trimmed(&loops, 16).expect("triangulates");
        let inner_survives = d.indices.chunks(3).any(|t| {
            let c = [
                (d.uvs[t[0] as usize][0] + d.uvs[t[1] as usize][0] + d.uvs[t[2] as usize][0]) / 3.0,
                (d.uvs[t[0] as usize][1] + d.uvs[t[1] as usize][1] + d.uvs[t[2] as usize][1]) / 3.0,
            ];
            point_in_loop(c, &loops.loops[1])
        });
        assert!(
            inner_survives,
            "island inside the nested loop was discarded"
        );
    }

    #[test]
    fn rational_quarter_circle_is_round() {
        // Rational quadratic quarter arc, radius 1: the standard weights.
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let cvs = [[1.0, 0.0, 1.0], [w, w, w], [0.0, 1.0, 1.0]];
        let knots = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let points = tessellate_curve(&cvs, &knots, 3, [0.0, 1.0], 10);
        assert_eq!(points.len(), 11);
        for p in points {
            let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
            assert!((r - 1.0).abs() < 1e-9, "{p:?} gave radius {r}, expected 1");
        }
    }

    #[test]
    fn homogeneous_trim_controls_retain_double_precision() {
        let origin = 100_000_000.0;
        let points = [
            [origin + 0.25, origin + 0.25, 1.0],
            [origin + 0.75, origin + 0.25, 1.0],
            [origin + 0.75, origin + 0.75, 1.0],
            [origin + 0.25, origin + 0.75, 1.0],
            [origin + 0.25, origin + 0.25, 1.0],
        ];
        let loops = assemble_loops(
            &[1],
            &[2],
            &[5],
            &[0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 4.0],
            &[],
            &points,
            [origin, origin + 1.0],
            [origin, origin + 1.0],
            4,
        );
        assert_eq!(
            loops.loops,
            vec![vec![
                [0.25, 0.25],
                [0.75, 0.25],
                [0.75, 0.75],
                [0.25, 0.75],
                [0.25, 0.25],
            ]]
        );
    }

    #[test]
    fn malformed_trim_is_skipped_not_guessed() {
        assert!(
            assemble_loops(
                &[1, 1],
                &[2, 2],
                &[5, 5],
                &[
                    0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 4.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 4.0
                ],
                &[[0.0, 4.0], [0.0, 4.0]],
                &[
                    [0.25, 0.25, 1.0],
                    [0.75, 0.25, 1.0],
                    [0.75, 0.75, 1.0],
                    [0.25, 0.75, 1.0],
                    [0.25, 0.25, 1.0],
                ],
                [0.0, 1.0],
                [0.0, 1.0],
                4
            )
            .is_empty()
        );
        assert!(tessellate_curve(&[[0.0, 0.0, 1.0]], &[0.0, 1.0], 3, [0.0, 1.0], 4).is_empty());
        assert!(tessellate_curve(&[], &[], 3, [0.0, 1.0], 4).is_empty());
        let valid_points = [[0.0, 0.0, 1.0], [1.0, 1.0, 1.0]];
        for knots in [
            [0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, f64::NAN, 1.0],
        ] {
            assert!(tessellate_curve(&valid_points, &knots, 2, [0.0, 1.0], 4).is_empty());
        }
        assert!(
            tessellate_curve(
                &[[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
                &[0.0, 0.0, 1.0, 1.0],
                2,
                [0.0, 1.0],
                4
            )
            .is_empty()
        );
        assert!(
            assemble_loops(
                &[1],
                &[1],
                &[1],
                &[0.0, 1.0],
                &[],
                &[[0.0, 0.0, 1.0]],
                [0.0, 1.0],
                [0.0, 1.0],
                4
            )
            .is_empty()
        );
        assert!(
            assemble_loops(
                &[1],
                &[2],
                &[2],
                &[0.0, 0.0, 1.0, 1.0],
                &[[0.0, 2.0]],
                &[[0.0, 0.0, 1.0], [1.0, 0.0, 1.0]],
                [0.0, 1.0],
                [0.0, 1.0],
                4
            )
            .is_empty()
        );
        assert!(triangulate_trimmed(&TrimLoops::default(), 1).is_none());
    }
}
