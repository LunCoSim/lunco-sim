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
//! ## Lights are scene content
//!
//! Nothing spawns a default sun. The engine default is authored USD —
//! `lunco://lighting/sun.usda` — that scenes reference, so the engine default
//! and a scene's own opinions are two layers on ONE prim. Composition resolves
//! them before anything reaches the ECS, which makes "one scene, one sun"
//! structural rather than an invariant the engine has to police against
//! spawn ordering. A scene that references no sun is unlit.
//!
//! [`on_usd_light_added`] recomputes the global ambient from authored
//! `DomeLight`s only (**no dome ⇒ ambient 0**), so an airless-Moon scene
//! authoring a single `DistantLight` gets jet-black shadow cores for free.
//!
//! ## Shadow quality knobs
//!
//! The sun's shadow range is standard `UsdLuxShadowAPI`:
//! `inputs:shadow:distance`. Non-positive means the configured Graphics
//! default, since UsdLux's -1 "no limit" has no meaning for a cascade split.
//! Texel density scales inversely with it, so a scene wanting crisp near-field
//! shadows over a huge terrain authors a shorter distance.
//!
//! `lunco:shadow:firstCascadeFarBound` is the renderer-specific knob:
//! cascaded shadow maps are a rasterizer technique UsdLux has no attribute for,
//! so it takes a renderer namespace the way `ri:` / `karma:` / `arnold:` do.
//! Its configured Graphics value is used only when the scene leaves it
//! unauthored.

use bevy::light::GlobalAmbientLight;
use bevy::prelude::*;
use lunco_render::{LunarSunShadow, ShadowRangeAuthorship};
use openusd::sdf::{Path as SdfPath, Value};

use crate::dome;
use crate::read::UsdRead;

/// Marker for a *dominant* scene light — a sun (`DistantLight`) or sky
/// (`DomeLight`) — i.e. one that establishes how the whole scene is lit.
/// Its `Add` observer recomputes the global ambient from authored
/// `DomeLight`s ([`on_usd_light_added`]). Deliberately NOT stamped on local
/// lights like `SphereLight` headlights: a spawned vessel's lamp is not a sky,
/// and letting it trigger the ambient recompute would make scene brightness
/// depend on what happens to be spawned.
#[derive(Component)]
pub struct UsdAuthoredLight;

/// Ambient contribution of an authored `DomeLight` prim (its
/// `inputs:intensity` × 2^`inputs:exposure`, in `GlobalAmbientLight::brightness`
/// units).
#[derive(Component)]
pub(crate) struct UsdDomeAmbient(pub(crate) f32);

/// The USD attribute that turns a `DomeLight` from a scalar ambient term into an
/// HDRI environment. Named once here because two crates now have to agree on the
/// test: `dome::read_dome_environment` (which resolves it) and
/// [`untextured_dome_intensity_sum`] (which must exclude the domes it claims).
pub const DOME_TEXTURE_ATTR: &str = "inputs:texture:file";

/// Sum of `inputs:intensity` × 2^`inputs:exposure` over every **untextured**
/// `DomeLight` prim in `data`, skipping `exclude` — i.e. the ambient brightness
/// the scene would compose if `exclude` did not exist.
///
/// This is the layer-data mirror of what [`on_usd_light_added`] computes from
/// ECS: that observer sums the [`UsdDomeAmbient`] of every dome entity, and a
/// dome contributes `intensity` × 2^`exposure` in `GlobalAmbientLight::brightness`
/// units — the same [`read_intensity_with_exposure`] photometry the `DomeLight`
/// arm of [`instantiate_light_prim`] applies, or the solve here would compose a
/// different total than the instantiated light. A *textured* dome contributes
/// nothing — its image becomes IBL instead — so it is excluded here too, or the
/// two would disagree.
///
/// Exists so a command that wants to *set the composed total* can solve for the
/// one dome it owns instead of blindly authoring the total and double-counting
/// whatever the scene already authored.
///
/// # Limitation
///
/// Reads flattened layer data (`UsdDocument::composed_arc()` is an sdf
/// layer-stack merge, not PCP composition), so a dome that only exists inside a
/// `references`d asset is **not** counted, while the ECS sum does see it. Scenes
/// author their ambient fill directly, so this is the rare case; it would show up
/// as the ambient slider reading back higher than it was set.
pub fn untextured_dome_intensity_sum(data: &openusd::sdf::Data, exclude: Option<&SdfPath>) -> f32 {
    use crate::usd_data::UsdDataExt;

    // Collect paths first: `prim_type_name`/`field` re-borrow `data` immutably,
    // which is fine, but iterating while calling them keeps two borrows alive
    // across the closure — cloning the handful of dome paths is cheaper to read.
    let dome_paths: Vec<SdfPath> = data
        .iter()
        .map(|(p, _)| p.clone())
        .filter(|p| data.prim_type_name(p).as_deref() == Some("DomeLight"))
        .collect();

    dome_paths
        .iter()
        .filter(|p| exclude != Some(*p))
        .filter(|p| !dome_has_texture(data, p))
        .filter(|p| data.prim_is_active(p))
        .filter_map(|p| dome_intensity(data, p))
        .sum()
}

