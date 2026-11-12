//! Mask-aware spherical raster reprojection, independent of a named site.

use crate::process::ProcessControl;
use lunco_assets_datasets::ProcessConfig;
use lunco_geotiff::{LocalProjection, SphericalRaster};

pub(crate) struct SampledRaster {
    pub samples: Vec<f64>,
    pub resolution: usize,
    pub span_m: f64,
    pub spacing_m: f64,
    pub center_lat: f64,
    pub center_lon: f64,
    pub radius_m: f64,
    pub projection: LocalProjection,
}

struct OutputGrid {
    lat: f64,
    lon: f64,
    span: f64,
    resolution: usize,
    spacing: f64,
    shift: f64,
    projection: LocalProjection,
}

impl OutputGrid {
    fn new(geo: &SphericalRaster, config: &ProcessConfig, nodes: bool) -> std::io::Result<Self> {
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid projected crop window",
            )
        };
        let lat = config.center_lat.ok_or_else(invalid)?;
        let lon = config.center_lon.ok_or_else(invalid)?;
        let span = config.window_m.ok_or_else(invalid)?;
        let [w, h] = config.target_resolution.ok_or_else(invalid)?;
        if !lat.is_finite()
            || lat.abs() > 90.0
            || !lon.is_finite()
            || !span.is_finite()
            || span <= 0.0
            || w != h
            || !(2..=16385).contains(&w)
            || !geo.radius_m.is_finite()
            || geo.radius_m <= 0.0
        {
            return Err(invalid());
        }
        let projection = match config.parameters.get("output_projection") {
            None => LocalProjection::Equirectangular,
            Some(toml::Value::String(value)) if value == "equirectangular" => {
                LocalProjection::Equirectangular
            }
            Some(toml::Value::String(value)) if value == "orthographic" => {
                LocalProjection::Orthographic
            }
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "output_projection must be 'equirectangular' or 'orthographic'",
                ));
            }
        };
        if projection == LocalProjection::Equirectangular
            && lat.abs() + (span / (2.0 * geo.radius_m)).to_degrees() >= 90.0
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "crop-centred equirectangular output cannot cross a pole",
            ));
        }
        if projection == LocalProjection::Orthographic
            && span * std::f64::consts::FRAC_1_SQRT_2 >= geo.radius_m
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "orthographic crop extends beyond the tangent hemisphere",
            ));
        }
        Ok(Self {
            lat,
            lon,
            span,
            resolution: w as usize,
            spacing: span / if nodes { (w - 1) as f64 } else { w as f64 },
            shift: if nodes { 0.0 } else { 0.5 },
            projection,
        })
    }

    fn source_pixel(
        &self,
        geo: &SphericalRaster,
        col: usize,
        row: usize,
    ) -> std::io::Result<[f64; 2]> {
        let east = -self.span / 2.0 + (col as f64 + self.shift) * self.spacing;
        let north = self.span / 2.0 - (row as f64 + self.shift) * self.spacing;
        let [latitude, longitude] = match self.projection {
            LocalProjection::Equirectangular => {
                geo.geographic_offset(self.lat, self.lon, east, north)
            }
            LocalProjection::Orthographic => lunco_geotiff::orthographic_geographic_offset(
                geo.radius_m,
                self.lat,
                self.lon,
                east,
                north,
            )
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "invalid orthographic crop coordinate",
                )
            })?,
        };
        Ok(geo.pixel_at(latitude, longitude))
    }
}

