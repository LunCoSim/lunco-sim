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

/// Read a heightmap's georeferencing — extent, pixel size, where on the body
/// the crop sits, and which lunar frame those coordinates are in
/// (`GeoTransform::frame`; `None` when the raster does not declare one —
/// unknown, never a default guess).
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
///
/// Thin wrapper over the shared decode core,
/// [`lunco_geotiff::decode_gray_f64`], which owns the sample-format match, the
/// lifted `Limits` (LROC single-strip rasters exceed the `tiff` default) and the
/// nodata→`NaN` mapping. This crate and `lunco-assets` used to carry a copy each
/// and they drifted; there is now one.
///
/// What stays here is the part only this caller can judge: a short read is fatal
/// for a height grid, so the sample count is checked against the dimensions.
pub fn decode_geotiff_f64(bytes: &[u8]) -> Result<(usize, usize, Vec<f64>), DemError> {
    let (w, h, heights) = lunco_geotiff::decode_gray_f64(Cursor::new(bytes))?;

    if heights.len() != w * h {
        return Err(DemError::SizeMismatch {
            expected: w * h,
            got: heights.len(),
        });
    }
    Ok((w, h, heights))
}

/// Build a [`HeightGrid`] from a decoded heightmap.
///
/// Requires a **square** raster (`HeightGrid` is square / origin-centred; the
/// PGDA tiles are square — a non-square ROI crop would need a rectangular grid,
/// a follow-up). Heights stay **absolute** (metres of elevation), so the surface
/// sits at its true lunar datum height.
///
/// ## Nodata is trimmed away, not invented
///
/// A crop whose window overran its source raster carries a nodata margin, and
/// the grid must come back hole-free. The resolution is to **shrink to the
/// largest fully-measured centred square** ([`largest_measured_centred_square`])
/// and report the smaller extent honestly — a smaller real surface beats a
/// larger invented one, the same trade the ROI cropper already makes when the
/// window runs off the raster's pixel bounds.
///
/// It used to fill nodata with the crop's minimum elevation instead. That is a
/// *constant* beside real relief, i.e. a CLIFF: on an Apollo-15 crop it stood a
/// ~5 km wall around the map edge and dragged `world bounds min.y` kilometres
/// below any ground in the scene. Extrapolating the edge outward removes the
/// wall but paints a flat apron of terrain nobody surveyed. Trimming states
/// what is actually known.
///
/// The extent shrinks WITH the sample count — re-derived from `pixel_size_m`,
/// never scaled — so the georeferencing stays exact, and the window stays
/// centred so the site anchor remains the centre sample.
///
/// The finite check below is an invariant guard: [`decode_geotiff_f64`] has
/// already mapped every declared sentinel to `NaN` — see [`nodata_to_nan`]. Do
/// not "simplify" that away: a non-finite value reaching the crop path is
/// invalid terrain and must not become an invented surface.
pub fn height_grid_from_geotiff(bytes: &[u8]) -> Result<HeightGrid, DemError> {
    let (w, h, mut heights) = decode_geotiff_f64(bytes)?;
    if w != h {
        return Err(DemError::NonSquare {
            width: w,
            height: h,
        });
    }

    // The raster states its own extent. No fallback: a raster with no
    // georeferencing cannot be placed, and a guessed extent would put terrain
    // silently at the wrong scale.
    let geo = read_geotiff_transform(bytes).map_err(DemError::NoGeoreferencing)?;

    if !heights.iter().any(|v| v.is_finite()) {
        return Err(DemError::AllNoData);
    }

    // Only a nodata component connected to the raster boundary is an overrun
    // margin. A disconnected hole is an interior measurement failure and must
    // be rejected; filling it would fabricate terrain in the authoritative DEM.
    let interior_nodata = interior_nodata_count(&heights, w, h);
    if interior_nodata > 0 {
        return Err(DemError::InteriorNoData {
            count: interior_nodata,
        });
    }

    // Prefer a SMALLER REAL surface over a larger invented one: trim the crop to
    // the largest centred square that is entirely measured, rather than
    // extrapolating a nodata margin into terrain that was never surveyed.
    let res = largest_measured_centred_square(&heights, w, h).ok_or(
        DemError::NoCenteredMeasuredSquare {
            width: w,
            height: h,
        },
    )?;
    if res < w {
        heights = crop_centred(&heights, w, res);
    }
    debug_assert!(heights.iter().all(|v| v.is_finite()));

    // Node-based: the span is (res - 1) pixels wide, so the trimmed extent must
    // be re-derived from the NEW sample count, not scaled from the old one.
    let half_extent = (geo.pixel_size_m * (res as f64 - 1.0) * 0.5) as f32;

    Ok(HeightGrid {
        res,
        half_extent,
        heights,
    })
}

