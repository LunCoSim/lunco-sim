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
///   The source may be a TIFF **or a PDS3 `.IMG`** (attached or detached
///   label — see [`crate::pds_img`]); for a PDS source the label's own
///   extent/scale serve as fallbacks for absent `src_*` fields.
/// - `kind = "map"`: crop the **same geographic ROI** the `dem` pipeline
///   uses out of a co-registered raster (ortho `.IMG`, `_SHADE`/`_SLOPE`/
///   `_CLRGRAD` TIFFs) and write an 8-bit PNG at `output` — the file a
///   terrain Material network's `asset inputs:<role>_map` points at.
///   Grayscale sources (NAC orthos are radiance floats) get a 1–99
///   percentile stretch; RGB sources crop as-is.
/// - `kind = "normalmap"`: crop + resample like `dem`, then derive a
///   world-space normal map PNG from the heights (RGB = `n*0.5+0.5`,
///   the encoding `terrain_layered.wgsl` decodes — same convention as
///   `lunco-terrain-core`'s derived bake: `normalize(-dh/dx, 1, -dh/dz)`).
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
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
    ///   [`process_asset`]'s `twin_root` arg). This is what makes a
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
    /// Lunar reference frame of the SOURCE product's coordinates —
    /// `"MOON_ME"` for anything LROC/LOLA-derived, `"MOON_PA"` for
    /// ephemeris-frame data. Stamped into the heightmap's GeoTIFF tags as
    /// provenance. Optional: a manifest that does not know its source's frame
    /// declares nothing, and every reader sees *unknown* — never a guess
    /// (ME↔PA disagree by ≈ 875 m on the surface).
    #[serde(default)]
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub frame: Option<String>,
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
///   @gltf-transform/cli`.
/// - `"dem"`: crop a square ROI from a raw LROC/NAC DTM and write the
///   square, georeferenced float32 `heightmap.tif` the runtime DEM reader
///   expects. `output` is a **folder** (the `demSource` target);
///   the heightmap lands at `<output>/materials/textures/heightmap.tif`.
/// - `"map"` / `"normalmap"`: co-registered ROI crops — see the
///   [`ProcessConfig`] kind list.
///
/// `twin_root` is the caller-supplied Twin folder (the CLI's `--twin <DIR>`).
/// `output_root = "twin"` resolves against it directly; `"cache"` resolves
/// against that Twin's OWN cache, so a processed product stays inside the
/// folder that declared it. Without a Twin, both fall back to the shared cache.
#[cfg(not(target_arch = "wasm32"))]
pub fn process_asset(
    source_path: &Path,
    process: &ProcessConfig,
    twin_root: Option<&Path>,
) -> Result<(), std::io::Error> {
    let owner_cache = twin_root.map(crate::twin_cache_dir);
    let output_path = process_output_path(process, owner_cache.as_deref(), twin_root);

    // Create output directory if needed
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    process_asset_to(source_path, process, &output_path)
}

/// Where a `[*.process]` step writes its product — the ONE resolver, so the
/// step that WRITES the artifact and the registry that reports whether it
/// EXISTS can never disagree about the path.
///
/// `cache_root` is the cache of whoever DECLARED the asset — a Twin's own
/// `.cache` for a Twin manifest, the shared pool for a crate's — and is what
/// the default `output_root = "cache"` resolves against. The processed product
/// belongs beside the source it was derived from, so a Twin packed into an
/// archive carries its derived textures with it instead of leaving them behind
/// in a machine-global cache the recipient does not have.
///
/// `twin_root` is the Twin FOLDER itself, for `output_root = "twin"` (authored
/// content the Twin ships, not a cache artifact). Both fall back to the shared
/// cache when absent.
#[cfg(not(target_arch = "wasm32"))]
pub fn process_output_path(
    process: &ProcessConfig,
    cache_root: Option<&Path>,
    twin_root: Option<&Path>,
) -> std::path::PathBuf {
    match process.output_root.as_str() {
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
        _ => cache_root
            .map(std::path::PathBuf::from)
            .unwrap_or_else(cache_dir)
            .join(&process.output),
    }
}

