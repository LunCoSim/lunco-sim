use bevy::prelude::*;
use big_space::prelude::*;
use lunco_celestial::CelestialPlugin;
use lunco_celestial::CelestialBody;
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
    fn position(&self, _body_id: i32, epoch_jd: f64) -> Option<lunco_celestial::frames::EclipticAu> {
        Some(lunco_celestial::frames::EclipticAu::new(bevy::math::DVec3::new(epoch_jd, 0.0, 0.0)))
    }
}

/// Build the headless celestial app the tests share (see the notes in
/// `test_celestial_startup_and_movement` for why each piece is here).
///
/// Note the `CelestialBodyDecl` spawns: celestial content is **opt-in per scene**
/// (doc 19 §11e). A scene declares its bodies in USD (`LunCoCelestialBodyAPI` →
/// `CelestialBodyDecl`), and nothing celestial — hierarchy, globes, orbit views,
/// ephemeris — exists without them. These stand in for that declaration, exactly as
/// `assets/celestial/solar_system.usda` does for a real scene.
fn celestial_test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::input::InputPlugin::default());
    app.add_plugins(bevy::transform::TransformPlugin);
    let _ = lunco_assets::register_lunco_asset_sources(&mut app);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.init_resource::<Assets<Mesh>>();
    app.init_asset::<Image>();
    app.add_plugins(CelestialPlugin);
    // The scene asks for a sky: Sun, Earth, Moon.
    for naif in [10, 399, 301] {
        app.world_mut()
            .spawn(lunco_celestial::CelestialBodyDecl { naif });
    }
    app
}

/// **P4 regression — the orbit view must be STAR-FIXED.**
///
/// `big_space_setup`'s doc block claimed "Grid Anchor (inertial) — does NOT
/// rotate", and the Observer Camera was parented to the Earth Grid on the
/// strength of that claim ("On Earth Grid (inertial) for orbit view"). The
/// opposite is true: `body_rotation_system` queries `CelestialReferenceFrame`,
/// which lives on the **grids**, so the Earth Grid spins once per sidereal day
/// and dragged the camera around a ~19,000 km circle with it.
///
/// The camera now hangs off an `InertialAnchor`: tracks Earth's position, never
/// its rotation. Assert exactly that split — the body grid DOES rotate, the
/// camera's parent does NOT, and the two stay co-located.
#[test]
fn observer_camera_hangs_in_a_star_fixed_frame() {
    let mut app = celestial_test_app();
    app.insert_resource(EphemerisResource { provider: Arc::new(StubEphemeris) });
    app.update();

    // The camera's parent must be the inertial anchor, not the rotating grid.
    let mut cam_q = app
        .world_mut()
        .query_filtered::<&ChildOf, With<bevy::camera::Camera>>();
    let parent = cam_q
        .iter(app.world())
        .next()
        .expect("Observer Camera should exist (spawn_observer_camera defaults true)")
        .parent();

    assert!(
        app.world().get::<lunco_celestial::InertialAnchor>(parent).is_some(),
        "the Observer Camera must be parented to an InertialAnchor"
    );
    assert!(
        app.world().get::<lunco_celestial::EarthRoot>(parent).is_none(),
        "…and NOT to the Earth Grid, which rotates once per sidereal day"
    );

    let earth_rot_of = |app: &mut App| -> Quat {
        let mut q = app
            .world_mut()
            .query_filtered::<&Transform, With<lunco_celestial::EarthRoot>>();
        q.iter(app.world()).next().unwrap().rotation
    };
    // Second update: the hierarchy is SPAWNED in `Update`, but `body_rotation_system`
    // runs in `PreUpdate` — so after one frame the grid still sits at identity, and
    // `rot_before` would be identity rather than the grid's epoch rotation. The
    // assertion below would then measure the ABSOLUTE angle at the epoch instead of the
    // 0.33-day delta it claims to. And since the mission epoch is seeded from the WALL
    // clock, that absolute angle is whatever today's GMST happens to be — the test
    // passed or failed depending on the time of day it ran. Step once more so the grid
    // carries its epoch rotation, and the comparison is a true delta.
    app.update();
    let earth_rot_before = earth_rot_of(&mut app);

    // Advance a third of a sidereal day — a ~119° spin.
    {
        let mut mission = app.world_mut().resource_mut::<lunco_time::MissionClock>();
        mission.anchor.epoch0_jd += 0.33;
        mission.mission_epoch0_jd += 0.33;
    }
    app.update();

    // The body grid spun… (compare against ITS OWN prior rotation — the absolute
    // angle vs identity depends on the epoch and could be anything.)
    let earth_rot_after = earth_rot_of(&mut app);
    assert!(
        earth_rot_after.angle_between(earth_rot_before) > 1.0,
        "the Earth Grid must carry the body's spin: 0.33 sidereal days ≈ 119°, \
         but the rotation moved by {:.3} rad",
        earth_rot_after.angle_between(earth_rot_before)
    );

    // …and the camera's frame did NOT.
    let anchor_tf = *app.world().get::<Transform>(parent).unwrap();
    assert!(
        anchor_tf.rotation.angle_between(Quat::IDENTITY) < 1e-6,
        "the InertialAnchor must never rotate — the orbit view is star-fixed \
         (got {:?})",
        anchor_tf.rotation
    );

    // But it still FOLLOWS Earth: same cell + translation as the body grid.
    let mut earth_pose_q = app
        .world_mut()
        .query_filtered::<(&CellCoord, &Transform), With<lunco_celestial::EarthRoot>>();
    let (earth_cell, earth_tf) = earth_pose_q.iter(app.world()).next().unwrap();
    assert_eq!(
        *app.world().get::<CellCoord>(parent).unwrap(),
        *earth_cell,
        "the anchor must track Earth's cell"
    );
    assert!(
        (anchor_tf.translation - earth_tf.translation).length() < 1e-3,
        "the anchor must track Earth's translation"
    );
}

