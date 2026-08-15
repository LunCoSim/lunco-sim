//! Derived data layers — slope / normal / ambient-occlusion / surface-pack
//! rasters computed as **pure functions of a [`HeightSource`]** over a region.
//!
//! These are the engine-neutral half of the layered terrain pipeline (design
//! `docs/architecture/terrain-layered-rendering.md` Part C.2 / tracker P3b). The Bevy
//! layer turns the returned buffers into `Image`s and binds them to the
//! `terrain_layered.wgsl` material's `surface_map` (binding 6/7, packed
//! R=roughness G=AO B=rockDens A=hazard) and `normal_map` (binding 8/9) slots —
//! but the math lives here so it stays:
//!
//! - **pure + deterministic** → derived maps are content-addressable (`hash(source
//!   id, region, resolution)`) and re-derivable on any peer, so networking
//!   transfers nothing (same property the height field already has);
//! - **wasm-safe** (std + the trait only, no render deps);
//! - **unit-testable** without a GPU or an `App`.
//!
//! All buffers are **row-major `res × res`**, texel-centred: texel `(ix, iz)` is
//! sampled at UV `((ix+0.5)/res, (iz+0.5)/res)` across the region, matching how a
//! linearly-filtered texture is read by the planar-UV terrain shader.

use crate::quadtree::Square;
use crate::source::{normal_at_bounded, HeightSource};

/// World XZ of texel `(ix, iz)` at the centre of its cell in a `res×res` raster
/// over `region`.
#[inline]
fn texel_world(region: &Square, res: usize, ix: usize, iz: usize) -> (f64, f64) {
    let size = 2.0 * region.half;
    let min_x = region.center[0] - region.half;
    let min_z = region.center[1] - region.half;
    let u = (ix as f64 + 0.5) / res as f64;
    let v = (iz as f64 + 0.5) / res as f64;
    (min_x + u * size, min_z + v * size)
}

/// Central-difference step (metres) for one texel — the raster's cell size.
#[inline]
fn texel_eps(region: &Square, res: usize) -> f64 {
    (2.0 * region.half) / res as f64
}

/// World-space surface normals over `region`, row-major `res×res`. Each is the
/// unit gradient normal `(−dY/dx, 1, −dY/dz)` from the source.
pub fn normal_map<S: HeightSource>(src: &S, region: &Square, res: usize) -> Vec<[f32; 3]> {
    let res = res.max(1);
    let eps = texel_eps(region, res);
    let mut out = Vec::with_capacity(res * res);
    for iz in 0..res {
        for ix in 0..res {
            let (x, z) = texel_world(region, res, ix, iz);
            let n = src.normal_at(x, z, eps);
            out.push([n[0] as f32, n[1] as f32, n[2] as f32]);
        }
    }
    out
}

/// [`normal_map`] and [`slope_map`] in ONE derive pass, row-major `res×res`.
/// Slope is `acos(n.y)` of the very normal already computed — exactly what
/// `HeightSource::slope_at` does internally — so running the two maps as
/// separate passes sampled every central difference twice. Bit-identical to
/// the standalone maps (same f64 ops in the same order); prefer this wherever
/// both maps are needed (the surface-pack bake in `derived_layers`).
pub fn normal_slope_maps<S: HeightSource>(
    src: &S,
    region: &Square,
    res: usize,
) -> (Vec<[f32; 3]>, Vec<f32>) {
    let res = res.max(1);
    let eps = texel_eps(region, res);
    let mut normals = Vec::with_capacity(res * res);
    let mut slopes = Vec::with_capacity(res * res);
    for iz in 0..res {
        for ix in 0..res {
            let (x, z) = texel_world(region, res, ix, iz);
            let n = src.normal_at(x, z, eps);
            normals.push([n[0] as f32, n[1] as f32, n[2] as f32]);
            slopes.push(n[1].clamp(-1.0, 1.0).acos() as f32);
        }
    }
    (normals, slopes)
}

