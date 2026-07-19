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
//! - `SphereLight` — a local point/spot (spot when `UsdLuxShapingAPI`'s
//!   `inputs:shaping:cone:angle` is authored).
//! - `RectLight` — a rectangular area light (deck-ceiling panels, softbox
//!   fills). UsdLux and Bevy agree on the geometry — XY plane, emitting along
//!   local **-Z** — so it maps 1:1 with no axis fixup. **Requires the
//!   `area_light_luts` cargo feature** (enabled on `lunco-render-bevy`); the
//!   component is render-free and authors fine without it, but samples no LTC
//!   tables and therefore renders as nothing.
//!
//! `DiskLight` / `CylinderLight` are deliberately NOT mapped: Bevy has no
//! equivalent, and approximating a disk with a rect would silently change the
//! authored lighting rather than admit the gap.
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
use openusd::sdf::{Path as SdfPath, Value};

use crate::dome;
use crate::read::UsdRead;

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
pub(crate) fn get_attribute_as_f32<R: UsdRead>(reader: &R, path: &SdfPath, attr: &str) -> Option<f32> {
    match reader.attr_value(path, attr)? {
        Value::Float(f) => Some(f),
        Value::Double(d) => Some(d as f32),
        Value::Int(i) => Some(i as f32),
        _ => None,
    }
}

/// Read a UsdLux light's authored intensity scaled by its exposure stops:
/// `inputs:intensity` × 2^`inputs:exposure`. Used wherever a UsdLux light is
/// turned into a Bevy light — the *unit* of the result depends on the target
/// component (lux for `DirectionalLight`, lumens for `Point`/`Spot`/`RectLight`),
/// but the photometric conversion is identical, so it lives here once.
pub(crate) fn read_intensity_with_exposure<R: UsdRead>(
    reader: &R,
    path: &SdfPath,
    default_intensity: f32,
) -> f32 {
    let intensity = get_attribute_as_f32(reader, path, "inputs:intensity").unwrap_or(default_intensity);
    let exposure = get_attribute_as_f32(reader, path, "inputs:exposure").unwrap_or(0.0);
    intensity * exposure.exp2()
}

/// Bool attribute reader (also accepts `int` 0/1 authoring).
///
/// Public because the shader-look authoring in `lunco-usd-sim` reads
/// `primvars:doNotCastShadows` through the same rules the `PbrLook` path uses —
/// two spellings of one primvar would be a drift bug waiting to happen.
pub fn get_attribute_as_bool<R: UsdRead>(
    reader: &R,
    path: &SdfPath,
    attr: &str,
) -> Option<bool> {
    match reader.attr_value(path, attr)? {
        Value::Bool(b) => Some(b),
        Value::Int(i) => Some(i != 0),
        _ => None,
    }
}