/// The body of [`process_asset`] once its output path is known.
#[cfg(not(target_arch = "wasm32"))]
fn process_asset_to(
    source_path: &Path,
    process: &ProcessConfig,
    output_path: &Path,
) -> Result<(), std::io::Error> {
    let output_path = output_path.to_path_buf();

    // ── Bake-key staleness check ──────────────────────────────────────────
    // The processed output is a pure function of (source bytes, this config,
    // pipeline version). Content-address it: a stamp beside the output holds
    // the key of the bake that produced it, and a matching key skips the
    // whole decode (the expensive part — a big mosaic decodes to GBs of f64).
    // Anything that could change the result — new source, edited ROI, a
    // pipeline fix (bump PIPELINE_VERSION) — changes the key and rebakes.
    // Never time-based: a cache that can't go stale beats one that expires.
    let stamp_path = bake_stamp_path(&output_path);
    let key = bake_key(source_path, process)?;
    if std::fs::read_to_string(&stamp_path).is_ok_and(|s| s.trim() == key) {
        println!("  ✓ up-to-date (bake key match) → {}", output_path.display());
        return Ok(());
    }

    match process.kind.as_str() {
        "gltf" => process_gltf(source_path, &output_path)?,
        "dem" => process_dem(source_path, &output_path, process)?,
        "map" => process_map(source_path, &output_path, process)?,
        "normalmap" => process_normalmap(source_path, &output_path, process)?,
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
                    "Unknown process kind `{}` (expected \"texture\", \"gltf\", \"dem\", \
                     \"map\", or \"normalmap\")",
                    other
                ),
            ))
        }
    }

    // Stamp only after a fully successful bake, so a failed/interrupted run
    // never masquerades as fresh.
    std::fs::write(&stamp_path, &key)?;

    println!("  ✓ processed → {}", output_path.display());
    Ok(())
}

/// Bump when any pipeline's OUTPUT changes for identical inputs (resampling
/// fix, encoding change, new geo tags) — invalidates every stamped bake.
const PIPELINE_VERSION: u32 = 1;

/// Where the bake stamp lives: inside the output folder for folder outputs
/// (`dem`), beside the file for file outputs. Both land under the twin's
/// gitignored terrain artifacts, never in tracked source.
#[cfg(not(target_arch = "wasm32"))]
fn bake_stamp_path(output_path: &Path) -> std::path::PathBuf {
    if output_path.extension().is_none() {
        output_path.join(".bakekey")
    } else {
        let mut name = output_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        name.push_str(".bakekey");
        output_path.with_file_name(name)
    }
}

/// Content-address of a bake: sha256 over the SOURCE BYTES (streamed — a
/// 908 MB mosaic hashes in seconds vs decoding to ~2 GB of f64), the full
/// serialized [`ProcessConfig`], and [`PIPELINE_VERSION`]. Deliberately not
/// size+mtime: mtimes differ across machines and bundle unpacks, and a bake
/// key must mean the same thing on every peer.
#[cfg(not(target_arch = "wasm32"))]
fn bake_key(source: &Path, cfg: &ProcessConfig) -> Result<String, std::io::Error> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut f = std::fs::File::open(source)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = std::io::Read::read(&mut f, &mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let cfg_json = serde_json::to_string(cfg)
        .map_err(|e| io_err(format!("serializing ProcessConfig for bake key: {e}")))?;
    hasher.update(cfg_json.as_bytes());
    hasher.update(PIPELINE_VERSION.to_le_bytes());
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
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
        .map_err(|e| std::io::Error::other(e.to_string()))
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
        .map_err(|e| std::io::Error::other(e.to_string()))
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
    let src = decode_gray_source(source)?;
    let (roi, scale, center_lat, center_lon) = resolve_roi(cfg, &src, "dem")?;
    let (out_n, win) = (roi.out_n, roi.win);
    let heights = resample_roi_bilinear(&src.samples, src.w, src.h, &roi);
    let out: Vec<f32> = heights.iter().map(|&v| v as f32).collect();

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
        let mut geo = lunco_geotiff::GeoTransform::centred_square(
            win as f64 * scale,
            out_n,
            BODY_RADIUS_M,
            center_lat,
            center_lon,
        );
        // Frame provenance: only the manifest can know which lunar frame the
        // source product is in, so a declared `frame` is stamped and an absent
        // one leaves the raster honestly silent. A typo must fail loudly here —
        // writing nothing would silently downgrade a known frame to unknown.
        if let Some(name) = cfg.frame.as_deref() {
            let frame = lunco_geotiff::LunarFrame::parse(name).ok_or_else(|| {
                io_err(format!(
                    "unknown `frame` \"{name}\" (expected \"MOON_ME\" or \"MOON_PA\")"
                ))
            })?;
            geo = geo.with_frame(frame);
        }
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

    Ok(())
}

