//! Terrain curvature must be a pure function of the DOCUMENT — never of ECS
//! iteration order.
//!
//! The body radius folds into the `SurfaceOracle` as the final `BodyCurvature`
//! modifier, so it decides the composed GEOMETRY and the `content_key` every
//! tile/derived-map cache keys on. It used to be resolved by
//! `q_site.iter().next()` — an arbitrary archetype-order pick. A scene carrying a
//! second `SiteAnchor` (ground stations author body 399 Earth) could curve a lunar
//! DEM to Earth's 6371 km radius, and *which* anchor won varied per launch with
//! async USD load order: terrain that differed every boot and re-baked its whole
//! cache. These tests pin the fix.

use bevy::prelude::*;
use lunco_celestial::geo::{Geodetic, GeodeticAnchor, SiteAnchor};
use lunco_terrain_surface::{TerrainBodyCurvature, TerrainGeoref};

const MOON: i32 = 301;
const EARTH: i32 = 399;
const MOON_R: f64 = 1_737_400.0;
const EARTH_R: f64 = 6_371_000.0;

/// Minimal DEM oracle for a terrain entity — the curvature sync only reads its
/// half extent.
fn dem_height_field() -> lunco_terrain_surface::DemHeightField {
    use lunco_obstacle_field::field::HeightGrid;
    use lunco_terrain_surface::SurfaceOracle;
    lunco_terrain_surface::DemHeightField(std::sync::Arc::new(SurfaceOracle::bare(
        std::sync::Arc::new(HeightGrid::new_flat(9, 4000.0)),
    )))
}

fn body(name: &str, ephemeris_id: i32, radius_m: f64) -> lunco_celestial::registry::BodyDescriptor {
    lunco_celestial::registry::BodyDescriptor {
        name: name.to_string(),
        ephemeris_id,
        radius_m,
        gm: 0.0,
        soi_radius_m: None,
        parent_id: None,
        iau: None,
    }
}

fn app_with_registry() -> App {
    let mut app = App::new();
    app.insert_resource(lunco_celestial::registry::CelestialBodyRegistry {
        bodies: vec![body("Moon", MOON, MOON_R), body("Earth", EARTH, EARTH_R)],
    });
    app.add_systems(Update, lunco_celestial::placement::sync_terrain_body_curvature);
    app
}

/// Spawn the anchors in the given order, plus one Moon terrain. Returns the
/// curvature radius the sync resolved.
fn curvature_with_anchor_order(anchors: &[i32]) -> f64 {
    let mut app = app_with_registry();
    for &body in anchors {
        app.world_mut().spawn((
            SiteAnchor,
            GeodeticAnchor { body, geodetic: Geodetic::new(0.0, 0.0, 0.0) },
        ));
    }
    // The terrain itself authors the Moon — that is the document's answer.
    app.world_mut().spawn((
        dem_height_field(),
        TerrainGeoref { body: MOON, ..Default::default() },
    ));
    app.update();
    app.world()
        .get_resource::<TerrainBodyCurvature>()
        .expect("curvature resource")
        .radius_m
}

/// THE REGRESSION: an Earth ground-station anchor loading before/after the lunar
/// site must not change the terrain's curvature. Under the old
/// `q_site.iter().next()` pick these two orders disagreed — Earth's radius won
/// whenever its anchor spawned first.
#[test]
fn anchor_spawn_order_does_not_change_curvature() {
    let earth_first = curvature_with_anchor_order(&[EARTH, MOON]);
    let moon_first = curvature_with_anchor_order(&[MOON, EARTH]);
    assert_eq!(
        earth_first, moon_first,
        "curvature must not depend on SiteAnchor spawn order"
    );
    assert_eq!(
        earth_first, MOON_R,
        "the terrain authors body 301, so it must curve to the Moon regardless of \
         which anchors exist"
    );
}

/// A single Earth anchor must not hijack a terrain that authors the Moon.
#[test]
fn foreign_anchor_does_not_hijack_terrain_body() {
    assert_eq!(curvature_with_anchor_order(&[EARTH]), MOON_R);
}

/// The terrain's own authored body is what curves it.
#[test]
fn terrain_body_selects_the_radius() {
    let mut app = app_with_registry();
    app.world_mut().spawn((
        SiteAnchor,
        GeodeticAnchor { body: MOON, geodetic: Geodetic::new(0.0, 0.0, 0.0) },
    ));
    app.world_mut().spawn((
        dem_height_field(),
        TerrainGeoref { body: EARTH, ..Default::default() },
    ));
    app.update();
    assert_eq!(
        app.world().get_resource::<TerrainBodyCurvature>().unwrap().radius_m,
        EARTH_R,
        "an Earth-authored terrain curves to Earth even under a Moon site anchor"
    );
}

/// A terrain with no authored georef falls back to the SAME default the celestial
/// bridge uses (Moon) — deterministically, not to whatever anchor exists.
#[test]
fn unauthored_terrain_defaults_to_moon() {
    let mut app = app_with_registry();
    app.world_mut().spawn((
        SiteAnchor,
        GeodeticAnchor { body: EARTH, geodetic: Geodetic::new(0.0, 0.0, 0.0) },
    ));
    app.world_mut().spawn(dem_height_field());
    app.update();
    assert_eq!(
        app.world().get_resource::<TerrainBodyCurvature>().unwrap().radius_m,
        MOON_R
    );
    assert_eq!(TerrainGeoref::default().body, lunco_terrain_surface::DEFAULT_ANCHOR_BODY);
}

/// No site anchor → no curvature (the gate is unchanged by the fix).
#[test]
fn no_site_anchor_means_no_curvature() {
    let mut app = app_with_registry();
    app.world_mut().spawn((
        dem_height_field(),
        TerrainGeoref { body: MOON, ..Default::default() },
    ));
    app.update();
    assert!(app.world().get_resource::<TerrainBodyCurvature>().is_none());
}
