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
//! [`CompositeHeightSource`] is the pure heart of the **orbit→surface bridge**: a
//! high-detail site source inside a georeferenced region, a coarse globe source
//! outside, blended at the edge — one continuous surface from orbit to the ground.

use crate::quadtree::Square;

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

/// The pure core of the **orbit→surface bridge**. Returns the high-detail `site`
/// source inside a georeferenced square `region` (XZ, metres), the coarse `globe`
/// source outside it, with a smooth crossover collar of width `blend_m` just
/// outside the region edge. Descending from orbit to a site, the streamer samples
/// ONE continuous height field — DEM detail in close, planetary elevation out
/// wide — instead of two disjoint terrains.
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
    /// Width (metres) of the blend collar just outside the region edge. `0` = a
    /// hard switch at the boundary.
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
            return self.globe.height_at(x, z); // beyond the collar → globe
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
