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
use lunco_usd_bevy::UsdRead;
use openusd::sdf::Path as SdfPath;

/// NAIF id of the default anchor body (the Moon).
const DEFAULT_ANCHOR_BODY: i32 = 301;

pub fn insert_celestial_comms_components(
    reader: &lunco_usd_bevy::StageView<'_>,
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
    if let Some(naif) = reader.scalar::<i32>(sdf_path, "lunco:body") {
        if naif != 0 {
            commands
                .entity(entity)
                .try_insert(lunco_celestial::CelestialBodyDecl { naif });
            info!("[usd-celestial] scene declares celestial body {naif} at {prim_path_str}");
        }
    }

    // --- Geodetic anchor (ground stations + scene site anchor) ---
    let lat = reader.real(sdf_path, "lunco:anchor:lat");
    let lon = reader.real(sdf_path, "lunco:anchor:lon");
    if lat.is_some() || lon.is_some() {
        let body = reader
            .scalar::<i32>(sdf_path, "lunco:anchor:body")
            .unwrap_or(DEFAULT_ANCHOR_BODY);
        let anchor = GeodeticAnchor {
            body,
            geodetic: Geodetic::new(
                lat.unwrap_or(0.0),
                lon.unwrap_or(0.0),
                reader.real(sdf_path, "lunco:anchor:height").unwrap_or(0.0),
            ),
        };
        commands.entity(entity).try_insert(anchor);
        // Root prim anchor = the scene's site frame.
        let is_root = prim_path_str.matches('/').count() == 1 && prim_path_str.starts_with('/');
        if is_root {
            commands.entity(entity).try_insert(SiteAnchor);
            info!(
                "[usd-celestial] site anchor {}: body {} lat {:.4} lon {:.4} h {:.1} m",
                prim_path_str, body, anchor.geodetic.lat_deg, anchor.geodetic.lon_deg,
                anchor.geodetic.height_m
            );
            // Scene-authored date: `double lunco:time:epochJd` picks the world
            // epoch (e.g. one where a polar site is sunlit — at Shackleton the
            // real sun crosses the horizon on a ~monthly cycle, so an unlucky
            // "now" default renders the whole demo pitch-black).
            if let Some(epoch_jd) = reader.real(sdf_path, "lunco:time:epochJd") {
                info!("[usd-celestial] scene epoch: JD {epoch_jd:.4}");
                commands.trigger(lunco_time::SetMissionEpoch { epoch_jd });
            }
        }
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
    if let Some(id) = reader.text(sdf_path, "lunco:mission:id") {
        let name = reader
            .text(sdf_path, "lunco:mission:name")
            .unwrap_or_else(|| id.clone());
        commands.entity(entity).try_insert(lunco_celestial::MissionDecl {
            id: id.clone(),
            name: name.clone(),
            description: reader
                .text(sdf_path, "lunco:mission:description")
                .unwrap_or_default(),
        });
        info!("[usd-celestial] scene declares mission {name} ({id}) at {prim_path_str}");
    }

    // --- Mission trajectory (LunCoMissionTrajectoryAPI) ---
    //
    // VISUALISATION parameters only. The state vectors are NOT here and never were:
    // the curve is sampled at runtime from the ephemeris provider keyed by
    // `trackedId`/`referenceId`, so this prim says how to DRAW a trajectory, not
    // where the spacecraft is. Keyed on `trackedId` — without a target there is
    // nothing to plot.
    if let Some(tracked_id) = reader.scalar::<i32>(sdf_path, "lunco:trajectory:trackedId") {
        let color = read_rgba(reader, sdf_path, "lunco:trajectory:color", [1.0, 1.0, 1.0, 1.0]);
        commands
            .entity(entity)
            .try_insert(lunco_celestial::MissionTrajectoryDecl {
                name: reader
                    .text(sdf_path, "lunco:trajectory:name")
                    .unwrap_or_else(|| prim_path_str.to_string()),
                tracked_id,
                // Defaults to the Moon for the same reason `lunco:anchor:body` does.
                reference_id: reader
                    .scalar::<i32>(sdf_path, "lunco:trajectory:referenceId")
                    .unwrap_or(DEFAULT_ANCHOR_BODY),
                color,
                sampling_days: reader
                    .real(sdf_path, "lunco:trajectory:samplingDays")
                    .unwrap_or(1.0),
                sampling_step: reader
                    .real(sdf_path, "lunco:trajectory:samplingStep")
                    .unwrap_or(0.01),
                // `text()`, not `scalar::<String>()` — the value is an authored
                // `token`, and reading a token with the string accessor returns
                // None and would silently default to Inertial.
                frame: reader
                    .text(sdf_path, "lunco:trajectory:frame")
                    .unwrap_or_else(|| "Inertial".to_string()),
                user_visible: reader.scalar::<bool>(sdf_path, "lunco:trajectory:userVisible"),
                start_epoch_jd: reader.real(sdf_path, "lunco:trajectory:startEpochJd"),
                end_epoch_jd: reader.real(sdf_path, "lunco:trajectory:endEpochJd"),
            });
        info!("[usd-celestial] mission trajectory {prim_path_str}: target {tracked_id}");
    }

    // --- Mission spacecraft marker (LunCoMissionSpacecraftAPI) ---
    //
    // Keyed on `ephemerisId`: the marker's whole job is to sit where the ephemeris
    // says that body is, so a prim naming no body is unplaceable, not defaulted.
    if let Some(ephemeris_id) = reader.scalar::<i32>(sdf_path, "lunco:spacecraft:ephemerisId") {
        commands
            .entity(entity)
            .try_insert(lunco_celestial::MissionSpacecraftDecl {
                name: reader
                    .text(sdf_path, "lunco:spacecraft:name")
                    .unwrap_or_else(|| prim_path_str.to_string()),
                ephemeris_id,
                reference_id: reader
                    .scalar::<i32>(sdf_path, "lunco:spacecraft:referenceId")
                    .unwrap_or(DEFAULT_ANCHOR_BODY),
                scale: reader
                    .real_f32(sdf_path, "lunco:spacecraft:scale")
                    .unwrap_or(1.0),
                start_epoch_jd: reader.real(sdf_path, "lunco:spacecraft:startEpochJd"),
                end_epoch_jd: reader.real(sdf_path, "lunco:spacecraft:endEpochJd"),
                marker_radius_km: reader.real_f32(sdf_path, "lunco:spacecraft:markerRadiusKm"),
                hit_radius_km: reader.real_f32(sdf_path, "lunco:spacecraft:hitRadiusKm"),
                marker_color: reader
                    .scalar::<Vec<f32>>(sdf_path, "lunco:spacecraft:markerColor")
                    .filter(|v| v.len() >= 4)
                    .map(|v| [v[0], v[1], v[2], v[3]]),
            });
        info!("[usd-celestial] mission spacecraft {prim_path_str}: ephemeris {ephemeris_id}");
    }

    // --- Keplerian orbit (satellites) ---
    if let Some(a_m) = reader.real(sdf_path, "lunco:orbit:semiMajorAxisM") {
        let body = reader
            .scalar::<i32>(sdf_path, "lunco:orbit:body")
            .unwrap_or(DEFAULT_ANCHOR_BODY);
        let elements = KeplerianElements {
            semi_major_axis_m: a_m,
            eccentricity: reader.real(sdf_path, "lunco:orbit:eccentricity").unwrap_or(0.0),
            inclination_deg: reader.real(sdf_path, "lunco:orbit:inclinationDeg").unwrap_or(0.0),
            raan_deg: reader.real(sdf_path, "lunco:orbit:raanDeg").unwrap_or(0.0),
            arg_periapsis_deg: reader
                .real(sdf_path, "lunco:orbit:argPeriapsisDeg")
                .unwrap_or(0.0),
            mean_anomaly_deg: reader.real(sdf_path, "lunco:orbit:meanAnomalyDeg").unwrap_or(0.0),
            epoch_jd: reader
                .real(sdf_path, "lunco:orbit:epochJd")
                .unwrap_or(lunco_time::J2000_JD),
        };
        commands.entity(entity).try_insert(KeplerOrbit { body, elements });
        info!(
            "[usd-celestial] orbit {}: body {} a {:.0} km e {:.2} i {:.1}°",
            prim_path_str, body, elements.semi_major_axis_m / 1000.0, elements.eccentricity,
            elements.inclination_deg
        );
    }

    // --- Libration point (a relay parked at L1/L2 of a pair) ---
    //
    // The third placement kind, beside geodetic (on a body) and Kepler (around one).
    // Keyed on `primary`, since an L-point is defined by a PAIR — the pair IS the
    // placement, so a prim naming only one body is not half-placed, it is unplaced.
    if let Some(primary) = reader.scalar::<i32>(sdf_path, "lunco:libration:primary") {
        let Some(secondary) = reader.scalar::<i32>(sdf_path, "lunco:libration:secondary") else {
            warn!(
                "[usd-celestial] {}: `lunco:libration:primary` without `:secondary` — an \
                 L-point is a property of a PAIR; prim left unplaced",
                prim_path_str
            );
            return;
        };
        // `text()` — the value is an authored `token`, and a token is not a string:
        // reading it with the wrong accessor returns None and would silently default.
        let token = reader
            .text(sdf_path, "lunco:libration:point")
            .unwrap_or_else(|| "L1".to_string());
        let Some(point) = LPoint::from_token(&token) else {
            warn!(
                "[usd-celestial] {}: `lunco:libration:point = \"{}\"` names no libration point \
                 (want L1..L5); prim left unplaced rather than silently parked at L1",
                prim_path_str, token
            );
            return;
        };
        commands
            .entity(entity)
            .try_insert(LibrationAnchor { primary, secondary, point });
        info!(
            "[usd-celestial] libration {}: {:?} of pair {}/{}",
            prim_path_str, point, primary, secondary
        );
    }

    // --- Solar-pose tracking marker (generic celestial placement) ---
    // A scene-local subsystem prim (a rover-mounted antenna, a panel) opts in so
    // the pose system tracks its solar-frame position; anchored/orbiting prims
    // are tracked automatically. Authored subsystems read it through the
    // `SolarPose` query — no domain component, no domain vocabulary.
    if reader
        .scalar::<bool>(sdf_path, "lunco:solarTracked")
        .unwrap_or(false)
    {
        commands
            .entity(entity)
            .try_insert(lunco_celestial::pose::SolarTracked);
    }

    // --- Connectivity node (generic link kernel) ---
    // Marks a prim as a link endpoint: the kernel pairs it with every other
    // node, applies the `link.connected` verdict, and publishes link state. Pose
    // tracking follows automatically. `class` is an authored role the routing /
    // verdict policy reads — the core never interprets it.
    if reader
        .scalar::<bool>(sdf_path, "lunco:linkNode")
        .unwrap_or(false)
    {
        let d = lunco_celestial::link::LinkNode::default();
        commands.entity(entity).try_insert(lunco_celestial::link::LinkNode {
            max_range_m: reader
                .real(sdf_path, "lunco:link:maxRangeM")
                .unwrap_or(d.max_range_m),
            min_elevation_deg: reader
                .real(sdf_path, "lunco:link:minElevationDeg")
                .unwrap_or(d.min_elevation_deg),
            class: reader.text(sdf_path, "lunco:link:class"),
        });
    }

    // --- Sight-line occluder (generic geometry, not a comms concept) ---
    // Marks THIS prim's geometry as opaque to link sight-lines: any link whose
    // segment crosses its box is severed. Author it on the geometry prim that
    // actually blocks (the child `Cube`, not its parent `Xform`), so the box
    // inherits that prim's pose, scale and extent.
    //
    // The BOX IS THE PRIM'S `extent` — core UsdGeom, no invented size vocabulary.
    // Absent (our reader sees only authored attributes, and USD computes extent
    // for a gprim rather than requiring it), it falls back to the unit-cube
    // convention: a `Cube` with `size = 1` scaled by S has half-extents S/2, which
    // is exactly how `props/wall.usda` and the sandbox slabs are written.
    //
    // NOT derived from `PhysicsCollisionAPI`: opacity is a material property, not
    // a collision one (a handrail collides but does not block; a radome may block
    // but not collide). See `LinkOccluder`.
    if reader
        .scalar::<bool>(sdf_path, "lunco:occluder")
        .unwrap_or(false)
    {
        commands
            .entity(entity)
            .try_insert(read_occluder_box(reader, sdf_path));
    }
}

/// Read an authored `color4f` (or any `float[4]`-shaped attribute) as RGBA,
/// falling back to `default` when it is absent or malformed.
///
/// `f32` first, then `f64`: a hand-authored `.usda` may spell the same colour as
/// either, and a value that round-trips through a `double` array would otherwise
/// read as absent and silently take the default — the same trap
/// [`read_occluder_box`] handles for `extent`.
fn read_rgba(
    reader: &lunco_usd_bevy::StageView<'_>,
    sdf_path: &SdfPath,
    attr: &str,
    default: [f32; 4],
) -> [f32; 4] {
    let v = reader
        .scalar::<Vec<f32>>(sdf_path, attr)
        .or_else(|| {
            reader
                .scalar::<Vec<f64>>(sdf_path, attr)
                .map(|v| v.into_iter().map(|f| f as f32).collect())
        })
        .filter(|v| v.len() >= 4);
    match v {
        Some(v) => [v[0], v[1], v[2], v[3]],
        None => default,
    }
}

/// The occluding box from the prim's UsdGeom `extent` (`float3[2]` — min, max),
/// else the unit-cube default. Both are pre-scale, in the prim's local space; the
/// kernel applies the `Transform` scale.
fn read_occluder_box(
    reader: &lunco_usd_bevy::StageView<'_>,
    sdf_path: &SdfPath,
) -> lunco_celestial::link::LinkOccluder {
    use bevy::math::DVec3;
    let extent = reader
        .scalar::<Vec<f32>>(sdf_path, "extent")
        .map(|v| v.into_iter().map(|f| f as f64).collect::<Vec<f64>>())
        .or_else(|| reader.scalar::<Vec<f64>>(sdf_path, "extent"));
    let Some(v) = extent.filter(|v| v.len() >= 6) else {
        return lunco_celestial::link::LinkOccluder::default();
    };
    let (min, max) = (DVec3::new(v[0], v[1], v[2]), DVec3::new(v[3], v[4], v[5]));
    lunco_celestial::link::LinkOccluder {
        half_extents: (max - min) * 0.5,
        center: (max + min) * 0.5,
    }
}
