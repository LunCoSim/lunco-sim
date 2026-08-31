//! USD → celestial components (doc 49): maps the authored `lunco:anchor:*` /
//! `lunco:orbit:*` / `lunco:link:*` vocabulary to `lunco-celestial` components.
//! Called from `process_usd_sim_prim_read` (once per prim, either read source).
//!
//! There is no comms vocabulary and no comms component — a connectivity endpoint
//! is a generic [`LinkNode`](lunco_celestial::LinkNode), and the domain (roles,
//! routing, link budget) is authored on top of it.
//!
//! ```usda
//! bool   lunco:linkNode = true                  # a generic connectivity endpoint
//! string lunco:link:class = "relay"             # authored role; the core never reads it
//! double lunco:link:maxRangeM = 100000000
//! double lunco:link:minElevationDeg = 5
//! bool   lunco:occluder = true       # this geometry blocks sight-lines; the box
//!                                    # is the prim's core UsdGeom `extent`
//! double lunco:anchor:lat = 40.4314    # + lon/height (shared with terrain
//! int    lunco:anchor:body = 399       #   georef; body defaults to Moon 301)
//! int    lunco:orbit:body = 301
//! double lunco:orbit:semiMajorAxisM = 6540000   # + eccentricity/inclinationDeg/
//!                                               #   raanDeg/argPeriapsisDeg/
//!                                               #   meanAnomalyDeg/epochJd
//! int    lunco:libration:primary = 399          # a libration point of a PAIR:
//! int    lunco:libration:secondary = 301        #   Earth-Moon L1 (a parked relay)
//! token  lunco:libration:point = "L1"           #   L1..L5
//! ```
//!
//! A root prim (path depth 1) authoring an anchor is the scene's **site
//! anchor**: the local scene origin sits at that geodetic point (ENU axes) —
//! it grounds every scene-local endpoint (rover masts) on the body.

use bevy::prelude::*;

use lunco_celestial::frames::LPoint;
use lunco_celestial::geo::{Geodetic, GeodeticAnchor, SiteAnchor};
use lunco_celestial::kepler::{KeplerOrbit, KeplerianElements};
use lunco_celestial::transform::LibrationAnchor;
use openusd::sdf::{Path as SdfPath, Value};

type ComposedReader<'a> = dyn lunco_usd_bevy::read::UsdReadObject + 'a;

/// NAIF id of the default anchor body (the Moon).
const DEFAULT_ANCHOR_BODY: i32 = 301;

/// Read an authored real while preserving the distinction between an omitted
/// property (which may use the schema default) and a malformed opinion.  A
/// failed numeric conversion must never become a zero-valued placement input.
fn read_real_strict(
    reader: &ComposedReader<'_>,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<f64>, ()> {
    match reader.real(path, attribute) {
        Some(value) if value.is_finite() => Ok(Some(value)),
        Some(_) => Err(()),
        None if reader.has_authored_attribute(path, attribute) => Err(()),
        None => Ok(None),
    }
}

/// Read a schema-declared NAIF/body integer without turning a wrong USD type
/// into the Moon default.
fn read_i32_strict(
    reader: &ComposedReader<'_>,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<i32>, ()> {
    match reader.attr_value(path, attribute) {
        Some(Value::Int(value)) => Ok(Some(value)),
        Some(Value::Int64(value)) if i32::try_from(value).is_ok() => Ok(Some(value as i32)),
        None if reader.has_authored_attribute(path, attribute) => Err(()),
        Some(_) => Err(()),
        None => Ok(None),
    }
}

/// Read a value only when the scene authored this property.  Schema fallback
/// values are useful for ordinary fields, but they must not make a keyed
/// declaration look present (`trackedId = 0` is the classic example).
fn read_authored_i32(
    reader: &ComposedReader<'_>,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<i32>, ()> {
    if !reader.has_authored_attribute(path, attribute) {
        return Ok(None);
    }
    read_i32_strict(reader, path, attribute)?
        .ok_or(())
        .map(Some)
}

fn read_authored_real(
    reader: &ComposedReader<'_>,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<f64>, ()> {
    if !reader.has_authored_attribute(path, attribute) {
        return Ok(None);
    }
    read_real_strict(reader, path, attribute)?
        .ok_or(())
        .map(Some)
}

fn read_authored_bool(
    reader: &ComposedReader<'_>,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<bool>, ()> {
    if !reader.has_authored_attribute(path, attribute) {
        return Ok(None);
    }
    match reader.attr_value(path, attribute) {
        Some(Value::Bool(value)) => Ok(Some(value)),
        // Keep the shared reader's documented exporter tolerance for integer
        // booleans, but reject every other authored USD type.
        Some(Value::Int(value)) => Ok(Some(value != 0)),
        Some(Value::Int64(value)) => Ok(Some(value != 0)),
        _ => Err(()),
    }
}

fn read_authored_string(
    reader: &ComposedReader<'_>,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<String>, ()> {
    if !reader.has_authored_attribute(path, attribute) {
        return Ok(None);
    }
    match reader.attr_value(path, attribute) {
        Some(Value::String(value)) => Ok(Some(value)),
        _ => Err(()),
    }
}

