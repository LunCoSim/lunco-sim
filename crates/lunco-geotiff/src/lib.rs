//! The **geo** half of a GeoTIFF.
//!
//! We have shipped TIFFs for a long time and called them GeoTIFFs. They were not:
//! the encoder wrote pixels only, and the decoder's own doc said *"Geo tags are
//! ignored — only the raster matters; spacing/extent come from `DemMetadata`."* So
//! every raster left the pipeline stripped of where it is, and a sidecar
//! `metadata.yaml` carried that fact alongside it — a second source of truth that
//! could, and did, drift.
//!
//! This crate is the missing half, in one place, shared by the writer
//! (`lunco-assets`) and the reader (`lunco-terrain-bake`). See
//! `docs/architecture/57-dem-georeferencing.md`.
//!
//! ## What we write, and why it is the honest description
//!
//! A **local metric frame centred on the crop**, not a planetary projection.
//!
//! That is a deliberate choice, not a shortcut. The sim works in scene metres with
//! the crop centre at the origin; a true lunar equirectangular would add a
//! conversion at every boundary, and every conversion is a chance to flip a sign.
//! The frame we declare is the frame the data is actually in, which is the only
//! description that cannot be wrong.
//!
//! ## Pixel-is-point, not pixel-is-area
//!
//! Our sampling is **node-based**: sample 0 sits exactly on `-half_extent` and
//! sample `res-1` exactly on `+half_extent`, spread corner to corner
//! (`HeightGrid`, `cell = 2*half_extent / (res - 1)`). That is
//! `RasterPixelIsPoint`. Declaring the GDAL-default `RasterPixelIsArea` would
//! offset every sample by half a pixel — 0.98 m on the Apollo 15 crop, which is
//! half a rover width and precisely the scale at which a slope baseline argument
//! is settled.

use std::io::{Read, Seek, Write};

use geotiff_core::geokeys::{GeoKeyDirectory, GeoKeyValue};
use geotiff_core::tags::{
    TAG_GEO_ASCII_PARAMS, TAG_GEO_DOUBLE_PARAMS, TAG_GEO_KEY_DIRECTORY, TAG_MODEL_PIXEL_SCALE,
    TAG_MODEL_TIEPOINT,
};
use tiff::encoder::{DirectoryEncoder, TiffKind};
use tiff::tags::Tag;

// GeoKey ids (OGC 01-004, Annex F). `geotiff-core` names the ones it models; the
// projection-parameter keys below are not in its constant set, so they are spelled
// out here against the spec.
use geotiff_core::geokeys::{
    GEOG_ANGULAR_UNITS as KEY_GEOG_ANGULAR_UNITS, GEOGRAPHIC_TYPE as KEY_GEOG_TYPE,
    GEOG_CITATION as KEY_GEOG_CITATION, GT_MODEL_TYPE as KEY_GT_MODEL_TYPE,
    GT_RASTER_TYPE as KEY_GT_RASTER_TYPE, PROJECTED_CS_TYPE as KEY_PROJECTED_CS_TYPE,
    PROJECTION as KEY_PROJECTION, PROJ_COORD_TRANS as KEY_PROJ_COORD_TRANS,
    PROJ_LINEAR_UNITS as KEY_PROJ_LINEAR_UNITS,
};
const KEY_GEOG_SEMI_MAJOR: u16 = 2057;
const KEY_GEOG_SEMI_MINOR: u16 = 2058;
const KEY_PROJ_STD_PARALLEL1: u16 = 3078;
const KEY_PROJ_NAT_ORIGIN_LONG: u16 = 3088;

const MODEL_TYPE_PROJECTED: u16 = 1;
/// Sample positions ARE the grid nodes — see the module note.
const RASTER_TYPE_PIXEL_IS_POINT: u16 = 2;
const USER_DEFINED: u16 = 32767;
const LINEAR_UNITS_METRE: u16 = 9001;
const ANGULAR_UNITS_DEGREE: u16 = 9102;
/// `CT_Equirectangular`.
const COORD_TRANS_EQUIRECTANGULAR: u16 = 17;

