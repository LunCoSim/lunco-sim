//! PDS3 `.IMG` raster decoding — the non-GeoTIFF half of the DEM/map ingest.
//!
//! LROC RDR products ship two raster containers: float32 GeoTIFF (`.TIF`, no
//! geo tags) and PDS3 `.IMG` (orthophotos, confidence maps, and most non-LROC
//! DEMs — SLDEM2015, Kaguya TC, Chang'e). A PDS3 raster is a fixed-length
//! record file whose first `LABEL_RECORDS × RECORD_BYTES` bytes are a plain-
//! text label describing the pixel layout; the label may instead live in a
//! detached `.LBL` sibling (SLDEM-style). This module parses the label
//! (attached or detached), decodes the sample grid to `f64`, and surfaces the
//! label's own equirectangular extent + map scale so a manifest that ingests
//! a PDS product does not have to restate what the product already declares.
//!
//! Scope: single-band, band-sequential rasters — every DTM/ortho the terrain
//! pipeline ingests. Multi-band sources decode band 0 (LROC ships no
//! multi-band DTM products; a future colour ortho would extend
//! [`PdsImage::decode`] rather than grow a new path).

use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};

/// Geographic extent a PDS3 `IMAGE_MAP_PROJECTION` object declares.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdsExtent {
    /// Minimum latitude in degrees.
    pub min_lat: f64,
    /// Maximum latitude in degrees.
    pub max_lat: f64,
    /// `WESTERNMOST_LONGITUDE`, degrees East as authored (labels use 0–360).
    pub west_lon: f64,
    /// `EASTERNMOST_LONGITUDE`, degrees East as authored.
    pub east_lon: f64,
}

/// A decoded PDS3 raster: the sample grid plus the projection facts the
/// label itself declares (used as fallbacks for absent manifest fields).
#[derive(Debug, Clone)]
pub struct PdsImage {
    /// Number of decoded samples per row.
    pub width: usize,
    /// Number of decoded rows.
    pub height: usize,
    /// Row-major samples, `SCALING_FACTOR`/`OFFSET` applied, missing
    /// constants mapped to `NaN`.
    pub samples: Vec<f64>,
    /// Projection extent declared by the source label, when present.
    pub extent: Option<PdsExtent>,
    /// `MAP_SCALE` in metres/pixel, when the label declares one.
    pub map_scale_m: Option<f64>,
    /// `MAP_PROJECTION_TYPE` verbatim (e.g. `EQUIRECTANGULAR`,
    /// `POLARSTEREOGRAPHIC`) — callers that only handle equirectangular
    /// sources use this to fail loudly instead of mis-projecting.
    pub projection: Option<String>,
}

/// One `KEY = VALUE` label line, with OBJECT nesting tracked just enough to
/// scope `LINES`/`SAMPLE_*` to the `IMAGE` object (labels also carry LINES
/// counts in compression/history objects on some products).
#[derive(Debug, Default, Clone)]
struct Label {
    record_bytes: Option<usize>,
    label_records: Option<usize>,
    /// `^IMAGE` record pointer (1-based records) or explicit byte offset.
    image_record: Option<usize>,
    image_byte_offset: Option<usize>,
    lines: Option<usize>,
    line_samples: Option<usize>,
    bands: Option<usize>,
    sample_bits: Option<usize>,
    sample_type: Option<String>,
    scaling_factor: Option<f64>,
    offset: Option<f64>,
    missing: Vec<f64>,
    /// Missing constants the label wrote in PDS radix form (`16#FF7FFFFB#`),
    /// held as raw bits until `SAMPLE_TYPE`/`SAMPLE_BITS` are known. A label may
    /// declare `CORE_NULL` before either of them, so these cannot be resolved
    /// inside the parse loop without depending on key order.
    missing_radix: Vec<u64>,
    extent: Option<PdsExtent>,
    map_scale_m: Option<f64>,
    map_resolution: Option<f64>,
    has_map_resolution: bool,
    projection: Option<String>,
    map_rotation: Option<f64>,
    has_map_rotation: bool,
    map_radius_m: Option<f64>,
    map_b_radius_m: Option<f64>,
    map_c_radius_m: Option<f64>,
    longitude_direction: Option<String>,
    map_center_lat: Option<f64>,
    map_center_lon: Option<f64>,
    line_projection_offset: Option<f64>,
    sample_projection_offset: Option<f64>,
}

fn strip_units(v: &str) -> &str {
    // `2.0 <METERS/PIXEL>` → `2.0`; `"EQUIRECTANGULAR"` → unquoted below.
    match v.find('<') {
        Some(i) => v[..i].trim(),
        None => v.trim(),
    }
}

fn parse_f64(v: &str) -> Option<f64> {
    // Decimal only. PDS radix values (`16#FF7FFFFB#`) are a bit pattern, not a
    // decimal number, so they are rejected here and handled by
    // [`parse_pds_radix`] — see its note for why they must NOT be dropped.
    strip_units(v).trim_matches('"').parse::<f64>().ok()
}