/// A grayscale source raster decoded for the geographic pipelines, plus the
/// projection facts the container itself supplied. PDS3 labels carry their
/// own extent/scale; raw LROC TIFFs carry nothing (their `.LBL` values go in
/// the manifest instead).
#[cfg(not(target_arch = "wasm32"))]
struct GraySource {
    w: usize,
    h: usize,
    samples: Vec<f64>,
    extent: Option<crate::pds_img::PdsExtent>,
    scale_m: Option<f64>,
    projection: Option<String>,
}

/// Decode a DEM-class source raster to grayscale `f64`: TIFF (any numeric
/// Gray layout) or PDS3 `.IMG` (attached/detached label).
#[cfg(not(target_arch = "wasm32"))]
fn decode_gray_source(source: &Path) -> Result<GraySource, std::io::Error> {
    use std::io::Cursor;

    let ext = source
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "img" {
        let p = crate::pds_img::PdsImage::decode(source)?;
        return Ok(GraySource {
            w: p.width,
            h: p.height,
            samples: p.samples,
            extent: p.extent,
            scale_m: p.map_scale_m,
            projection: p.projection,
        });
    }

    // ── Decode the source TIFF once (it can be 100+ MB). ──────────────────
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
    Ok(GraySource {
        w: src_w,
        h: src_h,
        samples: heights_f64,
        extent: None,
        scale_m: None,
        projection: None,
    })
}

/// A resolved square crop: source-pixel window + output resolution.
#[cfg(not(target_arch = "wasm32"))]
struct RoiCrop {
    x0: usize,
    y0: usize,
    /// Square source window side, in source pixels.
    win: usize,
    /// Output raster side, in samples.
    out_n: usize,
}