/// Where a raster sits, in the scene's own metric frame.
///
/// Deliberately *not* a full CRS: this describes a local metre grid centred on the
/// crop, which is what our data actually is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoTransform {
    /// Ground metres per pixel, both axes (square pixels only — a non-square DEM
    /// is rejected upstream).
    pub pixel_size_m: f64,
    /// Model X of the upper-left sample (west edge, since +X is east).
    pub origin_x_m: f64,
    /// Model Y of the upper-left sample (north edge; +Y is north, so this is
    /// positive and Y decreases with row — the sign flip that makes row 0 north).
    pub origin_y_m: f64,
    /// Body radius the frame is on, metres. Carried so a consumer can tell a
    /// lunar raster from a terrestrial one.
    pub body_radius_m: f64,
    /// Geodetic latitude of the crop centre, degrees — the projection's standard
    /// parallel.
    ///
    /// This is why the file declares an equirectangular projection rather than a
    /// bare local grid: near its own origin, projected metres ARE local metres, so
    /// the frame is identical — but the projection parameters carry *where on the
    /// body* the crop sits, which a local grid cannot express. That fact used to
    /// live only in `metadata.yaml`'s `coordinates:` block, and it is the last
    /// thing that kept the sidecar alive.
    pub center_lat_deg: f64,
    /// Geodetic longitude of the crop centre, degrees — the projection's natural
    /// origin longitude (central meridian).
    pub center_lon_deg: f64,
}

impl GeoTransform {
    /// The transform for a square crop of `size_m` across `res` samples, centred
    /// on the given geodetic point — the layout the DEM processor produces.
    pub fn centred_square(
        size_m: f64,
        res: usize,
        body_radius_m: f64,
        center_lat_deg: f64,
        center_lon_deg: f64,
    ) -> Self {
        let half = size_m * 0.5;
        // Node-based spacing: res-1 intervals span the full extent, matching
        // `HeightGrid`. Using `res` here would shrink the grid by one pixel.
        let pixel_size_m = if res > 1 {
            size_m / (res as f64 - 1.0)
        } else {
            size_m
        };
        Self {
            pixel_size_m,
            origin_x_m: -half,
            origin_y_m: half,
            body_radius_m,
            center_lat_deg,
            center_lon_deg,
        }
    }

    /// Full ground span, metres — `(res - 1) * pixel_size`, node-based.
    pub fn extent_m(&self, res: usize) -> f64 {
        if res > 1 {
            self.pixel_size_m * (res as f64 - 1.0)
        } else {
            self.pixel_size_m
        }
    }
}

/// Write the georeferencing tags into an open TIFF directory.
///
/// Call before finishing the image. The `citation` names the body for a human
/// reading the file in QGIS or `gdalinfo` — it carries no semantics.
pub fn write_geo_tags<W, K>(
    dir: &mut DirectoryEncoder<'_, W, K>,
    tf: &GeoTransform,
    citation: &str,
) -> tiff::TiffResult<()>
where
    W: Write + Seek,
    K: TiffKind,
{
    dir.write_tag(
        Tag::Unknown(TAG_MODEL_PIXEL_SCALE),
        &[tf.pixel_size_m, tf.pixel_size_m, 0.0][..],
    )?;
    // Raster (0,0) maps to the model's upper-left corner.
    dir.write_tag(
        Tag::Unknown(TAG_MODEL_TIEPOINT),
        &[0.0, 0.0, 0.0, tf.origin_x_m, tf.origin_y_m, 0.0][..],
    )?;

    // A user-defined equirectangular projection on a sphere, with its natural
    // origin AT the crop centre. That is what makes projected metres equal local
    // metres near the origin, so the sim's frame and the file's frame are the same
    // frame — no conversion, and no sign to get wrong.
    //
    // `geotiff-core` owns the encoding: key ordering, the value-offset indices into
    // the doubles array, and the ASCII terminators. Hand-rolling that is how you
    // produce a file that opens fine and georeferences wrongly.
    let mut keys = GeoKeyDirectory::new();
    keys.set(KEY_GT_MODEL_TYPE, GeoKeyValue::Short(MODEL_TYPE_PROJECTED));
    keys.set(
        KEY_GT_RASTER_TYPE,
        GeoKeyValue::Short(RASTER_TYPE_PIXEL_IS_POINT),
    );
    keys.set(KEY_GEOG_TYPE, GeoKeyValue::Short(USER_DEFINED));
    keys.set(KEY_GEOG_CITATION, GeoKeyValue::Ascii(citation.to_string()));
    keys.set(
        KEY_GEOG_ANGULAR_UNITS,
        GeoKeyValue::Short(ANGULAR_UNITS_DEGREE),
    );
    keys.set(
        KEY_GEOG_SEMI_MAJOR,
        GeoKeyValue::Double(vec![tf.body_radius_m]),
    );
    keys.set(
        KEY_GEOG_SEMI_MINOR,
        GeoKeyValue::Double(vec![tf.body_radius_m]),
    );
    keys.set(KEY_PROJECTED_CS_TYPE, GeoKeyValue::Short(USER_DEFINED));
    keys.set(KEY_PROJECTION, GeoKeyValue::Short(USER_DEFINED));
    keys.set(
        KEY_PROJ_COORD_TRANS,
        GeoKeyValue::Short(COORD_TRANS_EQUIRECTANGULAR),
    );
    keys.set(KEY_PROJ_LINEAR_UNITS, GeoKeyValue::Short(LINEAR_UNITS_METRE));
    keys.set(
        KEY_PROJ_STD_PARALLEL1,
        GeoKeyValue::Double(vec![tf.center_lat_deg]),
    );
    keys.set(
        KEY_PROJ_NAT_ORIGIN_LONG,
        GeoKeyValue::Double(vec![tf.center_lon_deg]),
    );

    // `CompressedDataCorrupt` is the only `TiffFormatError` variant carrying a free
    // message; the geo-key encoder's failures are ours, not the codec's, so the
    // message is what matters here rather than the variant.
    let (directory, doubles, ascii) = keys.serialize().map_err(|e| {
        tiff::TiffError::FormatError(tiff::TiffFormatError::CompressedDataCorrupt(format!(
            "geo key directory: {e}"
        )))
    })?;
    if !doubles.is_empty() {
        dir.write_tag(Tag::Unknown(TAG_GEO_DOUBLE_PARAMS), &doubles[..])?;
    }
    if !ascii.is_empty() {
        dir.write_tag(Tag::Unknown(TAG_GEO_ASCII_PARAMS), ascii.as_str())?;
    }
    dir.write_tag(Tag::Unknown(TAG_GEO_KEY_DIRECTORY), &directory[..])?;
    Ok(())
}

