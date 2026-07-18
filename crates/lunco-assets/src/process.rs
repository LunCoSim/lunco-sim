//! Texture processing — resize and convert source images to cached textures.
//!
//! Processing is configured in `Assets.toml` via the `[name.process]` section:
//!
//! ```toml
//! [earth]
//! url = "https://..."
//! dest = "textures/earth_source.jpg"
//!
//! [earth.process]
//! target_resolution = [4096, 2048]
//! output = "textures/earth.png"
//! ```

use std::path::Path;
use image::GenericImageView;
#[cfg(not(target_arch = "wasm32"))]
use resvg::tiny_skia;
#[cfg(not(target_arch = "wasm32"))]
use usvg::{Tree, Options};
use crate::cache_dir;

/// Processing configuration from `Assets.toml`.
///
/// `kind` selects the pipeline (default `"texture"`); other fields apply
/// per pipeline:
///
/// - `kind = "texture"` (default): resize an image to `target_resolution`
///   and re-encode as PNG. Used by Earth/Moon textures.
/// - `kind = "gltf"`: run a fixed `gltf-transform` cleanup pipeline on a
///   downloaded `.glb` to strip extensions Bevy 0.18's `bevy_gltf` doesn't
///   support. Currently: `KHR_draco_mesh_compression` (geometry) and
///   `EXT_texture_webp` (textures, re-encoded as PNG). Requires `npx`
///   (Node.js) on PATH; the CLI is fetched on demand.
/// - `kind = "dem"`: crop a square region-of-interest out of a raw LROC/NAC
///   DTM (a non-square float32 raster) and re-encode it as the **square**
///   float32 `heightmap.tif` the runtime DEM reader expects, georeferenced
///   with GeoTIFF tags. Pure-Rust (no GDAL) — the DTM's
///   equirectangular projection is just arithmetic once the manifest
///   supplies its scale + center (see the `dem_*` fields below).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProcessConfig {
    /// Pipeline selector. Defaults to `"texture"` for backwards
    /// compatibility with Earth/Moon entries.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Target [width, height] in pixels (texture pipeline only).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    #[serde(default)]
    pub target_resolution: Option<[u32; 2]>,
    /// Output path **relative to** [`ProcessConfig::output_root`].
    /// Examples: `"textures/earth.png"`, `"models/perseverance.glb"`,
    /// `"terrain/apollo15"` (a folder, for the `dem` pipeline — the
    /// heightmap lands at `<output>/materials/textures/heightmap.tif`).
    pub output: String,
    /// Where to write the processed file:
    /// - `"cache"` (default) — writes under the shared cache root
    ///   (`<LUNCOSIM_CACHE>/...`). Used by Earth/Moon textures and
    ///   other regeneratable artifacts that don't need to live in
    ///   the source tree.
    /// - `"assets"` — writes under the workspace `assets/` directory
    ///   (gitignored if the path matches a `.gitignore` rule). Used
    ///   for files USD `payload`/`references` need to find via
    ///   layer-relative paths — Bevy's default `assets://` source
    ///   resolves them, and so does Blender / usdview / Houdini.
    /// - `"twin"` — writes into a **Twin folder** whose root is supplied
    ///   by the caller (the CLI's `--twin <DIR>` flag, threaded through
    ///   [`process_texture`]'s `twin_root` arg). This is what makes a
    ///   standalone Twin (which is not a workspace crate, e.g. a school
    ///   project on disk) able to download + process its own assets
    ///   in place. `output` is interpreted relative to that root.
    #[serde(default = "default_output_root")]
    pub output_root: String,

    // ── `kind = "dem"` fields ────────────────────────────────────────────
    // The runtime DEM reader needs the DTM's *projection* to turn the
    // author's geographic ROI (center + window) into source pixels. Raw LROC
    // PDS TIFFs carry no GeoTIFF tags, so these come from the manifest (the
    // values for the Apollo 15 NAC DTM are in its PDS3 `.LBL`). For an
    // equirectangular mosaic, pixel→metre is `pixel_scale_m` and
    // lat/lon→pixel is a linear affine using `center_lat`/`center_lon` +
    // `pixel_scale_m` over the body radius — no general reprojection.
    /// Center latitude of the square ROI to crop (degrees, planetocentric).
    #[serde(default)]
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub center_lat: Option<f64>,
    /// Center longitude of the square ROI to crop (degrees, East-positive).
    #[serde(default)]
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub center_lon: Option<f64>,
    /// Side length of the square ROI in metres (the crop window). The DTM
    /// is sampled across this many metres and re-encoded at
    /// `target_resolution` × `target_resolution` samples.
    #[serde(default)]
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub window_m: Option<f64>,
    /// DTM source projection: metres per source pixel (e.g. `2.0` for the
    /// 2 m/px Apollo 15 NAC mosaic). Used to convert the ROI to a source-
    /// pixel window. Defaults to `2.0` (the recommended mosaic).
    #[serde(default = "default_dem_pixel_scale_m")]
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub pixel_scale_m: f64,
    /// Geographic extent of the SOURCE DTM, used to map the author's ROI
    /// (center + window) onto source pixels. These are the `MIN/MAX_LATITUDE`
    /// / `EASTERNMOST/WESTERNMOST_LONGITUDE` values from the DTM's PDS3
    /// label (NOT `CENTER_LONGITUDE`, which on some LROC labels carries a
    /// body-frame quirk inconsistent with the actual extent). All four must
    /// be set for `kind = "dem"`; a 2-point affine from the extent corners
    /// to the raster edges makes the projection self-consistent for any
    /// equirectangular mosaic regardless of longitude convention.
    #[serde(default)]
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub src_min_lat: Option<f64>,
    #[serde(default)]
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub src_max_lat: Option<f64>,
    #[serde(default)]
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub src_min_lon: Option<f64>,
    #[serde(default)]
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub src_max_lon: Option<f64>,
    /// Site identity. The runtime takes this from the DEM folder name; this
    /// field remains for manifests that name the site explicitly.
    #[serde(default)]
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub site_id: Option<String>,
}

