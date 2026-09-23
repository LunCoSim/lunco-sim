//! # lunco-celestial-ephemeris
//!
//! Concrete high-fidelity ephemeris provider for `lunco-celestial`.
//!
//! This crate is the heavy half of the celestial split: it pulls in
//! `celestial-ephemeris` (VSOP2013 + ELP/MPP02), `celestial-time`, and
//! `celestial-core` — none of which build on Windows MSVC because
//! `celestial-eop-data`'s `build.rs` shells out to the Unix `date`
//! command.
//!
//! Apps that need real planetary positions add [`EphemerisPlugin`]. The
//! celestial core owns the required provider contract; this plugin supplies
//! the production VSOP/ELP implementation for that contract.

use bevy::math::DVec3;
use bevy::prelude::*;
use celestial_core::Vector3;
use celestial_ephemeris::{moon::ElpMpp02Moon, planets::Vsop2013Emb, Vsop2013Earth, Vsop2013Sun};
use celestial_time::julian::JulianDate;
use celestial_time::TDB;
use lunco_celestial::ephemeris_id::{EARTH, EARTH_MOON_BARYCENTER, MOON, SUN};
use lunco_celestial::frames::{EclipticAu, IcrfAu};

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use lunco_celestial::ephemeris::{CsvDataPoint, EphemerisProvider, EphemerisResource};

/// Concrete implementation of the hybrid [`EphemerisProvider`].
///
/// Combines built-in analytical VSOP/ELP modules with scene-selected external
/// dataset artifacts (JPL Horizons CSV).
pub struct CelestialEphemerisProvider {
    _sun: Vsop2013Sun,
    earth: Vsop2013Earth,
    emb: Vsop2013Emb,
    moon: ElpMpp02Moon,
    // `Arc<RwLock>` lets the asset runtime publish scene-requested
    // vectors without widening the read-only provider trait. Reads on the
    // `position` path take an uncontended read lock.
    custom_data: Arc<RwLock<HashMap<i32, Vec<CsvDataPoint>>>>,
    /// Body-to-parent relationships from the body registry and the selected
    /// datasets' declared `center` metadata.
    parents: Arc<RwLock<HashMap<i32, i32>>>,
    /// Changes whenever a scene-requested dataset becomes available. The cadence
    /// policy observes this atomic revision instead of locking provider data
    /// on every render frame.
    motion_revision: Arc<AtomicU64>,
}

const AU_KM: f64 = 149_597_870.7;

/// Convert a JPL Horizons `CENTER` (`"@399"`, `"500@399"`, `"399"`) to its NAIF id.
fn parse_center(center: &str) -> Option<i32> {
    center.rsplit('@').next()?.trim().parse::<i32>().ok()
}

/// The parent tree, straight out of the body registry — no second copy.
fn parents_from_registry() -> HashMap<i32, i32> {
    lunco_celestial::CelestialBodyRegistry::default_system()
        .bodies
        .iter()
        .filter_map(|b| b.parent_id.map(|p| (b.ephemeris_id, p)))
        .collect()
}