/// Side length of the largest **centred** square window containing no nodata.
///
/// Centred, not merely largest, because the crop's centre IS the site anchor:
/// the terrain is placed by putting that sample on the geodetic point the scene
/// anchors to, so re-centring on the measured region would slide the whole
/// surface off its coordinates. Shrinking symmetrically keeps the anchor where
/// it is and simply admits a smaller map.
///
/// Returns `Some(w)` when the raster is fully measured. The window shrinks two
/// samples at a time so its parity matches `w` and the centre sample stays the
/// centre. Returns `None` when no measured centred square exists.
///
/// Monotone by construction — a smaller centred window is a subset of a larger
/// one, so it can never contain more nodata — which is what makes the binary
/// search valid. `O(w·h)` for the summed-area table, `O(log w)` probes after.
fn largest_measured_centred_square(heights: &[f64], w: usize, h: usize) -> Option<usize> {
    // Summed-area table of NODATA counts, so any window's count is 4 lookups.
    let mut sat = vec![0u32; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += u32::from(!heights[y * w + x].is_finite());
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row;
        }
    }
    let nodata_in = |x0: usize, y0: usize, n: usize| -> u32 {
        let (x1, y1) = (x0 + n, y0 + n);
        sat[y1 * (w + 1) + x1] + sat[y0 * (w + 1) + x0]
            - sat[y0 * (w + 1) + x1]
            - sat[y1 * (w + 1) + x0]
    };
    // `k` = samples trimmed from EACH side. Find the smallest clean `k`.
    let (mut lo, mut hi) = (0usize, w / 2);
    while lo < hi {
        let k = (lo + hi) / 2;
        let n = w - 2 * k;
        if n == 0 || nodata_in(k, k, n) > 0 {
            lo = k + 1;
        } else {
            hi = k;
        }
    }
    let res = w.saturating_sub(2 * lo);
    (res > 0).then_some(res)
}

/// Take the centred `n × n` window out of a `w × w` grid.
fn crop_centred(heights: &[f64], w: usize, n: usize) -> Vec<f64> {
    let off = (w - n) / 2;
    let mut out = Vec::with_capacity(n * n);
    for y in 0..n {
        let row = (y + off) * w + off;
        out.extend_from_slice(&heights[row..row + n]);
    }
    out
}

/// Count non-finite samples that are not connected to the raster boundary.
/// Boundary-connected samples are an honest crop overrun and are removed by
/// [`largest_measured_centred_square`]; disconnected samples are interior holes.
fn interior_nodata_count(heights: &[f64], w: usize, h: usize) -> usize {
    let mut exterior = vec![false; heights.len()];
    let mut queue = std::collections::VecDeque::new();
    let mut seed = |x: usize, y: usize| {
        let i = y * w + x;
        if !heights[i].is_finite() && !exterior[i] {
            exterior[i] = true;
            queue.push_back(i);
        }
    };
    for x in 0..w {
        seed(x, 0);
        seed(x, h - 1);
    }
    for y in 0..h {
        seed(0, y);
        seed(w - 1, y);
    }
    while let Some(i) = queue.pop_front() {
        let x = i % w;
        let y = i / w;
        let mut visit = |nx: usize, ny: usize| {
            let n = ny * w + nx;
            if !heights[n].is_finite() && !exterior[n] {
                exterior[n] = true;
                queue.push_back(n);
            }
        };
        if x > 0 {
            visit(x - 1, y);
        }
        if x + 1 < w {
            visit(x + 1, y);
        }
        if y > 0 {
            visit(x, y - 1);
        }
        if y + 1 < h {
            visit(x, y + 1);
        }
    }
    heights
        .iter()
        .enumerate()
        .filter(|(i, value)| !value.is_finite() && !exterior[*i])
        .count()
}