/// Slope angle from vertical (radians, `0` = flat) over `region`, row-major.
/// When the normal map is baked alongside (it always is in the surface-pack
/// bake), use [`normal_slope_maps`] — this standalone walk re-derives the same
/// central differences the normal pass just computed.
pub fn slope_map<S: HeightSource>(src: &S, region: &Square, res: usize) -> Vec<f32> {
    let res = res.max(1);
    let eps = texel_eps(region, res);
    let mut out = Vec::with_capacity(res * res);
    for iz in 0..res {
        for ix in 0..res {
            let (x, z) = texel_world(region, res, ix, iz);
            out.push(src.slope_at(x, z, eps) as f32);
        }
    }
    out
}

/// Horizon-based ambient occlusion in `[0, 1]` (`1` = fully open sky, `0` = fully
/// occluded) over `region`, row-major. For each texel it marches a few rays
/// outward, tracking the highest elevation angle the terrain rises to (the local
/// horizon), and returns `1 − mean(sin(horizon))`. Pure, deterministic, and
/// cheap (`dirs × steps` height samples per texel).
///
/// `radius_m` is how far each ray reaches; `dirs`/`steps` trade quality for cost.
pub fn ao_map<S: HeightSource>(
    src: &S,
    region: &Square,
    res: usize,
    radius_m: f64,
    dirs: usize,
    steps: usize,
    source_half_extent: f64,
) -> Vec<f32> {
    let res = res.max(1);
    let dirs = dirs.max(1);
    let steps = steps.max(1);
    let radius = radius_m.max(1e-3);
    let bounded = source_half_extent.is_finite();
    let source_half = source_half_extent.max(0.0);
    // Precompute ray directions evenly around the circle.
    let angles: Vec<(f64, f64)> = (0..dirs)
        .map(|d| {
            let a = std::f64::consts::TAU * (d as f64) / (dirs as f64);
            (a.cos(), a.sin())
        })
        .collect();

    let mut out = Vec::with_capacity(res * res);
    for iz in 0..res {
        for ix in 0..res {
            let (x, z) = texel_world(region, res, ix, iz);
            let h0 = src.height_at(x, z);
            let mut occ = 0.0f64;
            for &(dx, dz) in &angles {
                let mut max_sin = 0.0f64;
                for s in 1..=steps {
                    let dist = radius * (s as f64) / (steps as f64);
                    let sx = x + dx * dist;
                    let sz = z + dz * dist;
                    if bounded && (sx.abs() > source_half || sz.abs() > source_half) {
                        // The ray has left the finite authored terrain. Beyond
                        // that edge is open sky, not repeated edge elevation.
                        break;
                    }
                    let dh = src.height_at(sx, sz) - h0;
                    if dh > 0.0 {
                        // sin of the elevation angle to this sample.
                        let sin_e = dh / (dh * dh + dist * dist).sqrt();
                        if sin_e > max_sin {
                            max_sin = sin_e;
                        }
                    }
                }
                occ += max_sin;
            }
            let ao = 1.0 - occ / dirs as f64;
            out.push(ao.clamp(0.0, 1.0) as f32);
        }
    }
    out
}

/// Ray–terrain intersection over a [`HeightSource`] — the single-ray sibling of
/// [`ao_map`]'s horizon march. Marches `origin + t·dir` for `t ∈ (0, max]` in the
/// source's own frame (up = +Y) and returns the distance `t` where the ray first
/// dips `margin` below the surface, or `None` if it stays clear. Only the
/// `±half_extent` square around the local origin is treated as terrain; segments
/// outside it are open sky. `step` is the march spacing (source units, e.g. the
/// DEM sample pitch); a final bisection refines the crossing. Pure /
/// deterministic / wasm-safe — like every kernel here it takes only a
/// `HeightSource`, so line-of-sight is content-addressable and identical on
/// every peer.
pub fn los_hit<S: HeightSource>(
    src: &S,
    origin: [f64; 3],
    dir: [f64; 3],
    max: f64,
    half_extent: f64,
    step: f64,
    margin: f64,
) -> Option<f64> {
    let step = step.max(1e-3);
    let n = ((max / step).ceil() as i64).clamp(1, 8192);
    // (ray height, terrain height) at param `t`, or None outside the footprint.
    let sample = |t: f64| -> Option<(f64, f64)> {
        let x = origin[0] + dir[0] * t;
        let z = origin[2] + dir[2] * t;
        if x.abs() > half_extent || z.abs() > half_extent {
            return None;
        }
        Some((origin[1] + dir[1] * t, src.height_at(x, z)))
    };
    let mut prev_t = 0.0;
    for i in 1..=n {
        let t = ((i as f64) * step).min(max);
        if let Some((y, h)) = sample(t) {
            if y < h - margin {
                // Crossed below the surface in (prev_t, t]; bisect to refine.
                let (mut lo, mut hi) = (prev_t, t);
                for _ in 0..24 {
                    let mid = 0.5 * (lo + hi);
                    match sample(mid) {
                        Some((my, mh)) if my < mh => hi = mid,
                        _ => lo = mid,
                    }
                }
                return Some(hi);
            }
            prev_t = t;
        }
        if (t - max).abs() < f64::EPSILON {
            break;
        }
    }
    None
}