fn parse_metres(v: &str) -> Option<f64> {
    let value = parse_f64(v)?;
    let unit = match v.split_once('<') {
        Some((_, tail)) => Some(tail.split_once('>')?.0.trim().to_ascii_uppercase()),
        None if v.contains('>') => return None,
        None => None,
    };
    match unit.as_deref() {
        Some("KM") | Some("KM/PIX") | Some("KM/PIXEL") | Some("KILOMETERS/PIXEL") => {
            Some(value * 1000.0)
        }
        Some("M") | Some("M/PIX") | Some("M/PIXEL") | Some("METERS/PIXEL") | None => Some(value),
        _ => None,
    }
}

/// Parse PDS integer-radix notation — `base#digits#`, e.g. `16#FF7FFFFB#`.
///
/// **This is how real LROC products spell their null value**, and dropping it is
/// why an entire class of nodata went undetected. `CORE_NULL = 16#FF7FFFFB#` on
/// a `PC_REAL`/32-bit band is the IEEE-754 bit pattern of the ISIS special value
/// (≈ -3.4028233e38): precisely decodable, given the sample layout. Treating it
/// as "no decodable meaning" left the sentinel in the samples as a *finite*
/// float, where every downstream `is_finite()` test reads it as a real
/// measurement — the exact failure `lunco_geotiff::nodata_to_nan` exists to
/// prevent, reintroduced one decoder further up the chain.
///
/// Returns the raw bits; [`resolve_radix_missing`] turns them into the value a
/// sample would compare equal to, which needs `SAMPLE_TYPE`/`SAMPLE_BITS`.
fn parse_pds_radix(v: &str) -> Option<u64> {
    let s = strip_units(v).trim_matches('"').trim();
    let (base, rest) = s.split_once('#')?;
    let digits = rest.strip_suffix('#')?;
    let base: u32 = base.trim().parse().ok()?;
    if !(2..=36).contains(&base) {
        return None;
    }
    u64::from_str_radix(digits.trim(), base).ok()
}

/// Reinterpret a radix-declared missing constant as the f64 a decoded sample
/// carries, per the band's declared layout. Real bands hold a float bit pattern;
/// integer bands hold the integer itself.
fn resolve_radix_missing(bits: u64, sample_type: Option<&str>, sample_bits: Option<usize>) -> f64 {
    let is_real = sample_type.map(|t| t.contains("REAL")).unwrap_or(false);
    match (is_real, sample_bits.unwrap_or(32)) {
        (true, 32) => f32::from_bits(bits as u32) as f64,
        (true, 64) => f64::from_bits(bits),
        // Integer band: the radix IS the value.
        (_, _) => bits as f64,
    }
}

fn parse_usize(v: &str) -> Option<usize> {
    strip_units(v).trim_matches('"').parse::<usize>().ok()
}

