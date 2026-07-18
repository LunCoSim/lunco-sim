//! Loader for the DEM terrain assets produced by `lunar_terrain_exporter`
//! (NASA PGDA Product 78, 5 m/pixel LOLA south-pole DEMs).
//!
//! The exporter writes, per site, a directory:
//! ```text
//! <site>/materials/textures/heightmap.tif   # float32 GeoTIFF, elevation in metres
//! ```
//! This module turns that raster into a [`HeightGrid`] — the **same** height
//! surface the procedural obstacle field already drives — so the visual mesh
//! (`to_mesh_data`), the avian `Collider::heightfield` (`to_avian_heights`), and
//! the analytic bilinear `height_at` all come for free, with no DEM-specific
//! geometry/physics code. A blanket [`HeightSource`] impl then lets a loaded
//! grid flow through the streaming/source plumbing exactly like the analytic
//! source.
//!
//! Decoding uses the `tiff` crate (already in the workspace tree via `image`'s
//! `tiff` feature, pure-Rust → wasm-safe). **Every entry point takes bytes /
//! strings** — the loader never touches the filesystem, so it compiles and runs
//! identically on native and wasm with no `cfg` gating. Acquiring those bytes
//! (from disk, the Twin, or an HTTP fetch) is the host's job via the engine's
//! cross-platform I/O — `lunco-storage::Storage` (`read`/`read_sync`, with
//! `FileStorage` native + `WebStorage` web) or Bevy's `AssetServer`. The
//! streaming plugin (M3) wires that in; this module stays pure.

use std::fmt;
use std::io::Cursor;

use lunco_obstacle_field::field::HeightGrid;

// `HeightGrid: HeightSource` is now implemented in `lunco-obstacle-field` (with
// the type, per the orphan rule); only the tests below name the trait directly.
#[cfg(test)]
use lunco_terrain_core::source::HeightSource;

/// Read a heightmap's georeferencing — extent, pixel size, and where on the body
/// the crop sits.
///
/// The raster is the only source: it cannot disagree with the pixels it describes.
/// See `docs/architecture/57-dem-georeferencing.md`.
pub fn read_geotiff_transform(bytes: &[u8]) -> Result<lunco_geotiff::GeoTransform, String> {
    let mut dec = tiff::decoder::Decoder::new(Cursor::new(bytes))
        .map_err(|e| format!("not a readable TIFF: {e}"))?;
    lunco_geotiff::read_geo_tags(&mut dec).map_err(|e| e.to_string())
}

/// Decode a (single-band) GeoTIFF into row-major elevations.
/// Returns `(width, height, heights[row*width + col])`.
pub fn decode_geotiff_f64(bytes: &[u8]) -> Result<(usize, usize, Vec<f64>), DemError> {
    use tiff::decoder::DecodingResult as D;

    let mut dec = tiff::decoder::Decoder::new(Cursor::new(bytes)).map_err(DemError::Tiff)?;
    let (w, h) = dec.dimensions().map_err(DemError::Tiff)?;
    let (w, h) = (w as usize, h as usize);

    let heights: Vec<f64> = match dec.read_image().map_err(DemError::Tiff)? {
        D::F32(v) => v.into_iter().map(|x| x as f64).collect(),
        D::F64(v) => v,
        D::I16(v) => v.into_iter().map(|x| x as f64).collect(),
        D::U16(v) => v.into_iter().map(|x| x as f64).collect(),
        D::I32(v) => v.into_iter().map(|x| x as f64).collect(),
        D::U32(v) => v.into_iter().map(|x| x as f64).collect(),
        D::U8(v) => v.into_iter().map(|x| x as f64).collect(),
        _ => return Err(DemError::UnsupportedSamples),
    };

    if heights.len() != w * h {
        return Err(DemError::SizeMismatch { expected: w * h, got: heights.len() });
    }
    Ok((w, h, heights))
}

/// Build a [`HeightGrid`] from a decoded heightmap.
///
/// Requires a **square** raster (`HeightGrid` is square / origin-centred; the
/// PGDA tiles are square — a non-square ROI crop would need a rectangular grid,
/// a follow-up). Nodata / NaN samples are filled with the minimum finite
/// elevation so the collider and mesh have no holes (mirrors the exporter's own
/// `normalize_array` nodata handling). Heights stay **absolute** (metres of
/// elevation), so the surface sits at its true lunar datum height.
pub fn height_grid_from_geotiff(bytes: &[u8]) -> Result<HeightGrid, DemError> {
    let (w, h, mut heights) = decode_geotiff_f64(bytes)?;
    if w != h {
        return Err(DemError::NonSquare { width: w, height: h });
    }

    // The raster states its own extent. No fallback: a raster with no
    // georeferencing cannot be placed, and a guessed extent would put terrain
    // silently at the wrong scale.
    let geo = read_geotiff_transform(bytes).map_err(DemError::NoGeoreferencing)?;
    let half_extent = (geo.extent_m(w) * 0.5) as f32;

    // Fill nodata/NaN with the minimum finite elevation.
    let mut min = f64::INFINITY;
    for &v in &heights {
        if v.is_finite() && v < min {
            min = v;
        }
    }
    if !min.is_finite() {
        return Err(DemError::AllNoData);
    }
    for v in &mut heights {
        if !v.is_finite() {
            *v = min;
        }
    }

    Ok(HeightGrid { res: w, half_extent, heights })
}

