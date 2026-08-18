use bevy::prelude::*;
use big_space::prelude::*;

use crate::coords::ecliptic_to_bevy;
use crate::coords::world_position_seeded;
use crate::ephemeris::EphemerisResource;
use crate::registry::{CelestialBody, CelestialBodyRegistry, ReferenceFrame};
use lunco_materials::{ParamValue, ShaderLook};
use lunco_time::WorldTime;

/// Update body and frame positions based on ephemeris data.
/// The caller applies the shared celestial solve gate. Translation and body
/// rotation are committed in the same gated chain so no descendant can observe
/// a half-advanced celestial frame.
pub fn ephemeris_update_system(
    world: Res<WorldTime>,
    ephemeris: Option<Res<EphemerisResource>>,
    mut q_frames: Query<(&mut CellCoord, &mut Transform, &ReferenceFrame, &ChildOf)>,
    q_grids: Query<&Grid>,
) {
    let Some(ephemeris) = ephemeris else {
        return;
    };

    // The epoch gate is NOT here. It used to be a private
    // `Local<f64>` comparing against 1e-9 JD — i.e. "did the epoch change at
    // all" — which meant a running clock re-projected the whole body/frame
    // hierarchy every single frame. It is now the shared
    // `cadence::celestial_needs_solve` run condition, on an angular error
    // budget, applied at registration alongside the other four celestial
    // systems.
    //
    // Deliberately shared and not re-derived locally: two gates with two
    // `Local`s drift, and a half-advanced celestial tree puts the sun and the
    // bodies at different instants.

    for (mut cell, mut tf, frame, child_of) in &mut q_frames {
        let Some(ephemeris_id) = frame.center() else {
            continue;
        };

        // NEVER write the Solar Grid (id 10) here. Its parent-relative
        // position is zero by definition (`position(10) == 0`), so in an
        // un-anchored scene this write was a no-op — but in a site-anchored
        // scene it ZEROED the pin that `anchor_solar_frame_to_site` re-applies
        // later in the chain. Within that window the whole solar hierarchy sat
        // at its raw heliocentric pose (~1.5e11 m off), and any UNORDERED
        // reader that interleaved there (gravity field, focus commands, GT
        // propagation for freshly spawned tiles) captured garbage: alternating
        // gravity (surface jitter), Earth tiles frozen 1e11 m away (blinking
        // Earth), camera teleports into empty space (click-to-focus black
        // screen). Skipping the write means no frame — mid-chain or otherwise
        // — ever holds the un-anchored pose.
        if ephemeris_id == crate::ephemeris_id::SUN {
            continue;
        }

        // EphemerisProvider::position returns position relative to its parent defined in registry/hierarchy
        // P8(d): no data ⇒ leave the body where it is. It used to be teleported to its
        // parent's centre — a failed CSV fetch put the body inside the Sun, and nothing said so.
        let Some(rel_pos_au) = ephemeris.provider.position(ephemeris_id, world.epoch_jd) else {
            continue;
        };
        let pos_bevy_m = ecliptic_to_bevy(rel_pos_au).raw();

        // A frame is always a direct child of another Grid. Its local f64
        // centre is encoded once into that parent's cells; body entities stay
        // at identity inside their own frame and are not ephemeris writers.
        let Ok(parent_grid) = q_grids.get(child_of.parent()) else {
            error_once!(
                "[celestial] reference frame {:?} is not directly parented to a Grid",
                frame
            );
            continue;
        };
        let (new_cell, new_translation) = parent_grid.translation_to_grid(pos_bevy_m);
        *cell = new_cell;
        tf.translation = new_translation;
    }
}

