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
use resvg::tiny_skia;
use usvg::{Tree, Options};
use crate::cache_dir;

/// Processing configuration from `Assets.toml`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProcessConfig {
    /// Target [width, height] in pixels.
    pub target_resolution: [u32; 2],
    /// Output path relative to the cache root (e.g., "textures/earth.png").
    pub output: String,
}

/// Processes a single source image → output texture.
///
/// Supports: JPEG, PNG, TIFF, SVG
/// Output: PNG
#[cfg(not(target_arch = "wasm32"))]
pub fn process_texture(
    source_path: &Path,
    process: &ProcessConfig,
) -> Result<(), std::io::Error> {
    let output_path = cache_dir().join(&process.output);
    let [tw, th] = process.target_resolution;

    // Create output directory if needed
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let ext = source_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    match ext {
        "svg" => process_svg(source_path, &output_path, tw, th),
        "jpg" | "jpeg" | "png" | "tiff" | "tif" | "bmp" | "webp" => {
            process_image(source_path, &output_path, tw, th)
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Unsupported source format: .{}", ext),
        )),
    }?;

    println!("  ✓ processed → {}", output_path.display());
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
