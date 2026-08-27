//! Embedded assets for wasm32 builds.
//!
//! Mission data is baked into the binary at compile time. Textures are NOT —
//! Earth/Moon are tens of MB, so they load from declared lunco texture
//! resources over HTTP on web (see big_space_setup), not from the binary.
//!
//! This module embeds no shaders or textures. Live shaders are named by path in
//! a `ShaderLook` and loaded by `lunco-render-bevy`.
//!
//! On desktop, this plugin is a no-op — assets load normally from disk.

use bevy::prelude::*;

// ============================================================================
// Embedded Missions
// ============================================================================

// Mission JSON is owned by the asset crate — `lunco_assets::missions` embeds
// `assets/missions/` and hands it over by basename (see `build` below), so this
// crate holds no direct path into the shared asset tree.

// ============================================================================
// Embedded Ephemeris Data (wasm32)
// ============================================================================

#[cfg(all(target_arch = "wasm32", feature = "embed-assets"))]
const ARTEMIS_2_EPHEMERIS_CSV: &str =
    include_str!("../../../../.cache/ephemeris/target_-1024_2026-04-02_0159_2026-04-11_0001.csv");

// ============================================================================
// Embedded Assets Plugin
// ============================================================================

/// Registers the embedded mission/ephemeris data into the asset server.
/// On desktop this is a no-op; on wasm32 it's the only way to get assets.
pub struct EmbeddedAssetsPlugin;

impl Plugin for EmbeddedAssetsPlugin {
    #[allow(unused_variables)]
    fn build(&self, app: &mut App) {
        #[cfg(all(target_arch = "wasm32", feature = "embed-assets"))]
        {
            // Only the EPHEMERIS payload is embedded now. The mission's own
            // definition (trajectories, spacecraft, colours, sampling) is USD and
            // arrives through the ordinary stage-composition path like any other
            // scene content — a scene that references the mission file gets it on
            // wasm exactly as on desktop, so there is nothing mission-shaped left
            // to bake in here.
            app.insert_resource(EmbeddedMissionData {
                artemis_2_ephemeris_csv: ARTEMIS_2_EPHEMERIS_CSV.to_string(),
            });

            // Real ephemeris provider (VSOP2013 + embedded CSV) lives
            // in `lunco-celestial-ephemeris`. Apps that want it on
            // wasm32 add that crate's `EphemerisPlugin` after
            // `EmbeddedAssetsPlugin` — the plugin reads
            // `EmbeddedMissionData::artemis_2_ephemeris_csv` (set above)
            // and installs the explicit `EphemerisResource`.
        }
    }
}

/// Holds the embedded ephemeris payload (wasm32 only).
///
/// This carries EPHEMERIS only — the numbers. The mission *definition* moved to
/// USD (`assets/missions/artemis_2_mission.usda`) and loads through stage
/// composition on every platform, so there is no embedded mission JSON.
#[derive(Resource)]
pub struct EmbeddedMissionData {
    /// Embedded ephemeris CSV for Artemis 2 (target ID -1024).
    /// Format: JD, Date, X, Y, Z, VX, VY, VZ, LT, Range, RangeRate
    pub artemis_2_ephemeris_csv: String,
}
