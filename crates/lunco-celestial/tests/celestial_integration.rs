use bevy::prelude::*;
use big_space::prelude::*;
use lunco_celestial::CelestialPlugin;
use lunco_celestial::{EphemerisProvider, EphemerisResource};
use lunco_time::WorldTime;
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
    fn position(
        &self,
        _body_id: i32,
        epoch_jd: f64,
    ) -> Option<lunco_celestial::frames::EclipticAu> {
        Some(lunco_celestial::frames::EclipticAu::new(
            bevy::math::DVec3::new(epoch_jd, 0.0, 0.0),
        ))
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
    app.add_plugins(bevy::input::InputPlugin);
    app.add_plugins(bevy::transform::TransformPlugin);
    let _ = lunco_assets::register_lunco_asset_sources(&mut app);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.init_resource::<Assets<Mesh>>();
    app.init_asset::<Image>();
    app.add_plugins(CelestialPlugin);
    // The scene asks for a sky: Sun, Earth, Moon.
    for naif in [
        lunco_celestial::ephemeris_id::SUN,
        lunco_celestial::ephemeris_id::EARTH,
        lunco_celestial::ephemeris_id::MOON,
    ] {
        app.world_mut()
            .spawn(lunco_celestial::CelestialBodyDecl { naif });
    }
    app
}

/// **The `SolarSystemRoot` invariant: exactly one bearer, and it is the Grid.**
///
/// `placement::anchor_solar_frame_to_site` takes this marker with `single_mut()`.
/// A second bearer makes that call `Err`, so the solar frame is never anchored,
/// `SiteAligned` is never inserted, and `update_sun_light_system` falls back to an
/// identity alignment that aims the sun along raw ecliptic axes — below the local
/// horizon. The scene renders black.
///
/// That is what the hierarchy actually shipped: the Sun body carried the marker as
/// well as the Solar Grid. It stayed invisible because three of the four readers ask
/// only *"does a sky exist"*, which answers the same for one bearer or five, and the
/// fourth — the one that needed the identity — defended itself with a local
/// `With<Grid>` filter. The invariant was never true; it was worked around at the
/// single call site that would otherwise have noticed.
///
/// Asserted here rather than left to that runtime `warn!`, which can only fire once
/// the scene is already unlit, and only in a session someone is watching.
#[test]
fn solar_system_root_is_singular() {
    let mut app = celestial_test_app();
    app.update();

    let bearers: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<lunco_celestial::SolarSystemRoot>>()
        .iter(app.world())
        .collect();

    assert_eq!(
        bearers.len(),
        1,
        "`SolarSystemRoot` must name exactly one entity — found {}. \
         `anchor_solar_frame_to_site` reads it with `single_mut()`, so a second \
         bearer leaves the site frame unanchored and the scene unlit.",
        bearers.len()
    );

    // …and that one is the Solar Grid. The `single_mut()` query also demands
    // `With<Grid>`, so a lone bearer that is NOT a grid matches nothing and fails
    // in exactly the same silent way as two bearers do.
    assert!(
        app.world().get::<Grid>(bearers[0]).is_some(),
        "the `SolarSystemRoot` bearer must be the Solar Grid itself"
    );

    // The specific regression. The Sun is a BODY, reached through `CelestialBody`
    // like Earth and Moon; it must never also be the answer to "where is the solar
    // frame?". Checked by name rather than by count so a re-add is named, not just
    // counted.
    let sun_is_root = bearers.iter().any(|&e| {
        app.world()
            .get::<lunco_celestial::CelestialBody>(e)
            .is_some_and(|b| b.name == "Sun")
    });
    assert!(
        !sun_is_root,
        "the Sun body must not carry `SolarSystemRoot` — it is found through \
         `CelestialBody`/ephemeris 10, not through the frame marker"
    );
}

