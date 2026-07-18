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
//! # Why this does not need USD's winding rule
//!
//! USD never documents which side of a trim loop is kept. Guessing inverts every
//! hole, and that unknown ([`D2` in HAB1_OPEN_ITEMS]) is what stalled trimming
//! here for a long time.
//!
//! It is avoidable. We classify by **even-odd containment, counting the patch
//! domain rectangle as an implicit outermost loop**. A point inside the domain
//! and inside no loop crosses one boundary — odd, keep. Inside the domain and
//! inside one hole crosses two — even, discard. Inside a hole nested in an outer
//! trim loop crosses three — odd, keep.
//!
//! That rule is orientation-independent, so it produces the same answer whether
//! a loop was authored clockwise or counter-clockwise. The convention stops
//! mattering rather than being guessed.
//!
//! # Why `add_constraint_and_split`
//!
//! `spade`'s `add_constraint_edge` panics when a new constraint crosses an
//! existing one. The obvious guard — gate on `can_add_constraint` and skip —
//! is wrong here: skipping an edge silently drops part of a loop and yields a
//! hole with a missing side. [`ConstrainedDelaunayTriangulation::
//! add_constraint_and_split`] instead splits both constraints at the
//! intersection and inserts the crossing point, which is exactly the repair a
//! trim boundary wants.
//!
//! Valid loops should not cross at all — they are closed and non-self-
//! intersecting by definition. Crossings show up when curved loops are
//! discretised too coarsely, or when the authored data is malformed. Handling
//! them is robustness, not the normal path.

use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

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

/// Evaluate a rational B-spline curve in 2D at parameter `t` (de Boor).
///
/// `cvs` are homogeneous `(x, y, w)`. Returns the perspective-divided point.
/// Returns `None` when the parameters are inconsistent — a malformed trim is
/// skipped, never guessed at.
fn eval_rational_2d(cvs: &[[f64; 3]], knots: &[f64], order: usize, t: f64) -> Option<[f64; 2]> {
    let n = cvs.len();
    let degree = order.checked_sub(1)?;
    if n < order || knots.len() < n + order || degree == 0 {
        return None;
    }

    // Clamp into the valid span range: [knots[degree], knots[n]].
    let lo = knots[degree];
    let hi = knots[n];
    if !(hi > lo) {
        return None;
    }
    let t = t.clamp(lo, hi);

    // Find the knot span k such that knots[k] <= t < knots[k+1].
    let mut k = degree;
    while k + 1 < n && knots[k + 1] <= t {
        k += 1;
    }

    // de Boor on homogeneous coordinates. Working homogeneous throughout is what
    // makes this correct for weighted control points; dividing early would
    // linearly interpolate an already-projected point and bow the curve.
    let mut d: Vec<[f64; 3]> = (0..=degree)
        .map(|j| cvs[(k - degree + j).min(n - 1)])
        .collect();

    for r in 1..=degree {
        for j in (r..=degree).rev() {
            let i = k - degree + j;
            let den = knots[i + order - r] - knots[i];
            let a = if den.abs() < f64::EPSILON {
                0.0
            } else {
                (t - knots[i]) / den
            };
            for c in 0..3 {
                d[j][c] = (1.0 - a) * d[j - 1][c] + a * d[j][c];
            }
        }
    }

    let [x, y, w] = d[degree];
    if w.abs() < 1e-12 || !x.is_finite() || !y.is_finite() {
        return None;
    }
    Some([x / w, y / w])
}

