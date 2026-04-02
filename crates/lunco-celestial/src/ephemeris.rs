use bevy::prelude::*;
use bevy::math::DVec3;
use celestial_time::TDB;
use celestial_time::julian::JulianDate;
use celestial_ephemeris::{Vsop2013Earth, Vsop2013Sun, planets::Vsop2013Emb, moon::ElpMpp02Moon};
use celestial_core::Vector3;

use std::sync::Arc;
use std::fs::File;
use std::io::{BufRead, BufReader};

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

pub struct CelestialEphemerisProvider {
    _sun: Vsop2013Sun,
    earth: Vsop2013Earth,
    emb: Vsop2013Emb,
    moon: ElpMpp02Moon,
    artemis2_data: Vec<CsvDataPoint>,
}

impl CelestialEphemerisProvider {
    pub fn new() -> Self {
        let cache_dir = ".cache/ephemeris";
        let csv_path = ".cache/ephemeris/artemis_vectors.csv";
        
        // Ensure cache directory exists
        let _ = std::fs::create_dir_all(cache_dir);
        
        // Auto-download data if missing
        if !std::path::Path::new(csv_path).exists() {
            info!("Artemis 2 mission data missing. Downloading from JPL Horizons...");
            let start_time = "2026-04-02 02:00";
            let stop_time = "2026-04-10 23:50";
            let url = format!(
                "https://ssd.jpl.nasa.gov/api/horizons.api?format=text&COMMAND='-1024'&OBJ_DATA='NO'&MAKE_EPHEM='YES'&EPHEM_TYPE='VECTORS'&CENTER='500@399'&REF_PLANE='ECLIPTIC'&START_TIME='{}'&STOP_TIME='{}'&STEP_SIZE='10m'&CSV_FORMAT='YES'",
                start_time.replace(" ", "%20"), stop_time.replace(" ", "%20")
            );
            
            if let Ok(response) = ureq::get(&url).call() {
                if let Ok(text) = response.into_string() {
                    if let Some(start_idx) = text.find("$$SOE") {
                        if let Some(end_idx) = text.find("$$EOE") {
                            let csv_data = &text[start_idx..end_idx];
                            // Remove $$SOE and $$EOE markers, keep just the data lines
                            let clean_csv = csv_data.replace("$$SOE", "").replace("$$EOE", "");
                            if let Err(e) = std::fs::write(csv_path, clean_csv) {
                                error!("Failed to write Artemis 2 data to {}: {}", csv_path, e);
                            } else {
                                info!("Successfully downloaded and saved Artemis 2 data.");
                            }
                        }
                    }
                }
            } else {
                error!("Failed to download Artemis 2 data from JPL Horizons API.");
            }
        }

        let mut artemis2_data = Vec::new();
        // Load Artemis 2 data from CSV if exists
        if let Ok(file) = File::open(csv_path) {
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
                        artemis2_data.push(CsvDataPoint {
                            jd,
                            pos_au: DVec3::new(x / AU_KM, y / AU_KM, z / AU_KM),
                        });
                    }
                }
            }
            // Ensure sorted by JD
            artemis2_data.sort_by(|a, b| a.jd.partial_cmp(&b.jd).unwrap_or(std::cmp::Ordering::Equal));
        }
        
        Self {
            _sun: Vsop2013Sun,
            earth: Vsop2013Earth::new(),
            emb: Vsop2013Emb,
            moon: ElpMpp02Moon::new(),
            artemis2_data,
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
            -1024 => { // Artemis 2
                if !self.artemis2_data.is_empty() {
                    let data = &self.artemis2_data;
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
                
                // Fallback mock (if CSV is missing)
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
                p_m_geo_au * d + normal * lateral_offset * p_m_geo_au.length()
            },
            _ => DVec3::ZERO,
        }
    }
}

