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

#[cfg(not(target_arch = "wasm32"))]
use image::GenericImageView;
#[cfg(not(target_arch = "wasm32"))]
use lunco_assets_datasets::{
    bake_key, bake_stamp_path, default_dem_pixel_scale_m, process_output_path,
    processed_output_present, ProcessConfig,
};
#[cfg(not(target_arch = "wasm32"))]
use resvg::tiny_skia;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use usvg::{Options, Tree};

/// Cancellation and commit ownership for one processing attempt.
///
/// Processing may spend a long time decoding or resampling a large source, so
/// cancellation is cooperative at pipeline boundaries. The commit gate is
/// shared with the dataset download attempt: closing a Twin acquires it before
/// retiring the attempt, which makes the close boundary a real write barrier.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct ProcessControl {
    cancel: Arc<AtomicBool>,
    commit_gate: Arc<Mutex<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ProcessControl {
    /// Create processing control owned by an application worker.
    pub fn new(cancel: Arc<AtomicBool>, commit_gate: Arc<Mutex<()>>) -> Self {
        Self {
            cancel,
            commit_gate,
        }
    }

    /// Control for an explicit CLI processing invocation, which has no
    /// lifecycle owner to cancel it.
    pub fn unrestricted() -> Self {
        Self::new(Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(())))
    }

    fn check(&self) -> Result<(), std::io::Error> {
        if self.cancel.load(Ordering::Acquire) {
            Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "processing cancelled",
            ))
        } else {
            Ok(())
        }
    }

    fn commit_guard(&self) -> Result<std::sync::MutexGuard<'_, ()>, std::io::Error> {
        self.commit_gate.lock().map_err(|_| {
            std::io::Error::other("processing commit gate is poisoned; refusing to commit")
        })
    }
}

/// Native function implemented by one asset processor.
#[cfg(not(target_arch = "wasm32"))]
pub type ProcessorFn = fn(
    source: &Path,
    output: &Path,
    config: &ProcessConfig,
    control: &ProcessControl,
) -> Result<(), std::io::Error>;

/// One registered processing implementation.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
pub struct ProcessorSpec {
    /// Manifest value that selects this processor.
    pub kind: &'static str,
    /// Native implementation of the processor.
    pub run: ProcessorFn,
    /// Output sidecar extensions committed with the primary artifact.
    pub sidecars: &'static [&'static str],
}

#[cfg(not(target_arch = "wasm32"))]
impl ProcessorSpec {
    /// Define a processor and the sidecars it atomically publishes.
    pub const fn new(
        kind: &'static str,
        run: ProcessorFn,
        sidecars: &'static [&'static str],
    ) -> Self {
        Self {
            kind,
            run,
            sidecars,
        }
    }
}

