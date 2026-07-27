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

/// WHOSE horizon an elevation is measured against — and whether one exists.
///
/// This is a sum type rather than a `up: DVec3` + `body: i32` pair for two
/// reasons, both of which were live defects:
///
/// 1. **`DVec3::ZERO` was a sentinel for "free-flyer".** Every consumer had to
///    remember to test for it, and the one that mattered invented `90.0°` for it
///    inline. A caller that forgets gets `asin(0) = 0°` — a horizon-grazing link
///    that silently fails a mask.
/// 2. **`up` and `body` were independent fields that could disagree**, and did.
///    In `scenes/tests/comms_demo.usda` the three DSN complexes' `LinkAperture`
///    nodes are anchored to Earth through an ancestor, but took the LUNAR site's
///    vertical. Their positions were right, so the geometry looked sane — Madrid
///    genuinely saw the relay 57.3° above its own horizon — while the engine
///    measured that angle against a vertical pointing out of the Moon's south
///    pole and rejected all three on a 5° mask. The relay reached nothing and the
///    routing test failed with an empty path and no stated cause.
///
/// Pairing the vertical with the body it belongs to makes that disagreement
/// unrepresentable, and making "no horizon" its own variant makes every consumer
/// decide what to do about it at compile time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Horizon {
    /// Standing on `body`; `up` is the outward vertical at this point.
    Surface { body: i32, up: DVec3 },
    /// Free-flying — an orbit or a libration point near `body`. There is no local
    /// horizon, so an elevation mask has nothing to measure against.
    Free { body: i32 },
}

impl Horizon {
    /// The body this node is placed relative to. Always defined: a free-flyer
    /// still orbits something, and range/occultation needs to know what.
    pub fn body(&self) -> i32 {
        match self {
            Horizon::Surface { body, .. } | Horizon::Free { body } => *body,
        }
    }

    /// The outward local vertical, or `None` for a free-flyer. Returning an
    /// `Option` is the point: there is no vector that honestly means "no horizon".
    pub fn up(&self) -> Option<DVec3> {
        match self {
            Horizon::Surface { up, .. } => Some(*up),
            Horizon::Free { .. } => None,
        }
    }
}

/// An entity's pose, refreshed by [`update_solar_poses`]. `pos` is the solar
/// frame (range / occultation); `local` is the site's local scene frame (the
/// terrain oracle / `TerrainRaycast` frame — equals `pos` when no site anchor).
/// [`Horizon`] carries the local vertical together with the body it belongs to.
/// Read by authored subsystems and the `SolarPose` query. Derived per tick — not
/// networked/reflected.
#[derive(Component, Debug, Clone, Copy)]
pub struct SolarFramePose {
    pub pos: DVec3,
    pub local: DVec3,
    pub horizon: Horizon,
}

impl SolarFramePose {
    /// The body this pose is placed relative to.
    pub fn body(&self) -> i32 {
        self.horizon.body()
    }
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
    // A link node is usually a deep child of the thing that IS anchored (a dish's
    // feed aperture, six prims under the ground station), so its own entity carries
    // no anchor. Looked up by ancestry below.
    q_anchor: Query<&GeodeticAnchor>,
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
        // ONE RULE: a node is placed by the nearest placement in its ancestry,
        // INCLUDING itself. Its horizon belongs to that placement's body.
        //
        // The ancestry half is not an edge case — it is the normal shape. A link
        // node is the feed aperture six prims below the dish, so the anchor sits on
        // the ground station and never on the node. Only the POSITION comes down the
        // transform hierarchy; the horizon has to come from the placement, or the
        // node measures its elevation against whatever ground the scene happens to
        // be standing on.
        let (pos, horizon) = if let Some(a) = anchor {
            let Some(desc) = body_of(a.body) else {
                continue;
            };
            // No ephemeris ⇒ no pose. Skipping beats reporting a pose at the Sun's centre.
            let Some(center) = body_center(a.body, &mut centers) else {
                continue;
            };
            let pos = solar_position_of_geodetic(desc, &a.geodetic, center, jd);
            let up = (pos - center).normalize_or_zero();
            (pos, Horizon::Surface { body: a.body, up })
        } else if let Some(o) = orbit {
            let Some(desc) = body_of(o.body) else {
                continue;
            };
            let Some(center) = body_center(o.body, &mut centers) else {
                continue;
            };
            (
                center + o.elements.position_bevy_m(desc.gm, jd),
                Horizon::Free { body: o.body },
            )
        } else if let Some(l) = libration {
            // A libration point of a PAIR — Earth–Moon L1/L2 for a relay. Resolved by
            // the CR3BP solver in `transform`, which needs both bodies' positions and
            // masses; `None` if either is missing, and we skip rather than invent one.
            //
            // `body` is the SECONDARY — the body the point is parked near, which is
            // what a range/occultation test wants to know about.
            let Some(pos) = tree.libration_in_solar(l.primary, l.secondary, l.point) else {
                continue;
            };
            (pos.raw(), Horizon::Free { body: l.secondary })
        } else if let Some((site_body, frame)) = &site {
            // Scene-local: the position is wherever the transform hierarchy puts it.
            let Ok((cell, tf)) = q_spatial.get(entity) else {
                continue;
            };
            let cell = cell.copied().unwrap_or_default();
            let local = lunco_core::coords::world_position_seeded(
                entity, &cell, tf, &q_parents, &q_grids, &q_spatial,
            );
            let pos = frame.to_frame(local.0);
            // The horizon, however, belongs to the nearest ANCHORED ancestor — the
            // scene's own site frame only when nothing above this node claims a body.
            let ancestor_body = std::iter::successors(Some(entity), |e| {
                q_parents.get(*e).ok().map(|c| c.parent())
            })
            .find_map(|e| q_anchor.get(e).ok().map(|a| a.body));
            let horizon = match ancestor_body {
                // Derived from THIS node's own position, not the ancestor's: a dish on
                // a mast and its feed aperture stand on the same ground but not at the
                // same point, and the vertical is a property of the point.
                Some(body) => {
                    // No ephemeris ⇒ no pose, exactly as every branch above. Calling
                    // it free-flying would be a lie about the placement, and the
                    // alternative — a zero vertical — is the sentinel this type
                    // exists to delete.
                    let Some(center) = body_center(body, &mut centers) else {
                        continue;
                    };
                    Horizon::Surface {
                        body,
                        up: (pos - center).normalize_or_zero(),
                    }
                }
                None => Horizon::Surface {
                    body: *site_body,
                    up: frame.up,
                },
            };
            (pos, horizon)
        } else {
            continue;
        };
        // Site-local position (terrain frame); = solar pos when unanchored.
        let local = site.as_ref().map(|(_, f)| f.from_frame(pos)).unwrap_or(pos);
        let pose = SolarFramePose { pos, local, horizon };

        // Update in place (avoid per-tick insert churn); insert on first sight.
        if let Ok(mut existing) = q_pose.get_mut(entity) {
            *existing = pose;
        } else {
            commands.entity(entity).try_insert(pose);
        }
    }
}
