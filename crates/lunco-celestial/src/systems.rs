use bevy::prelude::*;
use big_space::prelude::*;

use crate::big_space_setup::{SolarSystemRoot, EarthRoot, InertialAnchor, MoonRoot};
use crate::ephemeris::EphemerisResource;
use lunco_time::WorldTime;
use crate::registry::{CelestialBody, CelestialBodyRegistry, CelestialReferenceFrame};
use crate::coords::ecliptic_to_bevy;
use crate::coords::world_position_seeded;
use lunco_materials::{ParamValue, ShaderLook};

/// Update body and frame positions based on ephemeris data.
/// Optimized: Only re-computes if Epoch has changed significantly.
pub fn ephemeris_update_system(
    world: Res<WorldTime>,
    ephemeris: Option<Res<EphemerisResource>>,
    mut q_entities: Query<(Entity, &mut CellCoord, &mut Transform, Option<&CelestialBody>, Option<&CelestialReferenceFrame>)>,
    _q_all_parents: Query<&ChildOf>,
    _q_frames: Query<&CelestialReferenceFrame>,
    q_grids: Query<&Grid>,
    mut last_jd: Local<f64>,
) {
    let Some(ephemeris) = ephemeris else { return; };

    // Gate: re-project the whole body/frame hierarchy only when the epoch
    // has actually advanced. `last_jd` starts at 0.0 (a JD this clock never
    // takes), so the first real epoch always runs; thereafter a paused /
    // time-warp-stopped clock skips the full recompute. (This is the gate
    // the doc comment always promised but never wired up.)
    if (world.epoch_jd - *last_jd).abs() < 1e-9 {
        return;
    }
    *last_jd = world.epoch_jd;

    for (entity, mut cell, mut tf, body, frame) in q_entities.iter_mut() {
        let ephemeris_id = if let Some(b) = body { b.ephemeris_id } else if let Some(f) = frame { f.ephemeris_id } else { continue; };

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
        if ephemeris_id == 10 {
            continue;
        }

        // EphemerisProvider::position returns position relative to its parent defined in registry/hierarchy
        // P8(d): no data ⇒ leave the body where it is. It used to be teleported to its
        // parent's centre — a failed CSV fetch put the body inside the Sun, and nothing said so.
        let Some(rel_pos_au) = ephemeris.provider.position(ephemeris_id, world.epoch_jd) else {
            continue;
        };
        let pos_bevy_m = ecliptic_to_bevy(rel_pos_au).raw();

        // Find the grid this entity is in. Since body/frame entities are typically children of their reference frame grid:
        // We need to resolve which grid we are relative to. 
        // In our setup, Earth Body is child of Earth Local Grid.
        // If the provider returns pos relative to parent, then for Earth (399) it is relative to EMB (3).
        // But Earth Body is in Earth Local Grid, which is child of EMB Grid.
        // This means Earth Body should have translation = ZERO in its own local grid.
        
        // WAIT: The design is:
        // Solar Grid (10) -> Sun Body
        // Solar Grid (10) -> EMB Grid (3)
        // EMB Grid (3) -> Earth Grid (399)
        // EMB Grid (3) -> Moon Grid (301)
        // Earth Grid (399) -> Earth Body
        // Moon Grid (301) -> Moon Body
        
        // So:
        // EMB Grid position relative to Solar Grid = EMB helio (rel 10)
        // Earth Grid position relative to EMB Grid = Earth rel EMB (rel 3)
        // Moon Grid position relative to EMB Grid = Moon rel EMB (rel 3)
        // Earth Body position relative to Earth Grid = ZERO
        
        let mut depth = 0;
        let mut current = entity;
        // Search up for the first Grid parent
        while let Ok(child_of) = _q_all_parents.get(current) {
            if depth > 10 { break; } depth += 1;
            let parent = child_of.parent();
            if let Ok(grid) = q_grids.get(parent) {
                // If this is a Body entity, its position relative to its own Local Grid should be zero?
                // No, typically the Grid Anchor moves relative to ITS parent.
                // If 'entity' is a ReferenceFrame (the Grid Anchor itself):
                if frame.is_some() {
                    let (new_cell, new_translation) = grid.translation_to_grid(pos_bevy_m);
                    *cell = new_cell;
                    tf.translation = new_translation;
                } else if body.is_some() {
                    // Body is usually at center of its local grid
                    *cell = CellCoord::default();
                    tf.translation = Vec3::ZERO;
                }
                break;
            }
            current = parent;
        }
    }
}

