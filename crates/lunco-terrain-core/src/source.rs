//! Terrain height source — the authoritative elevation the streamer samples.
//!
//! A [`HeightSource`] answers `height_at(x, z)` as a **pure function** of world
//! position. Purity is the load-bearing property: derived data (slope, normals,
//! collider arrays, meshes) is then content-addressable and re-derivable on any
//! peer, so networking transfers nothing and the cache key is just
//! `hash(source-id, region, resolution)`.
//!
//! Real deployments back this with a tiled DEM (LOLA/NAC). For bring-up we ship
//! [`AnalyticHeightSource`], a deterministic hash-based value-noise FBM, so the
//! streaming/LOD/collider machinery can be developed and tested with **no
//! external asset**. Swapping in a DEM source later changes only this trait impl.
//!
//! [`CompositeHeightSource`] is a reusable pure blend between two height
//! sources. Application-specific terrain handoffs own their georeferencing,
//! edge continuation, and rendering policy in the relevant domain crate.

use crate::quadtree::Square;

/// An affine height reference used by conservative surface enclosures.
#[derive(Clone, Copy, Debug)]
pub struct HeightPlane {
    pub origin: [f64; 2],
    pub height: f64,
    pub gradient: [f64; 2],
}

impl HeightPlane {
    pub fn horizontal(height: f64) -> Self {
        Self {
            origin: [0.0; 2],
            height,
            gradient: [0.0; 2],
        }
    }

    pub fn height_at(self, x: f64, z: f64) -> f64 {
        self.height
            + self.gradient[0] * (x - self.origin[0])
            + self.gradient[1] * (z - self.origin[1])
    }

    pub fn is_finite(self) -> bool {
        self.origin
            .into_iter()
            .chain(self.gradient)
            .chain([self.height])
            .all(f64::is_finite)
    }

    /// The affine extrema lie at rectangle corners. Padding encloses the
    /// arithmetic used by interval composition at this f64 boundary.
    pub fn height_bounds(self, rect: [f64; 4]) -> [f64; 2] {
        if !self.is_finite()
            || rect.into_iter().any(|v| !v.is_finite())
            || rect[0] > rect[2]
            || rect[1] > rect[3]
        {
            return [f64::NAN; 2];
        }
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for x in [rect[0], rect[2]] {
            for z in [rect[1], rect[3]] {
                let height = self.height_at(x, z);
                lo = lo.min(height);
                hi = hi.max(height);
            }
        }
        let magnitude = self.height.abs()
            + self.gradient[0].abs() * (rect[0].abs().max(rect[2].abs()) + self.origin[0].abs())
            + self.gradient[1].abs() * (rect[1].abs().max(rect[3].abs()) + self.origin[1].abs());
        let pad = 64.0 * f64::EPSILON * magnitude.max(1.0);
        [lo - pad, hi + pad]
    }
}

/// A height interval and an affine residual certificate for the same surface.
/// Both facts are retained: scalar queries consume `height`, while geometric
/// queries also retain slope correlation through `plane` and `residual`.
#[derive(Clone, Copy, Debug)]
pub struct HeightEnclosure {
    pub height: [f64; 2],
    pub plane: HeightPlane,
    pub residual: [f64; 2],
}

impl HeightEnclosure {
    pub fn from_bounds(height: [f64; 2]) -> Self {
        Self {
            height,
            plane: HeightPlane::horizontal(0.0),
            residual: height,
        }
    }

    pub fn is_valid(self, rect: [f64; 4]) -> bool {
        let ordered = |bounds: [f64; 2]| {
            bounds[0].is_finite() && bounds[1].is_finite() && bounds[0] <= bounds[1]
        };
        if !self.plane.is_finite() || !ordered(self.height) || !ordered(self.residual) {
            return false;
        }
        let projected = bounds_add(self.plane.height_bounds(rect), self.residual);
        ordered(projected) && self.height[0] <= projected[1] && self.height[1] >= projected[0]
    }

    pub fn add_delta(self, delta: [f64; 2]) -> Self {
        let height = bounds_add(self.height, delta);
        let residual = bounds_add(self.residual, delta);
        let pad = 64.0 * f64::EPSILON * height[0].abs().max(height[1].abs()).max(1.0);
        Self {
            height,
            plane: self.plane,
            residual: [residual[0] - pad, residual[1] + pad],
        }
    }

