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
//! Apps that need real planetary positions add [`EphemerisPlugin`],
//! which overwrites the `EphemerisResource` installed by
//! `lunco_celestial::CelestialPlugin`.

use bevy::prelude::*;
use bevy::math::DVec3;
use lunco_celestial::frames::{EclipticAu, IcrfAu};
use celestial_time::TDB;
use celestial_time::julian::JulianDate;
use celestial_ephemeris::{Vsop2013Earth, Vsop2013Sun, planets::Vsop2013Emb, moon::ElpMpp02Moon};
use celestial_core::Vector3;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use lunco_assets::ephemeris_path_for_target;
use lunco_celestial::ephemeris::{
    CsvDataPoint, EphemerisProvider, EphemerisResource, MissionConfig,
};

/// Concrete implementation of the hybrid [`EphemerisProvider`].
///
/// Combines built-in analytical VSOP/ELP modules with a local cache of
/// external mission data (JPL Horizons CSV).
pub struct CelestialEphemerisProvider {
    _sun: Vsop2013Sun,
    earth: Vsop2013Earth,
    emb: Vsop2013Emb,
    moon: ElpMpp02Moon,
    // `Arc<RwLock>` so the background JPL-Horizons fetch (kicked off by
    // `EphemerisPlugin`) can insert mission vectors after launch without
    // blocking app startup. Reads on the (hot) `position` path take an
    // uncontended read lock.
    custom_data: Arc<RwLock<HashMap<i32, Vec<CsvDataPoint>>>>,
    /// body → parent, THE gravitational hierarchy. Read from `BodyRegistry` (the registry's
    /// `BodyDescriptor::parent_id` is the single source of truth) plus each mission's own
    /// declared `center`.
    ///
    /// P8(c): this used to be a `match` hardcoded in `EphemerisProvider::global_position`,
    /// duplicating the registry — and the two had already drifted apart (the `match` knew
    /// mission id `-1024`; the registry did not). Two descriptions of the shape of the solar
    /// system is one too many.
    parents: Arc<RwLock<HashMap<i32, i32>>>,
}

const AU_KM: f64 = 149_597_870.7;

/// A JPL Horizons `CENTER` (`"@399"`, `"500@399"`, `"399"`) → NAIF id.
///
/// A mission's own config already says what it orbits, so the mission half of the tree comes
/// from the mission — not from a `match` arm someone has to remember to add. (The old hardcoded
/// tree knew exactly ONE mission id, `-1024`. The second mission would have rendered at the
/// Sun.)
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

/// Parse JPL-Horizons CSV vector text into sorted [`CsvDataPoint`]s.
/// Lines with `$$` markers and blanks are skipped. Column layout:
/// `jd, calendar, x, y, z, ...` (so x/y/z are indices 2/3/4).
fn parse_ephemeris_csv(text: &str) -> Vec<CsvDataPoint> {
    let mut points = Vec::new();
    for line in text.lines() {
        if line.contains("$$") || line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() >= 5 {
            if let (Ok(jd), Ok(x), Ok(y), Ok(z)) = (
                parts[0].trim().parse::<f64>(),
                parts[2].trim().parse::<f64>(),
                parts[3].trim().parse::<f64>(),
                parts[4].trim().parse::<f64>(),
            ) {
                points.push(CsvDataPoint {
                    jd,
                    // ASSERTED ecliptic, because the mission JSON asked JPL for
                    // `REF_PLANE=ECLIPTIC`. Still UNVALIDATED: a mission asking for `FRAME`
                    // would re-introduce the Shackleton bug for that one body, silently. The
                    // newtype makes the downstream plumbing safe; it cannot check what JPL
                    // was asked for.
                    pos_au: EclipticAu::new(DVec3::new(x / AU_KM, y / AU_KM, z / AU_KM)),
                });
            }
        }
    }
    points.sort_by(|a, b| a.jd.partial_cmp(&b.jd).unwrap_or(std::cmp::Ordering::Equal));
    points
}

