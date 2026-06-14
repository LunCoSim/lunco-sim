//! UsdLux light prims → Bevy lights.
//!
//! Scene files are the source of truth for lighting; binaries only provide
//! defaults for scenes that author none. Two UsdLux prim types are honoured:
//!
//! - `DistantLight` — the sun. Orientation comes from the prim's
//!   `xformOp:rotateXYZ` via the shared transform path in
//!   `instantiate_usd_prim`: USD distant lights emit along local **-Z**,
//!   the same convention as Bevy's `DirectionalLight`, so no extra
//!   axis-fixup is needed.
//! - `DomeLight` — sky fill. UsdLux deliberately has no "ambient light"
//!   property; a dome is the standard expression of one. Its intensity
//!   drives the `GlobalAmbientLight` resource.
//!
//! ## Fallback policy
//!
//! Binaries tag their built-in default sun with [`FallbackSceneLight`].
//! The moment any scene-authored light instantiates, every fallback light
//! is despawned and the global ambient is recomputed from authored
//! `DomeLight`s only — **no dome ⇒ ambient 0**. An airless-Moon scene
//! authors a single `DistantLight` and nothing else, and gets jet-black
//! shadow cores for free; scenes that author no lights leave the binary's
//! defaults untouched.
//!
//! ## Shadow quality knobs
//!
//! Cascade policy (count, biases, 4096² map) is engine policy, but the two
//! scene-dependent ranges are overridable per light with custom attributes:
//! `lunco:shadow:maxDistance` (default 1500 m) and
//! `lunco:shadow:firstCascadeFarBound` (default 40 m). A scene that wants
//! crisp near-field shadows over a huge terrain authors a shorter
//! `maxDistance` — texel density scales inversely with it.

use bevy::light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap, GlobalAmbientLight};
use bevy::prelude::*;
use openusd::sdf::{AbstractData, Path as SdfPath, Value};
use openusd::usda::TextReader;

/// Tag for a binary's built-in default sun — defined in `lunco-core` (so
/// non-USD crates can tag their lights too), re-exported here where the
/// despawn policy lives. Despawned as soon as the loaded scene authors its
/// own light prim.
pub use lunco_core::FallbackSceneLight;

/// Marker stamped on every entity instantiated from a UsdLux light prim.
/// Its `Add` observer enforces the fallback policy (see module docs).
#[derive(Component)]
pub struct UsdAuthoredLight;

/// Ambient contribution of an authored `DomeLight` prim (its
/// `inputs:intensity`, in `GlobalAmbientLight::brightness` units).
#[derive(Component)]
pub(crate) struct UsdDomeAmbient(pub(crate) f32);

/// Scalar attribute reader tolerant of `float`/`double`/`int` authoring.
pub(crate) fn get_attribute_as_f32(reader: &TextReader, path: &SdfPath, attr: &str) -> Option<f32> {
    let attr_path = path.append_property(attr).ok()?;
    let val = reader.try_get(&attr_path, "default").ok().flatten()?;
    match &*val {
        Value::Float(f) => Some(*f),
        Value::Double(d) => Some(*d as f32),
        Value::Int(i) => Some(*i as f32),
        _ => None,
    }
}

/// Bool attribute reader (also accepts `int` 0/1 authoring).
pub(crate) fn get_attribute_as_bool(
    reader: &TextReader,
    path: &SdfPath,
    attr: &str,
) -> Option<bool> {
    let attr_path = path.append_property(attr).ok()?;
    let val = reader.try_get(&attr_path, "default").ok().flatten()?;
    match &*val {
        Value::Bool(b) => Some(*b),
        Value::Int(i) => Some(*i != 0),
        _ => None,
    }
}

