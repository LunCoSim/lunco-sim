//! # Celestial Ephemeris & Trajectory Propagation
//!
//! This module implements the "Orrery"—the high-fidelity engine that computes 
//! the positions of all solar system bodies at any given epoch.
//!
//! ## The Strategy: Hybrid Analytical/Tabular Propagation
//! To achieve high performance without sacrificing precision, we use a 
//! hybrid approach:
//! 1. **Analytical Models (Fixed Bodies)**:
//!    - **VSOP2013/87**: Used for major planets (Earth, Sun, EMB). Provides 
//!      sub-kilometer precision over thousands of years with zero disk I/O.
//!    - **ELP/MPP02**: High-fidelity lunar theory for the Moon, accounting 
//!      for complex tidal and gravitational perturbations.
//! 2. **Tabular Fallback (Dynamic Missions)**:
//!    - For spacecraft (like Artemis 2) or custom asteroids, we dynamically 
//!      fetch and cache **JPL Horizons** vector data (CSV) to ensure 
//!      parity with real-world mission trajectories.
//!
//! ## Coordinate Standards
//! All internal ephemeris calculations are performed in the **Ecliptic J2000** 
//! frame (AU units) before being converted into the simulation's local 
//! meter-based grids.

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

#[derive(Debug, Clone)]
pub struct CsvDataPoint {
    pub jd: f64,
    pub pos_au: DVec3,
}

#[derive(Debug, Deserialize)]
pub struct MissionConfig {
    pub ephemeris_sources: Option<Vec<EphemerisSource>>,
}

#[derive(Debug, Deserialize)]
pub struct EphemerisSource {
    pub target_id: i32,
    pub command: String,
    pub center: String,
    pub ref_plane: String,
    pub start_time: String,
    pub stop_time: String,
    pub step_size: String,
}