fn default_kind() -> String {
    "texture".to_string()
}

fn default_output_root() -> String {
    "cache".to_string()
}

fn default_dem_pixel_scale_m() -> f64 {
    2.0
}

/// Processes a single source asset according to `process.kind`.
///
/// - `"texture"` (default): resize an image to `target_resolution` and
///   save as PNG. Supports JPEG, PNG, TIFF, BMP, WebP, SVG inputs.
/// - `"gltf"`: clean a `.glb` for Bevy 0.18 — decode Draco geometry,
///   re-encode WebP textures as PNG. Shells out to `npx
///   @gltf-transform/cli`; the function name `process_texture` is a
///   historical accident at this point but kept to avoid churning the
///   call site.
/// - `"dem"`: crop a square ROI from a raw LROC/NAC DTM and write the
///   square, georeferenced float32 `heightmap.tif` the runtime DEM reader
///   expects. `output` is a **folder** (the `demSource` target);
///   the heightmap lands at `<output>/materials/textures/heightmap.tif`.
///
/// `twin_root` resolves `output_root = "twin"` against a caller-supplied
/// Twin folder (the CLI's `--twin <DIR>`); `None` falls back to cache for
/// the `"cache"` / unrecognised roots (see [`ProcessConfig::output_root`]).
#[cfg(not(target_arch = "wasm32"))]
pub fn process_texture(
    source_path: &Path,
    process: &ProcessConfig,
    twin_root: Option<&Path>,
) -> Result<(), std::io::Error> {
    // Resolve the output path against the configured root.
    let output_path = match process.output_root.as_str() {
        "assets" => {
            // Workspace-rooted: writes under <workspace>/assets/<output>.
            // We resolve the workspace root by walking up from the
            // crate manifest dir (set at compile time) — same heuristic
            // as `cache_dir`'s fallback walk. Falls back to a CWD-relative
            // `assets/` if the manifest dir isn't useful.
            let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let mut current = Some(manifest);
            let mut ws_root = None;
            for _ in 0..10 {
                if let Some(dir) = &current {
                    if dir.join("assets").is_dir() && dir.join("Cargo.toml").is_file() {
                        ws_root = Some(dir.clone());
                        break;
                    }
                    current = dir.parent().map(std::path::PathBuf::from);
                }
            }
            ws_root
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("assets")
                .join(&process.output)
        }
        "twin" => {
            // Caller-supplied Twin folder root (the CLI's --twin flag).
            // Without it there's nowhere Twin-relative to write — fall back
            // to the cache so a manifest authored for a twin doesn't hard-
            // fail when run outside one.
            let root = twin_root
                .map(std::path::PathBuf::from)
                .unwrap_or_else(cache_dir);
            root.join(&process.output)
        }
        _ => cache_dir().join(&process.output),
    };

    // Create output directory if needed
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    match process.kind.as_str() {
        "gltf" => process_gltf(source_path, &output_path)?,
        "dem" => process_dem(source_path, &output_path, process)?,
        "texture" => {
            let [tw, th] = process.target_resolution.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "texture pipeline requires `target_resolution = [w, h]`",
                )
            })?;
            let ext = source_path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            match ext {
                "svg" => process_svg(source_path, &output_path, tw, th)?,
                "jpg" | "jpeg" | "png" | "tiff" | "tif" | "bmp" | "webp" => {
                    process_image(source_path, &output_path, tw, th)?
                }
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("Unsupported source format: .{}", ext),
                    ))
                }
            }
        }
        other => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "Unknown process kind `{}` (expected \"texture\", \"gltf\", or \"dem\")",
                    other
                ),
            ))
        }
    }

    println!("  ✓ processed → {}", output_path.display());
    Ok(())
}