fn read_authored_token(
    reader: &ComposedReader<'_>,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<String>, ()> {
    if !reader.has_authored_attribute(path, attribute) {
        return Ok(None);
    }
    match reader.attr_value(path, attribute) {
        Some(Value::Token(value)) => Ok(Some(value.to_string())),
        _ => Err(()),
    }
}

fn rgba_from_value(value: Value) -> Option<[f32; 4]> {
    let rgba = match value {
        Value::Vec4f(value) => [value.x, value.y, value.z, value.w],
        Value::Vec4d(value) => [
            value.x as f32,
            value.y as f32,
            value.z as f32,
            value.w as f32,
        ],
        _ => return None,
    };
    rgba.iter().all(|value| value.is_finite()).then_some(rgba)
}

fn read_authored_rgba(
    reader: &ComposedReader<'_>,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<[f32; 4]>, ()> {
    if !reader.has_authored_attribute(path, attribute) {
        return Ok(None);
    }
    reader
        .attr_value(path, attribute)
        .and_then(rgba_from_value)
        .ok_or(())
        .map(Some)
}

fn read_positive_optional_real(
    reader: &ComposedReader<'_>,
    path: &SdfPath,
    attribute: &str,
) -> Result<Option<f64>, ()> {
    match read_authored_real(reader, path, attribute)? {
        None | Some(0.0) => Ok(None),
        Some(value) if value.is_finite() && value > 0.0 => Ok(Some(value)),
        Some(_) => Err(()),
    }
}

/// Decode a geodetic anchor as one validated projection unit.  Missing latitude
/// or longitude remains the documented zero default, but an authored invalid
/// value rejects the anchor instead of placing it at Greenwich/equator.
fn read_geodetic_anchor(
    reader: &ComposedReader<'_>,
    path: &SdfPath,
) -> Result<Option<GeodeticAnchor>, ()> {
    let has_lat = reader.has_authored_attribute(path, "lunco:anchor:lat");
    let has_lon = reader.has_authored_attribute(path, "lunco:anchor:lon");
    if !has_lat && !has_lon {
        return Ok(None);
    }
    let lat = read_real_strict(reader, path, "lunco:anchor:lat")?.unwrap_or(0.0);
    let lon = read_real_strict(reader, path, "lunco:anchor:lon")?.unwrap_or(0.0);
    let height = read_real_strict(reader, path, "lunco:anchor:height")?.unwrap_or(0.0);
    let body = read_i32_strict(reader, path, "lunco:anchor:body")?.unwrap_or(DEFAULT_ANCHOR_BODY);
    if body == 0 || !(-90.0..=90.0).contains(&lat) || !height.is_finite() {
        return Err(());
    }
    Ok(Some(GeodeticAnchor {
        body,
        geodetic: Geodetic::new(lat, lon, height),
    }))
}

/// Decode one authored Kepler orbit.  The semi-major axis is the schema's
/// explicit presence key; zero/omitted means no orbit.  Every remaining field
/// gets its USD schema default only when it is genuinely omitted, and the
/// elliptic-only solver contract is validated before insertion.
fn read_kepler_orbit(
    reader: &ComposedReader<'_>,
    path: &SdfPath,
) -> Result<Option<KeplerOrbit>, ()> {
    if !reader.has_authored_attribute(path, "lunco:orbit:semiMajorAxisM") {
        return Ok(None);
    }
    let Some(semi_major_axis_m) = read_real_strict(reader, path, "lunco:orbit:semiMajorAxisM")?
    else {
        return Ok(None);
    };
    if semi_major_axis_m == 0.0 {
        return Ok(None);
    }
    let body = read_i32_strict(reader, path, "lunco:orbit:body")?.unwrap_or(DEFAULT_ANCHOR_BODY);
    let eccentricity = read_real_strict(reader, path, "lunco:orbit:eccentricity")?.unwrap_or(0.0);
    let inclination_deg =
        read_real_strict(reader, path, "lunco:orbit:inclinationDeg")?.unwrap_or(0.0);
    let raan_deg = read_real_strict(reader, path, "lunco:orbit:raanDeg")?.unwrap_or(0.0);
    let arg_periapsis_deg =
        read_real_strict(reader, path, "lunco:orbit:argPeriapsisDeg")?.unwrap_or(0.0);
    let mean_anomaly_deg =
        read_real_strict(reader, path, "lunco:orbit:meanAnomalyDeg")?.unwrap_or(0.0);
    let epoch_jd =
        read_real_strict(reader, path, "lunco:orbit:epochJd")?.unwrap_or(lunco_time::J2000_JD);
    if body == 0
        || !semi_major_axis_m.is_finite()
        || semi_major_axis_m < 0.0
        || !eccentricity.is_finite()
        || !(0.0..1.0).contains(&eccentricity)
        || !inclination_deg.is_finite()
        || !raan_deg.is_finite()
        || !arg_periapsis_deg.is_finite()
        || !mean_anomaly_deg.is_finite()
        || !epoch_jd.is_finite()
    {
        return Err(());
    }
    Ok(Some(KeplerOrbit {
        body,
        elements: KeplerianElements {
            semi_major_axis_m,
            eccentricity,
            inclination_deg,
            raan_deg,
            arg_periapsis_deg,
            mean_anomaly_deg,
            epoch_jd,
        },
    }))
}

