//! `UsdGeomNurbsCurves` → sampled polyline, via `truck-geometry`.
//!
//! ## Why a library and not a hand-written evaluator
//!
//! An earlier version of this module hand-rolled Cox-de Boor plus the rational
//! quotient. It worked and was tested — and it was still the wrong call:
//!
//! - **It covered curves only.** `NurbsPatch` is coming (261 lathe objects plus
//!   the dome), and hand-writing that too would leave the project with *two*
//!   evaluators that can disagree numerically. One library, one numeric core.
//! - **`truck-geometry` brings more than the basis function.** `RevolvedCurve`
//!   turns a profile into a surface of revolution — which is **80.4% of HAB-1's
//!   vertices** — and `Circle`/`Torus`/`Sphere` specifieds cover the seals,
//!   O-rings and chain links exactly.
//! - **The objections to it did not survive checking.** It is cgmath-based, so
//!   the `nalgebra` conflict belongs to `curvo`, not here; and `truck-geotrait`
//!   already declares `getrandom 0.2 + ["js"]` for wasm32 — the very pin this
//!   workspace uses.
//!
//! What is NOT delegated is the **sweep**: no crate implements
//! rotation-minimizing frames (`curvo` ships a `FrenetFrame`, which degenerates
//! on exactly the straight runs a habitat is full of), so [`crate::curve_sweep`]
//! stays hand-written. That is the honest split — buy the solved problem, write
//! the unsolved one.
//!
//! ## USD → truck
//!
//! USD authors `order` (degree + 1), a flat `knots` array of `vertexCount + order`
//! values, and optional `pointWeights`. truck wants a [`KnotVec`] and homogeneous
//! control points (`Vector4`, weight in `w`). The rational conversion is
//! `(x·w, y·w, z·w, w)` — pre-multiplied, which is what "homogeneous" means and a
//! detail that silently distorts the curve if missed.

// One glob: `prelude` re-exports `base::*` (which itself globs `cgmath64::*` for
// `Vector4` and `truck_geotrait::*` for the `ParametricCurve` / `BoundedCurve`
// traits that carry `subs` / `range_tuple`) alongside `nurbs::*`. Naming the
// sub-paths individually does NOT work — the traits must be in scope for their
// methods to resolve, and `truck_geotrait` is not a direct dependency.
use bevy::log::warn;
use truck_geometry::prelude::*;

/// Sample a `UsdGeomNurbsCurves` curve into a polyline of `steps + 1` points.
///
/// - `points` — control points.
/// - `weights` — one per control point, or empty for the polynomial case.
/// - `order` — USD's `order` = degree + 1.
/// - `knots` — USD's flat knot array (`points.len() + order` entries).
///
/// Returns an empty vec when the curve is malformed — a bad curve is skipped, not
/// guessed at, and never yields a partial polyline.
pub fn sample_nurbs_curve(
    points: &[[f32; 3]],
    weights: &[f64],
    order: usize,
    knots: &[f64],
    steps: usize,
) -> Vec<[f32; 3]> {
    let cv = points.len();
    if order < 2 || cv < order || knots.len() < cv + order {
        return Vec::new();
    }
    if !weights.is_empty() && weights.len() != cv {
        return Vec::new();
    }

    // USD's knot vector has `cv + order` entries; truck's `KnotVec` for a curve of
    // degree `p = order - 1` over `cv` control points wants the same length, so
    // this is a direct hand-over. Trim any excess USD authored beyond the
    // requirement rather than rejecting — assets over-author this routinely.
    let knot_vec = KnotVec::from(knots[..cv + order].to_vec());

    // Homogeneous control points: PRE-MULTIPLY xyz by w. `(x, y, z, w)` with raw
    // xyz is a different curve — the classic rational-NURBS mistake.
    let ctrl: Vec<Vector4> = points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let w = if weights.is_empty() { 1.0 } else { weights[i] };
            Vector4::new(p[0] as f64 * w, p[1] as f64 * w, p[2] as f64 * w, w)
        })
        .collect();

    // `try_new`, NOT `new` — the latter is `try_new(..).unwrap_or_else(|e| panic!())`.
    // Its `ZeroRange` check is one this function's own guards do NOT cover: a knot
    // vector whose values are all equal (`[0,0,0,0,0,0]`) passes every length and
    // count test above and then panics. Authored USD is untrusted input; a
    // malformed curve must be skipped, never taken down the app with it.
    let Ok(bspline) = BSplineCurve::try_new(knot_vec, ctrl) else {
        return Vec::new();
    };
    let curve = NurbsCurve::new(bspline);
    let (t0, t1) = {
        let r = curve.range_tuple();
        (r.0, r.1)
    };
    if !t0.is_finite() || !t1.is_finite() || t1 <= t0 {
        return Vec::new();
    }

    let steps = steps.max(1);
    let mut out = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = t0 + (t1 - t0) * (i as f64 / steps as f64);
        let p = curve.subs(t);
        if !p.x.is_finite() || !p.y.is_finite() || !p.z.is_finite() {
            return Vec::new();
        }
        out.push([p.x as f32, p.y as f32, p.z as f32]);
    }
    out
}