#[cfg(test)]
mod los_tests {
    use super::*;

    /// height = 0.1·x, flat in z (a constant slope) — the pure-kernel twin of
    /// `query.rs`'s `tilted_terrain`.
    struct Ramp;
    impl HeightSource for Ramp {
        fn height_at(&self, x: f64, _z: f64) -> f64 {
            0.1 * x
        }
    }

    #[test]
    fn ray_into_slope_hits_near_the_crossing() {
        let s = Ramp;
        // From (0,2,0) heading +x and down: y(x)=2−0.15x vs terrain 0.1x → the
        // ray sinks below the surface past x≈8.
        let (dx, dy) = (1.0_f64, -0.15_f64);
        let len = (dx * dx + dy * dy).sqrt();
        let t = los_hit(
            &s,
            [0.0, 2.0, 0.0],
            [dx / len, dy / len, 0.0],
            100.0,
            10.0,
            0.5,
            0.05,
        )
        .expect("ray should hit the slope");
        let hit_x = (dx / len) * t;
        assert!((hit_x - 8.0).abs() < 0.6, "hit x = {hit_x}");
    }

    #[test]
    fn ray_above_relief_and_outside_footprint_miss() {
        let s = Ramp;
        // Horizontal ray well above the highest terrain (0.1·10 = 1.0 at the edge).
        assert!(los_hit(
            &s,
            [-10.0, 100.0, 0.0],
            [1.0, 0.0, 0.0],
            20.0,
            10.0,
            0.5,
            0.05
        )
        .is_none());
        // Entirely outside the ±10 footprint → open sky, no hit.
        assert!(los_hit(
            &s,
            [200.0, 5.0, 0.0],
            [1.0, 0.0, 0.0],
            10.0,
            10.0,
            0.5,
            0.05
        )
        .is_none());
    }
}

/// Length scale (metres) mapping true curvature (`1/m`) to tonal contrast in
/// [`albedo_map`]: a bowl/rim of radius ≈ this reaches ~76 % of full darkening/
/// brightening (`tanh(1)`). A FIXED length — not the texel step — so the same
/// relief bakes the same tone at every tile size / LOD (the old per-`eps`
/// normalisation made curvature contrast scale with tile size → tonal seams
/// across LOD boundaries).
pub const CURVATURE_TONE_SCALE_M: f64 = 4.0;