    pub fn add_affine(self, delta: Self, rect: [f64; 4]) -> Self {
        let origin = delta.plane.origin;
        let plane = HeightPlane {
            origin,
            height: self.plane.height_at(origin[0], origin[1]) + delta.plane.height,
            gradient: [
                self.plane.gradient[0] + delta.plane.gradient[0],
                self.plane.gradient[1] + delta.plane.gradient[1],
            ],
        };
        let height = bounds_add(self.height, delta.height);
        let residual = bounds_add(self.residual, delta.residual);
        let magnitude = self
            .plane
            .height_bounds(rect)
            .into_iter()
            .chain(delta.plane.height_bounds(rect))
            .chain(height)
            .map(f64::abs)
            .fold(1.0, f64::max);
        let pad = 128.0 * f64::EPSILON * magnitude;
        Self {
            height,
            plane,
            residual: [residual[0] - pad, residual[1] + pad],
        }
    }
}

pub(crate) fn bounds_add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [(a[0] + b[0]).next_down(), (a[1] + b[1]).next_up()]
}

pub(crate) fn bounds_mul(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let values = [a[0] * b[0], a[0] * b[1], a[1] * b[0], a[1] * b[1]];
    [
        values.into_iter().fold(f64::INFINITY, f64::min).next_down(),
        values
            .into_iter()
            .fold(f64::NEG_INFINITY, f64::max)
            .next_up(),
    ]
}

pub(crate) fn bounds_scale(a: [f64; 2], factor: f64) -> [f64; 2] {
    bounds_mul(a, [factor; 2])
}

/// Authoritative elevation over the XZ plane. Implementations must be pure and
/// deterministic: equal inputs always yield equal outputs, on every platform.
pub trait HeightSource: Send + Sync {
    /// Elevation (world Y, metres) at world `(x, z)`.
    fn height_at(&self, x: f64, z: f64) -> f64;

    /// Surface normal via central differences over `eps` metres. Default impl
    /// works for any source; override if an analytic gradient is available.
    fn normal_at(&self, x: f64, z: f64, eps: f64) -> [f64; 3] {
        let hx = self.height_at(x + eps, z) - self.height_at(x - eps, z);
        let hz = self.height_at(x, z + eps) - self.height_at(x, z - eps);
        // gradient of height field → normal (−dY/dx, 1, −dY/dz), normalised
        let nx = -hx / (2.0 * eps);
        let nz = -hz / (2.0 * eps);
        let len = (nx * nx + 1.0 + nz * nz).sqrt();
        [nx / len, 1.0 / len, nz / len]
    }

    /// Slope angle (radians) from vertical — 0 = flat, π/2 = cliff. Drives the
    /// hazard / traversability layers.
    fn slope_at(&self, x: f64, z: f64, eps: f64) -> f64 {
        let n = self.normal_at(x, z, eps);
        n[1].clamp(-1.0, 1.0).acos()
    }
}

/// Compute a finite-difference normal without sampling outside a finite
/// square source footprint.  At an edge this becomes a genuine one-sided
/// derivative; it must not be approximated by sampling the source's clamped
/// value outside the DEM, which halves the measured slope.
pub fn normal_at_bounded(
    source: &dyn HeightSource,
    x: f64,
    z: f64,
    eps: f64,
    half_extent: f64,
) -> [f64; 3] {
    let half = half_extent.max(0.0);
    let eps = eps.abs().max(f64::EPSILON);
    let x = x.clamp(-half, half);
    let z = z.clamp(-half, half);
    let xl = (x - eps).max(-half);
    let xr = (x + eps).min(half);
    let zl = (z - eps).max(-half);
    let zr = (z + eps).min(half);
    let dx = xr - xl;
    let dz = zr - zl;
    let gx = if dx > 0.0 {
        (source.height_at(xr, z) - source.height_at(xl, z)) / dx
    } else {
        0.0
    };
    let gz = if dz > 0.0 {
        (source.height_at(x, zr) - source.height_at(x, zl)) / dz
    } else {
        0.0
    };
    let len = (gx * gx + 1.0 + gz * gz).sqrt();
    [-gx / len, 1.0 / len, -gz / len]
}

