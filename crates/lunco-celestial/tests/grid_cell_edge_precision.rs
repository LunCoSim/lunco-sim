//! A grid's `cell_edge_length` is a PRECISION knob, not a scale knob.
//!
//! `LocalFloatingOrigin::translation` is an **f32** holding the floating
//! origin's offset within one cell of that grid, bounded by
//! `maximum_distance_from_origin = edge/2 + switching_threshold`. When
//! big_space pushes the origin down the tree (`propagate_origin_to_child`) it
//! rebuilds the origin's position as `cells×edge` (exact f64) PLUS that f32.
//! Re-splitting at the child cannot recover bits the parent already dropped, so
//! the COARSEST grid in a chain sets the precision floor for its whole subtree.
//!
//! This reproduces the real Solar → EMB → Moon → Surface chain, with the site
//! pinned to the root origin exactly as `anchor_solar_frame_to_site` does, and
//! measures where a point 10 m from the site actually renders.
//!
//! With the old edges (Solar 1e9, EMB 1e8) the error is tens of metres — the
//! "lunar surface jitters / Earth jitters / orbit lines jump" report. With 2 km
//! cells it is sub-millimetre.

use bevy::prelude::*;
use bevy::math::DVec3;
use big_space::plugin::BigSpaceMinimalPlugins;
use big_space::prelude::*;

/// Realistic magnitudes (metres), bevy-ecliptic axes. Rotations omitted: they
/// are irrelevant to the cell/f32 split this test isolates.
///
/// These MUST NOT be round numbers. Round magnitudes (1.2e11, 2.9e8, …) leave
/// cell remainders that happen to be exactly representable in f32 — e.g.
/// 2.897e8 / ulp(32) = 9053125, an integer below 2^24 — so nothing rounds and
/// even the broken config measures zero error. Real ephemeris values never land
/// on f32 grid points; these mimic that.
const EMB_IN_SOLAR: DVec3 = DVec3::new(1.200_345_713e11, 3.214_907e9, -8.800_753_119e10); // ~1 AU
const MOON_IN_EMB: DVec3 = DVec3::new(-2.901_733_71e8, 1.337_411e7, 2.402_119_3e8); // ~3.8e8 m
const SITE_IN_MOON: DVec3 = DVec3::new(3.001_373e5, -1.700_217_9e6, 1.003_791e5); // ~1.74e6 m

/// Offset of the probe from the site, in the surface grid. This is the scale a
/// standing observer actually sees.
const PROBE_LOCAL: Vec3 = Vec3::new(10.0, 0.0, 0.0);

struct Chain {
    solar: (f32, f32),
    emb: (f32, f32),
    body: (f32, f32),
    surface: (f32, f32),
}

const OLD: Chain = Chain {
    solar: (1.0e9, 1.0e8),
    emb: (1.0e8, 1.0e7),
    body: (10_000.0, 1_000.0),
    surface: (1_000.0, 100.0),
};

const NEW: Chain = Chain {
    solar: (2_000.0, 100.0),
    emb: (2_000.0, 100.0),
    body: (2_000.0, 100.0),
    surface: (1_000.0, 100.0),
};

/// Build the chain, pin the site onto the root origin, return the probe's
/// rendered offset error in metres.
fn probe_error_m(chain: &Chain) -> f64 {
    (probe_render_pos(chain, DVec3::ZERO) - PROBE_LOCAL.as_dvec3()).length()
}

