//! Embedded assets for wasm32 builds.
//!
//! Mission data is baked into the binary at compile time. Textures are NOT —
//! Earth/Moon are tens of MB, so they load from `cached_textures://` over HTTP on
//! web (see `big_space_setup`), not `include_bytes!`.
//!
//! **No shaders are embedded here any more.** The only one ever was
//! `trajectory.wgsl`, held by a const `Handle<Shader>` for a `MaterialExtension`
//! that was never instantiated (see the removal note in `trajectories.rs`).
//! `Handle<Shader>` is `bevy_shader`, which pulls naga, so holding one made this
//! crate — and every binary linking it, `--no-ui` server included — link the GPU
//! stack for a dead asset. Live shaders are named by PATH in a `ShaderLook` and
//! loaded by `lunco-render-bevy`.
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
const ARTEMIS_2_EPHEMERIS_CSV: &str = include_str!("../../../../.cache/ephemeris/target_-1024_2026-04-02_0159_2026-04-11_0001.csv");

// ============================================================================
// Embedded Assets Plugin
// ============================================================================

/// Registers all embedded assets (shaders, textures, missions) into the asset server.
/// On desktop this is a no-op; on wasm32 it's the only way to get assets.
pub struct EmbeddedAssetsPlugin;

impl Plugin for EmbeddedAssetsPlugin {
    #[allow(unused_variables)]
    fn build(&self, app: &mut App) {
        #[cfg(all(target_arch = "wasm32", feature = "embed-assets"))]
        {
            // Register mission data (JSON via the asset-owning crate).
            let artemis_2 = lunco_assets::missions::mission_source("artemis-2.json")
                .expect("artemis-2.json must be embedded in assets/missions/")
                .to_string();
            app.insert_resource(EmbeddedMissionData {
                artemis_2,
                artemis_2_ephemeris_csv: ARTEMIS_2_EPHEMERIS_CSV.to_string(),
            });

            // Real ephemeris provider (VSOP2013 + embedded CSV) lives
            // in `lunco-celestial-ephemeris`. Apps that want it on
            // wasm32 add that crate's `EphemerisPlugin` after
            // `EmbeddedAssetsPlugin` — the plugin reads
            // `EmbeddedMissionData::artemis_2_ephemeris_csv` (set above)
            // and overwrites the default no-op `EphemerisResource`.
        }
    }
}

/// Holds embedded mission JSON data (wasm32 only).
#[derive(Resource)]
pub struct EmbeddedMissionData {
    pub artemis_2: String,
    /// Embedded ephemeris CSV for Artemis 2 (target ID -1024).
    /// Format: JD, Date, X, Y, Z, VX, VY, VZ, LT, Range, RangeRate
    pub artemis_2_ephemeris_csv: String,
}