/// Rotate each celestial body's Grid around its polar axis.
/// Per big_space docs: "if you have a planet rotating and orbiting around
/// its star... you can place the planet and all objects on its surface in
/// the same grid. The motion of the planet will be inherited by all children
/// in that grid, in high precision."
/// We rotate the Grid so tiles (and future rovers) automatically inherit rotation.
pub fn body_rotation_system(
    world: Res<WorldTime>,
    registry: Res<CelestialBodyRegistry>,
    mut q_grids: Query<(&mut Transform, &ReferenceFrame)>,
) {
    for (mut tf, frame) in q_grids.iter_mut() {
        if let Some(body) = frame.body_fixed() {
            if let Some(desc) = registry.get(body) {
                if desc.spins() {
                    // Shared with the geodesy math (`geo::body_rotation`) so
                    // rendered grids and comms/anchor positions cannot diverge.
                    let next = crate::geo::body_rotation(desc, world.epoch_jd).as_quat();
                    // Guarded write: an unconditional `tf.rotation = …` dirties the
                    // Transform every frame even when the value is unchanged (paused
                    // clock), re-running propagation and re-rounding the f32 compose
                    // chain. At orbital-pin distances that re-rounding is a sub-pixel
                    // per-frame wobble of the focused body — worst at its limb
                    // ("Earth jitters" with the clock paused). Only write on change.
                    if tf.rotation != next {
                        tf.rotation = next;
                    }
                }
            }
        }
    }
}

// NOTE: a `tile_rotation_sync_system` used to live here — an intentionally
// EMPTY body ("tiles stay at identity rotation in the Grid frame") whose
// `.after(TransformSystems::Propagate)` orderings were silently meaningless in
// PreUpdate (those sets have no members there). Deleted 2026-07-11; tiles are
// carried by their (rotating) grid, which is the correct scheme.

/// Pure direction math for [`update_sun_light_system`]: the direction a
/// `DirectionalLight` should EMIT along (its local `-Z` / forward) so sunlight
/// travels from the Sun toward the scene, given heliocentric Sun and Moon
/// positions (ecliptic J2000, AU). Returns `None` when degenerate (e.g. the
/// `NoOpEphemerisProvider` returns ZERO for everything).
/// The inputs are typed `EclipticAu` on purpose: this is the exact pipe that once carried
/// EQUATORIAL vectors while claiming to be ecliptic, and put the sun 45° below the horizon at
/// Shackleton. A raw `DVec3` can no longer be handed to it.
pub fn sun_emit_direction(
    p_sun: crate::frames::EclipticAu,
    p_moon: crate::frames::EclipticAu,
) -> Option<Vec3> {
    // `to_sun` = Moon→Sun in Bevy world space; the light emits the other way.
    let to_sun = crate::coords::ecliptic_to_bevy(p_sun - p_moon)
        .raw()
        .as_vec3()
        .normalize_or_zero();
    if to_sun.length_squared() < 0.5 {
        return None;
    }
    Some(-to_sun)
}

/// Point the scene's primary `DirectionalLight` along the **ephemeris** Sun
/// direction at the current epoch (architecture doc 19 — T2; replaces the old
/// hardcoded `Vec3::NEG_Z`).
///
/// The Sun sits at the heliocentre, so the Moon→Sun direction is just
/// `-ecliptic_to_bevy(global_position(Moon)).raw()` (mirrors the solar-panel pointing
/// in [`crate::missions`]). A `DirectionalLight` emits along its local forward
/// (`-Z`) and rays travel FROM the Sun INTO the scene, so the light's forward is
/// set to `-to_sun`. The brightest light is taken as the sun (the Earthshine
/// fill is ~12 lx vs ~128 000 lx), matching the canonical `pick_sun` rule and
/// avoiding both a marker dependency and the `single_mut()`-fails-with-two-lights
/// trap.
///
/// With the default `NoOpEphemerisProvider` every position is ZERO, so `to_sun`
/// degenerates and the system returns early — leaving the light under manual
/// `SetEnvironmentLight` (yaw/pitch) control. The ephemeris is therefore
/// authoritative ONLY when a real provider (`lunco-celestial-ephemeris`) is
/// installed; sandbox / NoOp contexts keep dynamic manual control. That single
/// authoritative writer per context resolves the earlier web-build conflict
/// where two systems fought over the sun direction every frame.
/// The sun's EMIT direction in world (site-ENU) axes, published each frame by
/// [`update_sun_light_system`]. Consumers include future eclipse and local
/// illumination logic. Camera exposure deliberately does not consume this:
/// earthshine is a lighting contribution, not a reason to open the camera and
/// wash out direct sunlight.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct SunDirectionWorld(pub Vec3);