/// Where the probe actually renders, in metres relative to the floating origin.
/// `drift` advances the EMB along its orbit, simulating an epoch tick: the pin
/// re-anchors, so a correct pipeline must return the SAME value every time.
fn probe_render_pos(chain: &Chain, drift: DVec3) -> DVec3 {
    let emb_in_solar = EMB_IN_SOLAR + drift;
    let root = Grid::new(2_000.0, 100.0);
    let solar = Grid::new(chain.solar.0, chain.solar.1);
    let emb = Grid::new(chain.emb.0, chain.emb.1);
    let body = Grid::new(chain.body.0, chain.body.1);
    let surface = Grid::new(chain.surface.0, chain.surface.1);

    // Store each frame's pose the way the real systems do: split through the
    // PARENT grid. Read the stored values back in f64 (exact) so the pin can
    // cancel what the renderer will actually compose.
    let (emb_cell, emb_tf) = solar.translation_to_grid(emb_in_solar);
    let (moon_cell, moon_tf) = emb.translation_to_grid(MOON_IN_EMB);
    let (surf_cell, surf_tf) = body.translation_to_grid(SITE_IN_MOON);

    let stored = |g: &Grid, c: CellCoord, t: Vec3| -> DVec3 {
        DVec3::new(
            c.x as f64 * g.cell_edge_length() as f64 + t.x as f64,
            c.y as f64 * g.cell_edge_length() as f64 + t.y as f64,
            c.z as f64 * g.cell_edge_length() as f64 + t.z as f64,
        )
    };
    // Site position in the Solar frame, composed from the STORED chain.
    let site_in_solar =
        stored(&solar, emb_cell, emb_tf) + stored(&emb, moon_cell, moon_tf) + stored(&body, surf_cell, surf_tf);

    // The pin: slide the Solar Grid so the site lands on the root origin.
    let (solar_cell, solar_tf) = root.translation_to_grid(-site_in_solar);

    let mut app = App::new();
    app.add_plugins(BigSpaceMinimalPlugins);

    let probe;
    {
        let world = app.world_mut();

        // Floating origin sits at the root origin — the parked camera.
        let origin = world
            .spawn((Transform::default(), CellCoord::default(), FloatingOrigin))
            .id();

        probe = world
            .spawn((Transform::from_translation(PROBE_LOCAL), CellCoord::default()))
            .id();
        let surface_e = world
            .spawn((surface, Transform::from_translation(surf_tf), surf_cell))
            .add_children(&[probe])
            .id();
        let body_e = world
            .spawn((body, Transform::from_translation(moon_tf), moon_cell))
            .add_children(&[surface_e])
            .id();
        let emb_e = world
            .spawn((emb, Transform::from_translation(emb_tf), emb_cell))
            .add_children(&[body_e])
            .id();
        let solar_e = world
            .spawn((solar, Transform::from_translation(solar_tf), solar_cell))
            .add_children(&[emb_e])
            .id();

        world
            .spawn(BigSpaceRootBundle::default())
            .insert(root)
            .add_children(&[origin, solar_e]);
    }

    app.update();

    let gt = app.world().get::<GlobalTransform>(probe).unwrap();
    // The pin cancels the stored chain exactly in f64, so the probe MUST render
    // at PROBE_LOCAL relative to the floating origin. Anything else is
    // precision lost inside big_space's origin propagation.
    gt.translation().as_dvec3()
}

#[test]
fn coarse_cells_destroy_surface_precision() {
    let err = probe_error_m(&OLD);
    println!("OLD (Solar 1e9 / EMB 1e8): probe renders {err:.4} m off");
    // The historical config loses metres. Assert it really is broken, so this
    // test documents the failure mode rather than silently passing if big_space
    // ever changes.
    assert!(
        err > 1.0,
        "expected the old 1e9/1e8 m cells to lose >1 m, got {err:.6} m — \
         if this now passes, big_space made LocalFloatingOrigin::translation f64 \
         and the cell-edge precision constraint is gone"
    );
}

#[test]
fn two_km_cells_keep_surface_precision_sub_millimetre() {
    let err = probe_error_m(&NEW);
    println!("NEW (all 2 km cells):      probe renders {err:.9} m off");
    assert!(
        err < 1.0e-3,
        "2 km cells must render a lunar-surface point to sub-mm, got {err:.9} m"
    );
}

/// A constant offset would be invisible. What the eye sees is the offset
/// CHANGING as the epoch advances and the pin re-anchors the tree. One frame at
/// 1x is ~26 m of EMB orbital motion (30 km/s); the site is pinned, so a correct
/// pipeline renders the probe at the identical place both times.
const ONE_FRAME_OF_ORBIT: DVec3 = DVec3::new(19.4, 0.0, -17.6); // |.| ~26 m

#[test]
fn coarse_cells_make_the_surface_jitter_between_frames() {
    let a = probe_render_pos(&OLD, DVec3::ZERO);
    let b = probe_render_pos(&OLD, ONE_FRAME_OF_ORBIT);
    let jitter = (b - a).length();
    println!("OLD per-frame jitter of a point 10 m away: {jitter:.4} m");
    assert!(
        jitter > 0.1,
        "expected the old cells to jitter >0.1 m per frame, got {jitter:.6} m"
    );
}

#[test]
fn two_km_cells_do_not_jitter_between_frames() {
    let a = probe_render_pos(&NEW, DVec3::ZERO);
    let b = probe_render_pos(&NEW, ONE_FRAME_OF_ORBIT);
    let jitter = (b - a).length();
    println!("NEW per-frame jitter of a point 10 m away: {jitter:.9} m");
    assert!(
        jitter < 1.0e-3,
        "2 km cells must hold a pinned surface point still to sub-mm, got {jitter:.9} m"
    );
}