/// Resolve the manifest's geographic ROI (center + window) to a source-pixel
/// crop via the 2-point extent affine. Extent and pixel scale come from the
/// manifest's `src_*`/`pixel_scale_m` fields, falling back to what the source
/// container itself declares (PDS3 labels only): the manifest wins when it
/// authors all four extent values; `pixel_scale_m` yields to the label's
/// `MAP_SCALE` when left at its serde default (2.0) — an authored value
/// identical to the default is indistinguishable, so pin the label's value in
/// the manifest if it must be exactly 2.0 against a disagreeing label.
///
/// Longitude convention: the affine is convention-agnostic, but `center_lon`
/// must use the SAME convention as the extent it is resolved against (LROC
/// labels author 0–360 °E).
///
/// Fails loudly on a non-equirectangular source (polar stereographic products
/// need a real projection, not this affine — the known pipeline gate).
#[cfg(not(target_arch = "wasm32"))]
fn resolve_roi(
    cfg: &ProcessConfig,
    src: &GraySource,
    kind: &str,
) -> Result<(RoiCrop, f64, f64, f64), std::io::Error> {
    if let Some(proj) = src.projection.as_deref() {
        if !proj.contains("EQUIRECTANGULAR") {
            return Err(io_err(format!(
                "{kind} pipeline: source declares projection {proj}; only \
                 EQUIRECTANGULAR sources can be cropped with the extent affine \
                 (polar-stereographic products are not yet ingestible)"
            )));
        }
    }

    let center_lat = cfg
        .center_lat
        .ok_or_else(|| io_err(format!("{kind} pipeline requires `center_lat`")))?;
    let center_lon = cfg
        .center_lon
        .ok_or_else(|| io_err(format!("{kind} pipeline requires `center_lon`")))?;
    let window_m = cfg
        .window_m
        .ok_or_else(|| io_err(format!("{kind} pipeline requires `window_m`")))?;
    let scale = if (cfg.pixel_scale_m - default_dem_pixel_scale_m()).abs() > 1e-12 {
        cfg.pixel_scale_m
    } else {
        src.scale_m.unwrap_or(cfg.pixel_scale_m)
    }
    .max(1e-6); // metres per source pixel
    let half_px = (window_m * 0.5 / scale).round() as isize;

    // Map the ROI center to a source pixel via a 2-point affine from the
    // source's geographic extent (its PDS3 MIN/MAX_LAT, EASTERNMOST/
    // WESTERNMOST LON) to the raster edges. This is self-consistent for any
    // equirectangular mosaic and sidesteps the unreliable `CENTER_LONGITUDE`
    // some LROC labels carry.
    let manifest_extent = match (cfg.src_min_lat, cfg.src_max_lat, cfg.src_min_lon, cfg.src_max_lon)
    {
        (Some(a), Some(b), Some(c), Some(d)) => Some((a, b, c, d)),
        _ => None,
    };
    let (min_lat, max_lat, min_lon, max_lon) = manifest_extent
        .or_else(|| src.extent.map(|e| (e.min_lat, e.max_lat, e.west_lon, e.east_lon)))
        .ok_or_else(|| {
            io_err(format!(
                "{kind} pipeline requires the source extent: set all four \
                 `src_min_lat`/`src_max_lat`/`src_min_lon`/`src_max_lon` (from \
                 the product's PDS3 label), or use a PDS3 `.IMG` source that \
                 declares its own IMAGE_MAP_PROJECTION"
            ))
        })?;
    let (src_w, src_h) = (src.w, src.h);
    // North-up: max_lat → row 0, min_lat → row (h-1). Lon grows with column.
    let lon_span = (max_lon - min_lon).abs().max(1e-9);
    let lat_span = (max_lat - min_lat).abs().max(1e-9);
    let center_col = ((center_lon - min_lon) / lon_span) * (src_w as f64 - 1.0);
    let center_row = ((max_lat - center_lat) / lat_span) * (src_h as f64 - 1.0);
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
            "  ⚠ {kind}: requested {:.0} m window but only ~{:.0} m fit inside the \
             source at ({}, {}) — crop shrunk to stay square.",
            window_m,
            win as f64 * scale,
            center_lat,
            center_lon
        );
    }

    // `target_resolution` may be [n, n] or just [n]; we take the first
    // component as the square side. Default to the source window's own
    // resolution if unset (a 1:1 square crop).
    let out_n = cfg
        .target_resolution
        .map(|[w, _]| w.max(1) as usize)
        .unwrap_or(win);
    Ok((RoiCrop { x0, y0, win, out_n }, scale, center_lat, center_lon))
}

/// Resample the crop's source window to `out_n × out_n` (bilinear;
/// nodata/NaN treated as 0 — same policy the DEM pipeline always had).
#[cfg(not(target_arch = "wasm32"))]
fn resample_roi_bilinear(
    samples: &[f64],
    src_w: usize,
    src_h: usize,
    roi: &RoiCrop,
) -> Vec<f64> {
    let (x0, y0, win, out_n) = (roi.x0, roi.y0, roi.win, roi.out_n);
    let mut out = vec![0.0f64; out_n * out_n];
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
                let v = samples.get(row * src_w + col).copied().unwrap_or(0.0);
                if v.is_finite() { v } else { 0.0 }
            };
            let v00 = s(sx0, sy0);
            let v10 = s(sx1, sy0);
            let v01 = s(sx0, sy1);
            let v11 = s(sx1, sy1);
            let top = v00 + (v10 - v00) * fx;
            let bot = v01 + (v11 - v01) * fx;
            let v = top + (bot - top) * fy;
            out[oy * out_n + ox] = if v.is_finite() { v } else { 0.0 };
        }
    }
    out
}