/// Source-pixel bounds needed for a projected output, including the bilinear
/// neighbour at each sample. The output lattice, not just its corners, is
/// examined because a projection can curve the footprint.
pub(crate) fn source_window(
    geo: &SphericalRaster,
    width: u32,
    height: u32,
    config: &ProcessConfig,
    nodes: bool,
    control: &ProcessControl,
) -> std::io::Result<(u32, u32, u32, u32)> {
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid projected crop window",
        )
    };
    let grid = OutputGrid::new(geo, config, nodes)?;
    if width == 0 || height == 0 {
        return Err(invalid());
    }
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for row in 0..grid.resolution {
        control.check()?;
        for col in 0..grid.resolution {
            let [x, y] = grid.source_pixel(geo, col, row)?;
            if x.is_finite()
                && y.is_finite()
                && x >= 0.0
                && y >= 0.0
                && x <= (width - 1) as f64
                && y <= (height - 1) as f64
            {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }
    if !min_x.is_finite() {
        return Err(invalid());
    }
    let x = min_x.floor() as u32;
    let y = min_y.floor() as u32;
    let end_x = ((max_x.floor() as u32).saturating_add(2)).min(width);
    let end_y = ((max_y.floor() as u32).saturating_add(2)).min(height);
    Ok((x, y, end_x - x, end_y - y))
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
    let grid = OutputGrid::new(geo, config, nodes)?;
    if width == 0 || height == 0 || samples.len() != width.saturating_mul(height) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "projected raster dimensions do not match samples",
        ));
    }
    let n = grid.resolution;
    let mut out = Vec::with_capacity(n * n);
    for row in 0..n {
        control.check()?;
        for col in 0..n {
            let [x, y] = grid.source_pixel(geo, col, row)?;
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
        span_m: grid.span,
        spacing_m: grid.spacing,
        center_lat: grid.lat,
        center_lon: grid.lon,
        radius_m: geo.radius_m,
        projection: grid.projection,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_geotiff::RasterProjection;

    #[test]
    fn projected_window_matches_full_raster_including_clipped_coverage() {
        let geo = SphericalRaster {
            radius_m: 1_737_400.0,
            pixel_size_m: [1.0, 1.0],
            origin_m: [-100.0, 100.0],
            false_origin_m: [0.0, 0.0],
            projection: RasterProjection::Equirectangular {
                parallel_deg: 0.0,
                latitude_deg: 0.0,
                longitude_deg: 0.0,
            },
        };
        let mut source: Vec<f64> = (0..201 * 201).map(|v| v as f64).collect();
        source[100 * 201 + 100] = f64::NAN;
        let control = ProcessControl::unrestricted();
        for (longitude, nodes) in [(0.0, true), (0.003, true), (0.0, false), (0.003, false)] {
            let config: ProcessConfig = toml::from_str(&format!(
                "kind = 'dem'\noutput = 'site'\ncenter_lat = 0.0\ncenter_lon = {longitude}\nwindow_m = 40.0\ntarget_resolution = [11, 11]\n"
            )).unwrap();
            let full = sample(&geo, &source, 201, 201, &config, nodes, &control).unwrap();
            let (x, y, w, h) = source_window(&geo, 201, 201, &config, nodes, &control).unwrap();
            let mut window = Vec::new();
            for row in y..y + h {
                window.extend_from_slice(
                    &source[(row * 201 + x) as usize..(row * 201 + x + w) as usize],
                );
            }
            let mut shifted = geo;
            shifted.origin_m[0] += x as f64;
            shifted.origin_m[1] -= y as f64;
            let cropped = sample(
                &shifted, &window, w as usize, h as usize, &config, nodes, &control,
            )
            .unwrap();
            for (a, b) in full.samples.iter().zip(cropped.samples) {
                assert!(
                    (a.is_nan() && b.is_nan()) || (a - b).abs() < 1e-8,
                    "{a} != {b}"
                );
            }
        }
        let outside: ProcessConfig = toml::from_str(
            "kind = 'dem'\noutput = 'site'\ncenter_lat = 0.0\ncenter_lon = 1.0\nwindow_m = 40.0\ntarget_resolution = [11, 11]\n"
        ).unwrap();
        assert!(source_window(&geo, 201, 201, &outside, true, &control).is_err());
    }

    #[test]
    fn orthographic_crop_can_cross_pole_and_rejects_unknown_projection() {
        let geo = SphericalRaster {
            radius_m: 1_737_400.0,
            pixel_size_m: [20.0, 20.0],
            origin_m: [-100_000.0, 100_000.0],
            false_origin_m: [0.0, 0.0],
            projection: RasterProjection::PolarStereographic {
                north: false,
                longitude_deg: 0.0,
                scale: 1.0,
            },
        };
        let mut config: ProcessConfig = toml::from_str(
            "kind = 'dem'\noutput = 'site'\ncenter_lat = -89.9\ncenter_lon = 0.0\nwindow_m = 20000.0\ntarget_resolution = [101, 101]\n"
        ).unwrap();
        assert!(OutputGrid::new(&geo, &config, true).is_err());
        config
            .parameters
            .insert("output_projection".into(), "orthographic".into());
        let grid = OutputGrid::new(&geo, &config, true).unwrap();
        let [x, y] = grid.source_pixel(&geo, 50, 50).unwrap();
        assert!(x.is_finite() && y.is_finite());
        assert!(
            source_window(
                &geo,
                10_001,
                10_001,
                &config,
                true,
                &ProcessControl::unrestricted()
            )
            .is_ok()
        );
        config
            .parameters
            .insert("output_projection".into(), "unknown".into());
        assert!(OutputGrid::new(&geo, &config, true).is_err());
    }
}
