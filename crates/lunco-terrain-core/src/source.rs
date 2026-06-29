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
        Self { seed, amplitude_m, feature_size_m, octaves }
    }
}

impl HeightSource for AnalyticHeightSource {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        let mut freq = 1.0 / self.feature_size_m.max(1e-6);
        let mut amp = self.amplitude_m;
        let mut h = 0.0;
        for o in 0..self.octaves {
            h += amp * vnoise(x * freq, z * freq, self.seed ^ (o as u64).wrapping_mul(0x9E37_79B9));
            freq *= 2.0; // lacunarity 2
            amp *= 0.5; // gain 0.5
        }
        h
    }
}

/// 2D value noise in `[-1, 1]`, bilinearly interpolated over an integer lattice
/// with a smoothstep fade. Pure function of `(x, z, seed)`.
fn vnoise(x: f64, z: f64, seed: u64) -> f64 {
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
fn hash01(x: i64, z: i64, seed: u64) -> f64 {
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
}