/// One sampled vertex of a NURBS surface: position, analytic normal, and the
/// `(u, v)` it came from.
pub struct PatchSample {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// Sample a `UsdGeomNurbsPatch` into a `(u_steps + 1) × (v_steps + 1)` grid.
///
/// - `points` — control net, **row-major**: `v` rows of `u` points, matching USD's
///   `uVertexCount` × `vVertexCount` layout.
/// - `weights` — one per control point, or empty for the polynomial case.
/// - `u_order` / `v_order` — USD's orders (degree + 1).
/// - `u_knots` / `v_knots` — `uVertexCount + uOrder` and `vVertexCount + vOrder`.
///
/// Normals come from `uder × vder` — **analytic**, so they are exact at the poles
/// and seams where averaging adjacent triangle normals creases. That matters for
/// HAB-1's dome, whose apex is exactly the degenerate spot a face-averaged normal
/// gets wrong.
///
/// Returns an empty vec when the patch is malformed — skipped, never guessed.
#[allow(clippy::too_many_arguments)]
pub fn sample_nurbs_patch(
    points: &[[f32; 3]],
    weights: &[f64],
    u_count: usize,
    v_count: usize,
    u_order: usize,
    v_order: usize,
    u_knots: &[f64],
    v_knots: &[f64],
    u_steps: usize,
    v_steps: usize,
) -> Vec<PatchSample> {
    let uvs: Vec<[f64; 2]> = {
        let (us, vs) = (u_steps.max(1), v_steps.max(1));
        let mut g = Vec::with_capacity((us + 1) * (vs + 1));
        for iv in 0..=vs {
            for iu in 0..=us {
                g.push([iu as f64 / us as f64, iv as f64 / vs as f64]);
            }
        }
        g
    };
    sample_nurbs_patch_at(
        points, weights, u_count, v_count, u_order, v_order, u_knots, v_knots, &uvs,
    )
}

/// Sample a patch at arbitrary **normalised** `(u, v)` in `[0, 1]²`.
///
/// The grid sampler above is this with a regular lattice. Trimming needs the
/// general form: a trimmed domain is an irregular triangulation, not a lattice,
/// so its vertices land wherever the trim boundary puts them.
///
/// Normalised rather than raw knot-space so callers never have to know how the
/// patch was knotted — `trim.rs` works in `[0, 1]²` throughout.
#[allow(clippy::too_many_arguments)]
pub fn sample_nurbs_patch_at(
    points: &[[f32; 3]],
    weights: &[f64],
    u_count: usize,
    v_count: usize,
    u_order: usize,
    v_order: usize,
    u_knots: &[f64],
    v_knots: &[f64],
    uvs: &[[f64; 2]],
) -> Vec<PatchSample> {
    // Every early return below is a SILENT SKIP at the call site: the caller turns
    // an empty vec into `None` and the surface simply is not there. A patch that
    // vanishes with no log line is the worst failure mode available — it cost most
    // of a session on the HAB-1 dome. Each path therefore names itself.
    if u_order < 2 || v_order < 2 || u_count < u_order || v_count < v_order {
        warn!(
            u_count,
            v_count,
            u_order,
            v_order,
            "NurbsPatch skipped: order/count invalid (need order >= 2 and count >= order)"
        );
        return Vec::new();
    }
    if points.len() < u_count * v_count {
        warn!(
            got = points.len(),
            need = u_count * v_count,
            u_count,
            v_count,
            "NurbsPatch skipped: too few control points for the declared net"
        );
        return Vec::new();
    }
    if u_knots.len() < u_count + u_order || v_knots.len() < v_count + v_order {
        warn!(
            u_knots = u_knots.len(),
            u_need = u_count + u_order,
            v_knots = v_knots.len(),
            v_need = v_count + v_order,
            "NurbsPatch skipped: knot vector shorter than count + order"
        );
        return Vec::new();
    }
    if !weights.is_empty() && weights.len() < u_count * v_count {
        warn!(
            got = weights.len(),
            need = u_count * v_count,
            "NurbsPatch skipped: fewer weights than control points"
        );
        return Vec::new();
    }

    // truck's control net is `Vec<Vec<P>>` indexed [u][v]; USD's `points` is
    // row-major over v-rows of u-points. Transpose while converting.
    let mut ctrl: Vec<Vec<Vector4>> = Vec::with_capacity(u_count);
    for iu in 0..u_count {
        let mut col = Vec::with_capacity(v_count);
        for iv in 0..v_count {
            let idx = iv * u_count + iu;
            let p = points[idx];
            let w = if weights.is_empty() {
                1.0
            } else {
                weights[idx]
            };
            // Homogeneous: PRE-MULTIPLY. Raw xyz with a weight is a different
            // surface, and a plausible-looking one.
            col.push(Vector4::new(
                p[0] as f64 * w,
                p[1] as f64 * w,
                p[2] as f64 * w,
                w,
            ));
        }
        ctrl.push(col);
    }

    let uk = KnotVec::from(u_knots[..u_count + u_order].to_vec());
    let vk = KnotVec::from(v_knots[..v_count + v_order].to_vec());
    // `try_new` for the same reason the curve path uses it — `new` panics, and a
    // zero-range knot vector passes every check above.
    let bsp = match BSplineSurface::try_new((uk, vk), ctrl) {
        Ok(b) => b,
        Err(e) => {
            warn!(
                error = %e,
                u_count, v_count, u_order, v_order,
                u_knots = ?&u_knots[..(u_count + u_order).min(u_knots.len())],
                v_knots = ?&v_knots[..(v_count + v_order).min(v_knots.len())],
                "NurbsPatch skipped: truck rejected the control net / knot vectors"
            );
            return Vec::new();
        }
    };
    let surface = NurbsSurface::new(bsp);

    let ((u0, u1), (v0, v1)) = surface.range_tuple();
    if !(u0.is_finite() && u1.is_finite() && v0.is_finite() && v1.is_finite())
        || u1 <= u0
        || v1 <= v0
    {
        warn!(
            u0,
            u1, v0, v1, "NurbsPatch skipped: degenerate parameter range (non-finite or zero-width)"
        );
        return Vec::new();
    }

    let mut out = Vec::with_capacity(uvs.len());
    {
        for &[tu, tv] in uvs {
            let u = u0 + (u1 - u0) * tu.clamp(0.0, 1.0);
            let v = v0 + (v1 - v0) * tv.clamp(0.0, 1.0);
            let p = surface.subs(u, v);
            let n = surface.normal(u, v);
            if !p.x.is_finite() || !p.y.is_finite() || !p.z.is_finite() {
                warn!(
                    u,
                    v,
                    x = p.x,
                    y = p.y,
                    z = p.z,
                    "NurbsPatch skipped: non-finite point evaluated (check for zero weights)"
                );
                return Vec::new();
            }
            // A degenerate row (a dome apex, where every control point collapses to
            // one) has a zero cross product and so a NaN normal. Substitute +Y
            // rather than emitting NaN into the vertex buffer, which would render
            // as a black hole and is miserable to trace back to a normal.
            let n = if n.x.is_finite() && n.y.is_finite() && n.z.is_finite() {
                [n.x as f32, n.y as f32, n.z as f32]
            } else {
                [0.0, 1.0, 0.0]
            };
            out.push(PatchSample {
                position: [p.x as f32, p.y as f32, p.z as f32],
                normal: n,
                uv: [tu as f32, tv as f32],
            });
        }
    }
    out
}

/// The default clamped, uniform knot vector for `cv` control points of `order`.
///
/// USD requires `knots`, but assets omit it often enough that reconstructing the
/// standard clamped vector beats dropping the curve. Clamped ⇒ the curve starts at
/// the first control point and ends at the last, which is what every DCC writes.
pub fn default_clamped_knots(cv: usize, order: usize) -> Vec<f64> {
    let mut k = Vec::with_capacity(cv + order);
    for _ in 0..order {
        k.push(0.0);
    }
    let interior = cv.saturating_sub(order);
    for i in 1..=interior {
        k.push(i as f64 / (interior + 1) as f64);
    }
    for _ in 0..order {
        k.push(1.0);
    }
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE test that justifies carrying weights at all.
    ///
    /// A quarter circle is *exactly* a rational quadratic with middle weight
    /// √2/2, and is **not** representable polynomially. Every sample must sit on
    /// the unit circle. This matters concretely: every pipe elbow, O-ring, seal
    /// and chain link in HAB-1 is a conic.
    #[test]
    fn rational_quarter_circle_is_exact() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let pts = [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        let knots = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let poly = sample_nurbs_curve(&pts, &[1.0, s, 1.0], 3, &knots, 64);
        assert_eq!(poly.len(), 65, "expected a full polyline");
        let mut worst = 0.0f64;
        for p in &poly {
            let r = ((p[0] as f64).powi(2) + (p[1] as f64).powi(2)).sqrt();
            worst = worst.max((r - 1.0).abs());
        }
        assert!(
            worst < 1e-6,
            "quarter-circle radius error {worst:e} (want exact)"
        );
    }

    /// The companion: without weights the same control net sags off the circle.
    /// Pins that the rational path is doing real work — if it silently degraded to
    /// polynomial, the exactness test above could still pass on unit weights.
    #[test]
    fn dropping_weights_visibly_breaks_the_circle() {
        let pts = [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        let knots = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let poly = sample_nurbs_curve(&pts, &[], 3, &knots, 2);
        let mid = poly[1];
        let r = ((mid[0] as f64).powi(2) + (mid[1] as f64).powi(2)).sqrt();
        assert!(
            (r - 1.0).abs() > 0.05,
            "polynomial mid-arc should sag off the circle, got r = {r}"
        );
    }

    /// Order 2 (degree 1) is a polyline through the control points.
    #[test]
    fn order_two_reproduces_the_control_polygon() {
        let pts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]];
        let knots = [0.0, 0.0, 0.5, 1.0, 1.0];
        let poly = sample_nurbs_curve(&pts, &[], 2, &knots, 2);
        assert_eq!(poly.len(), 3);
        for (got, want) in poly.iter().zip(pts.iter()) {
            for k in 0..3 {
                assert!((got[k] - want[k]).abs() < 1e-5, "{got:?} vs {want:?}");
            }
        }
    }

