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
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProcessConfig {
    /// Pipeline selector. Defaults to `"texture"` for backwards
    /// compatibility with Earth/Moon entries.
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Target [width, height] in pixels (texture pipeline only).
    #[serde(default)]
    pub target_resolution: Option<[u32; 2]>,
    /// Output path **relative to** [`ProcessConfig::output_root`].
    /// Examples: `"textures/earth.png"`, `"models/perseverance.glb"`.
    pub output: String,
    /// Where to write the processed file. Two values today:
    /// - `"cache"` (default) — writes under the shared cache root
    ///   (`<LUNCOSIM_CACHE>/...`). Used by Earth/Moon textures and
    ///   other regeneratable artifacts that don't need to live in
    ///   the source tree.
    /// - `"assets"` — writes under the workspace `assets/` directory
    ///   (gitignored if the path matches a `.gitignore` rule). Used
    ///   for files USD `payload`/`references` need to find via
    ///   layer-relative paths — Bevy's default `assets://` source
    ///   resolves them, and so does Blender / usdview / Houdini.
    #[serde(default = "default_output_root")]
    pub output_root: String,
}

fn default_kind() -> String {
    "texture".to_string()
}

fn default_output_root() -> String {
    "cache".to_string()
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
#[cfg(not(target_arch = "wasm32"))]
pub fn process_texture(
    source_path: &Path,
    process: &ProcessConfig,
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
        _ => cache_dir().join(&process.output),
    };

    // Create output directory if needed
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    match process.kind.as_str() {
        "gltf" => process_gltf(source_path, &output_path)?,
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
                format!("Unknown process kind `{}` (expected \"texture\" or \"gltf\")", other),
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

    processed.save(output)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
}