/// Parse the text of a PDS3 label. Tolerant: unknown keys are skipped,
/// multi-line values (quoted continuations) contribute only their first line
/// (none of the keys read here are multi-line).
fn parse_label(text: &str) -> Label {
    let mut lbl = Label::default();
    // OBJECT scoping: image-geometry keys are only trusted inside
    // `OBJECT = IMAGE` / `OBJECT = IMAGE_MAP_PROJECTION`.
    let mut objects: Vec<String> = Vec::new();
    let mut min_lat = None;
    let mut max_lat = None;
    let mut west_lon = None;
    let mut east_lon = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line == "END" {
            break;
        }
        let Some(eq) = line.find('=') else { continue };
        let key = line[..eq].trim();
        let val = line[eq + 1..].trim();

        match key {
            "OBJECT" => objects.push(strip_units(val).trim_matches('"').to_uppercase()),
            "END_OBJECT" => {
                objects.pop();
            }
            "RECORD_BYTES" => lbl.record_bytes = parse_usize(val),
            "LABEL_RECORDS" => lbl.label_records = parse_usize(val),
            "^IMAGE" => {
                // Forms: `2` (record) · `2 <BYTES>` (byte offset) ·
                // `("FILE.IMG", 2)` (detached label, record in that file).
                let v = val.trim();
                if v.starts_with('(') {
                    let inner = v.trim_matches(|c| c == '(' || c == ')');
                    if let Some(num) = inner.rsplit(',').next() {
                        lbl.image_record = parse_usize(num);
                    }
                } else if v.to_uppercase().contains("<BYTES>") {
                    lbl.image_byte_offset = parse_usize(v);
                } else {
                    lbl.image_record = parse_usize(v);
                }
            }
            _ => {}
        }

        let in_image = objects.iter().any(|o| o == "IMAGE");
        let in_proj = objects.iter().any(|o| o == "IMAGE_MAP_PROJECTION");
        if in_image && !in_proj {
            match key {
                "LINES" => lbl.lines = parse_usize(val),
                "LINE_SAMPLES" => lbl.line_samples = parse_usize(val),
                "BANDS" => lbl.bands = parse_usize(val),
                "SAMPLE_BITS" => lbl.sample_bits = parse_usize(val),
                "SAMPLE_TYPE" => {
                    lbl.sample_type = Some(strip_units(val).trim_matches('"').to_uppercase())
                }
                "SCALING_FACTOR" => lbl.scaling_factor = parse_f64(val),
                "OFFSET" => lbl.offset = parse_f64(val),
                "MISSING_CONSTANT"
                | "NULL"
                | "CORE_NULL"
                | "CORE_LOW_REPR_SATURATION"
                | "CORE_LOW_INSTR_SATURATION"
                | "CORE_HIGH_REPR_SATURATION"
                | "CORE_HIGH_INSTR_SATURATION" => {
                    // Decimal (`-3.4028227e38`) resolves now; radix
                    // (`16#FF7FFFFB#`) is deferred until the sample layout is
                    // known, since the label may declare it before SAMPLE_TYPE.
                    if let Some(m) = parse_f64(val) {
                        lbl.missing.push(m);
                    } else if let Some(bits) = parse_pds_radix(val) {
                        lbl.missing_radix.push(bits);
                    }
                }
                _ => {}
            }
        }
        if in_proj {
            match key {
                "MAP_PROJECTION_TYPE" => {
                    lbl.projection = Some(strip_units(val).trim_matches('"').to_uppercase())
                }
                "MAP_PROJECTION_ROTATION" => {
                    lbl.has_map_rotation = true;
                    lbl.map_rotation = parse_f64(val);
                }
                "MAP_SCALE" => lbl.map_scale_m = parse_metres(val),
                "MAP_RESOLUTION" => {
                    lbl.has_map_resolution = true;
                    lbl.map_resolution = parse_f64(val);
                }
                "A_AXIS_RADIUS" => lbl.map_radius_m = parse_metres(val),
                "B_AXIS_RADIUS" => lbl.map_b_radius_m = parse_metres(val),
                "C_AXIS_RADIUS" => lbl.map_c_radius_m = parse_metres(val),
                "POSITIVE_LONGITUDE_DIRECTION" => {
                    lbl.longitude_direction =
                        Some(strip_units(val).trim_matches('"').to_uppercase())
                }
                "CENTER_LATITUDE" => lbl.map_center_lat = parse_f64(val),
                "CENTER_LONGITUDE" => lbl.map_center_lon = parse_f64(val),
                "LINE_PROJECTION_OFFSET" => lbl.line_projection_offset = parse_f64(val),
                "SAMPLE_PROJECTION_OFFSET" => lbl.sample_projection_offset = parse_f64(val),
                "MINIMUM_LATITUDE" => min_lat = parse_f64(val),
                "MAXIMUM_LATITUDE" => max_lat = parse_f64(val),
                "WESTERNMOST_LONGITUDE" => west_lon = parse_f64(val),
                "EASTERNMOST_LONGITUDE" => east_lon = parse_f64(val),
                _ => {}
            }
        }
    }

    // Resolve deferred radix missing-constants now that SAMPLE_TYPE/SAMPLE_BITS
    // are known, regardless of the order the label declared them in.
    let (stype, sbits) = (lbl.sample_type.clone(), lbl.sample_bits);
    for bits in std::mem::take(&mut lbl.missing_radix) {
        lbl.missing
            .push(resolve_radix_missing(bits, stype.as_deref(), sbits));
    }

    if let (Some(a), Some(b), Some(w), Some(e)) = (min_lat, max_lat, west_lon, east_lon) {
        lbl.extent = Some(PdsExtent {
            min_lat: a,
            max_lat: b,
            west_lon: w,
            east_lon: e,
        });
    }
    lbl
}

fn io_err(msg: String) -> std::io::Error {
    std::io::Error::other(msg)
}

fn read_label(path: &Path, file: &mut std::fs::File) -> std::io::Result<(Label, Option<usize>)> {
    let mut prefix = Vec::new();
    file.take(64 * 1024).read_to_end(&mut prefix)?;
    let head = String::from_utf8_lossy(&prefix);
    if head.trim_start().starts_with("PDS_VERSION_ID") {
        return Ok((parse_label(&head), None));
    }
    let label_path = lunco_assets_datasets::pds_detached_label(path)?;
    Ok((parse_label(&std::fs::read_to_string(label_path)?), Some(0)))
}

/// A source lattice with explicit spherical projection and pixel registration.
pub(crate) struct ProjectedPds {
    pub width: u32,
    pub height: u32,
    pub geo: lunco_geotiff::SphericalRaster,
}

