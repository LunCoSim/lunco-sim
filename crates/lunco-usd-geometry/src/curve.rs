//! USD BasisCurves evaluation shared by camera and visual geometry adapters.
//!
//! The basis and segment rules live in the geometry package because they are
//! authored USD geometry semantics, not camera policy. Consumers choose how to
//! use the evaluated position or tangent; this module owns only validation,
//! interpolation, and USD's periodic/open control-point rules.

use bevy_math::cubic_splines::{
    CubicBezier, CubicCardinalSpline, CubicGenerator, CyclicCubicGenerator,
};
use bevy_math::Vec3;

/// Which standard basis the curve interpolates with (uniform token basis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveBasis {
    /// Passes THROUGH its points - what hand-placed control points want.
    CatmullRom,
    /// Cubic Bezier: 4 points per segment, endpoints shared (1 + 3n points).
    Bezier,
    /// `type = "linear"` - the polygon. Honest about what it is.
    Linear,
}

/// Evaluate the curve at normalised `u` ∈ [0, 1]. Invalid authored geometry is
/// rejected rather than replaced by a point, polygon, or other guessed curve.
///
/// Uniform in the curve parameter, NOT arc length — so points spaced unevenly
/// make a consumer traverse sparse stretches faster. Fine for an even orbit;
/// a path with clustered points wants arc-length reparameterisation, which is a
/// separate policy from this geometric evaluator.
pub fn eval_curve(points: &[Vec3], basis: CurveBasis, periodic: bool, u: f32) -> Option<Vec3> {
    eval_curve_with_tangent(points, basis, periodic, u).map(|(position, _)| position)
}

/// Evaluate the curve's position and exact parametric tangent at `u`.
///
/// The tangent comes from the same cubic segment coefficients as the position;
/// downstream aiming and overlays therefore do not depend on arbitrary
/// finite-difference offsets.
pub fn eval_curve_tangent(
    points: &[Vec3],
    basis: CurveBasis,
    periodic: bool,
    u: f32,
) -> Option<Vec3> {
    let (_, tangent) = eval_curve_with_tangent(points, basis, periodic, u)?;
    tangent
        .is_finite()
        .then_some(tangent)
        .filter(|tangent| tangent.length_squared() > f32::EPSILON)
}

fn eval_curve_with_tangent(
    points: &[Vec3],
    basis: CurveBasis,
    periodic: bool,
    u: f32,
) -> Option<(Vec3, Vec3)> {
    if points.len() < 2
        || !u.is_finite()
        || !(0.0..=1.0).contains(&u)
        || points.iter().any(|point| !point.is_finite())
    {
        return None;
    }
    match basis {
        CurveBasis::Linear => eval_linear_with_tangent(points, periodic, u),
        CurveBasis::Bezier => eval_bezier_with_tangent(points, periodic, u),
        CurveBasis::CatmullRom => eval_catmull_rom_with_tangent(points, periodic, u),
    }
}

fn eval_linear_with_tangent(points: &[Vec3], periodic: bool, u: f32) -> Option<(Vec3, Vec3)> {
    let segs = if periodic {
        points.len()
    } else {
        points.len() - 1
    };
    let (i, f) = segment(segs, u)?;
    let a = points[i % points.len()];
    let b = points[(i + 1) % points.len()];
    let result = a.lerp(b, f);
    let tangent = b - a;
    (result.is_finite() && tangent.is_finite()).then_some((result, tangent))
}

/// Catmull-Rom: interpolates its control points, so the curve goes THROUGH the
/// points you place. Periodic curves wrap. Non-periodic ones follow USD's end
/// conditions: the first and last CVs are TANGENT PHANTOMS, so the curve spans
/// p₁…pₙ₋₂ with `n − 3` segments (UsdGeomBasisCurves segment counting). Fewer
/// than 4 CVs cannot form a cubic segment and are rejected.
///
/// The numeric core is `bevy_math`'s [`CubicCardinalSpline`] at tension 0.5 —
/// the same generator `lunco-celestial/src/trajectories.rs` uses, and the same
/// basis matrix the old hand-rolled evaluator carried. Only the USD CV
/// bookkeeping lives here:
/// - cyclic: bevy's `to_curve_cyclic` segment `i` reads exactly the window
///   `(pᵢ₋₁, pᵢ, pᵢ₊₁, pᵢ₊₂) mod n` USD prescribes, so `t = u·n` is direct;
/// - open: bevy MIRRORS phantom endpoints (n − 1 segments over p₀…pₙ₋₁),
///   whereas USD says the authored ends ARE the phantoms, so sampling starts
///   one segment in — `t = 1 + u·(n − 3)` — and bevy's mirrored end segments
///   are never touched.
fn eval_catmull_rom_with_tangent(points: &[Vec3], periodic: bool, u: f32) -> Option<(Vec3, Vec3)> {
    let n = points.len();
    if (!periodic && n < 4) || (periodic && n < 3) {
        return None;
    }
    let spline = CubicCardinalSpline::new_catmull_rom(points.iter().copied());
    let (sampled, tangent) = if periodic {
        let curve = spline.to_curve_cyclic().ok()?;
        (curve.position(u * n as f32), curve.velocity(u * n as f32))
    } else {
        let curve = spline.to_curve().ok()?;
        let t = 1.0 + u * (n - 3) as f32;
        (curve.position(t), curve.velocity(t))
    };
    (sampled.is_finite() && tangent.is_finite()).then_some((sampled, tangent))
}

/// Cubic Bezier: 4 CVs per segment, consecutive segments sharing an endpoint.
///
/// Segment counting follows UsdGeomBasisCurves: a `nonperiodic` cubic bezier
/// carries `4 + 3(segs − 1)` CVs, a `periodic` one exactly `3·segs`. The
/// periodic form authors no closing CV — the final segment borrows the first CV
/// back as its endpoint, which is what makes the loop close rather than stop.
/// Too few CVs to form even one cubic segment, or a non-periodic count that is
/// not `4 + 3n`, is rejected.
///
/// Segment windows are gathered here (that indexing IS the USD wrapping rule);
/// the Bernstein evaluation itself is `bevy_math`'s [`CubicBezier`].
fn eval_bezier_with_tangent(points: &[Vec3], periodic: bool, u: f32) -> Option<(Vec3, Vec3)> {
    let n = points.len();
    let segs = if periodic { n / 3 } else { (n - 1) / 3 };
    if segs == 0 || (periodic && !n.is_multiple_of(3)) || (!periodic && !(n - 1).is_multiple_of(3))
    {
        return None;
    }
    // Wrapping the index is the whole of periodicity here: only the closing
    // segment's `b + 3` ever reaches `n`, and there it lands back on CV 0.
    let windows = (0..segs).map(|i| {
        let b = i * 3;
        [
            points[b % n],
            points[(b + 1) % n],
            points[(b + 2) % n],
            points[(b + 3) % n],
        ]
    });
    let curve = CubicBezier::new(windows).to_curve().ok()?;
    let t = u * segs as f32;
    let sampled = curve.position(t);
    let tangent = curve.velocity(t);
    (sampled.is_finite() && tangent.is_finite()).then_some((sampled, tangent))
}

/// Split `u` into (segment index, local fraction).
fn segment(segs: usize, u: f32) -> Option<(usize, f32)> {
    if segs == 0 || !u.is_finite() || !(0.0..=1.0).contains(&u) {
        return None;
    }
    let x = u * segs as f32;
    let i = if u == 1.0 {
        segs - 1
    } else {
        x.floor() as usize
    };
    Some((i, x - i as f32))
}
