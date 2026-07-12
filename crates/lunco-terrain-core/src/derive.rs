//! Derived data layers — slope / normal / ambient-occlusion / surface-pack
//! rasters computed as **pure functions of a [`HeightSource`]** over a region.
//!
//! These are the engine-neutral half of the layered terrain pipeline (design
//! `docs/terrain-layered-pipeline-design.md` Part C.2 / tracker P3b). The Bevy
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
use crate::source::HeightSource;

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

/// Slope angle from vertical (radians, `0` = flat) over `region`, row-major.
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
) -> Vec<f32> {
    let res = res.max(1);
    let dirs = dirs.max(1);
    let steps = steps.max(1);
    let radius = radius_m.max(1e-3);
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
                    let dh = src.height_at(x + dx * dist, z + dz * dist) - h0;
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
        let t = los_hit(&s, [0.0, 2.0, 0.0], [dx / len, dy / len, 0.0], 100.0, 10.0, 0.5, 0.05)
            .expect("ray should hit the slope");
        let hit_x = (dx / len) * t;
        assert!((hit_x - 8.0).abs() < 0.6, "hit x = {hit_x}");
    }

    #[test]
    fn ray_above_relief_and_outside_footprint_miss() {
        let s = Ramp;
        // Horizontal ray well above the highest terrain (0.1·10 = 1.0 at the edge).
        assert!(los_hit(&s, [-10.0, 100.0, 0.0], [1.0, 0.0, 0.0], 20.0, 10.0, 0.5, 0.05).is_none());
        // Entirely outside the ±10 footprint → open sky, no hit.
        assert!(los_hit(&s, [200.0, 5.0, 0.0], [1.0, 0.0, 0.0], 10.0, 10.0, 0.5, 0.05).is_none());
    }
}

/// Relief-correlated albedo scalar in `[0, 1]` (0.5 = neutral) over `region`,
/// row-major. Convex ground (crater rims, ejecta crests) reads slightly
/// brighter, concave ground (bowls, hollows) slightly darker, and steep faces
/// get a touch of mass-wasting brightness — the tonal variation that makes
/// distant relief legible even where geometry/shading detail has LOD'd away.
/// Curvature is the central-difference Laplacian normalised by the texel step,
/// squashed through `tanh` so extreme relief saturates instead of clipping.
///
/// `stencil_texels` widens the curvature stencil (in texels). A 1-texel
/// Laplacian on a source band-limited at 2 texels sits exactly AT Nyquist and
/// returns per-texel checker noise instead of curvature — rendered as a hard
/// mosaic of map texels at mid distance. Pair a stencil of `s` texels with a
/// source limited to wavelengths ≥ `2·s` texels; the `/ stencil` keeps the
/// response to SMOOTH curvature at the same visual level regardless of width.
pub fn albedo_map<S: HeightSource>(
    src: &S,
    region: &Square,
    res: usize,
    stencil_texels: f64,
) -> Vec<f32> {
    let res = res.max(1);
    let stencil = stencil_texels.max(1.0);
    let eps = texel_eps(region, res) * stencil;
    let mut out = Vec::with_capacity(res * res);
    for iz in 0..res {
        for ix in 0..res {
            let (x, z) = texel_world(region, res, ix, iz);
            let h = src.height_at(x, z);
            let lap = (src.height_at(x + eps, z)
                + src.height_at(x - eps, z)
                + src.height_at(x, z + eps)
                + src.height_at(x, z - eps)
                - 4.0 * h)
                / eps;
            // Concave (positive Laplacian) → darker; convex → brighter.
            let curve = (-lap * 2.0 / stencil).tanh() as f32;
            let slope = src.slope_at(x, z, eps) as f32;
            let a = 0.5 + 0.30 * curve + 0.10 * (slope / 0.6).min(1.0);
            out.push(a.clamp(0.0, 1.0));
        }
    }
    out
}

/// Bilinear upsample of a square scalar map from `src_res`² to `dst_res`².
/// Lets smooth-by-construction channels (AO) bake at reduced resolution —
/// quarter the hemisphere-march cost at half res — then expand to pack size.
pub fn upsample_bilinear(src: &[f32], src_res: usize, dst_res: usize) -> Vec<f32> {
    assert_eq!(src.len(), src_res * src_res);
    if src_res == dst_res {
        return src.to_vec();
    }
    let mut out = Vec::with_capacity(dst_res * dst_res);
    let scale = if dst_res > 1 { (src_res - 1) as f32 / (dst_res - 1) as f32 } else { 0.0 };
    for iz in 0..dst_res {
        let fz = iz as f32 * scale;
        let z0 = (fz as usize).min(src_res - 1);
        let z1 = (z0 + 1).min(src_res - 1);
        let tz = fz - z0 as f32;
        for ix in 0..dst_res {
            let fx = ix as f32 * scale;
            let x0 = (fx as usize).min(src_res - 1);
            let x1 = (x0 + 1).min(src_res - 1);
            let tx = fx - x0 as f32;
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

/// Encode world-space normals into the standard `[0,1]`-biased RGBA8 normal-map
/// layout (`rgb = n*0.5 + 0.5`) the `normal_map` slot decodes, with the
/// relief-correlated **albedo scalar riding the alpha channel** (0.5 = neutral;
/// see [`albedo_map`]). `albedo` may be empty (→ 255, the legacy opaque alpha).
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

    /// A conical pit centred at the origin (height rises with radius up to 0).
    struct Pit;
    impl HeightSource for Pit {
        fn height_at(&self, x: f64, z: f64) -> f64 {
            -50.0 + (x * x + z * z).sqrt().min(50.0)
        }
    }

    fn region() -> Square {
        Square { center: [0.0, 0.0], half: 100.0 }
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
        let ao = ao_map(&s, &r, 8, 30.0, 8, 6);
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

    #[test]
    fn pit_bottom_is_more_occluded_than_rim() {
        let r = region();
        let ao = ao_map(&Pit, &r, 16, 60.0, 8, 8);
        // texel index helper
        let res = 16;
        let at = |ix: usize, iz: usize| ao[iz * res + ix];
        let center = at(res / 2, res / 2); // bottom of the pit
        let corner = at(0, 0); // out near the rim
        assert!(center < corner, "pit bottom {center} not < rim {corner}");
        assert!((0.0..=1.0).contains(&center) && (0.0..=1.0).contains(&corner));
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
    fn albedo_map_rim_brighter_than_bowl() {
        let r = region();
        let a = albedo_map(&Pit, &r, 16, 1.0);
        let res = 16;
        let at = |ix: usize, iz: usize| a[iz * res + ix];
        // The conical pit's floor is concave (positive Laplacian) → darker than
        // neutral; flat ground far from the pit stays near neutral.
        let center = at(res / 2, res / 2);
        let corner = at(0, 0);
        assert!(center < corner, "bowl {center} not darker than open ground {corner}");
        assert!(a.iter().all(|&v| (0.0..=1.0).contains(&v)));
    }
}
