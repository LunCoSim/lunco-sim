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

use std::path::Path;

/// Geographic extent a PDS3 `IMAGE_MAP_PROJECTION` object declares.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdsExtent {
    pub min_lat: f64,
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
    pub width: usize,
    pub height: usize,
    /// Row-major samples, `SCALING_FACTOR`/`OFFSET` applied, missing
    /// constants mapped to `NaN`.
    pub samples: Vec<f64>,
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
    extent: Option<PdsExtent>,
    map_scale_m: Option<f64>,
    projection: Option<String>,
}

fn strip_units(v: &str) -> &str {
    // `2.0 <METERS/PIXEL>` → `2.0`; `"EQUIRECTANGULAR"` → unquoted below.
    match v.find('<') {
        Some(i) => v[..i].trim(),
        None => v.trim(),
    }
}

fn parse_f64(v: &str) -> Option<f64> {
    // PDS radices like `16#FF7FFFFB#` (raw-bit missing constants) carry no
    // decodable geographic meaning here — reject, callers treat as absent.
    strip_units(v).trim_matches('"').parse::<f64>().ok()
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
                    lbl.sample_type =
                        Some(strip_units(val).trim_matches('"').to_uppercase())
                }
                "SCALING_FACTOR" => lbl.scaling_factor = parse_f64(val),
                "OFFSET" => lbl.offset = parse_f64(val),
                "MISSING_CONSTANT" | "NULL" | "CORE_NULL" => {
                    if let Some(m) = parse_f64(val) {
                        lbl.missing.push(m);
                    }
                }
                _ => {}
            }
        }
        if in_proj {
            match key {
                "MAP_PROJECTION_TYPE" => {
                    lbl.projection =
                        Some(strip_units(val).trim_matches('"').to_uppercase())
                }
                "MAP_SCALE" => lbl.map_scale_m = parse_f64(val),
                "MINIMUM_LATITUDE" => min_lat = parse_f64(val),
                "MAXIMUM_LATITUDE" => max_lat = parse_f64(val),
                "WESTERNMOST_LONGITUDE" => west_lon = parse_f64(val),
                "EASTERNMOST_LONGITUDE" => east_lon = parse_f64(val),
                _ => {}
            }
        }
    }

    if let (Some(a), Some(b), Some(w), Some(e)) = (min_lat, max_lat, west_lon, east_lon) {
        lbl.extent = Some(PdsExtent { min_lat: a, max_lat: b, west_lon: w, east_lon: e });
    }
    lbl
}

fn io_err(msg: String) -> std::io::Error {
    std::io::Error::other(msg)
}