/// Discretise one trim curve into a polyline of `steps` segments.
fn tessellate_curve(
    cvs: &[[f64; 3]],
    knots: &[f64],
    order: usize,
    range: [f64; 2],
    steps: usize,
) -> Vec<[f64; 2]> {
    let steps = steps.max(2);
    let (t0, t1) = (range[0], range[1]);
    (0..=steps)
        .filter_map(|i| {
            let t = t0 + (t1 - t0) * (i as f64 / steps as f64);
            eval_rational_2d(cvs, knots, order, t)
        })
        .collect()
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
    points: &[[f32; 3]],
    u_range: [f64; 2],
    v_range: [f64; 2],
    steps_per_curve: usize,
) -> TrimLoops {
    let mut out = TrimLoops::default();
    let (mut curve_i, mut knot_i, mut pt_i) = (0usize, 0usize, 0usize);

    let (du, dv) = (u_range[1] - u_range[0], v_range[1] - v_range[0]);
    if !(du.is_finite() && dv.is_finite()) || du <= 0.0 || dv <= 0.0 {
        return out;
    }

    for &n_curves in counts {
        let n_curves = n_curves.max(0) as usize;
        let mut loop_pts: Vec<[f64; 2]> = Vec::new();

        for _ in 0..n_curves {
            let Some(&order) = orders.get(curve_i) else {
                return out;
            };
            let Some(&vc) = vertex_counts.get(curve_i) else {
                return out;
            };
            let (order, vc) = (order.max(2) as usize, vc.max(0) as usize);
            let n_knots = vc + order;

            if pt_i + vc > points.len() || knot_i + n_knots > knots.len() {
                return out;
            }

            let cvs: Vec<[f64; 3]> = points[pt_i..pt_i + vc]
                .iter()
                .map(|p| [p[0] as f64, p[1] as f64, p[2] as f64])
                .collect();
            let kn = &knots[knot_i..knot_i + n_knots];
            let range = ranges
                .get(curve_i)
                .copied()
                .unwrap_or([kn[order - 1], kn[vc]]);

            let seg = tessellate_curve(&cvs, kn, order, range, steps_per_curve);
            // Drop the duplicated joint between consecutive curves of one loop.
            let seg = if loop_pts.is_empty() {
                seg
            } else {
                seg.into_iter().skip(1).collect()
            };
            loop_pts.extend(seg);

            curve_i += 1;
            knot_i += n_knots;
            pt_i += vc;
        }

        // Normalise, and close the loop if the author left it open.
        let mut norm: Vec<[f64; 2]> = loop_pts
            .into_iter()
            .map(|[u, v]| [(u - u_range[0]) / du, (v - v_range[0]) / dv])
            .collect();
        if norm.len() >= 3 {
            let (first, last) = (norm[0], norm[norm.len() - 1]);
            if (first[0] - last[0]).abs() > 1e-9 || (first[1] - last[1]).abs() > 1e-9 {
                norm.push(first);
            }
            out.loops.push(norm);
        }
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
    let mut cdt: ConstrainedDelaunayTriangulation<Point2<f64>> =
        ConstrainedDelaunayTriangulation::new();

    // Domain corners and a seeded interior grid.
    let g = grid.max(2);
    for iv in 0..=g {
        for iu in 0..=g {
            let (u, v) = (iu as f64 / g as f64, iv as f64 / g as f64);
            // Skip seeds that fall inside a loop — they would be discarded later
            // anyway, and leaving them out keeps the triangulation smaller.
            let p = [u, v];
            if loops.loops.iter().any(|l| point_in_loop(p, l)) {
                continue;
            }
            let _ = cdt.insert(Point2::new(u, v));
        }
    }

    // Loop vertices + constraint edges.
    for l in &loops.loops {
        let handles: Vec<_> = l
            .iter()
            .filter_map(|&[u, v]| cdt.insert(Point2::new(u, v)).ok())
            .collect();
        for w in handles.windows(2) {
            if w[0] == w[1] {
                continue;
            }
            // `add_constraint_and_split`, NOT `add_constraint_edge`: the latter
            // panics when loops cross, and skipping the edge would leave the
            // hole with a missing side. See the module docs.
            cdt.add_constraint_and_split(w[0], w[1], |p| p);
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
        if crossings % 2 == 0 {
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
        assert!(inner_survives, "island inside the nested loop was discarded");
    }

    #[test]
    fn rational_quarter_circle_is_round() {
        // Rational quadratic quarter arc, radius 1: the standard weights.
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let cvs = [[1.0, 0.0, 1.0], [w, w, w], [0.0, 1.0, 1.0]];
        let knots = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        for i in 0..=10 {
            let t = i as f64 / 10.0;
            let p = eval_rational_2d(&cvs, &knots, 3, t).expect("evaluates");
            let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
            assert!((r - 1.0).abs() < 1e-9, "t={t} gave radius {r}, expected 1");
        }
    }

    #[test]
    fn malformed_trim_is_skipped_not_guessed() {
        assert!(eval_rational_2d(&[[0.0, 0.0, 1.0]], &[0.0, 1.0], 3, 0.5).is_none());
        assert!(eval_rational_2d(&[], &[], 3, 0.5).is_none());
    }
}
