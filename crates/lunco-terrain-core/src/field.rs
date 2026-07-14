//! Surface **fields** — the data-side parallel to [`HeightSource`].
//!
//! Where a [`HeightSource`] answers "how high is the ground here", a
//! [`SurfaceField`] answers "what is the *value of some analysis channel* here" —
//! slope, aspect, elevation, curvature, and (in the surface crate) AO / hazard /
//! connectivity. A field is a **pure function of a `HeightSource`**, so it is:
//!
//! - **headless** — the value exists with no renderer, GPU, or `App`; the rover
//!   planner, cosim, and the scripting query read the SAME number the shader colours;
//! - **deterministic + content-addressable** — equal inputs, equal outputs on every
//!   platform, so a materialised field raster is re-derivable and needs no per-peer
//!   transfer (the property the height channel already has);
//! - **one definition, many consumers** — render is one downstream consumer, never
//!   the definition (see `docs/architecture/terrain-layered-rendering.md`).
//!
//! The renderer materialises a field to a tile raster ([`field_map`]) and colours it
//! through a transfer function; a tool queries it point-wise
//! ([`SurfaceField::value_at`]); a USD `lunco:layer = "field"` prim *describes* which
//! field + parameters, so the layer is addressable and composable like any other.

use crate::quadtree::Square;
use crate::source::HeightSource;

/// A scalar field over the terrain surface — a pure function of a [`HeightSource`],
/// evaluated headless and deterministically. Implementors are the analysis channels
/// (slope, aspect, elevation, …); each is addressable by [`id`] for USD authoring and
/// cache keys.
///
/// [`id`]: SurfaceField::id
pub trait SurfaceField: Send + Sync {
    /// The field value at world `(x, z)`, sampled on `src` with derivative step
    /// `eps` metres (for fields that need finite differences; ignored otherwise).
    fn value_at(&self, src: &dyn HeightSource, x: f64, z: f64, eps: f64) -> f32;

    /// Stable identifier for USD layer authoring + cache keys (e.g. `"slope"`).
    fn id(&self) -> &'static str;
}

/// Materialise a field to a row-major `res × res` raster over `region`, **headless**.
/// Texel `(ix, iz)` is sampled at UV `((ix+0.5)/res, (iz+0.5)/res)` across the region
/// — the same texel-centred convention [`crate::derive`] and the terrain shader use,
/// so a materialised field aligns with the derived maps and the tile UVs.
///
/// Generic over the source (like [`crate::derive::slope_map`], which this reproduces
/// exactly for [`SlopeField`]) so the concrete oracle is monomorphised in, not reached
/// through a `&dyn` at the call site.
pub fn field_map<S: HeightSource>(
    field: &dyn SurfaceField,
    src: &S,
    region: &Square,
    res: usize,
) -> Vec<f32> {
    let res = res.max(1);
    let size = 2.0 * region.half;
    let eps = size / res as f64; // cell size = finite-difference step
    let min_x = region.center[0] - region.half;
    let min_z = region.center[1] - region.half;
    let mut out = Vec::with_capacity(res * res);
    for iz in 0..res {
        let z = min_z + ((iz as f64 + 0.5) / res as f64) * size;
        for ix in 0..res {
            let x = min_x + ((ix as f64 + 0.5) / res as f64) * size;
            out.push(field.value_at(src, x, z, eps));
        }
    }
    out
}

/// Slope from vertical (radians, `0` = flat, `π/2` = cliff) — the canonical
/// traversability / hazard field. Matches [`HeightSource::slope_at`] and
/// [`crate::derive::slope_map`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SlopeField;

impl SurfaceField for SlopeField {
    fn value_at(&self, src: &dyn HeightSource, x: f64, z: f64, eps: f64) -> f32 {
        src.slope_at(x, z, eps) as f32
    }
    fn id(&self) -> &'static str {
        "slope"
    }
}

/// Aspect — the compass azimuth (radians, `0..2π`, measured from +Z clockwise
/// toward +X) the slope faces *downhill*, from the surface normal's XZ projection.
/// Returns `0` on flat ground (aspect is undefined there).
#[derive(Debug, Clone, Copy, Default)]
pub struct AspectField;

