//! Spherical source projections shared by offline raster consumers.

use super::*;
use std::io::{Read, Seek};

/// Supported projected rasters. All parameters are read from standard GeoKeys.
#[derive(Debug, Clone, Copy)]
pub enum RasterProjection {
    /// Spherical polar stereographic, with an explicit polar origin and scale.
    PolarStereographic {
        /// True for a north-polar origin.
        north: bool,
        /// Pole longitude, degrees east.
        longitude_deg: f64,
        /// Scale at the pole.
        scale: f64,
    },
    /// Spherical equirectangular projection.
    Equirectangular {
        /// Standard parallel in degrees.
        parallel_deg: f64,
        /// Natural origin latitude, degrees.
        latitude_deg: f64,
        /// Natural origin longitude, degrees east.
        longitude_deg: f64,
    },
}

/// Source sample placement and spherical CRS, independent of raster sample type.
#[derive(Debug, Clone, Copy)]
pub struct SphericalRaster {
    /// Sphere radius in metres.
    pub radius_m: f64,
    /// Source pixel size, metres along each projected axis.
    pub pixel_size_m: [f64; 2],
    /// Projected coordinate of sample (0,0), after area/point normalization.
    pub origin_m: [f64; 2],
    /// False easting and northing, metres.
    pub false_origin_m: [f64; 2],
    /// Projection read from the GeoKey directory.
    pub projection: RasterProjection,
}

impl SphericalRaster {
    /// Project a geographic point into fractional source sample coordinates.
    pub fn pixel_at(&self, latitude_deg: f64, longitude_deg: f64) -> [f64; 2] {
        let latitude = latitude_deg.to_radians();
        let (x, y) = match self.projection {
            RasterProjection::PolarStereographic {
                north,
                longitude_deg: origin,
                scale,
            } => {
                let longitude = (longitude_deg - origin).to_radians();
                let sign = if north { 1.0 } else { -1.0 };
                let rho = 2.0
                    * self.radius_m
                    * scale
                    * (std::f64::consts::FRAC_PI_4 - sign * latitude / 2.0).tan();
                (rho * longitude.sin(), -sign * rho * longitude.cos())
            }
            RasterProjection::Equirectangular {
                parallel_deg,
                latitude_deg: origin_lat,
                longitude_deg: origin_lon,
            } => {
                let longitude = (longitude_deg - origin_lon + 180.0).rem_euclid(360.0) - 180.0;
                (
                    self.radius_m * longitude.to_radians() * parallel_deg.to_radians().cos(),
                    self.radius_m * (latitude_deg - origin_lat).to_radians(),
                )
            }
        };
        [
            (x + self.false_origin_m[0] - self.origin_m[0]) / self.pixel_size_m[0],
            (self.origin_m[1] - y - self.false_origin_m[1]) / self.pixel_size_m[1],
        ]
    }

    /// Inverse of the crop-centred equirectangular CRS written by `write_geo_tags`.
    pub fn geographic_offset(&self, lat_deg: f64, lon_deg: f64, east: f64, north: f64) -> [f64; 2] {
        [
            lat_deg + (north / self.radius_m).to_degrees(),
            (lon_deg + (east / (self.radius_m * lat_deg.to_radians().cos())).to_degrees())
                .rem_euclid(360.0),
        ]
    }
}

