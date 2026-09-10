//! Generate the maintained DEM used by the production terrain-rocks scene.
//!
//! The fixture is deliberately generated from a small, deterministic relief
//! function instead of copied from a Twin cache. It is a real, georeferenced
//! float32 GeoTIFF with non-flat relief, so the production scene exercises the
//! same DEM reader, terrain oracle, streamed tiles, collider ring, and rocks
//! scatter path as a downloaded site without making external data part of the
//! repository.
//!
//! ```text
//! cargo run -p lunco-assets --example generate_rocks_fixture -- \
//!   assets/scenes/tests/terrain/rocks_fixture
//! ```

use std::{env, fs::File, path::PathBuf};

use lunco_geotiff::{GeoTransform, LunarFrame};
use tiff::encoder::{colortype, TiffEncoder};

const SAMPLE_SIDE: usize = 257;
const TERRAIN_SIDE_M: f64 = 1_000.0;
const SITE_LAT_DEG: f64 = 26.0371;
const SITE_LON_DEG: f64 = 3.6584;

fn relief(x: f64, z: f64) -> f32 {
    // Broad measured-style landforms plus two smaller ridges. This is not a
    // flat slab: the camera and physics see continuous, multi-scale relief.
    let broad = 18.0 * (x / 145.0).sin() * (z / 175.0).cos();
    let ridge = 9.0 * ((x + 0.7 * z) / 92.0).sin().powi(2);
    let basin = -7.0 * (-((x - 145.0).powi(2) + (z + 115.0).powi(2)) / 38_000.0).exp();
    (broad + ridge + basin) as f32
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: generate_rocks_fixture <output-site-directory>")?;
    let texture_dir = output.join("materials/textures");
    std::fs::create_dir_all(&texture_dir)?;

    let half = TERRAIN_SIDE_M * 0.5;
    let mut heights = Vec::with_capacity(SAMPLE_SIDE * SAMPLE_SIDE);
    for row in 0..SAMPLE_SIDE {
        let z = -half + TERRAIN_SIDE_M * row as f64 / (SAMPLE_SIDE - 1) as f64;
        for col in 0..SAMPLE_SIDE {
            let x = -half + TERRAIN_SIDE_M * col as f64 / (SAMPLE_SIDE - 1) as f64;
            heights.push(relief(x, z));
        }
    }

    let geo = GeoTransform::centred_square(
        TERRAIN_SIDE_M,
        SAMPLE_SIDE,
        lunco_core::MOON_MEAN_RADIUS_M,
        SITE_LAT_DEG,
        SITE_LON_DEG,
    )
    .with_frame(LunarFrame::MoonMe);
    let tif_path = texture_dir.join("heightmap.tif");
    let mut encoder = TiffEncoder::new(File::create(&tif_path)?)?;
    let mut image =
        encoder.new_image::<colortype::Gray32Float>(SAMPLE_SIDE as u32, SAMPLE_SIDE as u32)?;
    lunco_geotiff::write_geo_tags(image.encoder(), &geo, "Moon 2000")?;
    image.write_data(&heights)?;

    println!(
        "generated {} ({}x{}, {:.1} m side, deterministic relief)",
        tif_path.display(),
        SAMPLE_SIDE,
        SAMPLE_SIDE,
        TERRAIN_SIDE_M
    );
    Ok(())
}
