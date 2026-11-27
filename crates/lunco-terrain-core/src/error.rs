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

/// Measure lost triangle detail and the height envelope using the node's mesh
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
                let mesh_height = triangle_height(h00, h10, h01, h11, u, v);
                let px = x0 + (ci as f64 + u) * step;
                let pz = z0 + (cj as f64 + v) * step;
                let truth = src.height_at(px, pz);
                height_range[0] = height_range[0].min(truth);
                height_range[1] = height_range[1].max(truth);
                let e = (truth - mesh_height).abs();
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

/// Stratified estimate of the error of the actual visual lattice against the
/// next finer filtered surface. Unlike a coarse proxy grid, probes evaluate the
/// indexed triangles that the renderer will draw. `probe_res` bounds work;
/// a posting-density target separately protects features between sampled cells.
pub fn measure_lattice_geometry(
    mesh: &dyn HeightSource,
    reference: &dyn HeightSource,
    region: Square,
    mesh_res: usize,
    probe_res: usize,
) -> NodeGeometry {
    let cells = mesh_res.max(2) - 1;
    let probes = (probe_res.max(2) - 1).min(cells);
    let step = region.side() / cells as f64;
    let x0 = region.center[0] - region.half;
    let z0 = region.center[1] - region.half;
    let cell_index = |i: usize| {
        if probes == 1 {
            cells / 2
        } else {
            i * (cells - 1) / (probes - 1)
        }
    };
    let mut height_range = [f64::INFINITY, f64::NEG_INFINITY];
    let mut max_error = 0.0_f64;
    for j in 0..probes {
        for i in 0..probes {
            let x = x0 + cell_index(i) as f64 * step;
            let z = z0 + cell_index(j) as f64 * step;
            let corners = [
                mesh.height_at(x, z),
                mesh.height_at(x + step, z),
                mesh.height_at(x, z + step),
                mesh.height_at(x + step, z + step),
            ];
            // Include postings as well as interior points: filtering can alter
            // a rim even where a fine sample coincides with the coarse grid.
            for &(u, v) in CELL_SAMPLES
                .iter()
                .chain([(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].iter())
            {
                let rendered =
                    triangle_height(corners[0], corners[1], corners[2], corners[3], u, v);
                let truth = reference.height_at(x + u * step, z + v * step);
                height_range[0] = height_range[0].min(rendered).min(truth);
                height_range[1] = height_range[1].max(rendered).max(truth);
                max_error = max_error.max((truth - rendered).abs());
            }
        }
    }
    NodeGeometry {
        error: max_error,
        height_range,
    }
}

/// Interpolate the indexed mesh triangles at unit-cell `(u, v)` coordinates.
/// The shared diagonal joins `(1, 0)` and `(0, 1)`.
#[inline]
fn triangle_height(h00: f64, h10: f64, h01: f64, h11: f64, u: f64, v: f64) -> f64 {
    if u + v <= 1.0 {
        h00 + u * (h10 - h00) + v * (h01 - h00)
    } else {
        h11 + (1.0 - u) * (h01 - h11) + (1.0 - v) * (h10 - h11)
    }
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
        // A plane is represented exactly by the mesh triangles.
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
    fn saddle_error_measures_the_indexed_triangles() {
        struct Saddle;
        impl HeightSource for Saddle {
            fn height_at(&self, x: f64, z: f64) -> f64 {
                x * z
            }
        }
        let region = sq(0.5, 0.5, 0.5);
        // The centre of the diagonal has source height 1/4 while both
        // triangle endpoints have height zero. Quarter probes exercise both
        // triangles; finer cells reduce the error by the squared spacing.
        assert_eq!(measure_node_geometry(&Saddle, region, 2).error, 0.25);
        assert_eq!(measure_node_geometry(&Saddle, region, 3).error, 0.0625);
    }

    #[test]
    fn indexed_triangle_surface_has_zero_residual() {
        struct Triangles;
        impl HeightSource for Triangles {
            fn height_at(&self, x: f64, z: f64) -> f64 {
                if x + z <= 1.0 {
                    2.0 * x + 3.0 * z
                } else {
                    7.0 - 4.0 * (1.0 - x) - 5.0 * (1.0 - z)
                }
            }
        }
        assert_eq!(
            measure_node_geometry(&Triangles, sq(0.5, 0.5, 0.5), 2).error,
            0.0
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
    fn lattice_error_is_measured_against_the_rendered_grid() {
        struct Curved;
        impl HeightSource for Curved {
            fn height_at(&self, x: f64, z: f64) -> f64 {
                x * x + z * z
            }
        }
        let region = sq(0.0, 0.0, 8.0);
        let coarse = measure_lattice_geometry(&Curved, &Curved, region, 9, 5);
        let fine = measure_lattice_geometry(&Curved, &Curved, region, 33, 5);
        assert!((coarse.error - 2.0).abs() < 1e-10);
        assert!((fine.error - 0.125).abs() < 1e-10);
        assert_eq!(coarse.height_range[1], fine.height_range[1]);
        let omitted = measure_lattice_geometry(&Flat, &Curved, region, 33, 5);
        assert!(omitted.error > 1.0);
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

/// Conservative operator-norm bound on the Hessian of one tensor-product
/// Catmull-Rom raster cell. The second derivatives are Bézier polynomials:
/// their control hull bounds every point, including extrema between samples.
pub fn catmull_cell_curvature(samples: [[f64; 4]; 4], spacing: f64) -> f64 {
    let controls = |p: [f64; 4]| {
        [
            p[1],
            p[1] + (p[2] - p[0]) / 6.0,
            p[2] - (p[3] - p[1]) / 6.0,
            p[2],
        ]
    };
    let rows = samples.map(controls);
    let mut b = [[0.0; 4]; 4];
    for x in 0..4 {
        let col = controls(std::array::from_fn(|z| rows[z][x]));
        for z in 0..4 {
            b[z][x] = col[z];
        }
    }
    let mut xx = 0.0_f64;
    let mut zz = 0.0_f64;
    let mut xz = 0.0_f64;
    for z in 0..4 {
        for x in 0..2 {
            xx = xx.max((6.0 * (b[z][x + 2] - 2.0 * b[z][x + 1] + b[z][x])).abs());
        }
    }
    for z in 0..2 {
        for x in 0..4 {
            zz = zz.max((6.0 * (b[z + 2][x] - 2.0 * b[z + 1][x] + b[z][x])).abs());
        }
    }
    for z in 0..3 {
        for x in 0..3 {
            xz = xz.max((9.0 * (b[z + 1][x + 1] - b[z + 1][x] - b[z][x + 1] + b[z][x])).abs());
        }
    }
    // The symmetric Hessian's spectral norm is at most its largest absolute
    // row sum. Account for floating-point error in the control differences.
    let roundoff = 128.0
        * f64::EPSILON
        * samples
            .iter()
            .flatten()
            .fold(1.0_f64, |m, &a| m.max(a.abs()));
    ((xx.max(zz) + xz + roundoff) / (spacing * spacing)).next_up()
}

/// Immutable max pyramid over source raster curvature. Rectangle queries use
/// whole hierarchy nodes wherever possible and exact cell maxima at boundaries.
/// This is independent of camera and mesh depth, and is reusable after zoom.
#[derive(Debug)]
pub struct RasterErrorPyramid {
    levels: Vec<(usize, Vec<f64>)>,
    cells: usize,
}
impl RasterErrorPyramid {
    pub fn new(cells: usize, values: Vec<f64>) -> Option<Self> {
        if cells == 0
            || cells.checked_mul(cells)? != values.len()
            || values.iter().any(|v| !v.is_finite() || *v < 0.0)
        {
            return None;
        }
        let mut levels = vec![(cells, values)];
        while levels.last()?.0 > 1 {
            let (n, prev) = levels.last()?;
            let side = n.div_ceil(2);
            let mut next = vec![0.0_f64; side * side];
            for z in 0..*n {
                for x in 0..*n {
                    let at = (z / 2) * side + x / 2;
                    next[at] = next[at].max(prev[z * n + x]);
                }
            }
            levels.push((side, next));
        }
        Some(Self { levels, cells })
    }
    pub fn max_rect(&self, rect: [usize; 4]) -> Option<f64> {
        if rect[0] >= rect[2] || rect[1] >= rect[3] || rect[2] > self.cells || rect[3] > self.cells
        {
            return None;
        }
        fn visit(p: &RasterErrorPyramid, level: usize, x: usize, z: usize, r: [usize; 4]) -> f64 {
            let block = 1usize << level;
            let min = [x * block, z * block];
            let max = [
                ((x + 1) * block).min(p.cells),
                ((z + 1) * block).min(p.cells),
            ];
            if min[0] >= r[2] || min[1] >= r[3] || max[0] <= r[0] || max[1] <= r[1] {
                return 0.0;
            }
            let (side, values) = &p.levels[level];
            if level == 0 || (min[0] >= r[0] && min[1] >= r[1] && max[0] <= r[2] && max[1] <= r[3])
            {
                return values[z * side + x];
            }
            let child_side = p.levels[level - 1].0;
            let mut bound = 0.0_f64;
            for cz in 2 * z..(2 * z + 2).min(child_side) {
                for cx in 2 * x..(2 * x + 2).min(child_side) {
                    bound = bound.max(visit(p, level - 1, cx, cz, r));
                }
            }
            bound
        }
        Some(visit(self, self.levels.len() - 1, 0, 0, rect))
    }
}

#[cfg(test)]
mod curvature_tests {
    use super::*;
    #[test]
    fn pyramid_keeps_an_unsampled_peaked_cell() {
        let mut values = vec![1.0; 7 * 7];
        values[3 * 7 + 5] = 200.0;
        let p = RasterErrorPyramid::new(7, values).unwrap();
        assert_eq!(p.max_rect([0, 0, 7, 7]), Some(200.0));
        assert_eq!(p.max_rect([0, 0, 5, 7]), Some(1.0));
        assert_eq!(p.max_rect([5, 3, 6, 4]), Some(200.0));
        assert_eq!(p.max_rect([7, 0, 8, 1]), None);
    }
    #[test]
    fn cubic_control_bound_preserves_affine_and_curved_surfaces() {
        let affine =
            std::array::from_fn(|z| std::array::from_fn(|x| 3.0 * x as f64 - 2.0 * z as f64));
        assert!(catmull_cell_curvature(affine, 1.0) < 1e-10);
        let curved = std::array::from_fn(|z| std::array::from_fn(|x| (x * x + z * z) as f64));
        let bound = catmull_cell_curvature(curved, 1.0);
        assert!(bound >= 2.0 && bound < 2.000001);
    }
}