/// If `prim_type` is a supported UsdLux light, attach the corresponding
/// Bevy light components to `entity` and return `true`. Called from
/// `instantiate_usd_prim`; the prim's transform/visibility are applied by
/// the shared path there.
pub(crate) fn instantiate_light_prim<R: UsdRead>(
    reader: &R,
    sdf_path: &SdfPath,
    prim_type: Option<&str>,
    commands: &mut Commands,
    entity: Entity,
    // A `DomeLight`'s `inputs:texture:file` is an asset path relative to the
    // stage layer, so resolving it needs both the server and the stage it came
    // from — same pair `apply_standard_material` uses for its texture inputs.
    asset_server: &AssetServer,
    stage_id: bevy::asset::AssetId<crate::UsdStageAsset>,
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
                #[cfg(target_arch = "wasm32")]
                num_cascades: 2,
                #[cfg(not(target_arch = "wasm32"))]
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
            commands.entity(entity).try_insert((
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
            // Two domes in one prim type, and USD says which by whether the
            // author supplied an image:
            //
            //  * `inputs:texture:file` authored → a real HDRI environment.
            //    Image-based lighting + (optionally) a visible sky. See
            //    `dome.rs`.
            //  * no texture → the historical meaning: a flat ambient term.
            //    UsdLux has no "ambient light" concept, and a bare dome is the
            //    standard way to spell one.
            //
            // A textured dome deliberately contributes NO `UsdDomeAmbient`. The
            // IBL is a strictly better version of the same quantity; summing
            // both would count the sky twice and wash out every shadow.
            let Some(env) = dome::read_dome_environment(reader, sdf_path, asset_server, stage_id)
            else {
                let intensity =
                    get_attribute_as_f32(reader, sdf_path, "inputs:intensity").unwrap_or(0.0);
                commands
                    .entity(entity)
                    .try_insert((UsdDomeAmbient(intensity), UsdAuthoredLight));
                info!("[usd-bevy] {} DomeLight ambient={intensity}", sdf_path.as_str());
                return true;
            };

            info!(
                "[usd-bevy] {} DomeLight HDRI intensity={} skybox={}",
                sdf_path.as_str(),
                env.intensity,
                env.skybox,
            );
            commands.entity(entity).try_insert((env, UsdAuthoredLight));
            true
        }
        Some("SphereLight") => {
            // UNITS — this was documented backwards, and the error is a factor of
            // 4π (≈12.6x) that presents as "the light is authored but does nothing".
            //
            // Bevy's `PointLight::intensity` is luminous POWER in **lumens**, not
            // luminous intensity in candela. For an isotropic emitter the two are
            // related by `I = Φ / 4π`, so the illuminance a Bevy point light puts on
            // a surface at distance d is
            //
            //     E = Φ / (4π d²)      NOT      E = I / d²
            //
            // i.e. authoring the candela figure gives a light 12.6x too DIM. A
            // 1000 lm value here is roughly a 75 W-equivalent domestic bulb at ~1 m
            // — a plausible default for a rover work lamp, and it is a lumens-scale
            // number precisely because Bevy wants lumens.
            //
            // (`DirectionalLight::illuminance` really is lux, and `RectLight` really
            // is lumens — see below. The three are not interchangeable.)
            let base_lm = read_intensity_with_exposure(reader, sdf_path, 1000.0);

            // ── `inputs:radius` + `inputs:normalize` (UsdLux area semantics) ──────
            //
            // Both were previously unread, so authoring a radius had ZERO effect and
            // USD's area-scaling rule was simply absent.
            //
            // The spec (`crates/lunco-usd/schema/core/usdLux.usda`):
            //   * `LightAPI.inputs:intensity` — "scales the brightness of the light
            //     linearly"; `inputs:exposure` — "scales ... exponentially" (2^e).
            //   * `LightAPI.inputs:normalize` (default `0`) — "Controls if the light
            //     power should be normalized by the surface area of the light. If
            //     enabled, the light power remains constant if the light's area or
            //     angular size is changed."
            //   * `SphereLight.inputs:radius` (default `0.5`) — "the radius of the
            //     sphere".
            //
            // Read the normalize clause in reverse and it defines the DEFAULT case:
            // if power is only constant-under-area-change when normalize is ON, then
            // with it OFF `intensity` fixes RADIANCE and total power must scale with
            // the emitting area. For a sphere, A = 4πr². So:
            //
            //     Φ = intensity · 2^exposure · (normalize ? 1 : A(r)/A(r₀))
            //     A(r)/A(r₀) = (4πr²)/(4πr₀²) = (r/r₀)²
            //
            // i.e. the area term is quadratic in radius and the 4π cancels.
            //
            // WHY THE RATIO, against the schema-default r₀ = 0.5, rather than a bare
            // 4πr²: the absolute `intensity`→lumens mapping is a convention this
            // codebase already chose (see the units comment above), not something the
            // spec fixes — UsdLux `intensity` is dimensionless. Only the RATIO between
            // two radii is observable, and expressing it against the schema default
            // makes an unauthored radius exactly neutral (`(0.5/0.5)² = 1`). Using a
            // bare 4πr² would instead have silently rescaled every already-calibrated
            // light in the asset library by π on a change that authored nothing.
            const DEFAULT_SPHERE_RADIUS: f32 = 0.5; // UsdLux SphereLight schema default
            let light_radius =
                get_attribute_as_f32(reader, sdf_path, "inputs:radius").unwrap_or(DEFAULT_SPHERE_RADIUS);
            let normalize =
                get_attribute_as_bool(reader, sdf_path, "inputs:normalize").unwrap_or(false);
            // `max(0)`: a negative radius is meaningless and would still square to a
            // positive scale, quietly brightening the light.
            let area_scale = if normalize {
                1.0
            } else {
                (light_radius.max(0.0) / DEFAULT_SPHERE_RADIUS).powi(2)
            };
            let intensity_lm = base_lm * area_scale;

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
            let shadow_maps_enabled = get_attribute_as_bool(reader, sdf_path, "inputs:shadow:enable").unwrap_or(false);
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
                commands.entity(entity).try_insert(SpotLight {
                    color,
                    intensity: intensity_lm,
                    range,
                    // The same `inputs:radius`, now also as the light's physical
                    // source size — which is what the attribute geometrically MEANS.
                    // Bevy uses it for soft shadow penumbra / specular highlight size,
                    // so an authored radius reads as a bigger, softer source as well
                    // as a brighter one.
                    radius: light_radius.max(0.0),
                    shadow_maps_enabled,
                    inner_angle,
                    outer_angle,
                    ..default()
                });
                info!(
                    "[usd-bevy] {} SphereLight (SpotLight) intensity={} lm (base {} x area {}), radius={} m, normalize={}, range={} m, cone={} deg",
                    sdf_path.as_str(),
                    intensity_lm,
                    base_lm,
                    area_scale,
                    light_radius,
                    normalize,
                    range,
                    cone_angle_deg
                );
            } else {
                // Pointlight path (standard SphereLight). No `UsdAuthoredLight`
                // — local light, must not retire the fallback sun (see above).
                commands.entity(entity).try_insert(PointLight {
                    color,
                    intensity: intensity_lm,
                    range,
                    // See the SpotLight arm: `inputs:radius` is the source size too.
                    radius: light_radius.max(0.0),
                    shadow_maps_enabled,
                    ..default()
                });
                info!(
                    "[usd-bevy] {} SphereLight (PointLight) intensity={} lm (base {} x area {}), radius={} m, normalize={}, range={} m",
                    sdf_path.as_str(),
                    intensity_lm,
                    base_lm,
                    area_scale,
                    light_radius,
                    normalize,
                    range
                );
            }
            true
        }
        Some("RectLight") => {
            // `UsdLuxRectLight` and Bevy's `RectLight` share a geometry
            // convention exactly: the rectangle lies in the local XY plane and
            // emits along local **-Z**. So orientation needs no fixup — the
            // shared transform path in `instantiate_usd_prim` already places it,
            // the same deal `DistantLight` gets.
            //
            // Luminous POWER in lumens, the same unit `PointLight`/`SpotLight`
            // take (see the SphereLight arm above — that comment used to claim
            // candela, and it was wrong). UsdLux `inputs:intensity` is a
            // dimensionless scale, so it is read as lumens here; the larger
            // default simply reflects that an area light stands in for a panel
            // rather than a bulb.
            let intensity_lm = read_intensity_with_exposure(reader, sdf_path, 10_000.0);
            let color = crate::get_attribute_as_vec3(reader, sdf_path, "inputs:color")
                .map(|c| Color::linear_rgb(c.x, c.y, c.z))
                .unwrap_or(Color::WHITE);
            // `inputs:width` / `inputs:height` are the UsdLuxRectLight schema's
            // own properties; 1 m square is the schema fallback.
            let width = get_attribute_as_f32(reader, sdf_path, "inputs:width").unwrap_or(1.0);
            let height = get_attribute_as_f32(reader, sdf_path, "inputs:height").unwrap_or(1.0);
            let range = get_attribute_as_f32(reader, sdf_path, "lunco:light:range").unwrap_or(30.0);

            // No `UsdAuthoredLight`: like SphereLight, a rect is a LOCAL light (a
            // deck-ceiling panel, a softbox fill), not a scene-dominant sun/sky.
            // Stamping it would retire the binary's fallback sun.
            commands.entity(entity).try_insert(RectLight {
                color,
                intensity: intensity_lm,
                range,
                width,
                height,
            });
            info!(
                "[usd-bevy] {} RectLight intensity={} lm, {}x{} m, range={} m",
                sdf_path.as_str(),
                intensity_lm,
                width,
                height,
                range
            );
            true
        }
        _ => false,
    }
}