/// If `prim_type` is a supported UsdLux light, attach the corresponding
/// Bevy light components to `entity` and return `true`. Called from
/// `instantiate_usd_prim`; the prim's transform/visibility are applied by
/// the shared path there.
pub(crate) fn instantiate_light_prim(
    reader: &TextReader,
    sdf_path: &SdfPath,
    prim_type: Option<&str>,
    commands: &mut Commands,
    entity: Entity,
) -> bool {
    match prim_type {
        Some("DistantLight") => {
            // UsdLux spec default intensity is 1.0, but 1 lx is invisible
            // under Bevy's physically-based exposure — an unauthored
            // intensity almost certainly means "give me a sun", so default
            // to a workable 10 000 lx and let authors override.
            let intensity =
                get_attribute_as_f32(reader, sdf_path, "inputs:intensity").unwrap_or(10_000.0);
            let exposure =
                get_attribute_as_f32(reader, sdf_path, "inputs:exposure").unwrap_or(0.0);
            let illuminance = intensity * exposure.exp2();
            let color = crate::get_attribute_as_vec3(reader, sdf_path, "inputs:color")
                .map(|c| Color::linear_rgb(c.x, c.y, c.z))
                .unwrap_or(Color::WHITE);

            let max_distance =
                get_attribute_as_f32(reader, sdf_path, "lunco:shadow:maxDistance")
                    .unwrap_or(1500.0);
            let first_bound =
                get_attribute_as_f32(reader, sdf_path, "lunco:shadow:firstCascadeFarBound")
                    .unwrap_or(40.0);
            // More cascades = the near/far split inside ONE light: tight
            // near cascades keep object/contact shadows crisp while the
            // extra far cascades carry mesh-accurate terrain self-shadow
            // out to maxDistance (the heightfield march covers beyond).
            let num_cascades = get_attribute_as_f32(reader, sdf_path, "lunco:shadow:numCascades")
                .map(|n| (n as usize).clamp(1, 8))
                .unwrap_or(4);
            let cascades = CascadeShadowConfigBuilder {
                num_cascades,
                minimum_distance: 0.1,
                first_cascade_far_bound: first_bound,
                maximum_distance: max_distance,
                // Low overlap → crisper cascade-to-cascade transitions,
                // suits the hard airless-body shadow look.
                overlap_proportion: 0.1,
            }
            .build();

            // UsdLux `inputs:angle` = angular diameter in degrees; drives
            // the physically-scaled penumbra of horizon shadows.
            let angle =
                get_attribute_as_f32(reader, sdf_path, "inputs:angle").unwrap_or(0.53);

            // With the terrain casting into (and receiving) the cascades,
            // grazing lunar sun angles make self-shadow acne the dominant
            // artifact — these defaults favour acne-free terrain over the
            // last few centimetres of contact tightness. Authorable per
            // scene, and live-tunable via `SetEnvironmentLight`.
            let depth_bias = get_attribute_as_f32(reader, sdf_path, "lunco:shadow:depthBias")
                .unwrap_or(0.06);
            let normal_bias = get_attribute_as_f32(reader, sdf_path, "lunco:shadow:normalBias")
                .unwrap_or(2.5);

            commands.insert_resource(DirectionalLightShadowMap { size: 4096 });
            commands.entity(entity).insert((
                lunco_core::SunAngularDiameter(angle),
                DirectionalLight {
                    illuminance,
                    color,
                    shadows_enabled: true,
                    shadow_depth_bias: depth_bias,
                    shadow_normal_bias: normal_bias,
                    ..Default::default()
                },
                cascades,
                UsdAuthoredLight,
            ));
            info!(
                "[usd-bevy] {} DistantLight illuminance={illuminance} shadow range {first_bound}..{max_distance} m",
                sdf_path.as_str()
            );
            true
        }
        Some("DomeLight") => {
            let intensity =
                get_attribute_as_f32(reader, sdf_path, "inputs:intensity").unwrap_or(0.0);
            commands
                .entity(entity)
                .insert((UsdDomeAmbient(intensity), UsdAuthoredLight));
            info!("[usd-bevy] {} DomeLight ambient={intensity}", sdf_path.as_str());
            true
        }
        _ => false,
    }
}

/// Fires once per authored light prim: despawns the binary's fallback
/// lights and recomputes the scene-wide ambient from authored domes (zero
/// when the scene authors none). Runs again harmlessly if more lights
/// arrive — the computation is idempotent over current world state.
pub(crate) fn on_usd_light_added(
    _trigger: On<Add, UsdAuthoredLight>,
    fallbacks: Query<Entity, With<FallbackSceneLight>>,
    domes: Query<&UsdDomeAmbient>,
    ambient: Option<ResMut<GlobalAmbientLight>>,
    mut commands: Commands,
) {
    for e in &fallbacks {
        info!("[usd-bevy] scene authored a light — despawning fallback light {e:?}");
        commands.entity(e).despawn();
    }
    if let Some(mut ambient) = ambient {
        ambient.brightness = domes.iter().map(|d| d.0).sum();
    }
}
