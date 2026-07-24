//! Sweep a circular profile along a centerline — the geometry half of
//! `UsdGeomBasisCurves` / `UsdGeomNurbsCurves`.
//!
//! A USD curve prim with `widths` is not a line: it is a **tube**. `widths` is a
//! diameter in object space, so a curve is swept geometry and this module is what
//! makes it visible. (HAB-1's source scene is the motivating case — every one of
//! its 189 curve objects has `bevel_depth > 0` and `extrude = 0`, i.e. a round
//! tube along a centerline, and its 31 chain links are the same primitive with a
//! closed centerline.)
//!
//! ## Why rotation-minimizing frames, and not Frenet
//!
//! Sweeping needs a frame — an orthonormal (tangent, normal, binormal) — at every
//! sample, so the profile ring can be placed. The textbook choice is the **Frenet**
//! frame, built from the curve's second derivative. It is the wrong choice here,
//! and not marginally:
//!
//! - Frenet's normal is `T'/|T'|`, which is **undefined where curvature is zero** —
//!   i.e. along every straight run. A conduit that goes straight, bends, then goes
//!   straight again has an undefined frame on two thirds of its length.
//! - Worse than undefined: as curvature approaches zero the normal does not vanish
//!   quietly, it *flips*. The tube visibly snap-rotates about its own axis at the
//!   moment the curve straightens. On a habitat full of straight pipe runs this is
//!   the dominant visual artifact.
//!
//! A **rotation-minimizing frame** (RMF) instead carries the previous frame forward
//! with the least possible twist. It is defined everywhere, including on perfectly
//! straight segments, and it has no preferred "up" to flip toward.
//!
//! The implementation is the **double-reflection method** of Wang, Jüttler, Zheng &
//! Liu, *Computation of Rotation Minimizing Frames* (ACM TOG 27(1), 2008). Two
//! Householder reflections carry the frame from one sample to the next: the first
//! reflects through the plane between the two points, the second corrects onto the
//! next tangent. It is fourth-order accurate — far better than the O(h) projection
//! method — and costs a handful of dot products per sample.
//!
//! No crate in the tree provides this, and none of the NURBS crates surveyed
//! (`curvo`, `truck`) do either: `curvo` ships a `FrenetFrame`, which has exactly
//! the degeneracy described above. So this is written rather than pulled in, and it
//! is deliberately independent of *which* evaluator produced the centerline — the
//! same sweep serves `BasisCurves` (via [`crate::camera_path::eval_curve`]) and
//! NURBS curves later.

use bevy::asset::RenderAssetUsages;
use bevy::math::{Vec2, Vec3};
// `bevy_mesh`, NOT `bevy::render::render_resource` — the latter is a re-export
// through `bevy_render` (wgpu + naga). `bevy_mesh` depends only on `wgpu-types`,
// so naming these here costs no GPU stack.
// See docs/architecture/render-decoupling.md.
use bevy_mesh::{Indices, Mesh, PrimitiveTopology};

/// An orthonormal frame carried along a centerline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// Unit tangent — the direction of travel.
    pub tangent: Vec3,
    /// Unit normal — where the profile's local +X points.
    pub normal: Vec3,
}

impl Frame {
    /// The binormal, completing the right-handed basis.
    pub fn binormal(&self) -> Vec3 {
        self.tangent.cross(self.normal)
    }
}

/// Pick any unit vector perpendicular to `t`.
///
/// Chooses the world axis *least* aligned with `t` before crossing, so the result
/// never degenerates: crossing with the most-aligned axis would give a near-zero
/// vector for an axis-aligned tangent, which is exactly the common case for a
/// habitat's straight vertical risers and horizontal runs.
fn any_perpendicular(t: Vec3) -> Vec3 {
    let a = if t.x.abs() <= t.y.abs() && t.x.abs() <= t.z.abs() {
        Vec3::X
    } else if t.y.abs() <= t.z.abs() {
        Vec3::Y
    } else {
        Vec3::Z
    };
    t.cross(a).normalize_or_zero()
}