/// Read projection facts without loading the raster. PDS projection offsets
/// locate the projection origin relative to the first sample centre.
pub(crate) fn projected_metadata(path: &Path) -> std::io::Result<Option<ProjectedPds>> {
    let mut file = std::fs::File::open(path)?;
    let (label, _) = read_label(path, &mut file)?;
    if !matches!(
        label.projection.as_deref(),
        Some("SIMPLE CYLINDRICAL" | "EQUIRECTANGULAR" | "POLAR STEREOGRAPHIC")
    ) {
        return Ok(None);
    }
    let invalid = || {
        io_err(format!(
            "{}: incomplete or invalid PDS raster projection",
            path.display()
        ))
    };
    let width = u32::try_from(label.line_samples.ok_or_else(invalid)?).map_err(|_| invalid())?;
    let height = u32::try_from(label.lines.ok_or_else(invalid)?).map_err(|_| invalid())?;
    let radius = label.map_radius_m.ok_or_else(invalid)?;
    let b_radius = label.map_b_radius_m.ok_or_else(invalid)?;
    let c_radius = label.map_c_radius_m.ok_or_else(invalid)?;
    let scale = label.map_scale_m.ok_or_else(invalid)?;
    let lat = label.map_center_lat.ok_or_else(invalid)?;
    let lon = label.map_center_lon.ok_or_else(invalid)?;
    let line = label.line_projection_offset.ok_or_else(invalid)?;
    let sample = label.sample_projection_offset.ok_or_else(invalid)?;
    if width == 0
        || height == 0
        || !radius.is_finite()
        || radius <= 0.0
        || b_radius != radius
        || c_radius != radius
        || label.longitude_direction.as_deref() != Some("EAST")
        || !scale.is_finite()
        || scale <= 0.0
        || !lat.is_finite()
        || lat.abs() > 90.0
        || !lon.is_finite()
        || !line.is_finite()
        || !sample.is_finite()
        || (label.has_map_rotation && label.map_rotation != Some(0.0))
    {
        return Err(invalid());
    }
    if label.has_map_resolution {
        let ppd = label.map_resolution.ok_or_else(invalid)?;
        if !ppd.is_finite() || ppd <= 0.0 {
            return Err(invalid());
        }
        let expected = radius * std::f64::consts::PI / (180.0 * ppd);
        if ((scale - expected) / expected).abs() > 1e-5 {
            return Err(invalid());
        }
    }
    let projection = match label.projection.as_deref() {
        Some("SIMPLE CYLINDRICAL") => lunco_geotiff::RasterProjection::Equirectangular {
            parallel_deg: 0.0,
            latitude_deg: lat,
            longitude_deg: lon,
        },
        Some("EQUIRECTANGULAR") if lat.abs() < 90.0 => {
            lunco_geotiff::RasterProjection::Equirectangular {
                parallel_deg: lat,
                latitude_deg: 0.0,
                longitude_deg: lon,
            }
        }
        Some("POLAR STEREOGRAPHIC") if lat.abs() == 90.0 => {
            lunco_geotiff::RasterProjection::PolarStereographic {
                north: lat > 0.0,
                longitude_deg: lon,
                scale: 1.0,
            }
        }
        _ => return Err(invalid()),
    };
    Ok(Some(ProjectedPds {
        width,
        height,
        geo: lunco_geotiff::SphericalRaster {
            radius_m: radius,
            pixel_size_m: [scale, scale],
            origin_m: [-sample * scale, line * scale],
            false_origin_m: [0.0, 0.0],
            projection,
        },
    }))
}

impl PdsImage {
    /// Decode a PDS3 raster from `path` (`.IMG`). The label is taken from the
    /// file head when attached, else from a sibling `.LBL`/`.lbl` (detached).
    pub fn decode(path: &Path) -> Result<PdsImage, std::io::Error> {
        Self::decode_region(path, None, None)
    }