pub fn insert_celestial_comms_components(
    reader: &ComposedReader<'_>,
    entity: Entity,
    prim_path_str: &str,
    sdf_path: &SdfPath,
    commands: &mut Commands,
) {
    // --- Celestial body declaration (LunCoCelestialBodyAPI) ---
    //
    // The scene says which bodies exist; Rust does not. A prim authoring
    // `int lunco:body = 399` IS the Earth, and its presence is what turns the whole
    // celestial stack on (`lunco_celestial::celestial_declared`). No such prim ⇒ no
    // sky. This replaces `CelestialConfig.spawn_hierarchy`, a code-side boolean that
    // a scene could only trip as a side effect, never actually *request*.
    match read_i32_strict(reader, sdf_path, "lunco:body") {
        Ok(Some(naif)) if naif != 0 => {
            commands
                .entity(entity)
                .try_insert(lunco_celestial::CelestialBodyDecl { naif });
            info!("[usd-celestial] scene declares celestial body {naif} at {prim_path_str}");
        }
        Ok(Some(_)) | Ok(None) => {}
        Err(()) => {
            warn!(
                "[usd-celestial] {} has malformed `lunco:body`; celestial body declaration refused",
                prim_path_str
            );
        }
    }

    // --- A body's reflected fill (earthshine and its analogues) ---
    //
    // A `DistantLight` nested UNDER a celestial body prim is that body's
    // reflected light — earthshine at the Moon, Jupiter-shine at Europa. The
    // namespace is the whole declaration: the parent already says which body it
    // is (`lunco:body`), so the light needs no attribute to state what it
    // belongs to, and the rule generalises to any body without a second schema.
    //
    // This is what makes "which light is the sun?" answerable STRUCTURALLY. The
    // scene's key light is a top-level `DistantLight`; a body's fill hangs under
    // that body. The sun-pickers filter on this marker instead of taking
    // whichever `DirectionalLight` is brightest — a guess that silently picked
    // the wrong light whenever a scene had two, and picked by archetype
    // iteration order whenever two were equally bright.
    //
    // The fill used to be spawned from Rust at startup, which is why it had no
    // USD identity to filter on in the first place.
    if reader.type_name(sdf_path).as_deref() == Some("DistantLight") {
        let parent_is_body = match sdf_path.parent() {
            Some(parent) => match read_i32_strict(reader, &parent, "lunco:body") {
                Ok(Some(naif)) => naif != 0,
                Ok(None) => false,
                Err(()) => {
                    warn!(
                        "[usd-celestial] {} has malformed parent `lunco:body`; body fill classification refused",
                        prim_path_str
                    );
                    false
                }
            },
            None => false,
        };
        if parent_is_body {
            // WEB: WebGL2 supports ONE `DirectionalLight`, and a second culls
            // the sun — so on wasm the fill is dropped rather than composed. The
            // scene still AUTHORS it; the platform declines to realise it, which
            // keeps the asset identical across targets.
            #[cfg(target_arch = "wasm32")]
            {
                commands.entity(entity).try_despawn();
                info!("[usd-celestial] {prim_path_str}: body fill dropped (WebGL2 single-light)");
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                commands
                    .entity(entity)
                    .try_insert(lunco_environment::Earthshine);
                info!("[usd-celestial] {prim_path_str} is a body fill light (not the scene sun)");
            }
        }
    }

    // --- Body imagery authored on the body prim ---
    //
    // `asset lunco:body:albedoMap = @lunco://textures/earth.png@` — which map a
    // body wears is scene content, so a Twin can dress its own Earth without
    // touching the engine's `Assets.toml`. Read as an ASSET, not a string:
    // `sdf::Value` keeps those distinct and a `text()` read of an asset-typed
    // attribute silently yields nothing.
    if let Some(albedo) = reader.asset(sdf_path, "lunco:body:albedoMap") {
        if !albedo.is_empty() {
            commands
                .entity(entity)
                .try_insert(lunco_celestial::AuthoredBodyAlbedo { asset: albedo });
        }
    }

    // --- Geodetic anchor (ground stations + scene site anchor) ---
    // Terrain owns the same authored lat/lon as a DEM georeference, but that
    // data is not a second ECS placement. `TerrainGeoref` is projected by the
    // terrain bridge and the terrain remains in its site scene branch; giving
    // it `GeodeticAnchor` here would make the celestial placement pass detach
    // it from the scene and double-place the surface relative to the rover.
    let is_terrain = reader.has_api_schema(sdf_path, "LunCoTerrainAPI");
    let anchor = if is_terrain {
        Ok(None)
    } else {
        read_geodetic_anchor(reader, sdf_path)
    };
    match anchor {
        Ok(Some(anchor)) => {
            commands.entity(entity).try_insert(anchor);
            // Root prim anchor = the scene's site frame.
            let is_root = prim_path_str.matches('/').count() == 1 && prim_path_str.starts_with('/');
            if is_root {
                commands.entity(entity).try_insert(SiteAnchor);
                info!(
                    "[usd-celestial] site anchor {}: body {} lat {:.4} lon {:.4} h {:.1} m",
                    prim_path_str,
                    anchor.body,
                    anchor.geodetic.lat_deg,
                    anchor.geodetic.lon_deg,
                    anchor.geodetic.height_m
                );
                // Scene-authored date: `double lunco:time:epochJd` picks the world
                // epoch (e.g. one where a polar site is sunlit — at Shackleton the
                // real sun crosses the horizon on a ~monthly cycle, so an unlucky
                // "now" default renders the whole demo pitch-black).
                match read_real_strict(reader, sdf_path, "lunco:time:epochJd") {
                    Ok(Some(epoch_jd)) if epoch_jd != 0.0 => {
                        info!("[usd-celestial] scene epoch: JD {epoch_jd:.4}");
                        commands.trigger(lunco_time::SetMissionEpoch { epoch_jd });
                    }
                    Ok(_) => {}
                    Err(()) => warn!(
                        "[usd-celestial] {} has malformed `lunco:time:epochJd`; authored epoch ignored",
                        prim_path_str
                    ),
                }
            }
        }
        Ok(None) => {}
        Err(()) => warn!(
            "[usd-celestial] {} has malformed geodetic anchor attributes; anchor ignored",
            prim_path_str
        ),
    }

    // --- Mission declaration (LunCoMissionAPI) ---
    //
    // A mission is OPT-IN per scene, and separately from the sky: declaring bodies
    // says "this world has a Moon", not "spawn Artemis II into my landing film".
    // Missions used to be loaded by scanning `assets/missions/*.json` whenever ANY
    // celestial body was declared, so every lunar scene silently acquired every
    // mission on disk. Now a scene asks by referencing the mission's USD file, the
    // same way it asks for a sky by referencing `solar_system.usda`.
    //
    // Keyed on `lunco:mission:id` — the identifying attribute, following the
    // libration/orbit convention above. A prim without one is not a half-declared
    // mission, it is simply not a mission.
    let mission_id = match read_authored_string(reader, sdf_path, "lunco:mission:id") {
        Ok(Some(id)) if !id.is_empty() => Some(id),
        Ok(None) => None,
        Ok(Some(_)) => {
            warn!(
                "[usd-celestial] {} has an empty mission id; declaration ignored",
                prim_path_str
            );
            None
        }
        Err(()) => {
            warn!(
                "[usd-celestial] {} has malformed mission attributes; declaration ignored",
                prim_path_str
            );
            None
        }
    };
    if let Some(id) = mission_id {
        let mission = (|| {
            let name = read_authored_string(reader, sdf_path, "lunco:mission:name")?
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| id.clone());
            let description = read_authored_string(reader, sdf_path, "lunco:mission:description")?
                .unwrap_or_default();
            Ok(lunco_celestial::MissionDecl {
                id: id.clone(),
                name,
                description,
            })
        })();
        match mission {
            Ok(mission) => {
                let name = mission.name.clone();
                commands.entity(entity).try_insert(mission);
                info!("[usd-celestial] scene declares mission {name} ({id}) at {prim_path_str}");
            }
            Err(()) => warn!(
                "[usd-celestial] {} has invalid mission attributes; declaration ignored",
                prim_path_str
            ),
        }
    }

    // --- Mission trajectory (LunCoMissionTrajectoryAPI) ---
    //
    // VISUALISATION parameters only. The state vectors are NOT here and never were:
    // the curve is sampled at runtime from the ephemeris provider keyed by
    // `trackedId`/`referenceId`, so this prim says how to DRAW a trajectory, not
    // where the spacecraft is. Keyed on `trackedId` — without a target there is
    // nothing to plot.
    let trajectory_target = match read_authored_i32(reader, sdf_path, "lunco:trajectory:trackedId")
    {
        Ok(Some(id)) if id != 0 => Some(id),
        Ok(None) => None,
        Ok(Some(id)) => {
            warn!(
                "[usd-celestial] {} has invalid trajectory tracked id {}; declaration ignored",
                prim_path_str, id
            );
            None
        }
        Err(()) => {
            warn!(
                "[usd-celestial] {} has malformed trajectory attributes; declaration ignored",
                prim_path_str
            );
            None
        }
    };
    if let Some(tracked_id) = trajectory_target {
        let trajectory = (|| {
            let reference_id = read_authored_i32(reader, sdf_path, "lunco:trajectory:referenceId")?
                .unwrap_or(DEFAULT_ANCHOR_BODY);
            if reference_id == 0 {
                return Err(());
            }
            let color = read_authored_rgba(reader, sdf_path, "lunco:trajectory:color")?
                .unwrap_or([1.0, 1.0, 1.0, 1.0]);
            let sampling_days =
                read_authored_real(reader, sdf_path, "lunco:trajectory:samplingDays")?
                    .unwrap_or(1.0);
            let sampling_step =
                read_authored_real(reader, sdf_path, "lunco:trajectory:samplingStep")?
                    .unwrap_or(0.01);
            if !sampling_days.is_finite()
                || sampling_days <= 0.0
                || !sampling_step.is_finite()
                || sampling_step <= 0.0
            {
                return Err(());
            }
            let frame = read_authored_token(reader, sdf_path, "lunco:trajectory:frame")?
                .unwrap_or_else(|| "Inertial".to_string());
            if !matches!(frame.as_str(), "Inertial" | "BodyFixed") {
                return Err(());
            }
            let user_visible =
                read_authored_bool(reader, sdf_path, "lunco:trajectory:userVisible")?;
            let start_epoch_jd =
                read_authored_real(reader, sdf_path, "lunco:trajectory:startEpochJd")?
                    .and_then(|value| (value != 0.0).then_some(value));
            let end_epoch_jd = read_authored_real(reader, sdf_path, "lunco:trajectory:endEpochJd")?
                .and_then(|value| (value != 0.0).then_some(value));
            if start_epoch_jd.is_some() != end_epoch_jd.is_some()
                || matches!((start_epoch_jd, end_epoch_jd), (Some(start), Some(end)) if end < start)
            {
                return Err(());
            }
            let name = read_authored_string(reader, sdf_path, "lunco:trajectory:name")?
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| prim_path_str.to_string());
            Ok(lunco_celestial::MissionTrajectoryDecl {
                name,
                tracked_id,
                reference_id,
                color,
                sampling_days,
                sampling_step,
                frame,
                user_visible,
                start_epoch_jd,
                end_epoch_jd,
            })
        })();
        match trajectory {
            Ok(trajectory) => {
                commands.entity(entity).try_insert(trajectory);
                info!("[usd-celestial] mission trajectory {prim_path_str}: target {tracked_id}");
            }
            Err(()) => warn!(
                "[usd-celestial] {} has invalid mission trajectory attributes; declaration ignored",
                prim_path_str
            ),
        }
    }

    // --- Mission spacecraft marker (LunCoMissionSpacecraftAPI) ---
    //
    // Keyed on `ephemerisId`: the marker's whole job is to sit where the ephemeris
    // says that body is, so a prim naming no body is unplaceable, not defaulted.
    let spacecraft_id = match read_authored_i32(reader, sdf_path, "lunco:spacecraft:ephemerisId") {
        Ok(Some(id)) if id != 0 => Some(id),
        Ok(None) => None,
        Ok(Some(id)) => {
            warn!(
                "[usd-celestial] {} has invalid spacecraft ephemeris id {}; declaration ignored",
                prim_path_str, id
            );
            None
        }
        Err(()) => {
            warn!(
                "[usd-celestial] {} has malformed spacecraft attributes; declaration ignored",
                prim_path_str
            );
            None
        }
    };
    if let Some(ephemeris_id) = spacecraft_id {
        let spacecraft = (|| {
            let reference_id = read_authored_i32(reader, sdf_path, "lunco:spacecraft:referenceId")?
                .unwrap_or(DEFAULT_ANCHOR_BODY);
            if reference_id == 0 {
                return Err(());
            }
            let scale =
                read_real_strict(reader, sdf_path, "lunco:spacecraft:scale")?.unwrap_or(1.0);
            if !scale.is_finite() || scale <= 0.0 {
                return Err(());
            }
            let scale = scale as f32;
            if !scale.is_finite() || scale <= 0.0 {
                return Err(());
            }
            let start_epoch_jd =
                read_authored_real(reader, sdf_path, "lunco:spacecraft:startEpochJd")?
                    .and_then(|value| (value != 0.0).then_some(value));
            let end_epoch_jd = read_authored_real(reader, sdf_path, "lunco:spacecraft:endEpochJd")?
                .and_then(|value| (value != 0.0).then_some(value));
            if start_epoch_jd.is_some() != end_epoch_jd.is_some()
                || matches!((start_epoch_jd, end_epoch_jd), (Some(start), Some(end)) if end < start)
            {
                return Err(());
            }
            let marker_radius_km =
                read_positive_optional_real(reader, sdf_path, "lunco:spacecraft:markerRadiusKm")?;
            let hit_radius_km =
                read_positive_optional_real(reader, sdf_path, "lunco:spacecraft:hitRadiusKm")?;
            let name = read_authored_string(reader, sdf_path, "lunco:spacecraft:name")?
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| prim_path_str.to_string());
            let marker_color =
                read_authored_rgba(reader, sdf_path, "lunco:spacecraft:markerColor")?;
            let marker_radius_km = marker_radius_km.map(|value| value as f32);
            let hit_radius_km = hit_radius_km.map(|value| value as f32);
            if marker_radius_km.is_some_and(|value| !value.is_finite() || value <= 0.0)
                || hit_radius_km.is_some_and(|value| !value.is_finite() || value <= 0.0)
            {
                return Err(());
            }
            Ok(lunco_celestial::MissionSpacecraftDecl {
                name,
                ephemeris_id,
                reference_id,
                scale,
                start_epoch_jd,
                end_epoch_jd,
                marker_radius_km,
                hit_radius_km,
                marker_color,
            })
        })();
        match spacecraft {
            Ok(spacecraft) => {
                commands.entity(entity).try_insert(spacecraft);
                info!(
                    "[usd-celestial] mission spacecraft {prim_path_str}: ephemeris {ephemeris_id}"
                );
            }
            Err(()) => warn!(
                "[usd-celestial] {} has invalid mission spacecraft attributes; declaration ignored",
                prim_path_str
            ),
        }
    }

    // --- Keplerian orbit (satellites) ---
    match read_kepler_orbit(reader, sdf_path) {
        Ok(Some(orbit)) => {
            let body = orbit.body;
            let elements = orbit.elements;
            commands.entity(entity).try_insert(orbit);
            info!(
                "[usd-celestial] orbit {}: body {} a {:.0} km e {:.2} i {:.1}°",
                prim_path_str,
                body,
                elements.semi_major_axis_m / 1000.0,
                elements.eccentricity,
                elements.inclination_deg
            );
        }
        Ok(None) => {}
        Err(()) => warn!(
            "[usd-celestial] {} has malformed Kepler orbit attributes; orbit ignored",
            prim_path_str
        ),
    }

    // --- Libration point (a relay parked at L1/L2 of a pair) ---
    //
    // The third placement kind, beside geodetic (on a body) and Kepler (around one).
    // Keyed on `primary`, since an L-point is defined by a PAIR — the pair IS the
    // placement, so a prim naming only one body is not half-placed, it is unplaced.
    let libration = match read_authored_i32(reader, sdf_path, "lunco:libration:primary") {
        Ok(None) => Ok(None),
        Ok(Some(primary)) if primary != 0 => (|| {
            let secondary =
                read_authored_i32(reader, sdf_path, "lunco:libration:secondary")?.ok_or(())?;
            if secondary == 0 {
                return Err(());
            }
            let token = read_authored_token(reader, sdf_path, "lunco:libration:point")?
                .unwrap_or_else(|| "L1".to_string());
            let point = LPoint::from_token(&token).ok_or(())?;
            Ok(LibrationAnchor {
                primary,
                secondary,
                point,
            })
        })()
        .map(Some),
        Ok(Some(_)) | Err(()) => Err(()),
    };
    match libration {
        Ok(Some(anchor)) => {
            info!(
                "[usd-celestial] libration {}: {:?} of pair {}/{}",
                prim_path_str, anchor.point, anchor.primary, anchor.secondary
            );
            commands.entity(entity).try_insert(anchor);
        }
        Ok(None) => {}
        Err(()) => warn!(
            "[usd-celestial] {} has malformed libration attributes; anchor ignored",
            prim_path_str
        ),
    }

    // --- Solar-pose tracking marker (generic celestial placement) ---
    // A scene-local subsystem prim (a rover-mounted antenna, a panel) opts in so
    // the pose system tracks its solar-frame position; anchored/orbiting prims
    // are tracked automatically. Authored subsystems read it through the
    // `SolarPose` query — no domain component, no domain vocabulary.
    let solar_tracked = match read_authored_bool(reader, sdf_path, "lunco:solarTracked") {
        Ok(Some(value)) => value,
        Ok(None) => false,
        Err(()) => {
            warn!(
                "[usd-celestial] {} has malformed `lunco:solarTracked`; tracking ignored",
                prim_path_str
            );
            false
        }
    };
    if solar_tracked {
        commands
            .entity(entity)
            .try_insert(lunco_celestial::pose::SolarTracked);
    }

    // --- Connectivity node (generic link kernel) ---
    // Marks a prim as a link endpoint: the kernel pairs it with every other
    // node, applies the `link.connected` verdict, and publishes link state. Pose
    // tracking follows automatically. `class` is an authored role the routing /
    // verdict policy reads — the core never interprets it.
    let link_node = match read_authored_bool(reader, sdf_path, "lunco:linkNode") {
        Ok(Some(value)) => value,
        Ok(None) => false,
        Err(()) => {
            warn!(
                "[usd-celestial] {} has malformed `lunco:linkNode`; link node ignored",
                prim_path_str
            );
            false
        }
    };
    if link_node {
        let link = (|| {
            let defaults = lunco_celestial::link::LinkNode::default();
            let max_range_m = read_real_strict(reader, sdf_path, "lunco:link:maxRangeM")?
                .unwrap_or(defaults.max_range_m);
            let min_elevation_deg =
                read_real_strict(reader, sdf_path, "lunco:link:minElevationDeg")?
                    .unwrap_or(defaults.min_elevation_deg);
            if !max_range_m.is_finite()
                || max_range_m <= 0.0
                || !min_elevation_deg.is_finite()
                || !(-90.0..=90.0).contains(&min_elevation_deg)
            {
                return Err(());
            }
            let class = read_authored_string(reader, sdf_path, "lunco:link:class")?
                .filter(|class| !class.is_empty());
            Ok(lunco_celestial::link::LinkNode {
                max_range_m,
                min_elevation_deg,
                class,
            })
        })();
        match link {
            Ok(link) => {
                commands.entity(entity).try_insert(link);
            }
            Err(()) => warn!(
                "[usd-celestial] {} has invalid link node attributes; node ignored",
                prim_path_str
            ),
        }
    }

    // --- Separate rover radio endpoint ---
    // A Wi-Fi endpoint shares the generic link geometry observation but is
    // intentionally not part of `LinkState`: the latter remains governed by
    // the authored direct-link policy.
    let wifi_node = match read_authored_bool(reader, sdf_path, "lunco:wifiNode") {
        Ok(Some(value)) => value,
        Ok(None) => false,
        Err(()) => {
            warn!(
                "[usd-celestial] {} has malformed `lunco:wifiNode`; Wi-Fi node ignored",
                prim_path_str
            );
            false
        }
    };
    if wifi_node {
        let wifi = (|| {
            let max_range_m =
                read_real_strict(reader, sdf_path, "lunco:wifi:maxRangeM")?.unwrap_or(5_000.0);
            if !max_range_m.is_finite() || max_range_m <= 0.0 {
                return Err(());
            }
            Ok(lunco_celestial::wifi::WifiNode { max_range_m })
        })();
        match wifi {
            Ok(wifi) => {
                commands.entity(entity).try_insert(wifi);
            }
            Err(()) => warn!(
                "[usd-celestial] {} has invalid Wi-Fi node attributes; node ignored",
                prim_path_str
            ),
        }
    }

    // --- Sight-line occluder (generic geometry, not a comms concept) ---
    // Marks THIS prim's geometry as opaque to link sight-lines: any link whose
    // segment crosses its box is severed. Author it on the geometry prim that
    // actually blocks (the child `Cube`, not its parent `Xform`), so the box
    // inherits that prim's pose, scale and extent.
    //
    // The BOX IS THE PRIM'S `extent` — core UsdGeom, no invented size vocabulary.
    // If a Cube omits its computed extent from the layer, the projector derives
    // it from the standard `size` attribute. Other geometry must author `extent`.
    //
    // NOT derived from `PhysicsCollisionAPI`: opacity is a material property, not
    // a collision one (a handrail collides but does not block; a radome may block
    // but not collide). See `LinkOccluder`.
    let occluder = match read_authored_bool(reader, sdf_path, "lunco:occluder") {
        Ok(Some(value)) => value,
        Ok(None) => false,
        Err(()) => {
            warn!(
                "[usd-celestial] {} has malformed `lunco:occluder`; occluder ignored",
                prim_path_str
            );
            false
        }
    };
    if occluder {
        match read_occluder_box(reader, sdf_path) {
            Ok(occluder) => {
                commands.entity(entity).try_insert((
                    occluder,
                    // LinkOccluder is tested against LinkNode::SolarFramePose.
                    // The marker makes that frame an explicit projection
                    // contract for every authored blocker, including a
                    // scene-local wall.
                    lunco_celestial::pose::SolarTracked,
                ));
            }
            Err(()) => warn!(
                "[usd-celestial] {} has malformed UsdGeom `extent`; occluder ignored",
                prim_path_str
            ),
        }
    }
}

