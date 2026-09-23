use bevy::prelude::*;
use big_space::prelude::*;
use lunco_celestial::{EphemerisProvider, EphemerisResource};
use lunco_celestial_spatial::CelestialPlugin;
use lunco_time::WorldTime;
use std::sync::Arc;

/// Test ephemeris that returns an **epoch-dependent** position, so advancing the
/// clock provably moves a body. The test installs a real provider explicitly;
/// without one, ephemeris-driven motion is unavailable rather than synthesized.
/// The scale (AU per day) is large enough that a 10-day step shifts
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

    fn maximum_angular_rate_rad_per_day(&self) -> f64 {
        0.0
    }

}

/// Build the headless celestial app the integration tests share. These tests
/// exercise the ECS/spatial mechanisms without loading visual assets or a GPU.
///
/// Note the `CelestialBodyDecl` spawns: celestial content is **opt-in per scene**
/// (doc 19 §11e). A scene declares its bodies in USD (`LunCoCelestialBodyAPI` →
/// `CelestialBodyDecl`), and nothing celestial — hierarchy, globes, orbit views,
/// ephemeris — exists without them. The fixture supplies those declarations
/// directly without loading scene assets.
fn celestial_test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::input::InputPlugin);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.init_resource::<Assets<Mesh>>();
    app.init_asset::<Image>();
    install_test_input_bindings(&mut app);
    app.add_plugins(CelestialPlugin);
    // Production loads the authored quality profile before celestial hierarchy
    // creation. This mechanics fixture supplies the smallest valid profile so
    // it exercises celestial propagation instead of render-policy loading.
    app.world_mut()
        .resource_mut::<lunco_render::RenderingQualitySettings>()
        .apply_profile(celestial_test_quality_profile());
    // The scene asks for a sky: Sun, Earth, Moon.
    declare_test_bodies(app.world_mut());
    app
}

fn celestial_test_quality_profile() -> lunco_render::RenderQualityProfile {
    let mut profile = lunco_render::RenderQualityProfile::default();
    profile.directional_shadow_map_size = 16;
    profile.point_shadow_map_size = 16;
    profile.directional_cascades = 1;
    profile.max_directional_shadow_casters = 1;
    profile.max_point_shadow_casters = 1;
    profile.max_spot_shadow_casters = 1;
    profile.shadow_budget_bytes = 1_000_000;
    profile.horizon_shadow_cache_sun_threshold_deg = 1.0;
    profile.horizon_march_steps = 1;
    profile.horizon_cache_samples_per_axis = 1;
    profile.shadow_first_cascade_far_bound = 1.0;
    profile.shadow_maximum_distance = 2.0;
    profile.render_failure_quiet_period_secs = 1.0;
    profile.render_failure_give_up_after_secs = 2.0;
    profile.distant_light_default_illuminance = 1.0;
    profile.local_light_default_intensity = 1.0;
    profile.rect_light_default_intensity = 1.0;
    profile.local_light_default_range = 1.0;
    profile.dome_cubemap_face_size = 1;
    profile.primitive_sphere_longitudes = 3;
    profile.primitive_sphere_latitudes = 2;
    profile.primitive_radial_segments = 3;
    profile.primitive_capsule_longitudes = 3;
    profile.primitive_capsule_latitudes = 2;
    profile.terrain_mesh_cache_bytes = 1;
    profile.terrain_derived_map_resolution = 1;
    profile.terrain_derived_ao_directions = 1;
    profile.terrain_derived_ao_steps = 1;
    profile.terrain_derived_ao_radius_fraction = 0.1;
    profile.terrain_derived_roughness_saturation_radians = 0.1;
    profile.terrain_derived_texture_anisotropy = 1;
    profile.terrain_rock_max_instances = 1;
    profile.terrain_rock_mesh_buckets = 2;
    profile.terrain_rock_mesh_cube_count = 1;
    profile.terrain_rock_lod_fade_distance = 1.0;
    profile.terrain_lod_tile_resolution = 3;
    profile.terrain_lod_cinematic_resolution = 3;
    profile.terrain_lod_pixel_error = 0.1;
    profile.terrain_lod_max_depth = 1;
    profile.terrain_lod_probe_resolution = 3;
    profile.terrain_lod_bakes_per_frame = 1;
    profile.terrain_lod_max_inflight_bakes = 1;
    profile.terrain_lod_tile_budget = 1;
    profile.terrain_lod_cover_edits_per_frame = 1;
    profile.terrain_lod_hysteresis_ratio = 1.1;
    profile.nurbs_surface_samples_per_control_span = 1;
    profile.nurbs_surface_minimum_subdivisions = 1;
    profile.nurbs_surface_maximum_subdivisions = 1;
    profile.nurbs_trim_curve_samples = 1;
    profile.nurbs_trim_minimum_subdivisions = 1;
    profile.nurbs_trim_maximum_subdivisions = 1;
    profile.curve_samples_per_segment = 1;
    profile.curve_radial_segments = 3;
    profile
}

