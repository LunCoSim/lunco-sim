//! Mask-aware spherical raster reprojection, independent of a named site.

use crate::process::ProcessControl;
use lunco_assets_datasets::ProcessConfig;
use lunco_geotiff::SphericalRaster;

pub(crate) struct SampledRaster {
    pub samples: Vec<f64>,
    pub resolution: usize,
    pub span_m: f64,
    pub spacing_m: f64,
    pub center_lat: f64,
    pub center_lon: f64,
    pub radius_m: f64,
}

pub(crate) fn sample(
    geo: &SphericalRaster,
    samples: &[f64],
    width: usize,
    height: usize,
    config: &ProcessConfig,
    nodes: bool,
    control: &ProcessControl,
) -> std::io::Result<SampledRaster> {
    let invalid = |s: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, s);
    let lat = config
        .center_lat
        .ok_or_else(|| invalid("projected crop requires center_lat"))?;
    let lon = config
        .center_lon
        .ok_or_else(|| invalid("projected crop requires center_lon"))?;
    let span = config
        .window_m
        .ok_or_else(|| invalid("projected crop requires window_m"))?;
    let [w, h] = config
        .target_resolution
        .ok_or_else(|| invalid("projected crop requires target_resolution"))?;
    if !lat.is_finite()
        || lat.abs() >= 90.0
        || !lon.is_finite()
        || !span.is_finite()
        || span <= 0.0
        || w != h
        || w < 2
        || w > 16385
        || samples.len() != width.saturating_mul(height)
    {
        return Err(invalid(
            "projected crop needs finite coordinates, positive span, and a square resolution in 2..=16385",
        ));
    }
    if lat.abs() + (span / (2.0 * geo.radius_m)).to_degrees() >= 90.0 {
        return Err(invalid(
            "crop-centred equirectangular output cannot cross a pole",
        ));
    }
    let n = w as usize;
    let spacing = span / if nodes { (n - 1) as f64 } else { n as f64 };
    let shift = if nodes { 0.0 } else { 0.5 };
    let mut out = Vec::with_capacity(n * n);
    for row in 0..n {
        control.check()?;
        for col in 0..n {
            let east = -span / 2.0 + (col as f64 + shift) * spacing;
            let north = span / 2.0 - (row as f64 + shift) * spacing;
            let [latitude, longitude] = geo.geographic_offset(lat, lon, east, north);
            let [x, y] = geo.pixel_at(latitude, longitude);
            if !x.is_finite()
                || !y.is_finite()
                || x < 0.0
                || y < 0.0
                || x > (width - 1) as f64
                || y > (height - 1) as f64
            {
                out.push(f64::NAN);
                continue;
            }
            let x0 = x.floor() as usize;
            let y0 = y.floor() as usize;
            let x1 = (x0 + 1).min(width - 1);
            let y1 = (y0 + 1).min(height - 1);
            let tx = x - x0 as f64;
            let ty = y - y0 as f64;
            let mut sum = 0.0;
            let mut weight = 0.0;
            for (index, w) in [
                (y0 * width + x0, (1.0 - tx) * (1.0 - ty)),
                (y0 * width + x1, tx * (1.0 - ty)),
                (y1 * width + x0, (1.0 - tx) * ty),
                (y1 * width + x1, tx * ty),
            ] {
                let value = samples[index];
                if value.is_finite() {
                    sum += value * w;
                    weight += w;
                }
            }
            out.push(if weight > 1e-12 {
                sum / weight
            } else {
                f64::NAN
            });
        }
    }
    Ok(SampledRaster {
        samples: out,
        resolution: n,
        span_m: span,
        spacing_m: spacing,
        center_lat: lat,
        center_lon: lon,
        radius_m: geo.radius_m,
    })
}