pub fn update_sun_light_system(
    ephemeris: Option<Res<EphemerisResource>>,
    world: Res<WorldTime>,
    sun_cal: Option<Res<lunco_environment::LunarSun>>,
    mut sun_dir_out: ResMut<SunDirectionWorld>,
    // Declared by `lunco-environment` (which cannot depend on this crate) and
    // filled here — the same shape as `LunarSun` below. `Option` because a build
    // without `EnvironmentPlugin` has no such resource and must still get a sun.
    mut earth_dir_out: Option<ResMut<lunco_environment::EarthDirectionWorld>>,
    // The scene's sun, identified STRUCTURALLY. A body's reflected fill
    // (earthshine and its analogues) is authored under that body's prim and
    // carries `Earthshine`; the scene's key light is not. See
    // `lunco_environment::horizon::SunQuery` for the same filter render-side.
    mut q_light: Query<
        (Entity, &mut Transform, &mut DirectionalLight, Option<&Name>),
        (
            Without<lunco_environment::Earthshine>,
            Without<bevy::camera::visibility::RenderLayers>,
        ),
    >,
    // The site-ENU alignment lives on the Site Align Grid (the Solar Grid's
    // rotation is IDENTITY — see `anchor_solar_frame_to_site`).
    q_solar: Query<
        (&Transform, Option<&crate::big_space_setup::SiteAligned>),
        (
            With<crate::big_space_setup::SiteAlignGrid>,
            With<big_space::prelude::Grid>,
            Without<DirectionalLight>,
        ),
    >,
    // Query the site anchor so observer body is dynamic (Earth 399, Moon 301, etc.)
    q_site: Query<&crate::geo::GeodeticAnchor, With<crate::geo::SiteAnchor>>,
    orbital_pin: Option<Res<crate::placement::OrbitalViewPin>>,
    // The bodies as big_space placed them — used ONLY as a cross-check against
    // the aim below, never as its source. See the disagreement note there.
    q_bodies: Query<(Entity, &CellCoord, &Transform, &CelestialBody), Without<DirectionalLight>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<DirectionalLight>>,
    // Last reported sun elevation, so the aim is logged on material change only.
    mut last_logged_elevation: Local<f32>,
) {
    let Some((align_grid_tf, site_aligned)) = q_solar.iter().next() else {
        return;
    };
    let Some(ephemeris) = ephemeris else {
        return;
    };
    // No site frame yet ⇒ nothing valid to compute. RETURN rather than aim with
    // an identity alignment: without the site rotation the ecliptic direction is
    // not a world direction, and writing it overwrites the USD-authored
    // `xformOp:rotateXYZ` — a correct, measured opinion (`lunco://lighting/sun.usda`
    // states this contract) — with a guess that lands tens of degrees off, usually
    // below the horizon. That overwrite is what rendered every site-anchored scene
    // black. An unaimed sun keeps the scene lit as authored.
    if site_aligned.is_none() {
        return;
    }
    let align_rot = align_grid_tf.rotation;

    let Some(observer_body) = q_site
        .iter()
        .next()
        .map(|a| a.body)
        .or_else(|| orbital_pin.as_ref().filter(|p| p.active).map(|p| p.body))
    else {
        return;
    };

    let (Some(p_sun), Some(p_observer)) = (
        ephemeris
            .provider
            .global_position(crate::ephemeris_id::SUN, world.epoch_jd),
        ephemeris
            .provider
            .global_position(observer_body, world.epoch_jd),
    ) else {
        return;
    };
    let Some(ecliptic_dir) = sun_emit_direction(p_sun, p_observer) else {
        // NoOp / degenerate ephemeris — leave the light to manual control.
        return;
    };

    // `ecliptic_dir` is in ECLIPTIC (solar-frame) axes. `align` on `SiteAlignGrid`
    // is built from rows east/up/−north, i.e. it maps SOLAR → SITE, so the site-ENU
    // world direction is `align * dir`.
    //
    // It used to be `align.inverse() * dir`, under a comment asserting `align` was
    // `R_site_to_solar`. That single inversion aimed the sun 64° the wrong way —
    // elevation −56° at a site whose scene authors +8.1° — so the light came from
    // under the ground and every site-anchored scene rendered black while the sun
    // disc hung in the sky. Verified against the two values `traverse.usda` authors
    // as the measured consequence of its epoch (`lunco:sun:elevationDeg` 8.1,
    // `lunco:sun:azimuthDeg` 95.7): this expression yields 8.14° / 95.7°.
    let dir = (align_rot * ecliptic_dir).normalize();

    // Report each material change once, with a CROSS-CHECK against the Sun body's
    // placed position. The two must agree: the light has to come from the sun that
    // is drawn. They currently do NOT (body ≈ −55° while the aim is +8°), which
    // means the disc is placed by a different frame than the aim — a real defect,
    // but in the PLACEMENT, not here. The aim above is the one verified against the
    // scene, so it stays authoritative and this line makes the disagreement visible
    // instead of leaving it to be discovered as "the sun is in the wrong place".
    let elevation_deg = (-dir.y).asin().to_degrees();
    if (elevation_deg - *last_logged_elevation).abs() > 0.5 {
        *last_logged_elevation = elevation_deg;
        // World axes are East=+X, Up=+Y, North=−Z; the reported azimuth is the
        // SUN's (the direction to it), not the emit direction's.
        let to_sun = -dir;
        let azimuth_deg = to_sun.x.atan2(-to_sun.z).to_degrees().rem_euclid(360.0);
        let body_elevation = q_bodies
            .iter()
            .find(|(_, _, _, b)| b.ephemeris_id == crate::ephemeris_id::SUN)
            .map(|(e, cell, tf, _)| {
                let p = lunco_core::coords::world_position_seeded(
                    e, cell, tf, &q_parents, &q_grids, &q_spatial,
                );
                (p.0.normalize_or_zero().y as f32).asin().to_degrees()
            });
        debug!(
            "[celestial] sun aim: elevation {elevation_deg:.2}°, azimuth {azimuth_deg:.1}° \
             @ JD {:.5} (observer {observer_body}, sun BODY elevation {:?} — must match)",
            world.epoch_jd, body_elevation,
        );
    }
    let up = if dir.dot(Vec3::Y).abs() > 0.99 {
        Vec3::X
    } else {
        Vec3::Y
    };
    if sun_dir_out.0 != dir {
        sun_dir_out.0 = dir;
    }

    // …and Earth, the OTHER thing on this body points at. Same rotation, same
    // frame — an antenna bridge that recomputed the align rotation for itself
    // could disagree with the light by a frame, and a dish that lags the world by
    // a frame is a dish that hunts.
    //
    // The direction is TOWARD Earth (a look-at vector), not an emit direction:
    // Earth is a target here, not a light source, so it never gets the sun's sign
    // flip. `lunco-environment` turns it into az/el and publishes the ports.
    if let (Some(earth_dir_out), Some(p_earth)) = (
        earth_dir_out.as_mut(),
        ephemeris
            .provider
            .global_position(crate::ephemeris_id::EARTH, world.epoch_jd),
    ) {
        let to_earth = crate::coords::ecliptic_to_bevy(p_earth - p_observer)
            .raw()
            .as_vec3()
            .normalize_or_zero();
        // Degenerate (NoOp provider, or Earth and the observer body coincident)
        // stays ZERO — the resource's documented "not known", which the bridge
        // refuses to publish rather than reporting Earth due north on the horizon.
        let next = if to_earth.length_squared() > 0.5 {
            // `align_rot`, NOT its inverse — this path carried the identical
            // error, so the dish pointed at an Earth 64° from the one in the sky.
            (align_rot * to_earth).normalize()
        } else {
            Vec3::ZERO
        };
        if earth_dir_out.0 != next {
            earth_dir_out.0 = next;
        }
    }

    // The sun is the scene's one non-fill `DirectionalLight` — see the query.
    // It used to be "the brightest", which is a guess: it silently picked when a
    // scene had two suns, and with equal illuminance it picked by archetype
    // iteration order. That guess is exactly how an engine-spawned duplicate
    // came to take the ephemeris aim while the scene's own sun stayed frozen.
    // Exactly one unscoped scene sun owns this write. ECS/archetype order is
    // not a lighting policy, so an ambiguous scene must not silently choose.
    if q_light.iter().count() != 1 {
        return;
    }
    if let Ok((light_entity, mut light_tf, mut light, light_name)) = q_light.single_mut() {
        debug!("[celestial] selected scene sun entity={light_entity:?} name={light_name:?}");
        // DEAD-BAND the aim. Unguarded, this rewrote the light every frame
        // from a direction that steps in f32-quat ULPs (the site pin's
        // `align` is recomputed per frame) — continuous sub-texel
        // light-direction churn defeats the cascade shadow maps' texel
        // snapping, so every shadow edge crawls and waggles ("the shadow on
        // the moon oscillates"), worst at the polar site's grazing sun.
        // 2e-5 rad ≈ one update per ~1.4 s real at 5.7× time — real sun
        // motion still tracks; between updates the direction is FROZEN and
        // the shadow map is byte-stable.
        let current_fwd: Vec3 = light_tf.forward().into();
        if current_fwd.angle_between(dir) > 2.0e-5 {
            light_tf.look_to(dir, up);
        }

        // 1/r² illuminance. `LunarSun`'s calibrated pair (~128 klx / EV 15)
        // is the 1 AU value; ephemeris positions are AU, so the live scale is
        // 1/r². At the Moon this breathes ±3% over the year (Earth-orbit
        // eccentricity); a site on a body elsewhere gets its real solar
        // constant. Exposure deliberately does NOT compensate — the
        // brightness difference IS the realism. Dead-banded at 0.5%:
        // sub-percent deltas are invisible and per-frame light mutation is
        // needless render-world churn.
        if let Some(cal) = &sun_cal {
            let r2 = (p_sun - p_observer).length_squared();
            if r2 > 1.0e-4 {
                let target = (cal.illuminance_lux as f64 / r2) as f32;
                if (light.illuminance - target).abs() > target * 5.0e-3 {
                    debug!(
                        "sun illuminance {:.0} lx (r = {:.4} AU, 1 AU cal {:.0} lx)",
                        target,
                        r2.sqrt(),
                        cal.illuminance_lux
                    );
                    light.illuminance = target;
                }
            }
        }
    }
}