/// A mission ephemeris source whose CSV cache is missing and must be
/// fetched from JPL Horizons. Fetching is deferred to a background task
/// (see [`EphemerisPlugin`]) so a slow / unreachable JPL endpoint can't
/// stall app launch.
struct PendingFetch {
    target_id: i32,
    url: String,
    csv_path: PathBuf,
}

impl CelestialEphemerisProvider {
    /// Construct from local mission caches only (no network). Equivalent
    /// to [`load_local`](Self::load_local) but discards the missing-source
    /// list — used by `Default` and callers that don't want background
    /// fetching.
    pub fn new() -> Self {
        Self::load_local().0
    }

    /// Discover missions in `assets/missions`, load any ephemeris CSV
    /// caches already on disk, and return the provider plus the list of
    /// sources whose cache is missing. Performs **no network I/O** — the
    /// (potentially slow / hanging) JPL Horizons fetches are done off the
    /// main thread by [`EphemerisPlugin`]. This is the H1 launch-stall fix:
    /// `build()` no longer blocks on `ureq` while constructing the app.
    // `disallowed_methods` bans `std::fs` for its wasm failure mode. Unreachable
    // here: the `read_dir` below returns `Err` on wasm, so the loop that owns
    // every `read_to_string` never runs. The web target does not use this path at
    // all — it constructs the provider via `new_with_embedded_ephemeris`, which
    // takes the CSVs as data instead of reading them off a disk it does not have.
    #[allow(clippy::disallowed_methods)]
    fn load_local() -> (Self, Vec<PendingFetch>) {
        let mut custom_data: HashMap<i32, Vec<CsvDataPoint>> = HashMap::new();
        // The tree comes from the registry; missions add their own declared centre below.
        let mut parents: HashMap<i32, i32> = parents_from_registry();
        let mut pending = Vec::new();
        let missions_dir = lunco_assets::assets_dir().join("missions");

        if let Ok(entries) = std::fs::read_dir(missions_dir) {
            for entry in entries.flatten() {
                if !entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(entry.path()) else { continue };
                let Ok(config) = serde_json::from_str::<MissionConfig>(&content) else { continue };
                let Some(sources) = config.ephemeris_sources else { continue };
                for src in sources {
                    let safe_start = src.start_time.replace(' ', "_").replace(':', "");
                    let safe_stop = src.stop_time.replace(' ', "_").replace(':', "");
                    let csv_path = ephemeris_path_for_target(
                        &src.target_id.to_string(),
                        &safe_start,
                        &safe_stop,
                    );

                    if std::path::Path::new(&csv_path).exists() {
                        if let Ok(text) = std::fs::read_to_string(&csv_path) {
                            custom_data.insert(src.target_id, parse_ephemeris_csv(&text));
                            match parse_center(&src.center) {
                                Some(parent) => { parents.insert(src.target_id, parent); }
                                None => warn!(
                                    "[ephemeris] mission {} has an unparseable center '{}' — it \
                                     will be treated as heliocentric, which is almost certainly \
                                     not what you meant",
                                    src.target_id, src.center
                                ),
                            }
                        }
                    } else {
                        // Queue for background fetch instead of blocking here.
                        // CQ-303: percent-encode each interpolated value per
                        // RFC 3986 (escape everything but the unreserved set).
                        // The prior `.replace(' ', "%20")` only handled spaces,
                        // so `& # + ,` in a mission value silently corrupted the
                        // query. The single quotes stay literal — they're part
                        // of the Horizons query syntax; only the user value
                        // inside each pair is encoded.
                        let enc = |s: &str| -> String {
                            let mut out = String::with_capacity(s.len());
                            for &b in s.as_bytes() {
                                match b {
                                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
                                    | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
                                    _ => out.push_str(&format!("%{b:02X}")),
                                }
                            }
                            out
                        };
                        let url = format!(
                            "https://ssd.jpl.nasa.gov/api/horizons.api?format=text&COMMAND='{}'&OBJ_DATA='NO'&MAKE_EPHEM='YES'&EPHEM_TYPE='VECTORS'&CENTER='{}'&REF_PLANE='{}'&START_TIME='{}'&STOP_TIME='{}'&STEP_SIZE='{}'&CSV_FORMAT='YES'",
                            enc(&src.command),
                            enc(&src.center),
                            enc(&src.ref_plane),
                            enc(&src.start_time),
                            enc(&src.stop_time),
                            enc(&src.step_size),
                        );
                        pending.push(PendingFetch {
                            target_id: src.target_id,
                            url,
                            csv_path,
                        });
                    }
                }
            }
        }

        (
            Self {
                _sun: Vsop2013Sun,
                earth: Vsop2013Earth::new(),
                emb: Vsop2013Emb,
                moon: ElpMpp02Moon::new(),
                custom_data: Arc::new(RwLock::new(custom_data)),
                parents: Arc::new(RwLock::new(parents)),
            },
            pending,
        )
    }