impl PdsImage {
    /// Decode a PDS3 raster from `path` (`.IMG`). The label is taken from the
    /// file head when attached, else from a sibling `.LBL`/`.lbl` (detached).
    pub fn decode(path: &Path) -> Result<PdsImage, std::io::Error> {
        let bytes = std::fs::read(path)?;

        // Attached label? The head of a labelled file is printable PDS text
        // starting with `PDS_VERSION_ID`. Probe generously — labels are small.
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(64 * 1024)]).into_owned();
        let (label, data_offset_hint) = if head.trim_start().starts_with("PDS_VERSION_ID") {
            (parse_label(&head), None)
        } else {
            // Detached label: same stem, .LBL / .lbl.
            let sibling = ["LBL", "lbl"].iter().find_map(|ext| {
                let p = path.with_extension(ext);
                p.is_file().then_some(p)
            });
            let Some(lbl_path) = sibling else {
                return Err(io_err(format!(
                    "{}: no attached PDS3 label and no detached .LBL sibling",
                    path.display()
                )));
            };
            let text = std::fs::read_to_string(&lbl_path)?;
            // Detached labels address the data file from record 1 unless the
            // `^IMAGE` pointer says otherwise.
            (parse_label(&text), Some(0usize))
        };

        let lines = label
            .lines
            .ok_or_else(|| io_err("PDS label missing IMAGE LINES".into()))?;
        let line_samples = label
            .line_samples
            .ok_or_else(|| io_err("PDS label missing IMAGE LINE_SAMPLES".into()))?;
        let bands = label.bands.unwrap_or(1);
        let bits = label.sample_bits.unwrap_or(32);
        let stype = label.sample_type.clone().unwrap_or_else(|| "PC_REAL".into());

        // Where the pixels start.
        let data_offset = if let Some(off) = label.image_byte_offset {
            off
        } else if let Some(rec) = label.image_record {
            let rb = label.record_bytes.unwrap_or(line_samples * bits / 8);
            (rec.saturating_sub(1)) * rb
        } else if let Some(hint) = data_offset_hint {
            hint
        } else {
            let rb = label
                .record_bytes
                .ok_or_else(|| io_err("PDS label has no ^IMAGE pointer and no RECORD_BYTES".into()))?;
            label.label_records.unwrap_or(1) * rb
        };

        let bytes_per = bits / 8;
        let n = lines * line_samples;
        let need = data_offset + n * bands.max(1) * bytes_per;
        if bytes.len() < need {
            return Err(io_err(format!(
                "{}: file too short for label geometry ({} < {} bytes; {}×{}×{}b at offset {})",
                path.display(),
                bytes.len(),
                need,
                line_samples,
                lines,
                bits,
                data_offset
            )));
        }

        // Band-sequential band 0.
        let data = &bytes[data_offset..data_offset + n * bytes_per];
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
                    if unsigned { b[0] as f64 } else { b[0] as i8 as f64 }
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
                    )))
                }
            };
            // Relative tolerance: a label's decimal missing constant and the
            // f32 bit pattern in the file round-trip differently through f64
            // (LROC's -3.4028227e38 sentinel differs in the 8th digit).
            let missing = label
                .missing
                .iter()
                .any(|m| raw == *m || (raw - m).abs() <= m.abs() * 1e-6);
            samples.push(if missing { f64::NAN } else { raw * scale + offs });
        }

        Ok(PdsImage {
            width: line_samples,
            height: lines,
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

    fn write_attached_img(dir: &Path, name: &str, label: &str, pixels: &[u8]) -> std::path::PathBuf {
        // Attached label padded to one record.
        let record_bytes: usize = 512;
        let mut file = label.as_bytes().to_vec();
        assert!(file.len() <= record_bytes, "test label fits one record");
        file.resize(record_bytes, b' ');
        file.extend_from_slice(pixels);
        let p = dir.join(name);
        std::fs::write(&p, file).unwrap();
        p
    }

    #[test]
    fn attached_pc_real_with_extent_decodes() {
        let dir = std::env::temp_dir().join(format!("lunco-pds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

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
        let p = write_attached_img(&dir, "t.IMG", label, &px);

        let img = PdsImage::decode(&p).unwrap();
        assert_eq!((img.width, img.height), (3, 2));
        assert_eq!(img.samples[0], 1.0);
        assert!(img.samples[4].is_nan(), "missing constant → NaN");
        assert_eq!(img.map_scale_m, Some(2.0));
        assert_eq!(img.projection.as_deref(), Some("EQUIRECTANGULAR"));
        let e = img.extent.unwrap();
        assert_eq!((e.min_lat, e.max_lat), (7.5, 8.5));
        assert_eq!((e.west_lon, e.east_lon), (33.1, 33.3));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detached_label_and_integer_samples_decode() {
        let dir = std::env::temp_dir().join(format!("lunco-pds-det-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Raw 2×2 LSB signed 16-bit data file, no attached label.
        let vals: [i16; 4] = [-100, 0, 100, 2000];
        let mut px = Vec::new();
        for v in vals {
            px.extend_from_slice(&v.to_le_bytes());
        }
        let img_path = dir.join("d.IMG");
        std::fs::write(&img_path, &px).unwrap();
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
        std::fs::write(dir.join("d.LBL"), label).unwrap();

        let img = PdsImage::decode(&img_path).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.samples, vec![-40.0, 10.0, 60.0, 1010.0]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
