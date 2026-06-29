//! Loader for the DEM terrain assets produced by `lunar_terrain_exporter`
//! (NASA PGDA Product 78, 5 m/pixel LOLA south-pole DEMs).
//!
//! The exporter writes, per site, a directory:
//! ```text
//! <site>/metadata.yaml
//! <site>/materials/textures/heightmap.tif   # float32 GeoTIFF, elevation in metres
//! ```
//! This module turns that pair into a [`HeightGrid`] — the **same** height
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
use crate::source::HeightSource;

/// Sidecar metadata emitted alongside the heightmap (`metadata.yaml`). Only the
/// fields the loader needs are kept; the file is a tiny flat YAML map (plus a
/// nested `coordinates:` block), so it is hand-parsed — no yaml dependency.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DemMetadata {
    pub site_id: String,
    /// Raster width / height in samples (the exporter always writes square-ish
    /// tiles; the loader requires square — see [`height_grid_from_geotiff`]).
    pub resolution_x: usize,
    pub resolution_y: usize,
    /// Real-world span of the tile in metres (full pixel extent).
    pub size_x_m: f64,
    pub size_y_m: f64,
    pub elevation_min_m: f64,
    pub elevation_max_m: f64,
    /// Tile-centre geographic coordinates (for georeferencing; not used by the
    /// height math itself).
    pub center_lat: f64,
    pub center_lon: f64,
}

impl DemMetadata {
    /// Parse the exporter's `metadata.yaml`. Tolerant: unknown keys are ignored,
    /// quoting is stripped, and the one nested block (`coordinates:`) is tracked
    /// by indentation. Returns [`DemError::Metadata`] only if the essential
    /// `size_x_m` / `resolution_x` are absent.
    pub fn from_yaml_str(s: &str) -> Result<Self, DemError> {
        let mut m = DemMetadata::default();
        let mut in_coords = false;
        for line in s.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let indent = line.len() - line.trim_start().len();
            let Some((key, raw_val)) = trimmed.split_once(':') else {
                continue;
            };
            let key = key.trim();
            let val = raw_val.trim().trim_matches('"').trim_matches('\'');

            // A top-level key ends any nested block we were inside.
            if indent == 0 {
                in_coords = false;
            }
            if key == "coordinates" && val.is_empty() {
                in_coords = true;
                continue;
            }

            match (in_coords, key) {
                (true, "lat") => m.center_lat = val.parse().unwrap_or(m.center_lat),
                (true, "lon") => m.center_lon = val.parse().unwrap_or(m.center_lon),
                (_, "site_id") => m.site_id = val.to_string(),
                (_, "resolution_x") => m.resolution_x = val.parse().unwrap_or(0),
                (_, "resolution_y") => m.resolution_y = val.parse().unwrap_or(0),
                (_, "size_x_m") => m.size_x_m = val.parse().unwrap_or(0.0),
                (_, "size_y_m") => m.size_y_m = val.parse().unwrap_or(0.0),
                (_, "elevation_min_m") => m.elevation_min_m = val.parse().unwrap_or(0.0),
                (_, "elevation_max_m") => m.elevation_max_m = val.parse().unwrap_or(0.0),
                _ => {}
            }
        }
        if m.size_x_m <= 0.0 || m.resolution_x == 0 {
            return Err(DemError::Metadata(
                "metadata.yaml missing size_x_m / resolution_x".into(),
            ));
        }
        Ok(m)
    }
}

/// Decode a (single-band) GeoTIFF into row-major elevations. Geo tags are
/// ignored — only the raster matters; spacing/extent come from [`DemMetadata`].
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