/// Keep each [`InertialAnchor`] co-located with its body's grid — **position
/// only**.
///
/// The body grids rotate ([`body_rotation_system`]), which is right for the
/// surface features parented to them and wrong for an orbit camera. This copies
/// the body grid's `(CellCoord, translation)` onto the anchor and leaves the
/// anchor's `rotation` at IDENTITY, giving a star-fixed frame that still follows
/// the body through space.
///
/// Runs after `ephemeris_update_system` (which writes the pose being copied).
/// Guarded writes: an unconditional write would dirty the Transform every frame
/// on a paused clock and re-run propagation — the same re-rounding wobble
/// `body_rotation_system` documents.
pub fn sync_inertial_anchors(
    q_frames: Query<(&CelestialReferenceFrame, &CellCoord, &Transform), Without<InertialAnchor>>,
    mut q_anchors: Query<
        (&InertialAnchor, &mut CellCoord, &mut Transform),
        Without<CelestialReferenceFrame>,
    >,
) {
    for (anchor, mut cell, mut tf) in q_anchors.iter_mut() {
        let Some((_, src_cell, src_tf)) = q_frames
            .iter()
            .find(|(frame, _, _)| frame.ephemeris_id == anchor.ephemeris_id)
        else {
            continue;
        };
        if *cell != *src_cell {
            *cell = *src_cell;
        }
        if tf.translation != src_tf.translation {
            tf.translation = src_tf.translation;
        }
        // `tf.rotation` is deliberately NEVER written. That is the anchor.
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
    mut q_grids: Query<(&mut Transform, &CelestialReferenceFrame)>,
) {
    for (mut tf, frame) in q_grids.iter_mut() {
        if let Some(desc) = registry.bodies.iter().find(|d| d.ephemeris_id == frame.ephemeris_id) {
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
    let to_sun = crate::coords::ecliptic_to_bevy(p_sun - p_moon).raw().as_vec3().normalize_or_zero();
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
/// [`update_sun_light_system`]. Consumers: per-view-mode exposure (sun
/// elevation at the site decides whether the surface is sunlit or
/// earthshine-lit), future eclipse/illumination logic.
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
    mut q_light: Query<(&mut Transform, &mut DirectionalLight)>,
    // The site-ENU alignment now lives on the Site Align Grid (the Solar
    // Grid's rotation is IDENTITY — see `anchor_solar_frame_to_site`).
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
) {
    let Some((align_grid_tf, site_aligned)) = q_solar.iter().next() else {
        return;
    };
    let align_rot = if site_aligned.is_some() {
        align_grid_tf.rotation
    } else {
        Quat::IDENTITY
    };
    let Some(ephemeris) = ephemeris else { return; };

    let observer_body = q_site
        .iter()
        .next()
        .map(|a| a.body)
        .or_else(|| orbital_pin.as_ref().filter(|p| p.active).map(|p| p.body))
        .unwrap_or(399);

    let (Some(p_sun), Some(p_observer)) = (
        ephemeris.provider.global_position(10, world.epoch_jd),
        ephemeris.provider.global_position(observer_body, world.epoch_jd),
    ) else {
        return;
    };
    let Some(dir) = sun_emit_direction(p_sun, p_observer) else {
        // NoOp / degenerate ephemeris — leave the light to manual control.
        return;
    };
    // `dir` is in ECLIPTIC (solar-frame) axes. `align_rot` on `SiteAlignGrid`
    // is R_site_to_solar, so transforming from solar to site-ENU world frame
    // requires align_rot.inverse().
    let dir = (align_rot.inverse() * dir).normalize();
    let up = if dir.dot(Vec3::Y).abs() > 0.99 { Vec3::X } else { Vec3::Y };
    if sun_dir_out.0 != dir {
        sun_dir_out.0 = dir;
    }

    // …and Earth, the OTHER thing on this body points at. Same gate, same
    // rotation, same frame — an antenna bridge that recomputed the align rotation
    // for itself could disagree with the light by a frame, and a dish that lags
    // the world by a frame is a dish that hunts.
    //
    // The direction is TOWARD Earth (a look-at vector), not an emit direction:
    // Earth is a target here, not a light source, so it never gets the sun's sign
    // flip. `lunco-environment` turns it into az/el and publishes the ports.
    if let (Some(earth_dir_out), Some(p_earth)) = (
        earth_dir_out.as_mut(),
        ephemeris.provider.global_position(399, world.epoch_jd),
    ) {
        let to_earth = crate::coords::ecliptic_to_bevy(p_earth - p_observer)
            .raw()
            .as_vec3()
            .normalize_or_zero();
        // Degenerate (NoOp provider, or Earth and the observer body coincident)
        // stays ZERO — the resource's documented "not known", which the bridge
        // refuses to publish rather than reporting Earth due north on the horizon.
        let next = if to_earth.length_squared() > 0.5 {
            (align_rot.inverse() * to_earth).normalize()
        } else {
            Vec3::ZERO
        };
        if earth_dir_out.0 != next {
            earth_dir_out.0 = next;
        }
    }

    // The sun is the brightest `DirectionalLight` (Earthshine fill is far dimmer).
    if let Some((mut light_tf, mut light)) = q_light
        .iter_mut()
        .max_by(|a, b| a.1.illuminance.total_cmp(&b.1.illuminance))
    {
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
                    info!(
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

pub fn celestial_telemetry_system(
    world: Res<WorldTime>,
    q_earth: Query<(&Transform, &big_space::prelude::CellCoord), With<EarthRoot>>,
    q_moon: Query<(&Transform, &big_space::prelude::CellCoord), With<MoonRoot>>,
    q_sun: Query<&Transform, With<SolarSystemRoot>>,
    mut timer: Local<u32>,
) {
    if *timer % 60 == 0 {
        if let Some((tf, cell)) = q_earth.iter().next() { info!("TELEMETRY: Epoch: {:.4}, Earth Cell: {:?}, Earth Pos: {:?}", world.epoch_jd, cell, tf.translation); }
        if let Some((tf, cell)) = q_moon.iter().next() { info!("TELEMETRY: Moon Cell: {:?}, Moon Pos: {:?}", cell, tf.translation); }
        if let Some(tf) = q_sun.iter().next() { info!("TELEMETRY: Sun Pos: {:?}", tf.translation); }
    }
    *timer += 1;
}

/// Force per-frame `GlobalTransform` recomputation for the celestial subtree.
///
/// **Measured to be load-bearing (2026-07-11, `LUNCO_JUMP_PROBE=1`).** An
/// attempt to delete this after the root-grid fix strobed the ENTIRE
/// celestial tree to the world origin ~1 frame in 5–9 (whole-chain
/// plain-f32 compat output surviving to the renderer; 3385 jump events in
/// 15 s). Ordering `PropagateHighPrecision.after(propagate_parent_transforms)`
/// is evidently not sufficient on those frames for entities the HP pass
/// doesn't rewrite — force-dirtying their `Transform` guarantees it rewrites
/// them all, every frame. The real fix is retiring the root `Transform`
/// (Avian bubble, doc 47 Phase 5 / doc 45 two-space split); until then this
/// stays.
///
/// The `Or<>` must cover EVERY cell-entity living on a celestial grid.
/// `GeodeticAnchor`/`KeplerOrbit` prims (ground stations, satellites) were
/// missing — they kept flickering between their anchor pose and the collapsed
/// compat pose while the listed entities were protected (the "DSS ping-pong",
/// task #18). `Without<SiteAnchor>` is REQUIRED with them: the site-anchored
/// SCENE ROOT also carries `GeodeticAnchor`, and force-dirtying it dirties
/// the whole local scene every frame ("everything is jumping around",
/// 2026-07-10 regression).
#[allow(clippy::type_complexity)]
pub fn touch_celestial_transforms(
    q_site: Query<(), With<crate::geo::SiteAnchor>>,
    mut q: Query<
        &mut Transform,
        (
            Or<(
                With<CelestialBody>,
                With<CelestialReferenceFrame>,
                With<lunco_terrain_globe::TerrainTile>,
                With<crate::geo::GeodeticAnchor>,
                With<crate::kepler::KeplerOrbit>,
                With<crate::trajectories::TrajectoryPath>,
            )>,
            Without<crate::geo::SiteAnchor>,
        ),
    >,
) {
    // Only needed while a site anchor re-pins the solar frame; a plain scene
    // has no moving grid chain to go stale against.
    if q_site.is_empty() {
        return;
    }
    for mut tf in q.iter_mut() {
        tf.set_changed();
    }
}

// NOTE (2026-07-12): an orbital-only "touch_site_scene_transforms" pass was
// tried here to force the site subtree onto the HP propagation path while the
// orbital camera is on a celestial grid. Measured result: it did NOT remove
// the camera↔terrain divergence (the site branch and the celestial branch
// still disagree at f32-ULP scale across the Solar Grid's ~1 AU joint) and it
// cost whole-subtree GT recomputation every frame (~1 FPS in orbital view).
// The structural fix is re-branching the site scene under its anchor body's
// grid (task #24) so no 1 AU joint separates camera from terrain.

pub fn celestial_visuals_system(
    q_camera: Query<(Entity, &CellCoord, &Transform), (With<Camera>, With<lunco_core::Avatar>)>,
    q_bodies: Query<(Entity, &CellCoord, &Transform, &CelestialBody)>,
    mut q_tiles: Query<(&mut ShaderLook, &lunco_terrain_globe::TileCoord), With<lunco_terrain_globe::TerrainTile>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_site: Query<(), With<crate::geo::SiteAnchor>>,
) {
    let Some((cam_ent, cam_cell, cam_tf)) = q_camera.iter().next() else { return; };
    let cam_abs = world_position_seeded(cam_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial);

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
        let body_abs = world_position_seeded(body_ent, body_cell, body_tf, &q_parents, &q_grids, &q_spatial);
        let altitude = ((cam_abs - body_abs).length() - body.radius_m).max(0.0);
        let transition = if site_anchored {
            0.0
        } else {
            ((start_transition_alt - altitude) / (start_transition_alt - end_transition_alt))
                .clamp(0.0, 1.0) as f32
        };
        per_body.insert(body_ent, transition);
    }

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
        let Some(&transition) = per_body.get(&coord.body) else { continue };
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
    use bevy::math::DVec3;
    use crate::frames::EclipticAu;

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
        assert!((d.length() - 1.0).abs() < 1e-5, "emit dir must be unit length");

        // The light emits AWAY from the Sun: with the Moon on the far side, the
        // emit direction flips to the antipode.
        let d_opp = sun_emit_direction(EclipticAu::ZERO, EclipticAu::new(DVec3::new(-1.0, 0.0, 0.0)))
            .expect("non-degenerate");
        assert!((d + d_opp).length() < 1e-5, "antipodal Moon → antipodal light");
    }

    #[test]
    fn emit_direction_tracks_the_moon_position() {
        // Two distinct Moon positions give two distinct light directions — i.e.
        // advancing the epoch (which moves the Moon) re-aims the sun.
        let a = sun_emit_direction(EclipticAu::ZERO, EclipticAu::new(DVec3::new(1.0, 0.2, 0.0))).unwrap();
        let b = sun_emit_direction(EclipticAu::ZERO, EclipticAu::new(DVec3::new(1.0, 0.0, 0.3))).unwrap();
        assert!((a - b).length() > 1e-3, "different Moon positions → different sun aim");
    }
}