/// A scene projection creates its body declarations and site anchor together,
/// but the celestial hierarchy is deferred until the end of that Update.  The
/// anchor pass therefore has to wait for the Solar Grid instead of diagnosing a
/// healthy first load as an unlit scene; the `Added<SolarSystemRoot>` cadence
/// edge must then run it on the following frame.
#[test]
fn site_anchor_waits_for_the_deferred_solar_grid_then_aligns() {
    let mut app = celestial_test_app();
    app.insert_resource(EphemerisResource {
        provider: Arc::new(StubEphemeris),
    });
    app.world_mut().spawn((
        lunco_celestial::geo::SiteAnchor,
        lunco_celestial::geo::GeodeticAnchor {
            body: lunco_celestial::ephemeris_id::MOON,
            geodetic: lunco_celestial::geo::Geodetic::new(26.13, 3.63, 0.3),
        },
    ));

    // The grid is created in Update, after this frame's PreUpdate anchor pass.
    app.update();
    assert!(
        app.world_mut()
            .query_filtered::<(), With<lunco_celestial::SolarSystemRoot>>()
            .iter(app.world())
            .next()
            .is_some(),
        "the declared bodies must create the Solar Grid"
    );
    assert!(
        app.world_mut()
            .query_filtered::<(), With<lunco_celestial::SiteAligned>>()
            .iter(app.world())
            .next()
            .is_none(),
        "the site cannot align before the deferred Solar Grid exists"
    );

    // `Added<SolarSystemRoot>` invalidates the cadence gate, so this is not
    // delayed until the next clock-tolerance interval.
    app.update();
    assert!(
        app.world_mut()
            .query_filtered::<(), With<lunco_celestial::SiteAligned>>()
            .iter(app.world())
            .next()
            .is_some(),
        "the site must align on the first frame after the Solar Grid appears"
    );
}

/// **P4 regression — the orbit view must be STAR-FIXED.**
///
/// `big_space_setup`'s doc block claimed "Grid Anchor (inertial) — does NOT
/// rotate", and the Observer Camera was parented to the Earth Grid on the
/// strength of that claim ("On Earth Grid (inertial) for orbit view"). The
/// opposite is true: `body_rotation_system` rotates only
/// `ReferenceFrame::BodyFixed`, so the Earth body-fixed Grid spins once per sidereal day
/// and dragged the camera around a ~19,000 km circle with it.
///
/// The camera now hangs off an `EclipticJ2000` frame: tracks Earth's position, never
/// its rotation. Assert exactly that split — the body grid DOES rotate, the
/// camera's parent does NOT, and the two stay co-located.
#[test]
fn observer_camera_hangs_in_a_star_fixed_frame() {
    let mut app = celestial_test_app();
    app.insert_resource(EphemerisResource {
        provider: Arc::new(StubEphemeris),
    });
    app.update();

    // The camera's parent must be the inertial anchor, not the rotating grid.
    // The headless test app deliberately does not install the render plugin that
    // turns this authored observer entity into a Bevy `Camera3d`. The celestial
    // contract is the entity's authored identity and inertial parent; the render
    // projection is a downstream client concern.
    let mut cam_q = app
        .world_mut()
        .query_filtered::<(&ChildOf, &Name), With<lunco_core::Avatar>>();
    let parent = cam_q
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "Observer Camera")
        .map(|(child, _)| child.parent())
        .expect("Observer Camera should exist (spawn_observer_camera defaults true)");

    assert!(
        app.world()
            .get::<lunco_celestial::ReferenceFrame>(parent)
            .is_some_and(|frame| {
                *frame
                    == lunco_celestial::ReferenceFrame::EclipticJ2000 {
                        center: lunco_celestial::ephemeris_id::EARTH,
                    }
            }),
        "the Observer Camera must be parented to Earth's EclipticJ2000 frame"
    );
    assert!(
        app.world()
            .get::<lunco_celestial::EarthRoot>(parent)
            .is_none(),
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
        "the EclipticJ2000 frame must never rotate — the orbit view is star-fixed \
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

#[test]
fn each_builtin_orbit_target_has_one_colocated_star_fixed_grid() {
    let mut app = celestial_test_app();
    app.insert_resource(EphemerisResource {
        provider: Arc::new(StubEphemeris),
    });
    app.update();
    app.update();

    let orbit_frames: Vec<(Entity, i32, CellCoord, Transform)> = {
        let mut query = app.world_mut().query::<(
            Entity,
            &lunco_celestial::ReferenceFrame,
            &CellCoord,
            &Transform,
        )>();
        query
            .iter(app.world())
            .filter_map(|(entity, frame, cell, transform)| match *frame {
                lunco_celestial::ReferenceFrame::EclipticJ2000 { center } => {
                    Some((entity, center, *cell, *transform))
                }
                lunco_celestial::ReferenceFrame::World
                | lunco_celestial::ReferenceFrame::BodyFixed { .. } => None,
            })
            .collect()
    };

    for body_id in [
        lunco_celestial::ephemeris_id::SUN,
        lunco_celestial::ephemeris_id::EARTH,
        lunco_celestial::ephemeris_id::MOON,
    ] {
        let matches: Vec<_> = orbit_frames
            .iter()
            .filter(|(_, id, _, _)| *id == body_id)
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "body {body_id} must own exactly one unambiguous EclipticJ2000 frame"
        );
        assert!(
            app.world().get::<Grid>(matches[0].0).is_some(),
            "EclipticJ2000 frame for body {body_id} must be a BigSpace Grid"
        );
    }

    for body_id in [
        lunco_celestial::ephemeris_id::EARTH,
        lunco_celestial::ephemeris_id::MOON,
    ] {
        let (_, _, orbit_cell, orbit_transform) = orbit_frames
            .iter()
            .find(|(_, id, _, _)| *id == body_id)
            .unwrap();
        assert!(
            orbit_transform.rotation.angle_between(Quat::IDENTITY) < 1e-6,
            "body {body_id} orbit frame must remain star-fixed"
        );

        let (body_cell, body_transform) = {
            let mut query =
                app.world_mut()
                    .query::<(&lunco_celestial::ReferenceFrame, &CellCoord, &Transform)>();
            query
                .iter(app.world())
                .find(|(frame, _, _)| {
                    **frame == lunco_celestial::ReferenceFrame::BodyFixed { body: body_id }
                })
                .map(|(_, cell, transform)| (*cell, *transform))
                .expect("body-fixed frame must exist")
        };
        assert_eq!(*orbit_cell, body_cell);
        assert!(
            orbit_transform
                .translation
                .abs_diff_eq(body_transform.translation, 1e-6),
            "body {body_id} inertial and body-fixed grids must be co-located"
        );
    }
}