/// Scene reload must tear the sky down **completely** — by architecture, not a
/// maintained despawn list. When the declarations disappear (a scene without bodies is
/// loaded) every celestial-derived entity must be gone: no orbiting ghost bodies, no
/// stale orbit lines, no globe tiles. This is what fixes "reload without sun/earth and
/// it still moves".
#[test]
fn scene_reload_without_bodies_tears_the_whole_sky_down() {
    let mut app = celestial_test_app(); // declares Sun/Earth/Moon
    app.insert_resource(EphemerisResource { provider: Arc::new(StubEphemeris) });
    // Let the hierarchy + orbit views spawn.
    app.update();
    app.update();

    let count_derived = |app: &mut App| {
        app.world_mut()
            .query_filtered::<(), With<lunco_celestial::CelestialDerived>>()
            .iter(app.world())
            .count()
    };
    assert!(count_derived(&mut app) > 0, "the sky should have spawned");

    // Reload into a scene WITHOUT bodies: despawn every `CelestialBodyDecl` (that is
    // what scene-clear does to the USD-projected declaration entities).
    let decls: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<lunco_celestial::CelestialBodyDecl>>()
        .iter(app.world())
        .collect();
    for e in decls {
        app.world_mut().despawn(e);
    }

    // Teardown fires on the next frame and clears everything…
    app.update();
    app.update();
    assert_eq!(
        count_derived(&mut app),
        0,
        "no celestial-derived entity may survive a reload into a body-less scene"
    );
    assert!(
        app.world_mut()
            .query_filtered::<(), With<lunco_celestial::SolarSystemRoot>>()
            .iter(app.world())
            .next()
            .is_none(),
        "the hierarchy root must be gone"
    );

    // …and re-declaring bodies rebuilds it (the idempotent gate, not a spent latch).
    for naif in [10, 399, 301] {
        app.world_mut()
            .spawn(lunco_celestial::CelestialBodyDecl { naif });
    }
    app.update();
    app.update();
    assert!(
        count_derived(&mut app) > 0,
        "re-declaring bodies must rebuild the sky — teardown must not be a one-way latch"
    );
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
    // NO material asset stores, and no `Shader` asset type, any more: the crate is
    // render-free (2026-07-13). It states appearance as INTENT (`PbrLook` /
    // `ShaderLook` components) and never `.add()`s a material or holds a
    // `Handle<Shader>` — `lunco-render-bevy` does both, and this headless app simply
    // never adds it. Which is exactly the property this test now also proves: the
    // whole celestial hierarchy builds and steps with no GPU stack registered at all.
    app.init_asset::<Image>();
    // `GizmoPlugin` is likewise gone — it came from `bevy_gizmos` (a render feature),
    // and nothing in this crate draws gizmos.
    app.add_plugins(CelestialPlugin);
    // The scene declares its bodies — celestial content is opt-in (doc 19 §11e), so
    // without these there is no hierarchy, no globes and no ephemeris at all.
    for naif in [10, 399, 301] {
        app.world_mut()
            .spawn(lunco_celestial::CelestialBodyDecl { naif });
    }
    // Override the NoOp provider (installed by CelestialPlugin) with one whose
    // output depends on the epoch, so the clock seek below actually repositions
    // Earth's grid via `ephemeris_update_system`.
    app.insert_resource(EphemerisResource { provider: Arc::new(StubEphemeris) });

    // Ensure startup systems run
    app.update();
    
    let epoch_before = app.world().resource::<WorldTime>().epoch_jd;
    
    // 1. Verify Sun and Earth exist.
    //
    // `EarthRoot` is the Earth *grid* (a frame) inside the EMB grid. Its pose is
    // `CellCoord × cell_edge + Transform`, and BOTH parts move as it orbits —
    // the cells are real (2 km edges; see `big_space_setup`). Comparing only the
    // `Transform` residual would pass even if the cell were computed wrong, and
    // would break outright the moment Earth crossed a cell boundary. Compose.
    let mut query = app.world_mut().query::<(&lunco_celestial::EarthRoot, &CellCoord, &Transform)>();
    let earth = query.iter(app.world()).next().expect("No EarthRoot found");
    let earth_pose_1 = (*earth.1, earth.2.translation);
    
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
    
    // 3. Verify Earth has moved.
    let mut grid_q = app.world_mut().query::<(&lunco_celestial::EMBRoot, &big_space::prelude::Grid)>();
    let edge = grid_q
        .iter(app.world())
        .next()
        .expect("No EMBRoot grid found")
        .1
        .cell_edge_length() as f64;

    let mut query = app.world_mut().query::<(&lunco_celestial::EarthRoot, &CellCoord, &Transform)>();
    let earth = query.iter(app.world()).next().expect("No EarthRoot found");
    let earth_pose_2 = (*earth.1, earth.2.translation);

    let compose = |(cell, tf): (CellCoord, bevy::math::Vec3)| {
        bevy::math::DVec3::new(
            cell.x as f64 * edge + tf.x as f64,
            cell.y as f64 * edge + tf.y as f64,
            cell.z as f64 * edge + tf.z as f64,
        )
    };
    let moved = (compose(earth_pose_2) - compose(earth_pose_1)).length();

    // Earth about the EMB traces a ~4.7e6 m radius circle once a month, so 10
    // days must move it by megametres. A bare `assert_ne!` on the residual would
    // also pass on a one-ULP wobble.
    assert!(
        moved > 1.0e6,
        "Earth should have moved megametres about the EMB after 10 days, moved {moved:.3e} m \
         (the spine re-derived the epoch and the ephemeris repositioned the grid)"
    );

    // The cells must actually be carrying the magnitude — a regression to
    // `switching_threshold = 1e30` (cells always zero, position entirely in an
    // f32 `Transform`) is what destroyed render precision. See
    // `tests/grid_cell_edge_precision.rs`.
    assert_ne!(
        earth_pose_2.0,
        CellCoord::default(),
        "Earth's CellCoord must be non-zero: its 4.7e6 m offset cannot live in an f32 Transform"
    );
}