/// Relief-correlated albedo scalar in `[0, 1]` (0.5 = neutral) over `region`,
/// row-major. Convex ground (crater rims, ejecta crests) reads slightly
/// brighter, concave ground (bowls, hollows) slightly darker, and steep faces
/// get a touch of mass-wasting brightness — the tonal variation that makes
/// distant relief legible even where geometry/shading detail has LOD'd away.
/// Curvature is the true central-difference Laplacian (`Δh / eps²`, units 1/m),
/// scaled by [`CURVATURE_TONE_SCALE_M`] and squashed through `tanh` so extreme
/// relief saturates instead of clipping.
///
/// `stencil_texels` widens the curvature stencil (in texels). A 1-texel
/// Laplacian on a source band-limited at 2 texels sits exactly AT Nyquist and
/// returns per-texel checker noise instead of curvature — rendered as a hard
/// mosaic of map texels at mid distance. Pair a stencil of `s` texels with a
/// source limited to wavelengths ≥ `2·s` texels; with the `eps²` normalisation
/// the response to SMOOTH curvature is width-independent by construction.
pub fn albedo_map<S: HeightSource>(
    src: &S,
    region: &Square,
    res: usize,
    stencil_texels: f64,
    source_half_extent: f64,
) -> Vec<f32> {
    let res = res.max(1);
    let stencil = stencil_texels.max(1.0);
    let eps = texel_eps(region, res) * stencil;
    let mut out = Vec::with_capacity(res * res);
    for iz in 0..res {
        for ix in 0..res {
            let (x, z) = texel_world(region, res, ix, iz);
            let lap = second_difference_bounded(src, x, z, eps, source_half_extent, true)
                + second_difference_bounded(src, x, z, eps, source_half_extent, false);
            // Concave (positive Laplacian) → darker; convex → brighter.
            let curve = (-lap * CURVATURE_TONE_SCALE_M).tanh() as f32;
            let slope = if source_half_extent.is_finite() {
                let n = normal_at_bounded(src, x, z, eps, source_half_extent);
                n[1].clamp(-1.0, 1.0).acos() as f32
            } else {
                src.slope_at(x, z, eps) as f32
            };
            let a = 0.5 + 0.30 * curve + 0.10 * (slope / 0.6).min(1.0);
            out.push(a.clamp(0.0, 1.0));
        }
    }
    out
}

/// Second derivative over a finite square. Central differences are used where
/// both sides are measured; at an edge a one-sided stencil is fitted entirely
/// inside the footprint. No sample is obtained by extending a DEM's clamped
/// edge beyond the authored surface.
fn second_difference_bounded<S: HeightSource>(
    src: &S,
    x: f64,
    z: f64,
    eps: f64,
    half_extent: f64,
    along_x: bool,
) -> f64 {
    let eps = eps.abs().max(f64::EPSILON);
    if !half_extent.is_finite() {
        let h0 = src.height_at(x, z);
        let hp = if along_x {
            src.height_at(x + eps, z)
        } else {
            src.height_at(x, z + eps)
        };
        let hm = if along_x {
            src.height_at(x - eps, z)
        } else {
            src.height_at(x, z - eps)
        };
        return (hp + hm - 2.0 * h0) / (eps * eps);
    }

    let half = half_extent.max(0.0);
    let p = if along_x { x } else { z };
    let forward_room = (half - p).max(0.0);
    let backward_room = (half + p).max(0.0);
    let sample = |offset: f64| {
        if along_x {
            src.height_at(x + offset, z)
        } else {
            src.height_at(x, z + offset)
        }
    };

    if forward_room >= eps && backward_room >= eps {
        return (sample(eps) + sample(-eps) - 2.0 * sample(0.0)) / (eps * eps);
    }

    // Use a two-step one-sided stencil, shrinking the step when the texel
    // centre is close to the boundary so both samples remain authored.
    if forward_room >= backward_room && forward_room > 0.0 {
        let step = eps.min(forward_room * 0.5);
        return (sample(2.0 * step) - 2.0 * sample(step) + sample(0.0)) / (step * step);
    }
    if backward_room > 0.0 {
        let step = eps.min(backward_room * 0.5);
        return (sample(0.0) - 2.0 * sample(-step) + sample(-2.0 * step)) / (step * step);
    }
    0.0
}

