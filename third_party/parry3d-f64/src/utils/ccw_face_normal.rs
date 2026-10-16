use crate::math::Vector;

/// Computes the direction pointing toward the right-hand-side of an oriented segment.
///
/// Returns `None` if the segment is degenerate.
#[inline]
#[cfg(feature = "dim2")]
pub fn ccw_face_normal(pts: [Vector; 2]) -> Option<Vector> {
    let ab = pts[1] - pts[0];
    let res = Vector::new(ab[1], -ab[0]);

    res.try_normalize()
}

/// Computes the normal of a counter-clock-wise triangle.
///
/// Returns `None` if the triangle is degenerate.
#[inline]
#[cfg(feature = "dim3")]
pub fn ccw_face_normal(pts: [Vector; 3]) -> Option<Vector> {
    crate::shape::Triangle::new(pts[0], pts[1], pts[2])
        .robust_scaled_normal()
        .try_normalize()
}

/// Adaptive orientation sign without precision narrowing, including thin simplices.
#[cfg(feature = "dim3")]
pub(crate) fn tetrahedron_orientation(a: Vector, b: Vector, c: Vector, d: Vector) -> f64 {
    let coordinate = |p: Vector| robust::Coord3D {
        x: p.x,
        y: p.y,
        z: p.z,
    };
    robust::orient3d(coordinate(a), coordinate(b), coordinate(c), coordinate(d))
}