    /// Wasm32 constructor that accepts embedded ephemeris CSV data.
    pub fn new_with_embedded_ephemeris(ephemeris_csvs: &[(&str, &str)]) -> Self {
        let mut custom_data = std::collections::HashMap::new();
        let parents = parents_from_registry();
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
                            points.push(CsvDataPoint {
                                jd,
                                pos_au: EclipticAu::new(DVec3::new(x / AU_KM, y / AU_KM, z / AU_KM)),
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
            parents: Arc::new(RwLock::new(parents)),
            custom_data: Arc::new(RwLock::new(custom_data)),
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
/// The `celestial-ephemeris` VSOP2013 wrappers (`heliocentric_position`) and
/// the ELP/MPP02 moon (`geocentric_position_icrs`) all return ICRS/equatorial
/// axes, while the `EphemerisProvider` contract — and everything downstream
/// (`ecliptic_to_bevy`, the geodesy in `lunco_celestial::geo`, and
/// `BodyDescriptor::polar_axis`, which maps the IAU/WGCCRE pole out of the ICRF
/// into exactly this frame) — is ecliptic J2000. Feeding equatorial vectors through
/// unconverted tilts every "up"/"north" by 23.4°: measured at the Shackleton
/// site anchor this rendered the sun ~45° below the horizon (pitch-black
/// ground) instead of the real grazing ~1°.
///
/// **It is now typed**, and that is the fix that outlives the incident: it takes an [`IcrfAu`]
/// and returns an [`EclipticAu`], so it is the ONLY way to produce the frame the
/// `EphemerisProvider` contract promises. A raw `DVec3` from VSOP/ELP cannot skip it, and the
/// geodesy downstream will not accept anything else. The bug is no longer a thing you can write.
///
/// The obliquity comes from `lunco_celestial::iau::OBLIQUITY_J2000_DEG` — the same constant the
/// IAU pole transform uses. It used to be a second literal here, with a comment in `iau.rs`
/// begging the two to agree; if they ever drifted, every "north" in the sim would be wrong by
/// the difference, silently.
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
    /// P8(d) for the built-in bodies: an evaluation error is `None` — "we do not
    /// know" — never a zero vector, which is a *position* (the frame origin) and
    /// indistinguishable from a real result.
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
    /// P8(c): read from the tree, which is the registry's — not a `match` that duplicates it.
    fn parent_id(&self, body_id: i32) -> Option<i32> {
        self.parents.read().ok()?.get(&body_id).copied()
    }

    fn position(&self, body_id: i32, epoch_jd: f64) -> Option<EclipticAu> {
        let julian = JulianDate::new(epoch_jd, 0.0);
        let tdb = TDB::from_julian_date(julian);

        match body_id {
            10 => Some(EclipticAu::ZERO), // the Sun IS the origin of this frame
            3 => {
                let p = self.emb_heliocentric(&tdb)?;
                Some(equatorial_to_ecliptic(IcrfAu::new(DVec3::new(p.x, p.y, p.z))))
            }
            399 => {
                let p_emb = self.emb_heliocentric(&tdb)?;
                let p_earth = self.earth_heliocentric(&tdb)?;
                Some(equatorial_to_ecliptic(IcrfAu::new(DVec3::new(
                    p_earth.x - p_emb.x,
                    p_earth.y - p_emb.y,
                    p_earth.z - p_emb.z,
                ))))
            }
            301 => {
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
                // Uncontended read lock; the background fetch only takes a
                // write lock briefly when a CSV finishes downloading.
                let guard = self.custom_data.read().unwrap_or_else(|e| e.into_inner());
                if let Some(data) = guard.get(&other_id) {
                    if !data.is_empty() {
                        if epoch_jd <= data.first().unwrap().jd { return Some(data.first().unwrap().pos_au); }
                        if epoch_jd >= data.last().unwrap().jd { return Some(data.last().unwrap().pos_au); }
                        let idx = data.partition_point(|p| p.jd <= epoch_jd);
                        if idx > 0 && idx < data.len() {
                            let p0 = &data[idx - 1];
                            let p1 = &data[idx];
                            let t = (epoch_jd - p0.jd) / (p1.jd - p0.jd);
                            return Some(p0.pos_au.lerp(p1.pos_au, t));
                        }
                    }
                }
                // P8(d): an unknown id — or a body whose CSV failed to fetch — lands HERE, at
                // the parent's centre, indistinguishable from a valid position. A failed
                // Mars fetch renders Mars inside the Sun and nothing says so. Making this an
                // `Option<EclipticAu>` is the right fix and is now CHEAP (the type is already
                // threaded); it forces all ~22 call sites to decide what "no ephemeris" means,
                // P8(d) FIXED. This used to return ZERO — a *position* — so a body whose CSV
                // failed to fetch rendered at its parent's centre, indistinguishable from a
                // real result. `None` says what is actually true: we do not know. Callers now
                // skip the body rather than drawing it inside the Sun.
                bevy::log::warn_once!(
                    "[ephemeris] no data for NAIF id {body_id} — it will not be placed. \
                     (Previously it was drawn at its parent's centre, silently.)"
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use lunco_celestial::{solar_tangent_frame, CelestialBodyRegistry, Geodetic};

    /// The REAL conversion, not a copy of it.
    ///
    /// This used to be a hand-rolled `fn ecl_to_bevy` here — a second implementation of
    /// `lunco_celestial::coords::ecliptic_to_bevy`, written only because that one was
    /// `pub(crate)` and therefore unreachable from this crate. A conversion people have to copy
    /// is a conversion that drifts, and this pair is the one whose drift once put the sun 45°
    /// below the horizon. `coords` is now `pub`, so the test exercises the same code the
    /// product does.
    use lunco_celestial::coords::ecliptic_to_bevy;

    /// End-to-end frame check: with the provider's equatorial→ecliptic
    /// conversion and the tilt-aware geodesy, the sun's elevation at the
    /// Shackleton site must stay GRAZING (bounded by the moon axis tilt +
    /// site colatitude, ~±2.5°) and must actually rise above +1° at some
    /// epoch within a year. Both fail loudly under the historical frame
    /// bugs (equatorial vectors fed to ecliptic geodesy put the sun ±23-45°
    /// off the horizon).
    #[test]
    fn shackleton_sun_stays_grazing_and_gets_lit_epochs() {
        let provider = CelestialEphemerisProvider::new();
        let registry = CelestialBodyRegistry::default_system();
        let moon = registry.bodies.iter().find(|b| b.ephemeris_id == 301).unwrap();
        let site = Geodetic::new(-89.45, -136.7, 1200.0);

        let mut best = (0.0_f64, f64::MIN);
        for step in 0..=(366 * 4) {
            let jd = 2461228.5 + step as f64 * 0.25; // 6 h steps from 2026-07-07
            let p_moon = provider.global_position(301, jd).expect("VSOP/ELP always have the Moon");
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
        println!("best lit epoch: JD {:.2} (elevation {:.3}°)", best.0, best.1);
        assert!(
            best.1 > 1.0,
            "Shackleton should reach >1° sun elevation within a year; best {:.3}°",
            best.1
        );
    }

    /// **P2 regression — the Moon's near side must actually face Earth.**
    ///
    /// The test above cannot see the bug that shipped: it checks Shackleton's
    /// *elevation*, and at a pole elevation is **longitude-insensitive**. So a
    /// rotation model with the correct RATE and NO PHASE (`W₀` absent — exactly
    /// what this codebase had) passes it while the whole Moon sits 38.3° out of
    /// true, ~1160 km of surface at the equator.
    ///
    /// This is the longitude-SENSITIVE check. The Moon is tidally locked, so the
    /// **sub-Earth point** — where Earth is at the lunar zenith — must stay near
    /// lunar longitude 0 forever. Optical libration in longitude swings it ±8°
    /// (the orbit is eccentric, the spin is uniform), so bound it at 10°.
    ///
    /// Under the old model this reads ≈ 38° at J2000 and wanders — a hard fail.
    #[test]
    fn moon_near_side_faces_earth_across_epochs() {
        use lunco_celestial::{body_fixed_to_geodetic, body_rotation};

        let provider = CelestialEphemerisProvider::new();
        let registry = CelestialBodyRegistry::default_system();
        let moon = registry.bodies.iter().find(|b| b.ephemeris_id == 301).unwrap();

        let mut worst = (0.0_f64, 0.0_f64);
        // ~14 months at 11-day steps: samples every phase of the libration cycle.
        for step in 0..40 {
            let jd = 2_451_545.0 + step as f64 * 11.0;

            // Earth as seen from the Moon, in the engine (ecliptic-Bevy) frame.
            let to_earth = ecliptic_to_bevy(
                provider.global_position(399, jd).expect("Earth")
                    - provider.global_position(301, jd).expect("Moon"),
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
        println!("worst sub-Earth longitude: {:.2}° at JD {:.1}", worst.1, worst.0);

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

/// Drop into an app to replace the NoOp ephemeris provider installed by
/// `CelestialPlugin` with the full VSOP/ELP/JPL implementation.
///
/// ```ignore
/// app.add_plugins(lunco_celestial::CelestialPlugin)
///    .add_plugins(lunco_celestial_ephemeris::EphemerisPlugin);
/// ```
pub struct EphemerisPlugin;

impl Plugin for EphemerisPlugin {
    fn build(&self, app: &mut App) {
        // Build from local caches only — never block app launch on the
        // network (H1). Missing mission CSVs are fetched off-thread.
        let (provider, _pending) = CelestialEphemerisProvider::load_local();

        #[cfg(not(target_arch = "wasm32"))]
        {
            app.insert_resource(EphemerisDownloads {
                data: provider.custom_data.clone(),
                missing: _pending,
                requested: false,
                tasks: Vec::new(),
            });
            // NO startup fetch. Launching the app must not open a network
            // connection on its own: the missing datasets are CATALOGUED here
            // and downloaded only when the user asks (Settings ▸ Ephemeris
            // data ▸ Download). `start_requested_ephemeris_fetches` is the sole
            // place a request turns into traffic.
            app.add_systems(
                Update,
                (start_requested_ephemeris_fetches, poll_ephemeris_fetches),
            );
        }

        app.insert_resource(EphemerisResource {
            provider: Arc::new(provider),
        });
    }
}

/// Downloadable ephemeris datasets and their fetch state.
///
/// `missing` is the CATALOGUE: mission ephemeris sources declared in
/// `assets/missions/*.json` whose CSV cache is not on disk. Nothing is fetched
/// until [`request_download`](Self::request_download) is called — the app never
/// reaches the network on its own. `data` aliases the live provider's
/// `custom_data`, so vectors downloaded mid-session become available to
/// `position()` immediately (no restart).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
pub struct EphemerisDownloads {
    data: Arc<RwLock<HashMap<i32, Vec<CsvDataPoint>>>>,
    missing: Vec<PendingFetch>,
    requested: bool,
    tasks: Vec<(i32, PathBuf, bevy::tasks::Task<Option<String>>)>,
}

#[cfg(not(target_arch = "wasm32"))]
impl EphemerisDownloads {
    /// NAIF ids of the mission datasets that are declared but not cached.
    pub fn missing_ids(&self) -> Vec<i32> {
        self.missing.iter().map(|p| p.target_id).collect()
    }

    /// How many declared datasets are missing locally.
    pub fn missing_count(&self) -> usize {
        self.missing.len()
    }

    /// How many downloads are currently in flight.
    pub fn in_flight(&self) -> usize {
        self.tasks.len()
    }

    /// Ask for the missing datasets to be downloaded from JPL Horizons.
    ///
    /// This is the ONLY thing that authorises network traffic in this crate;
    /// call it from an explicit user action (a Settings-menu button), never
    /// from startup or scene load.
    pub fn request_download(&mut self) {
        self.requested = true;
    }
}

/// Blocking JPL-Horizons fetch with a hard timeout, run on a task-pool
/// thread. Returns the cleaned CSV body (between `$$SOE`/`$$EOE`), or
/// `None` on any network / timeout / parse-boundary failure.
#[cfg(not(target_arch = "wasm32"))]
fn fetch_horizons(url: &str) -> Option<String> {
    let resp = ureq::get(url)
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .build()
        .call()
        .ok()?;
    let text = resp.into_body().read_to_string().ok()?;
    let start = text.find("$$SOE")?;
    let end = text.find("$$EOE")?;
    Some(text[start..end].replace("$$SOE", "").replace("$$EOE", ""))
}

/// Spawn one async fetch per missing mission CSV — but only after the user
/// asked for it via [`EphemerisDownloads::request_download`]. Cheap no-op
/// otherwise, which is the normal state.
#[cfg(not(target_arch = "wasm32"))]
fn start_requested_ephemeris_fetches(mut fetch: ResMut<EphemerisDownloads>) {
    if !fetch.requested {
        return;
    }
    fetch.requested = false;
    if fetch.missing.is_empty() {
        return;
    }
    let pool = bevy::tasks::AsyncComputeTaskPool::get();
    let pending = std::mem::take(&mut fetch.missing);
    for p in pending {
        info!("[ephemeris] fetching mission vectors for NAIF {} (async)", p.target_id);
        let url = p.url;
        let task = pool.spawn(async move { fetch_horizons(&url) });
        fetch.tasks.push((p.target_id, p.csv_path, task));
    }
}

/// Update: poll outstanding fetches; on success write the CSV cache and
/// insert the parsed vectors into the shared map so `position()` sees
/// them immediately. Cheap no-op once all tasks have drained.
#[cfg(not(target_arch = "wasm32"))]
fn poll_ephemeris_fetches(mut fetch: ResMut<EphemerisDownloads>) {
    use bevy::tasks::{block_on, futures_lite::future};
    if fetch.tasks.is_empty() {
        return;
    }
    let data = fetch.data.clone();
    let tasks = std::mem::take(&mut fetch.tasks);
    let mut still_pending = Vec::new();
    for (target_id, csv_path, mut task) in tasks {
        match block_on(future::poll_once(&mut task)) {
            None => still_pending.push((target_id, csv_path, task)),
            Some(Some(clean_csv)) => {
                // Best-effort cache write: a failure here only means the next
                // launch re-fetches, so it's non-fatal — but it should not be
                // invisible.
                if let Err(e) = lunco_storage::write_file_sync(&csv_path, clean_csv.as_bytes()) {
                    warn!("[ephemeris] could not write CSV cache {}: {e}", csv_path.display());
                }
                let points = parse_ephemeris_csv(&clean_csv);
                if points.is_empty() {
                    warn!("[ephemeris] NAIF {} fetch returned no usable vectors", target_id);
                } else {
                    let n = points.len();
                    data.write().unwrap_or_else(|e| e.into_inner()).insert(target_id, points);
                    info!("[ephemeris] loaded {n} mission vectors for NAIF {target_id}");
                }
            }
            Some(None) => warn!("[ephemeris] fetch failed for NAIF {target_id}"),
        }
    }
    fetch.tasks = still_pending;
}