fn install_test_input_bindings(app: &mut App) {
    app.add_plugins(lunco_input_core::InputBindingsPlugin);
    app.insert_resource(lunco_input_core::InputBindingsSettings {
        look_button: "Right".to_string(),
        ..Default::default()
    });
}

/// Headless declarations still satisfy the hierarchy's authored-look contract.
/// An empty look keeps these mechanics tests independent of visual asset paths.
fn declare_test_bodies(world: &mut World) {
    for naif in [
        lunco_celestial::ephemeris_id::SUN,
        lunco_celestial::ephemeris_id::EARTH,
        lunco_celestial::ephemeris_id::MOON,
    ] {
        let mut declaration = world.spawn(lunco_celestial_spatial_core::CelestialBodyDecl { naif });
        if naif == lunco_celestial::ephemeris_id::EARTH
            || naif == lunco_celestial::ephemeris_id::MOON
        {
            declaration.insert(lunco_materials::ShaderLook::default());
        }
    }
}

/// **The `SolarSystemRoot` invariant: exactly one bearer, and it is the Grid.**
///
/// `SolarSystemRoot` is the one semantic marker for the inertial solar Grid.
/// Site mounting is represented by the site's own nested Grid; there is no
/// parallel alignment entity whose rotation can diverge from the body-fixed
/// surface frame.
#[test]
fn solar_system_root_is_singular() {
    let mut app = celestial_test_app();
    app.update();

    let bearers: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<lunco_celestial_spatial_core::SolarSystemRoot>>()
        .iter(app.world())
        .collect();

    assert_eq!(
        bearers.len(),
        1,
        "`SolarSystemRoot` must name exactly one entity — found {}",
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

/// A site is attached to the body's native surface Grid after the deferred
/// celestial hierarchy exists. The handoff must not create a second alignment
/// frame or re-pose the inertial solar Grid.
#[test]
fn site_anchor_mounts_under_the_body_surface_grid() {
    let mut app = celestial_test_app();
    app.insert_resource(EphemerisResource {
        provider: Arc::new(StubEphemeris),
    });
    app.update();

    let world_grid = app
        .world_mut()
        .query_filtered::<Entity, With<lunco_spatial::WorldGrid>>()
        .iter(app.world())
        .next()
        .expect("the canonical WorldGrid exists");
    let site = app
        .world_mut()
        .spawn((
            lunco_spatial::GridAnchor,
            lunco_celestial::geo::SiteAnchor,
            lunco_celestial::geo::GeodeticAnchor {
                body: lunco_celestial::ephemeris_id::MOON,
                geodetic: lunco_celestial::geo::Geodetic::new(26.13, 3.63, 0.3),
            },
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(world_grid),
        ))
        .id();

    app.update();
    let parent = app
        .world()
        .get::<ChildOf>(site)
        .expect("site parent")
        .parent();
    assert!(
        app.world()
            .get::<lunco_celestial_spatial::MoonSurfaceRoot>(parent)
            .is_some(),
        "the site must be a child of the Moon surface frame"
    );
    assert!(
        app.world().get::<Grid>(site).is_some(),
        "the site owns its nested Grid"
    );
    let solar_roots = {
        let world = app.world_mut();
        let mut q =
            world.query_filtered::<(), With<lunco_celestial_spatial_core::SolarSystemRoot>>();
        q.iter(world).count()
    };
    assert_eq!(
        solar_roots, 1,
        "site mounting must not duplicate or re-purpose the inertial solar Grid"
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
        .query_filtered::<(&ChildOf, &Name), With<lunco_embodiment_core::roles::Embodiment>>();
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
            .get::<lunco_celestial_spatial::EarthRoot>(parent)
            .is_none(),
        "…and NOT to the Earth Grid, which rotates once per sidereal day"
    );

    let earth_rot_of = |app: &mut App| -> Quat {
        let mut q = app
            .world_mut()
            .query_filtered::<&Transform, With<lunco_celestial_spatial::EarthRoot>>();
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
        .query_filtered::<(&CellCoord, &Transform), With<lunco_celestial_spatial::EarthRoot>>();
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
        .get::<lunco_celestial_spatial::SolarFramePose>(satellite)
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
            .query_filtered::<(), With<lunco_celestial_spatial::CelestialDerived>>()
            .iter(app.world())
            .count()
    };
    assert!(count_derived(&mut app) > 0, "the sky should have spawned");

    // A site scene selects a celestial surface Grid as Avian's frame. Reproduce
    // that lifecycle state explicitly: teardown must not leave the resource
    // pointing at an entity it is about to despawn.
    let surface_frame = app
        .world_mut()
        .query_filtered::<Entity, With<lunco_celestial_spatial::MoonSurfaceRoot>>()
        .single(app.world())
        .expect("Moon surface frame should exist");
    app.world_mut()
        .insert_resource(lunco_spatial::ActivePhysicsFrame(surface_frame));

    // Reload into a scene WITHOUT bodies. The scene owner runs the explicit
    // teardown transaction while the outgoing declarations still exist, then
    // reclaims the USD-projected declaration entities.
    lunco_core::run_scene_teardown(app.world_mut());
    let decls: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<lunco_celestial_spatial_core::CelestialBodyDecl>>()
        .iter(app.world())
        .collect();
    for e in decls {
        app.world_mut().despawn(e);
    }

    // Deferred teardown writes and entity reclamation settle normally.
    app.update();
    app.update();
    assert_eq!(
        count_derived(&mut app),
        0,
        "no celestial-derived entity may survive a reload into a body-less scene"
    );
    assert!(
        app.world_mut()
            .query_filtered::<(), With<lunco_celestial_spatial_core::SolarSystemRoot>>()
            .iter(app.world())
            .next()
            .is_none(),
        "the hierarchy root must be gone"
    );
    let persistent_grid = app
        .world_mut()
        .query_filtered::<Entity, With<lunco_spatial::WorldGrid>>()
        .single(app.world())
        .expect("the persistent world grid must survive scene teardown");
    assert_eq!(
        app.world()
            .resource::<lunco_spatial::ActivePhysicsFrame>()
            .0,
        persistent_grid,
        "scene teardown must restore Avian's frame before despawning the celestial surface Grid"
    );

    // …and re-declaring bodies rebuilds it (the idempotent gate, not a spent latch).
    declare_test_bodies(app.world_mut());
    app.update();
    app.update();
    assert!(
        count_derived(&mut app) > 0,
        "re-declaring bodies must rebuild the sky — teardown must not be a one-way latch"
    );
}

#[test]
fn test_celestial_startup_and_movement() {
    let mut app = celestial_test_app();
    // Install the provider whose output depends on the epoch, so the clock seek
    // below actually repositions Earth's grid via `ephemeris_update_system`.
    app.insert_resource(EphemerisResource {
        provider: Arc::new(StubEphemeris),
    });

    // Ensure startup systems run
    app.update();

    let epoch_before = app.world().resource::<WorldTime>().epoch_jd;

    // 1. Verify Sun and Earth exist.
    //
    // `EarthRoot` is the Earth *grid* (a frame) inside the EMB grid. Both its
    // cell and local Transform move as it orbits; compose through BigSpace so
    // the assertion covers cell-boundary movement too.
    let mut query = app
        .world_mut()
        .query::<(&lunco_celestial_spatial::EarthRoot, &CellCoord, &Transform)>();
    let earth = query.iter(app.world()).next().expect("No EarthRoot found");
    let earth_pose_1 = (*earth.1, *earth.2);

    // 2. Advance the clock by 10 days. The epoch is a *derived* view
    //    (`WorldTime.epoch_jd`, published after the fixed loop), so seek via the
    //    authority — re-anchor the `MissionClock` epoch. The next time projection
    //    updates `WorldTime.epoch_jd` and the ephemeris follows.
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
        .query::<(&lunco_celestial_spatial::EMBRoot, &big_space::prelude::Grid)>();
    let emb_grid = grid_q
        .iter(app.world())
        .next()
        .expect("No EMBRoot grid found")
        .1
        .clone();

    let mut query = app
        .world_mut()
        .query::<(&lunco_celestial_spatial::EarthRoot, &CellCoord, &Transform)>();
    let earth = query.iter(app.world()).next().expect("No EarthRoot found");
    let earth_pose_2 = (*earth.1, *earth.2);
    let moved = (emb_grid.grid_position_double(&earth_pose_2.0, &earth_pose_2.1)
        - emb_grid.grid_position_double(&earth_pose_1.0, &earth_pose_1.1))
    .length();

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
/// `solar_system.usda`) has no local ENU site frame. Gating
/// `update_sun_light_system` on a guessed identity mapping would aim the scene's
/// brightest `DirectionalLight` along raw ecliptic axes and can light the ground
/// from below.
///
/// With no anchor, no sun steering may happen AT ALL — asserted on
/// semantic `SunState`, the system's own
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

        fn maximum_angular_rate_rad_per_day(&self) -> f64 {
            0.0
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
        .query_filtered::<(), With<lunco_celestial_spatial_core::SolarSystemRoot>>();
    assert_eq!(
        q.iter(app.world()).count(),
        1,
        "the solar hierarchy must exist — otherwise this test proves nothing about the gate"
    );

    // CONTROL for the ephemeris: it must be able to produce a direction, or the
    // system early-returns and the gate is never reached.
    let ephem = app.world().resource::<EphemerisResource>();
    assert!(
        lunco_celestial_spatial::sun_emit_direction(
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
            .resource::<lunco_environment::SunState>()
            .direction_to_sun,
        None,
        "an unanchored scene has no known ecliptic→world rotation, so the sun must not be \
         steered at all — a direction here is the raw ecliptic vector aimed along the horizon"
    );
    let after = *app.world().entity(light).get::<Transform>().unwrap();
    assert_eq!(
        after.rotation, authored.rotation,
        "an unanchored scene's authored sun must not be re-aimed by the ephemeris"
    );
}

/// A scene-owned directional light remains the only light when celestial frames
/// are created. The plugin owns frame mechanics and must not invent scene content.
#[test]
fn the_celestial_takeover_spawns_no_sun_of_its_own() {
    let mut app = celestial_test_app();
    app.insert_resource(EphemerisResource {
        provider: Arc::new(StubEphemeris),
    });

    // A pre-existing scene-owned light must remain the sole light.
    let authored = app
        .world_mut()
        .spawn((
            DirectionalLight {
                illuminance: 10_000.0,
                ..default()
            },
            Transform::default(),
        ))
        .id();

    for _ in 0..8 {
        app.update();
    }

    // CONTROL: the takeover really ran, so the absence of a second sun means "suppressed",
    // not "the hierarchy never came up".
    let mut q_grid = app
        .world_mut()
        .query_filtered::<(), With<lunco_celestial_spatial_core::SolarSystemRoot>>();
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
         second sun beside it — two DirectionalLights would violate the structural \
         one-sun contract"
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
            lunco_celestial_spatial_core::LinkNode {
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
        .get::<lunco_celestial_spatial::pose::SolarFramePose>(station)
        .expect("the anchored station must receive a solar pose");
    let endpoint_pose = *app
        .world()
        .get::<lunco_celestial_spatial::pose::SolarFramePose>(endpoint)
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