/// The occluding box from the prim's UsdGeom `extent` (`float3[2]` — min, max).
/// A cube may omit its computed extent; in that case the standard `size` attribute
/// is the authoritative source for the local box. Both are pre-scale, in the
/// prim's local space; the kernel applies the `Transform` scale.
fn read_occluder_box(
    reader: &ComposedReader<'_>,
    sdf_path: &SdfPath,
) -> Result<lunco_celestial::link::LinkOccluder, ()> {
    use bevy::math::DVec3;
    if !reader.has_authored_attribute(sdf_path, "extent") {
        // `extent` is a computed/cacheable UsdGeom attribute. Its schema fallback
        // is not an authored opinion and is therefore not valid geometry for a
        // custom `size` (the shipped wall is size 1, while Cube's fallback extent
        // describes size 2). Derive only the standard cube case; every other
        // occluder must author the standard extent explicitly.
        if reader.type_name(sdf_path).as_deref() != Some("Cube") {
            return Err(());
        }
        let Some(size) = reader.real(sdf_path, "size") else {
            return Err(());
        };
        if !size.is_finite() || size <= 0.0 {
            return Err(());
        }
        return Ok(lunco_celestial::link::LinkOccluder {
            half_extents: DVec3::splat(size * 0.5),
            center: DVec3::ZERO,
        });
    }
    let Some(value) = reader.attr_value(sdf_path, "extent") else {
        return Err(());
    };
    let (min, max) = match value {
        Value::Vec3fVec(values) if values.len() == 2 => (
            DVec3::new(
                f64::from(values[0].x),
                f64::from(values[0].y),
                f64::from(values[0].z),
            ),
            DVec3::new(
                f64::from(values[1].x),
                f64::from(values[1].y),
                f64::from(values[1].z),
            ),
        ),
        Value::Vec3dVec(values) if values.len() == 2 => (
            DVec3::new(values[0].x, values[0].y, values[0].z),
            DVec3::new(values[1].x, values[1].y, values[1].z),
        ),
        _ => return Err(()),
    };
    let half_extents = (max - min) * 0.5;
    if !min.is_finite()
        || !max.is_finite()
        || !half_extents.is_finite()
        || half_extents.x <= 0.0
        || half_extents.y <= 0.0
        || half_extents.z <= 0.0
    {
        return Err(());
    }
    Ok(lunco_celestial::link::LinkOccluder {
        half_extents,
        center: (max + min) * 0.5,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_usd_bevy::{CanonicalStage, StageRecipe};

    fn view(source: &str) -> (CanonicalStage, SdfPath) {
        let stage = CanonicalStage::from_recipe(&StageRecipe::from_source("scene.usda", source))
            .expect("test stage must compose");
        let path = SdfPath::new("/World/Body").expect("test path");
        (stage, path)
    }

    #[test]
    fn malformed_anchor_values_are_not_replaced_with_zeroes() {
        let (stage, path) = view(
            r#"#usda 1.0
def Xform "World"
{
    def Xform "Body"
    {
        string lunco:anchor:lat = "north"
        double lunco:anchor:lon = 12.0
    }
}
"#,
        );
        assert!(
            read_geodetic_anchor(&stage.view(), &path).is_err(),
            "a malformed latitude must not become latitude zero"
        );
    }

    #[test]
    fn omitted_anchor_is_not_invented() {
        let (stage, path) = view(
            r#"#usda 1.0
def Xform "World"
{
    def Xform "Body" {}
}
"#,
        );
        assert!(matches!(
            read_geodetic_anchor(&stage.view(), &path),
            Ok(None)
        ));
    }

    #[test]
    fn malformed_orbit_eccentricity_is_not_clamped() {
        let (stage, path) = view(
            r#"#usda 1.0
def Xform "World"
{
    def Xform "Body"
    {
        double lunco:orbit:semiMajorAxisM = 7000000.0
        double lunco:orbit:eccentricity = 1.2
    }
}
"#,
        );
        assert!(
            read_kepler_orbit(&stage.view(), &path).is_err(),
            "the elliptic-only solver contract must reject e >= 1"
        );
    }

    #[test]
    fn omitted_orbit_elements_keep_only_documented_defaults() {
        let (stage, path) = view(
            r#"#usda 1.0
def Xform "World"
{
    def Xform "Body"
    {
        double lunco:orbit:semiMajorAxisM = 7000000.0
    }
}
"#,
        );
        let orbit = read_kepler_orbit(&stage.view(), &path)
            .expect("valid orbit")
            .expect("semi-major axis opts in");
        assert_eq!(orbit.body, DEFAULT_ANCHOR_BODY);
        assert_eq!(orbit.elements.semi_major_axis_m, 7000000.0);
        assert_eq!(orbit.elements.eccentricity, 0.0);
        assert_eq!(orbit.elements.epoch_jd, lunco_time::J2000_JD);
    }

    #[test]
    fn color4f_is_read_as_a_fixed_usd_vector() {
        let (stage, path) = view(
            r#"#usda 1.0
def Xform "World"
{
    def Xform "Body"
    {
        color4f lunco:trajectory:color = (0.1, 0.2, 0.3, 0.4)
    }
}
"#,
        );
        assert_eq!(
            read_authored_rgba(&stage.view(), &path, "lunco:trajectory:color")
                .expect("valid color"),
            Some([0.1, 0.2, 0.3, 0.4])
        );
    }

    #[test]
    fn negative_ephemeris_ids_are_preserved() {
        let (stage, path) = view(
            r#"#usda 1.0
def Xform "World"
{
    def Xform "Body"
    {
        int lunco:trajectory:trackedId = -1024
    }
}
"#,
        );
        assert_eq!(
            read_authored_i32(&stage.view(), &path, "lunco:trajectory:trackedId")
                .expect("valid ephemeris id"),
            Some(-1024)
        );
    }

    #[test]
    fn authored_usd_extent_is_used_for_occluder_geometry() {
        let (stage, path) = view(
            r#"#usda 1.0
def Xform "World"
{
    def Cube "Body"
    {
        float3[] extent = [(-2, -3, -4), (2, 3, 4)]
    }
}
"#,
        );
        let occluder = read_occluder_box(&stage.view(), &path).expect("valid extent");
        assert_eq!(occluder.center, bevy::math::DVec3::ZERO);
        assert_eq!(occluder.half_extents, bevy::math::DVec3::new(2.0, 3.0, 4.0));
    }

    #[test]
    fn malformed_usd_extent_is_not_replaced_with_a_unit_cube() {
        let (stage, path) = view(
            r#"#usda 1.0
def Xform "World"
{
    def Cube "Body"
    {
        float3[] extent = [(-2, -3, -4)]
    }
}
"#,
        );
        assert!(read_occluder_box(&stage.view(), &path).is_err());
    }

    #[test]
    fn omitted_cube_extent_uses_the_authored_standard_size() {
        let (stage, path) = view(
            r#"#usda 1.0
def Xform "World"
{
    def Cube "Body"
    {
        double size = 1.0
    }
}
"#,
        );
        let occluder = read_occluder_box(&stage.view(), &path).expect("valid cube size");
        assert_eq!(occluder.center, bevy::math::DVec3::ZERO);
        assert_eq!(occluder.half_extents, bevy::math::DVec3::splat(0.5));
    }

    #[test]
    fn omitted_extent_on_non_cube_is_rejected() {
        let (stage, path) = view(
            r#"#usda 1.0
def Xform "World"
{
    def Xform "Body" {}
}
"#,
        );
        assert!(read_occluder_box(&stage.view(), &path).is_err());
    }
}