/// Whether `prim` authors a non-empty `inputs:texture:file` — the exact test
/// `dome::read_dome_environment` uses to decide "HDRI environment" vs "scalar
/// ambient". Presence of the `default` field is the signal; the value's asset
/// path is not inspected here (this reader has no `AssetServer` to resolve it),
/// so a dome whose texture fails to *resolve* is treated as textured and
/// contributes nothing — matching the conservative direction (never over-count).
fn dome_has_texture(data: &openusd::sdf::Data, prim: &SdfPath) -> bool {
    use crate::usd_data::UsdDataExt;
    match prim.append_property(DOME_TEXTURE_ATTR) {
        Ok(attr) => data.field(&attr, "default").is_some(),
        Err(_) => false,
    }
}

/// `inputs:intensity` × 2^`inputs:exposure` on `prim` — the layer-data twin of
/// [`read_intensity_with_exposure`], which needs a `UsdRead`. `None` when
/// `inputs:intensity` is unauthored: the sum counts only authored opinions.
fn dome_intensity(data: &openusd::sdf::Data, prim: &SdfPath) -> Option<f32> {
    let intensity = field_f32(data, prim, "inputs:intensity")?;
    let exposure = field_f32(data, prim, "inputs:exposure").unwrap_or(0.0);
    Some(intensity * exposure.exp2())
}

/// Scalar `default` field on `prim`'s attribute `attr`, tolerant of
/// `float`/`double`/`int` authoring — the layer-data twin of
/// [`get_attribute_as_f32`].
fn field_f32(data: &openusd::sdf::Data, prim: &SdfPath, attr: &str) -> Option<f32> {
    use crate::usd_data::UsdDataExt;
    let attr = prim.append_property(attr).ok()?;
    match data.field(&attr, "default")? {
        Value::Float(f) => Some(*f),
        Value::Double(d) => Some(*d as f32),
        Value::Int(i) => Some(*i as f32),
        _ => None,
    }
}

/// The intensity to author on a *dedicated* ambient-fill dome so that the
/// scene's composed ambient total — the sum over all untextured domes, see
/// [`untextured_dome_intensity_sum`] — lands exactly on `requested_total`.
///
/// # Why subtract rather than author the total
///
/// The inspector READS the composed total (`GlobalAmbientLight::brightness`) but
/// WRITES a single dome. Author the total on that dome and a scene that already
/// has an untextured dome (say a regolith bounce at 2600) composes
/// `2600 + requested`; the slider then reads that back and visibly jumps away
/// from where the user put it, every drag.
///
/// Clamps at 0 — a dome cannot emit negatively — which means a request *below*
/// what other domes already contribute is unsatisfiable. Callers should surface
/// that rather than silently render the wrong brightness; see
/// [`ambient_fill_saturates`].
pub fn ambient_fill_intensity(requested_total: f32, other_domes_total: f32) -> f32 {
    (requested_total - other_domes_total).max(0.0)
}

/// Whether [`ambient_fill_intensity`] had to clamp — i.e. the other authored
/// domes alone already exceed `requested_total`, so the composed ambient will be
/// brighter than asked no matter what the fill dome does.
pub fn ambient_fill_saturates(requested_total: f32, other_domes_total: f32) -> bool {
    other_domes_total > requested_total
}

