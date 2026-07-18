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

    let curve = NurbsCurve::new(BSplineCurve::new(knot_vec, ctrl));
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
        assert!(worst < 1e-6, "quarter-circle radius error {worst:e} (want exact)");
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

    #[test]
    fn malformed_curves_are_refused_not_guessed() {
        let pts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        let knots = [0.0, 0.0, 1.0, 1.0];
        assert!(sample_nurbs_curve(&pts, &[], 1, &knots, 4).is_empty(), "order < 2");
        assert!(sample_nurbs_curve(&pts, &[], 4, &knots, 4).is_empty(), "cv < order");
        assert!(sample_nurbs_curve(&pts, &[], 2, &[0.0, 0.0], 4).is_empty(), "short knots");
        assert!(sample_nurbs_curve(&pts, &[1.0], 2, &knots, 4).is_empty(), "weight mismatch");
    }
}
