//! Check a prepared local/regional DEM pair with an explicitly declared frame.
//!
//! Usage: regional_probe <local.tif> <regional.tif> <MOON_ME|MOON_PA> <blend_m> <regional_offset_m> [diagnostic-output.tif]

use std::sync::Arc;

use lunco_geotiff::LunarFrame;
use lunco_terrain_bake::{dem, regional::RegisteredRegionalSurface};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(5..=6).contains(&args.len()) {
        return Err("usage: regional_probe <local.tif> <regional.tif> <MOON_ME|MOON_PA> <blend_m> <regional_offset_m> [diagnostic-output.tif]".into());
    }
    let frame = LunarFrame::parse(&args[2]).ok_or("unrecognized declared lunar frame")?;
    let blend: f64 = args[3].parse()?;
    let offset: f64 = args[4].parse()?;
    let local_bytes = std::fs::read(&args[0])?;
    let regional_bytes = std::fs::read(&args[1])?;
    let (local, mut local_geo) = dem::height_grid_and_transform(&local_bytes)?;
    if local_geo.frame.is_some_and(|actual| actual != frame) {
        return Err("declared local frame conflicts with the GeoTIFF".into());
    }
    local_geo.frame = Some(frame);
    let (regional, regional_geo) = dem::height_grid_and_transform(&regional_bytes)?;
    let local = Arc::new(local);
    let regional = Arc::new(regional);
    let half = f64::from(local.half_extent);
    let pair = RegisteredRegionalSurface::new(
        local.clone(),
        local_geo,
        regional,
        regional_geo,
        blend,
        offset,
    )?;
    let mut max_edge_slope_jump = 0.0_f64;
    let mut max_enclosure_width = 0.0_f64;
    let step = 0.001;
    for side in [-1.0, 1.0] {
        for i in 0..=64 {
            let along = -half + 2.0 * half * i as f64 / 64.0;
            for x_edge in [true, false] {
                let point = |normal: f64| {
                    if x_edge {
                        (normal, along)
                    } else {
                        (along, normal)
                    }
                };
                let edge = side * half;
                let (x, z) = point(edge);
                let at_edge = pair.height_at(x, z)?;
                let rect = [x - 0.05, z - 0.05, x + 0.05, z + 0.05];
                let bounds = pair.height_bounds(rect, &mut 10_000)?;
                max_enclosure_width = max_enclosure_width.max(bounds[1] - bounds[0]);
                for (px, pz) in [
                    (rect[0], rect[1]),
                    (rect[2], rect[1]),
                    (rect[0], rect[3]),
                    (rect[2], rect[3]),
                    (x, z),
                ] {
                    let height = pair.height_at(px, pz)?;
                    if height < bounds[0] || height > bounds[1] {
                        return Err("regional height escaped its enclosure".into());
                    }
                }
                if at_edge.to_bits() != local.height_at(x, z).to_bits() {
                    return Err("regional sampling changed the local boundary".into());
                }
                let (xi, zi) = point(edge - side * step);
                let (xo, zo) = point(edge + side * step);
                let inner = (at_edge - pair.height_at(xi, zi)?) / step;
                let outer = (pair.height_at(xo, zo)? - at_edge) / step;
                max_edge_slope_jump = max_edge_slope_jump.max((inner - outer).abs());
            }
        }
    }
    println!(
        "local half extent: {half} m; transition: {blend} m; frame: {}",
        frame.as_str()
    );
    println!(
        "local center: {} m; composed center: {} m",
        local.height_at(0.0, 0.0),
        pair.height_at(0.0, 0.0)?
    );
    let mut differences = Vec::new();
    for row in 0..=32 {
        for col in 0..=32 {
            let x = -half + 2.0 * half * col as f64 / 32.0;
            let z = -half + 2.0 * half * row as f64 / 32.0;
            differences.push(pair.regional_grid().height_at(x, z) + offset - local.height_at(x, z));
        }
    }
    let rms = (differences.iter().map(|d| d * d).sum::<f64>() / differences.len() as f64).sqrt();
    differences.sort_by(f64::total_cmp);
    println!(
        "regional minus local over {} interior samples: median {} m; RMS {} m; range [{}, {}] m",
        differences.len(),
        differences[differences.len() / 2],
        rms,
        differences[0],
        differences[differences.len() - 1]
    );
    println!("maximum edge slope difference at 1 mm stencil: {max_edge_slope_jump}");
    println!("maximum height enclosure width for 0.1 m edge windows: {max_enclosure_width} m");
    let mut remaining = 10_000;
    let broad = pair.height_bounds([half - 10.0, -10.0, half + 10.0, 10.0], &mut remaining)?;
    println!(
        "20 m edge enclosure width: {} m; examined cells: {}",
        broad[1] - broad[0],
        10_000 - remaining
    );
    println!("surface key: {:016x}", pair.content_key());
    if let Some(output) = args.get(5) {
        use tiff::encoder::{TiffEncoder, colortype};
        let grid = pair.regional_grid();
        let half = f64::from(grid.half_extent);
        let step = half * 2.0 / (grid.res - 1) as f64;
        let mut heights = Vec::with_capacity(grid.res * grid.res);
        for row in 0..grid.res {
            for col in 0..grid.res {
                heights.push(pair.height_at(-half + col as f64 * step, -half + row as f64 * step)?);
            }
        }
        let mut encoder = TiffEncoder::new(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(output)?,
        )?;
        let mut image =
            encoder.new_image::<colortype::Gray64Float>(grid.res as u32, grid.res as u32)?;
        lunco_geotiff::write_geo_tags(image.encoder(), &regional_geo, "Moon 2000")?;
        image.write_data(&heights)?;
        println!("diagnostic composite written: {output}");
    }
    Ok(())
}