/// glb cleanup pipeline. Runs `gltf-transform` twice in series:
///
/// 1. **`copy`** — re-emits the file, decoding `KHR_draco_mesh_compression`
///    transparently along the way. Bevy 0.18's `bevy_gltf` has no Draco
///    decoder; this strips the extension.
/// 2. **`png --formats "*"`** — re-encodes every embedded texture as PNG,
///    irrespective of source format (WebP, JPEG, PNG). Drops
///    `EXT_texture_webp` since none of the resulting textures need it.
///
/// Shells out to `npx --yes @gltf-transform/cli`. Node.js / `npx` must be
/// on `PATH`; the gltf-transform CLI itself is fetched by `npx` on first
/// run and cached. Native-only — wasm builds skip this whole module.
#[cfg(not(target_arch = "wasm32"))]
fn process_gltf(source: &Path, output: &Path) -> std::io::Result<()> {
    use std::process::Command;

    // Resolve the absolute path once, rather than spawning the bare name
    // `npx`. On Windows the executable is `npx.cmd` (a batch shim), which
    // `Command::new("npx")` won't find — `which` honors `PATHEXT` and
    // returns the real `npx.cmd`, which modern std launches via cmd.exe.
    let npx = which::which("npx").map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "gltf process step requires Node.js / `npx` on PATH (install Node 18+, then re-run)",
        )
    })?;

    // Stage 1 → temp file. We deliberately use a temp intermediate
    // rather than rewriting `source` so a failed second stage doesn't
    // leave the source corrupted, and so re-running `process` is
    // idempotent (the source is the immutable Assets.toml-pinned blob).
    let tmp = crate::temp_dir().join(format!(
        "gltf_decoded_{}.glb",
        std::process::id()
    ));

    let s1 = Command::new(&npx)
        .args(["--yes", "@gltf-transform/cli", "copy"])
        .arg(source)
        .arg(&tmp)
        .status()?;
    if !s1.success() {
        return Err(std::io::Error::other(format!(
            "gltf-transform copy failed: exit {:?}",
            s1.code()
        )));
    }

    let s2 = Command::new(&npx)
        .args(["--yes", "@gltf-transform/cli", "png"])
        .arg(&tmp)
        .arg(output)
        .args(["--formats", "*"])
        .status()?;
    let _ = std::fs::remove_file(&tmp);
    if !s2.success() {
        return Err(std::io::Error::other(format!(
            "gltf-transform png failed: exit {:?}",
            s2.code()
        )));
    }

    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn process_svg(source: &Path, output: &Path, tw: u32, th: u32) -> Result<(), std::io::Error> {
    let svg_data = std::fs::read(source)?;
    let opt = Options::default();
    let tree = Tree::from_data(&svg_data, &opt)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

    let size = tree.size();
    let mut pixmap = tiny_skia::Pixmap::new(tw, th)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid resolution"))?;

    let transform = tiny_skia::Transform::from_scale(
        tw as f32 / size.width(),
        th as f32 / size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    pixmap.save_png(output)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
}

#[cfg(not(target_arch = "wasm32"))]
fn process_image(source: &Path, output: &Path, tw: u32, th: u32) -> Result<(), std::io::Error> {
    let img = image::open(source)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

    let (w, h) = img.dimensions();
    let processed = if w != tw || h != th {
        img.resize_exact(tw, th, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    // Color textures must land as 8-bit: bevy tags 8-bit PNGs sRGB but loads
    // 16-bit ones LINEAR, so a 16-bit sRGB source (the LROC moon map) skips
    // gamma decode in the engine and renders washed-out white. 8 bits is
    // plenty for albedo, and it quarters the file.
    let processed = image::DynamicImage::ImageRgb8(processed.to_rgb8());

    processed.save(output)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
}

/// `kind = "dem"` pipeline — produce a runtime-loadable terrain site folder.
///
/// Reads a raw LROC/NAC DTM (a non-square float32 raster with no GeoTIFF
/// tags), crops the **square** region-of-interest the manifest specifies
/// (`center_lat`/`center_lon` + `window_m`), resamples it to a square
/// float32 raster, and writes the two files the runtime DEM reader
/// (`lunco-terrain-surface/src/terrain.rs`) looks up under a `demSource`
/// folder reference:
///
/// - `<output>/materials/textures/heightmap.tif` — square float32 GeoTIFF,
///   elevation in metres. The reader rejects non-square rasters
///   (`lunco-terrain-bake/src/dem.rs:148`), so the output is forced square
///   even when the source ROI is anisotropic.
///   (`DemMetadata::from_yaml_str`): `site_id`, `resolution_x`/`y`,
///   `size_x_m`/`y_m`, `elevation_min`/`max_m`, `coordinates`.
///
/// No GDAL: the DTM's equirectangular projection is closed-form. With the
/// mosaic true at `proj_center_lat`/`proj_center_lon`, `pixel_scale_m`
/// metres per source pixel, and a spherical body of `body_radius_m`, a
/// geographic `(lat, lon)` maps to source pixel
/// `(col, row)` by:
///
/// ```text
/// col = src_w/2 + (lon - proj_center_lon)·cos(proj_center_lat)·R / scale
/// row = src_h/2 - (lat - proj_center_lat)·R               / scale
/// ```
///
/// (longitude shrinks by `cos(lat)`; row grows downward — PDS rasters are
/// north-up.) For a ≤2 km site the accumulated scale error from ignoring
/// second-order terms is well under a pixel and irrelevant to the sim.
#[cfg(not(target_arch = "wasm32"))]
fn process_dem(
    source: &Path,
    output_dir: &Path,
    cfg: &ProcessConfig,
) -> Result<(), std::io::Error> {
    use std::io::Cursor;

    // ── Decode the source DTM once (it can be 100+ MB). ───────────────────
    // LROC mosaics ship as a single giant strip (e.g. the 2 m/px Apollo 15 DTM
    // is a 2555×14311 float32 raster = ~146 MB in one strip), which blows past
    // the `tiff` crate's default 128 MB `intermediate_buffer_size`. This is an
    // offline build-time tool decoding ONE known-large raster into memory, so
    // `Limits::unlimited()` is appropriate — it is NOT on a wasm/page hot path.
    let bytes = std::fs::read(source)?;
    let mut dec = tiff::decoder::Decoder::new(Cursor::new(bytes.as_slice()))
        .map_err(|e| io_err(format!("decoding DTM TIFF: {e}")))?
        .with_limits(tiff::decoder::Limits::unlimited());
    let (src_w, src_h) = dec
        .dimensions()
        .map_err(|e| io_err(format!("reading DTM dimensions: {e}")))?;
    let (src_w, src_h) = (src_w as usize, src_h as usize);
    use tiff::decoder::DecodingResult as D;
    let heights_f64: Vec<f64> = match dec.read_image().map_err(|e| io_err(format!("reading DTM pixels: {e}")))? {
        D::F32(v) => v.into_iter().map(|x| x as f64).collect(),
        D::F64(v) => v,
        D::U8(v) => v.into_iter().map(|x| x as f64).collect(),
        D::U16(v) => v.into_iter().map(|x| x as f64).collect(),
        D::I16(v) => v.into_iter().map(|x| x as f64).collect(),
        D::U32(v) => v.into_iter().map(|x| x as f64).collect(),
        D::I32(v) => v.into_iter().map(|x| x as f64).collect(),
        _ => return Err(io_err("unsupported DTM sample format (need numeric Gray)".into())),
    };

    // ── Resolve the ROI window (source pixels). ───────────────────────────
    let center_lat = cfg
        .center_lat
        .ok_or_else(|| io_err("dem pipeline requires `center_lat`".into()))?;
    let center_lon = cfg
        .center_lon
        .ok_or_else(|| io_err("dem pipeline requires `center_lon`".into()))?;
    let window_m = cfg
        .window_m
        .ok_or_else(|| io_err("dem pipeline requires `window_m`".into()))?;
    let scale = cfg.pixel_scale_m.max(1e-6); // metres per source pixel
    let half_px = (window_m * 0.5 / scale).round() as isize;

    // Map the ROI center to a source pixel via a 2-point affine from the
    // DTM's geographic extent (its PDS3 MIN/MAX_LAT, EASTERNMOST/WESTERNMOST
    // LON) to the raster edges. This is self-consistent for any
    // equirectangular mosaic and sidesteps the unreliable `CENTER_LONGITUDE`
    // some LROC labels carry. Requires all four extent values.
    let min_lat = cfg
        .src_min_lat
        .ok_or_else(|| io_err("dem pipeline requires `src_min_lat`".into()))?;
    let max_lat = cfg
        .src_max_lat
        .ok_or_else(|| io_err("dem pipeline requires `src_max_lat`".into()))?;
    let min_lon = cfg
        .src_min_lon
        .ok_or_else(|| io_err("dem pipeline requires `src_min_lon`".into()))?;
    let max_lon = cfg
        .src_max_lon
        .ok_or_else(|| io_err("dem pipeline requires `src_max_lon`".into()))?;
    // North-up: max_lat → row 0, min_lat → row (h-1). Lon grows with column.
    let lon_span = (max_lon - min_lon).abs().max(1e-9);
    let lat_span = (max_lat - min_lat).abs().max(1e-9);
    let center_col =
        ((center_lon - min_lon) / lon_span) * (src_w as f64 - 1.0);
    let center_row =
        ((max_lat - center_lat) / lat_span) * (src_h as f64 - 1.0);
    let cc = center_col.round() as isize;
    let cr = center_row.round() as isize;

    // Clamp the square window to the source; if the author's window falls
    // off the edge, shrink it (still square) rather than emit nodata rows —
    // a smaller-than-asked real surface beats a half-nodata one.
    let max_half_w = cc.max(0).min(src_w as isize - 1);
    let max_half_e = (src_w as isize - 1 - cc).max(0);
    let max_half_n = cr.max(0).min(src_h as isize - 1);
    let max_half_s = (src_h as isize - 1 - cr).max(0);
    let half = half_px
        .min(max_half_w)
        .min(max_half_e)
        .min(max_half_n)
        .min(max_half_s)
        .max(1);
    let win = (2 * half + 1) as usize; // square source window side length
    let x0 = (cc - half).max(0) as usize;
    let y0 = (cr - half).max(0) as usize;
    if (half as f64) < (half_px as f64) * 0.9 {
        eprintln!(
            "  ⚠ dem: requested {:.0} m window but only ~{:.0} m fit inside the \
             source at ({}, {}) — crop shrunk to stay square.",
            window_m,
            win as f64 * scale,
            center_lat,
            center_lon
        );
    }

    // ── Resample the window to the target resolution (forced square). ─────
    // `target_resolution` may be [n, n] or just [n]; we take the first
    // component as the square side. Default to the source window's own
    // resolution if unset (a 1:1 square crop).
    let out_n = cfg
        .target_resolution
        .map(|[w, _]| w.max(1) as usize)
        .unwrap_or(win);
    let mut out = vec![0.0f32; out_n * out_n];
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for oy in 0..out_n {
        // Source row (nearest at the window's centre; bilinear inside).
        let sy_f = y0 as f64 + (oy as f64 / (out_n - 1).max(1) as f64) * (win - 1) as f64;
        let sy0 = sy_f.floor() as usize;
        let sy1 = (sy0 + 1).min(src_h - 1);
        let fy = sy_f - sy0 as f64;
        for ox in 0..out_n {
            let sx_f = x0 as f64 + (ox as f64 / (out_n - 1).max(1) as f64) * (win - 1) as f64;
            let sx0 = sx_f.floor() as usize;
            let sx1 = (sx0 + 1).min(src_w - 1);
            let fx = sx_f - sx0 as f64;
            // Bilinear over the four neighbours; nodata/NaN treated as 0.
            let s = |col: usize, row: usize| -> f64 {
                heights_f64.get(row * src_w + col).copied().unwrap_or(0.0)
            };
            let v00 = s(sx0, sy0);
            let v10 = s(sx1, sy0);
            let v01 = s(sx0, sy1);
            let v11 = s(sx1, sy1);
            let top = v00 + (v10 - v00) * fx;
            let bot = v01 + (v11 - v01) * fx;
            let mut v = top + (bot - top) * fy;
            if !v.is_finite() {
                v = 0.0;
            }
            if v < min {
                min = v;
            }
            if v > max {
                max = v;
            }
            out[oy * out_n + ox] = v as f32;
        }
    }
    if !min.is_finite() {
        min = 0.0;
    }
    if !max.is_finite() {
        max = 0.0;
    }

    // ── Write the square float32 heightmap. ───────────────────────────────
    let tex_dir = output_dir.join("materials").join("textures");
    std::fs::create_dir_all(&tex_dir)?;
    let tif_path = tex_dir.join("heightmap.tif");
    {
        use tiff::encoder::{colortype, TiffEncoder};
        let mut enc =
            TiffEncoder::new(std::fs::File::create(&tif_path)?).map_err(tiff_io_err)?;

        // The GEO half — without it QGIS opens the raster in pixel units and every
        // slope computed from it is wrong by the ground-sample factor, silently.
        //
        // `win * scale` is the true on-the-ground span, and the frame is
        // node-based: sample 0 on the west/north edge, sample n-1 on the east/south.
        // Body radius, for the GeoTIFF citation only — it does not enter the
        // pixel→metre mapping, which is a local metric frame.
        //
        // ⚠ THE ENGINE HAS NO CANONICAL MOON RADIUS. Three values are in the tree
        // and they disagree: `1737.0e3` (`lunco-celestial/src/registry.rs:178`,
        // the one the sim actually places bodies with), `1.7374e6`
        // (`lunco-sandbox/src/ui/mod.rs:363`) and `1_737_400.0`
        // (`lunco-celestial/tests/terrain_curvature_determinism.rs:19`). The 400 m
        // spread is a real altitude bias the moment anything prints a height —
        // flagged as an open decision in the school twin's driver-UI doc.
        //
        // We follow the registry, because a raster should describe the body the
        // simulation puts it on, not a more defensible number. Reconcile the three
        // and this follows.
        const BODY_RADIUS_M: f64 = 1737.0e3;
        let geo = lunco_geotiff::GeoTransform::centred_square(
            win as f64 * scale,
            out_n,
            BODY_RADIUS_M,
            center_lat,
            center_lon,
        );
        let mut img = enc
            .new_image::<colortype::Gray32Float>(out_n as u32, out_n as u32)
            .map_err(tiff_io_err)?;
        lunco_geotiff::write_geo_tags(img.encoder(), &geo, "Moon 2000")
            .map_err(tiff_io_err)?;
        img.write_data(&out).map_err(tiff_io_err)?;
    }

    // No sidecar. Extent, resolution, centre lat/lon and body radius live in the
    // raster's geo tags; source URL and checksum in `Assets.toml`; site id in the
    // folder name. See `docs/architecture/57-dem-georeferencing.md`.
    let _ = (min, max);

    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn io_err(msg: String) -> std::io::Error {
    std::io::Error::other(msg)
}

#[cfg(not(target_arch = "wasm32"))]
fn tiff_io_err(e: tiff::TiffError) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Encode a `w*h` row-major f32 raster as an in-memory TIFF — the same
    /// proven pattern `lunco-terrain-bake` uses for its fixtures.
    fn encode_tiff_f32(w: u32, h: u32, data: &[f32]) -> Vec<u8> {
        use tiff::encoder::{colortype, TiffEncoder};
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut buf).unwrap();
            enc.write_image::<colortype::Gray32Float>(w, h, data).unwrap();
        }
        buf.into_inner()
    }

    /// `kind = "dem"` must turn a **non-square** source DTM into a **square**
    /// georeferenced float32 `heightmap.tif`. This is the
    /// one invariant the runtime DEM reader enforces (`w == h`) and the
    /// exact reason the crop step exists — so assert it directly.
    #[test]
    fn dem_process_crops_non_square_to_square() {
        // 12 wide × 8 tall non-square source; a gradient so bilinear has
        // something distinct to sample. row-major: v = row*12 + col.
        let (sw, sh) = (12u32, 8u32);
        let src: Vec<f32> = (0..sh)
            .flat_map(|r| (0..sw).map(move |c| (r * sw + c) as f32 * 10.0))
            .collect();
        let tif = encode_tiff_f32(sw, sh, &src);

        let tmp = std::env::temp_dir().join(format!(
            "lunco-assets-dem-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let src_path = tmp.join("source.tif");
        std::fs::write(&src_path, &tif).unwrap();

        // Center the ROI in the middle of the source; window small enough to
        // fit. The 2-point extent affine (extent corners → raster edges) places
        // the center. pixel_scale_m=2 so an 8 m window = 4 px half → 9px square.
        let cfg = ProcessConfig {
            kind: "dem".into(),
            output: "site".into(),
            output_root: "cache".into(),
            target_resolution: Some([4, 4]),
            center_lat: Some(0.0),
            center_lon: Some(0.0),
            window_m: Some(8.0), // 8 m ÷ 2 m/px = 4 px half → 9px square window
            pixel_scale_m: 2.0,
            // Source extent: lat/lon each span [-1, 1] over the 12×8 raster, so
            // (0,0) maps to the centre column/row.
            src_min_lat: Some(-1.0),
            src_max_lat: Some(1.0),
            src_min_lon: Some(-1.0),
            src_max_lon: Some(1.0),
            site_id: Some("testsite".into()),
        };
        let out_dir = tmp.join("site");
        process_dem(&src_path, &out_dir, &cfg).expect("dem process should succeed");

        // heightmap is square float32.
        let out_bytes = std::fs::read(out_dir.join("materials/textures/heightmap.tif")).unwrap();
        let mut dec = tiff::decoder::Decoder::new(Cursor::new(out_bytes.as_slice())).unwrap();
        let (w, h) = dec.dimensions().unwrap();
        assert_eq!(w, 4, "output width is the target");
        assert_eq!(h, 4, "output height equals width (SQUARE)");
        match dec.read_image().unwrap() {
            tiff::decoder::DecodingResult::F32(v) => {
                assert_eq!(v.len(), 16);
                // Values are a resampled slice of the gradient — all finite,
                // and within the source's min..max range (0..880).
                assert!(v.iter().all(|x| x.is_finite()));
                assert!(v.iter().all(|x| (*x as f64) >= -1.0 && (*x as f64) <= 900.0));
            }
            other => panic!("expected F32 heightmap, got {other:?}"),
        }

        // metadata.yaml parses and carries the essentials the loader needs.
        let meta = std::fs::read_to_string(out_dir.join("metadata.yaml")).unwrap();
        assert!(meta.contains("site_id: testsite"));
        assert!(meta.contains("resolution_x: 4"));
        assert!(meta.contains("size_x_m:"));
        // elevation min/max must be present and ordered.
        assert!(meta.contains("elevation_min_m:"));
        assert!(meta.contains("elevation_max_m:"));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
