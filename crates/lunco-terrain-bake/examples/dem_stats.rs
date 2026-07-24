//! Report a DEM raster's valid-data coverage: how much of the crop is real
//! measurement, and where the measured region actually sits.
//!
//! `cargo run -p lunco-terrain-bake --example dem_stats -- <path.tif>`

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: dem_stats <path.tif>");
    let bytes = std::fs::read(&path).expect("read raster");
    let (w, h, heights) = lunco_terrain_bake::dem::decode_geotiff_f64(&bytes).expect("decode");

    let total = heights.len();
    let valid = heights.iter().filter(|v| v.is_finite()).count();
    println!(
        "raster {w}x{h}  valid {valid}/{total} ({:.1}%)",
        100.0 * valid as f64 / total as f64
    );

    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in heights.iter().filter(|v| v.is_finite()) {
        lo = lo.min(*v);
        hi = hi.max(*v);
    }
    println!("elevation range: {lo:.1} .. {hi:.1} m");

    // Bounding box of the measured region, in pixels.
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for (i, v) in heights.iter().enumerate() {
        if v.is_finite() {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    println!(
        "measured bbox: x {x0}..={x1}  y {y0}..={y1}  ({} x {})",
        x1 - x0 + 1,
        y1 - y0 + 1
    );

    // Per-row / per-column validity, coarsely — shows whether the hole is an
    // edge margin (croppable) or interior gaps (not).
    let bar = |frac: f64| "#".repeat((frac * 40.0).round() as usize);
    println!("\nrow validity (every {}th):", h / 24);
    for y in (0..h).step_by((h / 24).max(1)) {
        let n = (0..w).filter(|&x| heights[y * w + x].is_finite()).count();
        println!(
            "  y{y:>6} {:>5.1}% {}",
            100.0 * n as f64 / w as f64,
            bar(n as f64 / w as f64)
        );
    }
    println!("\ncol validity (every {}th):", w / 24);
    for x in (0..w).step_by((w / 24).max(1)) {
        let n = (0..h).filter(|&y| heights[y * w + x].is_finite()).count();
        println!(
            "  x{x:>6} {:>5.1}% {}",
            100.0 * n as f64 / h as f64,
            bar(n as f64 / h as f64)
        );
    }
}