/// Registry of native processors selected by authored `process.kind` values.
///
/// The registry keeps the bake dispatcher open for domain crates: adding a
/// heavy decoder or transform does not require growing one central match. The
/// registry is a Rust extension seam because processors own I/O and math;
/// Rhai remains responsible for selecting and sequencing authored policy.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
pub struct ProcessorRegistry {
    specs: BTreeMap<String, ProcessorSpec>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ProcessorRegistry {
    /// Create a registry containing the processors shipped by LunCoSim.
    pub fn builtin() -> Self {
        let mut registry = Self::default();
        registry.register(ProcessorSpec::new("texture", process_texture, &[]));
        registry.register(ProcessorSpec::new("gltf", process_gltf_adapter, &[]));
        registry.register(ProcessorSpec::new("dem", process_dem, &[]));
        registry.register(ProcessorSpec::new("map", process_map, &["mean"]));
        registry.register(ProcessorSpec::new("albedo", process_albedo, &["mean"]));
        registry.register(ProcessorSpec::new("normalmap", process_normalmap, &[]));
        registry
    }

    /// Register or replace one processor. The returned value is the previous
    /// definition, if any, so an application can reject accidental overrides.
    pub fn register(&mut self, spec: ProcessorSpec) -> Option<ProcessorSpec> {
        self.specs.insert(spec.kind.to_owned(), spec)
    }

    /// Resolve a processor selected by a manifest.
    pub fn get(&self, kind: &str) -> Option<&ProcessorSpec> {
        self.specs.get(kind)
    }

    /// Sorted processor names, suitable for diagnostics and capability
    /// discovery.
    pub fn kinds(&self) -> impl Iterator<Item = &str> {
        self.specs.keys().map(String::as_str)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for ProcessorRegistry {
    fn default() -> Self {
        Self {
            specs: BTreeMap::new(),
        }
    }
}

/// Processes a single source asset according to `process.kind`.
///
/// - `"texture"`: resize an image to `target_resolution` and
///   save as PNG. Supports JPEG, PNG, TIFF, BMP, WebP, SVG inputs.
/// - `"gltf"`: clean a `.glb` for Bevy by decoding Draco geometry. Inputs
///   using `EXT_texture_webp` are rejected until a lossless Rust-owned WebP
///   conversion path is available; the extension is never discarded.
/// - `"dem"`: crop a square ROI from a raw LROC/NAC DTM and write the
///   square, georeferenced float32 `heightmap.tif` the runtime DEM reader
///   expects. `output` is a **folder** (the `demSource` target);
///   the heightmap lands at `<output>/materials/textures/heightmap.tif`.
/// - `"map"`: co-registered ROI crop for a display/analysis raster.
/// - `"albedo"`: co-registered material-albedo bake. Grayscale orthophotos
///   are treated as illumination-bearing measurements: their low-frequency
///   field is removed and only bounded local detail is retained around an
///   authored neutral regolith albedo.
/// - `"normalmap"`: co-registered ROI crop — see the [`ProcessConfig`] kind
///   documentation. Registered extensions use the same shared staging and
///   commit contract and may consume `ProcessConfig::parameters`.
///
/// `cache_root` is the cache that owns the declaration's derived artifact.
/// Engine entries use the global cache; a Twin entry uses its own cache unless
/// its declaration opts into the shared pool. `twin_root` is the caller-supplied
/// Twin folder for `output_root = "twin"`.
#[cfg(not(target_arch = "wasm32"))]
pub fn process_asset(
    source_path: &Path,
    process: &ProcessConfig,
    cache_root: &Path,
    twin_root: Option<&Path>,
    control: &ProcessControl,
) -> Result<(), std::io::Error> {
    let registry = ProcessorRegistry::builtin();
    process_asset_with_registry(
        source_path,
        process,
        cache_root,
        twin_root,
        control,
        &registry,
    )
}

/// Process one asset with an application-supplied processor registry.
///
/// This is the extension point for a domain-specific native processor. The
/// registry changes dispatch only; output-path resolution, bake keys, staging,
/// cancellation, and atomic commit remain shared here.
#[cfg(not(target_arch = "wasm32"))]
pub fn process_asset_with_registry(
    source_path: &Path,
    process: &ProcessConfig,
    cache_root: &Path,
    twin_root: Option<&Path>,
    control: &ProcessControl,
    registry: &ProcessorRegistry,
) -> Result<(), std::io::Error> {
    control.check()?;
    let output_path = process_output_path(process, Some(cache_root), twin_root)?;

    // Create output directory if needed
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    process_asset_to(source_path, process, &output_path, control, registry)
}

/// The body of [`process_asset`] once its output path is known.
#[cfg(not(target_arch = "wasm32"))]
fn process_asset_to(
    source_path: &Path,
    process: &ProcessConfig,
    output_path: &Path,
    control: &ProcessControl,
    registry: &ProcessorRegistry,
) -> Result<(), std::io::Error> {
    control.check()?;
    let output_path = output_path.to_path_buf();
    let processor = registry.get(&process.kind).ok_or_else(|| {
        let kinds = registry.kinds().collect::<Vec<_>>().join(", ");
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "Unknown process kind '{}' (registered processors: {})",
                process.kind, kinds
            ),
        )
    })?;

    // ── Bake-key staleness check ──────────────────────────────────────────
    // The processed output is a pure function of (source bytes, this config,
    // pipeline version). Content-address it: a stamp beside the output holds
    // the key of the bake that produced it, and a matching key plus a complete
    // consumer artifact skips the whole decode (the expensive part — a big
    // mosaic decodes to GBs of f64).
    // Anything that could change the result — new source, edited ROI, a
    // pipeline fix (bump PROCESS_PIPELINE_VERSION) — changes the key and rebakes.
    // Never time-based: a cache that can't go stale beats one that expires.
    let stamp_path = bake_stamp_path(&output_path);
    let key = bake_key(source_path, process)?;
    if std::fs::read_to_string(&stamp_path).is_ok_and(|s| s.trim() == key)
        && processed_output_present(&output_path, process, Some(source_path))
    {
        println!(
            "  ✓ up-to-date (bake key match) → {}",
            output_path.display()
        );
        return Ok(());
    }

    let stage_root = staging_root(&output_path)?;
    let _stage_cleanup = StageCleanup(stage_root.clone());
    let stage_output = stage_root.join(output_path.file_name().ok_or_else(|| {
        io_err(format!(
            "processing output has no file name: {}",
            output_path.display()
        ))
    })?);

    (processor.run)(source_path, &stage_output, process, control)?;

    control.check()?;
    // Stamp only after a fully successful bake, so a failed/interrupted run
    // never masquerades as fresh. The staged artifact and all of its sidecars
    // become visible together under the commit gate.
    std::fs::write(bake_stamp_path(&stage_output), &key)?;
    commit_staged_output(&stage_root, &output_path, processor.sidecars, control)?;

    println!("  ✓ processed → {}", output_path.display());
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
struct StageCleanup(std::path::PathBuf);

#[cfg(not(target_arch = "wasm32"))]
impl Drop for StageCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn staging_root(output_path: &Path) -> Result<std::path::PathBuf, std::io::Error> {
    let parent = output_path.parent().ok_or_else(|| {
        io_err(format!(
            "processing output has no parent: {}",
            output_path.display()
        ))
    })?;
    static STAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = STAGE.fetch_add(1, Ordering::Relaxed);
    let root = parent.join(format!(".lunco-process-{}-{id}", std::process::id()));
    std::fs::create_dir(&root)?;
    Ok(root)
}

#[cfg(not(target_arch = "wasm32"))]
fn commit_staged_output(
    stage_root: &Path,
    output_path: &Path,
    sidecars: &[&str],
    control: &ProcessControl,
) -> Result<(), std::io::Error> {
    let _gate = control.commit_guard()?;
    control.check()?;

    let mut destinations = vec![output_path.to_path_buf(), bake_stamp_path(output_path)];
    if output_path.extension().is_some() {
        destinations.extend(
            sidecars
                .iter()
                .map(|suffix| output_path.with_extension(suffix)),
        );
    }
    let backup_root = stage_root.with_file_name(format!(
        ".{}-backup",
        stage_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("lunco-process")
    ));
    std::fs::create_dir(&backup_root)?;
    let mut backups = Vec::new();
    let backup_result = (|| {
        for (index, destination) in destinations.iter().enumerate() {
            if destination.exists() {
                let backup = backup_root.join(index.to_string());
                std::fs::rename(destination, &backup)?;
                backups.push((destination.clone(), backup));
            }
        }
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = backup_result {
        for (destination, backup) in backups.iter().rev() {
            let _ = std::fs::rename(backup, destination);
        }
        drop(_gate);
        let _ = std::fs::remove_dir_all(&backup_root);
        return Err(error);
    }

    let result = (|| {
        for entry in std::fs::read_dir(stage_root)? {
            let entry = entry?;
            let destination = output_path
                .parent()
                .ok_or_else(|| io_err("staged output has no parent".into()))?
                .join(entry.file_name());
            std::fs::rename(entry.path(), destination)?;
        }
        Ok::<(), std::io::Error>(())
    })();

    if let Err(error) = result {
        for (index, destination) in destinations.iter().rev().enumerate() {
            if destination.exists() {
                // A failed commit must not recursively delete a large
                // replacement while Twin teardown waits on the barrier. Move
                // it aside in O(1); the cleanup runs after the guard drops.
                let _ = std::fs::rename(destination, backup_root.join(format!("failed-{index}")));
            }
        }
        for (destination, backup) in backups.iter().rev() {
            let _ = std::fs::rename(backup, destination);
        }
        drop(_gate);
        let _ = std::fs::remove_dir_all(&backup_root);
        return Err(error);
    }

    drop(_gate);
    let _ = std::fs::remove_dir_all(&backup_root);
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn process_texture(
    source: &Path,
    output: &Path,
    config: &ProcessConfig,
    _control: &ProcessControl,
) -> Result<(), std::io::Error> {
    let [tw, th] = config.target_resolution.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "texture pipeline requires `target_resolution = [w, h]`",
        )
    })?;
    let ext = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    match ext {
        "svg" => process_svg(source, output, tw, th),
        "jpg" | "jpeg" | "png" | "tiff" | "tif" | "bmp" | "webp" => {
            process_image(source, output, tw, th)
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Unsupported source format: .{ext}"),
        )),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn process_gltf_adapter(
    source: &Path,
    output: &Path,
    _config: &ProcessConfig,
    control: &ProcessControl,
) -> Result<(), std::io::Error> {
    process_gltf(source, output, control)
}

/// Normalize Draco geometry in a GLB using the pure-Rust glTF implementation.
///
/// Draco geometry is materialized as ordinary glTF accessors. WebP texture
/// extensions remain authored data: dropping the extension without converting
/// its image bytes would silently change texture selection. The processor
/// therefore rejects such input until the conversion is implemented. The
/// resulting document is emitted as a GLB while non-transformed authored data
/// remains intact. Native-only — wasm builds skip this whole module.
#[cfg(not(target_arch = "wasm32"))]
fn process_gltf(source: &Path, output: &Path, control: &ProcessControl) -> std::io::Result<()> {
    control.check()?;
    let mut import = draco_gltf::open(source, draco_gltf::ValidationProfile::Gltf20)
        .map_err(|error| io_err(format!("reading GLB for normalization: {error}")))?;
    control.check()?;
    import
        .decompress_in_place()
        .map_err(|error| io_err(format!("decoding Draco geometry: {error}")))?;
    control.check()?;
    // TODO: Add a Rust-owned, lossless WebP conversion path. It must decode
    // every image selected by EXT_texture_webp, encode the replacement bytes,
    // update image MIME types and buffer-view references, and remove the
    // extension only after those authored texture semantics are preserved.
    if json_contains_key(import.document.as_value(), "EXT_texture_webp") {
        return Err(io_err(
            "GLB uses EXT_texture_webp; Rust WebP conversion is not implemented yet; refusing to discard the authored texture extension".into(),
        ));
    }
    control.check()?;
    let bytes = import
        .to_bytes(draco_gltf::OutputFormat::GlbV2)
        .map_err(|error| io_err(format!("writing normalized GLB: {error}")))?;
    std::fs::write(output, bytes)
}

#[cfg(not(target_arch = "wasm32"))]
fn json_contains_key(value: &draco_gltf::JsonValue, key: &str) -> bool {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            draco_gltf::JsonValue::Array(values) => pending.extend(values),
            draco_gltf::JsonValue::Object(entries) => {
                for (name, value) in entries {
                    if name == key {
                        return true;
                    }
                    pending.push(value);
                }
            }
            draco_gltf::JsonValue::Null
            | draco_gltf::JsonValue::Bool(_)
            | draco_gltf::JsonValue::Number(_)
            | draco_gltf::JsonValue::String(_) => {}
        }
    }
    false
}

#[cfg(not(target_arch = "wasm32"))]
fn process_svg(source: &Path, output: &Path, tw: u32, th: u32) -> Result<(), std::io::Error> {
    let svg_data = std::fs::read(source)?;
    let opt = Options::default();
    let tree = Tree::from_data(&svg_data, &opt)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

    let size = tree.size();
    let mut pixmap = tiny_skia::Pixmap::new(tw, th).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid resolution")
    })?;

    let transform =
        tiny_skia::Transform::from_scale(tw as f32 / size.width(), th as f32 / size.height());
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    pixmap
        .save_png(output)
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

    processed
        .save(output)
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
    control: &ProcessControl,
) -> Result<(), std::io::Error> {
    control.check()?;
    let mut src = decode_gray_source(source)?;
    apply_dem_height_units(
        &mut src.samples,
        cfg.source_height_scale_m_per_unit,
        cfg.source_height_offset_m,
        control,
    )?;
    let (roi, scale, center_lat, center_lon) = resolve_roi(cfg, &src, "dem")?;
    let (out_n, win) = (roi.out_n, roi.win);
    // Sanity ceiling only. A DEM's voids are FILLED below, not avoided — this
    // exists to catch a window that has wandered off the product entirely.
    reject_if_mostly_nodata(
        &roi_samples(&src.samples, src.w, src.h, &roi, control)?,
        "dem",
        0.05,
    )?;
    let mut heights = resample_roi_bilinear(&src.samples, src.w, src.h, &roi, control)?;
    let filled = fill_dem_voids(&mut heights, out_n, "dem", control)?;
    if filled > 0 {
        println!(
            "    filled {filled} void sample(s) ({:.2}%) by neighbour interpolation",
            100.0 * filled as f64 / heights.len().max(1) as f64
        );
    }
    let out: Vec<f32> = heights.iter().map(|&v| v as f32).collect();

    // ── Write the square float32 heightmap. ───────────────────────────────
    let tex_dir = output_dir.join("materials").join("textures");
    std::fs::create_dir_all(&tex_dir)?;
    let tif_path = tex_dir.join("heightmap.tif");
    {
        use tiff::encoder::{colortype, TiffEncoder};
        let mut enc = TiffEncoder::new(std::fs::File::create(&tif_path)?).map_err(tiff_io_err)?;

        // The GEO half — without it QGIS opens the raster in pixel units and every
        // slope computed from it is wrong by the ground-sample factor, silently.
        //
        // `win * scale` is the true on-the-ground span, and the frame is
        // node-based: sample 0 on the west/north edge, sample n-1 on the east/south.
        // Body radius, for the GeoTIFF citation only — it does not enter the
        // pixel→metre mapping, which is a local metric frame.
        //
        // The canonical lunar radius: IAU/WGCCRE mean radius, 1737.4 km
        // (Archinal et al., WGCCRE report) — the datum the LOLA/LRO products this
        // pipeline ingests are themselves referenced to.
        //
        // ⚠ DO NOT re-type this number. It is declared once, in `lunco-core`
        // (`lunco_core::MOON_MEAN_RADIUS_M`), precisely so this offline build tool
        // and the simulation's `lunco_celestial::registry` — which re-exports it —
        // cannot stamp two different datums. This site used to mirror the VALUE.
        let mut geo = lunco_geotiff::GeoTransform::centred_square(
            win as f64 * scale,
            out_n,
            lunco_core::MOON_MEAN_RADIUS_M,
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
        lunco_geotiff::write_geo_tags(img.encoder(), &geo, "Moon 2000").map_err(tiff_io_err)?;
        img.write_data(&out).map_err(tiff_io_err)?;
    }

    // No sidecar. Extent, resolution, centre lat/lon and body radius live in the
    // raster's geo tags; source URL and checksum in `Assets.toml`; site id in the
    // folder name. See `docs/architecture/57-dem-georeferencing.md`.

    Ok(())
}

/// Convert the source DEM's declared vertical units into metres relative to the
/// body's reference surface. The source declaration owns this conversion; the
/// raster reader and the terrain runtime consume metres and never guess units.
#[cfg(not(target_arch = "wasm32"))]
fn apply_dem_height_units(
    samples: &mut [f64],
    scale_m_per_unit: f64,
    offset_m: f64,
    control: &ProcessControl,
) -> Result<(), std::io::Error> {
    if !scale_m_per_unit.is_finite() || scale_m_per_unit == 0.0 {
        return Err(io_err(
            "dem source height scale must be finite and non-zero".into(),
        ));
    }
    if !offset_m.is_finite() {
        return Err(io_err("dem source height offset must be finite".into()));
    }
    for (index, value) in samples.iter_mut().enumerate() {
        if index.is_multiple_of(4096) {
            control.check()?;
        }
        if value.is_finite() {
            *value = *value * scale_m_per_unit + offset_m;
            if !value.is_finite() {
                return Err(io_err(
                    "dem source height conversion produced a non-finite elevation".into(),
                ));
            }
        }
    }
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
    // The sample-format match, the lifted `tiff` limits (LROC mosaics ship as a
    // single giant strip past the crate's 128 MB default) and the nodata→`NaN`
    // mapping all live in `lunco_geotiff::decode_gray_f64` — the one core this
    // writer and the terrain baker's reader now share. They used to hold a copy
    // each and drifted; the copy here is gone deliberately, do not restore it.
    let bytes = std::fs::read(source)?;
    let (src_w, src_h, heights_f64) = lunco_geotiff::decode_gray_f64(Cursor::new(bytes.as_slice()))
        .map_err(|e| io_err(format!("decoding DTM TIFF: {e}")))?;
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
/// Bounding box (inclusive) of the samples that are actual measurements.
///
/// PDS orthorectified products are footprints padded to a rectangle: the imaged
/// strip is not axis-aligned, so the raster carries `CORE_NULL` around it. The
/// raster's dimensions therefore say nothing about where the data is, and any
/// crop clamped to `0..w` can land squarely in the padding.
///
/// Scans rows/columns rather than per-pixel so the cost stays linear in the
/// raster and negligible against the decode that produced it.
fn valid_data_bounds(samples: &[f64], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    // An RGB source carries its data in `planes` and leaves `samples` empty. It
    // also has no NaN sentinel — colour products encode absence as a colour, not
    // as a non-finite — so there is nothing to scan and the raster IS the bound.
    if samples.len() != w.checked_mul(h)? {
        return Some((0, w.checked_sub(1)?, 0, h.checked_sub(1)?)).map(|(a, b, c, d)| (a, c, b, d));
    }
    let finite = |x: usize, y: usize| samples.get(y * w + x).is_some_and(|v| v.is_finite());
    let row_has = |y: usize| (0..w).any(|x| finite(x, y));
    let col_has = |x: usize| (0..h).any(|y| finite(x, y));
    let y0 = (0..h).find(|&y| row_has(y))?;
    let y1 = (0..h).rev().find(|&y| row_has(y))?;
    let x0 = (0..w).find(|&x| col_has(x))?;
    let x1 = (0..w).rev().find(|&x| col_has(x))?;
    Some((x0, y0, x1, y1))
}

/// The RAW source samples inside the ROI, un-resampled.
///
/// The nodata guard sees these rather than the resampled output so its coverage
/// decision is made against source measurements, independently of interpolation.
#[cfg(not(target_arch = "wasm32"))]
fn roi_samples(
    samples: &[f64],
    src_w: usize,
    src_h: usize,
    roi: &RoiCrop,
    control: &ProcessControl,
) -> Result<Vec<f64>, std::io::Error> {
    // RGB sources keep their data in `planes`; nothing to check here (see
    // `valid_data_bounds`). Returning empty makes the guard a no-op rather than
    // an index panic.
    if samples.len() != src_w.saturating_mul(src_h) {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(roi.win * roi.win);
    for (row, y) in (roi.y0..(roi.y0 + roi.win).min(src_h)).enumerate() {
        if row.is_multiple_of(4096) {
            control.check()?;
        }
        for x in roi.x0..(roi.x0 + roi.win).min(src_w) {
            out.push(samples[y * src_w + x]);
        }
    }
    Ok(out)
}

/// Interpolate DEM voids from their valid neighbours. Returns how many were filled.
///
/// WHY A DEM MUST NEVER SHIP A VOID. A NaN height is not a cosmetic gap: the
/// heightmap drives the render mesh AND the collider, so it becomes a hole in the
/// world that a rover can drive into. That is categorically different from a void
/// in an ortho or normal map, which bakes to a neutral value and merely looks flat.
///
/// WHY VOIDS EXIST AT ALL — and they are NOT our bug. `NAC_DTM_APOLLO15.TIF` is
/// derived from a STEREO PAIR, and stereo matching fails where there is no texture
/// to match: shadowed crater floors above all. Gaps are a normal property of every
/// stereo-derived DTM, present in the source as shipped.
///
/// WHY NOT JUST SHRINK THE WINDOW. That was the first instinct and it is wrong: it
/// discards hundreds of metres of good terrain to dodge a handful of bad samples,
/// and it is luck-dependent — the next site, or the same site re-centred, lands on
/// a void again. Void-fill is what DEM pipelines do; the window should be chosen
/// for coverage, not to route around correlation failures.
///
/// Iterative neighbour averaging (8-connected): each pass replaces every void that
/// has at least one valid neighbour with their mean, so fills grow inward from the
/// rim one ring per pass. Converges in `radius` passes for a void of that radius.
/// The cap bounds the work and, more usefully, refuses to invent a large interior:
/// a gap that survives it is too big to fill honestly and is a real authoring
/// error, not a speckle.
#[cfg(not(target_arch = "wasm32"))]
fn fill_dem_voids(
    v: &mut [f64],
    n: usize,
    kind: &str,
    control: &ProcessControl,
) -> Result<usize, std::io::Error> {
    const MAX_PASSES: usize = 64;
    let idx = |x: usize, y: usize| y * n + x;
    let mut filled = 0usize;
    for _ in 0..MAX_PASSES {
        control.check()?;
        let holes: Vec<usize> = (0..v.len()).filter(|&i| !v[i].is_finite()).collect();
        if holes.is_empty() {
            return Ok(filled);
        }
        let snapshot = v.to_vec();
        let mut progressed = false;
        for (index, i) in holes.into_iter().enumerate() {
            if index.is_multiple_of(4096) {
                control.check()?;
            }
            let (x, y) = (i % n, i / n);
            let mut sum = 0.0;
            let mut cnt = 0.0;
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx >= n as isize || ny >= n as isize {
                        continue;
                    }
                    let s = snapshot[idx(nx as usize, ny as usize)];
                    if s.is_finite() {
                        sum += s;
                        cnt += 1.0;
                    }
                }
            }
            if cnt > 0.0 {
                v[i] = sum / cnt;
                filled += 1;
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    let left = v.iter().filter(|s| !s.is_finite()).count();
    if left > 0 {
        return Err(io_err(format!(
            "{kind} pipeline: {left} void sample(s) could not be filled after {MAX_PASSES} \
             passes — the gap is larger than {MAX_PASSES} samples across. Filling it would be \
             inventing terrain; re-centre the window or pick a source with coverage here."
        )));
    }
    Ok(filled)
}

/// Reject a crop that is mostly padding.
///
/// The clamp above keeps the window inside the valid-data BOUNDING BOX, which is
/// the right bound for a rectangular footprint but still admits null corners when
/// the imaged strip is rotated inside it. This is the backstop: a derived map is
/// consumed as if every texel were a measurement — the ortho multiplies albedo,
/// the normal map replaces the shading normal — so shipping one that is a third
/// padding produces confident, wrong ground rather than a visible error.
///
/// Fails the bake instead of warning. A warning is what the previous behaviour
/// effectively was, and an 18%-dead map shipped anyway.
#[cfg(not(target_arch = "wasm32"))]
fn reject_if_mostly_nodata(values: &[f64], kind: &str, limit: f64) -> Result<(), std::io::Error> {
    if values.is_empty() {
        return Ok(());
    }
    let bad = values.iter().filter(|v| !v.is_finite()).count();
    let frac = bad as f64 / values.len() as f64;
    if frac > limit {
        return Err(io_err(format!(
            "{kind} pipeline: {:.1}% of the cropped window has no data (limit {:.1}%). \
             The requested `window_m`/`center_lat`/`center_lon` reaches past this \
             source's imaged footprint — shrink the window or re-centre it. Baking \
             this would put a flat, uniformly-shaded band into the terrain.",
            frac * 100.0,
            limit * 100.0
        )));
    }
    Ok(())
}

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
    let manifest_extent = match (
        cfg.src_min_lat,
        cfg.src_max_lat,
        cfg.src_min_lon,
        cfg.src_max_lon,
    ) {
        (Some(a), Some(b), Some(c), Some(d)) => Some((a, b, c, d)),
        _ => None,
    };
    let (min_lat, max_lat, min_lon, max_lon) = manifest_extent
        .or_else(|| {
            src.extent
                .map(|e| (e.min_lat, e.max_lat, e.west_lon, e.east_lon))
        })
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
    //
    // CLAMPED AGAINST VALID DATA, NOT THE RASTER. These are not the same bound,
    // and assuming they were is what put a dead margin into every derived map of
    // the Apollo 15 site: `NAC_DTM_APOLLO15_M111571816_2M.IMG` is an
    // orthorectified frame PADDED to a rectangle with `CORE_NULL`, so ~16% of its
    // raster carries no measurement at all. A window can sit entirely inside
    // `src_w`/`src_h` and still be one-sixth null.
    //
    // Measured on the shipped bake before this fix: `ortho.png` and `normal.png`
    // both went dead at exactly x=2100/2500 for every row — the ortho baked to
    // flat white (which the orthophoto transfer then rendered at its brightest
    // bounded tone) and the normal map to a zero-relief plane. A smooth,
    // straight-edged band along the site boundary, in an engine where nothing
    // downstream can tell "no data" from "flat bright ground".
    let (vx0, vy0, vx1, vy1) = valid_data_bounds(&src.samples, src_w, src_h)
        .ok_or_else(|| io_err(format!("{kind} pipeline: source has no finite samples")))?;
    let (vx0, vy0) = (vx0 as isize, vy0 as isize);
    let (vx1, vy1) = (vx1 as isize, vy1 as isize);
    let max_half_w = (cc - vx0).max(0);
    let max_half_e = (vx1 - cc).max(0);
    let max_half_n = (cr - vy0).max(0);
    let max_half_s = (vy1 - cr).max(0);
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
    Ok((
        RoiCrop { x0, y0, win, out_n },
        scale,
        center_lat,
        center_lon,
    ))
}

/// Resample the crop's source window to `out_n × out_n` (bilinear;
/// finite neighbours are renormalised and an all-nodata sample remains NaN).
#[cfg(not(target_arch = "wasm32"))]
fn resample_roi_bilinear(
    samples: &[f64],
    src_w: usize,
    src_h: usize,
    roi: &RoiCrop,
    control: &ProcessControl,
) -> Result<Vec<f64>, std::io::Error> {
    let (x0, y0, win, out_n) = (roi.x0, roi.y0, roi.win, roi.out_n);
    let mut out = vec![0.0f64; out_n * out_n];
    for oy in 0..out_n {
        control.check()?;
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
            // Bilinear with the weights RENORMALISED over the finite neighbours,
            // and NaN out when all four are void.
            //
            // This used to substitute `0.0` for every non-finite neighbour, which
            // is wrong twice over. Near a void it blended real terrain against a
            // fabricated zero, dragging the edge toward sea level — a dark fringe
            // in a map, a cliff in a DEM. And where all four were void it emitted
            // `0.0`, an ordinary-looking elevation, so "no data" arrived downstream
            // wearing the costume of a measurement: `is_finite()` could no longer
            // find it, the void-fill below had nothing to fill, and the terrain
            // plunged to absolute zero in a smooth confident sheet.
            //
            // Renormalising keeps every partial neighbourhood honest (three real
            // samples give a real answer, weighted as if the fourth were absent
            // rather than zero) and preserves NaN where there is genuinely nothing,
            // so `fill_dem_voids` can see and repair it.
            let s = |col: usize, row: usize| -> Option<f64> {
                samples
                    .get(row * src_w + col)
                    .copied()
                    .filter(|v| v.is_finite())
            };
            let corners = [
                (s(sx0, sy0), (1.0 - fx) * (1.0 - fy)),
                (s(sx1, sy0), fx * (1.0 - fy)),
                (s(sx0, sy1), (1.0 - fx) * fy),
                (s(sx1, sy1), fx * fy),
            ];
            let mut acc = 0.0;
            let mut wsum = 0.0;
            for (val, w) in corners {
                if let Some(v) = val {
                    acc += v * w;
                    wsum += w;
                }
            }
            out[oy * out_n + ox] = if wsum > 1e-12 { acc / wsum } else { f64::NAN };
        }
    }
    Ok(out)
}

/// `kind = "map"` pipeline — crop a co-registered raster to the same
/// geographic ROI as the site's DEM and write an 8-bit PNG layer map.
///
/// RGB sources (LROC `_SLOPE`/`_CLRGRAD` colour TIFFs) crop as-is. Grayscale
/// sources (`_SHADE`, ortho `.IMG` radiance) get a 1–99 percentile stretch to
/// a normalized linear contrast signal — NAC radiance floats would otherwise
/// land in a few gray levels — then that signal is encoded as sRGB bytes.
/// Output is always RGB PNG: Bevy tags 8-bit PNGs sRGB, and an R-only gray
/// would sample red in the layered shader's albedo slot. Encoding the stretched
/// signal here is required because the runtime loader correctly decodes the
/// authored PNG back to linear samples.
#[cfg(not(target_arch = "wasm32"))]
fn process_map(
    source: &Path,
    output_path: &Path,
    cfg: &ProcessConfig,
    control: &ProcessControl,
) -> Result<(), std::io::Error> {
    control.check()?;
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
            // `resolve_roi` validates the requested crop against finite source
            // data. RGB maps have no grayscale plane, but every decoded RGB
            // texel is still valid source coverage; use one channel as the
            // coverage plane instead of passing an empty sample vector.
            samples: planes[0].clone(),
            extent: None,
            scale_m: None,
            projection: None,
        };
        let (roi, _scale, _clat, _clon) = resolve_roi(cfg, &probe, "map")?;
        let out_n = roi.out_n;
        let rgb: Vec<Vec<f64>> = planes
            .iter()
            .map(|pl| resample_roi_bilinear(pl, w, h, &roi, control))
            .collect::<Result<_, _>>()?;
        let mut png = image::RgbImage::new(out_n as u32, out_n as u32);
        for (i, px) in png.pixels_mut().enumerate() {
            if i.is_multiple_of(out_n.max(1) * 64) {
                control.check()?;
            }
            for (channel, plane) in px.0.iter_mut().zip(&rgb) {
                *channel = plane[i].round().clamp(0.0, 255.0) as u8;
            }
        }
        png.save(output_path)
            .map_err(|e| io_err(format!("writing map PNG: {e}")))?;
        // RGB source maps preserve their authored channels. They do not use the
        // grayscale percentile stretch, so the sidecar records that no bake-time
        // stretch was applied. Runtime albedo transfer remains the shared shader
        // contract; this sidecar is for bake inspection, not a hidden setting.
        std::fs::write(output_path.with_extension("mean"), "1.000000\n")?;
        return Ok(());
    }

    // Grayscale path (PDS `.IMG` orthos: single-band radiance).
    let src = decode_gray_source(source)?;
    let (roi, _scale, _clat, _clon) = resolve_roi(cfg, &src, "map")?;
    let out_n = roi.out_n;
    reject_if_mostly_nodata(
        &roi_samples(&src.samples, src.w, src.h, &roi, control)?,
        "map",
        0.02,
    )?;
    let gray = resample_roi_bilinear(&src.samples, src.w, src.h, &roi, control)?;

    // 1–99 percentile stretch over the CROP (not the whole mosaic — the crop
    // is the scene, and mosaic-wide outliers would flatten its contrast).
    //
    // ONE predicate for "this sample is a measurement", shared by the stretch and
    // the bake below. They disagreed before: the stretch excluded these samples,
    // then the bake mapped them to black — the most destructive value available
    // for a map consumed multiplicatively. A sample the stretch refuses to learn
    // from must not be one the bake trusts.
    //
    // Both halves are load-bearing, for different reasons:
    //
    // * `is_finite()` — the decoder maps every declared null to `NaN`, including
    //   the PDS radix form (`CORE_NULL = 16#FF7FFFFB#`) that real LROC products
    //   use. While that sentinel survived as a finite float it BECAME the 1st
    //   percentile on any crop with a nodata margin, dragging `lo` to ≈ -3.4e38
    //   so every real sample mapped to 1.0 — a solid-white layer with a black
    //   nodata hole. If a baked map is ever bimodal white/black again, suspect
    //   the decoder before touching the stretch.
    //
    // * `!= 0.0` — the authored radiance convention reserves exact zero for
    //   empty/black samples. The resampler preserves NaN and renormalises over
    //   finite neighbours, so this check remains a source-domain guard rather
    //   than a correction for interpolation.
    let is_measurement = |v: f64| v.is_finite() && v != 0.0;

    let (lo, hi) = grayscale_percentile_bounds(&gray, is_measurement);
    // Accumulated over the MEASURED samples only — nodata bakes to white, and
    // folding that into the mean would bias the display contrast by the size of the margin.
    // Keep the sidecar in the normalized linear domain; the PNG bytes below
    // are sRGB-encoded for the runtime image loader.
    let mut sum_measured = 0.0f64;
    let mut n_measured = 0.0f64;
    let mut png = image::RgbImage::new(out_n as u32, out_n as u32);
    for (i, px) in png.pixels_mut().enumerate() {
        // Nodata bakes to WHITE, not black. The map pipeline produces
        // display/analysis rasters, and white is the neutral convention for
        // consumers that use the map as a multiplicative signal. Black would
        // turn unsurveyed ground into an unlit void for those consumers.
        //
        // NOTE this cannot be left to the `as u8` cast: were a NaN to reach here
        // it would cast to 0 in Rust (saturating), i.e. silently to the worst
        // possible answer. The resampler currently rules that out by zeroing
        // non-finites first, so today the guard earns its keep on the `0.0` case —
        // but it must survive that resampler being fixed to propagate NaN.
        let v = if is_measurement(gray[i]) {
            let contrast = ((gray[i] - lo) / (hi - lo)).clamp(0.0, 1.0);
            let s = encode_map_contrast_to_srgb_u8(contrast);
            sum_measured += contrast;
            n_measured += 1.0;
            s
        } else {
            255
        };
        px.0 = [v, v, v];
    }
    png.save(output_path)
        .map_err(|e| io_err(format!("writing map PNG: {e}")))?;

    // Keep the measured mean beside the map for bake inspection. `kind = "map"`
    // is an analysis/display product, not a material-albedo product: a caller
    // that binds a grayscale orthophoto as `inputs:albedo_map` must use
    // `kind = "albedo"` so source illumination is removed at the bake boundary.
    if n_measured > 0.0 {
        let mean = sum_measured / n_measured;
        let sidecar = output_path.with_extension("mean");
        std::fs::write(&sidecar, format!("{mean:.6}\n"))?;
        println!("    map mean {mean:.4} (linear contrast; PNG is sRGB-encoded)",);
    }
    Ok(())
}