/// Errors from loading a DEM terrain asset.
#[derive(Debug)]
pub enum DemError {
    Tiff(tiff::TiffError),
    /// The TIFF sample format isn't a supported numeric height type.
    UnsupportedSamples,
    SizeMismatch { expected: usize, got: usize },
    NonSquare { width: usize, height: usize },
    /// Every sample was nodata/NaN — no surface to build.
    AllNoData,
    /// The raster carries no usable georeferencing, so its ground extent is
    /// unknown. Fatal by design: the alternative is terrain at a guessed scale.
    NoGeoreferencing(String),
}

impl fmt::Display for DemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DemError::Tiff(e) => write!(f, "failed to decode heightmap GeoTIFF: {e}"),
            DemError::UnsupportedSamples => write!(f, "unsupported TIFF sample format for heights"),
            DemError::SizeMismatch { expected, got } => {
                write!(f, "decoded {got} samples, expected {expected} (w*h)")
            }
            DemError::NonSquare { width, height } => {
                write!(f, "non-square DEM {width}x{height}; only square tiles are supported")
            }
            DemError::AllNoData => write!(f, "DEM is entirely nodata"),
            DemError::NoGeoreferencing(m) => write!(
                f,
                "heightmap has no usable georeferencing, so its ground extent is \
                 unknown ({m}). Re-run `cargo run -p lunco-assets -- process --twin <dir>`"
            ),
        }
    }
}

impl std::error::Error for DemError {}

#[cfg(test)]
mod tests {
    use super::*;
    use tiff::encoder::{colortype, TiffEncoder};

    /// Encode a georeferenced `w*h` f32 raster spanning `size_m`, as the DEM
    /// processor does. Tests must build the same kind of file production reads.
    fn encode_dem(w: u32, data: &[f32], size_m: f64) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut buf).unwrap();
            let geo = lunco_geotiff::GeoTransform::centred_square(
                size_m,
                w as usize,
                1737.0e3,
                26.0371,
                3.6584,
            );
            let mut img = enc.new_image::<colortype::Gray32Float>(w, w).unwrap();
            lunco_geotiff::write_geo_tags(img.encoder(), &geo, "Moon 2000").unwrap();
            img.write_data(data).unwrap();
        }
        buf.into_inner()
    }

    /// A raster with no georeferencing has no knowable extent, so it must be
    /// rejected with an actionable error rather than placed at a guessed scale.
    #[test]
    fn plain_tiff_is_rejected_not_guessed() {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut buf).unwrap();
            enc.write_image::<colortype::Gray32Float>(2, 2, &[0.0f32; 4]).unwrap();
        }
        let err = height_grid_from_geotiff(&buf.into_inner()).unwrap_err();
        assert!(matches!(err, DemError::NoGeoreferencing(_)), "{err}");
        assert!(err.to_string().contains("lunco-assets"), "{err}");
    }

    #[test]
    fn decode_roundtrip_and_grid() {
        // 2x2 grid, row-major [z*2 + x], spanning [-1, 1].
        let data = [0.0f32, 10.0, 20.0, 30.0];
        let bytes = encode_dem(2, &data, 2.0);
        let (w, h, heights) = decode_geotiff_f64(&bytes).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(heights, vec![0.0, 10.0, 20.0, 30.0]);

        let grid = height_grid_from_geotiff(&bytes).unwrap();
        assert_eq!(grid.res, 2);
        assert_eq!(grid.half_extent, 1.0);
        // Corners map to the four samples; centre is their mean.
        assert_eq!(grid.height_at(-1.0, -1.0), 0.0);
        assert_eq!(grid.height_at(1.0, -1.0), 10.0);
        assert_eq!(grid.height_at(-1.0, 1.0), 20.0);
        assert_eq!(grid.height_at(1.0, 1.0), 30.0);
        assert_eq!(grid.height_at(0.0, 0.0), 15.0);
    }

    /// The extent the grid reports must be the extent the raster declares —
    /// this is the agreement a sidecar could break.
    #[test]
    fn extent_comes_from_the_raster() {
        let bytes = encode_dem(2, &[0.0f32; 4], 1002.0);
        let grid = height_grid_from_geotiff(&bytes).unwrap();
        assert_eq!(grid.half_extent, 501.0);
    }

    #[test]
    fn height_source_trait_dispatch() {
        let bytes = encode_dem(2, &[1.0, 2.0, 3.0, 4.0], 2.0);
        let grid = height_grid_from_geotiff(&bytes).unwrap();
        let h = <HeightGrid as HeightSource>::height_at(&grid, 0.0, 0.0);
        assert_eq!(h, 2.5);
    }
}