/// Bilinear upsample of a square scalar map from `src_res`² to `dst_res`².
/// Lets smooth-by-construction channels (AO) bake at reduced resolution —
/// quarter the hemisphere-march cost at half res — then expand to pack size.
///
/// Both maps are TEXEL-CENTRED over the same region (texel `i` samples
/// `(i+0.5)/res`, see [`texel_world`]), so the mapping is the texel-centred
/// `src = (dst + 0.5)·src_res/dst_res − 0.5` with edge clamping — a node-based
/// `(src_res−1)/(dst_res−1)` map here landed the upsampled channel half a
/// source texel off the full-res channels packed into the same RGBA8.
pub fn upsample_bilinear(src: &[f32], src_res: usize, dst_res: usize) -> Vec<f32> {
    assert_eq!(src.len(), src_res * src_res);
    if src_res == dst_res {
        return src.to_vec();
    }
    let mut out = Vec::with_capacity(dst_res * dst_res);
    let scale = src_res as f32 / dst_res as f32;
    // Texel-centred source coordinate for destination texel `i`, clamped so the
    // outermost half-texel band extends the edge value.
    let coord = |i: usize| -> (usize, usize, f32) {
        let f = ((i as f32 + 0.5) * scale - 0.5).clamp(0.0, (src_res - 1) as f32);
        let i0 = (f as usize).min(src_res - 1);
        let i1 = (i0 + 1).min(src_res - 1);
        (i0, i1, f - i0 as f32)
    };
    for iz in 0..dst_res {
        let (z0, z1, tz) = coord(iz);
        for ix in 0..dst_res {
            let (x0, x1, tx) = coord(ix);
            let top = src[z0 * src_res + x0] * (1.0 - tx) + src[z0 * src_res + x1] * tx;
            let bot = src[z1 * src_res + x0] * (1.0 - tx) + src[z1 * src_res + x1] * tx;
            out.push(top * (1.0 - tz) + bot * tz);
        }
    }
    out
}

/// Roughness in `[0, 1]` from slope: flat ground keeps a high regolith base
/// roughness, steeper faces read rougher. `base` at 0° rising to `1` near the
/// `steep_rad` slope.
#[inline]
pub fn roughness_from_slope(slope_rad: f32, base: f32, steep_rad: f32) -> f32 {
    let t = (slope_rad / steep_rad.max(1e-3)).clamp(0.0, 1.0);
    (base + (1.0 - base) * t).clamp(0.0, 1.0)
}