/// Parse one complete JPL-Horizons CSV vector block into strictly ordered
/// [`CsvDataPoint`]s. Data column layout is `jd, calendar, x, y, z, ...`.
fn parse_ephemeris_csv(text: &str) -> Result<Vec<CsvDataPoint>, String> {
    let mut points = Vec::new();
    let mut in_data = false;
    let mut saw_start = false;
    let mut saw_end = false;
    for (line_index, line) in text.lines().enumerate() {
        let line = line.trim();
        if !in_data {
            if line == "$$SOE" {
                if saw_start {
                    return Err(format!("line {}: duplicate $$SOE marker", line_index + 1));
                }
                saw_start = true;
                in_data = true;
            }
            continue;
        }
        if line == "$$EOE" {
            saw_end = true;
            break;
        }
        if line.is_empty() {
            continue;
        }

        let mut columns = line.split(',');
        let first = columns.next().unwrap_or_default().trim();
        let row_error = |reason: &str| format!("line {}: {reason}", line_index + 1);
        let jd = first
            .parse::<f64>()
            .map_err(|_| row_error("Julian date must be numeric"))?;
        if !jd.is_finite() {
            return Err(row_error("Julian date must be finite"));
        }
        if points
            .last()
            .is_some_and(|previous: &CsvDataPoint| previous.jd >= jd)
        {
            return Err(row_error("Julian dates must be strictly increasing"));
        }
        let values = columns.collect::<Vec<_>>();
        if values.len() < 4 {
            return Err(row_error(
                "vector row requires calendar and three coordinates",
            ));
        }
        let parse_coordinate = |column: usize, axis: &str| {
            values[column]
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| row_error(&format!("{axis} coordinate must be finite numeric data")))
        };
        let (x, y, z) = (
            parse_coordinate(1, "x")?,
            parse_coordinate(2, "y")?,
            parse_coordinate(3, "z")?,
        );
        points.push(CsvDataPoint {
            jd,
            // Horizons vectors use the ecliptic frame specified by the
            // dataset declaration's `REF_PLANE=ECLIPTIC` query.
            pos_au: EclipticAu::new(DVec3::new(x / AU_KM, y / AU_KM, z / AU_KM)),
        });
    }
    if !saw_start {
        return Err("missing $$SOE marker".into());
    }
    if !saw_end {
        return Err("missing $$EOE marker".into());
    }
    if points.is_empty() {
        return Err("no Horizons vector rows were found".into());
    }
    Ok(points)
}

/// Return the exact angular-rate bound of the provider's piecewise-linear CSV
/// interpolation. Only adjacent in-coverage samples contribute motion. An invalid segment or one
/// passing through the origin cannot provide a finite direction bound and
/// therefore keeps the cadence gate exact.
fn maximum_piecewise_linear_angular_rate(points: &[CsvDataPoint]) -> f64 {
    let mut maximum_rate = 0.0_f64;
    for pair in points.windows(2) {
        let p0 = pair[0].pos_au.raw();
        let p1 = pair[1].pos_au.raw();
        let dt = pair[1].jd - pair[0].jd;
        if !dt.is_finite() || dt <= 0.0 || !p0.is_finite() || !p1.is_finite() {
            return f64::INFINITY;
        }

        let velocity = (p1 - p0) / dt;
        let velocity_squared = velocity.length_squared();
        if velocity_squared == 0.0 {
            continue;
        }

        // The interpolated point is p(t) = p0 + velocity * (t * dt),
        // equivalently p0 + velocity * tau for tau in [0, dt]. Find the
        // closest point on that segment to the origin; angular speed is
        // |p × v| / |p|² and is maximized at the smallest radius here.
        let tau = (-p0.dot(velocity) / velocity_squared).clamp(0.0, dt);
        let closest = p0 + velocity * tau;
        let radius_squared = closest.length_squared();
        if !radius_squared.is_finite() || radius_squared <= f64::MIN_POSITIVE {
            return f64::INFINITY;
        }

        let rate = p0.cross(velocity).length() / radius_squared;
        if !rate.is_finite() {
            return f64::INFINITY;
        }
        maximum_rate = maximum_rate.max(rate);
    }
    maximum_rate
}

/// The `[<key>.ephemeris]` sub-table of a declared dataset: what the bytes are.
///
/// Transport (`url`, `dest`, `sha256`) and this domain metadata share one
/// manifest entry; the downloader ignores this sub-table.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct EphemerisDatasetMeta {
    /// NAIF id these vectors describe.
    pub naif_id: i32,
    /// JPL `CENTER` of the query that produced them (`"500@399"`, `"@399"`,
    /// `"399"`) — the body the positions are relative to. It is a property of
    /// THESE bytes, not a scene choice: read it wrong and the body is placed
    /// around the wrong parent.
    pub center: String,
}

/// Parse a complete delivered Horizons response into ordered vectors.
///
/// A present but incomplete or malformed asset returns an error; it is never
/// silently treated as "no data".
fn parse_vectors(text: &str) -> Result<Vec<CsvDataPoint>, String> {
    parse_ephemeris_csv(text)
}

