//! Texture processor — converts SVG/source images into cached PNG textures.
//!
//! Reads metadata from `assets/textures/*.json`, downloads or locates source
//! images, rescales them, and writes output to the shared cache directory
//! (resolved via `lunco_assets::textures_dir()` — typically `~/.cache/luncosim/textures/`).

use std::fs;
use std::path::{Path, PathBuf};
use resvg::tiny_skia;
use usvg::{Tree, Options};
use image::GenericImageView;
use serde::Deserialize;
use lunco_assets::{assets_dir, textures_dir};

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct TextureMetadata {
    name: String,
    source_path: Option<String>,
    source_url: Option<String>,
    target_resolution: [u32; 2],
    license: Option<String>,
    license_url: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting refined texture processor...");

    let cache_dir = textures_dir();
    fs::create_dir_all(&cache_dir)?;

    let metadata_dir = assets_dir().join("textures");
    let entries = fs::read_dir(&metadata_dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            let base_name = path.file_stem().and_then(|s| s.to_str()).unwrap();
            println!("Reading metadata for {}...", base_name);
            let meta: TextureMetadata = serde_json::from_str(&fs::read_to_string(&path)?)?;
            process_texture(base_name, &meta, &cache_dir)?;
        }
    }

    println!("All textures refined and processed successfully in cache!");
    Ok(())
}

fn process_texture(base_name: &str, meta: &TextureMetadata, cache_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let cache_path = cache_dir.join(format!("{base_name}.png"));
    println!("Processing {}: {}...", meta.name, cache_path.display());

    // 1. Resolve source path (download if source_url is present)
    let source_path = if let Some(url) = &meta.source_url {
        let url_path = url.split('?').next().unwrap_or(url);
        let ext = Path::new(url_path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("bin");
        let scp = cache_dir.join(format!("{base_name}_source.{ext}"));

        if !scp.exists() {
            println!("Downloading source to {}...", scp.display());
            let response = ureq::get(url).call()?;
            let mut file = fs::File::create(&scp)?;
            std::io::copy(&mut response.into_reader(), &mut file)?;
            println!("Download complete.");
        }
        scp
    } else if let Some(sp) = &meta.source_path {
        PathBuf::from(sp)
    } else {
        return Err("Missing source_path or source_url".into());
    };

    let source_ext = source_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let [tw, th] = meta.target_resolution;

    if source_ext == "svg" {
        println!("Rendering SVG {} -> {}...", source_path.display(), cache_path.display());
        let svg_data = fs::read(&source_path)?;
        let mut opt = Options::default();
        opt.resources_dir = Some(assets_dir().join("maps"));
        #[cfg(feature = "text")]
        {
            use std::sync::Arc;
            let mut fontdb = usvg::fontdb::Database::new();
            fontdb.load_system_fonts();
            opt.fontdb = Arc::new(fontdb);
        }
        let tree = Tree::from_data(&svg_data, &opt)?;
        let size = tree.size();
        let mut pixmap = tiny_skia::Pixmap::new(tw, th).unwrap();
        let transform = tiny_skia::Transform::from_scale(
            tw as f32 / size.width(),
            th as f32 / size.height(),
        );
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        pixmap.save_png(&cache_path)?;
    } else {
        println!("Converting Image {} -> {}...", source_path.display(), cache_path.display());
        let img = image::open(&source_path)?;
        let (w, h) = img.dimensions();
        let processed_img = if w != tw || h != th {
            println!("Resizing texture to {}x{}...", tw, th);
            img.resize_exact(tw, th, image::imageops::FilterType::Lanczos3)
        } else {
            img
        };
        processed_img.save(&cache_path)?;
    }

    println!("Success: {}", cache_path.display());
    Ok(())
}