/// Read a spherical projected CRS. A plain TIFF has no CRS; incomplete,
/// unsupported, non-spherical, or non-metric authored georeferencing is an error.
pub fn read_spherical_raster<R: Read + Seek>(
    dec: &mut tiff::decoder::Decoder<R>,
) -> Result<Option<SphericalRaster>, GeoReadError> {
    if dec.get_tag_f64_vec(Tag::Unknown(34264)).is_ok() {
        return Err(GeoReadError::Malformed(
            "matrix raster transforms are unsupported; provide scale and tiepoint georeferencing",
        ));
    }
    let directory = dec.get_tag_u16_vec(Tag::Unknown(TAG_GEO_KEY_DIRECTORY));
    let has_scale = dec
        .get_tag_f64_vec(Tag::Unknown(TAG_MODEL_PIXEL_SCALE))
        .is_ok();
    if directory.is_err() && !has_scale {
        return Ok(None);
    }
    let directory = directory.map_err(|_| GeoReadError::Malformed("missing GeoKey directory"))?;
    let doubles = dec
        .get_tag_f64_vec(Tag::Unknown(TAG_GEO_DOUBLE_PARAMS))
        .unwrap_or_default();
    let ascii = dec
        .get_tag_ascii_string(Tag::Unknown(TAG_GEO_ASCII_PARAMS))
        .unwrap_or_default();
    let keys = GeoKeyDirectory::parse(&directory, &doubles, &ascii)
        .ok_or(GeoReadError::Malformed("invalid GeoKey directory"))?;
    let value = |id| {
        keys.get_double(id)
            .and_then(|v| v.first().copied())
            .filter(|v| v.is_finite())
    };
    let required = |id| {
        value(id).ok_or(GeoReadError::Malformed(
            "required finite spherical projection parameter is missing",
        ))
    };
    if keys.get_short(KEY_GT_MODEL_TYPE) != Some(MODEL_TYPE_PROJECTED)
        || keys.get_short(KEY_PROJ_LINEAR_UNITS) != Some(LINEAR_UNITS_METRE)
        || keys.get_short(KEY_GEOG_ANGULAR_UNITS) != Some(ANGULAR_UNITS_DEGREE)
    {
        return Err(GeoReadError::Malformed(
            "projection requires projected metres and geographic degrees",
        ));
    }
    let radius = required(KEY_GEOG_SEMI_MAJOR)?;
    let minor = required(KEY_GEOG_SEMI_MINOR)?;
    if radius <= 0.0 || (radius - minor).abs() > radius * 1e-12 {
        return Err(GeoReadError::Malformed(
            "raster projection requires a positive spherical datum",
        ));
    }
    let projection = match keys.get_short(KEY_PROJ_COORD_TRANS) {
        Some(15) => {
            let latitude = required(KEY_PROJ_NAT_ORIGIN_LAT)?;
            if latitude.abs() != 90.0 {
                return Err(GeoReadError::Malformed(
                    "polar stereographic requires a polar origin",
                ));
            }
            let scale = required(3092)?;
            if scale <= 0.0 {
                return Err(GeoReadError::Malformed("polar scale must be positive"));
            }
            RasterProjection::PolarStereographic {
                north: latitude > 0.0,
                longitude_deg: required(3095)?,
                scale,
            }
        }
        Some(COORD_TRANS_EQUIRECTANGULAR) => {
            let parallel_deg = required(KEY_PROJ_STD_PARALLEL1)?;
            let latitude_deg = required(KEY_PROJ_CENTER_LAT)?;
            if parallel_deg.abs() >= 90.0 || latitude_deg.abs() > 90.0 {
                return Err(GeoReadError::Malformed(
                    "invalid equirectangular standard parallel or origin",
                ));
            }
            RasterProjection::Equirectangular {
                parallel_deg,
                latitude_deg,
                longitude_deg: required(KEY_PROJ_CENTER_LONG)?,
            }
        }
        _ => {
            return Err(GeoReadError::Malformed(
                "unsupported spherical raster projection",
            ));
        }
    };
    let scale = dec
        .get_tag_f64_vec(Tag::Unknown(TAG_MODEL_PIXEL_SCALE))
        .map_err(|_| GeoReadError::NoPixelScale)?;
    let tie = dec
        .get_tag_f64_vec(Tag::Unknown(TAG_MODEL_TIEPOINT))
        .map_err(|_| GeoReadError::NoTiepoint)?;
    if scale.len() < 2
        || tie.len() < 6
        || !scale.iter().all(|v| v.is_finite())
        || !tie.iter().all(|v| v.is_finite())
        || scale[0] <= 0.0
        || scale[1] <= 0.0
    {
        return Err(GeoReadError::Malformed(
            "invalid projected pixel scale or tiepoint",
        ));
    }
    let shift = match keys
        .get_short(KEY_GT_RASTER_TYPE)
        .unwrap_or(RASTER_TYPE_PIXEL_IS_AREA)
    {
        RASTER_TYPE_PIXEL_IS_AREA => 0.5,
        RASTER_TYPE_PIXEL_IS_POINT => 0.0,
        _ => return Err(GeoReadError::Malformed("unsupported raster registration")),
    };
    Ok(Some(SphericalRaster {
        radius_m: radius,
        pixel_size_m: [scale[0], scale[1]],
        origin_m: [
            tie[3] - tie[0] * scale[0] + shift * scale[0],
            tie[4] + tie[1] * scale[1] - shift * scale[1],
        ],
        false_origin_m: [value(3082).unwrap_or(0.0), value(3083).unwrap_or(0.0)],
        projection,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projected_reader_roundtrips_nodes_and_rejects_matrix_transforms() {
        for matrix in [false, true] {
            let mut bytes = std::io::Cursor::new(Vec::new());
            {
                let mut encoder = tiff::encoder::TiffEncoder::new(&mut bytes).unwrap();
                let mut image = encoder
                    .new_image::<tiff::encoder::colortype::Gray64Float>(2, 2)
                    .unwrap();
                write_geo_tags(
                    image.encoder(),
                    &GeoTransform::centred_square(4.0, 2, 3_000_000.0, 10.0, 20.0),
                    "Test sphere",
                )
                .unwrap();
                if matrix {
                    image
                        .encoder()
                        .write_tag(Tag::Unknown(34264), &[1.0f64; 16][..])
                        .unwrap();
                }
                image.write_data(&[1.0f64; 4]).unwrap();
            }
            bytes.set_position(0);
            let mut decoder = tiff::decoder::Decoder::new(bytes).unwrap();
            if matrix {
                assert!(
                    read_spherical_raster(&mut decoder)
                        .unwrap_err()
                        .to_string()
                        .contains("matrix")
                );
            } else {
                assert_eq!(
                    read_spherical_raster(&mut decoder)
                        .unwrap()
                        .unwrap()
                        .pixel_at(10.0, 20.0),
                    [0.5, 0.5]
                );
            }
        }
    }
    #[test]
    fn polar_hemispheres_longitudes_and_offsets_are_general() {
        for north in [false, true] {
            let raster = SphericalRaster {
                radius_m: 3_000_000.0,
                pixel_size_m: [2.0, 3.0],
                origin_m: [0.0, 0.0],
                false_origin_m: [10.0, 20.0],
                projection: RasterProjection::PolarStereographic {
                    north,
                    longitude_deg: 70.0,
                    scale: 0.9,
                },
            };
            let pole = if north { 90.0 } else { -90.0 };
            assert_eq!(raster.pixel_at(pole, 70.0), [5.0, -20.0 / 3.0]);
            let [lat, lon] =
                raster.geographic_offset(if north { 85.0 } else { -85.0 }, 70.0, 20.0, 30.0);
            assert!(lat.is_finite() && lon.is_finite());
            let [x, y] = raster.pixel_at(lat, lon);
            assert!(x.is_finite() && y.is_finite());
        }
    }
}