/// Build a [`HeightGrid`] from a decoded heightmap + its metadata.
///
/// Requires a **square** raster (`HeightGrid` is square / origin-centred; the
/// PGDA tiles are square — a non-square ROI crop would need a rectangular grid,
/// a follow-up). Nodata / NaN samples are filled with the minimum finite
/// elevation so the collider and mesh have no holes (mirrors the exporter's own
/// `normalize_array` nodata handling). Heights stay **absolute** (metres of
/// elevation), so the surface sits at its true lunar datum height.
pub fn height_grid_from_geotiff(
    bytes: &[u8],
    meta: &DemMetadata,
) -> Result<HeightGrid, DemError> {
    let (w, h, mut heights) = decode_geotiff_f64(bytes)?;
    if w != h {
        return Err(DemError::NonSquare { width: w, height: h });
    }
    if meta.resolution_x != 0 && meta.resolution_x != w {
        return Err(DemError::ResolutionMismatch { meta: meta.resolution_x, raster: w });
    }

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

    // The tile spans `size_x_m` metres across `w` samples, origin-centred. The
    // tiny half-pixel difference between pixel-extent and node-span is well
    // below the 5 m sample pitch and irrelevant to physics/visuals.
    let half_extent = (meta.size_x_m * 0.5) as f32;
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
    ResolutionMismatch { meta: usize, raster: usize },
    /// Every sample was nodata/NaN — no surface to build.
    AllNoData,
    Metadata(String),
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
            DemError::ResolutionMismatch { meta, raster } => {
                write!(f, "metadata resolution {meta} != raster {raster}")
            }
            DemError::AllNoData => write!(f, "DEM is entirely nodata"),
            DemError::Metadata(m) => write!(f, "metadata: {m}"),
        }
    }
}

impl std::error::Error for DemError {}

#[cfg(test)]
mod tests {
    use super::*;
    use tiff::encoder::{colortype, TiffEncoder};

    /// Encode a `w*h` row-major f32 raster as an in-memory GeoTIFF for tests.
    fn encode_tiff_f32(w: u32, h: u32, data: &[f32]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut buf).unwrap();
            enc.write_image::<colortype::Gray32Float>(w, h, data).unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn metadata_parses_flat_and_nested() {
        let s = "\
site_id: connecting_ridge
display_name: Connecting Ridge
description: Site 01 - ridge between craters
coordinates:
  lat: -89.45
  lon: -12.3
size_x_m: 16000
size_y_m: 16000
resolution_x: 3200
resolution_y: 3200
elevation_min_m: 1239.43
elevation_max_m: 2470.1
source: nasa_pgda_78
";
        let m = DemMetadata::from_yaml_str(s).unwrap();
        assert_eq!(m.site_id, "connecting_ridge");
        assert_eq!(m.resolution_x, 3200);
        assert_eq!(m.size_x_m, 16000.0);
        assert_eq!(m.center_lat, -89.45);
        assert_eq!(m.center_lon, -12.3);
        assert_eq!(m.elevation_min_m, 1239.43);
    }

    #[test]
    fn metadata_requires_essentials() {
        assert!(DemMetadata::from_yaml_str("site_id: x\n").is_err());
    }

    #[test]
    fn decode_roundtrip_and_grid() {
        // 2x2 grid, row-major [z*2 + x].
        let data = [0.0f32, 10.0, 20.0, 30.0];
        let bytes = encode_tiff_f32(2, 2, &data);
        let (w, h, heights) = decode_geotiff_f64(&bytes).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(heights, vec![0.0, 10.0, 20.0, 30.0]);

        let meta = DemMetadata { size_x_m: 2.0, resolution_x: 2, ..Default::default() };
        let grid = height_grid_from_geotiff(&bytes, &meta).unwrap();
        assert_eq!(grid.res, 2);
        assert_eq!(grid.half_extent, 1.0); // spans [-1, 1]
        // Corners map to the four samples; centre is their mean.
        assert_eq!(grid.height_at(-1.0, -1.0), 0.0);
        assert_eq!(grid.height_at(1.0, -1.0), 10.0);
        assert_eq!(grid.height_at(-1.0, 1.0), 20.0);
        assert_eq!(grid.height_at(1.0, 1.0), 30.0);
        assert_eq!(grid.height_at(0.0, 0.0), 15.0);
    }