pub fn celestial_visuals_system(
    q_camera: Query<(Entity, &CellCoord, &Transform), (With<Camera>, With<lunco_core::Avatar>)>,
    q_bodies: Query<(Entity, &CellCoord, &Transform, &CelestialBody)>,
    mut q_tiles: Query<
        (&mut ShaderLook, &lunco_terrain_globe::TileCoord),
        With<lunco_terrain_globe::TerrainTile>,
    >,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_site: Query<(), With<crate::geo::SiteAnchor>>,
    // Tiles the globe LOD streamed in since last frame — they spawn carrying a
    // clone of the body's look, which has no `transition` in it yet. The filter
    // reads `TileCoord`/`TerrainTile` only, never `ShaderLook`, so it does not
    // conflict with the `&mut ShaderLook` above.
    q_new_tiles: Query<
        (),
        (
            With<lunco_terrain_globe::TerrainTile>,
            Added<lunco_terrain_globe::TileCoord>,
        ),
    >,
    // A body whose look was REPLACED (imagery arrived, or the scene bound a
    // Material). `imagery::apply_look_to_tiles` re-inserts the whole `ShaderLook`
    // on every resident tile, wiping the `transition` this system wrote — and
    // every caller of it changes `GlobeLod` in the same breath, which is why the
    // body-side component is a sound proxy for "the tiles' looks are about to be
    // overwritten". Watching `Changed<ShaderLook>` directly would need read
    // access to the component this system writes.
    q_relooked: Query<(), Changed<crate::globe_lod::GlobeLod>>,
    // Last frame's per-body transitions, and a short "keep writing" countdown.
    mut last_per_body: Local<std::collections::HashMap<Entity, f32>>,
    mut force_frames: Local<u8>,
) {
    let Some((cam_ent, cam_cell, cam_tf)) = q_camera.iter().next() else {
        return;
    };
    let cam_abs =
        world_position_seeded(cam_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial);

    // The blueprint grid is an EDITOR affordance, and a scene with a site anchor is
    // not being edited from orbit — it is being stood on. Suppress the ramp there and
    // leave every body fully textured.
    //
    // Why this is the root fix and not a special case: the ramp exists so that a
    // camera diving at a body in the inspector sees STRUCTURE (a lat/long graticule,
    // then a Cartesian grid) instead of a 4K global mosaic smeared to ~5 km/texel.
    // That trade is right when there is nothing else to look at. A site-anchored
    // scene always has something else to look at — its own authored ground — so the
    // globe's job there is the FAR field and the limb, and for that the LROC albedo
    // is exactly the right data at exactly the right scale.
    //
    // Left on, the trade inverted badly: `blueprint.wgsl` switches to its Cartesian
    // XZ mode at `transition >= 0.5` and that mode does not sample the albedo at all,
    // so a lander at 90 m got a black-on-white wireframe where the Moon should be —
    // and, because the globe sphere is coincident with the site's own ground slab at
    // the datum, the two z-fought into concentric moiré rings across the whole frame.
    let site_anchored = !q_site.is_empty();

    // Per-body camera altitude → per-body texture↔blueprint transition.
    // Body-local coords (camera relative to body center) prevent thrashing
    // at high time warp — only depends on camera's position relative to the
    // body, not where the body happens to be in orbit.
    //
    // EVERY body gets its transition — not just the nearest. The old
    // nearest-only version left distant bodies' tiles on the material default
    // forever: Earth seen from a lunar site rendered as the blueprint
    // wireframe (invisible thin lines against black sky) — the long-standing
    // "no Earth in the sky" bug. With per-body altitudes a distant body
    // computes transition 0.0 = fully textured globe.
    //
    // High (0.0 transition) at 100 km, Blueprint (1.0 transition) at 10 km.
    let start_transition_alt = 100_000.0;
    let end_transition_alt = 10_000.0;
    let mut per_body: std::collections::HashMap<Entity, f32> = std::collections::HashMap::new();
    for (body_ent, body_cell, body_tf, body) in q_bodies.iter() {
        let body_abs = world_position_seeded(
            body_ent, body_cell, body_tf, &q_parents, &q_grids, &q_spatial,
        );
        let altitude = ((cam_abs - body_abs).length() - body.radius_m).max(0.0);
        let transition = if site_anchored {
            0.0
        } else {
            ((start_transition_alt - altitude) / (start_transition_alt - end_transition_alt))
                .clamp(0.0, 1.0) as f32
        };
        per_body.insert(body_ent, transition);
    }

    // Whole-pass gate over the ~600 resident tiles. The per-body altitudes above
    // are a handful of ancestor walks; the tile loop below is the part that
    // scales with the LOD's resident set, and with the transitions unmoved it
    // asks 600 times whether a value it already wrote is still what it wrote.
    //
    // NOT the cadence gate (`cadence::tracked_needs_solve`), and that is the
    // point: the transition is a function of CAMERA ALTITUDE, so gating it on the
    // epoch budget would leave a body on the wrong side of the texture↔blueprint
    // ramp for ~71 s of sim time after a dive — the ramp would visibly lag the
    // approach. It gates on its own inputs instead.
    //
    // Two frames rather than one after a dirty input: `apply_look_to_tiles`
    // replaces the tiles' `ShaderLook` through `Commands`, which apply at a sync
    // point that may fall AFTER this system. Writing the transition into a look
    // that is about to be overwritten would silently lose it until the next time
    // something else moved, so the write is repeated once the replacement has
    // landed.
    let dirty = *last_per_body != per_body || !q_new_tiles.is_empty() || !q_relooked.is_empty();
    if dirty {
        *force_frames = 2;
    }
    if *force_frames == 0 {
        return;
    }
    *force_frames -= 1;
    *last_per_body = per_body.clone();

    // Write the transition into each tile's appearance INTENT; `lunco-render-bevy`
    // rebinds the material. Every tile of a body gets the SAME value, so the binder's
    // content-keyed cache still resolves the body's whole tile set to one material and
    // one bind group — the property the old single shared `Handle<ShaderMaterial>`
    // gave by construction.
    //
    // GUARDED WRITE, and it is load-bearing: `Mut` only marks the component changed on
    // `DerefMut`, so comparing first means a parked camera dirties nothing and the
    // rebind system does no work. Unguarded, all ~600 resident tiles would re-key and
    // re-bind every frame.
    for (mut look, coord) in q_tiles.iter_mut() {
        let Some(&transition) = per_body.get(&coord.body) else {
            continue;
        };
        let next = ParamValue::F32(transition);
        if look.values.get("transition") != Some(&next) {
            look.values.insert("transition".into(), next);
        }
    }
}

