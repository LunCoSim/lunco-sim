use bevy::prelude::*;
use big_space::prelude::*;
use lunco_celestial::CelestialPlugin;
use lunco_celestial::CelestialBody;
use lunco_materials::ShaderMaterial;
use lunco_time::WorldTime;
use lunco_celestial::{EphemerisProvider, EphemerisResource};
use std::sync::Arc;

/// Test ephemeris that returns an **epoch-dependent** position, so advancing the
/// clock provably moves a body. The default `NoOpEphemerisProvider` returns
/// `ZERO` at every epoch — it can't validate motion (Earth stays pinned at the
/// origin), which is why this test only ever exercised motion with a real
/// provider. The scale (AU per day) is large enough that a 10-day step shifts
/// Earth across many `Grid` cells, so the `CellCoord` change is unambiguous.
#[derive(Debug)]
struct StubEphemeris;
impl EphemerisProvider for StubEphemeris {
    fn position(&self, _body_id: i32, epoch_jd: f64) -> bevy::math::DVec3 {
        bevy::math::DVec3::new(epoch_jd, 0.0, 0.0)
    }
}

#[test]
fn test_celestial_startup_and_movement() {
    let mut app = App::new();

    // Minimum plugins for headless simulation
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::input::InputPlugin::default());
    app.add_plugins(bevy::transform::TransformPlugin);
    // `setup_big_space_hierarchy` loads `cached_textures://earth.png` at Startup.
    // The source must be registered *before* `AssetPlugin`, else bevy 0.18 panics
    // on the async load task (it resolves the source off-thread). The app entry
    // registers these; the test must too — otherwise it only passed by timing
    // luck (the load task never ran before the 2 `update()`s completed).
    let _ = lunco_assets::register_lunco_asset_sources(&mut app);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.init_resource::<Assets<Mesh>>();
    app.init_resource::<Assets<StandardMaterial>>();
    // `init_asset` (not `init_resource`): `setup_big_space_hierarchy` does
    // `asset_server.load("shaders/blueprint.wgsl")`, and bevy 0.18 panics on the
    // async load task unless the Shader asset *type* is registered (handle
    // provider), which only `init_asset` does. (Image is already `init_asset`'d
    // below; Mesh/StandardMaterial/ShaderMaterial are only `.add()`'d, so the
    // plain resource suffices for them.)
    app.init_asset::<bevy_shader::Shader>();
    app.init_resource::<Assets<ShaderMaterial>>();
    app.init_asset::<Image>();
    app.add_plugins(bevy::gizmos::GizmoPlugin);
    app.add_plugins(CelestialPlugin);
    // Override the NoOp provider (installed by CelestialPlugin) with one whose
    // output depends on the epoch, so the clock seek below actually repositions
    // Earth's grid via `ephemeris_update_system`.
    app.insert_resource(EphemerisResource { provider: Arc::new(StubEphemeris) });

    // Ensure startup systems run
    app.update();
    
    let epoch_before = app.world().resource::<WorldTime>().epoch_jd;
    
    // 1. Verify Sun and Earth exist
    let mut query = app.world_mut().query::<(&lunco_celestial::EarthRoot, &CellCoord, &Transform)>();
    let earth = query.iter(app.world()).next().expect("No EarthRoot found");
    // EarthRoot is the Earth *grid* (a frame). Its orbital motion lands in the
    // `Transform` residual, NOT `CellCoord`: `emb_grid`'s big_space
    // `switching_threshold` is 1e30, so the body never crosses a cell boundary
    // and `CellCoord` stays (0,0,0) — the position lives entirely in `Transform`.
    let earth_tf_1 = earth.2.translation;
    
    // 2. Advance the clock by 10 days. The epoch is a *derived* view
    //    (`WorldTime.epoch_jd`, written by the `lunco-time` spine each frame), so
    //    seek via the authority — re-anchor the `MissionClock` epoch. The spine
    //    then re-derives `WorldTime.epoch_jd` and the ephemeris follows.
    {
        let mut mission = app.world_mut().resource_mut::<lunco_time::MissionClock>();
        mission.anchor.epoch0_jd += 10.0;
        mission.mission_epoch0_jd += 10.0;
    }

    app.update();

    // Sanity: the seek propagated through the spine to the derived epoch.
    let epoch_after = app.world().resource::<WorldTime>().epoch_jd;
    assert!(
        (epoch_after - (epoch_before + 10.0)).abs() < 1e-3,
        "derived epoch should track the MissionClock re-anchor (+10 days)"
    );
    
    // 3. Verify Earth has moved
    let mut query = app.world_mut().query::<(&lunco_celestial::EarthRoot, &CellCoord, &Transform)>();
    let earth = query.iter(app.world()).next().expect("No EarthRoot found");
    let earth_tf_2 = earth.2.translation;

    assert_ne!(
        earth_tf_1, earth_tf_2,
        "Earth's position should have changed after advancing the clock 10 days \
         (the spine re-derived the epoch and the ephemeris repositioned the grid)"
    );
}