#[test]
fn trajectories_mount_only_in_their_declared_frame_class() {
    let mut app = celestial_test_app();
    app.insert_resource(EphemerisResource {
        provider: Arc::new(StubEphemeris),
    });
    app.update();
    app.update();

    let moon = lunco_celestial::ephemeris_id::MOON;
    let earth = lunco_celestial::ephemeris_id::EARTH;
    let body_fixed = app
        .world_mut()
        .spawn((
            lunco_celestial::TrajectoryView {
                tracked_id: earth,
                reference_id: moon,
                frame: lunco_celestial::TrajectoryFrame::BodyFixed,
                ..Default::default()
            },
            lunco_celestial::TrajectoryPath::default(),
            Transform::default(),
            GlobalTransform::default(),
        ))
        .id();
    let inertial = app
        .world_mut()
        .spawn((
            lunco_celestial::TrajectoryView {
                tracked_id: -10_001,
                reference_id: moon,
                frame: lunco_celestial::TrajectoryFrame::Inertial,
                ..Default::default()
            },
            lunco_celestial::TrajectoryPath::default(),
            Transform::default(),
            GlobalTransform::default(),
        ))
        .id();

    app.update();
    app.update();

    let fixed_parent = app.world().get::<ChildOf>(body_fixed).unwrap().parent();
    let fixed_frame = app
        .world()
        .get::<lunco_celestial::ReferenceFrame>(fixed_parent)
        .expect("a body-fixed trajectory must parent to a body-fixed frame Grid");
    assert_eq!(
        *fixed_frame,
        lunco_celestial::ReferenceFrame::BodyFixed { body: moon }
    );

    let inertial_parent = app.world().get::<ChildOf>(inertial).unwrap().parent();
    let inertial_frame = app
        .world()
        .get::<lunco_celestial::ReferenceFrame>(inertial_parent)
        .expect("an inertial trajectory must parent to an inertial frame Grid");
    assert_eq!(
        *inertial_frame,
        lunco_celestial::ReferenceFrame::EclipticJ2000 { center: moon }
    );
}