#[cfg(test)]
mod sun_dir_tests {
    //! Pure ephemeris→sun-direction math ([`sun_emit_direction`], doc 19 — T2).
    use super::*;
    use crate::frames::EclipticAu;
    use bevy::math::DVec3;

    #[test]
    fn degenerate_ephemeris_yields_no_direction() {
        // NoOpEphemerisProvider returns ZERO for every body → no sun direction,
        // so the system leaves the light under manual control.
        assert!(sun_emit_direction(EclipticAu::ZERO, EclipticAu::ZERO).is_none());
    }

    #[test]
    fn emit_direction_is_unit_and_points_away_from_sun() {
        // Sun at the heliocentre, Moon offset along +X (ecliptic).
        let d = sun_emit_direction(EclipticAu::ZERO, EclipticAu::new(DVec3::new(1.0, 0.0, 0.0)))
            .expect("non-degenerate");
        assert!(
            (d.length() - 1.0).abs() < 1e-5,
            "emit dir must be unit length"
        );

        // The light emits AWAY from the Sun: with the Moon on the far side, the
        // emit direction flips to the antipode.
        let d_opp = sun_emit_direction(
            EclipticAu::ZERO,
            EclipticAu::new(DVec3::new(-1.0, 0.0, 0.0)),
        )
        .expect("non-degenerate");
        assert!(
            (d + d_opp).length() < 1e-5,
            "antipodal Moon → antipodal light"
        );
    }

    #[test]
    fn emit_direction_tracks_the_moon_position() {
        // Two distinct Moon positions give two distinct light directions — i.e.
        // advancing the epoch (which moves the Moon) re-aims the sun.
        let a = sun_emit_direction(EclipticAu::ZERO, EclipticAu::new(DVec3::new(1.0, 0.2, 0.0)))
            .unwrap();
        let b = sun_emit_direction(EclipticAu::ZERO, EclipticAu::new(DVec3::new(1.0, 0.0, 0.3)))
            .unwrap();
        assert!(
            (a - b).length() > 1e-3,
            "different Moon positions → different sun aim"
        );
    }
}
