//! Sampled node geometry shared by visual error and camera-distance selection.
//! The existing vertex and interior probes estimate both lost detail and the
//! node's height envelope. These are visual estimates, not physical certificates.

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

/// Visual estimate from one shared set of node probes, in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeGeometry {
    pub error: f64,
    pub height_range: [f64; 2],
}

/// Measure lost bilinear detail and the height envelope using the node's mesh
/// vertices and interior probes. `res` is the vertex count per side (at least two).
pub fn measure_node_geometry(src: &dyn HeightSource, region: Square, res: usize) -> NodeGeometry {
    thread_local! {
        /// Reused vertex-height scratch — this runs per LOD node (on cache miss),
        /// so a fresh `Vec` per call churned the allocator. Purely local, so the
        /// reuse is invisible to callers.
        static VERT_SCRATCH: std::cell::RefCell<Vec<f64>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    let n = res.max(2);
    let side = region.side();
    let step = side / (n as f64 - 1.0);
    let x0 = region.center[0] - region.half;
    let z0 = region.center[1] - region.half;

    // TAKE the scratch buffer out of the cell for the duration instead of holding a
    // `borrow_mut` across `src.height_at()`. `HeightSource` is a PUBLIC trait: an
    // implementation that transitively re-enters `measure_node_geometry` (a composed
    // oracle, an LOD-aware source) would otherwise hit `BorrowMutError` and PANIC.
    // With `take`, the re-entrant call simply allocates its own Vec — slower, never
    // fatal — and the outer call puts its buffer back at the end.
    let mut verts = VERT_SCRATCH.with(|s| s.take());
    verts.clear();
    verts.resize(n * n, 0.0f64);
    for j in 0..n {
        for i in 0..n {
            verts[j * n + i] = src.height_at(x0 + i as f64 * step, z0 + j as f64 * step);
        }
    }

    let mut height_range = [f64::INFINITY, f64::NEG_INFINITY];
    for &height in &verts {
        height_range[0] = height_range[0].min(height);
        height_range[1] = height_range[1].max(height);
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
                height_range[0] = height_range[0].min(truth);
                height_range[1] = height_range[1].max(truth);
                let e = (truth - bil).abs();
                if e > max_err {
                    max_err = e;
                }
            }
        }
    }
    // Return the buffer for the next call (a re-entrant inner call may have parked
    // its own — either is fine; the buffer is pure scratch).
    VERT_SCRATCH.with(|s| s.replace(verts));
    NodeGeometry {
        error: max_err,
        height_range,
    }
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
        Square {
            center: [cx, cz],
            half,
        }
    }

    #[test]
    fn flat_surface_has_zero_error() {
        // A plane is represented exactly by any bilinear mesh → no error anywhere.
        assert_eq!(
            measure_node_geometry(&Flat, sq(0.0, 0.0, 100.0), 8).error,
            0.0
        );
        assert_eq!(
            measure_node_geometry(&Flat, sq(500.0, -300.0, 50.0), 16).error,
            0.0
        );
        assert_eq!(
            measure_node_geometry(&Flat, sq(0.0, 0.0, 100.0), 8).height_range,
            [42.0, 42.0]
        );
    }

    #[test]
    fn feature_gives_positive_error_scaling_with_amplitude() {
        let region = sq(0.0, 0.0, 100.0);
        let small = measure_node_geometry(
            &Bump {
                amp: 1.0,
                sigma: 20.0,
            },
            region,
            8,
        )
        .error;
        let big = measure_node_geometry(
            &Bump {
                amp: 10.0,
                sigma: 20.0,
            },
            region,
            8,
        )
        .error;
        assert!(small > 0.0, "a bump must register error");
        assert!(big > small * 5.0, "error scales ~linearly with amplitude");
    }

    #[test]
    fn error_falls_as_resolution_rises() {
        // A denser mesh captures the bump better → less residual error.
        let src = Bump {
            amp: 10.0,
            sigma: 30.0,
        };
        let region = sq(0.0, 0.0, 100.0);
        let coarse = measure_node_geometry(&src, region, 4).error;
        let fine = measure_node_geometry(&src, region, 32).error;
        assert!(
            fine < coarse,
            "finer mesh {fine} should beat coarse {coarse}"
        );
    }

    #[test]
    fn error_is_local_feature_here_flats_elsewhere() {
        // The bump sits at the origin; a node over it has error, a distant node ≈0.
        let src = Bump {
            amp: 10.0,
            sigma: 15.0,
        };
        let over = measure_node_geometry(&src, sq(0.0, 0.0, 40.0), 8).error;
        let away = measure_node_geometry(&src, sq(2000.0, 2000.0, 40.0), 8).error;
        assert!(over > 1.0, "node over feature should measure real error");
        assert!(away < 1e-6, "node far from feature should be ~flat");
    }

    #[test]
    fn deterministic() {
        let src = AnalyticHeightSource::default();
        let region = sq(123.0, -456.0, 250.0);
        assert_eq!(
            measure_node_geometry(&src, region, 16),
            measure_node_geometry(&src, region, 16)
        );
    }

    /// A `HeightSource` that re-enters `measure_node_geometry` from `height_at` — the
    /// scratch buffer must not be borrow-locked across the callback (it used to be:
    /// `BorrowMutError` panic).
    struct Reentrant {
        depth: std::sync::atomic::AtomicU32,
    }
    impl HeightSource for Reentrant {
        fn height_at(&self, x: f64, _z: f64) -> f64 {
            use std::sync::atomic::Ordering::SeqCst;
            if self.depth.load(SeqCst) == 0 {
                self.depth.store(1, SeqCst);
                let inner = measure_node_geometry(self, sq(0.0, 0.0, 10.0), 4).error;
                self.depth.store(0, SeqCst);
                return x + inner;
            }
            x
        }
    }

    #[test]
    fn a_reentrant_source_does_not_panic() {
        let src = Reentrant {
            depth: std::sync::atomic::AtomicU32::new(0),
        };
        let e = measure_node_geometry(&src, sq(0.0, 0.0, 100.0), 8).error;
        assert!(e.is_finite());
    }
}