/// Carry an orthonormal frame along `points` with minimal twist.
///
/// Returns one [`Frame`] per input point. `points` must have at least two entries;
/// fewer yields an empty result (a tube needs a direction).
///
/// Double-reflection RMF (Wang et al. 2008 §4). For each step the frame is
/// reflected through the bisecting plane of the segment, then reflected again onto
/// the next tangent — two reflections compose to a rotation, so the frame stays
/// orthonormal without a re-normalisation fudge.
pub fn rotation_minimizing_frames(points: &[Vec3]) -> Vec<Frame> {
    if points.len() < 2 {
        return Vec::new();
    }

    // Per-point tangents: central differences inside, one-sided at the ends.
    // A repeated point yields a zero tangent; carry the previous one rather than
    // emitting a NaN frame (duplicate control points are common in authored data).
    let n = points.len();
    let mut tangents = Vec::with_capacity(n);
    for i in 0..n {
        let raw = if i == 0 {
            points[1] - points[0]
        } else if i == n - 1 {
            points[n - 1] - points[n - 2]
        } else {
            points[i + 1] - points[i - 1]
        };
        let t = raw.normalize_or_zero();
        tangents.push(if t == Vec3::ZERO {
            *tangents.last().unwrap_or(&Vec3::Z)
        } else {
            t
        });
    }

    let mut frames = Vec::with_capacity(n);
    frames.push(Frame {
        tangent: tangents[0],
        normal: any_perpendicular(tangents[0]),
    });

    for i in 0..n - 1 {
        let prev = frames[i];

        // Reflection 1 — through the plane bisecting the segment.
        let v1 = points[i + 1] - points[i];
        let c1 = v1.dot(v1);
        if c1 <= f32::EPSILON {
            // Coincident points: nothing to carry the frame along, so keep it.
            frames.push(Frame {
                tangent: tangents[i + 1],
                normal: prev.normal,
            });
            continue;
        }
        let r_l = prev.normal - (2.0 / c1) * v1.dot(prev.normal) * v1;
        let t_l = prev.tangent - (2.0 / c1) * v1.dot(prev.tangent) * v1;

        // Reflection 2 — onto the next tangent.
        let v2 = tangents[i + 1] - t_l;
        let c2 = v2.dot(v2);
        let normal = if c2 <= f32::EPSILON {
            // Tangent unchanged (a straight run): reflection 2 is the identity.
            // This is the case Frenet cannot express at all.
            r_l
        } else {
            r_l - (2.0 / c2) * v2.dot(r_l) * v2
        };

        // Re-orthogonalise against the tangent. The double reflection is exact in
        // theory; this only sheds accumulated f32 drift over a long run.
        let t = tangents[i + 1];
        let normal = (normal - t * t.dot(normal)).normalize_or_zero();
        let normal = if normal == Vec3::ZERO {
            any_perpendicular(t)
        } else {
            normal
        };

        frames.push(Frame { tangent: t, normal });
    }

    frames
}