    /// Decode a bounded band-zero window in source sample coordinates.
    pub(crate) fn decode_region(
        path: &Path,
        region: Option<(u32, u32, u32, u32)>,
        control: Option<&crate::process::ProcessControl>,
    ) -> Result<PdsImage, std::io::Error> {
        let mut file = std::fs::File::open(path)?;
        let (label, data_offset_hint) = read_label(path, &mut file)?;

        let lines = label
            .lines
            .ok_or_else(|| io_err("PDS label missing IMAGE LINES".into()))?;
        let line_samples = label
            .line_samples
            .ok_or_else(|| io_err("PDS label missing IMAGE LINE_SAMPLES".into()))?;
        let bands = label.bands.unwrap_or(1);
        let bits = label.sample_bits.unwrap_or(32);
        let stype = label
            .sample_type
            .clone()
            .unwrap_or_else(|| "PC_REAL".into());

        // Where the pixels start.
        let data_offset = if let Some(off) = label.image_byte_offset {
            off
        } else if let Some(rec) = label.image_record {
            let rb = label.record_bytes.unwrap_or(line_samples * bits / 8);
            (rec.saturating_sub(1)) * rb
        } else if let Some(hint) = data_offset_hint {
            hint
        } else {
            let rb = label.record_bytes.ok_or_else(|| {
                io_err("PDS label has no ^IMAGE pointer and no RECORD_BYTES".into())
            })?;
            label.label_records.unwrap_or(1) * rb
        };

        let bytes_per = bits / 8;
        // Label-controlled geometry — checked arithmetic so a hostile or
        // corrupt label can't overflow `usize` and wrap past the bounds
        // check below.
        let geometry_overflow = || {
            io_err(format!(
                "PDS label geometry overflows: {lines} lines × {line_samples} samples × {} bands × {bits} bits at offset {data_offset}",
                bands.max(1)
            ))
        };
        let total = lines
            .checked_mul(line_samples)
            .ok_or_else(geometry_overflow)?;
        let need = total
            .checked_mul(bands.max(1))
            .and_then(|v| v.checked_mul(bytes_per))
            .and_then(|v| v.checked_add(data_offset))
            .ok_or_else(geometry_overflow)?;
        let file_len = file.metadata()?.len();
        if file_len < need as u64 {
            return Err(io_err(format!(
                "{}: file too short for label geometry ({} < {} bytes; {}×{}×{}b at offset {})",
                path.display(),
                file_len,
                need,
                line_samples,
                lines,
                bits,
                data_offset
            )));
        }

        let (x, y, width, height) = match region {
            None => (0, 0, line_samples, lines),
            Some((x, y, w, h)) => (x as usize, y as usize, w as usize, h as usize),
        };
        if width == 0
            || height == 0
            || x.checked_add(width).is_none_or(|end| end > line_samples)
            || y.checked_add(height).is_none_or(|end| end > lines)
        {
            return Err(io_err("PDS sample window lies outside the raster".into()));
        }
        let n = width.checked_mul(height).ok_or_else(geometry_overflow)?;
        let row_bytes = width.checked_mul(bytes_per).ok_or_else(geometry_overflow)?;
        let mut data = vec![0; n.checked_mul(bytes_per).ok_or_else(geometry_overflow)?];
        for row in 0..height {
            if let Some(control) = control {
                control.check()?;
            }
            let offset = (y + row)
                .checked_mul(line_samples)
                .and_then(|v| v.checked_add(x))
                .and_then(|v| v.checked_mul(bytes_per))
                .and_then(|v| v.checked_add(data_offset))
                .ok_or_else(geometry_overflow)?;
            file.seek(SeekFrom::Start(offset as u64))?;
            file.read_exact(&mut data[row * row_bytes..(row + 1) * row_bytes])?;
        }
        let le = stype.contains("PC_") || stype.contains("LSB");
        let unsigned = stype.contains("UNSIGNED");
        let real = stype.contains("REAL");
        let scale = label.scaling_factor.unwrap_or(1.0);
        let offs = label.offset.unwrap_or(0.0);

        let mut samples = Vec::with_capacity(n);
        for i in 0..n {
            let b = &data[i * bytes_per..(i + 1) * bytes_per];
            let raw: f64 = match (real, bits, le) {
                (true, 32, true) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
                (true, 32, false) => f32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64,
                (true, 64, true) => f64::from_le_bytes(b.try_into().unwrap()),
                (true, 64, false) => f64::from_be_bytes(b.try_into().unwrap()),
                (false, 8, _) => {
                    if unsigned {
                        b[0] as f64
                    } else {
                        b[0] as i8 as f64
                    }
                }
                (false, 16, true) => {
                    if unsigned {
                        u16::from_le_bytes([b[0], b[1]]) as f64
                    } else {
                        i16::from_le_bytes([b[0], b[1]]) as f64
                    }
                }
                (false, 16, false) => {
                    if unsigned {
                        u16::from_be_bytes([b[0], b[1]]) as f64
                    } else {
                        i16::from_be_bytes([b[0], b[1]]) as f64
                    }
                }
                (false, 32, true) => {
                    if unsigned {
                        u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64
                    } else {
                        i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64
                    }
                }
                (false, 32, false) => {
                    if unsigned {
                        u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64
                    } else {
                        i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as f64
                    }
                }
                _ => {
                    return Err(io_err(format!(
                        "unsupported PDS sample layout: {stype} / {bits} bits"
                    )));
                }
            };
            // Relative tolerance: a label's decimal missing constant and the
            // f32 bit pattern in the file round-trip differently through f64
            // (LROC's -3.4028227e38 sentinel differs in the 8th digit).
            let missing = label
                .missing
                .iter()
                .any(|m| raw == *m || (raw - m).abs() <= m.abs() * 1e-6);
            samples.push(if missing {
                f64::NAN
            } else {
                raw * scale + offs
            });
        }

        Ok(PdsImage {
            width,
            height,
            samples,
            extent: label.extent,
            map_scale_m: label.map_scale_m,
            projection: label.projection,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polar_pds_preserves_sample_registration_and_rejects_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("polar.IMG");
        let samples = [0.13_f32, f32::from_bits(0xff7f_fffc), 0.15, 0.16];
        let bytes: Vec<u8> = samples.into_iter().flat_map(f32::to_le_bytes).collect();
        lunco_storage::write_file_sync(&path, &bytes).unwrap();
        let label = r#"PDS_VERSION_ID = PDS3
OBJECT = IMAGE
 LINES = 2
 LINE_SAMPLES = 2
 SAMPLE_TYPE = PC_REAL
 SAMPLE_BITS = 32
 CORE_LOW_REPR_SATURATION = 16#FF7FFFFC#
END_OBJECT = IMAGE
OBJECT = IMAGE_MAP_PROJECTION
 MAP_PROJECTION_TYPE = "POLAR STEREOGRAPHIC"
 MAP_PROJECTION_ROTATION = 0.0
 MAP_SCALE = 100 <METERS/PIXEL>
 A_AXIS_RADIUS = 1737.4 <KM>
 B_AXIS_RADIUS = 1737.4 <KM>
 C_AXIS_RADIUS = 1737.4 <KM>
 POSITIVE_LONGITUDE_DIRECTION = EAST
 CENTER_LATITUDE = -90
 CENTER_LONGITUDE = 0
 LINE_PROJECTION_OFFSET = 0.5
 SAMPLE_PROJECTION_OFFSET = 0.5
END_OBJECT = IMAGE_MAP_PROJECTION
END
"#;
        let label_path = dir.path().join("polar.LBL");
        lunco_storage::write_file_sync(&label_path, label.as_bytes()).unwrap();
        let raster = projected_metadata(&path).unwrap().unwrap().geo;
        assert_eq!(raster.pixel_at(-90.0, 0.0), [0.5, 0.5]);
        assert!(raster.pixel_at(-89.999, 0.0)[1] < 0.5);
        let decoded = PdsImage::decode(&path).unwrap();
        assert!(decoded.samples[1].is_nan());
        assert!((decoded.samples[0] - 0.13).abs() < 1e-7);
        lunco_storage::write_file_sync(
            &label_path,
            label.replace("ROTATION = 0.0", "ROTATION = 1.0").as_bytes(),
        )
        .unwrap();
        assert!(projected_metadata(&path).is_err());
    }

    #[test]
    fn projected_pds_reads_registered_window_and_rejects_invalid_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("regional.IMG");
        let pixels: Vec<u8> = (0..12).flat_map(|i| (i as f32).to_le_bytes()).collect();
        lunco_storage::write_file_sync(&path, &pixels).unwrap();
        let label = r#"PDS_VERSION_ID = PDS3
OBJECT = IMAGE
 LINES = 3
 LINE_SAMPLES = 4
 SAMPLE_BITS = 32
 SAMPLE_TYPE = PC_REAL
 SCALING_FACTOR = 2
 OFFSET = 10
END_OBJECT = IMAGE
OBJECT = IMAGE_MAP_PROJECTION
 MAP_PROJECTION_TYPE = "SIMPLE CYLINDRICAL"
 MAP_SCALE = 0.0592252938 <km/pix>
 A_AXIS_RADIUS = 1737.4 <km>
 B_AXIS_RADIUS = 1737.4 <km>
 C_AXIS_RADIUS = 1737.4 <km>
 POSITIVE_LONGITUDE_DIRECTION = "EAST"
 CENTER_LATITUDE = 0
 CENTER_LONGITUDE = 180
 LINE_PROJECTION_OFFSET = 1.0 <pix>
 SAMPLE_PROJECTION_OFFSET = 1.5 <pix>
END_OBJECT = IMAGE_MAP_PROJECTION
END
"#;
        let label_path = dir.path().join("regional.LBL");
        lunco_storage::write_file_sync(&label_path, label.as_bytes()).unwrap();
        let metadata = projected_metadata(&path).unwrap().unwrap();
        assert_eq!((metadata.width, metadata.height), (4, 3));
        assert_eq!(metadata.geo.radius_m, 1_737_400.0);
        assert!((metadata.geo.pixel_size_m[0] - 59.2252938).abs() < 1e-9);
        let center = metadata.geo.pixel_at(0.0, 180.0);
        assert!((center[0] - 1.5).abs() < 1e-12 && (center[1] - 1.0).abs() < 1e-12);
        let window = PdsImage::decode_region(&path, Some((1, 1, 2, 2)), None).unwrap();
        assert_eq!((window.width, window.height), (2, 2));
        assert_eq!(window.samples, vec![20.0, 22.0, 28.0, 30.0]);
        assert!(PdsImage::decode_region(&path, Some((3, 2, 2, 1)), None).is_err());
        lunco_storage::write_file_sync(
            &label_path,
            label
                .replace(" A_AXIS_RADIUS = 1737.4 <km>\n", "")
                .as_bytes(),
        )
        .unwrap();
        assert!(projected_metadata(&path).is_err());
        lunco_storage::write_file_sync(
            &label_path,
            label
                .replace(
                    " B_AXIS_RADIUS = 1737.4 <km>",
                    " B_AXIS_RADIUS = 1736.0 <km>",
                )
                .as_bytes(),
        )
        .unwrap();
        assert!(projected_metadata(&path).is_err());
        lunco_storage::write_file_sync(
            &label_path,
            label
                .replace(
                    " POSITIVE_LONGITUDE_DIRECTION = \"EAST\"",
                    " POSITIVE_LONGITUDE_DIRECTION = \"WEST\"",
                )
                .as_bytes(),
        )
        .unwrap();
        assert!(projected_metadata(&path).is_err());
        lunco_storage::write_file_sync(
            &label_path,
            label
                .replace("0.0592252938 <km/pix>", "0.0592252938 <unknown>")
                .as_bytes(),
        )
        .unwrap();
        assert!(projected_metadata(&path).is_err());
        lunco_storage::write_file_sync(
            &label_path,
            label
                .replace(
                    "\nOBJECT = IMAGE_MAP_PROJECTION",
                    "\nOBJECT = IMAGE_MAP_PROJECTION\n MAP_RESOLUTION = invalid",
                )
                .as_bytes(),
        )
        .unwrap();
        assert!(projected_metadata(&path).is_err());
        lunco_storage::write_file_sync(
            &label_path,
            label
                .replace("0.0592252938 <km/pix>", "0.0592252938 <km/pix")
                .as_bytes(),
        )
        .unwrap();
        assert!(projected_metadata(&path).is_err());
    }

    #[test]
    fn equirectangular_pds_uses_center_latitude_as_standard_parallel() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("site.IMG");
        lunco_storage::write_file_sync(&path, &[0]).unwrap();
        let label = r#"PDS_VERSION_ID = PDS3
OBJECT = IMAGE
 LINES = 10673
 LINE_SAMPLES = 2984
 SAMPLE_BITS = 32
 SAMPLE_TYPE = PC_REAL
END_OBJECT = IMAGE
OBJECT = IMAGE_MAP_PROJECTION
 MAP_PROJECTION_TYPE = EQUIRECTANGULAR
 MAP_SCALE = 5 <METERS/PIXEL>
 A_AXIS_RADIUS = 1737.4 <KM>
 B_AXIS_RADIUS = 1737.4 <KM>
 C_AXIS_RADIUS = 1737.4 <KM>
 POSITIVE_LONGITUDE_DIRECTION = EAST
 CENTER_LATITUDE = 26
 CENTER_LONGITUDE = 180
 LINE_PROJECTION_OFFSET = 162962.5
 SAMPLE_PROJECTION_OFFSET = 963210.5
END_OBJECT = IMAGE_MAP_PROJECTION
END
"#;
        lunco_storage::write_file_sync(&dir.path().join("site.LBL"), label.as_bytes()).unwrap();
        let raster = projected_metadata(&path).unwrap().unwrap().geo;
        let upper_left = raster.pixel_at(26.87079391, 3.29288009);
        // The published geographic bound need not be a sample center.
        assert!(upper_left[0].abs() < 0.51, "{upper_left:?}");
        assert!(upper_left[1].abs() < 0.01, "{upper_left:?}");
        let sample = raster.pixel_at(26.0, 179.0);
        let degrees = std::f64::consts::PI / 180.0;
        let pixels_per_degree = 1_737_400.0 * degrees / 5.0;
        assert!(
            (sample[0] - (963_210.5 - pixels_per_degree * (26.0_f64 * degrees).cos())).abs() < 1e-8
        );
        assert!((sample[1] - (162_962.5 - 26.0 * pixels_per_degree)).abs() < 1e-8);
    }