    #[test]
    fn height_source_trait_dispatch() {
        let bytes = encode_tiff_f32(2, 2, &[1.0, 2.0, 3.0, 4.0]);
        let meta = DemMetadata { size_x_m: 2.0, resolution_x: 2, ..Default::default() };
        let grid = height_grid_from_geotiff(&bytes, &meta).unwrap();
        // Through the trait (f64), widening the inherent f32 sampler.
        let h = <HeightGrid as HeightSource>::height_at(&grid, 0.0, 0.0);
        assert_eq!(h, 2.5);
    }

    #[test]
    fn nodata_is_filled_with_min() {
        let data = [5.0f32, f32::NAN, 7.0, 9.0];
        let bytes = encode_tiff_f32(2, 2, &data);
        let meta = DemMetadata { size_x_m: 2.0, resolution_x: 2, ..Default::default() };
        let grid = height_grid_from_geotiff(&bytes, &meta).unwrap();
        // The NaN sample (x1,z0) was filled with the min finite value (5.0).
        assert_eq!(grid.height_at(1.0, -1.0), 5.0);
    }

    #[test]
    fn non_square_is_rejected() {
        let bytes = encode_tiff_f32(2, 3, &[0.0; 6]);
        let meta = DemMetadata { size_x_m: 2.0, resolution_x: 2, ..Default::default() };
        assert!(matches!(
            height_grid_from_geotiff(&bytes, &meta),
            Err(DemError::NonSquare { width: 2, height: 3 })
        ));
    }

    #[test]
    fn resolution_mismatch_is_rejected() {
        let bytes = encode_tiff_f32(2, 2, &[0.0; 4]);
        let meta = DemMetadata { size_x_m: 2.0, resolution_x: 4096, ..Default::default() };
        assert!(matches!(
            height_grid_from_geotiff(&bytes, &meta),
            Err(DemError::ResolutionMismatch { meta: 4096, raster: 2 })
        ));
    }

    /// End-to-end against a real `lunar_terrain_exporter` asset. Disabled unless
    /// `LUNCO_DEM_TEST_DIR` points at a site directory (e.g. the moonbase Twin's
    /// `terrain/connecting_ridge`), so CI without the 40 MB asset still passes.
    /// Uses `std::fs` — test-only scaffolding; the library itself stays
    /// filesystem-free / wasm-safe.
    #[test]
    fn loads_real_exporter_asset() {
        let Ok(dir) = std::env::var("LUNCO_DEM_TEST_DIR") else {
            eprintln!("skipping: set LUNCO_DEM_TEST_DIR to a DEM site dir to run");
            return;
        };
        let dir = std::path::Path::new(&dir);
        let meta = DemMetadata::from_yaml_str(
            &std::fs::read_to_string(dir.join("metadata.yaml")).unwrap(),
        )
        .unwrap();
        let tif = std::fs::read(dir.join("materials/textures/heightmap.tif")).unwrap();
        let grid = height_grid_from_geotiff(&tif, &meta).unwrap();

        assert_eq!(grid.res, meta.resolution_x);
        assert_eq!(grid.half_extent, (meta.size_x_m * 0.5) as f32);
        // Centre height is finite and within the metadata's elevation envelope.
        let h = <HeightGrid as HeightSource>::height_at(&grid, 0.0, 0.0);
        assert!(h.is_finite());
        assert!(
            h >= meta.elevation_min_m - 1.0 && h <= meta.elevation_max_m + 1.0,
            "centre height {h} outside [{}, {}]",
            meta.elevation_min_m,
            meta.elevation_max_m,
        );
        eprintln!(
            "loaded {} ({}^2, {} m, centre {h:.1} m, elev [{:.1}, {:.1}])",
            meta.site_id, grid.res, meta.size_x_m, meta.elevation_min_m, meta.elevation_max_m,
        );
    }
}