/// **The sun may only be steered once the site frame is REAL.**
///
/// A scene that opts into bodies but anchors no site (the flat sandbox referencing
/// `solar_system.usda`) has a `SiteAlignGrid` — it is spawned with the hierarchy —
/// but `anchor_solar_frame_to_site` never writes a rotation to it, so it holds
/// IDENTITY. Gating `update_sun_light_system` on the grid's PRESENCE therefore read
/// that identity as a known ecliptic→world rotation and aimed the scene's brightest
/// `DirectionalLight` down the raw ecliptic vector — along the horizon, arena unlit.
///
/// The gate is `SiteAligned`, which only the writer sets. With no anchor, no sun
/// steering may happen AT ALL — asserted on `SunDirectionWorld`, the system's own
/// published output, rather than on one light: the steering picks the BRIGHTEST
/// `DirectionalLight`, so an assertion aimed at a particular light passes for the
/// irrelevant reason that some other light won the max.
#[test]
fn an_unanchored_celestial_scene_keeps_its_authored_sun() {
    // The ephemeris must be NON-DEGENERATE, or `sun_emit_direction` returns `None`
    // and the system early-returns before ever reaching the gate — a test that
    // passes without exercising anything. `StubEphemeris` puts every body at the
    // same place at JD 0, which is exactly that degenerate case.
    #[derive(Debug)]
    struct SunAndMoon;
    impl EphemerisProvider for SunAndMoon {
        fn position(&self, body_id: i32, _jd: f64) -> Option<lunco_celestial::frames::EclipticAu> {
            Some(match body_id {
                301 => lunco_celestial::frames::EclipticAu::new(bevy::math::DVec3::new(1.0, 0.0, 0.0)),
                _ => lunco_celestial::frames::EclipticAu::ZERO,
            })
        }
    }

    let mut app = celestial_test_app();
    app.insert_resource(EphemerisResource { provider: Arc::new(SunAndMoon) });

    // The sandbox's own light: the brightest `DirectionalLight`, aimed by hand.
    let authored = Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.7, -0.9, 0.0));
    let light = app
        .world_mut()
        .spawn((DirectionalLight { illuminance: 128_000.0, ..default() }, authored))
        .id();

    for _ in 0..8 {
        app.update();
    }

    // CONTROL for the assertion itself: the celestial hierarchy really did come up,
    // so this is "the gate held", not "nothing ran".
    let mut q = app
        .world_mut()
        .query_filtered::<(), With<lunco_celestial::SiteAlignGrid>>();
    assert_eq!(
        q.iter(app.world()).count(),
        1,
        "the align grid must exist — otherwise this test proves nothing about the gate"
    );
    let mut q_aligned = app
        .world_mut()
        .query_filtered::<(), With<lunco_celestial::SiteAligned>>();
    assert_eq!(
        q_aligned.iter(app.world()).count(),
        0,
        "no site is anchored, so no align rotation may be claimed as established"
    );

    // CONTROL for the ephemeris: it must be able to produce a direction, or the
    // system early-returns and the gate is never reached.
    let ephem = app.world().resource::<EphemerisResource>();
    assert!(
        lunco_celestial::sun_emit_direction(
            ephem.provider.global_position(10, 0.0).unwrap(),
            ephem.provider.global_position(301, 0.0).unwrap(),
        )
        .is_some(),
        "the stub ephemeris is degenerate — this test would pass without steering ever \
         being attempted"
    );

    assert_eq!(
        app.world().resource::<lunco_celestial::SunDirectionWorld>().0,
        Vec3::ZERO,
        "an unanchored scene has no known ecliptic→world rotation, so the sun must not be \
         steered at all — a direction here is the raw ecliptic vector aimed along the horizon"
    );
    let after = *app.world().entity(light).get::<Transform>().unwrap();
    assert_eq!(
        after.rotation, authored.rotation,
        "an unanchored scene's authored sun must not be re-aimed by the ephemeris"
    );
}