/// Piecewise-linear height on the measured posting line of a square raster.
///
/// The collar inner ring and local terrain boundary evaluate this same function.
/// Tile edges may intersect between raster postings; linear interpolation keeps
/// those additional vertices on the same rendered boundary as the native ring.
/// `half_extent` and posting positions follow the raster's `f32` geometry
/// contract, while sampled heights remain `f64`.
pub fn square_boundary_height_at(
    source: &dyn HeightSource,
    x: f64,
    z: f64,
    half_extent: f64,
    resolution: usize,
) -> Option<f64> {
    let half = half_extent.abs();
    let tolerance = (half * 1.0e-9).max(1.0e-6);
    let x_edge = (x.abs() - half).abs() <= tolerance;
    let z_edge = (z.abs() - half).abs() <= tolerance;
    if !x_edge && !z_edge {
        return None;
    }

    // At a corner either edge gives the same authored endpoint. Prefer X so
    // all consumers resolve corners in one deterministic direction.
    let (along, along_is_z) = if x_edge { (z, true) } else { (x, false) };
    let (lower, upper, t) = square_boundary_sample_interval(along, half_extent, resolution)?;
    let sample = |index: usize| {
        let coordinate = square_boundary_sample_coordinate(index, resolution, half_extent)?;
        if along_is_z {
            Some(source.height_at(x.signum() * half_extent, coordinate))
        } else {
            Some(source.height_at(coordinate, z.signum() * half_extent))
        }
    };
    let low = sample(lower)?;
    Some(low + (sample(upper)? - low) * t)
}

/// Posting indices and interpolation weight on the raster's render lattice.
/// Shared by measured boundary heights and immutable perimeter signal sampling.
pub fn square_boundary_sample_interval(
    along: f64,
    half_extent: f64,
    resolution: usize,
) -> Option<(usize, usize, f64)> {
    if !along.is_finite() {
        return None;
    }
    let spacing = square_boundary_posting_spacing(half_extent, resolution)? as f32;
    let half = half_extent as f32;
    let posting = ((along as f32 + half) / spacing).clamp(0.0, resolution as f32 - 1.0);
    let lower = (posting.floor() as usize).min(resolution - 1);
    Some((
        lower,
        (lower + 1).min(resolution - 1),
        f64::from(posting - lower as f32),
    ))
}

/// Physical spacing between the authored postings of a square raster edge.
/// The calculation follows `HeightGrid`'s `f32` geometry contract so every
/// boundary consumer uses the same sample coordinates and normal stencil.
pub fn square_boundary_posting_spacing(half_extent: f64, resolution: usize) -> Option<f64> {
    if resolution < 2 || !half_extent.is_finite() || half_extent <= 0.0 {
        return None;
    }
    let half = half_extent as f32;
    let spacing = (2.0_f32 * half) / (resolution as f32 - 1.0);
    (spacing.is_finite() && spacing > 0.0).then_some(spacing as f64)
}

/// Position of one square raster boundary posting, using the raster's stored
/// single-precision extent and spacing exactly as `HeightGrid` does.
pub fn square_boundary_sample_coordinate(
    index: usize,
    resolution: usize,
    half_extent: f64,
) -> Option<f64> {
    if index >= resolution {
        return None;
    }
    square_boundary_posting_spacing(half_extent, resolution)?;
    let half = half_extent as f32;
    let spacing = (2.0_f32 * half) / (resolution as f32 - 1.0);
    Some((-half + index as f32 * spacing) as f64)
}

/// A finite-square view of a [`HeightSource`].  It owns the boundary contract:
/// height samples are clamped to the measured footprint and derivatives use
/// one-sided differences there.  Consumers such as field rasters can therefore
/// safely use their normal stencil at an authored DEM edge without inventing an
/// outside surface.
pub struct BoundedHeightSource<'a> {
    source: &'a dyn HeightSource,
    half_extent: f64,
}

impl<'a> BoundedHeightSource<'a> {
    pub fn new(source: &'a dyn HeightSource, half_extent: f64) -> Self {
        Self {
            source,
            half_extent: half_extent.max(0.0),
        }
    }
}

impl HeightSource for BoundedHeightSource<'_> {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        self.source.height_at(
            x.clamp(-self.half_extent, self.half_extent),
            z.clamp(-self.half_extent, self.half_extent),
        )
    }

    fn normal_at(&self, x: f64, z: f64, eps: f64) -> [f64; 3] {
        normal_at_bounded(self.source, x, z, eps, self.half_extent)
    }
}