/// Scalar attribute reader tolerant of `float`/`double`/`int` authoring.
pub(crate) fn get_attribute_as_f32(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    attr: &str,
) -> Option<f32> {
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
pub(crate) fn read_intensity_with_exposure(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    default_intensity: f32,
) -> f32 {
    let intensity = reader
        .real_f32(path, "inputs:intensity")
        .unwrap_or(default_intensity);
    let exposure = reader.real_f32(path, "inputs:exposure").unwrap_or(0.0);
    intensity * exposure.exp2()
}

/// A UsdLux light's effective linear-RGB colour: `inputs:color` (schema
/// fallback white), multiplied by the blackbody colour for
/// `inputs:colorTemperature` (fallback 6500 K) when
/// `inputs:enableColorTemperature` is authored `true` — the `UsdLuxLightAPI`
/// rule, shared by every light arm here and the dome tint in `dome.rs`.
pub(crate) fn read_light_color(reader: &crate::StageView<'_>, path: &SdfPath) -> Vec3 {
    let color = crate::get_attribute_as_vec3(reader, path, "inputs:color").unwrap_or(Vec3::ONE);
    if get_attribute_as_bool(reader, path, "inputs:enableColorTemperature").unwrap_or(false) {
        let kelvin =
            get_attribute_as_f32(reader, path, "inputs:colorTemperature").unwrap_or(6500.0);
        color * blackbody_rgb(kelvin)
    } else {
        color
    }
}

/// Linear-RGB colour of a Planckian (blackbody) radiator at `kelvin`, using the
/// standard Kim et al. cubic-spline approximation of the Planckian locus in CIE
/// xy, converted through XYZ to linear sRGB and normalized to a max component of
/// 1 — so 6500 K comes out ≈ white, low temperatures warm orange, high ones
/// blue. Input is clamped to the approximation's 1667–25000 K validity range.
fn blackbody_rgb(kelvin: f32) -> Vec3 {
    let t = f64::from(kelvin.clamp(1667.0, 25000.0));
    let x = if t <= 4000.0 {
        -0.2661239e9 / (t * t * t) - 0.2343589e6 / (t * t) + 0.8776956e3 / t + 0.179910
    } else {
        -3.0258469e9 / (t * t * t) + 2.1070379e6 / (t * t) + 0.2226347e3 / t + 0.240390
    };
    let y = if t <= 2222.0 {
        ((-1.1063814 * x - 1.34811020) * x + 2.18555832) * x - 0.20219683
    } else if t <= 4000.0 {
        ((-0.9549476 * x - 1.37418593) * x + 2.09137015) * x - 0.16748867
    } else {
        ((3.0817580 * x - 5.87338670) * x + 3.75112997) * x - 0.37001483
    };
    // xyY (Y = 1) → XYZ → linear sRGB (D65).
    let (big_x, big_z) = (x / y, (1.0 - x - y) / y);
    let r = 3.2404542 * big_x - 1.5371385 - 0.4985314 * big_z;
    let g = -0.9692660 * big_x + 1.8760108 + 0.0415560 * big_z;
    let b = 0.0556434 * big_x - 0.2040259 + 1.0572252 * big_z;
    let rgb = Vec3::new(r.max(0.0) as f32, g.max(0.0) as f32, b.max(0.0) as f32);
    rgb / rgb.max_element()
}

/// UsdLux area scaling for a `RectLight`: with `inputs:normalize` off (the
/// schema default) emitted power scales with the emitting area, and the ratio is
/// taken against the 1×1 m schema-fallback rect so an unauthored size is exactly
/// neutral — the rect analogue of the SphereLight arm's `(r/r₀)²`. `max(0)`
/// because a negative dimension is meaningless, not a sign flip.
fn rect_area_scale(normalize: bool, width: f32, height: f32) -> f32 {
    if normalize {
        1.0
    } else {
        width.max(0.0) * height.max(0.0)
    }
}

/// Bool attribute reader (also accepts `int` 0/1 authoring).
///
/// Public because the shader-look authoring in `lunco-usd-sim` reads
/// `primvars:doNotCastShadows` through the same rules the `PbrLook` path uses —
/// two spellings of one primvar would be a drift bug waiting to happen.
pub fn get_attribute_as_bool(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    attr: &str,
) -> Option<bool> {
    match reader.attr_value(path, attr)? {
        Value::Bool(b) => Some(b),
        Value::Int(i) => Some(i != 0),
        _ => None,
    }
}

/// Attenuation cutoff for a local light, in metres.
///
/// `LunCoLightAPI` declares `lunco:light:range` with a fallback of `0` and defines
/// that value as "engine default", so an unauthored attribute and an attribute that
/// resolves to the schema fallback must land on the same number. Reading it with a
/// plain `unwrap_or` honours only the first: a prim that applies the API without
/// overriding the range reads back `0` and gets a light with no influence volume at
/// all — lit in the authoring tool, black in the engine. Non-positive therefore
/// means default here, not "zero metres".
///
fn read_light_range(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    default: f32,
    convention: crate::units::ConventionTransform,
) -> f32 {
    match get_attribute_as_f32(reader, path, "lunco:light:range") {
        Some(r) if r > 0.0 => convention.length(r as f64) as f32,
        _ => default,
    }
}

/// The sun's shadow-casting range — standard `UsdLuxShadowAPI`.
///
/// UsdLux defines the schema fallback as **-1 = no limit**. A cascade shadow map
/// has no unlimited mode (the split has to end somewhere), so a non-positive
/// authored value means "engine default" here — the same rule
/// [`read_light_range`] applies to `lunco:light:range`, and for the same reason:
/// an author who applies the API without overriding the attribute must land on
/// the engine default, not on a light that shadows nothing.
fn read_shadow_distance(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    default: f32,
    convention: crate::units::ConventionTransform,
) -> f32 {
    match get_attribute_as_f32(reader, path, "inputs:shadow:distance") {
        Some(d) if d > 0.0 => convention.length(d as f64) as f32,
        _ => default,
    }
}

/// `UsdLuxShadowAPI`'s schema fallback for `inputs:shadow:enable`: **true**.
///
/// Spelled once, as a named constant, because an unauthored attribute is the
/// case where a reader's default IS the engine's answer — and a default that
/// disagrees with the schema is a silent deviation. The scene says nothing, the
/// stage resolves to one value, the engine uses another, and the only evidence
/// is a look nobody authored. A light that should not cast says so, in USD.
const USDLUX_SHADOW_ENABLE: bool = true;

/// Whether this light casts shadows — `UsdLuxShadowAPI`, at its schema
/// fallback.
///
/// The same rule for every light type. It used to be `true` for the sun and
/// `false` for local lights, on the reasoning that each shadow-casting
/// spot/point re-renders the scene into its own map and a few rovers stack up a
/// dozen extra passes. That cost is real, but it is not a licence to answer a
/// question the stage already answered: the fix is for the light to author
/// `inputs:shadow:enable = false` — which shipped local lights now do — so the
/// scene states its own render budget and the engine reads it.
fn read_shadow_enable(reader: &crate::StageView<'_>, path: &SdfPath) -> bool {
    get_attribute_as_bool(reader, path, "inputs:shadow:enable").unwrap_or(USDLUX_SHADOW_ENABLE)
}

/// If `prim_type` is a supported UsdLux light, attach the corresponding
/// Bevy light components to `entity` and return `true`. Called from
/// `instantiate_usd_prim`; the prim's transform/visibility are applied by
/// the shared path there.
pub(crate) fn instantiate_light_prim(
    reader: &crate::StageView<'_>,
    sdf_path: &SdfPath,
    prim_type: Option<&str>,
    commands: &mut Commands,
    entity: Entity,
    // A `DomeLight`'s `inputs:texture:file` is an asset path relative to the
    // stage layer, so resolving it needs both the server and the stage it came
    // from — same pair `apply_standard_material` uses for its texture inputs.
    asset_server: &AssetServer,
    stage_id: bevy::asset::AssetId<crate::UsdStageAsset>,
    quality: lunco_render::RenderQualityProfile,
) -> bool {
    let convention = crate::units::stage_convention(reader);
    // A SphereLight's scene-property ports are backed by a deferred Bevy light
    // component. Publish the generic pending state before projection can create
    // wires, so an epoch seal cannot turn that short installation window into a
    // terminal missing-port error. The ready marker is published atomically with
    // the eventual PointLight/SpotLight below.
    if prim_type == Some("SphereLight") {
        commands
            .entity(entity)
            .try_insert(lunco_core::PortSurfacePending);
    }
    match prim_type {
        Some("DistantLight") => {
            // UsdLux spec default intensity is 1.0, but 1 lx is invisible
            // under Bevy's physically-based exposure — an unauthored
            // intensity almost certainly means "give me a sun", so default
            // to the calibrated 128 000 lx lunar sun and let authors override.
            let illuminance_lux = read_intensity_with_exposure(
                reader,
                sdf_path,
                quality.distant_light_default_illuminance,
            );
            let c = read_light_color(reader, sdf_path);
            let color = Color::linear_rgb(c.x, c.y, c.z);

            // Start from the canonical lunar sun (single source of truth) and
            // override only the attributes the prim authors. An unauthored
            // attribute therefore lands on engine policy by construction — no
            // copy of the cascade split / bias / atlas values can drift here.
            //
            // `firstCascadeFarBound` is the near/far split inside ONE light:
            // tight near cascades keep contact shadows crisp while the far
            // cascades carry mesh-accurate terrain self-shadow out to
            // `inputs:shadow:distance` (the heightfield march covers beyond).
            // `inputs:angle` is the sun's angular diameter driving the
            // horizon-shadow penumbra.
            let d = LunarSunShadow::for_profile(quality);
            // Physical identity (illuminance + apparent size) is *authored* on
            // this prim: illuminance from `intensity`×2^`exposure`, angular size
            // from `inputs:angle`. The unauthored fallback is
            // `UsdLuxDistantLight`'s own — one constant, shared with
            // `lunco_environment::LunarSun`, which sits above this loader and so
            // cannot be read from here.
            let angular_diameter_deg = reader
                .real_f32(sdf_path, "inputs:angle")
                .unwrap_or(lunco_render::SOLAR_ANGULAR_DIAMETER_DEG);
            // Renderer settings supply defaults. Authored content overrides
            // only the two range attributes below, and the provenance marker
            // preserves that distinction for live Graphics edits.
            let first_cascade_far_bound =
                if reader.has_authored_attribute(sdf_path, "lunco:shadow:firstCascadeFarBound") {
                    reader
                        .real_f32(sdf_path, "lunco:shadow:firstCascadeFarBound")
                        .map(|distance| convention.length(distance as f64) as f32)
                        .unwrap_or(d.first_cascade_far_bound)
                } else {
                    d.first_cascade_far_bound
                };
            let sun = LunarSunShadow {
                maximum_distance: read_shadow_distance(
                    reader,
                    sdf_path,
                    d.maximum_distance,
                    convention,
                ),
                first_cascade_far_bound,
                ..d
            };

            // A body's reflected fill authors `false` — see
            // `lunco://lighting/earthshine.usda`.
            let casts_shadows = read_shadow_enable(reader, sdf_path);

            commands.insert_resource(sun.shadow_map());
            commands.entity(entity).try_insert((
                lunco_core::SunAngularDiameter(angular_diameter_deg),
                sun.directional_light(color, illuminance_lux, casts_shadows),
                sun.cascade_config(),
                ShadowRangeAuthorship {
                    first_cascade_far_bound: reader
                        .has_authored_attribute(sdf_path, "lunco:shadow:firstCascadeFarBound"),
                    maximum_distance: reader
                        .has_authored_attribute(sdf_path, "inputs:shadow:distance")
                        && reader
                            .real_f32(sdf_path, "inputs:shadow:distance")
                            .is_some_and(|distance| distance > 0.0),
                },
                UsdAuthoredLight,
            ));
            debug!(
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
                let intensity = read_intensity_with_exposure(reader, sdf_path, 1.0);
                commands
                    .entity(entity)
                    .try_insert((UsdDomeAmbient(intensity), UsdAuthoredLight));
                debug!(
                    "[usd-bevy] {} DomeLight ambient={intensity}",
                    sdf_path.as_str()
                );
                return true;
            };

            debug!(
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
            let base_lm = read_intensity_with_exposure(
                reader,
                sdf_path,
                quality.local_light_default_intensity,
            );

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
            let default_sphere_radius = convention.length(DEFAULT_SPHERE_RADIUS as f64) as f32;
            let light_radius = convention.length(
                get_attribute_as_f32(reader, sdf_path, "inputs:radius")
                    .unwrap_or(DEFAULT_SPHERE_RADIUS) as f64,
            ) as f32;
            let normalize =
                get_attribute_as_bool(reader, sdf_path, "inputs:normalize").unwrap_or(false);
            // `max(0)`: a negative radius is meaningless and would still square to a
            // positive scale, quietly brightening the light.
            let area_scale = if normalize {
                1.0
            } else {
                (light_radius.max(0.0) / default_sphere_radius).powi(2)
            };
            // `inputs:intensity` is the authored photometric power. Do not impose an
            // importer-side ceiling: a lunar rover deliberately needs a much brighter
            // work beam than a terrestrial cabin lamp to remain visible at its camera
            // exposure, and clamping here silently changes the USD scene.
            let intensity_lm = base_lm * area_scale;

            let c = read_light_color(reader, sdf_path);
            let color = Color::linear_rgb(c.x, c.y, c.z);
            // COST WARNING for authors, not a reader deviation: each
            // shadow-casting spot/point renders the whole scene again into its
            // own map every frame, so several rovers (two SphereLights each)
            // stack up a dozen extra passes — profiled as the dominant render
            // cost on the moonbase twin (`queue_shadows` /
            // `check_point_light_mesh…`) and enough to blow past Bevy's
            // per-cluster shadow-caster cap. A local light that does not need to
            // cast therefore authors `inputs:shadow:enable = false`, and the
            // light still ILLUMINATES.
            let shadow_maps_enabled = read_shadow_enable(reader, sdf_path);
            let range = read_light_range(
                reader,
                sdf_path,
                quality.local_light_default_range,
                convention,
            );

            if let Some(cone_angle_deg) = reader.real_f32(sdf_path, "inputs:shaping:cone:angle") {
                // Spotlight path (UsdLuxShapingAPI applied)
                let outer_angle = cone_angle_deg
                    .to_radians()
                    .clamp(0.0, std::f32::consts::FRAC_PI_2);
                let softness = reader
                    .real_f32(sdf_path, "inputs:shaping:cone:softness")
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let inner_angle = outer_angle * (1.0 - softness);

                // No `UsdAuthoredLight`: a SphereLight is a *local* light (e.g. a
                // vessel headlight), not a scene-dominant sun/sky. Stamping it
                // would re-run the dome ambient recompute every time a rover
                // spawns — see the marker docs.
                commands
                    .entity(entity)
                    .try_remove::<lunco_core::PortSurfacePending>();
                commands.entity(entity).try_insert((
                    SpotLight {
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
                        shadow_depth_bias: quality.shadow_depth_bias,
                        shadow_normal_bias: quality.shadow_normal_bias,
                        shadow_map_near_z: quality.local_shadow_map_near_z,
                        inner_angle,
                        outer_angle,
                        ..default()
                    },
                    lunco_core::PortSurfaceReady,
                ));
                debug!(
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
                // — local light, not a scene-dominant sun/sky (see above).
                commands
                    .entity(entity)
                    .try_remove::<lunco_core::PortSurfacePending>();
                commands.entity(entity).try_insert((
                    PointLight {
                        color,
                        intensity: intensity_lm,
                        range,
                        // See the SpotLight arm: `inputs:radius` is the source size too.
                        radius: light_radius.max(0.0),
                        shadow_maps_enabled,
                        shadow_depth_bias: quality.shadow_depth_bias,
                        shadow_normal_bias: quality.shadow_normal_bias,
                        shadow_map_near_z: quality.local_shadow_map_near_z,
                        ..default()
                    },
                    lunco_core::PortSurfaceReady,
                ));
                debug!(
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
            let base_lm = read_intensity_with_exposure(
                reader,
                sdf_path,
                quality.rect_light_default_intensity,
            );
            let c = read_light_color(reader, sdf_path);
            let color = Color::linear_rgb(c.x, c.y, c.z);
            // `inputs:width` / `inputs:height` are the UsdLuxRectLight schema's
            // own properties; 1 m square is the schema fallback.
            let width = convention.length(
                get_attribute_as_f32(reader, sdf_path, "inputs:width").unwrap_or(1.0) as f64,
            ) as f32;
            let height = convention.length(
                get_attribute_as_f32(reader, sdf_path, "inputs:height").unwrap_or(1.0) as f64,
            ) as f32;
            // `inputs:normalize` — the same UsdLux area rule the SphereLight arm
            // implements (see the long derivation there): with normalize OFF (the
            // schema default) `intensity` fixes radiance, so emitted power scales
            // with the emitting area. For a rect A = w·h, and the ratio against
            // the 1×1 m schema fallback makes an unauthored size exactly neutral.
            let normalize =
                get_attribute_as_bool(reader, sdf_path, "inputs:normalize").unwrap_or(false);
            let area_scale = rect_area_scale(normalize, width, height);
            let intensity_lm = base_lm * area_scale;
            let range = read_light_range(
                reader,
                sdf_path,
                quality.local_light_default_range,
                convention,
            );

            // `UsdLuxRectLight.inputs:texture:file` (an image mapped across the
            // rect) has no Bevy equivalent — say so rather than silently drop it.
            if reader
                .asset(sdf_path, "inputs:texture:file")
                .filter(|p| !p.is_empty())
                .is_some()
            {
                warn!(
                    "[usd-bevy] {} RectLight inputs:texture:file is unsupported — \
                     the light emits its flat color instead",
                    sdf_path.as_str(),
                );
            }

            // No `UsdAuthoredLight`: like SphereLight, a rect is a LOCAL light (a
            // deck-ceiling panel, a softbox fill), not a scene-dominant sun/sky.
            commands.entity(entity).try_insert(RectLight {
                color,
                intensity: intensity_lm,
                range,
                width,
                height,
            });
            debug!(
                "[usd-bevy] {} RectLight intensity={} lm (base {} x area {}), {}x{} m, normalize={}, range={} m",
                sdf_path.as_str(),
                intensity_lm,
                base_lm,
                area_scale,
                width,
                height,
                normalize,
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
/// projector (`lunco-luncosim::project_env_settings`) assigned into this same
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
/// Edge-triggered by design: the ambient total is a pure reduction over the
/// domes that currently exist, so recomputing it when the dome set changes is
/// exactly right.
pub(crate) fn on_usd_light_added(
    _trigger: On<Add, UsdAuthoredLight>,
    domes: Query<&UsdDomeAmbient>,
    ambient: Option<ResMut<GlobalAmbientLight>>,
) {
    if let Some(mut ambient) = ambient {
        ambient.brightness = domes.iter().map(|d| d.0).sum();
    }
}

#[cfg(test)]
mod ambient_fill_tests {
    //! The solve behind the ambient slider: the inspector reads the COMPOSED
    //! total but writes ONE dome, so that dome's intensity is the requested
    //! total minus what every other untextured dome already contributes.

    use super::*;

    fn data(usda: &str) -> openusd::sdf::Data {
        openusd::usda::parse(usda).expect("parse USDA")
    }

    fn path(s: &str) -> SdfPath {
        SdfPath::new(s).expect("valid path")
    }

    const SCENE: &str = r#"#usda 1.0

def Xform "World"
{
    def DomeLight "RegolithBounce"
    {
        float inputs:intensity = 30
    }

    def DomeLight "Starfield"
    {
        asset inputs:texture:file = @./stars.hdr@
        float inputs:intensity = 500
    }

    def "Environment"
    {
        def DomeLight "AmbientFill"
        {
            float inputs:intensity = 70
        }
    }
}
"#;

    #[test]
    fn sum_excludes_the_fill_dome_itself() {
        let d = data(SCENE);
        let fill = path("/World/Environment/AmbientFill");
        // Only `RegolithBounce` counts: the fill is excluded by request and the
        // starfield by its texture.
        assert_eq!(untextured_dome_intensity_sum(&d, Some(&fill)), 30.0);
    }

    #[test]
    fn textured_domes_never_contribute() {
        let d = data(SCENE);
        // Without excluding anything, the 500-intensity starfield must STILL be
        // absent — its image becomes IBL, not a scalar ambient term.
        assert_eq!(untextured_dome_intensity_sum(&d, None), 30.0 + 70.0);
    }

    #[test]
    fn requested_100_over_an_existing_30_authors_70() {
        let d = data(SCENE);
        let fill = path("/World/Environment/AmbientFill");
        let others = untextured_dome_intensity_sum(&d, Some(&fill));
        assert_eq!(others, 30.0);
        assert_eq!(ambient_fill_intensity(100.0, others), 70.0);
        assert!(!ambient_fill_saturates(100.0, others));
        // The point of the subtraction: re-composing lands on the request, so
        // the slider reads back exactly where the user left it.
        assert_eq!(others + ambient_fill_intensity(100.0, others), 100.0);
    }

    #[test]
    fn no_other_domes_means_author_the_total_verbatim() {
        let d = data("#usda 1.0\n\ndef Xform \"World\"\n{\n}\n");
        assert_eq!(untextured_dome_intensity_sum(&d, None), 0.0);
        assert_eq!(ambient_fill_intensity(42.0, 0.0), 42.0);
    }

    #[test]
    fn clamps_at_zero_and_reports_saturation() {
        // Other domes already out-shine the request: a dome cannot emit
        // negatively, so the composed total will exceed what was asked.
        assert_eq!(ambient_fill_intensity(10.0, 30.0), 0.0);
        assert!(ambient_fill_saturates(10.0, 30.0));
        // Exactly equal is satisfiable (author zero), not saturated.
        assert_eq!(ambient_fill_intensity(30.0, 30.0), 0.0);
        assert!(!ambient_fill_saturates(30.0, 30.0));
    }

    #[test]
    fn inactive_domes_do_not_contribute() {
        let d = data(
            r#"#usda 1.0

def Xform "World"
{
    def DomeLight "Off" (active = false)
    {
        float inputs:intensity = 999
    }

    def DomeLight "On"
    {
        float inputs:intensity = 5
    }
}
"#,
        );
        assert_eq!(untextured_dome_intensity_sum(&d, None), 5.0);
    }

    #[test]
    fn exposure_scales_the_ambient_sum() {
        // The DomeLight arm of `instantiate_light_prim` reads
        // `inputs:intensity` × 2^`inputs:exposure`, so the solve must compose
        // the same total or the slider would read back wrong on any dome that
        // authors an exposure.
        let d = data(
            r#"#usda 1.0

def DomeLight "Fill"
{
    float inputs:intensity = 8
    float inputs:exposure = 3
}
"#,
        );
        assert_eq!(untextured_dome_intensity_sum(&d, None), 64.0);
    }
}

#[cfg(test)]
mod photometry_tests {
    use super::*;
    use crate::canonical::{CanonicalStage, StageRecipe};

    #[test]
    fn rect_power_scales_with_area_unless_normalized() {
        // Schema-fallback 1×1 m is exactly neutral.
        assert_eq!(rect_area_scale(false, 1.0, 1.0), 1.0);
        // normalize OFF (the default): power scales with w·h.
        assert_eq!(rect_area_scale(false, 2.0, 3.0), 6.0);
        // normalize ON: authored intensity IS the power, whatever the size.
        assert_eq!(rect_area_scale(true, 2.0, 3.0), 1.0);
        // A negative dimension clamps to zero rather than flipping sign.
        assert_eq!(rect_area_scale(false, -2.0, 3.0), 0.0);
    }

    #[test]
    fn blackbody_is_white_at_6500k_warm_below_cool_above() {
        let white = blackbody_rgb(6500.0);
        for ch in white.to_array() {
            assert!(ch > 0.9, "6500 K should be ≈ white, got {white:?}");
        }
        let warm = blackbody_rgb(2000.0);
        assert_eq!(warm.x, 1.0);
        assert!(warm.z < 0.1, "2000 K should be orange, got {warm:?}");
        let cool = blackbody_rgb(10000.0);
        assert_eq!(cool.z, 1.0);
        assert!(cool.x < 0.9, "10000 K should be blue, got {cool:?}");
    }

    #[test]
    fn authored_light_lengths_convert_from_stage_units_to_metres() {
        let source = r#"#usda 1.0

def Xform "World"
{
    def SphereLight "Lamp"
    {
        float lunco:light:range = 90
        float inputs:radius = 2
        float inputs:shadow:distance = 600
    }
}
"#;
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", source))
            .expect("stage builds");
        let view = stage.view();
        let lamp = SdfPath::new("/World/Lamp").unwrap();
        let convention = crate::units::stage_convention(&view);

        assert_eq!(read_light_range(&view, &lamp, 30.0, convention), 0.9);
        assert_eq!(
            convention.length(get_attribute_as_f32(&view, &lamp, "inputs:radius").unwrap() as f64)
                as f32,
            0.02
        );
        assert_eq!(read_shadow_distance(&view, &lamp, 1500.0, convention), 6.0);
    }

    #[test]
    fn omitted_intensity_uses_renderer_default_and_authored_intensity_wins() {
        let source = r#"#usda 1.0

def Xform "World"
{
    def DistantLight "Sun"
    {
    }
    def SphereLight "Bulb"
    {
    }
    def RectLight "Panel"
    {
        float inputs:intensity = 23
    }
}
"#;
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", source))
            .expect("stage builds");
        let view = stage.view();

        assert_eq!(
            read_intensity_with_exposure(&view, &SdfPath::new("/World/Sun").unwrap(), 77_000.0),
            77_000.0
        );
        assert_eq!(
            read_intensity_with_exposure(&view, &SdfPath::new("/World/Bulb").unwrap(), 700.0),
            700.0
        );
        assert_eq!(
            read_intensity_with_exposure(&view, &SdfPath::new("/World/Panel").unwrap(), 10_000.0),
            23.0
        );
    }
}
