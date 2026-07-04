//! Measured geometric error — detail earned by the surface, not by camera distance.
//!
//! Today [`Quadtree::geometric_error`](crate::quadtree::Quadtree::geometric_error)
//! is **uniform** (`root / 2^depth`): every node at a depth is assumed equally
//! detailed, so a dead-flat mare and a jagged crater rim refine on the same
//! schedule. That wastes triangles on the flats and starves the rims.
//!
//! [`measure_node_error`] instead asks the [`HeightSource`] how much vertical
//! detail a node actually *loses* when drawn at its own mesh resolution: it
//! bilinearly interpolates the node's corner/vertex grid and compares that against
//! the true surface sampled at interior points, returning the worst deviation
//! (metres). A flat node measures ≈0 (refine nothing); a node straddling a crater
//! rim measures large (refine early, refine deep). Because it is a pure function of
//! the source, every peer computes the same error → the error-driven selection in
//! [`Quadtree::select_with_error`](crate::quadtree::Quadtree::select_with_error)
//! stays view-independent and deterministic, exactly like the uniform path.
//!
//! It is a *sampled* estimate, not a strict bound: a feature hiding entirely
//! between the interior sample points is missed. The interior sample pattern is
//! chosen dense enough (cell centre, edge midpoints, quarter points) that any
//! feature comparable to the node's cell size shows up — which is all the LOD needs
//! to decide "is there detail here worth refining toward."

use crate::quadtree::Square;
use crate::source::HeightSource;

/// Interior sample offsets within a single grid cell, in `(u, v)` unit-cell
/// coordinates (0..1 across the cell). Chosen to catch a feature sitting between
/// the cell's four corner vertices: centre, the four edge midpoints, and the four
/// quarter-diagonal points.
const CELL_SAMPLES: &[(f64, f64)] = &[
    (0.5, 0.5),
    (0.5, 0.0),
    (0.0, 0.5),
    (0.5, 1.0),
    (1.0, 0.5),
    (0.25, 0.25),
    (0.75, 0.25),
    (0.25, 0.75),
    (0.75, 0.75),
];

/// Worst vertical deviation (metres) between a node's `res × res` bilinear mesh and
/// the true `src` surface, sampled at interior points of every cell. `res` is the
/// node's mesh vertex count per side (≥ 2). Zero for a surface the mesh represents
/// exactly (e.g. planar); grows with detail the mesh cannot capture.
pub fn measure_node_error(src: &dyn HeightSource, region: Square, res: usize) -> f64 {
    let n = res.max(2);
    let side = region.side();
    let step = side / (n as f64 - 1.0);
    let x0 = region.center[0] - region.half;
    let z0 = region.center[1] - region.half;

    // Vertex height grid the node's mesh would use.
    let mut verts = vec![0.0f64; n * n];
    for j in 0..n {
        for i in 0..n {
            verts[j * n + i] = src.height_at(x0 + i as f64 * step, z0 + j as f64 * step);
        }
    }

    let mut max_err = 0.0f64;
    for cj in 0..n - 1 {
        for ci in 0..n - 1 {
            let h00 = verts[cj * n + ci];
            let h10 = verts[cj * n + ci + 1];
            let h01 = verts[(cj + 1) * n + ci];
            let h11 = verts[(cj + 1) * n + ci + 1];
            for &(u, v) in CELL_SAMPLES {
                let bil = bilerp(h00, h10, h01, h11, u, v);
                let px = x0 + (ci as f64 + u) * step;
                let pz = z0 + (cj as f64 + v) * step;
                let truth = src.height_at(px, pz);
                let e = (truth - bil).abs();
                if e > max_err {
                    max_err = e;
                }
            }
        }
    }
    max_err
}

/// Bilinear interpolation of a cell's four corner heights at unit-cell `(u, v)`.
#[inline]
fn bilerp(h00: f64, h10: f64, h01: f64, h11: f64, u: f64, v: f64) -> f64 {
    let a = h00 + (h10 - h00) * u;
    let b = h01 + (h11 - h01) * u;
    a + (b - a) * v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::AnalyticHeightSource;

    struct Flat;
    impl HeightSource for Flat {
        fn height_at(&self, _x: f64, _z: f64) -> f64 {
            42.0
        }
    }

    /// A single sharp Gaussian bump centred at the origin — the "feature" a flat
    /// mesh cannot represent.
    struct Bump {
        amp: f64,
        sigma: f64,
    }
    impl HeightSource for Bump {
        fn height_at(&self, x: f64, z: f64) -> f64 {
            let r2 = x * x + z * z;
            self.amp * (-r2 / (2.0 * self.sigma * self.sigma)).exp()
        }
    }

    fn sq(cx: f64, cz: f64, half: f64) -> Square {
        Square { center: [cx, cz], half }
    }

    #[test]
    fn flat_surface_has_zero_error() {
        // A plane is represented exactly by any bilinear mesh → no error anywhere.
        assert_eq!(measure_node_error(&Flat, sq(0.0, 0.0, 100.0), 8), 0.0);
        assert_eq!(measure_node_error(&Flat, sq(500.0, -300.0, 50.0), 16), 0.0);
    }

    #[test]
    fn feature_gives_positive_error_scaling_with_amplitude() {
        let region = sq(0.0, 0.0, 100.0);
        let small = measure_node_error(&Bump { amp: 1.0, sigma: 20.0 }, region, 8);
        let big = measure_node_error(&Bump { amp: 10.0, sigma: 20.0 }, region, 8);
        assert!(small > 0.0, "a bump must register error");
        assert!(big > small * 5.0, "error scales ~linearly with amplitude");
    }

    #[test]
    fn error_falls_as_resolution_rises() {
        // A denser mesh captures the bump better → less residual error.
        let src = Bump { amp: 10.0, sigma: 30.0 };
        let region = sq(0.0, 0.0, 100.0);
        let coarse = measure_node_error(&src, region, 4);
        let fine = measure_node_error(&src, region, 32);
        assert!(fine < coarse, "finer mesh {fine} should beat coarse {coarse}");
    }

    #[test]
    fn error_is_local_feature_here_flats_elsewhere() {
        // The bump sits at the origin; a node over it has error, a distant node ≈0.
        let src = Bump { amp: 10.0, sigma: 15.0 };
        let over = measure_node_error(&src, sq(0.0, 0.0, 40.0), 8);
        let away = measure_node_error(&src, sq(2000.0, 2000.0, 40.0), 8);
        assert!(over > 1.0, "node over feature should measure real error");
        assert!(away < 1e-6, "node far from feature should be ~flat");
    }

    #[test]
    fn deterministic() {
        let src = AnalyticHeightSource::default();
        let region = sq(123.0, -456.0, 250.0);
        assert_eq!(measure_node_error(&src, region, 16), measure_node_error(&src, region, 16));
    }
}