impl SurfaceField for AspectField {
    fn value_at(&self, src: &dyn HeightSource, x: f64, z: f64, eps: f64) -> f32 {
        let n = src.normal_at(x, z, eps);
        // The normal is `(−dh/dx, 1, −dh/dz)`, so its horizontal projection
        // `(n.x, n.z)` already points in the steepest-descent (downhill) direction.
        let (dx, dz) = (n[0], n[2]);
        if dx.abs() < 1e-12 && dz.abs() < 1e-12 {
            return 0.0; // flat → aspect undefined
        }
        let a = dx.atan2(dz); // [−π, π] from +Z toward +X
        (if a < 0.0 { a + std::f64::consts::TAU } else { a }) as f32
    }
    fn id(&self) -> &'static str {
        "aspect"
    }
}

/// Elevation — the surface height itself (world Y, metres) as a field, for a
/// hypsometric ramp.
#[derive(Debug, Clone, Copy, Default)]
pub struct ElevationField;

impl SurfaceField for ElevationField {
    fn value_at(&self, src: &dyn HeightSource, x: f64, z: f64, _eps: f64) -> f32 {
        src.height_at(x, z) as f32
    }
    fn id(&self) -> &'static str {
        "elevation"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::AnalyticHeightSource;

    /// A tilted plane `y = grade·x` — known slope `atan(grade)`, aspect facing −X.
    struct Ramp {
        grade: f64,
    }
    impl HeightSource for Ramp {
        fn height_at(&self, x: f64, _z: f64) -> f64 {
            self.grade * x
        }
    }

    fn sq(half: f64) -> Square {
        Square { center: [0.0, 0.0], half }
    }

    #[test]
    fn slope_field_matches_source_and_map() {
        let src = AnalyticHeightSource::default();
        let region = sq(200.0);
        let f = SlopeField;
        // Point value equals the source's slope_at at the same eps.
        let eps = (2.0 * region.half) / 16.0;
        let v = f.value_at(&src, 12.0, -7.0, eps);
        assert!((v as f64 - src.slope_at(12.0, -7.0, eps)).abs() < 1e-6);
        // Materialised raster equals the dedicated slope_map (same convention).
        let via_field = field_map(&f, &src, &region, 16);
        let via_map = crate::derive::slope_map(&src, &region, 16);
        assert_eq!(via_field, via_map);
        assert_eq!(via_field.len(), 16 * 16);
    }

    #[test]
    fn slope_of_a_known_ramp() {
        let src = Ramp { grade: 0.5 }; // 26.57°
        let f = SlopeField;
        let s = f.value_at(&src, 3.0, 3.0, 0.5) as f64;
        assert!((s - 0.5_f64.atan()).abs() < 1e-6, "slope {s} != atan(0.5)");
    }

    #[test]
    fn flat_ground_is_zero_slope_and_up() {
        let src = AnalyticHeightSource::new(0, 0.0, 100.0, 4); // flat
        assert!(SlopeField.value_at(&src, 5.0, -5.0, 0.5).abs() < 1e-6);
        assert_eq!(AspectField.value_at(&src, 5.0, -5.0, 0.5), 0.0); // undefined → 0
    }

    #[test]
    fn aspect_of_a_ramp_faces_downhill() {
        // y = 0.5·x rises toward +X → downhill is −X → aspect azimuth = atan2(-1,0)
        // wrapped to [0,2π) = 3π/2.
        let src = Ramp { grade: 0.5 };
        let a = AspectField.value_at(&src, 1.0, 1.0, 0.5) as f64;
        assert!((a - 1.5 * std::f64::consts::PI).abs() < 1e-4, "aspect {a}");
    }

    #[test]
    fn elevation_field_is_the_height() {
        let src = AnalyticHeightSource::default();
        let e = ElevationField.value_at(&src, 10.0, 20.0, 1.0) as f64;
        assert!((e - src.height_at(10.0, 20.0)).abs() < 1e-4);
    }

    #[test]
    fn deterministic() {
        let src = AnalyticHeightSource::default();
        let region = sq(123.0);
        assert_eq!(field_map(&SlopeField, &src, &region, 8), field_map(&SlopeField, &src, &region, 8));
    }

    #[test]
    fn ids_are_stable() {
        assert_eq!(SlopeField.id(), "slope");
        assert_eq!(AspectField.id(), "aspect");
        assert_eq!(ElevationField.id(), "elevation");
    }
}