#[test]
fn spacecraft_mount_only_in_their_declared_inertial_reference_grid() {
    let mut app = celestial_test_app();
    app.insert_resource(EphemerisResource {
        provider: Arc::new(StubEphemeris),
    });
    app.update();
    app.update();

    let moon = lunco_celestial::ephemeris_id::MOON;
    let spacecraft = app
        .world_mut()
        .spawn((
            lunco_core::Spacecraft {
                name: "Frame probe".into(),
                ephemeris_id: -10_001,
                reference_id: moon,
                user_visible: true,
                ..Default::default()
            },
            Transform::from_scale(Vec3::splat(2.0)),
            GlobalTransform::default(),
            Visibility::default(),
        ))
        .id();

    app.update();

    let parent = app
        .world()
        .get::<ChildOf>(spacecraft)
        .expect("spacecraft must be mounted atomically in its reference grid")
        .parent();
    let inertial = app
        .world()
        .get::<lunco_celestial::ReferenceFrame>(parent)
        .expect("spacecraft state vectors are inertial and require an inertial frame Grid");
    assert_eq!(
        *inertial,
        lunco_celestial::ReferenceFrame::EclipticJ2000 { center: moon }
    );
    assert!(app.world().get::<Grid>(parent).is_some());
    assert!(app.world().get::<CellCoord>(spacecraft).is_some());
    assert_eq!(
        app.world().get::<Transform>(spacecraft).unwrap().scale,
        Vec3::splat(2.0),
        "atomic frame migration must preserve the authored marker scale"
    );
}

#[test]
fn rendered_and_analytical_orbit_use_the_same_typed_frame_transform() {
    let mut app = celestial_test_app();
    app.insert_resource(EphemerisResource {
        provider: Arc::new(StubEphemeris),
    });
    let earth = lunco_celestial::ephemeris_id::EARTH;
    let orbit = lunco_celestial::KeplerOrbit {
        body: earth,
        elements: lunco_celestial::KeplerianElements {
            semi_major_axis_m: 7_000_000.0,
            inclination_deg: 51.6,
            raan_deg: 37.0,
            ..Default::default()
        },
    };
    let satellite = app
        .world_mut()
        .spawn((orbit, Transform::default(), GlobalTransform::default()))
        .id();

    for _ in 0..4 {
        app.update();
    }

    let jd = app.world().resource::<WorldTime>().epoch_jd;
    let registry = app
        .world()
        .resource::<lunco_celestial::CelestialBodyRegistry>();
    let ephemeris = app.world().resource::<EphemerisResource>();
    let descriptor = registry.get(earth).unwrap();
    let body_inertial =
        lunco_celestial::frames::Pos::<lunco_celestial::frames::BodyInertial>::at_body(
            earth,
            orbit.elements.position_bevy_m(descriptor.gm, jd),
        );
    let expected =
        lunco_celestial::transform::FrameTree::new(jd, registry, ephemeris.provider.as_ref())
            .body_inertial_to_solar(body_inertial)
            .unwrap()
            .raw();
    let tracked = app
        .world()
        .get::<lunco_celestial::SolarFramePose>(satellite)
        .expect("KeplerOrbit must produce a SolarFramePose");

    assert!(
        tracked.pos.abs_diff_eq(expected, 1e-6),
        "analytical orbit pose bypassed the body-inertial to solar transform: expected={expected:?}, got={:?}",
        tracked.pos
    );
}

