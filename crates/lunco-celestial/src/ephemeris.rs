use bevy::prelude::*;
use bevy::math::DVec3;
use celestial_time::TDB;
use celestial_time::julian::JulianDate;
use celestial_ephemeris::{Vsop2013Earth, Vsop2013Sun, planets::Vsop2013Emb, moon::ElpMpp02Moon};
use celestial_core::Vector3;

use std::sync::Arc;
use std::fs::File;
use std::io::{BufRead, BufReader};
use serde::Deserialize;

pub trait EphemerisProvider: Send + Sync + 'static {
    /// Position of body relative to its parent, in ecliptic J2000, AU.
    fn position(&self, body_id: i32, epoch_jd: f64) -> DVec3;
    /// Absolute Heliocentric position of body, in ecliptic J2000, AU.
    fn global_position(&self, body_id: i32, epoch_jd: f64) -> DVec3 {
        let mut pos = self.position(body_id, epoch_jd);
        let mut current_id = body_id;
        
        for _ in 0..10 { // Max depth
            let parent_id = match current_id {
                399 => 3,     // Earth -> EMB
                301 => 3,     // Moon -> EMB
                3 => 10,      // EMB -> Sun
                -1024 => 399, // Artemis 2 -> Earth
                10 => break,  // Sun has no parent
                _ => break,
            };
            if parent_id == 10 {
                break;
            }
            pos += self.position(parent_id, epoch_jd);
            current_id = parent_id;
        }
        pos
    }
}

#[derive(Resource)]
pub struct EphemerisResource {
    pub provider: Arc<dyn EphemerisProvider>,
}

#[derive(Clone)]
struct CsvDataPoint {
    jd: f64,
    pos_au: DVec3,
}

#[derive(Debug, Deserialize)]
struct MissionConfig {
    ephemeris_sources: Option<Vec<EphemerisSourceConfig>>,
}

#[derive(Debug, Deserialize)]
struct EphemerisSourceConfig {
    target_id: i32,
    command: String,
    center: String,
    ref_plane: String,
    start_time: String,
    stop_time: String,
    step_size: String,
}

pub struct CelestialEphemerisProvider {
    _sun: Vsop2013Sun,
    earth: Vsop2013Earth,
    emb: Vsop2013Emb,
    moon: ElpMpp02Moon,
    custom_data: std::collections::HashMap<i32, Vec<CsvDataPoint>>,
}

