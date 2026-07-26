//! Generic solar-frame pose tracking — domain-free celestial placement.
//!
//! `update_solar_poses` writes each tracked entity's position (+ local up for
//! surface points) in the solar frame to a [`SolarFramePose`] component, resolved
//! from its `GeodeticAnchor` (ground stations), `KeplerOrbit` (satellites, incl.
//! LEO / lunar-orbit relays), or — for scene-local prims that move with a body
//! (a rover-mounted antenna) — the site tangent frame. The scene-local path needs
//! the big_space `Query` context that a read-only `query("SolarPose")` provider
//! cannot get, which is exactly why this is a SYSTEM (docs 10/12).
//!
//! This is the generic substrate any subsystem reuses (comms / solar / thermal /
//! sensors): mark a prim [`SolarTracked`] (or give it an anchor/orbit) and its
//! solar pose follows from placement — no domain concept here.

use bevy::math::DVec3;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};

use lunco_time::WorldTime;

use crate::coords::ecliptic_to_bevy;
use crate::ephemeris::EphemerisResource;
use crate::geo::{solar_position_of_geodetic, solar_tangent_frame, GeodeticAnchor, SiteAnchor};
use crate::kepler::KeplerOrbit;
use crate::link::LinkNode;
use crate::registry::CelestialBodyRegistry;
use crate::transform::{FrameTree, LibrationAnchor};

/// Opt-in marker: track this entity's solar pose even though it has no anchor or
/// orbit (a scene-local prim positioned through the site frame — e.g. an antenna
/// bolted to a moving rover). Entities with a `GeodeticAnchor`/`KeplerOrbit` are
/// tracked automatically and need no marker.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct SolarTracked;

/// An entity's pose, refreshed by [`update_solar_poses`]. `pos` is the solar
/// frame (range / occultation); `local` is the site's local scene frame (the
/// terrain oracle / `TerrainRaycast` frame — equals `pos` when no site anchor).
/// `up` is the local vertical for a surface point, `DVec3::ZERO` for a free
/// orbit (no local horizon). Read by authored subsystems and the `SolarPose`
/// query. Derived per tick — not networked/reflected.
#[derive(Component, Debug, Clone, Copy)]
pub struct SolarFramePose {
    pub pos: DVec3,
    pub local: DVec3,
    pub up: DVec3,
    pub body: i32,
}