/// Traversability hazard in `[0, 1]` from slope: `0` below `safe_rad`, ramping to
/// `1` at/above `cliff_rad` (smoothstep between).
#[inline]
pub fn hazard_from_slope(slope_rad: f32, safe_rad: f32, cliff_rad: f32) -> f32 {
    let lo = safe_rad.min(cliff_rad);
    let hi = cliff_rad.max(safe_rad);
    if hi - lo < 1e-6 {
        return if slope_rad >= hi { 1.0 } else { 0.0 };
    }
    let t = ((slope_rad - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t) // smoothstep
}

/// Pack the surface data layers into the RGBA8 layout the `terrain_layered.wgsl`
/// `surface_map` slot samples: **R = roughness, G = AO, B = rock density,
/// A = unused**. Inputs are `[0, 1]` per channel, row-major `res×res`; `rock` may
/// be empty (→ 0) until a rock-density layer feeds it.
///
/// `A` once carried a slope-hazard bake. Hazard is now a *view*, not baked data:
/// the shader evaluates it per-pixel from the geometric normal against the live
/// `overlay_*` uniforms ([`crate::transfer::TransferFn::SlopeHazard`]), so the
/// critical angle re-tunes with no re-bake. Nothing sampled `A`, and baking it
/// pinned a second, frozen copy of the safe/cliff angles into the cache key.
pub fn pack_surface_rgba8(roughness: &[f32], ao: &[f32], rock: &[f32]) -> Vec<u8> {
    // The two required channels must be the same length (rock may be empty → 0);
    // a mismatch means a caller bug that would otherwise silently produce a short,
    // misaddressed texture. Catch it in dev; still degrade to the shortest in release.
    debug_assert!(
        roughness.len() == ao.len(),
        "surface-map channels differ: rough={}, ao={}",
        roughness.len(),
        ao.len(),
    );
    let n = roughness.len().min(ao.len());
    let mut out = Vec::with_capacity(n * 4);
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    for i in 0..n {
        let b = rock.get(i).copied().unwrap_or(0.0);
        out.push(q(roughness[i]));
        out.push(q(ao[i]));
        out.push(q(b));
        out.push(255);
    }
    out
}

/// Encode source-frame normals into the standard `[0,1]`-biased RGBA8 normal-map
/// layout (`rgb = n*0.5 + 0.5`) the `normal_map` slot decodes, with the
/// relief-correlated **albedo scalar riding the alpha channel** (0.5 = neutral;
/// see [`albedo_map`]). `albedo` may be empty (→ 255, the opaque-alpha default).
pub fn pack_normal_rgba8(normals: &[[f32; 3]], albedo: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(normals.len() * 4);
    let enc = |v: f32| ((v * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    for (i, n) in normals.iter().enumerate() {
        out.push(enc(n[0]));
        out.push(enc(n[1]));
        out.push(enc(n[2]));
        out.push(albedo.get(i).map_or(255, |&a| q(a)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::HeightSource;

    /// Flat plane at a constant height.
    struct Flat(f64);
    impl HeightSource for Flat {
        fn height_at(&self, _x: f64, _z: f64) -> f64 {
            self.0
        }
    }

    /// Ground tilted along +X: height = k·x.
    struct Ramp(f64);
    impl HeightSource for Ramp {
        fn height_at(&self, x: f64, _z: f64) -> f64 {
            self.0 * x
        }
    }

    /// A deliberately discontinuous continuation used only to prove that a
    /// finite AO footprint never samples beyond its authored boundary.
    struct EdgeWall;
    impl HeightSource for EdgeWall {
        fn height_at(&self, x: f64, _z: f64) -> f64 {
            if x > 100.0 {
                100.0
            } else {
                0.0
            }
        }
    }

    /// A conical pit centred at the origin (height rises with radius up to 0).
    struct Pit;
    impl HeightSource for Pit {
        fn height_at(&self, x: f64, z: f64) -> f64 {
            -50.0 + (x * x + z * z).sqrt().min(50.0)
        }
    }

    fn region() -> Square {
        Square {
            center: [0.0, 0.0],
            half: 100.0,
        }
    }

    #[test]
    fn flat_source_is_up_normal_flat_open() {
        let s = Flat(7.0);
        let r = region();
        let n = normal_map(&s, &r, 8);
        for v in &n {
            assert!(v[0].abs() < 1e-5 && v[2].abs() < 1e-5 && (v[1] - 1.0).abs() < 1e-5);
        }
        let slope = slope_map(&s, &r, 8);
        assert!(slope.iter().all(|&v| v.abs() < 1e-5));
        // Flat → unoccluded everywhere.
        let ao = ao_map(&s, &r, 8, 30.0, 8, 6, f64::INFINITY);
        assert!(ao.iter().all(|&v| (v - 1.0).abs() < 1e-4));
    }

    #[test]
    fn ramp_slope_and_normal_known() {
        let s = Ramp(0.1); // gradient 0.1 → slope atan(0.1)
        let r = region();
        let slope = slope_map(&s, &r, 8);
        let want = 0.1f64.atan() as f32;
        for &v in &slope {
            assert!((v - want).abs() < 1e-3, "slope {v} != {want}");
        }
        // Normal tilts away from the climb (−x), still mostly up.
        let n = normal_map(&s, &r, 8);
        assert!(n.iter().all(|v| v[0] < 0.0 && v[1] > 0.9));
    }

    /// The fused pass must be BIT-identical to the two standalone maps — it is
    /// the same `normal_at` and the same `acos(clamp(n.y))` `slope_at` performs,
    /// just not sampled twice. Determinism (content-addressable derived maps)
    /// rides on this equality.
    #[test]
    fn fused_normal_slope_matches_standalone_maps() {
        let r = region();
        let (n_fused, s_fused) = normal_slope_maps(&Pit, &r, 16);
        assert_eq!(n_fused, normal_map(&Pit, &r, 16));
        assert_eq!(s_fused, slope_map(&Pit, &r, 16));
    }

    #[test]
    fn pit_bottom_is_more_occluded_than_rim() {
        let r = region();
        let ao = ao_map(&Pit, &r, 16, 60.0, 8, 8, f64::INFINITY);
        // texel index helper
        let res = 16;
        let at = |ix: usize, iz: usize| ao[iz * res + ix];
        let center = at(res / 2, res / 2); // bottom of the pit
        let corner = at(0, 0); // out near the rim
        assert!(center < corner, "pit bottom {center} not < rim {corner}");
        assert!((0.0..=1.0).contains(&center) && (0.0..=1.0).contains(&corner));
    }

    #[test]
    fn finite_ao_opens_at_the_authored_edge() {
        let s = EdgeWall;
        let r = Square {
            center: [0.0, 0.0],
            half: 100.0,
        };
        let bounded = ao_map(&s, &r, 8, 60.0, 8, 8, 100.0);
        let unbounded = ao_map(&s, &r, 8, 60.0, 8, 8, f64::INFINITY);
        // At the right edge, rays that leave the measured square see open sky;
        // extending the ramp beyond the DEM would incorrectly occlude them.
        assert!(bounded[3 * 8 + 7] > unbounded[3 * 8 + 7]);
    }

    #[test]
    fn slope_roughness_hazard_ramps() {
        // roughness rises with slope from the base.
        assert!((roughness_from_slope(0.0, 0.6, 0.7) - 0.6).abs() < 1e-6);
        assert!(roughness_from_slope(0.7, 0.6, 0.7) > 0.99);
        // hazard: 0 below safe, 1 at/above cliff, monotone between.
        let safe = 15f32.to_radians();
        let cliff = 30f32.to_radians();
        assert_eq!(hazard_from_slope(0.0, safe, cliff), 0.0);
        assert_eq!(hazard_from_slope(cliff, safe, cliff), 1.0);
        let mid = hazard_from_slope(22.5f32.to_radians(), safe, cliff);
        assert!(mid > 0.0 && mid < 1.0);
    }

    #[test]
    fn packing_layouts() {
        // surface: R=rough G=ao B=rock (empty → 0) A=unused (opaque)
        let surf = pack_surface_rgba8(&[1.0], &[0.5], &[]);
        assert_eq!(surf, vec![255, 128, 0, 255]);
        // normal: up vector → (0.5,1.0,0.5)*255 biased; empty albedo → opaque
        let nrm = pack_normal_rgba8(&[[0.0, 1.0, 0.0]], &[]);
        assert_eq!(nrm, vec![128, 255, 128, 255]);
        // albedo scalar rides the alpha channel
        let nrm = pack_normal_rgba8(&[[0.0, 1.0, 0.0]], &[0.5]);
        assert_eq!(nrm, vec![128, 255, 128, 128]);
    }

    #[test]
    fn upsample_is_texel_centred() {
        // 2×2 → 4×4, linear in x. Src texel centres sit at u = 0.25 / 0.75; dst
        // centres at 0.125 / 0.375 / 0.625 / 0.875 → texel-centred interpolation
        // gives [0, 0.25, 0.75, 1] per row (edges clamp-extended). The node-based
        // map returned [0, 1/3, 2/3, 1] — half a source texel off.
        let src = [0.0f32, 1.0, 0.0, 1.0];
        let up = upsample_bilinear(&src, 2, 4);
        let want = [0.0f32, 0.25, 0.75, 1.0];
        for iz in 0..4 {
            for ix in 0..4 {
                assert!(
                    (up[iz * 4 + ix] - want[ix]).abs() < 1e-6,
                    "({ix},{iz}) = {} want {}",
                    up[iz * 4 + ix],
                    want[ix]
                );
            }
        }
        // A constant map upsamples to the same constant.
        let flat = upsample_bilinear(&[0.7f32; 9], 3, 8);
        assert!(flat.iter().all(|&v| (v - 0.7).abs() < 1e-6));
    }

    #[test]
    fn albedo_map_rim_brighter_than_bowl() {
        let r = region();
        let a = albedo_map(&Pit, &r, 16, 1.0, f64::INFINITY);
        let res = 16;
        let at = |ix: usize, iz: usize| a[iz * res + ix];
        // The conical pit's floor is concave (positive Laplacian) → darker than
        // neutral; flat ground far from the pit stays near neutral.
        let center = at(res / 2, res / 2);
        let corner = at(0, 0);
        assert!(
            center < corner,
            "bowl {center} not darker than open ground {corner}"
        );
        assert!(a.iter().all(|&v| (0.0..=1.0).contains(&v)));
    }

    #[test]
    fn finite_albedo_keeps_a_linear_edge_source_linear() {
        let r = Square {
            center: [0.0, 0.0],
            half: 100.0,
        };
        let a = albedo_map(&Ramp(0.1), &r, 8, 1.0, 100.0);
        let first = a[0];
        assert!(a.iter().all(|&value| (value - first).abs() < 1.0e-6));
    }
}