/// `kind = "albedo"` converts an illumination-bearing grayscale orthophoto
/// into a material colour map.
///
/// A NAC orthophoto is a measured image, not intrinsic reflectance: its broad
/// brightness field contains the acquisition sun/view geometry. Feeding that
/// field directly into `inputs:albedo_map` makes the runtime light it twice.
/// This bake keeps the measured local texture detail, removes that low-frequency
/// field with a valid-sample box filter, and anchors the result at the authored
/// neutral regolith albedo. The output is therefore a stable material albedo,
/// not a claim that an unnormalised orthophoto has become calibrated reflectance.
#[cfg(not(target_arch = "wasm32"))]
fn process_albedo(
    source: &Path,
    output_path: &Path,
    cfg: &ProcessConfig,
    control: &ProcessControl,
) -> Result<(), std::io::Error> {
    control.check()?;
    let ext = source
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "img" {
        return Err(io_err(
            "albedo pipeline requires a grayscale PDS orthophoto `.IMG`; use `texture` for an already-authored colour albedo".into(),
        ));
    }

    let src = decode_gray_source(source)?;
    let (roi, scale, _clat, _clon) = resolve_roi(cfg, &src, "albedo")?;
    let out_n = roi.out_n;
    let crop = roi_samples(&src.samples, src.w, src.h, &roi, control)?;
    reject_if_mostly_nodata(&crop, "albedo", 0.02)?;
    let gray = resample_roi_bilinear(&src.samples, src.w, src.h, &roi, control)?;
    let measured = |v: f64| v.is_finite() && v != 0.0;
    let (lo, hi) = grayscale_percentile_bounds(&gray, measured);
    let span = (hi - lo).max(f64::EPSILON);
    let contrast: Vec<f64> = gray
        .iter()
        .map(|&v| {
            measured(v)
                .then_some(((v - lo) / span).clamp(0.0, 1.0))
                .unwrap_or(f64::NAN)
        })
        .collect();

    let base = cfg.albedo_base_linear.unwrap_or(0.13);
    let detail_strength = cfg.albedo_detail_strength.unwrap_or(0.35);
    let radius_m = cfg.albedo_illumination_radius_m.unwrap_or(40.0);
    if !base.is_finite() || !(0.0..=1.0).contains(&base) || base <= 0.0 {
        return Err(io_err(format!(
            "albedo pipeline requires `albedo_base_linear` in (0, 1], got {base}"
        )));
    }
    if !detail_strength.is_finite() || !(0.0..=1.0).contains(&detail_strength) {
        return Err(io_err(format!(
            "albedo pipeline requires `albedo_detail_strength` in [0, 1], got {detail_strength}"
        )));
    }
    if !radius_m.is_finite() || radius_m <= 0.0 {
        return Err(io_err(format!(
            "albedo pipeline requires a positive `albedo_illumination_radius_m` for an orthophoto, got {radius_m}"
        )));
    }

    let texel_m = (roi.win as f64 * scale) / out_n.max(1) as f64;
    let radius_px = if out_n < 2 {
        0
    } else {
        ((radius_m / texel_m).ceil() as usize).clamp(1, (out_n - 1) / 2)
    };
    let illumination = local_mean_field(&contrast, out_n, radius_px, control)?;
    let mut png = image::RgbImage::new(out_n as u32, out_n as u32);
    for (i, px) in png.pixels_mut().enumerate() {
        if i.is_multiple_of(out_n.max(1) * 64) {
            control.check()?;
        }
        // Invalid source coverage is the neutral material value, never black.
        // Valid texels retain only local texture residuals; broad image
        // illumination is intentionally not reintroduced into the material.
        let delta = if measured(gray[i]) && illumination[i].is_finite() {
            contrast[i] - illumination[i]
        } else {
            0.0
        };
        let linear = (base * (1.0 + detail_strength * delta)).clamp(0.0, 1.0);
        let encoded = encode_linear_to_srgb_u8(linear);
        px.0 = [encoded, encoded, encoded];
    }
    png.save(output_path)
        .map_err(|e| io_err(format!("writing albedo PNG: {e}")))?;
    std::fs::write(output_path.with_extension("mean"), format!("{base:.6}\n"))?;
    println!(
        "    albedo base {base:.4}, local detail {detail_strength:.3}, illumination radius {radius_m:.1} m ({radius_px} px)"
    );
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn grayscale_percentile_bounds<F>(values: &[f64], is_measurement: F) -> (f64, f64)
where
    F: Fn(f64) -> bool,
{
    let mut sorted: Vec<f64> = values
        .iter()
        .copied()
        .filter(|&value| is_measurement(value))
        .collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if sorted.is_empty() {
        return (0.0, 1.0);
    }
    let lo = sorted[(sorted.len() - 1) / 100];
    let hi = sorted[(sorted.len() - 1) * 99 / 100];
    if (hi - lo).abs() < f64::EPSILON {
        (lo, lo + 1.0)
    } else {
        (lo, hi)
    }
}

/// Compute a square valid-sample box mean in O(n²), without turning nodata
/// into an artificial dark or bright illumination source. Integral fields keep
/// the offline bake bounded even for large source crops and give edge pixels
/// the correctly clipped 2D window.
#[cfg(not(target_arch = "wasm32"))]
fn local_mean_field(
    values: &[f64],
    n: usize,
    radius: usize,
    control: &ProcessControl,
) -> Result<Vec<f64>, std::io::Error> {
    if values.len() != n.saturating_mul(n) {
        return Err(io_err(format!(
            "albedo local mean requires {expected} samples, got {actual}",
            expected = n.saturating_mul(n),
            actual = values.len()
        )));
    }
    // A 2D integral field is both simpler and more correct at the image edge
    // than averaging two independently clipped 1D windows: the latter gives
    // corner pixels the wrong weights. Keep a second integral for valid-sample
    // counts so nodata is excluded from the denominator rather than treated as
    // a dark illumination source.
    let stride = n
        .checked_add(1)
        .ok_or_else(|| io_err("albedo local mean dimension overflow".into()))?;
    let cells = stride
        .checked_mul(stride)
        .ok_or_else(|| io_err("albedo local mean integral field overflow".into()))?;
    let mut sums = vec![0.0; cells];
    let mut counts = vec![0u64; cells];
    for y in 0..n {
        control.check()?;
        let mut row_sum = 0.0;
        let mut row_count = 0usize;
        for x in 0..n {
            let value = values[y * n + x];
            if value.is_finite() {
                row_sum += value;
                row_count += 1;
            }
            let current = (y + 1) * stride + (x + 1);
            let above = y * stride + (x + 1);
            sums[current] = sums[above] + row_sum;
            counts[current] = counts[above] + row_count as u64;
        }
    }

    let mut output = vec![f64::NAN; values.len()];
    for y in 0..n {
        control.check()?;
        let y0 = y.saturating_sub(radius);
        let y1 = y.saturating_add(radius.saturating_add(1)).min(n);
        for x in 0..n {
            let x0 = x.saturating_sub(radius);
            let x1 = x.saturating_add(radius.saturating_add(1)).min(n);
            let top_left = y0 * stride + x0;
            let top_right = y0 * stride + x1;
            let bottom_left = y1 * stride + x0;
            let bottom_right = y1 * stride + x1;
            let sum = sums[bottom_right] + sums[top_left] - sums[top_right] - sums[bottom_left];
            let count =
                counts[bottom_right] + counts[top_left] - counts[top_right] - counts[bottom_left];
            output[y * n + x] = (count > 0)
                .then_some(sum / count as f64)
                .unwrap_or(f64::NAN);
        }
    }
    Ok(output)
}

/// Encode a normalized grayscale map signal for an 8-bit PNG loaded as sRGB.
/// The stretch is a linear contrast operation, while PNG's 8-bit colour
/// contract is nonlinear. Keeping the conversion at the bake boundary means
/// the runtime receives the same normalized contrast value it was authored
/// from after Bevy's sRGB decode.
#[cfg(not(target_arch = "wasm32"))]
fn encode_map_contrast_to_srgb_u8(value: f64) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let encoded = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}

