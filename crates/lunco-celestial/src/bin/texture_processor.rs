use std::fs;
use std::path::{Path, PathBuf};
use resvg::tiny_skia;
use usvg::{Tree, Options};
use image::GenericImageView;
use serde::Deserialize;

const CACHE_DIR: &str = ".cache/textures";
const METADATA_DIR: &str = "assets/textures";

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

    fs::create_dir_all(CACHE_DIR)?;

    let entries = fs::read_dir(METADATA_DIR)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            let base_name = path.file_stem().and_then(|s| s.to_str()).unwrap();
            println!("Reading metadata for {}...", base_name);
            let meta: TextureMetadata = serde_json::from_str(&fs::read_to_string(&path)?)?;
            process_texture(base_name, &meta)?;
        }
    }

    println!("All textures refined and processed successfully in cache!");
    Ok(())
}

fn process_texture(base_name: &str, meta: &TextureMetadata) -> Result<(), Box<dyn std::error::Error>> {
    let cache_path = format!("{}/{}.png", CACHE_DIR, base_name);
    println!("Processing {}: {}...", meta.name, cache_path);

    // 1. Resolve source path (download if source_url is present)
    let source_path = if let Some(url) = &meta.source_url {
        let url_path = url.split('?').next().unwrap_or(url);
        let ext = Path::new(url_path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("bin");
        let scp = format!("{}/{}_source.{}", CACHE_DIR, base_name, ext);

        if !Path::new(&scp).exists() {
            println!("Downloading source to {}...", scp);
            let response = ureq::get(url).call()?;
            let mut file = fs::File::create(&scp)?;
            std::io::copy(&mut response.into_reader(), &mut file)?;
            println!("Download complete.");
        }
        scp
    } else if let Some(sp) = &meta.source_path {
        sp.clone()
    } else {
        return Err("Missing source_path or source_url".into());
    };

    let source_ext = Path::new(&source_path).extension().and_then(|s| s.to_str()).unwrap_or("");
    let [tw, th] = meta.target_resolution;

    if source_ext == "svg" {
        println!("Rendering SVG {} -> {}...", source_path, cache_path);
        let svg_data = fs::read(&source_path)?;
        let mut opt = Options::default();
        opt.resources_dir = Some(PathBuf::from("assets/maps"));
        #[cfg(feature = "text")]
        {
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
        println!("Converting Image {} -> {}...", source_path, cache_path);
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

    println!("Success: {}", cache_path);
    Ok(())
}
