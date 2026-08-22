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
//! `inputs:shadow:distance`. USD's -1 "no limit" maps to the configured Graphics
//! default, since a cascade shadow map has no unlimited split.
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
use lunco_render::{
    LightGraphicsDefaults, LunarSunShadow, RenderQualityProfile, ShadowRangeAuthorship,
};
use openusd::sdf::{Path as SdfPath, Value};

use crate::dome;
use crate::read::UsdRead;

/// An authored light attribute could not be interpreted without inventing a
/// replacement value. The importer logs the precise prim/property and refuses
/// that light; live edits retain the previous valid state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LightReadError;

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
pub(crate) struct UsdDomeAmbient {
    pub(crate) value: f32,
    pub(crate) uses_graphics_default: bool,
    pub(crate) exposure_scale: f32,
}

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
pub fn untextured_dome_intensity_sum(
    data: &openusd::sdf::Data,
    exclude: Option<&SdfPath>,
) -> Result<f32, LightReadError> {
    use crate::usd_data::UsdDataExt;

    // Collect paths first: `prim_type_name`/`field` re-borrow `data` immutably,
    // which is fine, but iterating while calling them keeps two borrows alive
    // across the closure — cloning the handful of dome paths is cheaper to read.
    let dome_paths: Vec<SdfPath> = data
        .iter()
        .map(|(p, _)| p.clone())
        .filter(|p| data.prim_type_name(p).as_deref() == Some("DomeLight"))
        .collect();

    let mut total = 0.0;
    for path in dome_paths
        .iter()
        .filter(|p| exclude != Some(*p))
        .filter(|p| data.prim_is_active(p))
    {
        if dome_has_texture(data, path)? {
            continue;
        }
        if let Some(intensity) = dome_intensity(data, path)? {
            total += intensity;
        }
    }

    if total.is_finite() && total >= 0.0 {
        Ok(total)
    } else {
        error!("[usd-bevy] authored DomeLight ambient sum is non-finite or negative");
        Err(LightReadError)
    }
}

/// Whether `prim` authors a non-empty `inputs:texture:file` — the exact test
/// `dome::read_dome_environment` uses to decide "HDRI environment" vs "scalar
/// ambient". An empty asset is the explicit scalar spelling; a non-asset value,
/// animation, or connection is rejected because this authoring-layer reader
/// cannot resolve it safely.
fn dome_has_texture(data: &openusd::sdf::Data, prim: &SdfPath) -> Result<bool, LightReadError> {
    let attr = prim
        .append_property(DOME_TEXTURE_ATTR)
        .map_err(|_| LightReadError)?;
    let Some(spec) = data.spec(&attr) else {
        return Ok(false);
    };
    if spec.get("timeSamples").is_some() || spec.get("connectionPaths").is_some() {
        error!(
            "[usd-bevy] {} has an animated or connected DomeLight texture input, which \
             cannot be resolved by the authoring-layer ambient solver",
            prim.as_str()
        );
        return Err(LightReadError);
    }
    let Some(value) = spec.get("default") else {
        return Ok(false);
    };
    match value {
        Value::AssetPath(path) => Ok(!path.as_str().is_empty()),
        _ => {
            error!(
                "[usd-bevy] {} has authored DomeLight {} with an unsupported type",
                prim.as_str(),
                DOME_TEXTURE_ATTR
            );
            Err(LightReadError)
        }
    }
}

/// `inputs:intensity` × 2^`inputs:exposure` on `prim` — the layer-data twin of
/// [`read_intensity_with_exposure`], which needs a `UsdRead`. `None` when
/// `inputs:intensity` is unauthored: the sum counts only authored opinions.
fn dome_intensity(
    data: &openusd::sdf::Data,
    prim: &SdfPath,
) -> Result<Option<f32>, LightReadError> {
    let Some(intensity) = field_f32(data, prim, "inputs:intensity")? else {
        return Ok(None);
    };
    let exposure = field_f32(data, prim, "inputs:exposure")?.unwrap_or(0.0);
    let scaled = intensity * exposure.exp2();
    if scaled.is_finite() && scaled >= 0.0 {
        Ok(Some(scaled))
    } else {
        error!(
            "[usd-bevy] {} has non-finite or negative authored DomeLight intensity after exposure",
            prim.as_str()
        );
        Err(LightReadError)
    }
}