/// Errors from loading a DEM terrain asset.
#[derive(Debug)]
pub enum DemError {
    Tiff(tiff::TiffError),
    /// The TIFF sample format isn't a supported numeric height type.
    UnsupportedSamples,
    SizeMismatch {
        expected: usize,
        got: usize,
    },
    NonSquare {
        width: usize,
        height: usize,
    },
    /// Every sample was nodata/NaN — no surface to build.
    AllNoData,
    /// Non-finite samples remain inside the measured footprint. The loader does
    /// not invent heights for an interior measurement hole.
    InteriorNoData {
        count: usize,
    },
    /// Boundary trimming left no measured square centred on the authored site.
    NoCenteredMeasuredSquare {
        width: usize,
        height: usize,
    },
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
                write!(
                    f,
                    "non-square DEM {width}x{height}; only square tiles are supported"
                )
            }
            DemError::AllNoData => write!(f, "DEM is entirely nodata"),
            DemError::InteriorNoData { count } => write!(
                f,
                "DEM contains {count} interior nodata samples; repair the source raster"
            ),
            DemError::NoCenteredMeasuredSquare { width, height } => write!(
                f,
                "DEM {width}x{height} has no measured square centred on the authored site"
            ),
            DemError::NoGeoreferencing(m) => write!(
                f,
                "heightmap has no usable georeferencing, so its ground extent is \
                 unknown ({m}). Re-run `cargo run -p lunco-assets -- process --twin <dir>`"
            ),
        }
    }
}

impl std::error::Error for DemError {}