/// Why a raster's georeferencing could not be read.
///
/// Every variant names what is missing, because the caller's job is to tell a
/// human how to fix the file — not to fail quietly.
#[derive(Debug, Clone, PartialEq)]
pub enum GeoReadError {
    /// No `ModelPixelScaleTag`. The file is a plain TIFF: pixels, no position.
    NoPixelScale,
    /// No `ModelTiepointTag`.
    NoTiepoint,
    /// A tag was present but malformed (wrong arity).
    Malformed(&'static str),
}

impl std::fmt::Display for GeoReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoPixelScale => write!(
                f,
                "no ModelPixelScaleTag — this is a plain TIFF, not a GeoTIFF; \
                 re-run `cargo run -p lunco-assets -- process` to write georeferencing"
            ),
            Self::NoTiepoint => write!(f, "no ModelTiepointTag — pixel scale without an origin"),
            Self::Malformed(what) => write!(f, "malformed georeferencing tag: {what}"),
        }
    }
}

/// Read the georeferencing tags from a decoded TIFF.
///
/// Returns `Err` describing what is missing rather than a bare `None`: a raster
/// without tags must produce a message a human can act on.
pub fn read_geo_tags<R: Read + Seek>(
    dec: &mut tiff::decoder::Decoder<R>,
) -> Result<GeoTransform, GeoReadError> {
    let scale = dec
        .get_tag_f64_vec(Tag::Unknown(TAG_MODEL_PIXEL_SCALE))
        .map_err(|_| GeoReadError::NoPixelScale)?;
    if scale.len() < 2 {
        return Err(GeoReadError::Malformed("ModelPixelScale needs >= 2 values"));
    }
    let tie = dec
        .get_tag_f64_vec(Tag::Unknown(TAG_MODEL_TIEPOINT))
        .map_err(|_| GeoReadError::NoTiepoint)?;
    if tie.len() < 6 {
        return Err(GeoReadError::Malformed("ModelTiepoint needs 6 values"));
    }

    // Resolve keys through the directory, never positionally. A third-party
    // GeoTIFF orders its doubles however it likes, so reading `doubles[2]` by
    // index would happily return a false-easting as a latitude.
    let doubles = dec
        .get_tag_f64_vec(Tag::Unknown(TAG_GEO_DOUBLE_PARAMS))
        .unwrap_or_default();
    let ascii = dec
        .get_tag_ascii_string(Tag::Unknown(TAG_GEO_ASCII_PARAMS))
        .unwrap_or_default();
    let directory = dec
        .get_tag_u16_vec(Tag::Unknown(TAG_GEO_KEY_DIRECTORY))
        .unwrap_or_default();
    let keys = GeoKeyDirectory::parse(&directory, &doubles, &ascii);
    let double_key = |id: u16| -> Option<f64> {
        keys.as_ref()
            .and_then(|k| k.get_double(id))
            .and_then(|v| v.first().copied())
    };

    Ok(GeoTransform {
        pixel_size_m: scale[0],
        origin_x_m: tie[3],
        origin_y_m: tie[4],
        // Absent is not an error: a third-party raster may carry a full EPSG code
        // instead of user-defined axes, and the radius is informational for us.
        body_radius_m: double_key(KEY_GEOG_SEMI_MAJOR).unwrap_or(0.0),
        center_lat_deg: double_key(KEY_PROJ_STD_PARALLEL1).unwrap_or(0.0),
        center_lon_deg: double_key(KEY_PROJ_NAT_ORIGIN_LONG).unwrap_or(0.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Node-based spacing: the extent must span corner to corner, so `res - 1`
    /// intervals cover it. The Apollo 15 crop is the live case — 1002 m over 512
    /// samples — and getting this wrong scales the whole terrain by 512/511.
    #[test]
    fn centred_square_is_node_based() {
        let tf = GeoTransform::centred_square(1002.0, 512, 1737.0e3, 26.0371, 3.6584);
        assert!((tf.pixel_size_m - 1002.0 / 511.0).abs() < 1e-9);
        assert!((tf.origin_x_m + 501.0).abs() < 1e-9);
        assert!((tf.origin_y_m - 501.0).abs() < 1e-9);
        assert!((tf.extent_m(512) - 1002.0).abs() < 1e-9);
    }

    /// A single-sample raster has no interval to divide by; it must not produce
    /// an infinite pixel size.
    #[test]
    fn degenerate_single_sample_raster_does_not_divide_by_zero() {
        let tf = GeoTransform::centred_square(10.0, 1, 1737.0e3, 0.0, 0.0);
        assert!(tf.pixel_size_m.is_finite());
    }

    /// The error must name the fix, because it is the only thing a human reading
    /// a log has to go on.
    #[test]
    fn missing_scale_error_names_the_remedy() {
        let msg = GeoReadError::NoPixelScale.to_string();
        assert!(msg.contains("plain TIFF"), "{msg}");
        assert!(msg.contains("lunco-assets"), "{msg}");
    }

    /// ROUND TRIP THROUGH THE REAL CODEC.
    ///
    /// The point of this crate is that what we write, we can read. Asserting on
    /// the key array in isolation would prove only that I can build a Vec — it
    /// would not catch a wrong tag type, a bad ASCII terminator, or a directory
    /// whose key count disagrees with its entries, all of which produce a file
    /// that opens fine and georeferences wrongly.
    #[test]
    fn tags_written_are_tags_read() {
        use std::io::Cursor;
        use tiff::encoder::{colortype, TiffEncoder};

        let res = 512usize;
        let want = GeoTransform::centred_square(1002.0, res, 1737.0e3, 26.0371, 3.6584);

        let mut buf = Vec::new();
        {
            let mut enc = TiffEncoder::new(Cursor::new(&mut buf)).unwrap();
            let mut img = enc
                .new_image::<colortype::Gray32Float>(res as u32, res as u32)
                .unwrap();
            write_geo_tags(img.encoder(), &want, "Moon 2000").unwrap();
            img.write_data(&vec![0f32; res * res]).unwrap();
        }

        let mut dec = tiff::decoder::Decoder::new(Cursor::new(&buf)).unwrap();
        let got = read_geo_tags(&mut dec).expect("tags must read back");

        assert!(
            (got.pixel_size_m - want.pixel_size_m).abs() < 1e-9,
            "pixel size: {got:?}"
        );
        assert!((got.origin_x_m - want.origin_x_m).abs() < 1e-9, "origin x: {got:?}");
        assert!((got.origin_y_m - want.origin_y_m).abs() < 1e-9, "origin y: {got:?}");
        assert!(
            (got.body_radius_m - want.body_radius_m).abs() < 1e-6,
            "body radius: {got:?}"
        );
        // The extent must survive the trip — this is the number `metadata.yaml`
        // used to carry, and the whole reason the sidecar can go away.
        assert!((got.extent_m(res) - 1002.0).abs() < 1e-6, "extent: {got:?}");
    }

    /// A plain TIFF — what we shipped until 2026-07-19 — must fail with the
    /// actionable error, not silently read as a zero-origin frame.
    #[test]
    fn plain_tiff_reports_missing_georeferencing() {
        use std::io::Cursor;
        use tiff::encoder::{colortype, TiffEncoder};

        let mut buf = Vec::new();
        {
            let mut enc = TiffEncoder::new(Cursor::new(&mut buf)).unwrap();
            enc.write_image::<colortype::Gray32Float>(4, 4, &[0f32; 16])
                .unwrap();
        }
        let mut dec = tiff::decoder::Decoder::new(Cursor::new(&buf)).unwrap();
        assert_eq!(read_geo_tags(&mut dec), Err(GeoReadError::NoPixelScale));
    }
}