/// Refresh [`SolarFramePose`] for every tracked entity. Headless-safe; a no-op
/// until `WorldTime` + ephemeris + registry exist.
#[allow(clippy::too_many_arguments)]
pub fn update_solar_poses(
    world_time: Option<Res<WorldTime>>,
    ephemeris: Option<Res<EphemerisResource>>,
    registry: Option<Res<CelestialBodyRegistry>>,
    q_tracked: Query<
        (
            Entity,
            Option<&GeodeticAnchor>,
            Option<&KeplerOrbit>,
            Option<&LibrationAnchor>,
        ),
        Or<(
            With<GeodeticAnchor>,
            With<KeplerOrbit>,
            With<LibrationAnchor>,
            With<SolarTracked>,
            With<LinkNode>,
        )>,
    >,
    q_site: Query<&GeodeticAnchor, With<SiteAnchor>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    mut q_pose: Query<&mut SolarFramePose>,
    mut commands: Commands,
) {
    let (Some(world_time), Some(ephemeris), Some(registry)) = (world_time, ephemeris, registry)
    else {
        return;
    };
    let jd = world_time.epoch_jd;
    let body_of = |naif: i32| registry.bodies.iter().find(|b| b.ephemeris_id == naif);

    // `jd` is fixed for this whole solve and only a handful of distinct bodies
    // are referenced, but a body's position is a full analytic series (VSOP87 /
    // ELP-MPP02 — three series for the Moon). Evaluating it per TRACKED ENTITY
    // made the cost O(entities x series) when it is O(bodies x series): the same
    // shape of bug as the over-broad query this system's sibling already fixed.
    // Memoised per (body, jd) for the duration of the call; the map dies with it,
    // so no epoch can ever be served a stale centre.
    let mut centers: HashMap<i32, Option<DVec3>> = HashMap::default();
    // `None` ⇒ no ephemeris for that body. Callers skip it rather than reporting a pose at the
    // Sun's centre that looks exactly like a real one.
    let provider = ephemeris.provider.as_ref();
    let body_center = |naif: i32, centers: &mut HashMap<i32, Option<DVec3>>| -> Option<DVec3> {
        *centers
            .entry(naif)
            .or_insert_with(|| provider.global_position(naif, jd).map(|p| ecliptic_to_bevy(p).raw()))
    };

    // Loop-invariant like `centers`: `FrameTree` is a view over (jd, registry,
    // provider), so rebuilding it per libration entity bought nothing.
    let tree = FrameTree::new(jd, &registry, provider);

    // The site frame (scene-root anchor), for scene-local prims.
    let site = match q_site.iter().next() {
        Some(anchor) => match (body_of(anchor.body), body_center(anchor.body, &mut centers)) {
            (Some(desc), Some(center)) => Some((
                anchor.body,
                solar_tangent_frame(desc, &anchor.geodetic, center, jd),
            )),
            _ => None,
        },
        None => None,
    };

    for (entity, anchor, orbit, libration) in q_tracked.iter() {
        // (solar pos, up, body) per placement kind; a diverging branch skips.
        let (pos, up, body) = if let Some(a) = anchor {
            let Some(desc) = body_of(a.body) else {
                continue;
            };
            // No ephemeris ⇒ no pose. Skipping beats reporting a pose at the Sun's centre.
            let Some(center) = body_center(a.body, &mut centers) else {
                continue;
            };
            let pos = solar_position_of_geodetic(desc, &a.geodetic, center, jd);
            (pos, (pos - center).normalize_or_zero(), a.body)
        } else if let Some(o) = orbit {
            let Some(desc) = body_of(o.body) else {
                continue;
            };
            {
                let Some(center) = body_center(o.body, &mut centers) else {
                    continue;
                };
                (
                    center + o.elements.position_bevy_m(desc.gm, jd),
                    DVec3::ZERO,
                    o.body,
                )
            }
        } else if let Some(l) = libration {
            // A libration point of a PAIR — Earth–Moon L1/L2 for a relay. Resolved by
            // the CR3BP solver in `transform`, which needs both bodies' positions and
            // masses; `None` if either is missing, and we skip rather than invent one.
            let Some(pos) = tree.libration_in_solar(l.primary, l.secondary, l.point) else {
                continue;
            };
            // `up` is ZERO like an orbit: an L-point has no local horizon, so an
            // elevation mask against it is meaningless. `body` is the SECONDARY — the
            // body the point is parked near, which is what a range/occultation test
            // wants to know about.
            (pos.raw(), DVec3::ZERO, l.secondary)
        } else if let Some((body, frame)) = &site {
            let Ok((cell, tf)) = q_spatial.get(entity) else {
                continue;
            };
            let cell = cell.copied().unwrap_or_default();
            let local = lunco_core::coords::world_position_seeded(
                entity, &cell, tf, &q_parents, &q_grids, &q_spatial,
            );
            (frame.to_frame(local), frame.up, *body)
        } else {
            continue;
        };
        // Site-local position (terrain frame); = solar pos when unanchored.
        let local = site.as_ref().map(|(_, f)| f.from_frame(pos)).unwrap_or(pos);
        let pose = SolarFramePose {
            pos,
            local,
            up,
            body,
        };

        // Update in place (avoid per-tick insert churn); insert on first sight.
        if let Ok(mut existing) = q_pose.get_mut(entity) {
            *existing = pose;
        } else {
            commands.entity(entity).try_insert(pose);
        }
    }
}