/// Scene reload must tear the sky down **completely** — by architecture, not a
/// maintained despawn list. The replacement may be body-less or may declare a
/// different sky; either way, the outgoing derived entities and active physics
/// grid must be cleared at the scene boundary before the replacement integrates.
#[test]
fn scene_reload_without_bodies_tears_the_whole_sky_down() {
    let mut app = celestial_test_app(); // declares Sun/Earth/Moon
    app.insert_resource(EphemerisResource {
        provider: Arc::new(StubEphemeris),
    });
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

    // A site scene selects a celestial surface Grid as Avian's frame. Reproduce
    // that lifecycle state explicitly: teardown must not leave the resource
    // pointing at an entity it is about to despawn.
    let surface_frame = app
        .world_mut()
        .query_filtered::<Entity, With<lunco_celestial::MoonSurfaceRoot>>()
        .single(app.world())
        .expect("Moon surface frame should exist");
    app.world_mut()
        .insert_resource(lunco_core::ActivePhysicsFrame(surface_frame));

    // The shared scene boundary runs while the outgoing declarations still exist;
    // this is the ordering used by LoadScene/ClearScene.
    lunco_core::scene_lifecycle::run_scene_teardown(app.world_mut());

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

    // The teardown already cleared everything before the old declaration entities
    // were removed.
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
    let persistent_root = app
        .world_mut()
        .query_filtered::<Entity, With<lunco_core::WorldRoot>>()
        .single(app.world())
        .expect("the persistent world shell must survive scene teardown");
    assert_eq!(
        app.world().resource::<lunco_core::ActivePhysicsFrame>().0,
        persistent_root,
        "scene teardown must restore Avian's frame before despawning the celestial surface Grid"
    );

    // …and re-declaring bodies rebuilds it (the idempotent gate, not a spent latch).
    for naif in [
        lunco_celestial::ephemeris_id::SUN,
        lunco_celestial::ephemeris_id::EARTH,
        lunco_celestial::ephemeris_id::MOON,
    ] {
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
    app.add_plugins(bevy::input::InputPlugin);
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
    for naif in [
        lunco_celestial::ephemeris_id::SUN,
        lunco_celestial::ephemeris_id::EARTH,
        lunco_celestial::ephemeris_id::MOON,
    ] {
        app.world_mut()
            .spawn(lunco_celestial::CelestialBodyDecl { naif });
    }
    // Override the NoOp provider (installed by CelestialPlugin) with one whose
    // output depends on the epoch, so the clock seek below actually repositions
    // Earth's grid via `ephemeris_update_system`.
    app.insert_resource(EphemerisResource {
        provider: Arc::new(StubEphemeris),
    });

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
    let mut query = app
        .world_mut()
        .query::<(&lunco_celestial::EarthRoot, &CellCoord, &Transform)>();
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
    let mut grid_q = app
        .world_mut()
        .query::<(&lunco_celestial::EMBRoot, &big_space::prelude::Grid)>();
    let edge = grid_q
        .iter(app.world())
        .next()
        .expect("No EMBRoot grid found")
        .1
        .cell_edge_length() as f64;

    let mut query = app
        .world_mut()
        .query::<(&lunco_celestial::EarthRoot, &CellCoord, &Transform)>();
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
                lunco_celestial::ephemeris_id::MOON => {
                    lunco_celestial::frames::EclipticAu::new(bevy::math::DVec3::new(1.0, 0.0, 0.0))
                }
                _ => lunco_celestial::frames::EclipticAu::ZERO,
            })
        }
    }

    let mut app = celestial_test_app();
    app.insert_resource(EphemerisResource {
        provider: Arc::new(SunAndMoon),
    });

    // The sandbox's own light: the brightest `DirectionalLight`, aimed by hand.
    let authored = Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.7, -0.9, 0.0));
    let light = app
        .world_mut()
        .spawn((
            DirectionalLight {
                illuminance: 128_000.0,
                ..default()
            },
            authored,
        ))
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
            ephem
                .provider
                .global_position(lunco_celestial::ephemeris_id::SUN, 0.0)
                .unwrap(),
            ephem
                .provider
                .global_position(lunco_celestial::ephemeris_id::MOON, 0.0)
                .unwrap(),
        )
        .is_some(),
        "the stub ephemeris is degenerate — this test would pass without steering ever \
         being attempted"
    );

    assert_eq!(
        app.world()
            .resource::<lunco_celestial::SunDirectionWorld>()
            .0,
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

/// **The celestial takeover must not add a SECOND sun to a scene that authored one.**
///
/// The takeover used to spawn its own marked "fallback" sun. `lunco-usd-bevy` retired
/// such lights when an authored one appeared, but that retirement was edge-triggered on
/// the authored light's `Add` — and the celestial hierarchy is enabled by the site
/// anchor the scene load itself detects, so it runs AFTER that edge has passed. Its
/// unconditional spawn therefore re-created the duplicate the retirement existed to
/// prevent.
///
/// The spawn is now gone entirely: the engine sun is composed from
/// `lunco://lighting/sun.usda` as the weakest opinion on the scene's own `Sun` prim,
/// so there is one prim and nothing to race. This test pins that the takeover adds
/// no light of its own — the regression it guards is someone reintroducing a
/// "helpful" default sun here.
///
/// Two suns is not merely wasteful. `update_sun_light_system` used to steer the
/// BRIGHTEST `DirectionalLight`, so the 128 klx fallback took the aim and the
/// scene's authored sun stayed frozen at its authored `xformOp:rotateXYZ` — the
/// summer-space-school twin lighting and shadowing Hadley from a direction the
/// ephemeris never sanctioned ("the DistantLight does not follow the sun"). It
/// now picks structurally instead, but a second unfilterable top-level
/// `DistantLight` would still make "which one?" unanswerable — which is why the
/// spawn had to go rather than the tiebreak get smarter.
///
/// Asserted on the light COUNT and on which entity survives, because "the authored
/// one is aimed correctly" passes for the wrong reason as soon as the authored sun
/// happens to be the brighter of the two.
#[test]
fn the_celestial_takeover_spawns_no_sun_of_its_own() {
    let mut app = celestial_test_app();
    app.insert_resource(EphemerisResource {
        provider: Arc::new(StubEphemeris),
    });

    // The scene's own sun, present BEFORE the celestial takeover — the real
    // ordering, where the site anchor that enables the hierarchy is detected
    // during the same scene load that instantiated this light.
    let authored = app
        .world_mut()
        .spawn((
            DirectionalLight {
                illuminance: 10_000.0, // dimmer than the 128 klx fallback, as the twin authors it
                ..default()
            },
            Transform::default(),
        ))
        .id();

    for _ in 0..8 {
        app.update();
    }

    // CONTROL: the takeover really ran, so a missing fallback means "suppressed",
    // not "the hierarchy never came up".
    let mut q_grid = app
        .world_mut()
        .query_filtered::<(), With<lunco_celestial::SiteAlignGrid>>();
    assert_eq!(
        q_grid.iter(app.world()).count(),
        1,
        "the celestial hierarchy must have been built — otherwise this test proves nothing"
    );

    let mut q_lights = app.world_mut().query::<(Entity, &DirectionalLight)>();
    let lights: Vec<Entity> = q_lights.iter(app.world()).map(|(e, _)| e).collect();
    assert_eq!(
        lights,
        vec![authored],
        "the scene authored its own sun, so the celestial takeover must not spawn a \
         fallback beside it — two DirectionalLights make `update_sun_light_system`'s \
         brightest-wins pick steer the wrong one"
    );
}

/// A connectivity endpoint is usually a deep child of its physical station. Its
/// own prim has no geodetic anchor; the station ancestor does. Resolving only the
/// endpoint itself places an Earth feed in the lunar site frame, collapsing the
/// Earth link onto the rover and sending the rendered beam sideways.
#[test]
fn descendant_link_endpoint_uses_nearest_geodetic_anchor() {
    let mut app = celestial_test_app();
    let epoch_jd = 2_451_545.0;
    app.insert_resource(EphemerisResource {
        provider: Arc::new(StubEphemeris),
    });
    app.insert_resource(lunco_celestial::registry::CelestialBodyRegistry::default_system());
    app.insert_resource(WorldTime {
        epoch_jd,
        ..Default::default()
    });

    let site = app
        .world_mut()
        .spawn((
            Transform::IDENTITY,
            lunco_celestial::geo::SiteAnchor,
            lunco_celestial::geo::GeodeticAnchor {
                body: lunco_celestial::ephemeris_id::MOON,
                geodetic: lunco_celestial::geo::Geodetic::new(-86.0, 3.0, 0.3),
            },
        ))
        .id();
    let station = app
        .world_mut()
        .spawn((
            Transform::IDENTITY,
            ChildOf(site),
            lunco_celestial::geo::GeodeticAnchor {
                body: lunco_celestial::ephemeris_id::EARTH,
                geodetic: lunco_celestial::geo::Geodetic::new(40.4, -4.2, 837.0),
            },
        ))
        .id();
    let endpoint = app
        .world_mut()
        .spawn((
            Transform::from_xyz(0.0, 27.0, 0.0),
            ChildOf(station),
            lunco_celestial::link::LinkNode {
                class: Some("earth".into()),
                ..Default::default()
            },
        ))
        .id();

    // The first frame creates the hierarchy and the pose components; the second
    // observes the components after the deferred inserts have flushed.
    app.update();
    app.update();

    let station_pose = *app
        .world()
        .get::<lunco_celestial::pose::SolarFramePose>(station)
        .expect("the anchored station must receive a solar pose");
    let endpoint_pose = *app
        .world()
        .get::<lunco_celestial::pose::SolarFramePose>(endpoint)
        .expect("the descendant link endpoint must receive a solar pose");

    assert_eq!(station_pose.body(), lunco_celestial::ephemeris_id::EARTH);
    assert_eq!(endpoint_pose.body(), lunco_celestial::ephemeris_id::EARTH);
    assert!(
        (endpoint_pose.pos - station_pose.pos).length() < 28.0,
        "the feed should remain near its Earth station, not at the lunar site: {:?} vs {:?}",
        endpoint_pose.pos,
        station_pose.pos
    );
}