    /// A clamped curve interpolates its endpoints — the property that makes
    /// [`default_clamped_knots`] a safe fallback for assets missing `knots`.
    #[test]
    fn clamped_curve_interpolates_its_endpoints() {
        let pts = [
            [0.0, 0.0, 0.0],
            [1.0, 2.0, 0.0],
            [3.0, 2.0, 0.0],
            [4.0, 0.0, 0.0],
        ];
        let knots = default_clamped_knots(pts.len(), 4);
        let poly = sample_nurbs_curve(&pts, &[], 4, &knots, 16);
        assert!(!poly.is_empty());
        for k in 0..3 {
            assert!((poly[0][k] - pts[0][k]).abs() < 1e-4, "start {:?}", poly[0]);
            assert!(
                (poly[poly.len() - 1][k] - pts[3][k]).abs() < 1e-4,
                "end {:?}",
                poly[poly.len() - 1]
            );
        }
    }

    /// Repeated interior knots are a C0 corner, and where a naive Cox-de Boor
    /// divides by zero. USD assets carry them at every sharp bend.
    #[test]
    fn repeated_knots_do_not_produce_nan() {
        let pts = [
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [2.0, 0.0, 0.0],
            [3.0, 1.0, 0.0],
            [4.0, 0.0, 0.0],
        ];
        let knots = [0.0, 0.0, 0.0, 0.5, 0.5, 1.0, 1.0, 1.0];
        let poly = sample_nurbs_curve(&pts, &[], 3, &knots, 50);
        assert!(!poly.is_empty(), "repeated knots must still evaluate");
        for p in &poly {
            assert!(p.iter().all(|c| c.is_finite()), "NaN at {p:?}");
        }
    }