/// `kind = "map"` pipeline — crop a co-registered raster to the same
/// geographic ROI as the site's DEM and write an 8-bit PNG layer map.
///
/// RGB sources (LROC `_SLOPE`/`_CLRGRAD` colour TIFFs) crop as-is. Grayscale
/// sources (`_SHADE`, ortho `.IMG` radiance) get a 1–99 percentile stretch to
/// 8 bits — NAC radiance floats would otherwise land in a few gray levels.
/// Output is always RGB PNG: Bevy tags 8-bit PNGs sRGB, and an R-only gray
/// would sample red in the layered shader's albedo slot.
#[cfg(not(target_arch = "wasm32"))]
fn process_map(
    source: &Path,
    output_path: &Path,
    cfg: &ProcessConfig,
) -> Result<(), std::io::Error> {
    let ext = source
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // RGB path: anything the `image` crate can decode as colour (the LROC
    // derived-map TIFFs are plain 8-bit RGB strips).
    if ext != "img" {
        let mut reader = image::ImageReader::open(source)
            .map_err(|e| io_err(format!("opening map source: {e}")))?
            .with_guessed_format()
            .map_err(|e| io_err(format!("sniffing map source: {e}")))?;
        // Same reasoning as the DEM TIFF decode: one known-large offline
        // raster, not a page hot path.
        reader.no_limits();
        let img = reader
            .decode()
            .map_err(|e| io_err(format!("decoding map source: {e}")))?
            .to_rgb8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        // Per-channel planes as f64 so the shared bilinear resampler applies.
        let mut planes = [
            Vec::with_capacity(w * h),
            Vec::with_capacity(w * h),
            Vec::with_capacity(w * h),
        ];
        for p in img.pixels() {
            planes[0].push(p.0[0] as f64);
            planes[1].push(p.0[1] as f64);
            planes[2].push(p.0[2] as f64);
        }
        let probe = GraySource {
            w,
            h,
            samples: Vec::new(),
            extent: None,
            scale_m: None,
            projection: None,
        };
        let (roi, _scale, _clat, _clon) = resolve_roi(cfg, &probe, "map")?;
        let out_n = roi.out_n;
        let rgb: Vec<Vec<f64>> = planes
            .iter()
            .map(|pl| resample_roi_bilinear(pl, w, h, &roi))
            .collect();
        let mut png = image::RgbImage::new(out_n as u32, out_n as u32);
        for (i, px) in png.pixels_mut().enumerate() {
            for c in 0..3 {
                px.0[c] = rgb[c][i].round().clamp(0.0, 255.0) as u8;
            }
        }
        png.save(output_path)
            .map_err(|e| io_err(format!("writing map PNG: {e}")))?;
        return Ok(());
    }

    // Grayscale path (PDS `.IMG` orthos: single-band radiance).
    let src = decode_gray_source(source)?;
    let (roi, _scale, _clat, _clon) = resolve_roi(cfg, &src, "map")?;
    let out_n = roi.out_n;
    let gray = resample_roi_bilinear(&src.samples, src.w, src.h, &roi);

    // 1–99 percentile stretch over the CROP (not the whole mosaic — the crop
    // is the scene, and mosaic-wide outliers would flatten its contrast).
    let mut sorted: Vec<f64> = gray.iter().copied().filter(|v| v.is_finite() && *v != 0.0).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let (lo, hi) = if sorted.is_empty() {
        (0.0, 1.0)
    } else {
        let lo = sorted[(sorted.len() - 1) * 1 / 100];
        let hi = sorted[(sorted.len() - 1) * 99 / 100];
        if (hi - lo).abs() < f64::EPSILON { (lo, lo + 1.0) } else { (lo, hi) }
    };
    let mut png = image::RgbImage::new(out_n as u32, out_n as u32);
    for (i, px) in png.pixels_mut().enumerate() {
        let v = (((gray[i] - lo) / (hi - lo)).clamp(0.0, 1.0) * 255.0).round() as u8;
        px.0 = [v, v, v];
    }
    png.save(output_path)
        .map_err(|e| io_err(format!("writing map PNG: {e}")))?;
    Ok(())
}

