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
}

const AU_KM: f64 = 149_597_870.7;

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
                    pos_au: DVec3::new(x / AU_KM, y / AU_KM, z / AU_KM),
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
    fn load_local() -> (Self, Vec<PendingFetch>) {
        let mut custom_data: HashMap<i32, Vec<CsvDataPoint>> = HashMap::new();
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
            },
            pending,
        )
    }

    /// Wasm32 constructor that accepts embedded ephemeris CSV data.
    pub fn new_with_embedded_ephemeris(ephemeris_csvs: &[(&str, &str)]) -> Self {
        let mut custom_data = std::collections::HashMap::new();
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
            custom_data: Arc::new(RwLock::new(custom_data)),
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
            10 => DVec3::ZERO,
            3 => {
                let p = self.emb.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                DVec3::new(p.x, p.y, p.z)
            }
            399 => {
                let p_emb = self.emb.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                let p_earth = self.earth.heliocentric_position(&tdb).unwrap_or_else(|_| Vector3::zeros());
                DVec3::new(p_earth.x - p_emb.x, p_earth.y - p_emb.y, p_earth.z - p_emb.z)
            }
            301 => {
                let p_m_geo_arr = self.moon.geocentric_position_icrs(&tdb).unwrap_or_else(|_| [0.0, 0.0, 0.0]);
                const AU_KM: f64 = 149_597_870.7;
                let mut p_m_geo_au = DVec3::new(p_m_geo_arr[0] / AU_KM, p_m_geo_arr[1] / AU_KM, p_m_geo_arr[2] / AU_KM);

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
            }
            other_id => {
                // Uncontended read lock; the background fetch only takes a
                // write lock briefly when a CSV finishes downloading.
                let guard = self.custom_data.read().unwrap_or_else(|e| e.into_inner());
                if let Some(data) = guard.get(&other_id) {
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
            }
        }
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
            app.insert_resource(EphemerisFetch {
                data: provider.custom_data.clone(),
                pending: _pending,
                tasks: Vec::new(),
            });
            app.add_systems(Startup, spawn_ephemeris_fetches);
            app.add_systems(Update, poll_ephemeris_fetches);
        }

        app.insert_resource(EphemerisResource {
            provider: Arc::new(provider),
        });
    }
}

/// Background JPL-Horizons fetch state. `data` aliases the live
/// provider's `custom_data`, so vectors fetched after launch become
/// available to `position()` in the same session (no restart needed).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
struct EphemerisFetch {
    data: Arc<RwLock<HashMap<i32, Vec<CsvDataPoint>>>>,
    pending: Vec<PendingFetch>,
    tasks: Vec<(i32, PathBuf, bevy::tasks::Task<Option<String>>)>,
}

/// Blocking JPL-Horizons fetch with a hard timeout, run on a task-pool
/// thread. Returns the cleaned CSV body (between `$$SOE`/`$$EOE`), or
/// `None` on any network / timeout / parse-boundary failure.
#[cfg(not(target_arch = "wasm32"))]
fn fetch_horizons(url: &str) -> Option<String> {
    let resp = ureq::get(url)
        .timeout(std::time::Duration::from_secs(20))
        .call()
        .ok()?;
    let text = resp.into_string().ok()?;
    let start = text.find("$$SOE")?;
    let end = text.find("$$EOE")?;
    Some(text[start..end].replace("$$SOE", "").replace("$$EOE", ""))
}

/// Startup: spawn one async fetch per missing mission CSV. Runs after
/// plugin build so the `AsyncComputeTaskPool` is guaranteed initialized.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_ephemeris_fetches(mut fetch: ResMut<EphemerisFetch>) {
    if fetch.pending.is_empty() {
        return;
    }
    let pool = bevy::tasks::AsyncComputeTaskPool::get();
    let pending = std::mem::take(&mut fetch.pending);
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
fn poll_ephemeris_fetches(mut fetch: ResMut<EphemerisFetch>) {
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
                if let Some(parent) = csv_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&csv_path, &clean_csv);
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