/// Abstract interface for any system providing spatial state over time.
pub trait EphemerisProvider: Send + Sync + 'static {
    /// Returns the position of a body relative to its parent (Ecliptic J2000, AU).
    fn position(&self, body_id: i32, epoch_jd: f64) -> DVec3;

    /// Returns the Heliocentric J2000 position of a body by recursively 
    /// resolving the gravitational hierarchy.
    fn global_position(&self, body_id: i32, epoch_jd: f64) -> DVec3 {
        let mut pos = self.position(body_id, epoch_jd);
        let mut current_id = body_id;
        
        // Resolve position relative to Sun (NAIF 10) by walking up the parent tree.
        for _ in 0..10 { 
            let parent_id = match current_id {
                399 => 3,     // Earth -> EMB
                301 => 3,     // Moon -> EMB
                3 => 10,      // EMB -> Sun
                -1024 => 399, // Custom Mission -> Earth
                10 => break,  
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

/// Thread-safe resource facilitating access to the active ephemeris engine.
#[derive(Resource)]
pub struct EphemerisResource {
    pub provider: Arc<dyn EphemerisProvider>,
}

/// Concrete implementation of the hybrid [EphemerisProvider].
///
/// It combines built-in analytical VSOP/ELP modules with a local 
/// cache of external mission data.
pub struct CelestialEphemerisProvider {
    _sun: Vsop2013Sun,
    earth: Vsop2013Earth,
    emb: Vsop2013Emb,
    moon: ElpMpp02Moon,
    /// Local cache of CSV data points indexed by NAIF ID.
    custom_data: std::collections::HashMap<i32, Vec<CsvDataPoint>>,
}

impl CelestialEphemerisProvider {
    /// Initializes the provider and performs a look-ahead discovery of 
    /// cached mission data in `assets/missions`.
    pub fn new() -> Self {
        let mut custom_data = std::collections::HashMap::new();
        let missions_dir = "assets/missions";
        
        // Dynamic discovery of mission CSVs to support hotswappable scenarios.
        if let Ok(entries) = std::fs::read_dir(missions_dir) {
            for entry in entries.flatten() {
                if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        if let Ok(config) = serde_json::from_str::<MissionConfig>(&content) {
                            if let Some(sources) = config.ephemeris_sources {
                                for src in sources {
                                    // Cache pathing and JPL Horizons sync logic...
                                    let safe_start = src.start_time.replace(" ", "_").replace(":", "");
                                    let safe_stop = src.stop_time.replace(" ", "_").replace(":", "");
                                    let csv_path = format!(".cache/ephemeris/target_{}_{}_{}.csv", src.target_id, safe_start, safe_stop);
                                    
                                    // Automatic synchronization with External Authorities (JPL)
                                    if !std::path::Path::new(&csv_path).exists() {
                                        if let Some(parent) = std::path::Path::new(&csv_path).parent() {
                                            let _ = std::fs::create_dir_all(parent);
                                        }
                                        
                                        info!("Fetching high-fidelity mission vectors for NAIF {}...", src.target_id);
                                        let url = format!(
                                            "https://ssd.jpl.nasa.gov/api/horizons.api?format=text&COMMAND='{}'&OBJ_DATA='NO'&MAKE_EPHEM='YES'&EPHEM_TYPE='VECTORS'&CENTER='{}'&REF_PLANE='{}'&START_TIME='{}'&STOP_TIME='{}'&STEP_SIZE='{}'&CSV_FORMAT='YES'",
                                            src.command.replace(" ", "%20"),
                                            src.center.replace(" ", "%20"),
                                            src.ref_plane.replace(" ", "%20"),
                                            src.start_time.replace(" ", "%20"),
                                            src.stop_time.replace(" ", "%20"),
                                            src.step_size.replace(" ", "%20")
                                        );

                                        #[cfg(not(target_arch = "wasm32"))]
                                        if let Ok(response) = ureq::get(&url).call() {
                                            if let Ok(text) = response.into_string() {
                                                if let Some(start_idx) = text.find("$$SOE") {
                                                    if let Some(end_idx) = text.find("$$EOE") {
                                                        let csv_data = &text[start_idx..end_idx];
                                                        let clean_csv = csv_data.replace("$$SOE", "").replace("$$EOE", "");
                                                        let _ = std::fs::write(&csv_path, clean_csv);
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // Parse and ingest cached tabular data.
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

    /// Wasm32 constructor that accepts embedded ephemeris CSV data.
    /// Used when filesystem access is unavailable.
    pub fn new_with_embedded_ephemeris(ephemeris_csvs: &[(&str, &str)]) -> Self {
        let mut custom_data = std::collections::HashMap::new();
        // Parse embedded CSV data: each entry is (target_id_string, csv_content)
        for (target_id_str, csv_content) in ephemeris_csvs {
            if let Ok(target_id) = target_id_str.parse::<i32>() {
                let mut points = Vec::new();
                for line in csv_content.lines() {
                    if line.trim().is_empty() || line.contains("$$") { continue; }
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
                custom_data.insert(target_id, points);
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
        // All calculations use Terrestrial Dynamic Time (TDB) to ensure 
        // high-precision alignment with gravitational theories.
        let julian = JulianDate::new(epoch_jd, 0.0);
        let tdb = TDB::from_julian_date(julian);
        
        match body_id {
            10 => DVec3::ZERO, // Sun is the Barycentric origin of our ecliptic frame.
            3 => { // Earth-Moon Barycenter relative to Sun
                let p = self.emb.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                DVec3::new(p.x, p.y, p.z)
            },
            399 => { // Earth relative to EMB
                let p_emb = self.emb.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                let p_earth = self.earth.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                DVec3::new(p_earth.x - p_emb.x, p_earth.y - p_emb.y, p_earth.z - p_emb.z)
            },
            301 => { // Moon relative to EMB (Hybrid ELP/MPP02)
                let p_m_geo_arr = self.moon.geocentric_position_icrs(&tdb).unwrap_or_else(|_| [0.0, 0.0, 0.0]);
                const AU_KM: f64 = 149_597_870.7;
                let mut p_m_geo_au = DVec3::new(p_m_geo_arr[0] / AU_KM, p_m_geo_arr[1] / AU_KM, p_m_geo_arr[2] / AU_KM);
                
                // Rotational Correction: Convert Geocentric ICRS to ECLIPTIC J2000.
                let epsilon = (23.439281f64).to_radians();
                let (sin_e, cos_e) = epsilon.sin_cos();
                let y = p_m_geo_au.y * cos_e + p_m_geo_au.z * sin_e;
                let z = -p_m_geo_au.y * sin_e + p_m_geo_au.z * cos_e;
                p_m_geo_au.y = y;
                p_m_geo_au.z = z;

                let p_emb = self.emb.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                let p_earth = self.earth.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                let p_earth_rel_emb = DVec3::new(p_earth.x - p_emb.x, p_earth.y - p_emb.y, p_earth.z - p_emb.z);
                
                p_m_geo_au + p_earth_rel_emb
            },
            other_id => {
                // Tabular Interpolation: Linear LERP between cached JPL Horizons points.
                if let Some(data) = self.custom_data.get(&other_id) {
                    if !data.is_empty() {
                        if epoch_jd <= data.first().unwrap().jd { return data.first().unwrap().pos_au; }
                        if epoch_jd >= data.last().unwrap().jd { return data.last().unwrap().pos_au; }
                        let idx = data.partition_point(|p| p.jd <= epoch_jd);
                        if idx > 0 && idx < data.len() {
                            let p0 = &data[idx - 1];
                            let p1 = &data[idx];
                            let t = (epoch_jd - p0.jd) / (p1.jd - p0.jd);
                            return p0.pos_au.lerp(p1.pos_au, t);
                        }
                    }
                }
                DVec3::ZERO
            },
        }
    }
}


