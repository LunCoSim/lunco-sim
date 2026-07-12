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
//! double lunco:anchor:lat = 40.4314    # + lon/height (shared with terrain
//! int    lunco:anchor:body = 399       #   georef; body defaults to Moon 301)
//! int    lunco:orbit:body = 301
//! double lunco:orbit:semiMajorAxisM = 6540000   # + eccentricity/inclinationDeg/
//!                                               #   raanDeg/argPeriapsisDeg/
//!                                               #   meanAnomalyDeg/epochJd
//! ```
//!
//! A root prim (path depth 1) authoring an anchor is the scene's **site
//! anchor**: the local scene origin sits at that geodetic point (ENU axes) —
//! it grounds every scene-local endpoint (rover masts) on the body.

use bevy::prelude::*;

use lunco_celestial::geo::{Geodetic, GeodeticAnchor, SiteAnchor};
use lunco_celestial::kepler::{KeplerOrbit, KeplerianElements};
use lunco_usd_bevy::UsdRead;
use openusd::sdf::Path as SdfPath;

/// NAIF id of the default anchor body (the Moon).
const DEFAULT_ANCHOR_BODY: i32 = 301;

pub fn insert_celestial_comms_components<R: UsdRead>(
    reader: &R,
    entity: Entity,
    prim_path_str: &str,
    sdf_path: &SdfPath,
    commands: &mut Commands,
) {
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
        commands.entity(entity).insert(anchor);
        // Root prim anchor = the scene's site frame.
        let is_root = prim_path_str.matches('/').count() == 1 && prim_path_str.starts_with('/');
        if is_root {
            commands.entity(entity).insert(SiteAnchor);
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
        commands.entity(entity).insert(KeplerOrbit { body, elements });
        info!(
            "[usd-celestial] orbit {}: body {} a {:.0} km e {:.2} i {:.1}°",
            prim_path_str, body, elements.semi_major_axis_m / 1000.0, elements.eccentricity,
            elements.inclination_deg
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
            .insert(lunco_celestial::pose::SolarTracked);
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
        commands.entity(entity).insert(lunco_celestial::link::LinkNode {
            max_range_m: reader
                .real(sdf_path, "lunco:link:maxRangeM")
                .unwrap_or(d.max_range_m),
            min_elevation_deg: reader
                .real(sdf_path, "lunco:link:minElevationDeg")
                .unwrap_or(d.min_elevation_deg),
            class: reader.scalar::<String>(sdf_path, "lunco:link:class"),
        });
    }
}
