//! Embedded assets for wasm32 builds.
//!
//! Shaders, textures, and mission data are baked into the binary at compile time
//! so the web build doesn't need filesystem access or HTTP asset fetching.
//!
//! On desktop, these are ignored — assets load normally from disk.

use bevy::prelude::*;
use bevy_shader::Shader;
use bevy_asset::{RenderAssetUsages, load_internal_asset};
use bevy::image::{ImageSampler, ImageSamplerDescriptor, ImageAddressMode, ImageFilterMode};
use std::marker::PhantomData;
use uuid::Uuid;

// ============================================================================
// Embedded Shaders
// Shader source is embedded at compile time from root assets/ folder.
// Registered with known UUID handles at runtime so MaterialExtension can resolve them.
// ============================================================================

/// UUID for the blueprint shader — must match what `BlueprintExtension::fragment_shader()` returns.
const BLUEPRINT_SHADER_UUID: Uuid = Uuid::from_u128(0x1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d);
/// UUID for the trajectory shader — must match what `TrajectoryExtension::fragment_shader()` returns.
const TRAJECTORY_SHADER_UUID: Uuid = Uuid::from_u128(0x2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e);

/// Const UUID-based handle for the blueprint shader.
pub const BLUEPRINT_SHADER_HANDLE: Handle<Shader> = Handle::Uuid(BLUEPRINT_SHADER_UUID, PhantomData);
/// Const UUID-based handle for the trajectory shader.
pub const TRAJECTORY_SHADER_HANDLE: Handle<Shader> = Handle::Uuid(TRAJECTORY_SHADER_UUID, PhantomData);

// ============================================================================
// Embedded Textures (Earth & Moon)
// Use JPEG versions for smaller binary size (PNG is 55MB total, JPEG is 5MB)
// Moon uses a solid grey fallback since no JPEG source is available.
// ============================================================================

const EARTH_JPG: &[u8] = include_bytes!("../../../.cache/textures/earth_source.jpg");

// ============================================================================
// Embedded Missions
// ============================================================================

const ARTEMIS_2_JSON: &str = include_str!("../../../assets/missions/artemis-2.json");

// ============================================================================
// Embedded Ephemeris Data (wasm32)
// ============================================================================

const ARTEMIS_2_EPHEMERIS_CSV: &str = include_str!("../../../.cache/ephemeris/target_-1024_2026-04-02_0159_2026-04-11_0001.csv");

// ============================================================================
// Embedded Assets Plugin
// ============================================================================

/// Registers all embedded assets (shaders, textures, missions) into the asset server.
/// On desktop this is a no-op; on wasm32 it's the only way to get assets.
pub struct EmbeddedAssetsPlugin;

impl Plugin for EmbeddedAssetsPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(target_arch = "wasm32")]
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

            // Register textures (Earth only; Moon uses solid grey fallback)
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            let earth_handle = images.add(load_image_from_bytes(EARTH_JPG).unwrap());
            app.insert_resource(EmbeddedEarthTexture(earth_handle));
            app.insert_resource(EmbeddedMoonTexture(None));

            // Register mission data
            app.insert_resource(EmbeddedMissionData {
                artemis_2: ARTEMIS_2_JSON.to_string(),
                artemis_2_ephemeris_csv: ARTEMIS_2_EPHEMERIS_CSV.to_string(),
            });

            // Initialize EphemerisResource with embedded CSV data
            // Target ID -1024 = Artemis 2 spacecraft
            let provider = crate::ephemeris::CelestialEphemerisProvider::new_with_embedded_ephemeris(&[
                ("-1024", ARTEMIS_2_EPHEMERIS_CSV),
            ]);
            app.insert_resource(crate::ephemeris::EphemerisResource {
                provider: std::sync::Arc::new(provider),
            });
        }
    }
}

/// Holds the embedded Earth texture handle (wasm32 only).
#[derive(Resource)]
pub struct EmbeddedEarthTexture(pub Handle<Image>);

/// Holds the embedded Moon texture handle (wasm32 only).
/// Currently not embedded due to binary size constraints — Moon uses a solid grey fallback.
#[derive(Resource)]
pub struct EmbeddedMoonTexture(pub Option<Handle<Image>>);

/// Holds embedded mission JSON data (wasm32 only).
#[derive(Resource)]
pub struct EmbeddedMissionData {
    pub artemis_2: String,
    /// Embedded ephemeris CSV for Artemis 2 (target ID -1024).
    /// Format: JD, Date, X, Y, Z, VX, VY, VZ, LT, Range, RangeRate
    pub artemis_2_ephemeris_csv: String,
}

fn load_image_from_bytes(bytes: &[u8]) -> Result<Image, image::ImageError> {
    let img = image::load_from_memory(bytes)?;
    let mut image = Image::from_dynamic(img, true, RenderAssetUsages::RENDER_WORLD);
    // Use Linear mipmap filtering to reduce aliasing/jitter at distance
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mipmap_filter: ImageFilterMode::Linear,
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::ClampToEdge,
        ..default()
    });
    Ok(image)
}
