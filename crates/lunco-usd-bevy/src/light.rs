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

use bevy::light::GlobalAmbientLight;
use bevy::prelude::*;
use lunco_render::LunarSunShadow;
use openusd::sdf::{Data, Path as SdfPath, Value};

use crate::usd_data::UsdDataExt;

/// Tag for a binary's built-in default sun — defined in `lunco-core` (so
/// non-USD crates can tag their lights too), re-exported here where the
/// despawn policy lives. Despawned as soon as the loaded scene authors its
/// own light prim.
pub use lunco_core::FallbackSceneLight;

/// Marker for a *dominant* scene light — a sun (`DistantLight`) or sky
/// (`DomeLight`) — whose presence retires the binary's fallback sun/ambient.
/// Its `Add` observer enforces that policy (see module docs). Deliberately
/// NOT stamped on local lights like `SphereLight` headlights: a spawned
/// vessel's lamp must not darken the scene by despawning the fallback sun.
#[derive(Component)]
pub struct UsdAuthoredLight;

/// Ambient contribution of an authored `DomeLight` prim (its
/// `inputs:intensity`, in `GlobalAmbientLight::brightness` units).
#[derive(Component)]
pub(crate) struct UsdDomeAmbient(pub(crate) f32);

/// Scalar attribute reader tolerant of `float`/`double`/`int` authoring.
pub(crate) fn get_attribute_as_f32(reader: &Data, path: &SdfPath, attr: &str) -> Option<f32> {
    let attr_path = path.append_property(attr).ok()?;
    let val = reader.field(&attr_path, "default")?;
    match val {
        Value::Float(f) => Some(*f),
        Value::Double(d) => Some(*d as f32),
        Value::Int(i) => Some(*i as f32),
        _ => None,
    }
}

/// Read a UsdLux light's authored intensity scaled by its exposure stops:
/// `inputs:intensity` × 2^`inputs:exposure`. Used wherever a UsdLux light is
/// turned into a Bevy light — the *unit* of the result depends on the target
/// component (lux for `DirectionalLight`, candela for `Point`/`SpotLight`),
/// but the photometric conversion is identical, so it lives here once.
pub(crate) fn read_intensity_with_exposure(
    reader: &Data,
    path: &SdfPath,
    default_intensity: f32,
) -> f32 {
    let intensity = get_attribute_as_f32(reader, path, "inputs:intensity").unwrap_or(default_intensity);
    let exposure = get_attribute_as_f32(reader, path, "inputs:exposure").unwrap_or(0.0);
    intensity * exposure.exp2()
}