/// Sweep a circular profile of `radii[i]` along `points`, producing a tube mesh.
///
/// - `radii` is per-point, so a curve whose `widths` vary taper correctly. A
///   single-element slice is treated as constant (USD's `constant` interpolation).
/// - `sides` is the profile's segment count; 3 is the minimum that encloses volume.
/// - `closed` joins the last ring back to the first — a chain link, an O-ring, any
///   `wrap = "periodic"` curve.
///
/// Normals are computed analytically from the frame (they are exactly the radial
/// direction), not estimated from adjacent triangles: it is both cheaper and
/// correct at the seam, where averaged face normals would crease.
///
/// Returns `None` when there is nothing to sweep.
pub fn sweep_tube(points: &[Vec3], radii: &[f32], sides: usize, closed: bool) -> Option<Mesh> {
    let frames = rotation_minimizing_frames(points);
    if frames.is_empty() || radii.is_empty() || sides < 3 {
        return None;
    }

    let n = frames.len();
    let radius_at = |i: usize| radii[i.min(radii.len() - 1)];

    let mut positions = Vec::with_capacity(n * (sides + 1));
    let mut normals = Vec::with_capacity(n * (sides + 1));
    let mut uvs = Vec::with_capacity(n * (sides + 1));

    // `sides + 1` columns: the seam vertex is duplicated so its UVs can be 0 and 1
    // rather than wrapping — otherwise the whole texture reverses across one quad.
    for (i, f) in frames.iter().enumerate() {
        let r = radius_at(i);
        let b = f.binormal();
        let v = i as f32 / (n - 1).max(1) as f32;
        for s in 0..=sides {
            let a = (s % sides) as f32 / sides as f32 * std::f32::consts::TAU;
            let dir = f.normal * a.cos() + b * a.sin();
            positions.push((points[i] + dir * r).to_array());
            normals.push(dir.to_array());
            uvs.push(Vec2::new(s as f32 / sides as f32, v).to_array());
        }
    }

    let cols = sides + 1;
    let rings = if closed { n } else { n - 1 };
    let mut indices = Vec::with_capacity(rings * sides * 6);
    for i in 0..rings {
        let i0 = i * cols;
        let i1 = ((i + 1) % n) * cols;
        for s in 0..sides {
            let (a, b, c, d) = (i0 + s, i0 + s + 1, i1 + s + 1, i1 + s);
            indices.extend_from_slice(&[a as u32, d as u32, c as u32]);
            indices.extend_from_slice(&[a as u32, c as u32, b as u32]);
        }
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_orthonormal(f: &Frame, ctx: &str) {
        assert!(
            (f.tangent.length() - 1.0).abs() < 1e-4,
            "{ctx}: |t| = {}",
            f.tangent.length()
        );
        assert!(
            (f.normal.length() - 1.0).abs() < 1e-4,
            "{ctx}: |n| = {}",
            f.normal.length()
        );
        assert!(
            f.tangent.dot(f.normal).abs() < 1e-4,
            "{ctx}: t·n = {}",
            f.tangent.dot(f.normal)
        );
    }

    /// The case Frenet cannot do at all: a perfectly straight line has zero
    /// curvature, so `T'/|T'|` is 0/0. RMF must carry one frame the whole way with
    /// no twist — this is the whole reason for the algorithm.
    #[test]
    fn straight_line_has_a_defined_and_untwisted_frame() {
        let pts: Vec<Vec3> = (0..10).map(|i| Vec3::new(0.0, 0.0, i as f32)).collect();
        let frames = rotation_minimizing_frames(&pts);
        assert_eq!(frames.len(), 10);
        let n0 = frames[0].normal;
        for (i, f) in frames.iter().enumerate() {
            assert_orthonormal(f, &format!("frame {i}"));
            assert!(
                (f.normal - n0).length() < 1e-5,
                "frame {i} twisted on a straight line: {:?} vs {n0:?}",
                f.normal
            );
        }
    }

    /// An axis-aligned tangent is the case a naive `cross(t, Vec3::Y)` seed
    /// degenerates on — and vertical risers are exactly what a habitat is full of.
    #[test]
    fn axis_aligned_tangents_do_not_degenerate() {
        for dir in [Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X, -Vec3::Y, -Vec3::Z] {
            let pts: Vec<Vec3> = (0..4).map(|i| dir * i as f32).collect();
            let frames = rotation_minimizing_frames(&pts);
            for (i, f) in frames.iter().enumerate() {
                assert_orthonormal(f, &format!("dir {dir:?} frame {i}"));
            }
        }
    }

    /// Frames stay orthonormal around a full circle, and the total twist is small —
    /// a Frenet frame round a planar circle would be fine, but this pins that the
    /// double reflection does not accumulate drift.
    #[test]
    fn circle_frames_stay_orthonormal_with_minimal_twist() {
        let n = 64;
        let pts: Vec<Vec3> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos() * 5.0, a.sin() * 5.0, 0.0)
            })
            .collect();
        let frames = rotation_minimizing_frames(&pts);
        for (i, f) in frames.iter().enumerate() {
            assert_orthonormal(f, &format!("circle frame {i}"));
        }
        // A planar curve's RMF normal must stay in the plane's normal direction or
        // sweep with it — either way |n·z| must not wander, which is what a
        // twisting frame would show.
        let z0 = frames[0].normal.z.abs();
        for (i, f) in frames.iter().enumerate() {
            assert!(
                (f.normal.z.abs() - z0).abs() < 1e-3,
                "circle frame {i} twisted out of plane: n.z = {}",
                f.normal.z
            );
        }
    }

    /// A bend is where Frenet flips. Assert the frame varies CONTINUOUSLY through
    /// it — consecutive normals must never jump, which is precisely the snap a
    /// Frenet frame produces as curvature passes through zero.
    #[test]
    fn frame_is_continuous_through_a_straight_bend_straight_run() {
        let mut pts = Vec::new();
        for i in 0..8 {
            pts.push(Vec3::new(0.0, 0.0, i as f32)); // straight along +Z
        }
        for i in 1..8 {
            pts.push(Vec3::new(i as f32, 0.0, 7.0)); // turn, straight along +X
        }
        let frames = rotation_minimizing_frames(&pts);
        for w in frames.windows(2) {
            let d = (w[1].normal - w[0].normal).length();
            assert!(d < 0.5, "frame snapped between samples: |Δn| = {d}");
        }
    }

    #[test]
    fn sweep_produces_a_closed_tube_with_expected_counts() {
        let pts: Vec<Vec3> = (0..5).map(|i| Vec3::new(0.0, 0.0, i as f32)).collect();
        let mesh = sweep_tube(&pts, &[0.5], 8, false).expect("tube");
        let cols = 9; // sides + 1 (duplicated seam column)
        assert_eq!(mesh.count_vertices(), 5 * cols);
        // 4 ring gaps x 8 sides x 2 triangles x 3 indices
        let Some(Indices::U32(ix)) = mesh.indices() else {
            panic!("expected u32 indices")
        };
        assert_eq!(ix.len(), 4 * 8 * 6);
    }

    /// A closed curve (a chain link) joins its last ring to its first, so it has
    /// one more ring gap than an open one with the same point count.
    #[test]
    fn closed_sweep_joins_the_last_ring_to_the_first() {
        let n = 6;
        let pts: Vec<Vec3> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos(), a.sin(), 0.0)
            })
            .collect();
        let open = sweep_tube(&pts, &[0.1], 6, false).unwrap();
        let closed = sweep_tube(&pts, &[0.1], 6, true).unwrap();
        let count = |m: &Mesh| match m.indices() {
            Some(Indices::U32(v)) => v.len(),
            _ => 0,
        };
        assert_eq!(
            count(&closed),
            count(&open) + 6 * 6,
            "closed adds one ring gap"
        );
    }

    /// `widths` may be authored per-vertex, so the profile must taper.
    #[test]
    fn per_point_radii_taper_the_tube() {
        let pts: Vec<Vec3> = (0..3).map(|i| Vec3::new(0.0, 0.0, i as f32)).collect();
        let mesh = sweep_tube(&pts, &[1.0, 2.0, 3.0], 4, false).unwrap();
        let pos = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .unwrap()
            .as_float3()
            .unwrap();
        // First ring sits at radius 1 from the axis, last at radius 3.
        let r_of = |v: &[f32; 3]| (v[0] * v[0] + v[1] * v[1]).sqrt();
        assert!(
            (r_of(&pos[0]) - 1.0).abs() < 1e-4,
            "first ring r = {}",
            r_of(&pos[0])
        );
        assert!((r_of(&pos[pos.len() - 1]) - 3.0).abs() < 1e-4);
    }

    #[test]
    fn degenerate_inputs_are_refused_not_panicked() {
        assert!(rotation_minimizing_frames(&[]).is_empty());
        assert!(rotation_minimizing_frames(&[Vec3::ZERO]).is_empty());
        assert!(sweep_tube(&[Vec3::ZERO], &[1.0], 8, false).is_none());
        assert!(sweep_tube(&[Vec3::ZERO, Vec3::Z], &[], 8, false).is_none());
        assert!(sweep_tube(&[Vec3::ZERO, Vec3::Z], &[1.0], 2, false).is_none());
        // Repeated points must not produce NaN frames.
        let dup = [Vec3::ZERO, Vec3::ZERO, Vec3::Z, Vec3::Z];
        for f in rotation_minimizing_frames(&dup) {
            assert!(
                f.normal.is_finite() && f.tangent.is_finite(),
                "NaN frame: {f:?}"
            );
        }
    }
}