/// Scalar `default` field on `prim`'s attribute `attr`, tolerant of
/// `float`/`double`/`int`/`int64` authoring — the layer-data twin of
/// [`UsdRead::real_f32`].
fn field_f32(
    data: &openusd::sdf::Data,
    prim: &SdfPath,
    attr: &str,
) -> Result<Option<f32>, LightReadError> {
    let attr_path = prim.append_property(attr).map_err(|_| LightReadError)?;
    let Some(spec) = data.spec(&attr_path) else {
        return Ok(None);
    };
    if spec.get("timeSamples").is_some() || spec.get("connectionPaths").is_some() {
        error!(
            "[usd-bevy] {} has an animated or connected DomeLight {} input, which \
             cannot be resolved by the authoring-layer ambient solver",
            prim.as_str(),
            attr
        );
        return Err(LightReadError);
    }
    let Some(value) = spec.get("default") else {
        return Ok(None);
    };
    let value = match value {
        Value::Float(f) => *f,
        Value::Double(d) => *d as f32,
        Value::Int(i) => *i as f32,
        Value::Int64(i) => *i as f32,
        _ => {
            error!(
                "[usd-bevy] {} has authored DomeLight {} with an unsupported type",
                prim.as_str(),
                attr
            );
            return Err(LightReadError);
        }
    };
    if value.is_finite() {
        Ok(Some(value))
    } else {
        error!(
            "[usd-bevy] {} has non-finite authored DomeLight {}",
            prim.as_str(),
            attr
        );
        Err(LightReadError)
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

/// Read a UsdLux light's authored intensity scaled by its exposure stops:
/// `inputs:intensity` × 2^`inputs:exposure`. Used wherever a UsdLux light is
/// turned into a Bevy light — the *unit* of the result depends on the target
/// component (lux for `DirectionalLight`, lumens for `Point`/`Spot`/`RectLight`),
/// but the photometric conversion is identical, so it lives here once. `Err`
/// means an authored intensity/exposure could not be interpreted safely.
pub fn read_intensity_with_exposure(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    default_intensity: f32,
) -> Result<f32, LightReadError> {
    Ok(resolve_intensity_with_exposure(reader, path, default_intensity)?.0)
}

fn resolve_intensity_with_exposure(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    default_intensity: f32,
) -> Result<(f32, bool, f32), LightReadError> {
    let authored_intensity = read_authored_real(reader, path, "inputs:intensity")?;
    let intensity = authored_intensity.unwrap_or(default_intensity);
    let exposure = read_authored_real(reader, path, "inputs:exposure")?.unwrap_or(0.0);
    let exposure_scale = exposure.exp2();
    let scaled = intensity * exposure_scale;
    if scaled.is_finite() && scaled >= 0.0 {
        Ok((scaled, authored_intensity.is_none(), exposure_scale))
    } else {
        error!(
            "[usd-bevy] {} has non-finite or negative light intensity after exposure",
            path.as_str()
        );
        Err(LightReadError)
    }
}

/// The resolved intensity parts shared by scalar and textured dome paths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DomeIntensity {
    /// Effective `inputs:intensity` × 2^`inputs:exposure`.
    pub value: f32,
    /// Whether the base intensity came from Graphics because USD omitted it.
    pub uses_graphics_default: bool,
    /// The exposure multiplier retained for a live Graphics-default update.
    pub exposure_scale: f32,
}

/// Read the effective intensity of a `DomeLight`, using the Graphics setting
/// only when USD omits `inputs:intensity` and preserving authored intensity and
/// exposure exactly.
pub fn read_dome_intensity(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    quality: RenderQualityProfile,
) -> Result<DomeIntensity, LightReadError> {
    let (value, uses_graphics_default, exposure_scale) =
        resolve_intensity_with_exposure(reader, path, quality.dome_default_intensity)?;
    Ok(DomeIntensity {
        value,
        uses_graphics_default,
        exposure_scale,
    })
}

/// Read a real-valued USD light attribute only when an authored opinion exists.
/// Missing attributes retain the documented USD/Graphics default; malformed or
/// non-finite authored values are rejected instead of being converted into a
/// plausible-looking light.
fn read_authored_real(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    name: &str,
) -> Result<Option<f32>, LightReadError> {
    let has_authored_value = reader.has_authored_attribute(path, name);
    let has_connection = !reader.connections(path, name).is_empty();
    if !has_authored_value && !has_connection {
        return Ok(None);
    }
    match reader.real_f32(path, name) {
        Some(value) if value.is_finite() => Ok(Some(value)),
        Some(_) => {
            error!(
                "[usd-bevy] {} has non-finite authored light attribute {}",
                path.as_str(),
                name
            );
            Err(LightReadError)
        }
        None => {
            error!(
                "[usd-bevy] {} has authored light attribute {} with an unsupported type",
                path.as_str(),
                name
            );
            Err(LightReadError)
        }
    }
}

/// A UsdLux light's effective linear-RGB colour: `inputs:color` (schema
/// fallback white), multiplied by the blackbody colour for
/// `inputs:colorTemperature` (fallback 6500 K) when
/// `inputs:enableColorTemperature` is authored `true` — the `UsdLuxLightAPI`
/// rule, shared by every light arm here and the dome tint in `dome.rs`.
pub(crate) fn read_light_color(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
) -> Result<Vec3, LightReadError> {
    let color = if reader.has_authored_attribute(path, "inputs:color")
        || !reader.connections(path, "inputs:color").is_empty()
    {
        let Some(color) = crate::get_attribute_as_vec3(reader, path, "inputs:color") else {
            error!(
                "[usd-bevy] {} has authored inputs:color with an unsupported type",
                path.as_str()
            );
            return Err(LightReadError);
        };
        if !color.is_finite() || color.min_element() < 0.0 {
            error!(
                "[usd-bevy] {} has invalid authored inputs:color = {color:?}",
                path.as_str()
            );
            return Err(LightReadError);
        }
        color
    } else {
        Vec3::ONE
    };
    let enabled =
        read_authored_bool(reader, path, "inputs:enableColorTemperature")?.unwrap_or(false);
    if !enabled {
        return Ok(color);
    }
    let kelvin = read_authored_real(reader, path, "inputs:colorTemperature")?.unwrap_or(6500.0);
    let Some(temperature) = blackbody_rgb(kelvin) else {
        error!(
            "[usd-bevy] {} has unsupported authored color temperature {kelvin}; expected a finite value in [1667, 25000] K",
            path.as_str()
        );
        return Err(LightReadError);
    };
    Ok(color * temperature)
}

/// Read an authored USD boolean, preserving the distinction between an omitted
/// attribute and a malformed value.
pub(crate) fn read_authored_bool(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    name: &str,
) -> Result<Option<bool>, LightReadError> {
    if !reader.has_authored_attribute(path, name) && reader.connections(path, name).is_empty() {
        return Ok(None);
    }
    reader.boolean(path, name).map(Some).ok_or_else(|| {
        error!(
            "[usd-bevy] {} has authored light attribute {} with an unsupported type",
            path.as_str(),
            name
        );
        LightReadError
    })
}

/// Linear-RGB colour of a Planckian (blackbody) radiator at `kelvin`, using the
/// standard Kim et al. cubic-spline approximation of the Planckian locus in CIE
/// xy, converted through XYZ to linear sRGB and normalized to a max component of
/// 1 — so 6500 K comes out ≈ white, low temperatures warm orange, high ones
/// blue. Values outside the approximation's 1667–25000 K validity range are
/// rejected rather than clamped.
fn blackbody_rgb(kelvin: f32) -> Option<Vec3> {
    if !kelvin.is_finite() || !(1667.0..=25000.0).contains(&kelvin) {
        return None;
    }
    let t = f64::from(kelvin);
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
    let max = rgb.max_element();
    (max > 0.0 && max.is_finite()).then_some(rgb / max)
}

/// Convert one authored positive stage-space length to canonical metres without
/// admitting an overflowed or degenerate result.
fn positive_length(
    value: f32,
    convention: crate::units::ConventionTransform,
    path: &SdfPath,
    name: &str,
) -> Result<f32, LightReadError> {
    if !value.is_finite() || value <= 0.0 {
        error!(
            "[usd-bevy] {} has invalid authored {} = {value}; expected a positive value",
            path.as_str(),
            name
        );
        return Err(LightReadError);
    }
    let metres = convention.length(value as f64) as f32;
    if metres.is_finite() && metres > 0.0 {
        Ok(metres)
    } else {
        error!(
            "[usd-bevy] {} has {} that is not finite and positive after unit conversion",
            path.as_str(),
            name
        );
        Err(LightReadError)
    }
}

fn read_positive_length(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    name: &str,
    default: f32,
    convention: crate::units::ConventionTransform,
) -> Result<f32, LightReadError> {
    let value = read_authored_real(reader, path, name)?.unwrap_or(default);
    positive_length(value, convention, path, name)
}

/// UsdLux area scaling for a `RectLight`: with `inputs:normalize` off (the
/// schema default) emitted power scales with the emitting area, and the ratio is
/// taken against the 1×1 m schema-fallback rect so an unauthored size is exactly
/// neutral — the rect analogue of the SphereLight arm's `(r/r₀)²`. A
/// non-positive or overflowing area is rejected rather than normalized.
fn area_scale(normalize: bool, area_ratio: f32) -> Option<f32> {
    if normalize {
        Some(1.0)
    } else {
        (area_ratio.is_finite() && area_ratio > 0.0).then_some(area_ratio)
    }
}

/// Attenuation cutoff for a local light, in metres.
///
/// `LunCoLightAPI` declares `lunco:light:range` with a fallback of `0` and defines
/// that value as "engine default", so an unauthored attribute and an attribute that
/// resolves to the schema fallback must land on the same number. Reading it with a
/// plain `unwrap_or` honours only the first: a prim that applies the API without
/// overriding the range reads back `0` and gets a light with no influence volume at
/// all — lit in the authoring tool, black in the engine. Zero therefore means
/// default here, not "zero metres"; negative authored values are invalid.
///
fn read_light_range(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    default: f32,
    convention: crate::units::ConventionTransform,
) -> Result<(f32, bool), LightReadError> {
    match read_authored_real(reader, path, "lunco:light:range")? {
        None | Some(0.0) => Ok((default, true)),
        Some(r) if r > 0.0 => {
            let metres = convention.length(r as f64) as f32;
            if metres.is_finite() {
                Ok((metres, false))
            } else {
                error!(
                    "[usd-bevy] {} has an authored light range that is not finite after unit conversion",
                    path.as_str()
                );
                Err(LightReadError)
            }
        }
        Some(r) => {
            error!(
                "[usd-bevy] {} has invalid authored lunco:light:range = {r}; expected zero (engine default) or a positive value",
                path.as_str()
            );
            Err(LightReadError)
        }
    }
}

/// The sun's shadow-casting range — standard `UsdLuxShadowAPI`.
///
/// UsdLux defines the schema fallback as **-1 = no limit**. A cascade shadow map
/// has no unlimited mode (the split has to end somewhere), so the standard
/// authored `-1` value means "engine default" here. An author who applies the
/// API without overriding the attribute therefore lands on the engine default,
/// while other invalid negative/zero values are rejected.
fn read_shadow_distance(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
    default: f32,
    convention: crate::units::ConventionTransform,
) -> Result<(f32, bool), LightReadError> {
    match read_authored_real(reader, path, "inputs:shadow:distance")? {
        Some(d) if d > 0.0 => {
            let metres = convention.length(d as f64) as f32;
            if metres.is_finite() {
                Ok((metres, true))
            } else {
                error!(
                    "[usd-bevy] {} has an authored shadow distance that is not finite after unit conversion",
                    path.as_str()
                );
                Err(LightReadError)
            }
        }
        Some(-1.0) => Ok((default, false)),
        Some(distance) => {
            error!(
                "[usd-bevy] {} has invalid authored inputs:shadow:distance = {distance}; expected -1 (USD no-limit) or a positive value",
                path.as_str()
            );
            Err(LightReadError)
        }
        None => Ok((default, false)),
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
fn read_shadow_enable(
    reader: &crate::StageView<'_>,
    path: &SdfPath,
) -> Result<bool, LightReadError> {
    Ok(read_authored_bool(reader, path, "inputs:shadow:enable")?.unwrap_or(USDLUX_SHADOW_ENABLE))
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
    let convention = match crate::units::stage_convention(reader) {
        Ok(convention) => convention,
        Err(error) => {
            error!(
                "[usd-bevy] {} has invalid stage convention metadata: {error}; refusing light",
                sdf_path.as_str()
            );
            return false;
        }
    };
    match prim_type {
        Some("DistantLight") => {
            // UsdLux spec default intensity is 1.0, but 1 lx is invisible
            // under Bevy's physically-based exposure — an unauthored
            // intensity almost certainly means "give me a sun", so default
            // to the calibrated 128 000 lx lunar sun and let authors override.
            let Ok((illuminance_lux, intensity_uses_graphics_default, intensity_scale)) =
                resolve_intensity_with_exposure(
                    reader,
                    sdf_path,
                    quality.distant_light_default_illuminance,
                )
            else {
                return false;
            };
            let Ok(c) = read_light_color(reader, sdf_path) else {
                return false;
            };
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
            let angular_diameter_deg = match read_authored_real(reader, sdf_path, "inputs:angle") {
                Ok(Some(angle)) if (0.0..=180.0).contains(&angle) => angle,
                Ok(Some(angle)) => {
                    error!(
                        "[usd-bevy] {} has invalid DistantLight inputs:angle = {angle}; expected a finite angle in [0, 180] degrees",
                        sdf_path.as_str()
                    );
                    return false;
                }
                Ok(None) => lunco_core::SOLAR_ANGULAR_DIAMETER_DEG,
                Err(_) => return false,
            };
            // Renderer settings supply defaults. Authored content overrides
            // only the two range attributes below, and the provenance marker
            // preserves that distinction for live Graphics edits.
            let first_cascade_authored =
                reader.has_authored_attribute(sdf_path, "lunco:shadow:firstCascadeFarBound");
            let first_cascade_far_bound = match read_authored_real(
                reader,
                sdf_path,
                "lunco:shadow:firstCascadeFarBound",
            ) {
                Ok(Some(distance)) => {
                    let metres = convention.length(distance as f64) as f32;
                    if metres.is_finite() {
                        metres
                    } else {
                        error!(
                            "[usd-bevy] {} has a first shadow cascade bound that is not finite after unit conversion",
                            sdf_path.as_str()
                        );
                        return false;
                    }
                }
                Ok(None) => d.first_cascade_far_bound,
                Err(_) => return false,
            };
            let Ok((maximum_distance, maximum_authored)) =
                read_shadow_distance(reader, sdf_path, d.maximum_distance, convention)
            else {
                return false;
            };
            if first_cascade_far_bound <= d.minimum_distance
                || first_cascade_far_bound >= maximum_distance
            {
                error!(
                    "[usd-bevy] {} has invalid shadow cascade bounds: minimum {}, first {}, maximum {}",
                    sdf_path.as_str(),
                    d.minimum_distance,
                    first_cascade_far_bound,
                    maximum_distance
                );
                return false;
            }
            let sun = LunarSunShadow {
                maximum_distance,
                first_cascade_far_bound,
                ..d
            };

            // A body's reflected fill authors `false` — see
            // `lunco://lighting/earthshine.usda`.
            let Ok(casts_shadows) = read_shadow_enable(reader, sdf_path) else {
                return false;
            };
            let Some(cascade_config) = sun.cascade_config() else {
                error!(
                    "[usd-bevy] {} has invalid resolved shadow cascade settings; refusing the DistantLight",
                    sdf_path.as_str()
                );
                return false;
            };

            commands.insert_resource(sun.shadow_map());
            commands.entity(entity).try_insert((
                lunco_core::SunAngularDiameter(angular_diameter_deg),
                sun.directional_light(color, illuminance_lux, casts_shadows),
                cascade_config,
                LightGraphicsDefaults {
                    intensity_uses_graphics_default,
                    intensity_scale,
                    range_uses_graphics_default: false,
                },
                ShadowRangeAuthorship {
                    first_cascade_far_bound: first_cascade_authored,
                    maximum_distance: maximum_authored,
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
            let env = match dome::read_dome_environment(
                reader,
                sdf_path,
                asset_server,
                stage_id,
                quality,
            ) {
                Ok(Some(env)) => env,
                Ok(None) => {
                    let Ok(intensity) = read_dome_intensity(reader, sdf_path, quality) else {
                        return false;
                    };
                    commands.entity(entity).try_insert((
                        UsdDomeAmbient {
                            value: intensity.value,
                            uses_graphics_default: intensity.uses_graphics_default,
                            exposure_scale: intensity.exposure_scale,
                        },
                        UsdAuthoredLight,
                    ));
                    debug!(
                        "[usd-bevy] {} DomeLight ambient={}",
                        sdf_path.as_str(),
                        intensity.value
                    );
                    return true;
                }
                Err(_) => return false,
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
            let Ok((base_lm, intensity_uses_graphics_default, exposure_scale)) =
                resolve_intensity_with_exposure(
                    reader,
                    sdf_path,
                    quality.local_light_default_intensity,
                )
            else {
                return false;
            };

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
            let Ok(default_sphere_radius) = positive_length(
                DEFAULT_SPHERE_RADIUS,
                convention,
                sdf_path,
                "SphereLight radius",
            ) else {
                return false;
            };
            let Ok(light_radius) = read_positive_length(
                reader,
                sdf_path,
                "inputs:radius",
                DEFAULT_SPHERE_RADIUS,
                convention,
            ) else {
                return false;
            };
            let Ok(normalize) = read_authored_bool(reader, sdf_path, "inputs:normalize") else {
                return false;
            };
            let normalize = normalize.unwrap_or(false);
            let radius_ratio = light_radius / default_sphere_radius;
            let Some(area_scale) = area_scale(normalize, radius_ratio.powi(2)) else {
                error!(
                    "[usd-bevy] {} has a SphereLight area scale that is not finite",
                    sdf_path.as_str()
                );
                return false;
            };
            // `inputs:intensity` is the authored photometric power. Do not impose an
            // importer-side ceiling: a lunar rover deliberately needs a much brighter
            // work beam than a terrestrial cabin lamp to remain visible at its camera
            // exposure, and clamping here silently changes the USD scene.
            let intensity_lm = base_lm * area_scale;
            if !intensity_lm.is_finite() {
                error!(
                    "[usd-bevy] {} has a SphereLight intensity that is not finite after area scaling",
                    sdf_path.as_str()
                );
                return false;
            }

            let Ok(c) = read_light_color(reader, sdf_path) else {
                return false;
            };
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
            let Ok(shadow_maps_enabled) = read_shadow_enable(reader, sdf_path) else {
                return false;
            };
            let Ok((range, range_uses_graphics_default)) = read_light_range(
                reader,
                sdf_path,
                quality.local_light_default_range,
                convention,
            ) else {
                return false;
            };

            let cone_angle_deg =
                match read_authored_real(reader, sdf_path, "inputs:shaping:cone:angle") {
                    Ok(angle) => angle,
                    Err(_) => return false,
                };
            if let Some(cone_angle_deg) = cone_angle_deg {
                // Spotlight path (UsdLuxShapingAPI applied)
                if !(0.0..90.0).contains(&cone_angle_deg) {
                    error!(
                        "[usd-bevy] {} has unsupported SphereLight cone angle {cone_angle_deg}; Bevy requires a finite angle in (0, 90) degrees",
                        sdf_path.as_str()
                    );
                    return false;
                }
                let softness = match read_authored_real(
                    reader,
                    sdf_path,
                    "inputs:shaping:cone:softness",
                ) {
                    Ok(Some(softness)) if (0.0..=1.0).contains(&softness) => softness,
                    Ok(Some(softness)) => {
                        error!(
                            "[usd-bevy] {} has invalid SphereLight cone softness {softness}; expected a finite value in [0, 1]",
                            sdf_path.as_str()
                        );
                        return false;
                    }
                    Ok(None) => 0.0,
                    Err(_) => return false,
                };
                let outer_angle = cone_angle_deg.to_radians();
                let inner_angle = outer_angle * (1.0 - softness);

                // No `UsdAuthoredLight`: a SphereLight is a *local* light (e.g. a
                // vessel headlight), not a scene-dominant sun/sky. Stamping it
                // would re-run the dome ambient recompute every time a rover
                // spawns — see the marker docs.
                // Its scene-property ports are marked pending immediately before
                // the deferred light insertion, then become ready atomically with
                // the component below.
                commands
                    .entity(entity)
                    .try_insert(lunco_core::PortSurfacePending);
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
                        radius: light_radius,
                        shadow_maps_enabled,
                        shadow_depth_bias: quality.shadow_depth_bias,
                        shadow_normal_bias: quality.shadow_normal_bias,
                        shadow_map_near_z: quality.local_shadow_map_near_z,
                        inner_angle,
                        outer_angle,
                        ..default()
                    },
                    LightGraphicsDefaults {
                        intensity_uses_graphics_default,
                        intensity_scale: exposure_scale * area_scale,
                        range_uses_graphics_default,
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
                    .try_insert(lunco_core::PortSurfacePending);
                commands
                    .entity(entity)
                    .try_remove::<lunco_core::PortSurfacePending>();
                commands.entity(entity).try_insert((
                    PointLight {
                        color,
                        intensity: intensity_lm,
                        range,
                        // See the SpotLight arm: `inputs:radius` is the source size too.
                        radius: light_radius,
                        shadow_maps_enabled,
                        shadow_depth_bias: quality.shadow_depth_bias,
                        shadow_normal_bias: quality.shadow_normal_bias,
                        shadow_map_near_z: quality.local_shadow_map_near_z,
                        ..default()
                    },
                    LightGraphicsDefaults {
                        intensity_uses_graphics_default,
                        intensity_scale: exposure_scale * area_scale,
                        range_uses_graphics_default,
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
            let Ok((base_lm, intensity_uses_graphics_default, exposure_scale)) =
                resolve_intensity_with_exposure(
                    reader,
                    sdf_path,
                    quality.rect_light_default_intensity,
                )
            else {
                return false;
            };
            let Ok(c) = read_light_color(reader, sdf_path) else {
                return false;
            };
            let color = Color::linear_rgb(c.x, c.y, c.z);
            // `inputs:width` / `inputs:height` are the UsdLuxRectLight schema's
            // own properties; 1 m square is the schema fallback.
            let Ok(width) = read_positive_length(reader, sdf_path, "inputs:width", 1.0, convention)
            else {
                return false;
            };
            let Ok(height) =
                read_positive_length(reader, sdf_path, "inputs:height", 1.0, convention)
            else {
                return false;
            };
            // `inputs:normalize` — the same UsdLux area rule the SphereLight arm
            // implements (see the long derivation there): with normalize OFF (the
            // schema default) `intensity` fixes radiance, so emitted power scales
            // with the emitting area. For a rect A = w·h, and the ratio against
            // the 1×1 m schema fallback makes an unauthored size exactly neutral.
            let Ok(normalize) = read_authored_bool(reader, sdf_path, "inputs:normalize") else {
                return false;
            };
            let normalize = normalize.unwrap_or(false);
            let Some(area_scale) = area_scale(normalize, width * height) else {
                error!(
                    "[usd-bevy] {} has a RectLight area that is not finite",
                    sdf_path.as_str()
                );
                return false;
            };
            let intensity_lm = base_lm * area_scale;
            if !intensity_lm.is_finite() {
                error!(
                    "[usd-bevy] {} has a RectLight intensity that is not finite after area scaling",
                    sdf_path.as_str()
                );
                return false;
            }
            let Ok((range, range_uses_graphics_default)) = read_light_range(
                reader,
                sdf_path,
                quality.local_light_default_range,
                convention,
            ) else {
                return false;
            };

            // `UsdLuxRectLight.inputs:texture:file` (an image mapped across the
            // rect) has no Bevy equivalent — say so rather than silently drop it.
            let texture_authored = reader.has_authored_attribute(sdf_path, "inputs:texture:file")
                || !reader
                    .connections(sdf_path, "inputs:texture:file")
                    .is_empty();
            let texture_value = reader.asset(sdf_path, "inputs:texture:file");
            if texture_authored && texture_value.is_none() {
                error!(
                    "[usd-bevy] {} has authored RectLight inputs:texture:file with an unsupported type",
                    sdf_path.as_str()
                );
                return false;
            }
            if texture_value.is_some_and(|p| !p.is_empty()) {
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
            commands.entity(entity).try_insert(LightGraphicsDefaults {
                intensity_uses_graphics_default,
                intensity_scale: exposure_scale * area_scale,
                range_uses_graphics_default,
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
        ambient.brightness = domes.iter().map(|d| d.value).sum();
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
        assert_eq!(untextured_dome_intensity_sum(&d, Some(&fill)), Ok(30.0));
    }

    #[test]
    fn textured_domes_never_contribute() {
        let d = data(SCENE);
        // Without excluding anything, the 500-intensity starfield must STILL be
        // absent — its image becomes IBL, not a scalar ambient term.
        assert_eq!(untextured_dome_intensity_sum(&d, None), Ok(30.0 + 70.0));
    }

    #[test]
    fn requested_100_over_an_existing_30_authors_70() {
        let d = data(SCENE);
        let fill = path("/World/Environment/AmbientFill");
        let others = untextured_dome_intensity_sum(&d, Some(&fill)).unwrap();
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
        assert_eq!(untextured_dome_intensity_sum(&d, None), Ok(0.0));
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
        assert_eq!(untextured_dome_intensity_sum(&d, None), Ok(5.0));
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
        assert_eq!(untextured_dome_intensity_sum(&d, None), Ok(64.0));
    }

    #[test]
    fn malformed_dome_inputs_are_rejected_instead_of_treated_as_omitted() {
        let d = data(
            r#"#usda 1.0

def DomeLight "Malformed"
{
    float inputs:intensity = 8
    string inputs:exposure = "not-a-number"
}
"#,
        );
        assert_eq!(untextured_dome_intensity_sum(&d, None), Err(LightReadError));
    }

    #[test]
    fn empty_texture_asset_is_scalar_ambient() {
        let d = data(
            r#"#usda 1.0

def DomeLight "Scalar"
{
    asset inputs:texture:file = @@
    float inputs:intensity = 5
}
"#,
        );
        assert_eq!(untextured_dome_intensity_sum(&d, None), Ok(5.0));
    }
}

#[cfg(test)]
mod photometry_tests {
    use super::*;
    use crate::canonical::{CanonicalStage, StageRecipe};

    #[test]
    fn rect_power_scales_with_area_unless_normalized() {
        // Schema-fallback 1×1 m is exactly neutral.
        assert_eq!(area_scale(false, 1.0), Some(1.0));
        // normalize OFF (the default): power scales with w·h.
        assert_eq!(area_scale(false, 2.0 * 3.0), Some(6.0));
        // normalize ON: authored intensity IS the power, whatever the size.
        assert_eq!(area_scale(true, 2.0 * 3.0), Some(1.0));
        // A negative or overflowing area is rejected rather than turned into a
        // plausible zero/finite light.
        assert_eq!(area_scale(false, -2.0 * 3.0), None);
        assert_eq!(area_scale(false, f32::INFINITY), None);
    }

    #[test]
    fn blackbody_is_white_at_6500k_warm_below_cool_above() {
        let white = blackbody_rgb(6500.0).unwrap();
        for ch in white.to_array() {
            assert!(ch > 0.9, "6500 K should be ≈ white, got {white:?}");
        }
        let warm = blackbody_rgb(2000.0).unwrap();
        assert_eq!(warm.x, 1.0);
        assert!(warm.z < 0.1, "2000 K should be orange, got {warm:?}");
        let cool = blackbody_rgb(10000.0).unwrap();
        assert_eq!(cool.z, 1.0);
        assert!(cool.x < 0.9, "10000 K should be blue, got {cool:?}");
        assert!(blackbody_rgb(1000.0).is_none());
        assert!(blackbody_rgb(30000.0).is_none());
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
        let convention = crate::units::stage_convention(&view).expect("valid stage convention");

        assert_eq!(
            read_light_range(&view, &lamp, 30.0, convention),
            Ok((0.9, false))
        );
        assert_eq!(
            convention.length(view.real_f32(&lamp, "inputs:radius").unwrap() as f64) as f32,
            0.02
        );
        assert_eq!(
            read_shadow_distance(&view, &lamp, 1500.0, convention),
            Ok((6.0, true))
        );
    }

    #[test]
    fn malformed_authored_light_values_are_rejected() {
        let source = r#"#usda 1.0

def DistantLight "Sun"
{
    string inputs:intensity = "invalid"
    string inputs:shadow:distance = "invalid"
    string inputs:shadow:enable = "invalid"
}
"#;
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", source))
            .expect("stage builds");
        let view = stage.view();
        let sun = SdfPath::new("/Sun").unwrap();

        assert!(read_intensity_with_exposure(&view, &sun, 77_000.0).is_err());
        assert!(read_shadow_distance(
            &view,
            &sun,
            1500.0,
            crate::units::stage_convention(&view).expect("valid stage convention"),
        )
        .is_err());
        assert!(read_shadow_enable(&view, &sun).is_err());
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
            Ok(77_000.0)
        );
        assert_eq!(
            read_intensity_with_exposure(&view, &SdfPath::new("/World/Bulb").unwrap(), 700.0),
            Ok(700.0)
        );
        assert_eq!(
            read_intensity_with_exposure(&view, &SdfPath::new("/World/Panel").unwrap(), 10_000.0),
            Ok(23.0)
        );
    }

    #[test]
    fn omitted_dome_intensity_uses_graphics_default_and_authored_intensity_wins() {
        let source = r#"#usda 1.0

def Xform "World"
{
    def DomeLight "Ambient"
    {
    }
    def DomeLight "Authored"
    {
        float inputs:intensity = 23
    }
}
"#;
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", source))
            .expect("stage builds");
        let view = stage.view();
        let quality = lunco_render::RenderingQuality::High.profile();

        assert_eq!(
            read_dome_intensity(&view, &SdfPath::new("/World/Ambient").unwrap(), quality),
            Ok(DomeIntensity {
                value: quality.dome_default_intensity,
                uses_graphics_default: true,
                exposure_scale: 1.0,
            })
        );
        assert_eq!(
            read_dome_intensity(&view, &SdfPath::new("/World/Authored").unwrap(), quality),
            Ok(DomeIntensity {
                value: 23.0,
                uses_graphics_default: false,
                exposure_scale: 1.0,
            })
        );
    }
}