/// Deterministic analytic source: multi-octave hash value-noise FBM. Pure (no
/// RNG state), continuous, and identical across native + wasm. For bring-up and
/// tests only — replace with a DEM-backed source for real terrain.
#[derive(Debug, Clone, Copy)]
pub struct AnalyticHeightSource {
    pub seed: u64,
    /// Vertical amplitude of the coarsest octave (metres).
    pub amplitude_m: f64,
    /// World metres per coarsest-octave noise cell.
    pub feature_size_m: f64,
    pub octaves: u32,
}

impl Default for AnalyticHeightSource {
    fn default() -> Self {
        Self {
            seed: 0xC0FFEE,
            amplitude_m: 40.0,
            feature_size_m: 256.0,
            octaves: 5,
        }
    }
}

impl AnalyticHeightSource {
    pub fn new(seed: u64, amplitude_m: f64, feature_size_m: f64, octaves: u32) -> Self {
        Self {
            seed,
            amplitude_m,
            feature_size_m,
            octaves,
        }
    }
}

impl HeightSource for AnalyticHeightSource {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        let mut freq = 1.0 / self.feature_size_m.max(1e-6);
        let mut amp = self.amplitude_m;
        let mut h = 0.0;
        for o in 0..self.octaves {
            h += amp
                * vnoise(
                    x * freq,
                    z * freq,
                    self.seed ^ (o as u64).wrapping_mul(0x9E37_79B9),
                );
            freq *= 2.0; // lacunarity 2
            amp *= 0.5; // gain 0.5
        }
        h
    }
}

/// A reusable blend between a `site` source in a georeferenced square `region`
/// (XZ, metres) and a caller-supplied `globe` source. The smooth crossover
/// begins at the region boundary and has width `blend_m`.
///
/// The georef (lat/lon → this XZ `region`) and datum alignment are the caller's
/// job: `site` and `globe` must report heights in the **same vertical datum**
/// (e.g. metres above the body reference radius) for the blend to be meaningful.
/// Pure + deterministic like every `HeightSource`, so derived data stays
/// content-addressable and the bridge needs no per-peer transfer.
#[derive(Debug, Clone, Copy)]
pub struct CompositeHeightSource<S, G> {
    /// High-detail source (e.g. a site DEM) applied inside `region`.
    pub site: S,
    /// Coarse fallback source (e.g. the planetary globe) applied outside.
    pub globe: G,
    /// The georeferenced region (XZ, metres) where `site` applies.
    pub region: Square,
    /// Width (metres) of the site-to-globe blend outside `region`. `0` = a
    /// hard switch at the region boundary.
    pub blend_m: f64,
}

impl<S, G> CompositeHeightSource<S, G> {
    pub fn new(site: S, globe: G, region: Square, blend_m: f64) -> Self {
        Self {
            site,
            globe,
            region,
            blend_m: blend_m.max(0.0),
        }
    }
}

impl<S: HeightSource, G: HeightSource> HeightSource for CompositeHeightSource<S, G> {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        // Distance from the region: 0 inside/on-edge, growing outward.
        let d = self.region.distance_to([x, z]);
        if d <= 0.0 {
            return self.site.height_at(x, z); // fully inside → site
        }
        if self.blend_m <= 0.0 || d >= self.blend_m {
            return self.globe.height_at(x, z); // beyond the crossover → globe
        }
        // Crossover collar: smoothstep site→globe (C1 at both ends, so the seam is
        // continuous in value and slope).
        let t = smoothstep((d / self.blend_m).clamp(0.0, 1.0));
        lerp(self.site.height_at(x, z), self.globe.height_at(x, z), t)
    }
}

/// 2D value noise in `[-1, 1]`, bilinearly interpolated over an integer lattice
/// with a smoothstep fade. Pure function of `(x, z, seed)`. Shared with the
/// over-zoom micro-relief synthesiser.
pub(crate) fn vnoise(x: f64, z: f64, seed: u64) -> f64 {
    let xi = x.floor();
    let zi = z.floor();
    let xf = x - xi;
    let zf = z - zi;
    let (ix, iz) = (xi as i64, zi as i64);

    let v00 = hash01(ix, iz, seed);
    let v10 = hash01(ix + 1, iz, seed);
    let v01 = hash01(ix, iz + 1, seed);
    let v11 = hash01(ix + 1, iz + 1, seed);

    let u = smoothstep(xf);
    let w = smoothstep(zf);
    let a = lerp(v00, v10, u);
    let b = lerp(v01, v11, u);
    lerp(a, b, w) * 2.0 - 1.0 // [0,1] → [-1,1]
}

