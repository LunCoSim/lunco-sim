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
pub fn decode_geotiff_f64(bytes: &[u8]) -> Result<(usize, usize, Vec<f64>), DemError> {
    use tiff::decoder::DecodingResult as D;

    let mut dec = tiff::decoder::Decoder::new(Cursor::new(bytes)).map_err(DemError::Tiff)?;
    let (w, h) = dec.dimensions().map_err(DemError::Tiff)?;
    let (w, h) = (w as usize, h as usize);

    // Read the nodata declaration BEFORE the pixels: `read_image` advances the
    // decoder, and the tag is the raster telling us which samples are not
    // measurements. See `nodata_to_nan` for why this cannot be skipped.
    let declared_nodata = lunco_geotiff::read_gdal_nodata(&mut dec);

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
    let heights = heights
        .into_iter()
        .map(|v| lunco_geotiff::nodata_to_nan(v, declared_nodata))
        .collect();
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
/// The fill below tests `is_finite()`, which is sufficient ONLY because
/// [`decode_geotiff_f64`] has already mapped every sentinel to `NaN` — see
/// [`nodata_to_nan`]. Do not "simplify" that away: a finite sentinel reaching
/// here is indistinguishable from terrain and is filled into the surface.
pub fn height_grid_from_geotiff(bytes: &[u8]) -> Result<HeightGrid, DemError> {
    let (w, h, mut heights) = decode_geotiff_f64(bytes)?;
    if w != h {
        return Err(DemError::NonSquare { width: w, height: h });
    }

    // The raster states its own extent. No fallback: a raster with no
    // georeferencing cannot be placed, and a guessed extent would put terrain
    // silently at the wrong scale.
    let geo = read_geotiff_transform(bytes).map_err(DemError::NoGeoreferencing)?;

    if !heights.iter().any(|v| v.is_finite()) {
        return Err(DemError::AllNoData);
    }

    // Prefer a SMALLER REAL surface over a larger invented one: trim the crop to
    // the largest centred square that is entirely measured, rather than
    // extrapolating a nodata margin into terrain that was never surveyed.
    let res = largest_measured_centred_square(&heights, w, h);
    if res < w {
        heights = crop_centred(&heights, w, res);
    }
    // Only interior specks can remain (the trim removed the margin). Nearest-
    // neighbour so a speck reads as its surroundings, never as a pit.
    fill_nodata_from_nearest(&mut heights, res, res);

    // Node-based: the span is (res - 1) pixels wide, so the trimmed extent must
    // be re-derived from the NEW sample count, not scaled from the old one.
    let half_extent = (geo.pixel_size_m * (res as f64 - 1.0) * 0.5) as f32;

    Ok(HeightGrid { res, half_extent, heights })
}

/// Side length of the largest **centred** square window containing no nodata.
///
/// Centred, not merely largest, because the crop's centre IS the site anchor:
/// the terrain is placed by putting that sample on the geodetic point the scene
/// anchors to, so re-centring on the measured region would slide the whole
/// surface off its coordinates. Shrinking symmetrically keeps the anchor where
/// it is and simply admits a smaller map.
///
/// Returns `w` when the raster is fully measured. The window shrinks two samples
/// at a time so its parity matches `w` and the centre sample stays the centre.
///
/// Monotone by construction — a smaller centred window is a subset of a larger
/// one, so it can never contain more nodata — which is what makes the binary
/// search valid. `O(w·h)` for the summed-area table, `O(log w)` probes after.
fn largest_measured_centred_square(heights: &[f64], w: usize, h: usize) -> usize {
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
    w.saturating_sub(2 * lo).max(1)
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

/// Replace every `NaN` with its NEAREST measured height (breadth-first from the
/// measured/nodata boundary, 4-connected).
///
/// **Why nearest and not the global minimum.** Filling the nodata margin with
/// the crop's lowest elevation is a *constant*, and a constant next to real
/// relief is a CLIFF: an Apollo-15-sized crop spans kilometres of elevation, so
/// the fill met the terrain as a ~5 km vertical wall around the map edge —
/// visible as a sheer black/white barrier, and a `world bounds min.y` far below
/// any ground the scene contains. It also dragged the whole surface's height
/// range down, which costs precision in everything derived from it.
///
/// Nearest-neighbour extension instead continues the measured surface outward,
/// so the seam is C0 by construction — the fill equals the boundary sample it
/// came from, and there is no step to see. It does not invent relief (the
/// margin is flat in the direction of extension, which is honest: we have no
/// data there), it only refuses to invent a cliff.
///
/// BFS gives exact 4-connected nearest for free and touches each cell once, so
/// this is O(w·h) with no distance transform to get wrong. Ties resolve by
/// visit order, which is deterministic — the oracle's purity depends on this
/// being a pure function of the raster.
fn fill_nodata_from_nearest(heights: &mut [f64], w: usize, h: usize) {
    // Seed the frontier with every measured cell that touches a hole.
    let mut queue: std::collections::VecDeque<usize> =
        (0..heights.len()).filter(|&i| heights[i].is_finite()).collect();
    if queue.is_empty() || queue.len() == heights.len() {
        return;
    }
    while let Some(i) = queue.pop_front() {
        let v = heights[i];
        let (x, y) = (i % w, i / w);
        let mut visit = |nx: usize, ny: usize, q: &mut std::collections::VecDeque<usize>| {
            let n = ny * w + nx;
            if !heights[n].is_finite() {
                heights[n] = v;
                q.push_back(n);
            }
        };
        if x > 0 {
            visit(x - 1, y, &mut queue);
        }
        if x + 1 < w {
            visit(x + 1, y, &mut queue);
        }
        if y > 0 {
            visit(x, y - 1, &mut queue);
        }
        if y + 1 < h {
            visit(x, y + 1, &mut queue);
        }
    }
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
        let side = largest_measured_centred_square(&g, 6, 6);
        assert_eq!(side, 2, "must shrink about the CENTRE, not slide left");
        let cropped = crop_centred(&g, 6, side);
        assert_eq!(cropped.len(), side * side);
        assert!(cropped.iter().all(|v| v.is_finite()), "trimmed grid is hole-free");
        g.clear();
    }

    /// A fully-measured raster must not be trimmed at all.
    #[test]
    fn clean_raster_is_not_trimmed() {
        let g = vec![5.0f64; 16];
        assert_eq!(largest_measured_centred_square(&g, 4, 4), 4);
    }

    /// The nodata margin must not become a cliff: filling it with the crop's
    /// global minimum put a kilometres-tall wall around the map edge. The fill
    /// must equal the nearest measured sample, so the seam has no step.
    #[test]
    fn nodata_margin_is_extended_not_stepped_to_the_minimum() {
        // 4×4: a low pit at one corner, a plateau at 1000, and a nodata margin.
        let n = f64::NAN;
        let mut g = vec![
            -4000.0, 1000.0, 1000.0, n, //
            1000.0, 1000.0, 1000.0, n, //
            1000.0, 1000.0, 1000.0, n, //
            n, n, n, n,
        ];
        fill_nodata_from_nearest(&mut g, 4, 4);
        assert!(g.iter().all(|v| v.is_finite()), "no holes left");
        // Every filled cell took its neighbouring plateau value, NOT the -4000 min.
        for (i, v) in g.iter().enumerate() {
            assert!(*v >= 1000.0 || i == 0, "cell {i} = {v}: fill must not import the pit");
        }
        // The seam is C0: the filled cell equals the measured one beside it.
        assert_eq!(g[3], g[2]);
    }

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

    /// A raster stamped MOON_ME reads back MOON_ME; one that declares nothing
    /// reads back `None`. ME vs PA is ≈ 875 m of silent offset, so an unknown
    /// frame must stay unknown rather than default to the likely answer.
    #[test]
    fn lunar_frame_survives_the_raster_or_stays_unknown() {
        use lunco_geotiff::LunarFrame;

        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut buf).unwrap();
            let geo = lunco_geotiff::GeoTransform::centred_square(
                2.0, 2, 1737.0e3, 26.0371, 3.6584,
            )
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