/// Fires once per authored light prim: despawns the binary's fallback
/// lights and recomputes the scene-wide ambient from authored domes (zero
/// when the scene authors none). Runs again harmlessly if more lights
/// arrive — the computation is idempotent over current world state.
///
/// # Why assigning the SUM is correct, and why it used to be a bug
///
/// UsdLux semantics are additive — lights compose, and one light's presence must
/// never delete another's contribution. Summing the authored domes is exactly
/// that, and assigning the sum is safe **because authored domes are now the only
/// contributor to uniform ambient**.
///
/// That was not true before. A scene could also author
/// `lunco:env:ambientBrightness` on a custom environment prim, which a separate
/// projector (`lunco-sandbox::project_env_settings`) assigned into this same
/// field. Two writers, one field, and load order decided the winner. Worse, a
/// *textured* dome deliberately contributes no [`UsdDomeAmbient`] — its texture
/// becomes IBL instead, which is the strictly better version of the same quantity
/// — so authoring a starfield sky drove this sum to zero and silently deleted the
/// scene's regolith-bounce fill. The symptom was a scene that rendered correctly
/// until someone gave it a sky, and then rendered dark.
///
/// The custom attribute is deleted: uniform ambient is spelled as an untextured
/// `UsdLuxDomeLight`, the standard USD way, with deliberately no fallback read of
/// the old name. If a second independent ambient contributor is ever introduced,
/// this must become a composition of tracked contributions rather than an
/// assignment — that is precisely what would reintroduce the bug above.
pub(crate) fn on_usd_light_added(
    _trigger: On<Add, UsdAuthoredLight>,
    fallbacks: Query<Entity, With<FallbackSceneLight>>,
    domes: Query<&UsdDomeAmbient>,
    ambient: Option<ResMut<GlobalAmbientLight>>,
    mut commands: Commands,
) {
    for e in &fallbacks {
        info!("[usd-bevy] scene authored a light — despawning fallback light {e:?}");
        commands.entity(e).try_despawn();
    }
    if let Some(mut ambient) = ambient {
        ambient.brightness = domes.iter().map(|d| d.0).sum();
    }
}