/// Within a lattice cell, value noise is bilinear in two independent monotone
/// smoothstep coordinates. Rectangle extrema therefore occur at corners or
/// lattice cuts. Small footprints need at most nine source evaluations; larger
/// footprints retain the source's global range without enumerating cells.
enum NoiseRectangle {
    Global,
    Local {
        xs: [f64; 3],
        nx: usize,
        zs: [f64; 3],
        nz: usize,
    },
}

fn noise_rectangle(rect: [f64; 4]) -> Option<NoiseRectangle> {
    if rect.iter().any(|value| !value.is_finite())
        || rect[0] > rect[2]
        || rect[1] > rect[3]
        || rect
            .iter()
            .any(|value| value.floor() < i64::MIN as f64 || value.floor() >= i64::MAX as f64)
    {
        return None;
    }
    if rect[2].floor() - rect[0].floor() > 1.0 || rect[3].floor() - rect[1].floor() > 1.0 {
        return Some(NoiseRectangle::Global);
    }
    let cuts = |low: f64, high: f64| {
        let boundary = low.floor() + 1.0;
        if low == high {
            ([low; 3], 1)
        } else if boundary > low && boundary < high {
            ([low, boundary, high], 3)
        } else {
            ([low, high, high], 2)
        }
    };
    let (xs, nx) = cuts(rect[0], rect[2]);
    let (zs, nz) = cuts(rect[1], rect[3]);
    Some(NoiseRectangle::Local { xs, nx, zs, nz })
}

pub(crate) fn vnoise_bounds(rect: [f64; 4], seed: u64) -> Option<[f64; 2]> {
    let NoiseRectangle::Local { xs, nx, zs, nz } = noise_rectangle(rect)? else {
        return Some([-1.0, 1.0]);
    };
    let mut bounds = [f64::INFINITY, f64::NEG_INFINITY];
    for &x in &xs[..nx] {
        for &z in &zs[..nz] {
            let value = vnoise(x, z, seed);
            bounds[0] = bounds[0].min(value);
            bounds[1] = bounds[1].max(value);
        }
    }
    let pad = 64.0 * f64::EPSILON;
    Some([(bounds[0] - pad).max(-1.0), (bounds[1] + pad).min(1.0)])
}

/// Exact smoothstep derivative intervals inside each admitted noise cell.
/// The two partials factor into independent one-dimensional intervals.
pub(crate) fn vnoise_gradient_bounds(rect: [f64; 4], seed: u64) -> Option<[[f64; 2]; 2]> {
    let NoiseRectangle::Local { xs, nx, zs, nz } = noise_rectangle(rect)? else {
        return Some([[-3.0, 3.0]; 2]);
    };
    let smooth_bounds = |range: [f64; 2]| {
        let pad = 32.0 * f64::EPSILON;
        [
            (smoothstep(range[0]) - pad).max(0.0),
            (smoothstep(range[1]) + pad).min(1.0),
        ]
    };
    let derivative_bounds = |range: [f64; 2]| {
        let derivative = |t: f64| 6.0 * t * (1.0 - t);
        let a = derivative(range[0]);
        let b = derivative(range[1]);
        let high = if range[0] <= 0.5 && range[1] >= 0.5 {
            1.5
        } else {
            a.max(b)
        };
        let pad = 32.0 * f64::EPSILON;
        [(a.min(b) - pad).max(0.0), (high + pad).min(1.5)]
    };
    let mut result = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    for ix in 0..nx.saturating_sub(1).max(1) {
        for iz in 0..nz.saturating_sub(1).max(1) {
            let x = [xs[ix], xs[(ix + 1).min(nx - 1)]];
            let z = [zs[iz], zs[(iz + 1).min(nz - 1)]];
            let cell_x = x[0].floor();
            let cell_z = z[0].floor();
            let u = smooth_bounds([x[0] - cell_x, x[1] - cell_x]);
            let w = smooth_bounds([z[0] - cell_z, z[1] - cell_z]);
            let du = derivative_bounds([x[0] - cell_x, x[1] - cell_x]);
            let dw = derivative_bounds([z[0] - cell_z, z[1] - cell_z]);
            let (cx, cz) = (cell_x as i64, cell_z as i64);
            let h00 = hash01(cx, cz, seed);
            let h10 = hash01(cx + 1, cz, seed);
            let h01 = hash01(cx, cz + 1, seed);
            let h11 = hash01(cx + 1, cz + 1, seed);
            let along_z = bounds_add([h10 - h00; 2], bounds_scale(w, (h11 - h01) - (h10 - h00)));
            let along_x = bounds_add([h01 - h00; 2], bounds_scale(u, (h11 - h10) - (h01 - h00)));
            let gradient = [
                bounds_scale(bounds_mul(du, along_z), 2.0),
                bounds_scale(bounds_mul(dw, along_x), 2.0),
            ];
            for axis in 0..2 {
                result[axis][0] = result[axis][0].min(gradient[axis][0]);
                result[axis][1] = result[axis][1].max(gradient[axis][1]);
            }
        }
    }
    for axis in &mut result {
        let pad = 128.0 * f64::EPSILON * axis[0].abs().max(axis[1].abs()).max(1.0);
        axis[0] -= pad;
        axis[1] += pad;
    }
    Some(result)
}