impl CelestialEphemerisProvider {
    pub fn new() -> Self {
        let mut custom_data = std::collections::HashMap::new();
        let missions_dir = "assets/missions";
        
        if let Ok(entries) = std::fs::read_dir(missions_dir) {
            for entry in entries.flatten() {
                if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        if let Ok(config) = serde_json::from_str::<MissionConfig>(&content) {
                            if let Some(sources) = config.ephemeris_sources {
                                for src in sources {
                                    // Generate cache path dynamically based on target and time range
                                    let safe_start = src.start_time.replace(" ", "_").replace(":", "");
                                    let safe_stop = src.stop_time.replace(" ", "_").replace(":", "");
                                    let csv_path = format!(".cache/ephemeris/target_{}_{}_{}.csv", src.target_id, safe_start, safe_stop);
                                    
                                    if !std::path::Path::new(&csv_path).exists() {
                                        if let Some(parent) = std::path::Path::new(&csv_path).parent() {
                                            let _ = std::fs::create_dir_all(parent);
                                        }
                                        
                                        info!("Mission data missing for target {}. Downloading from JPL Horizons...", src.target_id);
                                        let url = format!(
                                            "https://ssd.jpl.nasa.gov/api/horizons.api?format=text&COMMAND='{}'&OBJ_DATA='NO'&MAKE_EPHEM='YES'&EPHEM_TYPE='VECTORS'&CENTER='{}'&REF_PLANE='{}'&START_TIME='{}'&STOP_TIME='{}'&STEP_SIZE='{}'&CSV_FORMAT='YES'",
                                            src.command.replace(" ", "%20"),
                                            src.center.replace(" ", "%20"),
                                            src.ref_plane.replace(" ", "%20"),
                                            src.start_time.replace(" ", "%20"),
                                            src.stop_time.replace(" ", "%20"),
                                            src.step_size.replace(" ", "%20")
                                        );
                                        
                                        if let Ok(response) = ureq::get(&url).call() {
                                            if let Ok(text) = response.into_string() {
                                                if let Some(start_idx) = text.find("$$SOE") {
                                                    if let Some(end_idx) = text.find("$$EOE") {
                                                        let csv_data = &text[start_idx..end_idx];
                                                        let clean_csv = csv_data.replace("$$SOE", "").replace("$$EOE", "");
                                                        if let Err(e) = std::fs::write(&csv_path, clean_csv) {
                                                            error!("Failed to write data to {}: {}", csv_path, e);
                                                        } else {
                                                            info!("Successfully downloaded data for target {}.", src.target_id);
                                                        }
                                                    }
                                                }
                                            }
                                        } else {
                                            error!("Failed to download data from JPL Horizons API for target {}.", src.target_id);
                                        }
                                    }

                                    let mut points = Vec::new();
                                    if let Ok(file) = File::open(&csv_path) {
                                        let reader = BufReader::new(file);
                                        for line in reader.lines().map_while(Result::ok) {
                                            if line.contains("$$") || line.trim().is_empty() { continue; }
                                            let parts: Vec<&str> = line.split(',').collect();
                                            if parts.len() >= 5 {
                                                if let (Ok(jd), Ok(x), Ok(y), Ok(z)) = (
                                                    parts[0].trim().parse::<f64>(),
                                                    parts[2].trim().parse::<f64>(),
                                                    parts[3].trim().parse::<f64>(),
                                                    parts[4].trim().parse::<f64>(),
                                                ) {
                                                    const AU_KM: f64 = 149_597_870.7;
                                                    points.push(CsvDataPoint {
                                                        jd,
                                                        pos_au: DVec3::new(x / AU_KM, y / AU_KM, z / AU_KM),
                                                    });
                                                }
                                            }
                                        }
                                        points.sort_by(|a, b| a.jd.partial_cmp(&b.jd).unwrap_or(std::cmp::Ordering::Equal));
                                        custom_data.insert(src.target_id, points);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        
        Self {
            _sun: Vsop2013Sun,
            earth: Vsop2013Earth::new(),
            emb: Vsop2013Emb,
            moon: ElpMpp02Moon::new(),
            custom_data,
        }
    }
}

impl Default for CelestialEphemerisProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl EphemerisProvider for CelestialEphemerisProvider {
    fn position(&self, body_id: i32, epoch_jd: f64) -> DVec3 {
        let julian = JulianDate::new(epoch_jd, 0.0);
        let tdb = TDB::from_julian_date(julian);
        
        match body_id {
            10 => DVec3::ZERO, // Sun is origin
            3 => { // EMB relative to Sun
                let p = self.emb.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                DVec3::new(p.x, p.y, p.z)
            },
            399 => { // Earth relative to EMB
                let p_emb = self.emb.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                let p_earth = self.earth.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                DVec3::new(p_earth.x - p_emb.x, p_earth.y - p_emb.y, p_earth.z - p_emb.z)
            },
            301 => { // Moon relative to EMB
                // ELP/MPP02 returns Geocentric ICRS (relative to Earth center)
                let p_m_geo_arr = self.moon.geocentric_position_icrs(&tdb).unwrap_or_else(|_| [0.0, 0.0, 0.0]);
                const AU_KM: f64 = 149_597_870.7;
                let mut p_m_geo_au = DVec3::new(p_m_geo_arr[0] / AU_KM, p_m_geo_arr[1] / AU_KM, p_m_geo_arr[2] / AU_KM);
                
                // Rotate Geocentric ICRS to ECLIPTIC
                let epsilon = (23.439281f64).to_radians();
                let (sin_e, cos_e) = epsilon.sin_cos();
                let y = p_m_geo_au.y * cos_e + p_m_geo_au.z * sin_e;
                let z = -p_m_geo_au.y * sin_e + p_m_geo_au.z * cos_e;
                p_m_geo_au.y = y;
                p_m_geo_au.z = z;

                // Earth relative to EMB
                let p_emb = self.emb.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                let p_earth = self.earth.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                let p_earth_rel_emb = DVec3::new(p_earth.x - p_emb.x, p_earth.y - p_emb.y, p_earth.z - p_emb.z);
                
                // Moon rel EMB = (Moon rel Earth) + (Earth rel EMB)
                p_m_geo_au + p_earth_rel_emb
            },
            other_id => {
                if let Some(data) = self.custom_data.get(&other_id) {
                    if !data.is_empty() {
                        if epoch_jd <= data.first().unwrap().jd {
                            return data.first().unwrap().pos_au;
                        }
                        if epoch_jd >= data.last().unwrap().jd {
                            return data.last().unwrap().pos_au;
                        }
                        // Binary search
                        let idx = data.partition_point(|p| p.jd <= epoch_jd);
                        if idx > 0 && idx < data.len() {
                            let p0 = &data[idx - 1];
                            let p1 = &data[idx];
                            let t = (epoch_jd - p0.jd) / (p1.jd - p0.jd);
                            return p0.pos_au.lerp(p1.pos_au, t);
                        }
                    }
                }
                
                // Fallback mock (if CSV is missing for -1024)
                if other_id == -1024 {
                    let period = 10.3; // days
                    let t = (epoch_jd % period) / period; // 0.0 to 1.0
                    
                    // Get Moon relative to Earth in Ecliptic
                    let jd_tdb = TDB::from_julian_date(JulianDate::new(epoch_jd, 0.0));
                    let p_m_geo = self.moon.geocentric_position_icrs(&jd_tdb).unwrap_or([0.0, 0.0, 0.0]);
                    const AU_KM: f64 = 149_597_870.7;
                    let mut p_m_geo_au = DVec3::new(p_m_geo[0] / AU_KM, p_m_geo[1] / AU_KM, p_m_geo[2] / AU_KM);
                    
                    // Rotate to ecliptic
                    let epsilon = (23.439281f64).to_radians();
                    let (sin_e, cos_e) = epsilon.sin_cos();
                    let y = p_m_geo_au.y * cos_e + p_m_geo_au.z * sin_e;
                    let z = -p_m_geo_au.y * sin_e + p_m_geo_au.z * cos_e;
                    p_m_geo_au.y = y;
                    p_m_geo_au.z = z;
                    
                    // Create a trajectory that goes from Earth to Moon and back
                    // t=0 (Earth), t=0.5 (Moon), t=1.0 (Earth)
                    // Distance d(t) = sin(t * PI) * 1.02  (goes slightly behind Moon)
                    let d = (t * std::f64::consts::PI).sin() * 1.02;
                    
                    // Lateral offset to make it a loop (figure-8 style)
                    // t=0.25 (outbound max lateral), t=0.75 (inbound max lateral)
                    let lateral_offset = (t * std::f64::consts::TAU).sin() * 0.15; // 15% lateral swing
                    
                    // Perpendicular vector for the offset (in orbital plane)
                    // Moon orbit is roughly in ecliptic (slight inclination)
                    // We use cross product with Y up to get lateral vector
                    let normal = DVec3::new(-p_m_geo_au.z, 0.0, p_m_geo_au.x).normalize_or_zero();
                    
                    // Return position relative to Earth (since global_position will add Earth helio)
                    return p_m_geo_au * d + normal * lateral_offset * p_m_geo_au.length();
                }
                
                DVec3::ZERO
            },
        }
    }
}

