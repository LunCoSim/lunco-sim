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
//! This reproduces the real Solar → EMB → Moon → Surface chain, with the
//! floating origin nested in the surface grid as the production camera is, and
//! measures where a point 10 m from the site actually renders. No ancestor is
//! re-posed to follow that origin.
//!
//! With the old edges (Solar 1e9, EMB 1e8) the error is tens of metres — the
//! "lunar surface jitters / Earth jitters / orbit lines jump" report. With 2 km
//! cells it is sub-millimetre.

use bevy::math::DVec3;
use bevy::prelude::*;
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
    surface: (2_000.0, 100.0),
};

const NEW: Chain = Chain {
    solar: (2_000.0, 100.0),
    emb: (2_000.0, 100.0),
    body: (2_000.0, 100.0),
    surface: (1_000.0, 100.0),
};

/// Build the chain and return the probe's rendered offset relative to the
/// nested floating origin.
fn probe_error_m(chain: &Chain) -> f64 {
    (probe_render_pos(chain, DVec3::ZERO) - PROBE_LOCAL.as_dvec3()).length()
}

/// Where the probe actually renders, in metres relative to the nested floating
/// origin. `drift` advances the EMB along its orbit, simulating an epoch tick;
/// the local surface pose must not be changed by that upstream motion.
fn probe_render_pos(chain: &Chain, drift: DVec3) -> DVec3 {
    let emb_in_solar = EMB_IN_SOLAR + drift;
    let root = Grid::new(2_000.0, 100.0);
    let solar = Grid::new(chain.solar.0, chain.solar.1);
    let emb = Grid::new(chain.emb.0, chain.emb.1);
    let body = Grid::new(chain.body.0, chain.body.1);
    let surface = Grid::new(chain.surface.0, chain.surface.1);

    // Store each frame's pose the way the real systems do: split through the
    // PARENT grid. The renderer composes these native BigSpace values; there is
    // no compensating world-origin translation.
    let (emb_cell, emb_tf) = solar.translation_to_grid(emb_in_solar);
    let (moon_cell, moon_tf) = emb.translation_to_grid(MOON_IN_EMB);
    let (surf_cell, surf_tf) = body.translation_to_grid(SITE_IN_MOON);

    let (solar_cell, solar_tf) = root.translation_to_grid(DVec3::ZERO);

    let mut app = App::new();
    app.add_plugins(BigSpaceMinimalPlugins);

    let probe;
    {
        let world = app.world_mut();

        probe = world
            .spawn((
                Transform::from_translation(PROBE_LOCAL),
                CellCoord::default(),
            ))
            .id();
        let surface_e = world
            .spawn((surface, Transform::from_translation(surf_tf), surf_cell))
            .add_children(&[probe])
            .id();
        // Production surface cameras claim the floating origin in the site
        // grid. Keeping it in this nested frame is what makes local f32
        // transforms independent of AU-scale parent motion.
        let origin = world
            .spawn((Transform::default(), CellCoord::default(), FloatingOrigin))
            .id();
        world.entity_mut(surface_e).add_child(origin);
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
            .add_children(&[solar_e]);
    }

    app.update();

    let probe_gt = app.world().get::<GlobalTransform>(probe).unwrap();
    let origin_gt = app
        .world()
        .iter_entities()
        .find(|e| e.contains::<FloatingOrigin>())
        .and_then(|e| e.get::<GlobalTransform>())
        .expect("nested floating origin global transform");
    probe_gt.translation().as_dvec3() - origin_gt.translation().as_dvec3()
}

#[test]
fn coarse_parent_cells_destroy_split_precision() {
    let grid = Grid::new(OLD.solar.0, OLD.solar.1);
    let (cell, local) = grid.translation_to_grid(EMB_IN_SOLAR);
    let reconstructed = DVec3::new(cell.x as f64, cell.y as f64, cell.z as f64)
        * grid.cell_edge_length() as f64
        + local.as_dvec3();
    let err = (reconstructed - EMB_IN_SOLAR).length();
    println!("OLD (Solar 1e9 / EMB 1e8) split error: {err:.4} m");
    // The historical config loses metres in the f32 cell-local remainder.
    // Assert the representation failure directly rather than inferring it from
    // a nested local pose whose parent motion is supposed to cancel.
    assert!(
        err > 1.0,
        "expected the old 1e9/1e8 m cells to lose >1 m, got {err:.6} m — \
         if this now passes, the cell-local precision contract changed"
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

/// One frame at 1x is ~26 m of EMB orbital motion (30 km/s). Moving that parent
/// must not alter a probe's local surface pose when the origin is nested in the
/// surface frame.
const ONE_FRAME_OF_ORBIT: DVec3 = DVec3::new(19.4, 0.0, -17.6); // |.| ~26 m

#[test]
fn nested_surface_origin_is_invariant_when_parent_grid_moves() {
    let a = probe_render_pos(&NEW, DVec3::ZERO);
    let b = probe_render_pos(&NEW, ONE_FRAME_OF_ORBIT);
    let jitter = (b - a).length();
    println!("native nested-grid local pose change: {jitter:.9} m");
    assert!(
        jitter < 1.0e-3,
        "native nested-grid camera pose must remain stable to sub-mm, got {jitter:.9} m"
    );
}