    fn write_attached_img(
        dir: &Path,
        name: &str,
        label: &str,
        pixels: &[u8],
    ) -> std::path::PathBuf {
        // Attached label padded to one record.
        let record_bytes: usize = 512;
        let mut file = label.as_bytes().to_vec();
        assert!(file.len() <= record_bytes, "test label fits one record");
        file.resize(record_bytes, b' ');
        file.extend_from_slice(pixels);
        let p = dir.join(name);
        lunco_storage::write_file_sync(&p, &file).unwrap();
        p
    }

    /// The spelling REAL LROC products use. `NAC_DTM_APOLLO15_E261N0036.IMG`
    /// declares `CORE_NULL = 16#FF7FFFFB#` — a radix bit pattern, not a decimal
    /// — and the parser used to drop it as "no decodable meaning". The sentinel
    /// then stayed in the samples as a FINITE float, so `is_finite()` read it as
    /// a measurement all the way down. Downstream, `process_map`'s 1–99
    /// percentile stretch took it as the 1st percentile, which drove `lo` to
    /// ≈ -3.4e38 and mapped every real sample to 1.0: the shipped `ortho.png`
    /// came out 81% pure white / 18% pure black, and the terrain shader
    /// multiplied that mask straight into the albedo as black holes.
    ///
    /// The decimal case is covered above; it passing is precisely why this went
    /// unnoticed, so both spellings must be tested.
    #[test]
    fn radix_core_null_is_missing() {
        let dir = tempfile::tempdir().expect("temporary PDS directory");

        // 0xFF7FFFFB as an IEEE-754 f32 — the ISIS null the real product ships.
        let null = f32::from_bits(0xFF7F_FFFB);
        assert!(
            null.is_finite(),
            "the whole problem: the sentinel is finite"
        );
        let vals: [f32; 4] = [-1900.0, null, -1895.5, -1892.25];
        let mut px = Vec::new();
        for v in vals {
            px.extend_from_slice(&v.to_le_bytes());
        }
        // CORE_NULL is declared BEFORE SAMPLE_TYPE/SAMPLE_BITS here on purpose:
        // resolution must not depend on label key order.
        let label = "PDS_VERSION_ID = PDS3\r\n\
                     RECORD_BYTES  = 512\r\n\
                     LABEL_RECORDS = 1\r\n\
                     ^IMAGE        = 2\r\n\
                     OBJECT = IMAGE\r\n\
                       LINES        = 2\r\n\
                       LINE_SAMPLES = 2\r\n\
                       CORE_NULL    = 16#FF7FFFFB#\r\n\
                       SAMPLE_TYPE  = PC_REAL\r\n\
                       SAMPLE_BITS  = 32\r\n\
                     END_OBJECT = IMAGE\r\n\
                     END\r\n";
        let p = write_attached_img(dir.path(), "radix.IMG", label, &px);

        let img = PdsImage::decode(&p).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert!(
            img.samples[1].is_nan(),
            "radix CORE_NULL must decode to NaN, got {}",
            img.samples[1]
        );
        // Real measurements must survive untouched.
        assert_eq!(img.samples[0], -1900.0);
        assert_eq!(img.samples[3], -1892.25);
    }