/// Deterministic lattice hash → `[0, 1)`. SplitMix64-style avalanche on the
/// mixed cell coordinates; no platform-dependent float ops in the hash itself.
/// Shared with the over-zoom craterlet generator.
pub(crate) fn hash01(x: i64, z: i64, seed: u64) -> f64 {
    let mut h = seed
        ^ (x as u64).wrapping_mul(0xA0761D_6478BD642F)
        ^ (z as u64).wrapping_mul(0xE7037E_D1A0B428DB);
    h ^= h >> 32;
    h = h.wrapping_mul(0xD6E8FE_B86659FD93);
    h ^= h >> 32;
    h = h.wrapping_mul(0xD6E8FE_B86659FD93);
    h ^= h >> 32;
    // top 53 bits → f64 in [0,1)
    (h >> 11) as f64 / (1u64 << 53) as f64
}

fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_enclosure_validates_finite_ordered_and_consistent_facts() {
        let rect = [-1.0, -1.0, 1.0, 1.0];
        assert!(HeightEnclosure::from_bounds([-2.0, 3.0]).is_valid(rect));
        assert!(!HeightEnclosure::from_bounds([3.0, -2.0]).is_valid(rect));
        assert!(!HeightEnclosure::from_bounds([f64::NAN, 3.0]).is_valid(rect));
        assert!(
            !HeightEnclosure {
                height: [1.0, 2.0],
                plane: HeightPlane::horizontal(0.0),
                residual: [0.0; 2]
            }
            .is_valid(rect)
        );
        assert!(
            !HeightEnclosure {
                height: [0.0; 2],
                plane: HeightPlane::horizontal(f64::INFINITY),
                residual: [0.0; 2]
            }
            .is_valid(rect)
        );
    }

    #[test]
    fn noise_gradient_intervals_cover_cell_interiors_crossings_and_narrow_rectangles() {
        for seed in [0, 424242, 0x9E37_79B9_7F4A_7C15] {
            for rect in [
                [0.13, 0.17, 0.91, 0.96],
                [0.8, 0.9, 1.2, 1.3],
                [-2.0, -3.0, 2.0, 4.0],
            ] {
                let bounds = vnoise_gradient_bounds(rect, seed).unwrap();
                for iz in 0..9 {
                    for ix in 0..9 {
                        let x = rect[0] + (rect[2] - rect[0]) * (ix as f64 + 0.37) / 9.0;
                        let z = rect[1] + (rect[3] - rect[1]) * (iz as f64 + 0.37) / 9.0;
                        let eps = 1e-6;
                        let measured = [
                            (vnoise(x + eps, z, seed) - vnoise(x - eps, z, seed)) / (2.0 * eps),
                            (vnoise(x, z + eps, seed) - vnoise(x, z - eps, seed)) / (2.0 * eps),
                        ];
                        for axis in 0..2 {
                            assert!(
                                measured[axis] >= bounds[axis][0] - 1e-8
                                    && measured[axis] <= bounds[axis][1] + 1e-8,
                                "{rect:?} axis{axis}: {} outside {:?}",
                                measured[axis],
                                bounds[axis]
                            );
                        }
                    }
                }
            }
            let narrow = vnoise_gradient_bounds([0.13, 0.17, 0.13001, 0.17001], seed).unwrap();
            assert!(narrow.into_iter().all(|b| b[1] - b[0] < 0.001));
            let origin = vnoise_gradient_bounds([0.0; 4], seed).unwrap();
            assert!(
                origin
                    .into_iter()
                    .all(|b| b[0] <= 0.0 && b[1] >= 0.0 && b[1] - b[0] < 1e-10)
            );
        }
    }

    #[test]
    fn value_noise_rectangles_cover_lattice_crossings_and_retain_corner_extrema() {
        for seed in [0, 424242, 0x9E37_79B9_7F4A_7C15] {
            for rect in [
                [0.13, 0.17, 0.91, 0.96],
                [0.83, 0.91, 1.21, 1.14],
                [-1.2, -0.13, -0.8, 0.27],
                [-2.0, -3.0, 2.0, 4.0],
                [0.0; 4],
            ] {
                let bounds = vnoise_bounds(rect, seed).unwrap();
                for z in 0..=24 {
                    for x in 0..=24 {
                        let wx = rect[0] + (rect[2] - rect[0]) * x as f64 / 24.0;
                        let wz = rect[1] + (rect[3] - rect[1]) * z as f64 / 24.0;
                        let value = vnoise(wx, wz, seed);
                        assert!(
                            value >= bounds[0] && value <= bounds[1],
                            "{rect:?}: {value} outside {bounds:?}"
                        );
                    }
                }
            }
            let rect = [0.13, 0.17, 0.91, 0.96];
            let bounds = vnoise_bounds(rect, seed).unwrap();
            let corners = [
                vnoise(rect[0], rect[1], seed),
                vnoise(rect[0], rect[3], seed),
                vnoise(rect[2], rect[1], seed),
                vnoise(rect[2], rect[3], seed),
            ];
            assert!((bounds[0] - corners.into_iter().fold(f64::INFINITY, f64::min)).abs() < 1e-12);
            assert!(
                (bounds[1] - corners.into_iter().fold(f64::NEG_INFINITY, f64::max)).abs() < 1e-12
            );
        }
        assert_eq!(vnoise_bounds([-2.0, -3.0, 2.0, 4.0], 0), Some([-1.0, 1.0]));
        assert!(vnoise_bounds([f64::NAN, 0.0, 1.0, 1.0], 0).is_none());
        assert!(vnoise_bounds([1.0, 0.0, 0.0, 1.0], 0).is_none());
    }

    struct Ramp;
    impl HeightSource for Ramp {
        fn height_at(&self, x: f64, _z: f64) -> f64 {
            2.0 * x
        }
    }

    #[test]
    fn square_boundary_preserves_both_axes_on_every_edge() {
        struct Plane;
        impl HeightSource for Plane {
            fn height_at(&self, x: f64, z: f64) -> f64 {
                2.0 * x + 3.0 * z
            }
        }
        for along in [-10.0, -7.5, -5.0, -2.5, 0.0, 2.5, 5.0, 7.5, 10.0] {
            for (x, z) in [(-10.0, along), (10.0, along), (along, -10.0), (along, 10.0)] {
                let sampled = square_boundary_height_at(&Plane, x, z, 10.0, 5).unwrap();
                assert!(
                    (sampled - Plane.height_at(x, z)).abs() < 1e-12,
                    "boundary ({x}, {z}) sampled {sampled}"
                );
            }
        }
    }

    #[test]
    fn deterministic() {
        let s = AnalyticHeightSource::default();
        assert_eq!(s.height_at(12.3, -45.6), s.height_at(12.3, -45.6));
        // a different source (seed) generally differs
        let s2 = AnalyticHeightSource { seed: 1, ..s };
        assert_ne!(s.height_at(12.3, -45.6), s2.height_at(12.3, -45.6));
    }

    #[test]
    fn bounded_by_amplitude_sum() {
        // FBM magnitude ≤ amp * (1 + 1/2 + 1/4 + ...) < 2 * amp
        let s = AnalyticHeightSource::default();
        let bound = 2.0 * s.amplitude_m;
        for i in -50..50 {
            let p = i as f64 * 7.13;
            assert!(s.height_at(p, -p).abs() < bound);
        }
    }

    #[test]
    fn bounded_normal_is_one_sided_at_the_edge() {
        let n = normal_at_bounded(&Ramp, 10.0, 0.0, 1.0, 10.0);
        assert!(((-n[0] / n[1]) - 2.0).abs() < 1e-12, "normal {n:?}");
        let n = normal_at_bounded(&Ramp, -10.0, 0.0, 1.0, 10.0);
        assert!(((-n[0] / n[1]) - 2.0).abs() < 1e-12, "normal {n:?}");
    }

    #[test]
    fn bounded_source_clamps_height_but_preserves_edge_slope() {
        let src = BoundedHeightSource::new(&Ramp, 10.0);
        assert_eq!(src.height_at(100.0, 0.0), 20.0);
        let n = src.normal_at(10.0, 0.0, 1.0);
        assert!(((-n[0] / n[1]) - 2.0).abs() < 1e-12, "normal {n:?}");
    }

    #[test]
    fn continuous_no_lattice_cracks() {
        // height is continuous: tiny position deltas → tiny height deltas, even
        // across integer lattice lines (the classic value-noise seam check).
        let s = AnalyticHeightSource::new(7, 10.0, 4.0, 4);
        let eps = 1e-4;
        for k in -20..20 {
            let x = k as f64 * 4.0; // land exactly on lattice boundaries
            let d = (s.height_at(x + eps, 1.0) - s.height_at(x - eps, 1.0)).abs();
            assert!(d < 1.0, "discontinuity {d} at x={x}");
        }
    }

    #[test]
    fn flat_source_has_up_normal() {
        // zero amplitude → flat → normal is +Y, slope 0
        let s = AnalyticHeightSource::new(0, 0.0, 100.0, 4);
        let n = s.normal_at(3.0, 4.0, 0.5);
        assert!((n[0]).abs() < 1e-9 && (n[2]).abs() < 1e-9 && (n[1] - 1.0).abs() < 1e-9);
        assert!(s.slope_at(3.0, 4.0, 0.5).abs() < 1e-6);
    }

    /// Constant-height source for composite tests (distinct site vs globe datums).
    struct Flat(f64);
    impl HeightSource for Flat {
        fn height_at(&self, _x: f64, _z: f64) -> f64 {
            self.0
        }
    }

    fn composite() -> CompositeHeightSource<Flat, Flat> {
        // site = 100 m inside a 200 m square at origin; globe = 0 m; 50 m collar.
        CompositeHeightSource::new(
            Flat(100.0),
            Flat(0.0),
            Square {
                center: [0.0, 0.0],
                half: 100.0,
            },
            50.0,
        )
    }

    #[test]
    fn composite_site_inside_globe_outside() {
        let c = composite();
        // Deep inside → pure site.
        assert_eq!(c.height_at(0.0, 0.0), 100.0);
        assert_eq!(c.height_at(50.0, -30.0), 100.0);
        // On the edge counts as inside (distance 0).
        assert_eq!(c.height_at(100.0, 0.0), 100.0);
        // Beyond the collar (edge 100 + blend 50 = 150) → pure globe.
        assert_eq!(c.height_at(200.0, 0.0), 0.0);
        assert_eq!(c.height_at(0.0, -300.0), 0.0);
    }

    #[test]
    fn composite_blends_monotonically_in_collar() {
        let c = composite();
        // Midway through the collar (edge 100 + 25 = x 125) → strictly between.
        let mid = c.height_at(125.0, 0.0);
        assert!(
            mid > 0.0 && mid < 100.0,
            "collar midpoint {mid} not between"
        );
        // Monotone decreasing site→globe across the collar.
        let mut prev = f64::INFINITY;
        for k in 0..=10 {
            let x = 100.0 + k as f64 * 5.0; // edge … edge+50
            let h = c.height_at(x, 0.0);
            assert!(h <= prev + 1e-9, "non-monotone at x={x}: {h} > {prev}");
            prev = h;
        }
    }

    #[test]
    fn composite_continuous_across_seams() {
        let c = composite();
        let eps = 1e-3;
        // Continuous at the inner edge (x=100) and outer edge of collar (x=150).
        for &edge in &[100.0_f64, 150.0] {
            let d = (c.height_at(edge + eps, 0.0) - c.height_at(edge - eps, 0.0)).abs();
            assert!(d < 1.0, "discontinuity {d} at seam x={edge}");
        }
    }

    #[test]
    fn composite_zero_blend_is_hard_switch() {
        let c = CompositeHeightSource::new(
            Flat(100.0),
            Flat(0.0),
            Square {
                center: [0.0, 0.0],
                half: 100.0,
            },
            0.0,
        );
        assert_eq!(c.height_at(100.0, 0.0), 100.0); // inside/edge
        assert_eq!(c.height_at(100.1, 0.0), 0.0); // just outside → globe
    }
}