#[cfg(not(target_arch = "wasm32"))]
fn encode_linear_to_srgb_u8(value: f64) -> u8 {
    encode_map_contrast_to_srgb_u8(value)
}

/// `kind = "normalmap"` pipeline — derive a DEM-local ENU normal map from the
/// DEM crop and write it as RGB8 PNG (`n * 0.5 + 0.5`).
///
/// Convention matches `lunco-terrain-core::derive::normal_map` and the decode
/// in `terrain_layered.wgsl`: `n = normalize(-dh/dx, 1, -dh/dz)` with `+x` =
/// increasing column (east) and `+z` = increasing row (south, since PDS
/// rasters are north-up) — source/object space, no tangent basis. Consumers
/// transform it through the terrain mesh instance before world-space lighting.
#[cfg(not(target_arch = "wasm32"))]
fn process_normalmap(
    source: &Path,
    output_path: &Path,
    cfg: &ProcessConfig,
    control: &ProcessControl,
) -> Result<(), std::io::Error> {
    control.check()?;
    let src = decode_gray_source(source)?;
    let (roi, scale, _clat, _clon) = resolve_roi(cfg, &src, "normalmap")?;
    let out_n = roi.out_n;
    reject_if_mostly_nodata(
        &roi_samples(&src.samples, src.w, src.h, &roi, control)?,
        "normalmap",
        0.05,
    )?;
    let mut h = resample_roi_bilinear(&src.samples, src.w, src.h, &roi, control)?;

    // Fill voids BEFORE differencing, for the same reason `process_dem` does — and
    // this map is derived from the same heights, so it must reach the same answer
    // or the shading normal disagrees with the geometry it is supposed to describe.
    //
    // This became load-bearing when `resample_roi_bilinear` was fixed to propagate
    // NaN instead of substituting `0.0`. A void now reaches the central difference
    // below, and NaN is total: `dhdx`/`dhdz` → `len` → `n` all become NaN, `enc`'s
    // `f64::clamp` PROPAGATES NaN rather than clamping it, and the `as u8` cast
    // saturates to 0. The texel ships as RGB(0,0,0), which decodes to the unit
    // normal (-1,-1,-1)/√3 — a surface facing into the ground, lit as a black
    // speck. Zeroing the heights instead (the old behaviour) merely flattened the
    // void; propagating NaN without filling it is strictly worse.
    let filled = fill_dem_voids(&mut h, out_n, "normalmap", control)?;
    if filled > 0 {
        println!(
            "    filled {filled} void sample(s) ({:.2}%) before deriving normals",
            100.0 * filled as f64 / h.len().max(1) as f64
        );
    }

    // Metres per output texel — the crop spans `win * scale` metres.
    let step = (roi.win as f64 * scale) / out_n.max(1) as f64;
    let at = |x: isize, z: isize| -> f64 {
        let x = x.clamp(0, out_n as isize - 1) as usize;
        let z = z.clamp(0, out_n as isize - 1) as usize;
        h[z * out_n + x]
    };
    let mut png = image::RgbImage::new(out_n as u32, out_n as u32);
    for z in 0..out_n as isize {
        control.check()?;
        for x in 0..out_n as isize {
            let dhdx = (at(x + 1, z) - at(x - 1, z)) / (2.0 * step);
            let dhdz = (at(x, z + 1) - at(x, z - 1)) / (2.0 * step);
            let len = (dhdx * dhdx + 1.0 + dhdz * dhdz).sqrt();
            let n = [-dhdx / len, 1.0 / len, -dhdz / len];
            let enc = |c: f64| ((c * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
            // NaN-safe at the VECTOR level, not per channel. `fill_dem_voids` above
            // guarantees finite input today, but `f64::clamp` PROPAGATES NaN — it
            // does not clamp it — and the `as u8` cast then saturates to 0, so a
            // single NaN ships RGB(0,0,0), which decodes to (-1,-1,-1)/√3: a normal
            // facing into the ground, lit as a black speck.
            //
            // The fallback must be the neutral UP normal (0,1,0) ⇒ RGB(128,255,128).
            // Encoding 128 on all three channels instead would decode to (0,0,0) —
            // a zero-length normal, which normalises to garbage downstream. Getting
            // that wrong is easy and silent, which is why this branch is explicit.
            let rgb = if n.iter().all(|c| c.is_finite()) {
                [enc(n[0]), enc(n[1]), enc(n[2])]
            } else {
                [128, 255, 128]
            };
            png.put_pixel(x as u32, z as u32, image::Rgb(rgb));
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
    use std::collections::BTreeMap;
    use std::io::Cursor;

    fn control() -> ProcessControl {
        ProcessControl::unrestricted()
    }

    #[test]
    fn gltf_webp_extension_is_detected_without_rewriting_authored_json() {
        let document = draco_gltf::Document::from_json_bytes(
            br#"{"asset":{"version":"2.0"},"textures":[{"extensions":{"EXT_texture_webp":{"source":1}}}]}"#,
        )
        .expect("valid glTF JSON");
        assert!(json_contains_key(document.as_value(), "EXT_texture_webp"));
        assert_eq!(
            document.to_json_bytes().expect("serialize glTF JSON"),
            br#"{"asset":{"version":"2.0"},"textures":[{"extensions":{"EXT_texture_webp":{"source":1}}}]}"#
        );
    }

    #[test]
    fn albedo_illumination_field_is_local_and_nodata_neutral() {
        let values = vec![1.0, 2.0, 3.0, 4.0, f64::NAN, 6.0, 7.0, 8.0, 9.0];
        let mean = local_mean_field(&values, 3, 1, &control()).expect("valid local field");
        assert!(
            (mean[0] - (7.0 / 3.0)).abs() < 1e-9,
            "clipped corner mean: {}",
            mean[0]
        );
        assert!(
            (mean[4] - 5.0).abs() < 1e-9,
            "nodata is excluded: {}",
            mean[4]
        );
        assert!(
            (mean[8] - (23.0 / 3.0)).abs() < 1e-9,
            "clipped corner mean: {}",
            mean[8]
        );

        let flat = vec![0.25; 9];
        let flat_mean = local_mean_field(&flat, 3, 1, &control()).expect("flat field");
        assert!(flat_mean.iter().all(|value| (*value - 0.25).abs() < 1e-9));
    }

    #[test]
    fn rgb_map_bake_writes_identity_normaliser_and_is_installed() {
        let tmp = tempfile::tempdir().expect("temporary map processing directory");
        let source = tmp.path().join("source.tif");
        let mut image = image::RgbImage::new(4, 4);
        for (index, pixel) in image.pixels_mut().enumerate() {
            *pixel = image::Rgb([index as u8, 80, 160]);
        }
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut encoded, image::ImageFormat::Tiff)
            .expect("encode RGB source raster");
        lunco_storage::write_file_sync(&source, encoded.get_ref()).expect("RGB source raster");

        let process = ProcessConfig {
            kind: "map".into(),
            output: "terrain/test-map.png".into(),
            output_root: "cache".into(),
            target_resolution: Some([2, 2]),
            center_lat: Some(0.5),
            center_lon: Some(0.5),
            window_m: Some(2.0),
            pixel_scale_m: 1.0,
            source_height_scale_m_per_unit: 1.0,
            source_height_offset_m: 0.0,
            src_min_lat: Some(0.0),
            src_max_lat: Some(1.0),
            src_min_lon: Some(0.0),
            src_max_lon: Some(1.0),
            site_id: None,
            frame: None,
            albedo_base_linear: None,
            albedo_detail_strength: None,
            albedo_illumination_radius_m: None,
            parameters: BTreeMap::new(),
        };
        process_asset(&source, &process, tmp.path(), None, &control()).expect("RGB map processing");

        let output = tmp.path().join("terrain/test-map.png");
        assert_eq!(
            lunco_storage::read_text_file_sync(&output.with_extension("mean")).unwrap(),
            "1.000000\n"
        );
        assert!(processed_output_present(&output, &process, Some(&source)));
    }

    /// The resampler propagates NaN now, so a void reaches the normal-map gradient.
    /// NaN is total there — `f64::clamp` propagates it and `as u8` saturates to 0,
    /// shipping RGB(0,0,0) = a normal facing INTO the ground. Guard both halves:
    /// voids must be filled, and the encoder must not depend on that having worked.
    #[test]
    fn a_void_never_encodes_as_an_inward_facing_normal() {
        // The neutral UP normal (0,1,0) encodes as (128,255,128) under n*0.5+0.5.
        // The obvious-but-wrong fallback, 128 on every channel, decodes to (0,0,0):
        // zero length, which normalises to garbage in the shader.
        let enc = |c: f64| ((c * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
        assert_eq!([enc(0.0), enc(1.0), enc(0.0)], [128u8, 255, 128]);

        // And the failure mode being guarded against: NaN clamps to NaN. The
        // production encoder must therefore fill voids before quantisation.
        assert!(f64::NAN.clamp(0.0, 255.0).is_nan(), "clamp propagates NaN");
    }

    /// A stereo DTM has voids where matching failed — shadowed crater floors
    /// above all. They must be FILLED, never shipped: a NaN height is a hole in
    /// the render mesh AND the collider, i.e. a hole in the world.
    #[test]
    fn dem_voids_are_filled_from_their_neighbours() {
        let n = 7;
        let mut v = vec![10.0f64; n * n];
        v[3 * n + 3] = f64::NAN; // single interior void
        v[3 * n + 4] = f64::NAN; // and its neighbour, so the fill must iterate
        let filled = fill_dem_voids(&mut v, n, "dem", &control()).expect("fills");
        assert_eq!(filled, 2);
        assert!(v.iter().all(|s| s.is_finite()), "no void may survive");
        // Flat input ⇒ flat output: interpolation must not invent relief.
        assert!(v.iter().all(|s| (s - 10.0).abs() < 1e-9));
    }

    /// The fill must refuse a gap too large to fill honestly rather than invent
    /// terrain across it — that would be a smooth, confident, fictional surface.
    #[test]
    fn an_unfillably_large_void_fails_rather_than_inventing_terrain() {
        let n = 200;
        let mut v = vec![f64::NAN; n * n];
        // One valid rim sample only: the interior is far wider than the pass cap.
        v[0] = 1.0;
        assert!(fill_dem_voids(&mut v, n, "dem", &control()).is_err());
    }

    #[test]
    fn dem_source_height_units_are_converted_before_sampling() {
        let mut samples = vec![1.25, f64::NAN, -2.0];
        apply_dem_height_units(&mut samples, 2000.0, 0.0, &control()).expect("valid conversion");
        assert_eq!(samples[0], 2500.0);
        assert!(samples[1].is_nan(), "nodata must remain non-finite");
        assert_eq!(samples[2], -4000.0);
        assert!(apply_dem_height_units(&mut samples, 0.0, 0.0, &control()).is_err());
        assert!(apply_dem_height_units(&mut samples, 1.0, f64::INFINITY, &control()).is_err());
    }

    #[test]
    fn process_output_root_requires_its_authoritative_root() {
        let mut cfg: ProcessConfig = toml::from_str(
            r#"
            kind = "texture"
            output = "textures/test.png"
            "#,
        )
        .expect("valid process config");

        cfg.output_root = "cache".into();
        assert!(process_output_path(&cfg, None, None).is_err());

        cfg.output_root = "twin".into();
        assert!(process_output_path(&cfg, Some(Path::new("/cache")), None).is_err());

        cfg.output_root = "unknown".into();
        assert!(process_output_path(&cfg, Some(Path::new("/cache")), None).is_err());
    }

    #[test]
    fn cancelled_processing_cannot_commit_a_staged_artifact() {
        let tmp = tempfile::tempdir().expect("temporary processing directory");
        let output = tmp.path().join("texture.png");
        lunco_storage::write_file_sync(&output, b"old").expect("old output");
        lunco_storage::write_file_sync(&bake_stamp_path(&output), b"old-key").expect("old stamp");

        let stage = tmp.path().join(".lunco-process-stage");
        lunco_storage::ensure_directory_sync(&stage).expect("stage directory");
        lunco_storage::write_file_sync(&stage.join("texture.png"), b"new").expect("staged output");
        lunco_storage::write_file_sync(&stage.join("texture.png.bakekey"), b"new-key")
            .expect("staged stamp");

        let control = ProcessControl::unrestricted();
        control.cancel.store(true, Ordering::Release);
        assert!(commit_staged_output(&stage, &output, &[], &control).is_err());
        assert_eq!(
            lunco_storage::read_file_sync(&output).expect("output remains"),
            b"old"
        );
        assert_eq!(
            lunco_storage::read_file_sync(&bake_stamp_path(&output)).expect("stamp remains"),
            b"old-key"
        );
    }

    /// PDS orthorectified products are footprints PADDED to a rectangle with
    /// `CORE_NULL`, so the raster's width is not the width of the data. Clamping a
    /// crop to `0..w` therefore lands in the padding while looking perfectly legal.
    ///
    /// This is the defect that shipped: `ortho.png` and `normal.png` both went dead
    /// at x=2100 of 2500 — every row, 16% of the site — because the window was
    /// inside the raster and nobody had asked whether it was inside the DATA.
    #[test]
    fn valid_bounds_ignore_the_null_padding_around_a_footprint() {
        // 10x4 raster whose right 4 columns are padding.
        let (w, h) = (10usize, 4usize);
        let mut s = vec![f64::NAN; w * h];
        for y in 0..h {
            for x in 0..6 {
                s[y * w + x] = 1.0;
            }
        }
        assert_eq!(
            valid_data_bounds(&s, w, h),
            Some((0, 0, 5, 3)),
            "bounds must stop at the last real column (5), not the last raster column (9)"
        );
    }

    /// An RGB source keeps its data in `planes` and has no NaN sentinel, so there
    /// is nothing to scan — the raster is the bound. Regression guard: an early
    /// version of the clamp rejected every RGB crop with "no finite samples".
    #[test]
    fn valid_bounds_fall_back_to_the_raster_for_plane_sources() {
        assert_eq!(valid_data_bounds(&[], 8, 5), Some((0, 0, 7, 4)));
    }

    /// The backstop. A bounding-box clamp still admits nulls when the imaged strip
    /// is rotated inside its box, and a derived map is consumed as if every texel
    /// were a measurement — the ortho MULTIPLIES albedo, the normal map REPLACES
    /// the shading normal. So a mostly-null crop must fail the bake, not warn:
    /// warning is effectively what the old behaviour was, and an 18%-dead map
    /// shipped anyway and rendered as a flat 3x-bright band.
    #[test]
    fn a_mostly_nodata_crop_fails_the_bake_rather_than_shipping() {
        let mut v = vec![1.0f64; 100];
        v[..10].fill(f64::NAN); // 10% dead
        assert!(
            reject_if_mostly_nodata(&v, "map", 0.02).is_err(),
            "10% nodata must fail a 2% limit"
        );
        assert!(
            reject_if_mostly_nodata(&v, "map", 0.5).is_ok(),
            "and pass when the caller genuinely allows it"
        );
        assert!(
            reject_if_mostly_nodata(&vec![1.0f64; 100], "map", 0.02).is_ok(),
            "a clean crop passes"
        );
        // Empty = RGB source, nothing to judge.
        assert!(reject_if_mostly_nodata(&[], "map", 0.0).is_ok());
    }

    /// Encode a `w*h` row-major f32 raster as an in-memory TIFF — the same
    /// proven pattern `lunco-terrain-bake` uses for its fixtures.
    fn encode_tiff_f32(w: u32, h: u32, data: &[f32]) -> Vec<u8> {
        use tiff::encoder::{colortype, TiffEncoder};
        let mut buf = Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut buf).unwrap();
            enc.write_image::<colortype::Gray32Float>(w, h, data)
                .unwrap();
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

        let tmp = tempfile::tempdir().expect("temporary DEM processing directory");
        let src_path = tmp.path().join("source.tif");
        lunco_storage::write_file_sync(&src_path, &tif).unwrap();

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
            source_height_scale_m_per_unit: 1.0,
            source_height_offset_m: 0.0,
            // Source extent: lat/lon each span [-1, 1] over the 12×8 raster, so
            // (0,0) maps to the centre column/row.
            src_min_lat: Some(-1.0),
            src_max_lat: Some(1.0),
            src_min_lon: Some(-1.0),
            src_max_lon: Some(1.0),
            site_id: Some("testsite".into()),
            frame: Some("MOON_ME".into()),
            albedo_base_linear: None,
            albedo_detail_strength: None,
            albedo_illumination_radius_m: None,
            parameters: BTreeMap::new(),
        };
        let out_dir = tmp.path().join("site");
        process_dem(&src_path, &out_dir, &cfg, &control()).expect("dem process should succeed");

        // heightmap is square float32.
        let out_bytes =
            lunco_storage::read_file_sync(&out_dir.join("materials/textures/heightmap.tif"))
                .unwrap();
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
                assert!(v
                    .iter()
                    .all(|x| (*x as f64) >= -1.0 && (*x as f64) <= 900.0));
            }
            other => panic!("expected F32 heightmap, got {other:?}"),
        }

        // The manifest's frame declaration lands in the raster's own tags.
        // (No metadata.yaml sidecar any more — the geo tags ARE the metadata.)
        let mut tag_dec = tiff::decoder::Decoder::new(Cursor::new(out_bytes.as_slice())).unwrap();
        let geo = lunco_geotiff::read_geo_tags(&mut tag_dec).unwrap();
        assert_eq!(geo.frame, Some(lunco_geotiff::LunarFrame::MoonMe));
    }

    /// `kind = "dem"` must ingest a PDS3 `.IMG` source using the label's own
    /// extent + MAP_SCALE (no `src_*` fields in the manifest) — the
    /// non-GeoTIFF path.
    #[test]
    fn dem_process_ingests_pds_img_via_label_extent() {
        let tmp = tempfile::tempdir().expect("temporary PDS DEM directory");

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
        let src_path = tmp.path().join("source.IMG");
        lunco_storage::write_file_sync(&src_path, &label).unwrap();

        let cfg = ProcessConfig {
            kind: "dem".into(),
            output: "site".into(),
            output_root: "cache".into(),
            target_resolution: Some([4, 4]),
            center_lat: Some(0.0),
            center_lon: Some(0.0),
            window_m: Some(8.0),
            pixel_scale_m: 2.0, // serde default — label's MAP_SCALE governs
            source_height_scale_m_per_unit: 1.0,
            source_height_offset_m: 0.0,
            src_min_lat: None, // absent on purpose: label extent must serve
            src_max_lat: None,
            src_min_lon: None,
            src_max_lon: None,
            site_id: None,
            frame: Some("MOON_ME".into()),
            albedo_base_linear: None,
            albedo_detail_strength: None,
            albedo_illumination_radius_m: None,
            parameters: BTreeMap::new(),
        };
        let out_dir = tmp.path().join("site");
        process_dem(&src_path, &out_dir, &cfg, &control()).expect("PDS IMG dem ingest succeeds");

        let out_bytes =
            lunco_storage::read_file_sync(&out_dir.join("materials/textures/heightmap.tif"))
                .unwrap();
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

        let tmp = tempfile::tempdir().expect("temporary normal-map directory");
        let src_path = tmp.path().join("source.tif");
        lunco_storage::write_file_sync(&src_path, &tif).unwrap();

        let cfg = ProcessConfig {
            kind: "normalmap".into(),
            output: "normal.png".into(),
            output_root: "cache".into(),
            target_resolution: Some([8, 8]),
            center_lat: Some(0.0),
            center_lon: Some(0.0),
            window_m: Some(16.0),
            pixel_scale_m: 1.0,
            source_height_scale_m_per_unit: 1.0,
            source_height_offset_m: 0.0,
            src_min_lat: Some(-1.0),
            src_max_lat: Some(1.0),
            src_min_lon: Some(-1.0),
            src_max_lon: Some(1.0),
            site_id: None,
            frame: None,
            albedo_base_linear: None,
            albedo_detail_strength: None,
            albedo_illumination_radius_m: None,
            parameters: BTreeMap::new(),
        };
        let out_path = tmp.path().join("normal.png");
        process_normalmap(&src_path, &out_path, &cfg, &control()).expect("normalmap succeeds");

        let output = lunco_storage::read_file_sync(&out_path).unwrap();
        let png = image::load_from_memory(&output).unwrap().to_rgb8();
        assert_eq!((png.width(), png.height()), (8, 8));
        let c = png.get_pixel(4, 4).0;
        // Up-slope in +x ⇒ normal tilts to -x: R < 128; no z tilt: B ≈ 128;
        // Y strongly positive.
        assert!(c[0] < 120, "R tilts negative-x on a +x ramp, got {}", c[0]);
        assert!(c[1] > 150, "G (up) stays dominant, got {}", c[1]);
        assert!(
            (c[2] as i32 - 128).abs() <= 6,
            "B stays neutral, got {}",
            c[2]
        );
    }

    /// `kind = "map"` crops an RGB source to the ROI and keeps colour.
    #[test]
    fn map_process_crops_rgb_source() {
        let tmp = tempfile::tempdir().expect("temporary map directory");

        // 16×16 RGB PNG: left half red, right half green.
        let mut img = image::RgbImage::new(16, 16);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            *p = if x < 8 {
                image::Rgb([200, 10, 10])
            } else {
                image::Rgb([10, 200, 10])
            };
        }
        let src_path = tmp.path().join("source.png");
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        lunco_storage::write_file_sync(&src_path, encoded.get_ref()).unwrap();

        let cfg = ProcessConfig {
            kind: "map".into(),
            output: "map.png".into(),
            output_root: "cache".into(),
            target_resolution: Some([8, 8]),
            center_lat: Some(0.0),
            center_lon: Some(0.0),
            window_m: Some(16.0),
            pixel_scale_m: 1.0,
            source_height_scale_m_per_unit: 1.0,
            source_height_offset_m: 0.0,
            src_min_lat: Some(-1.0),
            src_max_lat: Some(1.0),
            src_min_lon: Some(-1.0),
            src_max_lon: Some(1.0),
            site_id: None,
            frame: None,
            albedo_base_linear: None,
            albedo_detail_strength: None,
            albedo_illumination_radius_m: None,
            parameters: BTreeMap::new(),
        };
        let out_path = tmp.path().join("map.png");
        process_map(&src_path, &out_path, &cfg, &control()).expect("map crop succeeds");

        let output = lunco_storage::read_file_sync(&out_path).unwrap();
        let out = image::load_from_memory(&output).unwrap().to_rgb8();
        assert_eq!((out.width(), out.height()), (8, 8));
        assert!(out.get_pixel(1, 4).0[0] > 100, "west side stays red");
        assert!(out.get_pixel(6, 4).0[1] > 100, "east side stays green");
    }

    /// A grayscale crop carrying a nodata margin must still stretch on its REAL
    /// samples, and must bake nodata as WHITE (the neutral element of the
    /// multiply these maps feed), never black.
    ///
    /// This is the shape of the shipped Apollo-15 `ortho.png` bug: the nodata
    /// margin reached the 1–99 percentile stretch as a finite `-3.4e38`, became
    /// the 1st percentile, and flattened every real sample to pure white while
    /// the margin itself went pure black. `terrain_layered.wgsl` multiplies
    /// that map into the albedo, so the margin rendered as an unlit void.
    #[test]
    fn map_gray_stretch_ignores_nodata_and_bakes_it_neutral() {
        // A real gradient with a NaN margin, exactly as the decoder now yields.
        let n = 16usize;
        let mut gray = vec![0.0f64; n * n];
        for y in 0..n {
            for x in 0..n {
                // Right quarter is nodata; the rest ramps over a NARROW range,
                // like real elevations/radiance in one small crop.
                gray[y * n + x] = if x >= n * 3 / 4 {
                    f64::NAN
                } else {
                    -1900.0 + (x as f64) * 0.5
                };
            }
        }

        let mut sorted: Vec<f64> = gray
            .iter()
            .copied()
            .filter(|v| v.is_finite() && *v != 0.0)
            .collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let lo = sorted[(sorted.len() - 1) / 100];
        let hi = sorted[(sorted.len() - 1) * 99 / 100];
        assert!(
            lo > -2000.0 && hi > lo,
            "stretch bounds come from real samples, not the sentinel: lo={lo} hi={hi}"
        );

        let bake = |v: f64| -> u8 {
            if v.is_finite() {
                encode_map_contrast_to_srgb_u8((v - lo) / (hi - lo))
            } else {
                255
            }
        };
        assert_eq!(bake(f64::NAN), 255, "nodata is neutral (x1), never 0");
        assert_eq!(encode_map_contrast_to_srgb_u8(0.0), 0);
        assert!((187..=189).contains(&encode_map_contrast_to_srgb_u8(0.5)));
        assert_eq!(encode_map_contrast_to_srgb_u8(1.0), 255);
        // Real samples must actually use the range, not collapse to one level.
        let lo_px = bake(-1900.0);
        let hi_px = bake(-1900.0 + 11.0 * 0.5);
        assert!(
            hi_px.abs_diff(lo_px) > 100,
            "real samples span the range, got {lo_px}..{hi_px}"
        );
    }
}