    #[test]
    fn radix_notation_parses_across_bases() {
        assert_eq!(parse_pds_radix("16#FF7FFFFB#"), Some(0xFF7F_FFFB));
        assert_eq!(parse_pds_radix("2#1011#"), Some(0b1011));
        assert_eq!(parse_pds_radix("10#42#"), Some(42));
        // Not radix notation → None, so the decimal path keeps handling it.
        assert_eq!(parse_pds_radix("-3.4028227e38"), None);
        assert_eq!(parse_pds_radix("42"), None);
        // A real band reinterprets the bits; an integer band takes the value.
        assert_eq!(
            resolve_radix_missing(0xFF7F_FFFB, Some("PC_REAL"), Some(32)) as f32,
            f32::from_bits(0xFF7F_FFFB)
        );
        assert_eq!(
            resolve_radix_missing(255, Some("LSB_INTEGER"), Some(16)),
            255.0
        );
    }

    #[test]
    fn attached_pc_real_with_extent_decodes() {
        let dir = tempfile::tempdir().expect("temporary PDS directory");

        // 3×2 little-endian float32 grid, one value flagged missing.
        let vals: [f32; 6] = [1.0, 2.0, 3.0, 4.0, -3.4028227e38, 6.0];
        let mut px = Vec::new();
        for v in vals {
            px.extend_from_slice(&v.to_le_bytes());
        }
        let label = "PDS_VERSION_ID = PDS3\r\n\
                     RECORD_BYTES  = 512\r\n\
                     LABEL_RECORDS = 1\r\n\
                     ^IMAGE        = 2\r\n\
                     OBJECT = IMAGE\r\n\
                       LINES        = 2\r\n\
                       LINE_SAMPLES = 3\r\n\
                       SAMPLE_TYPE  = PC_REAL\r\n\
                       SAMPLE_BITS  = 32\r\n\
                       MISSING_CONSTANT = -3.4028227e38\r\n\
                     END_OBJECT = IMAGE\r\n\
                     OBJECT = IMAGE_MAP_PROJECTION\r\n\
                       MAP_PROJECTION_TYPE = \"EQUIRECTANGULAR\"\r\n\
                       MAP_SCALE = 2.0 <METERS/PIXEL>\r\n\
                       MAXIMUM_LATITUDE = 8.5 <DEG>\r\n\
                       MINIMUM_LATITUDE = 7.5 <DEG>\r\n\
                       EASTERNMOST_LONGITUDE = 33.3 <DEG>\r\n\
                       WESTERNMOST_LONGITUDE = 33.1 <DEG>\r\n\
                     END_OBJECT = IMAGE_MAP_PROJECTION\r\n\
                     END\r\n";
        let p = write_attached_img(dir.path(), "t.IMG", label, &px);

        let img = PdsImage::decode(&p).unwrap();
        assert_eq!((img.width, img.height), (3, 2));
        assert_eq!(img.samples[0], 1.0);
        assert!(img.samples[4].is_nan(), "missing constant → NaN");
        assert_eq!(img.map_scale_m, Some(2.0));
        assert_eq!(img.projection.as_deref(), Some("EQUIRECTANGULAR"));
        let e = img.extent.unwrap();
        assert_eq!((e.min_lat, e.max_lat), (7.5, 8.5));
        assert_eq!((e.west_lon, e.east_lon), (33.1, 33.3));
    }

