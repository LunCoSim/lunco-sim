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
use std::sync::{Arc, RwLock};

use lunco_celestial::ephemeris::{
    CsvDataPoint, EphemerisProvider, EphemerisResource,
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

/// The `[<key>.ephemeris]` sub-table of a declared dataset: what the bytes are.
///
/// Transport (`url`, `dest`, `sha256`) is `lunco-assets`' half of the same
/// entry; this is ours. Keeping both in ONE declaration is what removed the old
/// `assets/missions/*.ephemeris.json`, which restated the id and centre beside
/// a second copy of the Horizons query — two files to keep in step, and the
/// startup path that read them was where the app phoned home.
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

/// Parse a downloaded Horizons response into sorted vectors.
///
/// `None` when the file cannot be read or holds no usable rows — a present but
/// unparseable file is reported, never silently treated as "no data".
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::disallowed_methods)]
fn read_vectors(path: &std::path::Path) -> Option<Vec<CsvDataPoint>> {
    let text = std::fs::read_to_string(path).ok()?;
    let points = parse_ephemeris_csv(&text);
    if points.is_empty() {
        warn!(
            "[ephemeris] {} parsed to zero usable vectors — present, but not a Horizons \
             VECTORS response",
            path.display()
        );
        return None;
    }
    Some(points)
}

impl CelestialEphemerisProvider {
    /// Analytic bodies only — VSOP/ELP plus the registry's parent tree.
    ///
    /// Mission vectors are DECLARED datasets and are added by
    /// [`adopt_ephemeris_datasets`] once their files are on disk, so
    /// construction reads no manifests, no JSON, and certainly no network.
    pub fn new() -> Self {
        Self {
            _sun: Vsop2013Sun,
            earth: Vsop2013Earth::new(),
            emb: Vsop2013Emb,
            moon: ElpMpp02Moon::new(),
            custom_data: Arc::new(RwLock::new(HashMap::new())),
            parents: Arc::new(RwLock::new(parents_from_registry())),
        }
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
        // Analytic bodies now; mission datasets when their files exist.
        // Downloading is `lunco-assets`' concern — this crate DECLARES
        // (`Assets.toml`) and REPORTS. It owns no URL, no socket, no task.
        let provider = CelestialEphemerisProvider::new();
        // Handle onto the same maps the provider reads, so a dataset that
        // arrives later reaches `position()` without a restart. The trait is
        // read-only by design (a provider answers questions, it is not a
        // store), so the writable side is held here rather than widened there.
        app.insert_resource(EphemerisVectors {
            data: provider.custom_data.clone(),
            parents: provider.parents.clone(),
        });
        app.insert_resource(EphemerisResource {
            provider: Arc::new(provider),
        });

        #[cfg(not(target_arch = "wasm32"))]
        {
            // Declaring is `DatasetsPlugin`'s job — it scans
            // `assets/manifests/`, where this crate's datasets live as DATA
            // (`ephemeris.toml`). This crate only ADOPTS: whatever is already
            // cached is picked up on the first `Update`, and anything
            // downloaded later on the frame the registry reports it installed.
            // One code path for both.
            app.add_systems(Update, adopt_ephemeris_datasets);
        }
    }
}

/// Adopt every ephemeris dataset whose file is on disk — cached from an earlier
/// run, downloaded a moment ago, or shipped inside an open Twin.
///
/// Everything it needs is in the ONE declaration: `path` (where transport put
/// the bytes) and `[<key>.ephemeris]` (what they are). No directory scan, no
/// filename convention to reverse-engineer, no mission JSON.
///
/// The parent is registered even when the file is absent: it is astronomy, and
/// without it a later download would land in a provider that thinks the
/// spacecraft orbits the Sun.
#[cfg(not(target_arch = "wasm32"))]
fn adopt_ephemeris_datasets(
    registry: Option<Res<lunco_assets::datasets::DatasetRegistry>>,
    vectors: Option<Res<EphemerisVectors>>,
    mut seen: Local<std::collections::HashSet<String>>,
) {
    let (Some(registry), Some(vectors)) = (registry, vectors) else {
        return;
    };
    for entry in registry.entries() {
        let Some(meta) = entry.spec.domain::<EphemerisDatasetMeta>("ephemeris") else {
            continue; // not ours
        };
        let meta = match meta {
            Ok(m) => m,
            Err(e) => {
                // Loud: a typo'd declaration would otherwise mean a body that
                // silently never appears.
                if seen.insert(format!("bad:{}", entry.key)) {
                    error!(
                        "[ephemeris] dataset '{}' has a malformed [ephemeris] table: {e}",
                        entry.key
                    );
                }
                continue;
            }
        };

        if seen.insert(format!("parent:{}", entry.key)) {
            match parse_center(&meta.center) {
                Some(parent) => {
                    vectors
                        .parents
                        .write()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(meta.naif_id, parent);
                }
                None => error!(
                    "[ephemeris] dataset '{}' has an unparseable center '{}' — NAIF {} would \
                     be placed heliocentrically, so it is left out entirely",
                    entry.key, meta.center, meta.naif_id
                ),
            }
        }

        if !entry.state.is_installed() {
            if seen.insert(format!("absent:{}", entry.key)) {
                info!(
                    "[ephemeris] NAIF {} has no cached vectors — download '{}' from \
                     Settings ▸ Downloadable data (nothing is fetched automatically)",
                    meta.naif_id, entry.key
                );
            }
            continue;
        }
        if !seen.insert(format!("loaded:{}", entry.key)) {
            continue;
        }
        let Some(points) = read_vectors(&entry.path) else { continue };
        info!(
            "[ephemeris] loaded {} vectors for NAIF {} from dataset '{}'",
            points.len(),
            meta.naif_id,
            entry.key
        );
        vectors
            .data
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(meta.naif_id, points);
    }
}

/// Writable handles onto the provider's mission maps — the only way a dataset
/// that arrives after construction becomes visible to `position()`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
struct EphemerisVectors {
    data: Arc<RwLock<HashMap<i32, Vec<CsvDataPoint>>>>,
    parents: Arc<RwLock<HashMap<i32, i32>>>,
}

