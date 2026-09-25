use bevy::prelude::*;
use big_space::prelude::*;

use lunco_celestial::coords::ecliptic_to_bevy;
use lunco_celestial::ephemeris::EphemerisResource;
use lunco_celestial::{CelestialBody, CelestialBodyRegistry, ReferenceFrame};
use lunco_celestial_spatial_core::OrbitalViewPin;
use lunco_materials::{ParamValue, ShaderLook};
use lunco_spatial::coords::world_position_seeded;
use lunco_time::CelestialTime;

/// Update body and frame positions based on ephemeris data.
/// The caller applies the shared celestial solve gate. Translation and body
/// rotation are committed in the same gated chain so no descendant can observe
/// a half-advanced celestial frame.
pub fn ephemeris_update_system(
    celestial_time: Res<CelestialTime>,
    ephemeris: Option<Res<EphemerisResource>>,
    mut q_frames: Query<(&mut CellCoord, &mut Transform, &ReferenceFrame, &ChildOf)>,
    q_grids: Query<&Grid>,
) {
    let Some(ephemeris) = ephemeris else {
        return;
    };

    // The shared angular-error cadence gate is registered with the other
    // celestial systems so the body hierarchy and Sun projection solve at one
    // epoch.

    for (mut cell, mut tf, frame, child_of) in &mut q_frames {
        let Some(ephemeris_id) = frame.center() else {
            continue;
        };

        // EphemerisProvider::position returns position relative to its parent
        // in the body registry hierarchy.
        let Some(rel_pos_au) = ephemeris
            .provider
            .position(ephemeris_id, celestial_time.epoch_jd)
        else {
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
        if *cell != new_cell {
            *cell = new_cell;
        }
        if tf.translation != new_translation {
            tf.translation = new_translation;
        }
    }
}

/// Rotate each celestial body's Grid around its polar axis.
/// Per big_space docs: "if you have a planet rotating and orbiting around
/// its star... you can place the planet and all objects on its surface in
/// the same grid. The motion of the planet will be inherited by all children
/// in that grid, in high precision."
/// We rotate the Grid so tiles (and future rovers) automatically inherit rotation.
pub fn body_rotation_system(
    celestial_time: Res<CelestialTime>,
    registry: Res<CelestialBodyRegistry>,
    mut q_grids: Query<(&mut Transform, &ReferenceFrame)>,
) {
    for (mut tf, frame) in q_grids.iter_mut() {
        if let Some(body) = frame.body_fixed() {
            if let Some(desc) = registry.get(body) {
                if desc.spins() {
                    // Shared with the geodesy math (`geo::body_rotation`) so
                    // rendered grids and comms/anchor positions cannot diverge.
                    let next = lunco_celestial::geo::body_rotation(desc, celestial_time.epoch_jd)
                        .as_quat();
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
/// no provider or coincident positions).
/// The inputs are typed `EclipticAu` on purpose: this is the exact pipe that once carried
/// EQUATORIAL vectors while claiming to be ecliptic, and put the sun 45° below the horizon at
/// Shackleton. A raw `DVec3` can no longer be handed to it.
/// Publish the calibrated solar irradiance from the current celestial epoch.
/// Direction inputs are resolved independently from the actual tagged target
/// entity by the generic environment probe path, so every probe gets its own
/// BigSpace-relative bearing.
pub fn update_sun_light_system(
    ephemeris: Option<Res<EphemerisResource>>,
    celestial_time: Res<CelestialTime>,
    sun_cal: Option<Res<lunco_environment::LunarSun>>,
    mut sun_state: ResMut<lunco_environment::SunState>,
    // Query the site anchor so observer body is dynamic (Earth 399, Moon 301, etc.)
    q_site: Query<&lunco_celestial::geo::GeodeticAnchor, With<lunco_celestial::geo::SiteAnchor>>,
    orbital_pin: Option<Res<OrbitalViewPin>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let Some(ephemeris) = ephemeris else {
        sun_state.clear();
        return;
    };

    let mut contract_findings = Vec::new();
    let site_count = q_site.iter().count();
    if site_count > 1 {
        contract_findings.push(lunco_core::RuntimeDiagnostic {
            code: "site-anchor-cardinality".to_string(),
            severity: lunco_core::DiagnosticSeverity::Error,
            producer: "celestial-sun".to_string(),
            subject: "SiteAnchor".to_string(),
            message: format!(
                "expected at most one active SiteAnchor, found {site_count}; celestial observation is ambiguous"
            ),
        });
    }
    if !contract_findings.is_empty() {
        sun_state.clear();
        if let Some(mut diagnostics) = diagnostics {
            diagnostics.replace_producer("celestial-sun", contract_findings);
        }
        return;
    }
    if let Some(diagnostics) = diagnostics.as_deref_mut() {
        diagnostics.replace_producer("celestial-sun", std::iter::empty());
    }

    let site_anchor = q_site.single().ok();
    let Some(observer_body) = site_anchor
        .map(|anchor| anchor.body)
        .or_else(|| orbital_pin.as_ref().filter(|p| p.active).map(|p| p.body))
    else {
        sun_state.clear();
        return;
    };

    let (Some(p_sun), Some(p_observer)) = (
        ephemeris
            .provider
            .global_position(lunco_celestial::ephemeris_id::SUN, celestial_time.epoch_jd),
        ephemeris
            .provider
            .global_position(observer_body, celestial_time.epoch_jd),
    ) else {
        sun_state.clear();
        return;
    };

    if site_anchor.is_none() {
        // Orbital views without a site do not own a local ENU light frame.
        // Their authored DistantLight remains render-only; it is not a
        // substitute physical irradiance sample.
        sun_state.clear();
        return;
    }
    let irradiance = sun_cal.as_deref().and_then(|cal| {
        let r2 = (p_sun - p_observer).length_squared();
        (r2 > 1.0e-4).then_some((cal.illuminance_lux as f64 / r2) as f32)
    });
    sun_state.publish(irradiance);
}

pub fn celestial_visuals_system(
    q_camera: Query<
        (Entity, &CellCoord, &Transform),
        (
            With<Camera>,
            With<lunco_embodiment_core::roles::LocalEmbodiment>,
        ),
    >,
    q_bodies: Query<(Entity, &CellCoord, &Transform, &CelestialBody)>,
    mut q_tiles: Query<
        (&mut ShaderLook, &lunco_terrain_globe::TileCoord),
        With<lunco_terrain_globe::TerrainTile>,
    >,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_site: Query<(), With<lunco_celestial::geo::SiteAnchor>>,
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
    // The blueprint grid is an EDITOR affordance, and a scene with a site anchor is
    // not being edited from orbit — it is being stood on. Suppress the ramp there and
    // leave every body fully textured. Apply this before looking for a local avatar:
    // editor and preview cameras do not carry `LocalEmbodiment`, but they still render
    // the same site scene and must not inherit the material's blueprint default.
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
    let camera_abs = if site_anchored {
        None
    } else {
        let Some((cam_ent, cam_cell, cam_tf)) = q_camera.single().ok() else {
            return;
        };
        let Ok(cam_abs) = world_position_seeded(
            cam_ent,
            Some(cam_cell),
            cam_tf,
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            return;
        };
        Some(cam_abs)
    };

    // Per-body camera altitude → per-body texture↔blueprint transition.
    // Body-local coords (camera relative to body center) prevent thrashing
    // at high transport rates — only depends on camera's position relative to the
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
        let transition = if let Some(cam_abs) = camera_abs {
            let Ok(body_abs) = world_position_seeded(
                body_ent,
                Some(body_cell),
                body_tf,
                &q_parents,
                &q_grids,
                &q_spatial,
            ) else {
                continue;
            };
            let altitude = ((cam_abs - body_abs).length() - body.radius_m).max(0.0);
            ((start_transition_alt - altitude) / (start_transition_alt - end_transition_alt))
                .clamp(0.0, 1.0) as f32
        } else {
            0.0
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
    // ramp for an entire cadence interval after a dive — the ramp would visibly lag the
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