impl CelestialEphemerisProvider {
    /// Build the analytic provider with the standard body's parent tree.
    /// Scene-selected datasets are parsed only when the generic asset runtime
    /// delivers their bytes.
    pub fn new() -> Self {
        Self {
            _sun: Vsop2013Sun,
            earth: Vsop2013Earth::new(),
            emb: Vsop2013Emb,
            moon: ElpMpp02Moon::new(),
            custom_data: Arc::new(RwLock::new(HashMap::new())),
            parents: Arc::new(RwLock::new(parents_from_registry())),
            motion_revision: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl Default for CelestialEphemerisProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Rotate an **equatorial / ICRS** rectangular vector into **ecliptic J2000**
/// (rotation about +X by the J2000 mean obliquity).
///
/// The typed ICRF input and ecliptic output keep provider coordinates aligned
/// with downstream geodesy. The obliquity is shared with the IAU pole transform.
pub fn equatorial_to_ecliptic(p: IcrfAu) -> EclipticAu {
    let epsilon = lunco_celestial::iau::OBLIQUITY_J2000_DEG.to_radians();
    let (sin_e, cos_e) = epsilon.sin_cos();
    let p = p.raw();
    EclipticAu::new(DVec3::new(
        p.x,
        p.y * cos_e + p.z * sin_e,
        -p.y * sin_e + p.z * cos_e,
    ))
}

impl CelestialEphemerisProvider {
    /// Return no position when the analytical library cannot evaluate this epoch.
    fn emb_heliocentric(&self, tdb: &TDB) -> Option<Vector3> {
        self.emb.heliocentric_position(tdb).ok().or_else(|| {
            bevy::log::warn_once!(
                "[ephemeris] VSOP2013 EMB evaluation failed — Earth and Moon will not be placed."
            );
            None
        })
    }

    fn earth_heliocentric(&self, tdb: &TDB) -> Option<Vector3> {
        self.earth.heliocentric_position(tdb).ok().or_else(|| {
            bevy::log::warn_once!(
                "[ephemeris] VSOP2013 Earth evaluation failed — Earth and Moon will not be placed."
            );
            None
        })
    }

    fn moon_geocentric_icrs(&self, tdb: &TDB) -> Option<[f64; 3]> {
        self.moon.geocentric_position_icrs(tdb).ok().or_else(|| {
            bevy::log::warn_once!(
                "[ephemeris] ELP/MPP02 Moon evaluation failed — the Moon will not be placed."
            );
            None
        })
    }
}

impl EphemerisProvider for CelestialEphemerisProvider {
    fn parent_id(&self, body_id: i32) -> Option<i32> {
        self.parents.read().ok()?.get(&body_id).copied()
    }

    fn position(&self, body_id: i32, epoch_jd: f64) -> Option<EclipticAu> {
        if !epoch_jd.is_finite() {
            return None;
        }
        let julian = JulianDate::new(epoch_jd, 0.0);
        let tdb = TDB::from_julian_date(julian);

        match body_id {
            SUN => Some(EclipticAu::ZERO), // the Sun IS the origin of this frame
            EARTH_MOON_BARYCENTER => {
                let p = self.emb_heliocentric(&tdb)?;
                Some(equatorial_to_ecliptic(IcrfAu::new(DVec3::new(
                    p.x, p.y, p.z,
                ))))
            }
            EARTH => {
                let p_emb = self.emb_heliocentric(&tdb)?;
                let p_earth = self.earth_heliocentric(&tdb)?;
                Some(equatorial_to_ecliptic(IcrfAu::new(DVec3::new(
                    p_earth.x - p_emb.x,
                    p_earth.y - p_emb.y,
                    p_earth.z - p_emb.z,
                ))))
            }
            MOON => {
                let p_m_geo_arr = self.moon_geocentric_icrs(&tdb)?;
                const AU_KM: f64 = 149_597_870.7;
                let p_m_geo_au = equatorial_to_ecliptic(IcrfAu::new(DVec3::new(
                    p_m_geo_arr[0] / AU_KM,
                    p_m_geo_arr[1] / AU_KM,
                    p_m_geo_arr[2] / AU_KM,
                )));

                let p_emb = self.emb_heliocentric(&tdb)?;
                let p_earth = self.earth_heliocentric(&tdb)?;
                let p_earth_rel_emb = equatorial_to_ecliptic(IcrfAu::new(DVec3::new(
                    p_earth.x - p_emb.x,
                    p_earth.y - p_emb.y,
                    p_earth.z - p_emb.z,
                )));

                Some(p_m_geo_au + p_earth_rel_emb)
            }
            other_id => {
                // Uncontended read lock; the scene-requested asset read only
                // takes a write lock briefly when its parsed vectors arrive.
                let guard = self.custom_data.read().unwrap_or_else(|e| e.into_inner());
                if let Some(data) = guard.get(&other_id) {
                    if data.first().is_some_and(|point| epoch_jd < point.jd)
                        || data.last().is_some_and(|point| epoch_jd > point.jd)
                    {
                        return None;
                    }
                    if data.len() == 1 {
                        return data
                            .first()
                            .filter(|point| point.jd == epoch_jd)
                            .map(|point| point.pos_au);
                    }
                    let idx = data.partition_point(|p| p.jd <= epoch_jd);
                    if idx == 0 {
                        return None;
                    }
                    if idx == data.len() {
                        return data.last().map(|point| point.pos_au);
                    }
                    let p0 = &data[idx - 1];
                    let p1 = &data[idx];
                    let t = (epoch_jd - p0.jd) / (p1.jd - p0.jd);
                    return Some(p0.pos_au.lerp(p1.pos_au, t));
                }
                // A body without an analytic solution or selected dataset has
                // no position; callers skip placement and rendering.
                bevy::log::warn_once!(
                    "[ephemeris] no data for NAIF id {body_id} — it will not be placed."
                );
                None
            }
        }
    }

    fn maximum_angular_rate_rad_per_day(&self) -> f64 {
        // The analytic provider's fastest parent-relative vector is the
        // Moon's sidereal orbit. CSV positions are linearly interpolated by
        // `position`, so their bound is derived from the same authoritative
        // samples rather than forcing every render frame to solve exactly.
        let custom_rate = self
            .custom_data
            .read()
            .map(|data| {
                data.values()
                    .map(|points| maximum_piecewise_linear_angular_rate(points))
                    .fold(0.0, f64::max)
            })
            .unwrap_or(f64::INFINITY);
        (std::f64::consts::TAU / 27.321_661).max(custom_rate)
    }

    fn motion_revision(&self) -> u64 {
        self.motion_revision.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use lunco_celestial::{solar_tangent_frame, CelestialBodyRegistry, Geodetic};

    fn csv_point(jd: f64, x: f64, y: f64, z: f64) -> CsvDataPoint {
        CsvDataPoint {
            jd,
            pos_au: EclipticAu::new(DVec3::new(x, y, z)),
        }
    }

    #[test]
    fn vector_csv_rejects_invalid_or_repeated_samples() {
        let valid = "header\n$$SOE\n2451545.0, J2000, 149597870.7, 0, 0\n2451546.0, J2000, 0, 149597870.7, 0\n$$EOE";
        let points = parse_vectors(valid).expect("valid Horizons rows");
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].jd, 2_451_545.0);

        let duplicate = "$$SOE\n2451545.0, J2000, 1, 2, 3\n2451545.0, J2000, 4, 5, 6\n$$EOE";
        assert!(parse_vectors(duplicate).is_err());

        let non_finite = "$$SOE\n2451545.0, J2000, NaN, 2, 3\n$$EOE";
        assert!(parse_vectors(non_finite).is_err());

        let malformed_row = "$$SOE\ninvalid, J2000, 1, 2, 3\n$$EOE";
        assert!(parse_vectors(malformed_row).is_err());

        let missing_end = "$$SOE\n2451545.0, J2000, 1, 2, 3";
        assert!(parse_vectors(missing_end).is_err());
    }

    #[test]
    fn custom_vector_positions_are_unavailable_outside_dataset_coverage() {
        let provider = CelestialEphemerisProvider::new();
        const TEST_BODY: i32 = 10_024;
        provider.custom_data.write().unwrap().insert(
            TEST_BODY,
            vec![
                csv_point(10.0, 1.0, 0.0, 0.0),
                csv_point(20.0, 3.0, 0.0, 0.0),
            ],
        );

        assert!(provider.position(TEST_BODY, 9.0).is_none());
        assert_eq!(provider.position(TEST_BODY, 10.0).unwrap().raw().x, 1.0);
        assert_eq!(provider.position(TEST_BODY, 15.0).unwrap().raw().x, 2.0);
        assert_eq!(provider.position(TEST_BODY, 20.0).unwrap().raw().x, 3.0);
        assert!(provider.position(TEST_BODY, 21.0).is_none());
        assert!(provider.position(TEST_BODY, f64::NAN).is_none());
    }

    #[test]
    fn csv_motion_bound_matches_piecewise_linear_interpolation() {
        let points = [csv_point(0.0, 1.0, 0.0, 0.0), csv_point(2.0, 1.0, 1.0, 0.0)];

        // p(t) = (1, t / 2, 0), so the maximum angular rate is 1/2 rad/day
        // at the first sample and decreases as the radius grows.
        let rate = maximum_piecewise_linear_angular_rate(&points);
        assert!((rate - 0.5).abs() < 1.0e-12, "unexpected rate: {rate}");
    }

    #[test]
    fn csv_motion_bound_rejects_segments_through_origin() {
        let points = [
            csv_point(0.0, 1.0, 0.0, 0.0),
            csv_point(1.0, -1.0, 0.0, 0.0),
        ];

        assert!(maximum_piecewise_linear_angular_rate(&points).is_infinite());
    }

    /// Shared coordinate conversion used by the provider's consumers.
    use lunco_celestial::coords::ecliptic_to_bevy;

    /// End-to-end frame check: with the provider's equatorial→ecliptic
    /// conversion and the tilt-aware geodesy, the sun's elevation at the
    /// Shackleton site must stay GRAZING (bounded by the moon axis tilt +
    /// site colatitude, ~±2.5°) and must actually rise above +1° at some
    /// epoch within a year.
    #[test]
    fn shackleton_sun_stays_grazing_and_gets_lit_epochs() {
        let provider = CelestialEphemerisProvider::new();
        let registry = CelestialBodyRegistry::default_system();
        let moon = registry
            .bodies
            .iter()
            .find(|b| b.ephemeris_id == 301)
            .unwrap();
        let site = Geodetic::new(-89.45, -136.7, 1200.0);

        let mut best = (0.0_f64, f64::MIN);
        for step in 0..=(366 * 4) {
            let jd = 2461228.5 + step as f64 * 0.25; // 6 h steps from 2026-07-07
            let p_moon = provider
                .global_position(MOON, jd)
                .expect("VSOP/ELP always have the Moon");
            let center_m = ecliptic_to_bevy(p_moon).raw();
            let frame = solar_tangent_frame(moon, &site, center_m, jd);
            let to_sun = ecliptic_to_bevy(-p_moon).normalize().raw();
            let elev_deg = to_sun.dot(frame.up).clamp(-1.0, 1.0).asin().to_degrees();
            assert!(
                elev_deg.abs() < 2.5,
                "polar sun must graze (|elev| < 2.5°), got {elev_deg:.2}° at JD {jd:.2}"
            );
            if elev_deg > best.1 {
                best = (jd, elev_deg);
            }
        }
        println!(
            "best lit epoch: JD {:.2} (elevation {:.3}°)",
            best.0, best.1
        );
        assert!(
            best.1 > 1.0,
            "Shackleton should reach >1° sun elevation within a year; best {:.3}°",
            best.1
        );
    }

    /// The tidally locked Moon keeps the sub-Earth point near lunar longitude
    /// zero, allowing for optical libration.
    #[test]
    fn moon_near_side_faces_earth_across_epochs() {
        use lunco_celestial::{body_fixed_to_geodetic, body_rotation};

        let provider = CelestialEphemerisProvider::new();
        let registry = CelestialBodyRegistry::default_system();
        let moon = registry
            .bodies
            .iter()
            .find(|b| b.ephemeris_id == 301)
            .unwrap();

        let mut worst = (0.0_f64, 0.0_f64);
        // ~14 months at 11-day steps: samples every phase of the libration cycle.
        for step in 0..40 {
            let jd = 2_451_545.0 + step as f64 * 11.0;

            // Earth as seen from the Moon, in the engine (ecliptic-Bevy) frame.
            let to_earth = ecliptic_to_bevy(
                provider.global_position(EARTH, jd).expect("Earth")
                    - provider.global_position(MOON, jd).expect("Moon"),
            )
            .normalize()
            .raw();

            // Into the Moon's body-fixed frame → the sub-Earth geodetic point.
            let body_fixed = body_rotation(moon, jd).inverse() * to_earth;
            let sub_earth = body_fixed_to_geodetic(body_fixed, 1.0);

            // Longitude is the one the missing W₀ destroyed. (Latitude librates
            // ±6.7° too, from the 1.54° pole tilt + the 5.1° orbit inclination.)
            let lon = sub_earth.lon_deg;
            assert!(
                lon.abs() < 10.0,
                "the sub-Earth point must stay near lunar lon 0 (tidal lock; \
                 optical libration is ±8°), got {lon:.2}° at JD {jd:.1} — \
                 ≈38° means the W₀ prime-meridian epoch went missing again"
            );
            assert!(
                sub_earth.lat_deg.abs() < 10.0,
                "sub-Earth latitude librates ±6.7°, got {:.2}° at JD {jd:.1}",
                sub_earth.lat_deg
            );
            if lon.abs() > worst.1.abs() {
                worst = (jd, lon);
            }
        }
        println!(
            "worst sub-Earth longitude: {:.2}° at JD {:.1}",
            worst.1, worst.0
        );

        // And it must genuinely LIBRATE, not be pinned at 0 by a degenerate
        // model that happens to satisfy the bound.
        assert!(
            worst.1.abs() > 1.0,
            "the sub-Earth longitude should librate by several degrees; \
             |max| was only {:.3}°",
            worst.1.abs()
        );
    }
}

/// Install the full VSOP/ELP/JPL implementation for the celestial provider
/// contract.
///
/// ```ignore
/// app.add_plugins(lunco_celestial_spatial::CelestialPlugin)
///    .add_plugins(lunco_celestial_ephemeris::EphemerisPlugin);
/// ```
pub struct EphemerisPlugin;

impl Plugin for EphemerisPlugin {
    fn build(&self, app: &mut App) {
        let provider = CelestialEphemerisProvider::new();
        // Handle onto the same maps the provider reads, so a dataset that
        // arrives later reaches `position()` without a restart. The trait is
        // read-only by design (a provider answers questions, it is not a
        // store), so the writable side is held here rather than widened there.
        app.insert_resource(EphemerisVectors {
            data: provider.custom_data.clone(),
            parents: provider.parents.clone(),
            motion_revision: provider.motion_revision.clone(),
        });
        app.insert_resource(EphemerisResource {
            provider: Arc::new(provider),
        });

        #[cfg(not(target_arch = "wasm32"))]
        {
            app.init_resource::<LoadedEphemerisDatasets>()
                .add_observer(consume_ephemeris_dataset_artifact)
                .add_systems(lunco_core::SceneTeardown, clear_scene_ephemeris_datasets);
        }
    }
}

/// Parse a generic asset-runtime delivery when its declaration carries
/// ephemeris metadata. Dataset selection and text reads belong to the authored
/// application asset lifecycle policy and generic asset runtime respectively.
#[cfg(not(target_arch = "wasm32"))]
fn consume_ephemeris_dataset_artifact(
    trigger: On<lunco_assets_runtime::DatasetTextArtifactReady>,
    registry: Option<Res<lunco_assets_datasets::DatasetRegistry>>,
    vectors: Res<EphemerisVectors>,
    mut loaded: ResMut<LoadedEphemerisDatasets>,
) {
    let Some(registry) = registry else {
        error!("[ephemeris] dataset delivery has no declared dataset registry");
        return;
    };
    let text = &trigger.event().text;
    let Some(entry) = registry.entry(&trigger.event().id) else {
        error!(
            "[ephemeris] delivered dataset '{}' is no longer declared",
            trigger.event().id
        );
        return;
    };
    let Some(meta) = entry.spec.domain::<EphemerisDatasetMeta>("ephemeris") else {
        return;
    };
    let meta = match meta {
        Ok(meta) => meta,
        Err(error) => {
            error!(
                "[ephemeris] dataset '{}' has malformed ephemeris metadata: {error}",
                entry.id
            );
            return;
        }
    };
    let Some(parent_id) = parse_center(&meta.center) else {
        error!(
            "[ephemeris] dataset '{}' has an unparseable center '{}'",
            entry.id, meta.center
        );
        return;
    };
    let points = match parse_vectors(text) {
        Ok(points) => points,
        Err(error) => {
            error!("[ephemeris] dataset '{}' is invalid: {error}", entry.id);
            return;
        }
    };

    if let Some(previous_owner) = loaded.owners.get(&meta.naif_id) {
        if previous_owner != &entry.id {
            error!(
                "[ephemeris] datasets '{}' and '{}' both define NAIF {}; scene policy must select one",
                previous_owner, entry.id, meta.naif_id
            );
            return;
        }
    } else {
        loaded.owners.insert(meta.naif_id, entry.id.clone());
        loaded.prior_parents.insert(
            meta.naif_id,
            vectors
                .parents
                .read()
                .unwrap_or_else(|error| error.into_inner())
                .get(&meta.naif_id)
                .copied(),
        );
        loaded.prior_data.insert(
            meta.naif_id,
            vectors
                .data
                .read()
                .unwrap_or_else(|error| error.into_inner())
                .get(&meta.naif_id)
                .cloned(),
        );
    }
    vectors
        .parents
        .write()
        .unwrap_or_else(|error| error.into_inner())
        .insert(meta.naif_id, parent_id);
    vectors
        .data
        .write()
        .unwrap_or_else(|error| error.into_inner())
        .insert(meta.naif_id, points);
    loaded.ids.insert(meta.naif_id);
    vectors.motion_revision.fetch_add(1, Ordering::Release);
}

/// Writable handles onto the provider's external-data maps — the only way a dataset
/// that arrives after construction becomes visible to `position()`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
struct EphemerisVectors {
    data: Arc<RwLock<HashMap<i32, Vec<CsvDataPoint>>>>,
    parents: Arc<RwLock<HashMap<i32, i32>>>,
    motion_revision: Arc<AtomicU64>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
struct LoadedEphemerisDatasets {
    ids: std::collections::HashSet<i32>,
    owners: HashMap<i32, String>,
    prior_parents: HashMap<i32, Option<i32>>,
    prior_data: HashMap<i32, Option<Vec<CsvDataPoint>>>,
}

#[cfg(not(target_arch = "wasm32"))]
fn clear_scene_ephemeris_datasets(
    mut loaded: ResMut<LoadedEphemerisDatasets>,
    vectors: Res<EphemerisVectors>,
) {
    if loaded.ids.is_empty() {
        return;
    }
    let mut data = vectors
        .data
        .write()
        .unwrap_or_else(|error| error.into_inner());
    let mut parents = vectors
        .parents
        .write()
        .unwrap_or_else(|error| error.into_inner());
    let ids = std::mem::take(&mut loaded.ids);
    for id in ids {
        match loaded.prior_data.remove(&id).flatten() {
            Some(points) => {
                data.insert(id, points);
            }
            None => {
                data.remove(&id);
            }
        }
        loaded.owners.remove(&id);
        match loaded.prior_parents.remove(&id).flatten() {
            Some(parent) => {
                parents.insert(id, parent);
            }
            None => {
                parents.remove(&id);
            }
        }
    }
    vectors.motion_revision.fetch_add(1, Ordering::Release);
}
