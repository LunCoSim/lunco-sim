//! Embedded assets for wasm32 builds.
//!
//! Shaders and mission data are baked into the binary at compile time. Textures
//! are NOT — Earth/Moon are tens of MB, so they load from `cached_textures://`
//! over HTTP on web (see `big_space_setup`), not `include_bytes!`.
//!
//! On desktop, these are ignored — assets load normally from disk.

use bevy::prelude::*;
use bevy_shader::Shader;
#[cfg(all(target_arch = "wasm32", feature = "embed-assets"))]
use bevy_asset::load_internal_asset;
use std::marker::PhantomData;
use uuid::Uuid;

#[cfg(all(target_arch = "wasm32", feature = "embed-assets"))]
use lunco_materials::BLUEPRINT_SHADER_HANDLE;

// ============================================================================
// Embedded Shaders
// Shader source is embedded at compile time from root assets/ folder.
// Registered with known UUID handles at runtime so MaterialExtension can resolve them.
// ============================================================================

/// UUID for the trajectory shader — must match what `TrajectoryExtension::fragment_shader()` returns.
const TRAJECTORY_SHADER_UUID: Uuid = Uuid::from_u128(0x2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e);

/// Const UUID-based handle for the trajectory shader.
pub const TRAJECTORY_SHADER_HANDLE: Handle<Shader> = Handle::Uuid(TRAJECTORY_SHADER_UUID, PhantomData);

// ============================================================================
// Embedded Missions
// ============================================================================

#[cfg(all(target_arch = "wasm32", feature = "embed-assets"))]
const ARTEMIS_2_JSON: &str = include_str!("../../../assets/missions/artemis-2.json");

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
            // Register shaders with const UUID handles so MaterialExtension can resolve them
            load_internal_asset!(
                app,
                BLUEPRINT_SHADER_HANDLE,
                "../../../assets/shaders/blueprint_extension.wgsl",
                Shader::from_wgsl
            );
            load_internal_asset!(
                app,
                TRAJECTORY_SHADER_HANDLE,
                "../../../assets/shaders/trajectory.wgsl",
                Shader::from_wgsl
            );

            // Register mission data
            app.insert_resource(EmbeddedMissionData {
                artemis_2: ARTEMIS_2_JSON.to_string(),
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