    /// A zero-range knot vector passes every length/count guard and then makes
    /// truck's `new` **panic**. Authored USD is untrusted input, so this must be a
    /// skip, not a crash. Pins the `try_new` fix.
    #[test]
    fn zero_range_knots_are_skipped_not_panicked() {
        let pts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
        let degenerate = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert!(
            sample_nurbs_curve(&pts, &[], 3, &degenerate, 8).is_empty(),
            "a zero-range knot vector must be skipped, not panic"
        );
    }

    /// A flat bilinear patch: the surface must lie exactly in its own plane, and
    /// the analytic normal must be the plane's normal everywhere.
    #[test]
    fn flat_patch_is_planar_with_correct_normals() {
        // 2x2 control net in the XZ plane (y = 0).
        let pts = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
        ];
        let k = [0.0, 0.0, 1.0, 1.0];
        let g = sample_nurbs_patch(&pts, &[], 2, 2, 2, 2, &k, &k, 4, 4);
        assert_eq!(g.len(), 25, "5x5 grid");
        for s in &g {
            assert!(s.position[1].abs() < 1e-5, "not planar: {:?}", s.position);
            assert!(
                s.normal[1].abs() > 0.99,
                "normal should be ±Y on an XZ plane, got {:?}",
                s.normal
            );
        }
    }

    /// A rational patch swept from a quarter-circle profile: every sample must sit
    /// on the cylinder of radius 1. This is the surface analogue of the curve test
    /// and the case HAB-1's shell and every pipe fitting depend on.
    #[test]
    fn rational_patch_reproduces_an_exact_cylindrical_quarter() {
        let s = std::f64::consts::FRAC_1_SQRT_2;
        // u = quarter-circle arc (rational quadratic), v = linear extrusion along Y.
        let pts = [
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0], // v = 0 row
            [1.0, 2.0, 0.0],
            [1.0, 2.0, 1.0],
            [0.0, 2.0, 1.0], // v = 1 row
        ];
        // Weights follow the same row-major layout as the points.
        let w = [1.0, s, 1.0, 1.0, s, 1.0];
        let uk = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let vk = [0.0, 0.0, 1.0, 1.0];
        let g = sample_nurbs_patch(&pts, &w, 3, 2, 3, 2, &uk, &vk, 16, 4);
        assert!(!g.is_empty(), "patch must evaluate");
        let mut worst = 0.0f64;
        for smp in &g {
            let r = ((smp.position[0] as f64).powi(2) + (smp.position[2] as f64).powi(2)).sqrt();
            worst = worst.max((r - 1.0).abs());
        }
        assert!(
            worst < 1e-5,
            "cylindrical radius error {worst:e} (want exact)"
        );
    }

    #[test]
    fn malformed_patches_are_refused_not_guessed() {
        let pts = [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 1.0]];
        let k = [0.0, 0.0, 1.0, 1.0];
        // order < 2
        assert!(sample_nurbs_patch(&pts, &[], 2, 2, 1, 2, &k, &k, 2, 2).is_empty());
        // control count < order
        assert!(sample_nurbs_patch(&pts, &[], 2, 2, 4, 2, &k, &k, 2, 2).is_empty());
        // too few points for the declared net
        assert!(sample_nurbs_patch(&pts[..2], &[], 2, 2, 2, 2, &k, &k, 2, 2).is_empty());
        // short knots
        assert!(sample_nurbs_patch(&pts, &[], 2, 2, 2, 2, &[0.0, 0.0], &k, 2, 2).is_empty());
        // weight-count mismatch
        assert!(sample_nurbs_patch(&pts, &[1.0], 2, 2, 2, 2, &k, &k, 2, 2).is_empty());
        // zero-range knots — the panic case again, on the surface path
        let z = [0.0, 0.0, 0.0, 0.0];
        assert!(sample_nurbs_patch(&pts, &[], 2, 2, 2, 2, &z, &z, 2, 2).is_empty());
    }

    #[test]
    fn malformed_curves_are_refused_not_guessed() {
        let pts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        let knots = [0.0, 0.0, 1.0, 1.0];
        assert!(
            sample_nurbs_curve(&pts, &[], 1, &knots, 4).is_empty(),
            "order < 2"
        );
        assert!(
            sample_nurbs_curve(&pts, &[], 4, &knots, 4).is_empty(),
            "cv < order"
        );
        assert!(
            sample_nurbs_curve(&pts, &[], 2, &[0.0, 0.0], 4).is_empty(),
            "short knots"
        );
        assert!(
            sample_nurbs_curve(&pts, &[1.0], 2, &knots, 4).is_empty(),
            "weight mismatch"
        );
    }

    /// Builds the HAB-1 dome net: a half-ellipsoid, springline to apex, as a
    /// 9x3 rational patch. This is the surface that rendered NOTHING in the
    /// sandbox with no log line, which is what motivated the warns above.
    ///
    /// Returns (points, weights) in USD's v-major order.
    fn hab1_dome_net(a: f32, b: f32) -> (Vec<[f32; 3]>, Vec<f64>) {
        const C: f64 = std::f64::consts::FRAC_1_SQRT_2; // cos 45
                                                        // 4 quarter spans: on-circle points at 0/90/180/270, corners between.
        let ring = |r: f32, y: f32| -> Vec<[f32; 3]> {
            vec![
                [r, y, 0.0],
                [r, y, r],
                [0.0, y, r],
                [-r, y, r],
                [-r, y, 0.0],
                [-r, y, -r],
                [0.0, y, -r],
                [r, y, -r],
                [r, y, 0.0],
            ]
        };
        let w_u = [1.0, C, 1.0, C, 1.0, C, 1.0, C, 1.0];
        let w_v = [1.0, C, 1.0];

        let mut points = Vec::new();
        points.extend(ring(a, 0.0)); // v0: springline
        points.extend(ring(a, b)); // v1: the quarter-ellipse middle CV
        points.extend(ring(0.0, b)); // v2: apex — DEGENERATE, all identical

        let mut weights = Vec::new();
        for wv in w_v {
            for wu in w_u {
                weights.push(wu * wv);
            }
        }
        (points, weights)
    }

    /// THE DOME REGRESSION. Pins the answer to "why did the patch vanish".
    #[test]
    fn hab1_dome_half_ellipsoid_patch_evaluates() {
        let (points, weights) = hab1_dome_net(7.345, 4.300);
        let u_knots = [0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 4.0];
        let v_knots = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

        let g = sample_nurbs_patch(&points, &weights, 9, 3, 3, 3, &u_knots, &v_knots, 32, 8);
        assert!(
            !g.is_empty(),
            "dome patch produced no samples — read the warn to see which guard fired"
        );

        // Springline must sit on the 7.345 circle, INCLUDING at the 45 degree
        // diagonal, which is exactly where an unweighted net bulges to 7.791.
        for s in g.iter().filter(|s| s.uv[1] < 1e-9) {
            let r = (s.position[0].powi(2) + s.position[2].powi(2)).sqrt();
            assert!(
                (r - 7.345).abs() < 1e-3,
                "springline radius {r} != 7.345 at uv {:?}",
                s.uv
            );
            assert!(s.position[1].abs() < 1e-3, "springline must be at y=0");
        }

        // Apex closes to the axis at the fitted semi-height.
        for s in g.iter().filter(|s| s.uv[1] > 1.0 - 1e-9) {
            let r = (s.position[0].powi(2) + s.position[2].powi(2)).sqrt();
            assert!(r < 1e-3, "apex must collapse to the axis, got r={r}");
            assert!((s.position[1] - 4.300).abs() < 1e-3, "apex must be at b");
        }
    }

    /// THE LIGHTING REGRESSION. A closed shell's normals must point OUT.
    ///
    /// Geometry can be exact and the object still render as a black hole: the
    /// surface normal follows the parameterisation, `N = dP/du x dP/dv`, so a net
    /// wound the wrong way round in `u` yields inward normals. HAB-1 showed this
    /// as a pitch-black dome and a can that "let light inside" — lit on the
    /// surfaces facing AWAY from the sun, because every normal was reversed.
    ///
    /// Why no existing test caught it: `flat_patch_is_planar_with_correct_normals`
    /// asserts `normal[1].abs() > 0.99`. The `abs()` accepts both directions, so
    /// it passed with the normals inverted — the same shape of gap that let the
    /// trim-corner and shell-offset bugs ship. Orientation needs a SIGNED
    /// assertion or it is not being tested at all.
    ///
    /// For a surface of revolution about +Y, "outward" is well defined: at any
    /// non-apex sample the normal must have a positive component along the
    /// sample's own radial direction.
    ///
    /// This test pins the RAW net's handedness: HAB-1's rings are wound
    /// `+X -> +Z`, so `dU x dV` points INWARD and every sample here has a
    /// negative radial dot. That is a fact about the authored net, not a bug to
    /// fix in this function — the surfaces carry twelve trim loops whose `(u, v)`
    /// coordinates would all need mirroring to rewind them. The correction is
    /// `orientation = "leftHanded"` on the prim, applied at mesh-build time by
    /// `apply_patch_orientation`, and the assertion below is what makes that
    /// flag's necessity explicit instead of folklore.
    #[test]
    fn hab1_dome_raw_net_is_left_handed_so_the_prim_must_say_so() {
        let (points, weights) = hab1_dome_net(7.345, 4.300);
        let u_knots = [0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 4.0];
        let v_knots = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

        let g = sample_nurbs_patch(&points, &weights, 9, 3, 3, 3, &u_knots, &v_knots, 32, 8);
        assert!(!g.is_empty(), "dome patch produced no samples");

        let mut checked = 0;
        for s in &g {
            let (x, z) = (s.position[0], s.position[2]);
            let r = (x * x + z * z).sqrt();
            // Skip the apex, where the radial direction is undefined.
            if r < 1e-3 {
                continue;
            }
            let dot = (s.normal[0] * x + s.normal[2] * z) / r;
            // NEGATIVE: the raw net faces inward. If this ever flips positive,
            // someone rewound the rings — and every `orientation = "leftHanded"`
            // authored to compensate has just become a bug that turns the shells
            // black again. Fail loudly rather than let the two changes cancel.
            assert!(
                dot < 0.0,
                "raw net normal at uv {:?} points OUTWARD (position {:?}, normal {:?}, \
                 radial dot {dot}). The rings were rewound — now REMOVE \
                 `orientation = \"leftHanded\"` from the HAB-1 patches, or they will \
                 render inside-out.",
                s.uv,
                s.position,
                s.normal
            );
            checked += 1;
        }
        assert!(
            checked > 100,
            "expected many non-apex samples, got {checked}"
        );
    }

    /// The mid-latitude ring must lie ON the ellipse, not inside or outside it.
    /// A polynomial (unweighted) net passes the endpoint checks above and still
    /// fails here, so this is the assertion that actually pins rationality.
    #[test]
    fn hab1_dome_is_a_true_ellipsoid_not_a_bulged_approximation() {
        let (points, weights) = hab1_dome_net(7.345, 4.300);
        let u_knots = [0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 4.0];
        let v_knots = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let g = sample_nurbs_patch(&points, &weights, 9, 3, 3, 3, &u_knots, &v_knots, 32, 8);
        assert!(!g.is_empty());

        for s in &g {
            let (x, y, z) = (
                s.position[0] as f64,
                s.position[1] as f64,
                s.position[2] as f64,
            );
            let r = (x * x + z * z).sqrt();
            // (r/a)^2 + (y/b)^2 == 1 everywhere on a true half-ellipsoid.
            let f = (r / 7.345).powi(2) + (y / 4.300).powi(2);
            assert!(
                (f - 1.0).abs() < 1e-4,
                "point {:?} off the ellipsoid: implicit {f}",
                s.position
            );
        }
    }
}
