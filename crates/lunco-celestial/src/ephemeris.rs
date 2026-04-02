use bevy::prelude::*;
use bevy::math::DVec3;
use celestial_time::TDB;
use celestial_time::julian::JulianDate;
use celestial_ephemeris::{Vsop2013Earth, Vsop2013Sun, planets::Vsop2013Emb, moon::ElpMpp02Moon};
use celestial_core::Vector3;

use std::sync::Arc;

pub trait EphemerisProvider: Send + Sync + 'static {
    /// Position of body relative to its parent, in ecliptic J2000, AU.
    fn position(&self, body_id: u32, epoch_jd: f64) -> DVec3;
}

#[derive(Resource)]
pub struct EphemerisResource {
    pub provider: Arc<dyn EphemerisProvider>,
}

pub struct CelestialEphemerisProvider {
    sun: Vsop2013Sun,
    earth: Vsop2013Earth,
    emb: Vsop2013Emb,
    moon: ElpMpp02Moon,
}

impl CelestialEphemerisProvider {
    pub fn new() -> Self {
        Self {
            sun: Vsop2013Sun,
            earth: Vsop2013Earth::new(),
            emb: Vsop2013Emb,
            moon: ElpMpp02Moon::new(),
        }
    }
}

impl Default for CelestialEphemerisProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl EphemerisProvider for CelestialEphemerisProvider {
    fn position(&self, body_id: u32, epoch_jd: f64) -> DVec3 {
        let julian = JulianDate::new(epoch_jd, 0.0);
        let tdb = TDB::from_julian_date(julian);
        
        match body_id {
            10 => { // Sun helio
                let p = self.sun.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                DVec3::new(p.x, p.y, p.z)
            },
            3 => { // EMB helio
                let p = self.emb.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                DVec3::new(p.x, p.y, p.z)
            },
            399 => { // Earth helio
                let p_e = self.earth.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                DVec3::new(p_e.x, p_e.y, p_e.z)
            },
            301 => { // Moon helio
                // Moon relative to Earth (Geocentric ICRS)
                let p_m_geo_arr = self.moon.geocentric_position_icrs(&tdb).unwrap_or_else(|_| [0.0, 0.0, 0.0]);
                const AU_KM: f64 = 149_597_870.7;
                let mut p_m_geo_au = DVec3::new(p_m_geo_arr[0] / AU_KM, p_m_geo_arr[1] / AU_KM, p_m_geo_arr[2] / AU_KM);
                
                // --- Rotation from ICRS (Equatorial) to ECLIPTIC ---
                let epsilon = (23.439281f64).to_radians();
                let (sin_e, cos_e) = epsilon.sin_cos();
                let y = p_m_geo_au.y * cos_e + p_m_geo_au.z * sin_e;
                let z = -p_m_geo_au.y * sin_e + p_m_geo_au.z * cos_e;
                p_m_geo_au.y = y;
                p_m_geo_au.z = z;

                // Earth helio (Ecliptic)
                let p_e = self.earth.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                let p_e_helio = DVec3::new(p_e.x, p_e.y, p_e.z);
                
                p_e_helio + p_m_geo_au
            },
            _ => DVec3::ZERO,
        }
    }
}
