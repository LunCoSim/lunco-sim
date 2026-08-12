//! Probe a baked DEM crop for surface height and SUN VISIBILITY at scene-local
//! XZ, and search for the nearest sunlit, gentle patch.
//!
//! `cargo run -p lunco-terrain-bake --example site_probe -- <path.tif> <windowM> <sunAzDeg> <sunElDeg> <x> <z> [searchRadiusM]`
//!
//! Grid → scene mapping is the twin's: `x=(col-W/2)*sp, z=(row-H/2)*sp`,
//! `sp = windowM / W`. Row 0 = north, col 0 = west (north = -Z, east = +X).

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let path = &a[0];
    let window_m: f64 = a[1].parse().unwrap();
    let sun_az: f64 = a[2].parse().unwrap();
    let sun_el: f64 = a[3].parse().unwrap();
    let px: f64 = a[4].parse().unwrap();
    let pz: f64 = a[5].parse().unwrap();
    let radius: f64 = a.get(6).map(|s| s.parse().unwrap()).unwrap_or(0.0);

    let bytes = std::fs::read(path).expect("read raster");
    let (w, h, hs) = lunco_terrain_bake::dem::decode_geotiff_f64(&bytes).expect("decode");
    let sp = window_m / w as f64;

    let at = |x: f64, z: f64| -> f64 {
        let c = (x / sp + w as f64 / 2.0).round().clamp(0.0, (w - 1) as f64) as usize;
        let r = (z / sp + h as f64 / 2.0).round().clamp(0.0, (h - 1) as f64) as usize;
        hs[r * w + c]
    };
    // Horizontal unit vector TOWARD the sun. Azimuth is from north, clockwise
    // through east; north = -Z, east = +X.
    let (sx, sz) = (sun_az.to_radians().sin(), -sun_az.to_radians().cos());
    // Horizon angle toward the sun: the max elevation angle of terrain along
    // the solar azimuth. Lit iff it stays below the sun's own elevation.
    let horizon = |x: f64, z: f64| -> f64 {
        let h0 = at(x, z) + 1.5; // rover eye/deck height
        let mut worst: f64 = -90.0;
        let mut d = sp;
        while d < 3000.0 {
            let hx = at(x + sx * d, z + sz * d);
            if hx.is_finite() {
                worst = worst.max(((hx - h0) / d).atan().to_degrees());
            }
            d += sp.max(1.0);
        }
        worst
    };
    // Worst slope over a 4 m baseline in the 8 compass directions.
    let slope = |x: f64, z: f64| -> f64 {
        let h0 = at(x, z);
        let mut worst: f64 = 0.0;
        for k in 0..8 {
            let a = (k as f64) * std::f64::consts::FRAC_PI_4;
            let d = ((at(x + a.sin() * 4.0, z - a.cos() * 4.0) - h0) / 4.0)
                .atan()
                .to_degrees()
                .abs();
            worst = worst.max(d);
        }
        worst
    };

    println!("raster {w}x{h}  window {window_m} m  spacing {sp:.4} m/px");
    println!("sun az {sun_az} el {sun_el}  (horizontal dir x{sx:+.3} z{sz:+.3})");
    let hz = horizon(px, pz);
    println!(
        "\nprobe ({px}, {pz}): height {:.2}  horizon {:.2}°  {}  slope4m {:.1}°",
        at(px, pz),
        hz,
        if hz < sun_el { "LIT" } else { "IN SHADOW" },
        slope(px, pz)
    );

    if radius < 0.0 {
        // ASCII map of the crop: which ground the sun actually reaches.
        let half = -radius;
        let step = 40.0;
        let n = (half / step) as i32;
        println!("\n  '#' shadow   ',' lit but steep (>6°)   '.' lit + flat   'P' probe point");
        print!("\n        ");
        for ix in -n..=n {
            print!("{}", if (ix + n) % 5 == 0 { '|' } else { ' ' });
        }
        println!();
        for iz in -n..=n {
            let z = iz as f64 * step;
            print!("z{z:>6.0} ");
            for ix in -n..=n {
                let x = ix as f64 * step;
                let c = if (x - px).abs() < step / 2.0 && (z - pz).abs() < step / 2.0 {
                    'P'
                } else if horizon(x, z) > sun_el {
                    '#'
                } else if slope(x, z) > 6.0 {
                    ','
                } else {
                    '.'
                };
                print!("{c}");
            }
            println!();
        }
        print!("        ");
        for ix in -n..=n {
            print!("{}", if (ix + n) % 5 == 0 { '|' } else { ' ' });
        }
        println!("\n  x from {:.0} to {:.0}, step {step} m", -half, half);
        return;
    }

    if radius > 0.0 {
        // Nearest patch that is sunlit with margin AND flat enough to park on.
        let mut best: Option<(f64, f64, f64, f64, f64)> = None;
        let step = 10.0;
        let n = (radius / step) as i32;
        for iz in -n..=n {
            for ix in -n..=n {
                let (x, z) = (px + ix as f64 * step, pz + iz as f64 * step);
                if x.abs() > window_m / 2.0 - 50.0 || z.abs() > window_m / 2.0 - 50.0 {
                    continue;
                }
                let d = ((x - px).powi(2) + (z - pz).powi(2)).sqrt();
                if d > radius {
                    continue;
                }
                let hz = horizon(x, z);
                if hz > sun_el - 2.0 || slope(x, z) > 6.0 {
                    continue;
                }
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, x, z, at(x, z), hz));
                }
            }
        }
        match best {
            Some((d, x, z, y, hz)) => println!(
                "\nnearest LIT + flat patch: ({x}, {z})  {d:.0} m away  height {y:.2}  horizon {hz:.2}°  slope4m {:.1}°",
                slope(x, z)
            ),
            None => println!("\nno lit+flat patch within {radius} m"),
        }
    }
}