    #[test]
    fn detached_label_and_integer_samples_decode() {
        let dir = tempfile::tempdir().expect("temporary PDS directory");

        // Raw 2×2 LSB signed 16-bit data file, no attached label.
        let vals: [i16; 4] = [-100, 0, 100, 2000];
        let mut px = Vec::new();
        for v in vals {
            px.extend_from_slice(&v.to_le_bytes());
        }
        let img_path = dir.path().join("d.IMG");
        lunco_storage::write_file_sync(&img_path, &px).unwrap();
        // SLDEM-style detached label: scaling turns DN into metres.
        let label = "PDS_VERSION_ID = PDS3\r\n\
                     ^IMAGE = (\"d.IMG\", 1)\r\n\
                     RECORD_BYTES = 4\r\n\
                     OBJECT = IMAGE\r\n\
                       LINES = 2\r\n\
                       LINE_SAMPLES = 2\r\n\
                       SAMPLE_TYPE = LSB_INTEGER\r\n\
                       SAMPLE_BITS = 16\r\n\
                       SCALING_FACTOR = 0.5\r\n\
                       OFFSET = 10.0\r\n\
                     END_OBJECT = IMAGE\r\n\
                     END\r\n";
        lunco_storage::write_file_sync(&dir.path().join("d.LBL"), label.as_bytes()).unwrap();

        let img = PdsImage::decode(&img_path).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.samples, vec![-40.0, 10.0, 60.0, 1010.0]);
    }
}