/// Bool attribute reader (also accepts `int` 0/1 authoring).
pub(crate) fn get_attribute_as_bool(
    reader: &Data,
    path: &SdfPath,
    attr: &str,
) -> Option<bool> {
    let attr_path = path.append_property(attr).ok()?;
    let val = reader.field(&attr_path, "default")?;
    match val {
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
    reader: &Data,
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
            let illuminance_lux = read_intensity_with_exposure(reader, sdf_path, 10_000.0);
            let color = crate::get_attribute_as_vec3(reader, sdf_path, "inputs:color")
                .map(|c| Color::linear_rgb(c.x, c.y, c.z))
                .unwrap_or(Color::WHITE);

            // Start from the canonical lunar sun (single source of truth) and
            // override only the attributes the prim authors. An unauthored
            // attribute therefore matches the engine's fallback suns by
            // construction — no copy of the cascade split / bias / atlas values
            // can drift here.
            //
            // `lunco:shadow:numCascades` is the near/far split inside ONE light:
            // tight near cascades keep contact shadows crisp while the far
            // cascades carry mesh-accurate terrain self-shadow out to
            // `maxDistance` (the heightfield march covers beyond). The bias
            // defaults favour acne-free terrain over the last centimetres of
            // contact tightness; `inputs:angle` is the sun's angular diameter
            // driving the horizon-shadow penumbra.
            let d = LunarSunShadow::default();
            // Physical identity (illuminance + apparent size) is *authored* on
            // this prim: illuminance from `intensity`×2^`exposure`, angular size
            // from `inputs:angle`. The canonical default for an unauthored angle
            // lives in `lunco_environment::LunarSun` — but this loader sits below
            // environment (materials → usd-bevy forbids the edge), so it carries
            // its own fallback const. Keep it in sync with `LunarSun::default`.
            const DEFAULT_SUN_ANGULAR_DIAMETER_DEG: f32 = 0.53;
            let angular_diameter_deg = get_attribute_as_f32(reader, sdf_path, "inputs:angle")
                .unwrap_or(DEFAULT_SUN_ANGULAR_DIAMETER_DEG);
            let sun = LunarSunShadow {
                maximum_distance: get_attribute_as_f32(reader, sdf_path, "lunco:shadow:maxDistance")
                    .unwrap_or(d.maximum_distance),
                first_cascade_far_bound: get_attribute_as_f32(
                    reader, sdf_path, "lunco:shadow:firstCascadeFarBound",
                )
                .unwrap_or(d.first_cascade_far_bound),
                // TODO(review #3): clamp narrowed 1..=8 → 1..=4. If this is an
                // intentional alignment with the canonical 4-cascade default,
                // `warn!` when a scene authors >4 instead of silently clamping;
                // if unintentional, restore 1..=8.
                num_cascades: get_attribute_as_f32(reader, sdf_path, "lunco:shadow:numCascades")
                    .map(|n| (n as usize).clamp(1, 4))
                    .unwrap_or(d.num_cascades),
                depth_bias: get_attribute_as_f32(reader, sdf_path, "lunco:shadow:depthBias")
                    .unwrap_or(d.depth_bias),
                normal_bias: get_attribute_as_f32(reader, sdf_path, "lunco:shadow:normalBias")
                    .unwrap_or(d.normal_bias),
                ..d
            };

            commands.insert_resource(sun.shadow_map());
            commands.entity(entity).insert((
                lunco_core::SunAngularDiameter(angular_diameter_deg),
                sun.directional_light(color, illuminance_lux),
                sun.cascade_config(),
                UsdAuthoredLight,
            ));
            info!(
                "[usd-bevy] {} DistantLight illuminance={} shadow range {}..{} m",
                sdf_path.as_str(),
                illuminance_lux,
                sun.first_cascade_far_bound,
                sun.maximum_distance,
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
        Some("SphereLight") => {
            // Bevy interprets `Point`/`SpotLight::intensity` as luminous
            // intensity in candela (lm/sr), not total lumens.
            let intensity_cd = read_intensity_with_exposure(reader, sdf_path, 1000.0);
            let color = crate::get_attribute_as_vec3(reader, sdf_path, "inputs:color")
                .map(|c| Color::linear_rgb(c.x, c.y, c.z))
                .unwrap_or(Color::WHITE);
            // Local lights (SphereLight → Spot/Point: rover headlights, fill
            // lamps) default to NO cast shadows: each shadow-casting spot/point
            // renders the whole scene again into its own shadow map every frame,
            // and a scene with several rovers (two SphereLights each) stacks up a
            // dozen extra shadow passes — profiled as the dominant render cost on
            // the moonbase twin (`queue_shadows` / `check_point_light_mesh…`), and
            // it also blows past Bevy's per-cluster shadow-caster cap. The light
            // still ILLUMINATES; it just doesn't cast. A scene that genuinely
            // wants a hero cast shadow opts in per-light with
            // `inputs:shadow:enable = true`. (Was `unwrap_or(true)` — the
            // TODO(review #2) fix.)
            let shadows_enabled = get_attribute_as_bool(reader, sdf_path, "inputs:shadow:enable").unwrap_or(false);
            let range = get_attribute_as_f32(reader, sdf_path, "lunco:light:range").unwrap_or(30.0);

            if let Some(cone_angle_deg) = get_attribute_as_f32(reader, sdf_path, "inputs:shaping:cone:angle") {
                // Spotlight path (UsdLuxShapingAPI applied)
                let outer_angle = (cone_angle_deg.to_radians() / 2.0).clamp(0.0, std::f32::consts::FRAC_PI_2);
                let softness = get_attribute_as_f32(reader, sdf_path, "inputs:shaping:cone:softness")
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let inner_angle = outer_angle * (1.0 - softness);

                // No `UsdAuthoredLight`: a SphereLight is a *local* light (e.g. a
                // vessel headlight), not a scene-dominant sun/sky. Stamping it
                // would retire the binary's fallback sun the instant a rover
                // spawns — see the marker docs.
                commands.entity(entity).insert(SpotLight {
                    color,
                    intensity: intensity_cd,
                    range,
                    shadows_enabled,
                    inner_angle,
                    outer_angle,
                    ..default()
                });
                info!(
                    "[usd-bevy] {} SphereLight (SpotLight) intensity={} cd, range={} m, cone={} deg",
                    sdf_path.as_str(),
                    intensity_cd,
                    range,
                    cone_angle_deg
                );
            } else {
                // Pointlight path (standard SphereLight). No `UsdAuthoredLight`
                // — local light, must not retire the fallback sun (see above).
                commands.entity(entity).insert(PointLight {
                    color,
                    intensity: intensity_cd,
                    range,
                    shadows_enabled,
                    ..default()
                });
                info!(
                    "[usd-bevy] {} SphereLight (PointLight) intensity={} cd, range={} m",
                    sdf_path.as_str(),
                    intensity_cd,
                    range
                );
            }
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