/// `kind = "normalmap"` pipeline — derive a world-space normal map from the
/// DEM crop and write it as RGB8 PNG (`n * 0.5 + 0.5`).
///
/// Convention matches `lunco-terrain-core::derive::normal_map` and the decode
/// in `terrain_layered.wgsl`: `n = normalize(-dh/dx, 1, -dh/dz)` with `+x` =
/// increasing column (east) and `+z` = increasing row (south, since PDS
/// rasters are north-up) — world-space, no tangent basis.
#[cfg(not(target_arch = "wasm32"))]
fn process_normalmap(
    source: &Path,
    output_path: &Path,
    cfg: &ProcessConfig,
) -> Result<(), std::io::Error> {
    let src = decode_gray_source(source)?;
    let (roi, scale, _clat, _clon) = resolve_roi(cfg, &src, "normalmap")?;
    let out_n = roi.out_n;
    let h = resample_roi_bilinear(&src.samples, src.w, src.h, &roi);

    // Metres per output texel — the crop spans `win * scale` metres.
    let step = (roi.win as f64 * scale) / out_n.max(1) as f64;
    let at = |x: isize, z: isize| -> f64 {
        let x = x.clamp(0, out_n as isize - 1) as usize;
        let z = z.clamp(0, out_n as isize - 1) as usize;
        h[z * out_n + x]
    };
    let mut png = image::RgbImage::new(out_n as u32, out_n as u32);
    for z in 0..out_n as isize {
        for x in 0..out_n as isize {
            let dhdx = (at(x + 1, z) - at(x - 1, z)) / (2.0 * step);
            let dhdz = (at(x, z + 1) - at(x, z - 1)) / (2.0 * step);
            let len = (dhdx * dhdx + 1.0 + dhdz * dhdz).sqrt();
            let n = [-dhdx / len, 1.0 / len, -dhdz / len];
            let enc = |c: f64| ((c * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
            png.put_pixel(x as u32, z as u32, image::Rgb([enc(n[0]), enc(n[1]), enc(n[2])]));
        }
    }
    png.save(output_path)
        .map_err(|e| io_err(format!("writing normal-map PNG: {e}")))?;
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
            frame: Some("MOON_ME".into()),
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

        // The manifest's frame declaration lands in the raster's own tags.
        // (No metadata.yaml sidecar any more — the geo tags ARE the metadata.)
        let mut tag_dec =
            tiff::decoder::Decoder::new(Cursor::new(out_bytes.as_slice())).unwrap();
        let geo = lunco_geotiff::read_geo_tags(&mut tag_dec).unwrap();
        assert_eq!(geo.frame, Some(lunco_geotiff::LunarFrame::MoonMe));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `kind = "dem"` must ingest a PDS3 `.IMG` source using the label's own
    /// extent + MAP_SCALE (no `src_*` fields in the manifest) — the
    /// non-GeoTIFF path.
    #[test]
    fn dem_process_ingests_pds_img_via_label_extent() {
        let tmp = std::env::temp_dir().join(format!(
            "lunco-assets-dem-img-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // 8×8 PC_REAL grid with an attached label declaring extent + scale.
        let record_bytes: usize = 1024;
        let mut label = "PDS_VERSION_ID = PDS3\r\n\
                         RECORD_BYTES  = 1024\r\n\
                         LABEL_RECORDS = 1\r\n\
                         ^IMAGE        = 2\r\n\
                         OBJECT = IMAGE\r\n\
                           LINES        = 8\r\n\
                           LINE_SAMPLES = 8\r\n\
                           SAMPLE_TYPE  = PC_REAL\r\n\
                           SAMPLE_BITS  = 32\r\n\
                         END_OBJECT = IMAGE\r\n\
                         OBJECT = IMAGE_MAP_PROJECTION\r\n\
                           MAP_PROJECTION_TYPE = \"EQUIRECTANGULAR\"\r\n\
                           MAP_SCALE = 2.0 <METERS/PIXEL>\r\n\
                           MAXIMUM_LATITUDE = 1.0 <DEG>\r\n\
                           MINIMUM_LATITUDE = -1.0 <DEG>\r\n\
                           EASTERNMOST_LONGITUDE = 1.0 <DEG>\r\n\
                           WESTERNMOST_LONGITUDE = -1.0 <DEG>\r\n\
                         END_OBJECT = IMAGE_MAP_PROJECTION\r\n\
                         END\r\n"
            .as_bytes()
            .to_vec();
        label.resize(record_bytes, b' ');
        for i in 0..64u32 {
            label.extend_from_slice(&(i as f32 * 5.0).to_le_bytes());
        }
        let src_path = tmp.join("source.IMG");
        std::fs::write(&src_path, &label).unwrap();

        let cfg = ProcessConfig {
            kind: "dem".into(),
            output: "site".into(),
            output_root: "cache".into(),
            target_resolution: Some([4, 4]),
            center_lat: Some(0.0),
            center_lon: Some(0.0),
            window_m: Some(8.0),
            pixel_scale_m: 2.0, // serde default — label's MAP_SCALE governs
            src_min_lat: None,  // absent on purpose: label extent must serve
            src_max_lat: None,
            src_min_lon: None,
            src_max_lon: None,
            site_id: None,
            frame: Some("MOON_ME".into()),
        };
        let out_dir = tmp.join("site");
        process_dem(&src_path, &out_dir, &cfg).expect("PDS IMG dem ingest succeeds");

        let out_bytes = std::fs::read(out_dir.join("materials/textures/heightmap.tif")).unwrap();
        let mut dec = tiff::decoder::Decoder::new(Cursor::new(out_bytes.as_slice())).unwrap();
        let (w, h) = dec.dimensions().unwrap();
        assert_eq!((w, h), (4, 4));
        match dec.read_image().unwrap() {
            tiff::decoder::DecodingResult::F32(v) => {
                assert!(v.iter().all(|x| x.is_finite()));
                assert!(v.iter().any(|x| *x > 0.0), "real samples made it through");
            }
            other => panic!("expected F32 heightmap, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `kind = "normalmap"` writes an RGB8 PNG whose flat regions encode the
    /// up vector (128, 255, 128) and whose slopes tilt away from it.
    #[test]
    fn normalmap_process_encodes_world_space_normals() {
        // 16×16 ramp in +x: constant dh/dx, zero dh/dz.
        let (sw, sh) = (16u32, 16u32);
        let src: Vec<f32> = (0..sh)
            .flat_map(|_r| (0..sw).map(move |c| c as f32 * 2.0))
            .collect();
        let tif = encode_tiff_f32(sw, sh, &src);

        let tmp = std::env::temp_dir().join(format!(
            "lunco-assets-nrm-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let src_path = tmp.join("source.tif");
        std::fs::write(&src_path, &tif).unwrap();

        let cfg = ProcessConfig {
            kind: "normalmap".into(),
            output: "normal.png".into(),
            output_root: "cache".into(),
            target_resolution: Some([8, 8]),
            center_lat: Some(0.0),
            center_lon: Some(0.0),
            window_m: Some(16.0),
            pixel_scale_m: 1.0,
            src_min_lat: Some(-1.0),
            src_max_lat: Some(1.0),
            src_min_lon: Some(-1.0),
            src_max_lon: Some(1.0),
            site_id: None,
            frame: None,
        };
        let out_path = tmp.join("normal.png");
        process_normalmap(&src_path, &out_path, &cfg).expect("normalmap succeeds");

        let png = image::open(&out_path).unwrap().to_rgb8();
        assert_eq!((png.width(), png.height()), (8, 8));
        let c = png.get_pixel(4, 4).0;
        // Up-slope in +x ⇒ normal tilts to -x: R < 128; no z tilt: B ≈ 128;
        // Y strongly positive.
        assert!(c[0] < 120, "R tilts negative-x on a +x ramp, got {}", c[0]);
        assert!(c[1] > 150, "G (up) stays dominant, got {}", c[1]);
        assert!((c[2] as i32 - 128).abs() <= 6, "B stays neutral, got {}", c[2]);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `kind = "map"` crops an RGB source to the ROI and keeps colour.
    #[test]
    fn map_process_crops_rgb_source() {
        let tmp = std::env::temp_dir().join(format!(
            "lunco-assets-map-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // 16×16 RGB PNG: left half red, right half green.
        let mut img = image::RgbImage::new(16, 16);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            *p = if x < 8 { image::Rgb([200, 10, 10]) } else { image::Rgb([10, 200, 10]) };
        }
        let src_path = tmp.join("source.png");
        img.save(&src_path).unwrap();

        let cfg = ProcessConfig {
            kind: "map".into(),
            output: "map.png".into(),
            output_root: "cache".into(),
            target_resolution: Some([8, 8]),
            center_lat: Some(0.0),
            center_lon: Some(0.0),
            window_m: Some(16.0),
            pixel_scale_m: 1.0,
            src_min_lat: Some(-1.0),
            src_max_lat: Some(1.0),
            src_min_lon: Some(-1.0),
            src_max_lon: Some(1.0),
            site_id: None,
            frame: None,
        };
        let out_path = tmp.join("map.png");
        process_map(&src_path, &out_path, &cfg).expect("map crop succeeds");

        let out = image::open(&out_path).unwrap().to_rgb8();
        assert_eq!((out.width(), out.height()), (8, 8));
        assert!(out.get_pixel(1, 4).0[0] > 100, "west side stays red");
        assert!(out.get_pixel(6, 4).0[1] > 100, "east side stays green");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
