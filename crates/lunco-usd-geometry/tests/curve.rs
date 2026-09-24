//! Public integration coverage for the shared USD BasisCurves evaluator.

use bevy_math::Vec3;
use lunco_usd_geometry::curve::{CurveBasis, eval_curve, eval_curve_tangent};

fn ring() -> Vec<Vec3> {
    // Four points on a radius-1 circle in the XZ plane.
    vec![
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, -1.0),
        Vec3::new(-1.0, 0.0, 0.0),
    ]
}

#[test]
fn catmull_rom_passes_through_every_control_point() {
    // The property that makes it right for hand-placed points: u at a knot
    // returns that knot exactly, so dragging a point moves the curve THROUGH
    // where you put it (a Bezier hull would only approach it).
    let p = ring();
    for (i, want) in p.iter().enumerate() {
        let u = i as f32 / p.len() as f32; // periodic: 4 segments
        let got = eval_curve(&p, CurveBasis::CatmullRom, true, u).expect("valid curve");
        assert!(
            (got - *want).length() < 1e-5,
            "u={u} got {got:?} want {want:?}"
        );
    }
}

#[test]
fn periodic_curve_closes_without_a_seam() {
    let p = ring();
    let start = eval_curve(&p, CurveBasis::CatmullRom, true, 0.0).expect("valid curve");
    let end = eval_curve(&p, CurveBasis::CatmullRom, true, 1.0).expect("valid curve");
    assert!(
        (start - end).length() < 1e-5,
        "loop must close: {start:?} vs {end:?}"
    );
}

/// USD end conditions for a nonperiodic catmullRom: the first and last CVs
/// are tangent phantoms, so the curve spans p₁…pₙ₋₂. This pins the segment
/// offset into the `bevy_math` curve — bevy's own `to_curve` mirrors extra
/// phantoms and spans p₀…pₙ₋₁, so sampling without the `+1` offset would
/// wrongly start the shot at the authored phantom p₀.
#[test]
fn open_catmull_rom_spans_the_interior_cvs_only() {
    let p = vec![
        Vec3::new(-1.0, 0.0, 0.0), // tangent phantom
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(2.0, 0.0, 0.0),
        Vec3::new(3.0, 0.0, 0.0), // tangent phantom
    ];
    let start = eval_curve(&p, CurveBasis::CatmullRom, false, 0.0).expect("valid curve");
    let end = eval_curve(&p, CurveBasis::CatmullRom, false, 1.0).expect("valid curve");
    assert!(
        (start - p[1]).length() < 1e-5,
        "u=0 is p1, not the phantom p0"
    );
    assert!(
        (end - p[3]).length() < 1e-5,
        "u=1 is pₙ₋₂, not the phantom pₙ₋₁"
    );
    // Interior knot: n − 3 = 2 segments, so u = 0.5 sits exactly on p2.
    let mid = eval_curve(&p, CurveBasis::CatmullRom, false, 0.5).expect("valid curve");
    assert!((mid - p[2]).length() < 1e-5, "interior knot interpolated");
}

/// Fewer than 4 CVs cannot form an open cubic segment and are rejected rather
/// than being silently changed into a polygon.
#[test]
fn open_catmull_rom_under_four_cvs_is_rejected() {
    let p = vec![
        Vec3::ZERO,
        Vec3::new(2.0, 0.0, 0.0),
        Vec3::new(2.0, 2.0, 0.0),
    ];
    assert!(eval_curve(&p, CurveBasis::CatmullRom, false, 0.25).is_none());
}

#[test]
fn malformed_curve_inputs_are_rejected_without_geometry_fallback() {
    let p = vec![
        Vec3::ZERO,
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        Vec3::ONE,
        Vec3::NEG_X,
    ];
    assert!(eval_curve(&p, CurveBasis::Bezier, false, 0.5).is_none());
    assert!(eval_curve(&p, CurveBasis::Linear, false, 1.1).is_none());
    assert!(eval_curve(&[Vec3::NAN, Vec3::X], CurveBasis::Linear, false, 0.5).is_none());
}

#[test]
fn catmull_rom_is_smooth_where_linear_is_a_polygon() {
    // The whole point of the change. Midway between two control points, the
    // linear path cuts the chord (radius < 1) while Catmull-Rom bulges out
    // toward the true circle — i.e. it is not a 12-gon.
    let p = ring();
    let u = 0.125; // midpoint of the first periodic segment
    let lin = eval_curve(&p, CurveBasis::Linear, true, u).expect("valid curve");
    let cr = eval_curve(&p, CurveBasis::CatmullRom, true, u).expect("valid curve");
    let r_lin = (lin.x * lin.x + lin.z * lin.z).sqrt();
    let r_cr = (cr.x * cr.x + cr.z * cr.z).sqrt();
    assert!(r_lin < 0.72, "chord midpoint should cut inside: {r_lin}");
    assert!(
        r_cr > r_lin,
        "catmullRom must bulge past the chord: {r_cr} vs {r_lin}"
    );
    assert!(r_cr < 1.05, "…without overshooting the circle: {r_cr}");
}

#[test]
fn tangent_comes_from_the_curve_derivative() {
    let p = vec![
        Vec3::ZERO,
        Vec3::X,
        Vec3::new(2.0, 1.0, 0.0),
        Vec3::new(3.0, 1.0, 0.0),
    ];
    let tangent = eval_curve_tangent(&p, CurveBasis::Linear, false, 0.25)
        .expect("non-degenerate linear tangent");
    assert_eq!(tangent, Vec3::X);
    assert!(
        eval_curve_tangent(&[Vec3::ZERO, Vec3::ZERO], CurveBasis::Linear, false, 0.5).is_none()
    );
}

#[test]
fn bezier_hits_its_segment_endpoints() {
    let p = vec![
        Vec3::ZERO,
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
    ];
    assert!(
        (eval_curve(&p, CurveBasis::Bezier, false, 0.0).expect("valid curve") - p[0]).length()
            < 1e-5
    );
    assert!(
        (eval_curve(&p, CurveBasis::Bezier, false, 1.0).expect("valid curve") - p[3]).length()
            < 1e-5
    );
}

/// `wrap = "periodic"` means the closing segment ends on CV 0, so `u = 1`
/// lands exactly where `u = 0` did — a loop with no seam to jump across.
/// 6 CVs = 2 periodic segments (`3·segs`), none of them a closing endpoint.
#[test]
fn periodic_bezier_closes_onto_its_first_cv() {
    let p = vec![
        Vec3::ZERO,
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(2.0, 1.0, 0.0),
        Vec3::new(3.0, 0.0, 0.0),
        Vec3::new(2.0, -1.0, 0.0),
        Vec3::new(1.0, -1.0, 0.0),
    ];
    let start = eval_curve(&p, CurveBasis::Bezier, true, 0.0).expect("valid curve");
    let end = eval_curve(&p, CurveBasis::Bezier, true, 1.0).expect("valid curve");
    assert!((start - p[0]).length() < 1e-5, "periodic start is CV 0");
    assert!(
        (end - start).length() < 1e-5,
        "periodic end wraps back onto the start"
    );
    // A periodic six-CV path is not a valid nonperiodic cubic (the latter
    // requires 4 + 3(n - 1) control points), so it must not silently drop
    // the trailing two points. A valid four-CV open path ends on CV 3.
    assert!(eval_curve(&p, CurveBasis::Bezier, false, 1.0).is_none());
    let open_end = eval_curve(&p[..4], CurveBasis::Bezier, false, 1.0).expect("valid curve");
    assert!(
        (open_end - p[3]).length() < 1e-5,
        "nonperiodic stops at its last endpoint"
    );
}