/// The shared decode core's failures, in this crate's vocabulary. Kept as a
/// mapping rather than a wrapped variant so `DemError`'s public shape — which
/// callers match on — does not change now that the decode moved out.
impl From<lunco_geotiff::GrayDecodeError> for DemError {
    fn from(e: lunco_geotiff::GrayDecodeError) -> Self {
        match e {
            lunco_geotiff::GrayDecodeError::Tiff(e) => DemError::Tiff(e),
            lunco_geotiff::GrayDecodeError::UnsupportedSamples => DemError::UnsupportedSamples,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiff::encoder::{colortype, TiffEncoder};

    /// The real Apollo-15 shape: a clean nodata margin on ONE side (the crop
    /// overran its source raster). The trim must shrink symmetrically about the
    /// centre — the site anchor is the centre sample and may not move.
    #[test]
    fn one_sided_nodata_margin_trims_to_a_centred_square() {
        let n = f64::NAN;
        // 6x6, rightmost 2 columns nodata → valid x 0..=3, centre between 2 and 3.
        let mut g = Vec::new();
        for _y in 0..6 {
            g.extend_from_slice(&[1.0, 1.0, 1.0, 1.0, n, n]);
        }
        // Centred clean square: trimming 2 per side leaves x/y 2..=3 → side 2.
        let side = largest_measured_centred_square(&g, 6, 6).unwrap();
        assert_eq!(side, 2, "must shrink about the CENTRE, not slide left");
        let cropped = crop_centred(&g, 6, side);
        assert_eq!(cropped.len(), side * side);
        assert!(
            cropped.iter().all(|v| v.is_finite()),
            "trimmed grid is hole-free"
        );
        g.clear();
    }

    /// A fully-measured raster must not be trimmed at all.
    #[test]
    fn clean_raster_is_not_trimmed() {
        let g = vec![5.0f64; 16];
        assert_eq!(largest_measured_centred_square(&g, 4, 4), Some(4));
    }

    /// Boundary-connected nodata is a crop overrun and may be removed; it is not
    /// an interior measurement hole.
    #[test]
    fn boundary_nodata_is_distinguished_from_an_interior_hole() {
        let n = f64::NAN;
        let mut g = vec![
            -4000.0, 1000.0, 1000.0, n, //
            1000.0, 1000.0, 1000.0, n, //
            1000.0, 1000.0, 1000.0, n, //
            n, n, n, n,
        ];
        assert_eq!(interior_nodata_count(&g, 4, 4), 0);
        g[5] = n;
        assert_eq!(interior_nodata_count(&g, 4, 4), 1);
    }

    /// Interior nodata is not silently repaired by a nearest-neighbour terrain
    /// guess. The loader rejects it so the source asset can be corrected.
    #[test]
    fn interior_speck_is_rejected_by_the_boundary_classifier() {
        let mut g: Vec<f64> = (0..25).map(|i| i as f64).collect();
        g[12] = f64::NAN;
        assert_eq!(interior_nodata_count(&g, 5, 5), 1);
    }

    #[test]
    fn even_raster_without_a_measured_centre_is_rejected() {
        let mut g = vec![f64::NAN; 4 * 4];
        g[0] = 1.0;
        assert_eq!(largest_measured_centred_square(&g, 4, 4), None);
    }

    /// Encode a georeferenced `w*h` f32 raster spanning `size_m`, as the DEM
    /// processor does. Tests must build the same kind of file production reads.
    fn encode_dem(w: u32, data: &[f32], size_m: f64) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut buf).unwrap();
            let geo = lunco_geotiff::GeoTransform::centred_square(
                size_m, w as usize, 1737.0e3, 26.0371, 3.6584,
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
            enc.write_image::<colortype::Gray32Float>(2, 2, &[0.0f32; 4])
                .unwrap();
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

    #[test]
    fn raster_without_a_centred_measured_square_is_rejected() {
        let mut data = vec![f32::NAN; 16];
        data[0] = 1.0;
        let err = height_grid_from_geotiff(&encode_dem(4, &data, 4.0)).unwrap_err();
        assert!(
            matches!(err, DemError::NoCenteredMeasuredSquare { .. }),
            "{err}"
        );
    }

    /// The extent the grid reports must be the extent the raster declares —
    /// this is the agreement a sidecar could break.
    #[test]
    fn extent_comes_from_the_raster() {
        let bytes = encode_dem(2, &[0.0f32; 4], 1002.0);
        let grid = height_grid_from_geotiff(&bytes).unwrap();
        assert_eq!(grid.half_extent, 501.0);
    }

    /// A raster stamped MOON_ME reads back MOON_ME; one that declares nothing
    /// reads back `None`. ME vs PA is ≈ 875 m of silent offset, so an unknown
    /// frame must stay unknown rather than default to the likely answer.
    #[test]
    fn lunar_frame_survives_the_raster_or_stays_unknown() {
        use lunco_geotiff::LunarFrame;

        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut buf).unwrap();
            let geo =
                lunco_geotiff::GeoTransform::centred_square(2.0, 2, 1737.0e3, 26.0371, 3.6584)
                    .with_frame(LunarFrame::MoonMe);
            let mut img = enc.new_image::<colortype::Gray32Float>(2, 2).unwrap();
            lunco_geotiff::write_geo_tags(img.encoder(), &geo, "Moon 2000").unwrap();
            img.write_data(&[0.0f32; 4]).unwrap();
        }
        let tf = read_geotiff_transform(buf.get_ref()).unwrap();
        assert_eq!(tf.frame, Some(LunarFrame::MoonMe));

        // `encode_dem` declares no frame — the fixture for every pre-frame file.
        let bytes = encode_dem(2, &[0.0f32; 4], 2.0);
        assert_eq!(read_geotiff_transform(&bytes).unwrap().frame, None);
    }

    #[test]
    fn height_source_trait_dispatch() {
        let bytes = encode_dem(2, &[1.0, 2.0, 3.0, 4.0], 2.0);
        let grid = height_grid_from_geotiff(&bytes).unwrap();
        let h = <HeightGrid as HeightSource>::height_at(&grid, 0.0, 0.0);
        assert_eq!(h, 2.5);
    }
}
